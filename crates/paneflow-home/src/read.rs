use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

pub fn open_for_reading(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

pub fn not_regular_file(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} is not a regular file", path.display()),
    )
}

pub fn too_large(path: &Path, cap: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        format!("{} is over the {cap}-byte cap", path.display()),
    )
}

pub fn open_regular_for_reading(path: &Path) -> io::Result<(File, fs::Metadata)> {
    let file = open_for_reading(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(not_regular_file(path));
    }
    Ok((file, metadata))
}

pub fn read_regular_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let (file, metadata) = open_regular_for_reading(path)?;
    if metadata.len() > cap {
        return Err(too_large(path, cap));
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(too_large(path, cap));
    }
    Ok(bytes)
}

pub fn read_regular_string_capped(path: &Path, cap: u64) -> io::Result<String> {
    String::from_utf8(read_regular_capped(path, cap)?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not valid UTF-8", path.display()),
        )
    })
}

pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub fn secure_home_dir(home: &Path, tighten_existing: bool) -> io::Result<()> {
    create_private_dir_all(home)?;
    #[cfg(unix)]
    if tighten_existing {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(home)?.permissions().mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(home, fs::Permissions::from_mode(mode & 0o700 | 0o700))?;
        }
    }
    #[cfg(not(unix))]
    let _ = tighten_existing;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn an_existing_default_home_is_tightened_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".paneflow");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        secure_home_dir(&home, false).unwrap();
        assert_eq!(
            fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o755,
            "a home chosen through PANEFLOW_HOME keeps its mode"
        );
        secure_home_dir(&home, true).unwrap();
        assert_eq!(
            fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_regular_file_is_read_within_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(read_regular_capped(&path, 2).unwrap(), b"{}");
        assert_eq!(
            read_regular_capped(&path, 1).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
        assert_eq!(
            read_regular_capped(&dir.path().join("missing"), 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_directory_is_refused_as_not_regular() {
        let dir = tempfile::tempdir().unwrap();
        let refused = read_regular_capped(dir.path(), 8);
        assert!(refused.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_without_a_writer_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("session.json");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let refused = read_regular_capped(&fifo, 1024).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_dir_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home").join(".paneflow");
        create_private_dir_all(&home).unwrap();
        let mode = std::fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
