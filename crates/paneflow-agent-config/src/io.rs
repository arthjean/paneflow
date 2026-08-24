use std::io::{Error, ErrorKind, Result, Write};
use std::path::Path;

use serde_json::Value;

pub fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

pub fn config_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir()
}

/// Read a UTF-8 configuration file without conflating absence with failure.
pub fn read_optional_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    write_atomic(path, |file| {
        serde_json::to_writer_pretty(&mut *file, value).map_err(Error::other)?;
        file.write_all(b"\n")
    })
}

pub fn write_text_atomic(path: &Path, content: &str) -> Result<()> {
    write_atomic(path, |file| file.write_all(content.as_bytes()))
}

fn write_atomic(path: &Path, write: impl FnOnce(&mut std::fs::File) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "configuration path has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write(temporary.as_file_mut())?;
    temporary.flush()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_read_distinguishes_absence_from_invalid_utf8() {
        let directory = tempfile::TempDir::new().unwrap();
        let missing = directory.path().join("missing.json");
        assert_eq!(read_optional_text(&missing).unwrap(), None);

        let invalid = directory.path().join("invalid.json");
        std::fs::write(&invalid, [0xff]).unwrap();
        assert_eq!(
            read_optional_text(&invalid).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(&invalid).unwrap(), [0xff]);
    }
}
