#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod ai_hook;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use serde_json::{json, Value};

pub const MAX_FRAME_BYTES: usize = 256 * 1024;

const IPC_TIMEOUT: Duration = Duration::from_secs(10);

const MAX_RESPONSE_LEN: u64 = MAX_FRAME_BYTES as u64;

pub trait IpcTransport {
    fn call(&self, method: &str, params: Value) -> Result<Value, String>;
}

pub struct IpcClient {
    socket: PathBuf,
    next_id: AtomicU64,
}

impl IpcClient {
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            next_id: AtomicU64::new(1),
        }
    }
}

impl IpcTransport for IpcClient {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = build_request(id, method, params);
        let line = send_and_receive(&self.socket, &request).map_err(|e| {
            format!(
                "paneflow IPC unreachable at {} ({e}); is Paneflow running?",
                self.socket.display()
            )
        })?;
        parse_response(&line)
    }
}

pub(crate) fn build_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

pub(crate) fn parse_response(line: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("invalid JSON-RPC response from paneflow: {e}"))?;
    if let Some(message) = jsonrpc_error_message_from_value(&value) {
        return Err(message);
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "paneflow response missing both `result` and `error`".to_string())
}

pub fn jsonrpc_error_message(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    jsonrpc_error_message_from_value(&value)
}

fn jsonrpc_error_message_from_value(value: &Value) -> Option<String> {
    let err = value.get("error")?;
    let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    Some(format!("paneflow error {code}: {message}"))
}

#[cfg(any(not(windows), test))]
fn tolerate_unsupported(r: io::Result<()>) -> io::Result<()> {
    match r {
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Ok(()),
        other => other,
    }
}

fn send_and_receive(socket: &Path, request: &Value) -> io::Result<String> {
    let mut stream = connect_request_stream(socket)?;
    #[cfg(not(windows))]
    {
        tolerate_unsupported(stream.set_recv_timeout(Some(IPC_TIMEOUT)))?;
        tolerate_unsupported(stream.set_send_timeout(Some(IPC_TIMEOUT)))?;
    }

    let mut payload =
        serde_json::to_vec(request).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    payload.push(b'\n');

    #[cfg(windows)]
    {
        write_all_with_deadline(&mut stream, &payload, IPC_TIMEOUT)?;
        read_line_with_deadline(&mut stream, IPC_TIMEOUT)
    }

    #[cfg(not(windows))]
    {
        stream.write_all(&payload)?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        match reader.by_ref().take(MAX_RESPONSE_LEN).read_line(&mut line) {
            Ok(n) if n as u64 >= MAX_RESPONSE_LEN && !line.ends_with('\n') => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "paneflow response exceeded the size cap",
            )),
            Ok(_) => Ok(line),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "paneflow did not respond within 10s",
                ))
            }
            Err(e) => Err(e),
        }
    }
}

fn connect_request_stream(socket: &Path) -> io::Result<Stream> {
    let name = socket.to_fs_name::<GenericFilePath>()?;
    Stream::connect(name)
}

#[cfg(windows)]
mod windows_pipe {
    use super::{io, Duration, Stream};
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_NOT_CONNECTED, HANDLE,
        WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    struct Event(HANDLE);

    impl Event {
        fn new() -> io::Result<Self> {
            let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if handle.is_null() {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(handle))
            }
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn pipe_handle(stream: &Stream) -> HANDLE {
        let Stream::NamedPipe(pipe) = stream;
        pipe.as_handle().as_raw_handle() as HANDLE
    }

    fn closed_pipe(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32
        )
    }

    fn cancel_and_reap(handle: HANDLE, overlapped: &OVERLAPPED) {
        let _ = unsafe { CancelIoEx(handle, overlapped) };
        let mut transferred = 0;
        let _ = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
    }

