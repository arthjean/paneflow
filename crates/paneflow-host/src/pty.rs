#[cfg(windows)]
mod windows;

pub fn open(size: portable_pty::PtySize) -> Result<portable_pty::PtyPair, String> {
    #[cfg(windows)]
    windows::initialize()?;
    #[cfg(windows)]
    windows::deliver_ctrl_c_to_children()?;

    portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|error| error.to_string())
}
