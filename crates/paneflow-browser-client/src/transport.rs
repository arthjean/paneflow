use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;

#[cfg(unix)]
pub fn connect(endpoint: &Path) -> io::Result<Stream> {
    std::os::unix::net::UnixStream::connect(endpoint)
}

#[cfg(unix)]
pub fn set_read_timeout(stream: &Stream, timeout: Option<Duration>) -> io::Result<()> {
    stream.set_read_timeout(timeout)
}

#[cfg(unix)]
pub fn try_clone(stream: &Stream) -> io::Result<Stream> {
    stream.try_clone()
}

#[cfg(unix)]
pub fn close(stream: &Stream) {
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

#[cfg(unix)]
pub fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
pub struct Stream;

#[cfg(not(unix))]
impl io::Read for Stream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(unsupported())
    }
}

#[cfg(not(unix))]
impl io::Write for Stream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(unsupported())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(not(unix))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the browser host transport is not available on this platform yet",
    )
}

#[cfg(not(unix))]
pub fn connect(_endpoint: &Path) -> io::Result<Stream> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub fn set_read_timeout(_stream: &Stream, _timeout: Option<Duration>) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub fn try_clone(_stream: &Stream) -> io::Result<Stream> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub fn close(_stream: &Stream) {}

#[cfg(not(unix))]
pub fn detach(_command: &mut Command) {}