    fn wait_for_completion(
        handle: HANDLE,
        overlapped: &OVERLAPPED,
        timeout: Duration,
    ) -> io::Result<()> {
        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { WaitForSingleObject(overlapped.hEvent, timeout_ms) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => {
                cancel_and_reap(handle, overlapped);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "paneflow named-pipe I/O timed out",
                ))
            }
            WAIT_FAILED => {
                let error = io::Error::last_os_error();
                cancel_and_reap(handle, overlapped);
                Err(error)
            }
            other => {
                cancel_and_reap(handle, overlapped);
                Err(io::Error::other(format!(
                    "unexpected WaitForSingleObject result {other}"
                )))
            }
        }
    }

    pub(super) fn write_all(
        stream: &Stream,
        mut payload: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        let handle = pipe_handle(stream);

        while !payload.is_empty() {
            let event = Event::new()?;
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = event.0;
            let wanted = payload.len().min(u32::MAX as usize) as u32;
            let started = unsafe {
                WriteFile(
                    handle,
                    payload.as_ptr(),
                    wanted,
                    std::ptr::null_mut(),
                    &mut overlapped,
                )
            };
            if started == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                    return Err(error);
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                wait_for_completion(handle, &overlapped, remaining)?;
            }

            let mut transferred = 0;
            if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if transferred == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "paneflow IPC write made no progress",
                ));
            }
            payload = &payload[transferred as usize..];
        }
        Ok(())
    }

    pub(super) fn read_some(
        stream: &Stream,
        buffer: &mut [u8],
        timeout: Duration,
    ) -> io::Result<usize> {
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "paneflow named-pipe I/O timed out",
            ));
        }

        let handle = pipe_handle(stream);
        let event = Event::new()?;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        let wanted = buffer.len().min(u32::MAX as usize) as u32;
        let started = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                wanted,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return if closed_pipe(&error) {
                    Ok(0)
                } else {
                    Err(error)
                };
            }
            wait_for_completion(handle, &overlapped, timeout)?;
        }

        let mut transferred = 0;
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 1) } == 0 {
            let error = io::Error::last_os_error();
            return if closed_pipe(&error) {
                Ok(0)
            } else {
                Err(error)
            };
        }
        Ok(transferred as usize)
    }
}

#[cfg(windows)]
fn write_all_with_deadline(
    stream: &mut Stream,
    payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    windows_pipe::write_all(stream, payload, timeout)
}

#[cfg(windows)]
fn read_line_with_deadline(stream: &mut Stream, timeout: Duration) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let capacity = (MAX_RESPONSE_LEN as usize).saturating_sub(out.len());
        if capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "paneflow response exceeded the size cap",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let read_buffer_len = capacity.min(chunk.len());
        match windows_pipe::read_some(stream, &mut chunk[..read_buffer_len], remaining) {
            Ok(0) if out.is_empty() => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "paneflow closed the IPC connection without a response",
                ));
            }
            Ok(0) => break,
            Ok(n) => {
                if let Some(line) = append_capped_response_chunk(&mut out, &chunk[..n])? {
                    return Ok(line);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "paneflow did not respond within 10s",
                ));
            }
            Err(e) => return Err(e),
        }
    }
    String::from_utf8(out).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(windows)]
fn append_capped_response_chunk(out: &mut Vec<u8>, chunk: &[u8]) -> io::Result<Option<String>> {
    let frame_len = chunk
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(chunk.len(), |position| position + 1);
    if out.len().saturating_add(frame_len) > MAX_RESPONSE_LEN as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "paneflow response exceeded the size cap",
        ));
    }

    out.extend_from_slice(&chunk[..frame_len]);
    if chunk.get(frame_len.saturating_sub(1)) == Some(&b'\n') {
        return String::from_utf8(out.clone())
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
    if out.len() as u64 >= MAX_RESPONSE_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "paneflow response exceeded the size cap",
        ));
    }
    Ok(None)
}

pub fn subscribe_stream(
    socket: &Path,
    params: Value,
    mut on_line: impl FnMut(&str) -> bool,
) -> io::Result<()> {
    let name = socket.to_fs_name::<GenericFilePath>()?;
    let mut stream = Stream::connect(name)?;
    let request = build_request(1, "events.subscribe", params);
    let mut payload =
        serde_json::to_vec(&request).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    while let Some(line) = read_capped_event_line(&mut reader, &mut buf)? {
        if line.trim().is_empty() {
            continue;
        }
        if !on_line(&line) {
            break;
        }
    }
    Ok(())
}

fn read_capped_event_line<R>(reader: &mut R, buf: &mut Vec<u8>) -> io::Result<Option<String>>
where
    R: BufRead,
{
    buf.clear();
    loop {
        let remaining = MAX_RESPONSE_LEN.saturating_sub(buf.len() as u64);
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "paneflow event line exceeded the size cap",
            ));
        }

        let read = reader.by_ref().take(remaining).read_until(b'\n', buf)?;
        if read == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "paneflow event stream ended mid-line",
            ));
        }

        if buf.last() == Some(&b'\n') {
            if buf.ends_with(b"\r\n") {
                buf.truncate(buf.len().saturating_sub(2));
            } else {
                buf.truncate(buf.len().saturating_sub(1));
            }
            let line = String::from_utf8(buf.clone())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(Some(line));
        }
    }
}

pub enum StreamEvent<'a> {
    Line(&'a str),
    Tick,
    Closed,
}

