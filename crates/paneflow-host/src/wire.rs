use std::io;
use std::path::Path;
use std::time::Duration;

use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};

use crate::protocol::MAX_CONTROL_FRAME_BYTES;

pub(crate) const WRITE_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) enum LineRead {
    Line(String),
    Eof,
    TooLong,
}

pub(crate) struct Wire {
    #[cfg(windows)]
    stream: Stream,
    #[cfg(windows)]
    pending: Vec<u8>,
    #[cfg(not(windows))]
    reader: io::BufReader<Stream>,
    #[cfg(not(windows))]
    writer: Stream,
}

impl Wire {
    pub(crate) fn connect(endpoint: &Path) -> io::Result<Self> {
        let name = endpoint.to_fs_name::<GenericFilePath>()?;
        Self::new(Stream::connect(name)?)
    }

    pub(crate) fn new(stream: Stream) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self {
                stream,
                pending: Vec::new(),
            })
        }
        #[cfg(not(windows))]
        {
            use interprocess::TryClone;
            let writer = stream.try_clone()?;
            Ok(Self {
                reader: io::BufReader::new(stream),
                writer,
            })
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn read_line(&mut self, timeout: Duration) -> io::Result<LineRead> {
        use std::io::BufRead;
        let _ = self.reader.get_ref().set_recv_timeout(Some(timeout));
        let mut buf = Vec::new();
        loop {
            let remaining = (MAX_CONTROL_FRAME_BYTES + 1).saturating_sub(buf.len());
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
                if buf.len() > MAX_CONTROL_FRAME_BYTES + 1 {
                    return Ok(LineRead::TooLong);
                }
                return Ok(LineRead::Line(
                    String::from_utf8_lossy(&buf).trim_end().to_string(),
                ));
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn read_line(&mut self, timeout: Duration) -> io::Result<LineRead> {
        use paneflow_ipc_client::windows_pipe::read_some;
        let deadline = std::time::Instant::now() + timeout;
        let mut scratch = [0u8; 4096];
        loop {
            if let Some(newline) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=newline).collect();
                if line.len() > MAX_CONTROL_FRAME_BYTES + 1 {
                    return Ok(LineRead::TooLong);
                }
                return Ok(LineRead::Line(
                    String::from_utf8_lossy(&line).trim_end().to_string(),
                ));
            }
            if self.pending.len() > MAX_CONTROL_FRAME_BYTES {
                return Ok(LineRead::TooLong);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let read = match read_some(&self.stream, &mut scratch, remaining) {
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

    pub(crate) fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if line.len() > MAX_CONTROL_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control frame exceeds the 64 KiB limit",
            ));
        }
        let mut payload = Vec::with_capacity(line.len() + 1);
        payload.extend_from_slice(line);
        payload.push(b'\n');
        self.write_raw(&payload)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn write_raw(&mut self, payload: &[u8]) -> io::Result<()> {
        #[cfg(windows)]
        {
            paneflow_ipc_client::windows_pipe::write_all(&self.stream, payload, WRITE_DEADLINE)
        }
        #[cfg(not(windows))]
        {
            use std::io::Write;
            let _ = self.writer.set_send_timeout(Some(WRITE_DEADLINE));
            self.writer.write_all(payload)?;
            self.writer.flush()
        }
    }

    pub(crate) fn write_json(&mut self, value: &serde_json::Value) -> io::Result<()> {
        let line = serde_json::to_vec(value).map_err(io::Error::other)?;
        self.write_line(&line)
    }
}
