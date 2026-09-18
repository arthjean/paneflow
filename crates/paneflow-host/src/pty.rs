#[cfg(windows)]
mod windows;

pub fn open(size: portable_pty::PtySize) -> Result<portable_pty::PtyPair, String> {
    #[cfg(windows)]
    windows::initialize()?;

    portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|error| error.to_string())
}
