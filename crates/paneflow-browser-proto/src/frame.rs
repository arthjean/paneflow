use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::limits::MAX_MESSAGE_BYTES;

pub const LENGTH_PREFIX_BYTES: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("a protocol message declares {declared} bytes, over the {max}-byte limit")]
    TooLarge { declared: usize, max: usize },
    #[error("the connection closed")]
    Closed,
    #[error("the connection failed: {0}")]
    Io(#[from] io::Error),
    #[error("a protocol message is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLarge {
            declared: body.len(),
            max: MAX_MESSAGE_BYTES,
        });
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge {
        declared: body.len(),
        max: MAX_MESSAGE_BYTES,
    })?;
    let mut framed = Vec::with_capacity(LENGTH_PREFIX_BYTES + body.len());
    framed.extend_from_slice(&length.to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

pub fn write_message<T: Serialize, W: Write>(
    writer: &mut W,
    message: &T,
) -> Result<(), FrameError> {
    let framed = encode(message)?;
    writer.write_all(&framed)?;
    writer.flush()?;
    Ok(())
}

pub fn declared_length(prefix: [u8; LENGTH_PREFIX_BYTES]) -> Result<usize, FrameError> {
    let declared = u32::from_le_bytes(prefix) as usize;
    if declared > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLarge {
            declared,
            max: MAX_MESSAGE_BYTES,
        });
    }
    Ok(declared)
}

pub fn read_message<T: DeserializeOwned, R: Read>(reader: &mut R) -> Result<T, FrameError> {
    let mut prefix = [0u8; LENGTH_PREFIX_BYTES];
    match reader.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::Closed);
        }
        Err(error) => return Err(FrameError::Io(error)),
    }
    let declared = declared_length(prefix)?;
    let mut body = vec![0u8; declared];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            FrameError::Closed
        } else {
            FrameError::Io(error)
        }
    })?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn decode<T: DeserializeOwned>(buffer: &[u8]) -> Result<Option<(T, usize)>, FrameError> {
    let Some(prefix) = buffer.get(..LENGTH_PREFIX_BYTES) else {
        return Ok(None);
    };
    let mut bytes = [0u8; LENGTH_PREFIX_BYTES];
    bytes.copy_from_slice(prefix);
    let declared = declared_length(bytes)?;
    let end = LENGTH_PREFIX_BYTES + declared;
    let Some(body) = buffer.get(LENGTH_PREFIX_BYTES..end) else {
        return Ok(None);
    };
    Ok(Some((serde_json::from_slice(body)?, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Command, RequestId, TabId, WindowMessage};

    #[test]
    fn a_partial_buffer_decodes_to_nothing_until_the_message_is_complete() {
        let message = WindowMessage::Request {
            id: RequestId(7),
            command: Command::Reload { tab: TabId(3) },
        };
        let framed = encode(&message).unwrap();
        for cut in 0..framed.len() {
            assert!(decode::<WindowMessage>(&framed[..cut]).unwrap().is_none());
        }
        let (decoded, consumed) = decode::<WindowMessage>(&framed).unwrap().unwrap();
        assert_eq!(decoded, message);
        assert_eq!(consumed, framed.len());
    }

    #[test]
    fn a_declared_length_over_the_limit_is_refused_from_the_prefix_alone() {
        let declared = MAX_MESSAGE_BYTES + 1;
        let prefix = u32::try_from(declared).unwrap().to_le_bytes();
        assert!(matches!(
            decode::<WindowMessage>(&prefix),
            Err(FrameError::TooLarge { declared: d, max }) if d == declared && max == MAX_MESSAGE_BYTES
        ));
        assert!(matches!(
            read_message::<WindowMessage, _>(&mut &prefix[..]),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_message_over_the_limit_is_not_encoded() {
        let message = WindowMessage::Request {
            id: RequestId(1),
            command: Command::Navigate {
                tab: TabId(1),
                url: "a".repeat(MAX_MESSAGE_BYTES),
            },
        };
        assert!(matches!(encode(&message), Err(FrameError::TooLarge { .. })));
    }

    #[test]
    fn a_stream_closed_before_a_prefix_reads_as_closed() {
        assert!(matches!(
            read_message::<WindowMessage, _>(&mut &[1u8, 0][..]),
            Err(FrameError::Closed)
        ));
    }
}
