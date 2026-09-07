use std::io;
use std::mem;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::wire::{encode_message, MAX_MESSAGE_BYTES};

pub const MAX_CHANNEL_FDS: usize = 12;
pub const FRAME_CHANNEL_ENV: &str = "PANEFLOW_BROWSER_FRAME_FD";

pub struct FrameChannel {
    socket: OwnedFd,
}

impl FrameChannel {
    pub fn pair() -> io::Result<(Self, Self)> {
        let mut fds = [0 as RawFd; 2];
        let result = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { (Self::from_raw_fd(fds[0]), Self::from_raw_fd(fds[1])) })
    }

    pub unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self {
            socket: unsafe { OwnedFd::from_raw_fd(fd) },
        }
    }

    pub fn from_environment() -> io::Result<Self> {
        let value = std::env::var(FRAME_CHANNEL_ENV)
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "frame channel fd is absent"))?;
        let fd: RawFd = value
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame channel fd"))?;
        if fd < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame channel fd must not be a standard stream",
            ));
        }
        let mut kind: libc::c_int = 0;
        let mut length = mem::size_of::<libc::c_int>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut libc::c_int).cast(),
                &mut length,
            )
        };
        if result != 0 || kind != libc::SOCK_SEQPACKET {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame channel fd is not a sequenced-packet socket",
            ));
        }
        Ok(unsafe { Self::from_raw_fd(fd) })
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            socket: self.socket.try_clone()?,
        })
    }

    pub fn send(&self, value: &impl Serialize, fds: &[BorrowedFd<'_>]) -> io::Result<()> {
        if fds.len() > MAX_CHANNEL_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many descriptors for one frame message",
            ));
        }
        let bytes = encode_message(value)?;
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let space =
            unsafe { libc::CMSG_SPACE((fds.len() * mem::size_of::<RawFd>()) as u32) } as usize;
        let mut control = vec![0_u8; space.max(mem::size_of::<libc::cmsghdr>())];
        let mut header: libc::msghdr = unsafe { mem::zeroed() };
        header.msg_iov = &mut iov;
        header.msg_iovlen = 1;
        if !fds.is_empty() {
            header.msg_control = control.as_mut_ptr().cast();
            header.msg_controllen = space as _;
            let cmsg = unsafe { libc::CMSG_FIRSTHDR(&header) };
            unsafe {
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len =
                    libc::CMSG_LEN((fds.len() * mem::size_of::<RawFd>()) as u32) as _;
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for (index, fd) in fds.iter().enumerate() {
                    data.add(index).write_unaligned(fd.as_raw_fd());
                }
            }
        }
        let sent = unsafe { libc::sendmsg(self.socket.as_raw_fd(), &header, libc::MSG_NOSIGNAL) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent as usize != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "frame message was not sent atomically",
            ));
        }
        Ok(())
    }

    pub fn recv<T: DeserializeOwned>(&self) -> io::Result<Option<(T, Vec<OwnedFd>)>> {
        let mut bytes = vec![0_u8; MAX_MESSAGE_BYTES + 4];
        let mut iov = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let space = unsafe { libc::CMSG_SPACE((MAX_CHANNEL_FDS * mem::size_of::<RawFd>()) as u32) }
            as usize;
        let mut control = vec![0_u8; space];
        let mut header: libc::msghdr = unsafe { mem::zeroed() };
        header.msg_iov = &mut iov;
        header.msg_iovlen = 1;
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = space as _;
        let received =
            unsafe { libc::recvmsg(self.socket.as_raw_fd(), &mut header, libc::MSG_CMSG_CLOEXEC) };
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fds = Vec::new();
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&header) };
        while !cmsg.is_null() {
            let (level, kind, length) = unsafe {
                (
                    (*cmsg).cmsg_level,
                    (*cmsg).cmsg_type,
                    (*cmsg).cmsg_len as usize,
                )
            };
            if level == libc::SOL_SOCKET && kind == libc::SCM_RIGHTS {
                let base = unsafe { libc::CMSG_LEN(0) } as usize;
                let count = length.saturating_sub(base) / mem::size_of::<RawFd>();
                let data = unsafe { libc::CMSG_DATA(cmsg) }.cast::<RawFd>();
                for index in 0..count {
                    let fd = unsafe { data.add(index).read_unaligned() };
                    fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
                }
            }
            cmsg = unsafe { libc::CMSG_NXTHDR(&header, cmsg) };
        }
        if received == 0 {
            return Ok(None);
        }
        let flags = header.msg_flags;
        if flags & libc::MSG_TRUNC != 0 || flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame message exceeded the channel bound",
            ));
        }
        let received = received as usize;
        if received < 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame message is shorter than its header",
            ));
        }
        let length = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if length > MAX_MESSAGE_BYTES || length != received - 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame message length does not match its datagram",
            ));
        }
        let value = serde_json::from_slice(&bytes[4..received])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some((value, fds)))
    }
}

impl AsRawFd for FrameChannel {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn messages_cross_the_pair_with_descriptors_and_keep_their_boundaries() {
        let (left, right) = FrameChannel::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        left.send(&json!({"type": "first"}), &[file.as_fd()])
            .unwrap();
        left.send(&json!({"type": "second"}), &[]).unwrap();
        let (first, fds): (Value, Vec<OwnedFd>) = right.recv().unwrap().unwrap();
        assert_eq!(first["type"], "first");
        assert_eq!(fds.len(), 1);
        let (second, fds): (Value, Vec<OwnedFd>) = right.recv().unwrap().unwrap();
        assert_eq!(second["type"], "second");
        assert!(fds.is_empty());
        drop(left);
        assert!(right.recv::<Value>().unwrap().is_none());
    }

    #[test]
    fn oversize_and_descriptor_overflow_are_refused_before_sending() {
        let (left, _right) = FrameChannel::pair().unwrap();
        let big = json!({"payload": "x".repeat(MAX_MESSAGE_BYTES)});
        assert!(left.send(&big, &[]).is_err());
        let file = std::fs::File::open("/dev/null").unwrap();
        let fds = vec![file.as_fd(); MAX_CHANNEL_FDS + 1];
        assert!(left.send(&json!({"type": "x"}), &fds).is_err());
    }

    #[test]
    fn a_standard_stream_or_stream_socket_is_not_a_frame_channel() {
        unsafe { std::env::set_var(FRAME_CHANNEL_ENV, "1") };
        assert!(FrameChannel::from_environment().is_err());
        let mut fds = [0 as RawFd; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        unsafe { std::env::set_var(FRAME_CHANNEL_ENV, fds[0].to_string()) };
        assert!(FrameChannel::from_environment().is_err());
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
            std::env::remove_var(FRAME_CHANNEL_ENV);
        }
    }
}