pub fn subscribe_stream_timed(
    socket: &Path,
    params: Value,
    slice: Duration,
    mut on_event: impl FnMut(StreamEvent<'_>) -> bool,
) -> io::Result<()> {
    let name = socket.to_fs_name::<GenericFilePath>()?;
    let mut stream = Stream::connect(name)?;
    stream.set_recv_timeout(Some(slice)).map_err(|e| {
        if e.kind() == io::ErrorKind::Unsupported {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "the event stream needs a recv-timeout-capable socket (this \
                 platform's named pipe rejects it)",
            )
        } else {
            e
        }
    })?;
    let request = build_request(1, "events.subscribe", params);
    let mut payload =
        serde_json::to_vec(&request).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let remaining = MAX_RESPONSE_LEN.saturating_sub(buf.len() as u64);
        if remaining == 0 {
            on_event(StreamEvent::Closed);
            return Ok(());
        }
        match reader.by_ref().take(remaining).read_until(b'\n', &mut buf) {
            Ok(0) => {
                on_event(StreamEvent::Closed);
                return Ok(());
            }
            Ok(_) if buf.last() == Some(&b'\n') => {
                let keep = {
                    let line = String::from_utf8_lossy(&buf);
                    let line = line.trim();
                    line.is_empty() || on_event(StreamEvent::Line(line))
                };
                buf.clear();
                if !keep {
                    return Ok(());
                }
            }
            Ok(_) => {
                on_event(StreamEvent::Closed);
                return Ok(());
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if !on_event(StreamEvent::Tick) {
                    return Ok(());
                }
            }
            Err(_) => {
                on_event(StreamEvent::Closed);
                return Ok(());
            }
        }
    }
}

pub fn resolve_socket_path() -> Option<PathBuf> {
    if let Some(p) = socket_path_from_env(std::env::var("PANEFLOW_SOCKET_PATH").ok().as_deref()) {
        return Some(p);
    }
    default_socket_path()
}

pub(crate) fn socket_path_from_env(raw: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    path.is_absolute().then_some(path)
}

#[cfg(unix)]
fn default_socket_path() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
        })
        .or_else(cache_run_dir)?;
    let subdir = if cfg!(debug_assertions) {
        "paneflow-dev"
    } else {
        "paneflow"
    };
    let socket_file = if cfg!(debug_assertions) {
        "paneflow-dev.sock"
    } else {
        "paneflow.sock"
    };
    Some(runtime.join(subdir).join(socket_file))
}

#[cfg(unix)]
fn cache_run_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Caches").join("run"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|c| c.join("run"))
    }
}

