use std::io;
use std::path::Path;
use std::time::Duration;

use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};

pub const WRITE_DEADLINE: Duration = Duration::from_secs(5);

pub enum LineRead {
    Line(String),
    Eof,
    TooLong,
}

pub struct Wire {
    max_frame: usize,
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
    pub fn connect(endpoint: &Path, max_frame: usize) -> io::Result<Self> {
        let name = endpoint.to_fs_name::<GenericFilePath>()?;
        Self::new(Stream::connect(name)?, max_frame)
    }

    pub fn new(stream: Stream, max_frame: usize) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self {
                max_frame,
                stream,
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

    pub fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if line.len() > self.max_frame {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control frame exceeds the {} byte limit", self.max_frame),
            ));
        }
        let mut payload = Vec::with_capacity(line.len() + 1);
        payload.extend_from_slice(line);
        payload.push(b'\n');
        self.write_raw(&payload)
    }

    pub fn write_raw(&mut self, payload: &[u8]) -> io::Result<()> {
        #[cfg(windows)]
        {
            crate::windows_pipe::write_all(&self.stream, payload, WRITE_DEADLINE)
        }
        #[cfg(not(windows))]
        {
            use std::io::Write;
            let _ = self.writer.set_send_timeout(Some(WRITE_DEADLINE));
            self.writer.write_all(payload)?;
            self.writer.flush()
        }
    }

    pub fn write_json(&mut self, value: &serde_json::Value) -> io::Result<()> {
        let line = serde_json::to_vec(value).map_err(io::Error::other)?;
        self.write_line(&line)
    }
}
