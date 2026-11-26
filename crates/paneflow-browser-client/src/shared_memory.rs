#[cfg(unix)]
mod imp {
    use std::collections::VecDeque;
    use std::io::{self, Read};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    pub type SharedFd = OwnedFd;

    const MAX_FDS_PER_READ: usize = 16;

    pub struct Mapping {
        base: *const u8,
        len: usize,
        _fd: OwnedFd,
    }

    unsafe impl Send for Mapping {}
    unsafe impl Sync for Mapping {}

    impl Mapping {
        pub fn read_only(fd: OwnedFd, len: usize) -> Result<Self, String> {
            if len == 0 {
                return Err("a frame slot cannot be empty".to_string());
            }
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
                return Err(format!(
                    "cannot inspect a frame slot: {}",
                    io::Error::last_os_error()
                ));
            }
            let size = usize::try_from(stat.st_size).unwrap_or(0);
            if size < len {
                return Err(format!(
                    "a frame slot holds {size} bytes, fewer than the {len} its frames need"
                ));
            }
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd.as_raw_fd(),
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(format!(
                    "cannot map a frame slot: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(Self {
                base: base.cast(),
                len,
                _fd: fd,
            })
        }

        pub fn bytes(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.base, self.len) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.base.cast_mut().cast(), self.len);
            }
        }
    }

    pub type StreamReader = FdReader;

    pub struct FdReader {
        stream: UnixStream,
        fds: VecDeque<OwnedFd>,
    }

    impl FdReader {
        pub fn new(stream: UnixStream) -> Self {
            Self {
                stream,
                fds: VecDeque::new(),
            }
        }

        pub fn take_fds(&mut self, count: usize) -> Option<Vec<OwnedFd>> {
            if self.fds.len() < count {
                self.fds.clear();
                return None;
            }
            Some(self.fds.drain(..count).collect())
        }

        pub fn stream(&self) -> &UnixStream {
            &self.stream
        }
    }

    impl Read for FdReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let payload = std::mem::size_of::<i32>() * MAX_FDS_PER_READ;
            let space = unsafe { libc::CMSG_SPACE(payload as u32) } as usize;
            let mut control = vec![0u8; space];
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            };
            let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
            header.msg_iov = &mut iov;
            header.msg_iovlen = 1;
            header.msg_control = control.as_mut_ptr().cast();
            header.msg_controllen = space as _;
            let read = loop {
                let read =
                    unsafe { libc::recvmsg(self.stream.as_raw_fd(), &mut header, cloexec_flag()) };
                if read >= 0 {
                    break read as usize;
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            };
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&header);
                while !cmsg.is_null() {
                    if (*cmsg).cmsg_level == libc::SOL_SOCKET
                        && (*cmsg).cmsg_type == libc::SCM_RIGHTS
                    {
                        let data = libc::CMSG_DATA(cmsg);
                        let header_len = libc::CMSG_LEN(0) as usize;
                        let bytes = ((*cmsg).cmsg_len as usize).saturating_sub(header_len);
                        let count = bytes / std::mem::size_of::<i32>();
                        for index in 0..count {
                            let mut raw = 0i32;
                            std::ptr::copy_nonoverlapping(
                                data.add(index * std::mem::size_of::<i32>()),
                                std::ptr::addr_of_mut!(raw).cast::<u8>(),
                                std::mem::size_of::<i32>(),
                            );
                            if raw >= 0 {
                                self.fds.push_back(OwnedFd::from_raw_fd(raw));
                            }
                        }
                    }
                    cmsg = libc::CMSG_NXTHDR(&header, cmsg);
                }
            }
            Ok(read)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn cloexec_flag() -> libc::c_int {
        libc::MSG_CMSG_CLOEXEC
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn cloexec_flag() -> libc::c_int {
        0
    }

    #[cfg(test)]
    pub fn anonymous_for_test(contents: &[u8]) -> OwnedFd {
        use std::io::Write;
        let mut file = tempfile::tempfile().expect("a temporary file");
        file.write_all(contents).expect("written");
        OwnedFd::from(file)
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io::{self, Read};

    pub struct SharedFd;

    pub struct Mapping;

    impl Mapping {
        pub fn read_only(_fd: SharedFd, _len: usize) -> Result<Self, String> {
            Err("shared frames are not available on this platform yet".to_string())
        }

        pub fn bytes(&self) -> &[u8] {
            &[]
        }
    }

    pub type StreamReader = FdReader<crate::transport::Stream>;

    pub struct FdReader<S> {
        stream: S,
    }

    impl<S: Read> FdReader<S> {
        pub fn new(stream: S) -> Self {
            Self { stream }
        }

        pub fn take_fds(&mut self, _count: usize) -> Option<Vec<SharedFd>> {
            None
        }

        pub fn stream(&self) -> &S {
            &self.stream
        }
    }

    impl<S: Read> Read for FdReader<S> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.stream.read(buf)
        }
    }
}

pub use imp::*;