#[cfg(windows)]
fn default_socket_path() -> Option<PathBuf> {
    Some(PathBuf::from(if cfg!(debug_assertions) {
        r"\\.\pipe\paneflow-dev"
    } else {
        r"\\.\pipe\paneflow"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerate_unsupported_swallows_only_unsupported() {
        assert!(tolerate_unsupported(Ok(())).is_ok());
        assert!(
            tolerate_unsupported(Err(io::Error::from(io::ErrorKind::Unsupported))).is_ok(),
            "Unsupported (named-pipe timeout) must be tolerated"
        );
        let other = tolerate_unsupported(Err(io::Error::from(io::ErrorKind::PermissionDenied)));
        assert_eq!(
            other.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "a real error must still propagate unchanged"
        );
    }

    #[test]
    fn build_request_has_jsonrpc_envelope() {
        let req = build_request(7, "surface.list", json!({}));
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["id"], 7);
        assert_eq!(req["method"], "surface.list");
        assert_eq!(req["params"], json!({}));
    }

    #[test]
    fn parse_response_extracts_result() {
        let line = r#"{"jsonrpc":"2.0","result":{"surfaces":[]},"id":1}"#;
        let result = parse_response(line).expect("ok");
        assert_eq!(result, json!({"surfaces": []}));
    }

    #[test]
    fn parse_response_translates_error_envelope() {
        let line = r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"surface_id 9 not found"},"id":1}"#;
        let err = parse_response(line).expect_err("err");
        assert!(err.contains("-32602"), "got: {err}");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn jsonrpc_error_message_detects_stream_error_line() {
        let line = r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"bad filter"},"id":null}"#;
        let err = jsonrpc_error_message(line).expect("error");
        assert!(err.contains("-32602"), "got: {err}");
        assert!(err.contains("bad filter"), "got: {err}");
        assert!(jsonrpc_error_message(r#"{"type":"subscribed"}"#).is_none());
    }

    #[test]
    fn parse_response_rejects_missing_result_and_error() {
        let line = r#"{"jsonrpc":"2.0","id":1}"#;
        assert!(parse_response(line).is_err());
    }

    #[test]
    fn parse_response_rejects_malformed_json() {
        assert!(parse_response("not json").is_err());
    }

    #[test]
    fn capped_event_line_rejects_oversized_unterminated_frame() {
        let data = vec![b'x'; MAX_RESPONSE_LEN as usize];
        let mut reader = BufReader::new(std::io::Cursor::new(data));
        let mut buf = Vec::new();
        let err = read_capped_event_line(&mut reader, &mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn capped_event_line_reads_one_frame() {
        let mut reader = BufReader::new(std::io::Cursor::new(b"{\"type\":\"ai.stop\"}\nrest"));
        let mut buf = Vec::new();
        let line = read_capped_event_line(&mut reader, &mut buf)
            .expect("read")
            .expect("line");
        assert_eq!(line, "{\"type\":\"ai.stop\"}");
    }

    #[test]
    fn socket_path_from_env_requires_absolute() {
        #[cfg(not(windows))]
        let absolute = "/run/user/1000/paneflow/paneflow.sock";
        #[cfg(windows)]
        let absolute = r"\\.\pipe\paneflow";
        assert_eq!(
            socket_path_from_env(Some(absolute)),
            Some(PathBuf::from(absolute))
        );
        assert_eq!(socket_path_from_env(Some("relative/path.sock")), None);
        assert_eq!(socket_path_from_env(Some("")), None);
        assert_eq!(socket_path_from_env(None), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_socket_path_matches_build_profile() {
        let expected = if cfg!(debug_assertions) {
            r"\\.\pipe\paneflow-dev"
        } else {
            r"\\.\pipe\paneflow"
        };
        assert_eq!(default_socket_path(), Some(PathBuf::from(expected)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_ipc_client_round_trips_when_response_is_delayed() {
        use interprocess::local_socket::{Listener, ListenerOptions};
        use interprocess::TryClone;

        static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            r"\\.\pipe\paneflow-ipc-client-test-{}-{}",
            std::process::id(),
            NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
        ));
        let name = path.as_path().to_fs_name::<GenericFilePath>().unwrap();
        let listener: Listener = ListenerOptions::new().name(name).create_sync().unwrap();

        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept");
            let mut writer = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request");
            let request: Value = serde_json::from_str(line.trim()).expect("parse request");
            std::thread::sleep(Duration::from_millis(75));
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"].clone(),
                "result": {"surfaces": [{"surface_id": 1u64, "name": "windows"}]},
            });
            let mut serialized = serde_json::to_string(&response).unwrap();
            serialized.push('\n');
            writer
                .write_all(serialized.as_bytes())
                .expect("write response");
            writer.flush().expect("flush");
        });

        let client = IpcClient::new(path);
        let result = client.call("surface.list", json!({})).expect("call ok");
        assert_eq!(result["surfaces"][0]["name"], "windows");
        server.join().expect("server thread");
    }

    #[cfg(windows)]
    #[test]
    fn windows_response_cap_accepts_exactly_capped_line() {
        let mut out = vec![b'x'; MAX_RESPONSE_LEN as usize - 1];
        let line = append_capped_response_chunk(&mut out, b"\n")
            .expect("exact cap is valid")
            .expect("newline completes the frame");
        assert_eq!(line.len(), MAX_RESPONSE_LEN as usize);
    }

    #[cfg(windows)]
    #[test]
    fn windows_response_cap_rejects_newline_beyond_limit() {
        let mut out = vec![b'x'; MAX_RESPONSE_LEN as usize - 1];
        let error = append_capped_response_chunk(&mut out, b"y\n")
            .expect_err("newline beyond the cap must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn ipc_client_round_trips_against_a_live_socket() {
        use interprocess::local_socket::{Listener, ListenerOptions};
        use interprocess::TryClone;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("paneflow-test.sock");
        let name = path.as_path().to_fs_name::<GenericFilePath>().unwrap();
        let listener: Listener = ListenerOptions::new().name(name).create_sync().unwrap();

        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept");
            let mut writer = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request");
            let request: Value = serde_json::from_str(line.trim()).expect("parse request");
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"].clone(),
                "result": {"surfaces": [{"surface_id": 1u64, "name": "cargo-run"}]},
            });
            let mut serialized = serde_json::to_string(&response).unwrap();
            serialized.push('\n');
            writer
                .write_all(serialized.as_bytes())
                .expect("write response");
            writer.flush().expect("flush");
        });

        let client = IpcClient::new(path);
        let result = client.call("surface.list", json!({})).expect("call ok");
        assert_eq!(result["surfaces"][0]["name"], "cargo-run");

        server.join().expect("server thread");
    }

    #[cfg(unix)]
    #[test]
    fn ipc_client_call_errors_when_socket_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.sock");
        let client = IpcClient::new(path);
        let err = client
            .call("surface.list", json!({}))
            .expect_err("must fail with no listener");
        assert!(err.contains("unreachable"), "got: {err}");
    }
}
