use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use interprocess::local_socket::ConnectOptions;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use paneflow_ipc_client::ai_hook::{AiHookFrame, MAX_FRAME_BYTES};

const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_BACKOFF: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(300)];

pub(crate) fn send_frame(socket_path: &Path, frame: &AiHookFrame) -> io::Result<()> {
    let payload = serialize_frame(frame)?;
    let mut stream = connect_with_retry(socket_path)?;

    #[cfg(not(windows))]
    {
        if let Err(error) = stream.set_send_timeout(Some(WRITE_TIMEOUT)) {
            if error.kind() != io::ErrorKind::Unsupported {
                return Err(error);
            }
        }
        stream.write_all(&payload)
    }

    #[cfg(windows)]
    {
        write_all_with_deadline(&mut stream, &payload, WRITE_TIMEOUT)
    }
}

fn serialize_frame(frame: &AiHookFrame) -> io::Result<Vec<u8>> {
    let mut payload = serde_json::to_vec(&frame.to_value())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    payload.push(b'\n');
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "paneflow hook IPC frame exceeds the server size cap",
        ));
    }
    Ok(payload)
}

fn connect_with_retry(socket_path: &Path) -> io::Result<Stream> {
    let mut result = connect_once(socket_path);
    for backoff in CONNECT_BACKOFF {
        if result.is_ok() {
            break;
        }
        std::thread::sleep(backoff);
        result = connect_once(socket_path);
    }
    result
}

fn connect_once(socket_path: &Path) -> io::Result<Stream> {
    let name = socket_path.to_fs_name::<GenericFilePath>()?;
    #[cfg(windows)]
    {
        ConnectOptions::new()
            .name(name)
            .nonblocking_stream(true)
            .connect_sync()
    }
    #[cfg(not(windows))]
    {
        Stream::connect(name)
    }
}

#[cfg(windows)]
fn wait_for_io(deadline: Instant) -> io::Result<()> {
    let now = Instant::now();
    if now >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "paneflow hook IPC write timed out",
        ));
    }
    std::thread::sleep((deadline - now).min(Duration::from_millis(5)));
    Ok(())
}

#[cfg(windows)]
fn write_all_with_deadline(
    stream: &mut Stream,
    mut payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while !payload.is_empty() {
        match stream.write(payload) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "paneflow hook IPC write made no progress",
                ));
            }
            Ok(written) => payload = &payload[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_for_io(deadline)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_ipc_client::ai_hook::{AiHookMethod, AiHookParams, AiToolName};
    use serde_json::json;

    fn frame(payload: serde_json::Value) -> AiHookFrame {
        AiHookFrame::new(
            AiHookMethod::Stop,
            AiHookParams::new(
                1,
                AiToolName::parse("claude").expect("valid test tool"),
                payload,
            ),
        )
    }

    #[test]
    fn oversized_frame_is_rejected_before_connecting() {
        let error = send_frame(
            Path::new("this-path-is-never-opened"),
            &frame(json!({"message": "x".repeat(MAX_FRAME_BYTES)})),
        )
        .expect_err("oversized frame");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn frame_is_written_once_as_newline_delimited_json() {
        use std::io::{BufRead, BufReader};

        use interprocess::local_socket::{Listener, ListenerOptions};

        let directory = tempfile::TempDir::new().expect("temp directory");
        let path = directory.path().join("hook.sock");
        let name = path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("socket name");
        let listener: Listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("listener");
        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("read frame");
            line
        });

        send_frame(&path, &frame(json!({"session_id": "s1"}))).expect("send frame");
        let line = server.join().expect("server thread");
        assert!(line.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
        assert_eq!(value["method"], "ai.stop");
    }

    #[cfg(unix)]
    #[test]
    fn connection_is_retried_before_any_write() {
        use std::io::{BufRead, BufReader};

        use interprocess::local_socket::{Listener, ListenerOptions};

        let directory = tempfile::TempDir::new().expect("temp directory");
        let path = directory.path().join("late.sock");
        let server_path = path.clone();
        let server = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let name = server_path
                .as_path()
                .to_fs_name::<GenericFilePath>()
                .expect("socket name");
            let listener: Listener = ListenerOptions::new()
                .name(name)
                .create_sync()
                .expect("listener");
            let stream = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("read frame");
            line
        });

        send_frame(&path, &frame(json!({}))).expect("connect retry succeeds");
        assert!(server.join().expect("server thread").ends_with('\n'));
    }
}
