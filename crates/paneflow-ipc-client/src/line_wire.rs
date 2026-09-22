use std::io;
use std::path::Path;
use std::time::Duration;

use interprocess::local_socket::Stream;
#[cfg(not(windows))]
use interprocess::local_socket::{prelude::*, GenericFilePath};
#[cfg(windows)]
use interprocess::os::windows::named_pipe::{local_socket, pipe_mode, DuplexPipeStream};

pub const WRITE_DEADLINE: Duration = Duration::from_secs(5);

pub enum LineRead {
    Line(String),
    Eof,
    TooLong,
}

#[cfg(windows)]
enum WindowsWireStream {
    Accepted(local_socket::Stream),
    Connected(DuplexPipeStream<pipe_mode::Bytes>),
}

#[cfg(windows)]
impl WindowsWireStream {
    fn pipe(&self) -> &DuplexPipeStream<pipe_mode::Bytes> {
        match self {
            Self::Accepted(stream) => stream.inner(),
            Self::Connected(stream) => stream,
        }
    }
}

pub struct Wire {
    max_frame: usize,
    #[cfg(windows)]
    stream: WindowsWireStream,
    #[cfg(windows)]
    pending: Vec<u8>,
    #[cfg(not(windows))]
    reader: io::BufReader<Stream>,
    #[cfg(not(windows))]
    writer: Stream,
}

impl Wire {
    pub fn connect(endpoint: &Path, max_frame: usize) -> io::Result<Self> {
        Self::connect_with_timeout(endpoint, max_frame, crate::host_control::REQUEST_DEADLINE)
    }

    pub fn connect_with_timeout(
        endpoint: &Path,
        max_frame: usize,
        timeout: Duration,
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use interprocess::ConnectWaitMode;
            let pipe = DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path_with_wait_mode(
                endpoint.as_os_str(),
                ConnectWaitMode::Timeout(timeout),
            )?;
            Ok(Self {
                max_frame,
                stream: WindowsWireStream::Connected(pipe),
                pending: Vec::new(),
            })
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            let name = endpoint.to_fs_name::<GenericFilePath>()?;
            Self::new(Stream::connect(name)?, max_frame)
        }
    }

    pub fn new(stream: Stream, max_frame: usize) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let Stream::NamedPipe(stream) = stream;
            Ok(Self {
                max_frame,
                stream: WindowsWireStream::Accepted(stream),
                pending: Vec::new(),
            })
        }
        #[cfg(not(windows))]
        {
            use interprocess::TryClone;
            let writer = stream.try_clone()?;
            Ok(Self {
                max_frame,
                reader: io::BufReader::new(stream),
                writer,
            })
        }
    }

    #[cfg(not(windows))]
    pub fn read_line(&mut self, timeout: Duration) -> io::Result<LineRead> {
        use std::io::{BufRead, Read};
        let _ = self.reader.get_ref().set_recv_timeout(Some(timeout));
        let mut buf = Vec::new();
        loop {
            let remaining = (self.max_frame + 1).saturating_sub(buf.len());
            if remaining == 0 {
                return Ok(LineRead::TooLong);
            }
            let read = (&mut self.reader)
                .take(remaining as u64)
                .read_until(b'\n', &mut buf)?;
            if read == 0 {
                return Ok(if buf.is_empty() {
                    LineRead::Eof
                } else {
                    LineRead::Line(String::from_utf8_lossy(&buf).into_owned())
                });
            }
            if buf.last() == Some(&b'\n') {
                if buf.len() > self.max_frame + 1 {
                    return Ok(LineRead::TooLong);
                }
                return Ok(LineRead::Line(
                    String::from_utf8_lossy(&buf).trim_end().to_string(),
                ));
            }
        }
    }

    #[cfg(windows)]
    pub fn read_line(&mut self, timeout: Duration) -> io::Result<LineRead> {
        use crate::windows_pipe::read_some;
        let deadline = std::time::Instant::now() + timeout;
        let mut scratch = [0u8; 4096];
        loop {
            if let Some(newline) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=newline).collect();
                if line.len() > self.max_frame + 1 {
                    return Ok(LineRead::TooLong);
                }
                return Ok(LineRead::Line(
                    String::from_utf8_lossy(&line).trim_end().to_string(),
                ));
            }
            if self.pending.len() > self.max_frame {
                return Ok(LineRead::TooLong);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let read = match read_some(self.stream.pipe(), &mut scratch, remaining) {
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if read == 0 {
                return Ok(if self.pending.is_empty() {
                    LineRead::Eof
                } else {
                    let line = std::mem::take(&mut self.pending);
                    LineRead::Line(String::from_utf8_lossy(&line).into_owned())
                });
            }
            self.pending.extend_from_slice(&scratch[..read]);
        }
    }

    pub fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        self.write_line_with_timeout(line, WRITE_DEADLINE)
    }

    pub fn write_line_with_timeout(&mut self, line: &[u8], timeout: Duration) -> io::Result<()> {
        if line.len() > self.max_frame {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control frame exceeds the {} byte limit", self.max_frame),
            ));
        }
        let mut payload = Vec::with_capacity(line.len() + 1);
        payload.extend_from_slice(line);
        payload.push(b'\n');
        self.write_raw_with_timeout(&payload, timeout)
    }

    pub fn write_raw(&mut self, payload: &[u8]) -> io::Result<()> {
        self.write_raw_with_timeout(payload, WRITE_DEADLINE)
    }

    pub fn write_raw_with_timeout(&mut self, payload: &[u8], timeout: Duration) -> io::Result<()> {
        #[cfg(windows)]
        {
            crate::windows_pipe::write_all(self.stream.pipe(), payload, timeout)
        }
        #[cfg(not(windows))]
        {
            use std::io::Write;
            let _ = self.writer.set_send_timeout(Some(timeout));
            self.writer.write_all(payload)?;
            self.writer.flush()
        }
    }

    pub fn write_json(&mut self, value: &serde_json::Value) -> io::Result<()> {
        self.write_json_with_timeout(value, WRITE_DEADLINE)
    }

    pub fn write_json_with_timeout(
        &mut self,
        value: &serde_json::Value,
        timeout: Duration,
    ) -> io::Result<()> {
        let line = serde_json::to_vec(value).map_err(io::Error::other)?;
        self.write_line_with_timeout(&line, timeout)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};

    #[test]
    fn a_busy_windows_pipe_obeys_the_requested_connect_timeout() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\paneflow-wire-timeout-{}-{}",
            std::process::id(),
            home.path().file_name().unwrap().to_string_lossy()
        ));
        let listener = ListenerOptions::new()
            .name(endpoint.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let held =
            Stream::connect(endpoint.as_path().to_fs_name::<GenericFilePath>().unwrap()).unwrap();
        let accepted = listener.accept().unwrap();
        drop(listener);
        let started = std::time::Instant::now();
        let error = match Wire::connect_with_timeout(&endpoint, 1024, Duration::from_millis(100)) {
            Ok(_) => panic!("no acceptor remains"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        let control = crate::host_control::HostControl::open(
            &endpoint,
            "bounded-control",
            Duration::from_millis(100),
        );
        assert!(
            matches!(control, Err(crate::host_control::ControlConnectError::Transport(error)) if error.kind() == io::ErrorKind::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(accepted);
        drop(held);
    }
}
