use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

pub const HOOK_LOG_ENV: &str = "PANEFLOW_HOOK_LOG";

pub fn append(path: &Path, line: &str) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{HOOK_LOG_ENV} must be an absolute path"),
        ));
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    file.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_creates_nothing() {
        let relative = Path::new("paneflow-hook-log-relative-test.log");
        assert!(append(relative, "x\n").is_err());
        assert!(!relative.exists());
    }

    #[test]
    fn an_absolute_regular_file_gets_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("hook.log");
        append(&log, "one\n").unwrap();
        append(&log, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn a_symlinked_log_leaves_its_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("victim.txt");
        std::fs::write(&target, "keep").unwrap();
        let link = dir.path().join("hook.log");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&target, &link).is_ok();
        if !linked {
            return;
        }
        assert!(append(&link, "injected\n").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_returns_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("hook.log");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(append(&fifo, "x\n").is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }
}
