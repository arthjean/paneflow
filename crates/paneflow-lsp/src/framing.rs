use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

use serde_json::Value;

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_LINE_BYTES: u64 = 8 * 1024;

pub fn encode_message(body: &Value) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(body)?;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn write_message(writer: &mut impl Write, body: &Value) -> io::Result<()> {
    writer.write_all(&encode_message(body)?)?;
    writer.flush()
}

#[derive(Debug)]
pub enum Frame {
    Message(Value),
    Invalid(serde_json::Error),
}

#[derive(Debug)]
pub enum FrameError {
    EndOfStream,
    Io(io::Error),
    MissingContentLength,
    InvalidHeader(String),
    TooLarge(usize),
}

impl FrameError {
    pub fn is_protocol_error(&self) -> bool {
        matches!(
            self,
            Self::MissingContentLength | Self::InvalidHeader(_) | Self::TooLarge(_)
        )
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfStream => f.write_str("end of stream"),
            Self::Io(error) => write!(f, "read failed: {error}"),
            Self::MissingContentLength => f.write_str("message header has no Content-Length"),
            Self::InvalidHeader(line) => write!(f, "invalid message header line {line:?}"),
            Self::TooLarge(length) => write!(
                f,
                "message of {length} bytes exceeds the {MAX_MESSAGE_BYTES}-byte limit"
            ),
        }
    }
}

pub struct FrameReader<R> {
    reader: BufReader<R>,
    max_body: usize,
}

impl<R: Read> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_limit(reader, MAX_MESSAGE_BYTES)
    }

    pub fn with_limit(reader: R, max_body: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            max_body,
        }
    }

    pub fn read_frame(&mut self) -> Result<Frame, FrameError> {
        let mut content_length = None;
        let mut first_line = true;
        loop {
            let line = self.read_header_line(first_line)?;
            first_line = false;
            if line.is_empty() {
                break;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(FrameError::InvalidHeader(line));
            };
            if name.trim().eq_ignore_ascii_case("content-length") {
                let length = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| FrameError::InvalidHeader(line.clone()))?;
                content_length = Some(length);
            }
        }
        let length = content_length.ok_or(FrameError::MissingContentLength)?;
        if length > self.max_body {
            return Err(FrameError::TooLarge(length));
        }
        let mut body = vec![0u8; length];
        self.reader.read_exact(&mut body).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                FrameError::EndOfStream
            } else {
                FrameError::Io(error)
            }
        })?;
        Ok(match serde_json::from_slice(&body) {
            Ok(value) => Frame::Message(value),
            Err(error) => Frame::Invalid(error),
        })
    }

    fn read_header_line(&mut self, first_line: bool) -> Result<String, FrameError> {
        let mut line = Vec::new();
        let read = (&mut self.reader)
            .take(MAX_HEADER_LINE_BYTES)
            .read_until(b'\n', &mut line)
            .map_err(FrameError::Io)?;
        if read == 0 {
            return Err(if first_line {
                FrameError::EndOfStream
            } else {
                FrameError::InvalidHeader(String::new())
            });
        }
        if line.last() != Some(&b'\n') {
            if line.len() as u64 >= MAX_HEADER_LINE_BYTES {
                return Err(FrameError::InvalidHeader(
                    String::from_utf8_lossy(&line[..64.min(line.len())]).into_owned(),
                ));
            }
            return Err(FrameError::EndOfStream);
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        String::from_utf8(line).map_err(|error| {
            FrameError::InvalidHeader(String::from_utf8_lossy(error.as_bytes()).into_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Chunked {
        bytes: Vec<u8>,
        sizes: Vec<usize>,
        position: usize,
        step: usize,
    }

    impl Read for Chunked {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.position >= self.bytes.len() {
                return Ok(0);
            }
            let size = self.sizes[self.step % self.sizes.len()]
                .max(1)
                .min(buffer.len())
                .min(self.bytes.len() - self.position);
            self.step += 1;
            buffer[..size].copy_from_slice(&self.bytes[self.position..self.position + size]);
            self.position += size;
            Ok(size)
        }
    }

    #[test]
    fn a_written_message_is_a_content_length_header_then_the_utf8_body() {
        let body = json!({"jsonrpc": "2.0", "method": "é"});
        let mut out = Vec::new();
        write_message(&mut out, &body).unwrap();
        let text = serde_json::to_vec(&body).unwrap();
        let mut expected = format!("Content-Length: {}\r\n\r\n", text.len()).into_bytes();
        expected.extend_from_slice(&text);
        assert_eq!(out, expected);
        assert_eq!(text.len(), r#"{"jsonrpc":"2.0","method":"é"}"#.len());
    }

    #[test]
    fn messages_split_at_any_byte_boundary_decode_once_and_in_order() {
        let messages: Vec<Value> = (0..20)
            .map(|index| json!({"jsonrpc": "2.0", "id": index, "result": "x".repeat(index * 7)}))
            .collect();
        let mut stream = Vec::new();
        for message in &messages {
            stream.extend(encode_message(message).unwrap());
        }
        for sizes in [vec![1], vec![2, 3], vec![5, 1, 17], vec![64], vec![4096]] {
            let mut reader = FrameReader::new(Chunked {
                bytes: stream.clone(),
                sizes,
                position: 0,
                step: 0,
            });
            let mut decoded = Vec::new();
            loop {
                match reader.read_frame() {
                    Ok(Frame::Message(value)) => decoded.push(value),
                    Err(FrameError::EndOfStream) => break,
                    other => {
                        assert!(other.is_ok(), "{other:?}");
                        break;
                    }
                }
            }
            assert_eq!(decoded, messages);
        }
    }

    #[test]
    fn a_header_without_content_length_is_a_protocol_error() {
        let mut reader = FrameReader::new(&b"Content-Type: x\r\n\r\n{}"[..]);
        let error = reader.read_frame().unwrap_err();
        assert!(matches!(error, FrameError::MissingContentLength));
        assert!(error.is_protocol_error());
    }

    struct NeverRead;

    impl Read for NeverRead {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the body must not be read"))
        }
    }

    #[test]
    fn a_length_above_the_limit_fails_before_the_body_is_allocated_or_read() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        let mut reader = FrameReader::new(io::Read::chain(header.as_bytes(), NeverRead));
        let error = reader.read_frame().unwrap_err();
        assert!(matches!(error, FrameError::TooLarge(length) if length == MAX_MESSAGE_BYTES + 1));
        assert!(error.is_protocol_error());
    }

    #[test]
    fn an_invalid_json_body_is_reported_and_the_next_message_still_decodes() {
        let mut stream = b"Content-Length: 5\r\n\r\n{nope".to_vec();
        stream.extend(encode_message(&json!({"ok": true})).unwrap());
        let mut reader = FrameReader::new(&stream[..]);
        assert!(matches!(reader.read_frame(), Ok(Frame::Invalid(_))));
        assert!(
            matches!(reader.read_frame(), Ok(Frame::Message(value)) if value == json!({"ok": true}))
        );
    }
}
