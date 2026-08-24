use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

pub(crate) fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    validate_sha256(expected)?;
    let mut file = fs::File::open(path).map_err(|error| format!("cannot hash input: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash input: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    verify_digest(digest.finalize(), expected)
}

pub(crate) fn verify_text_hash(path: &Path, expected: &str) -> Result<(), String> {
    validate_sha256(expected)?;
    let text =
        fs::read_to_string(path).map_err(|error| format!("cannot read UTF-8 input: {error}"))?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    verify_digest(Sha256::digest(normalized.as_bytes()), expected)
}

pub(crate) fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid SHA-256 digest `{value}`; expected 64 lowercase hexadecimal characters"
        ))
    }
}

fn verify_digest(actual: impl std::fmt::LowerHex, expected: &str) -> Result<(), String> {
    let actual = format!("{actual:x}");
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch: expected {expected}, got {actual}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::io::Write;

    #[test]
    fn rejects_non_canonical_sha256() {
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256("abc").is_err());
    }

    #[test]
    fn text_hash_normalizes_line_endings() -> io::Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(b"first\r\nsecond\r")?;
        let expected = format!("{:x}", Sha256::digest(b"first\nsecond\n"));
        assert_eq!(verify_text_hash(file.path(), &expected), Ok(()));
        Ok(())
    }
}
