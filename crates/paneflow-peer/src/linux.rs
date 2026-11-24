use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::PathBuf;

use crate::membership::{ProcessFacts, ProcessTable};
use crate::{PeerIdentity, PeerRead, PeerSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    pub(crate) state: char,
    pub(crate) parent: u32,
    pub(crate) session: u32,
    pub(crate) started_at: u64,
}

impl Stat {
    fn exited(&self) -> bool {
        matches!(self.state, 'Z' | 'X' | 'x')
    }
}

pub(crate) fn parse_stat(stat: &str) -> Option<Stat> {
    let after_comm = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    Some(Stat {
        state: fields.first()?.chars().next()?,
        parent: fields.get(1)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        started_at: fields.get(19)?.parse().ok()?,
    })
}

pub(crate) trait Kernel {
    fn peer_pidfd(&self, socket: BorrowedFd<'_>) -> io::Result<OwnedFd>;
    fn peer_cred_pid(&self, socket: BorrowedFd<'_>) -> io::Result<u32>;
    fn pidfd_pid(&self, pidfd: &OwnedFd) -> io::Result<Option<u32>>;
    fn pidfd_open(&self, pid: u32) -> io::Result<OwnedFd>;
    fn stat(&self, pid: u32) -> io::Result<Stat>;
    fn executable(&self, pid: u32) -> Option<PathBuf>;
}

pub(crate) struct LiveKernel;

fn socket_option<T: Copy>(socket: BorrowedFd<'_>, option: libc::c_int, empty: T) -> io::Result<T> {
    let mut value = empty;
    let mut length = std::mem::size_of::<T>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&raw mut value).cast(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<T>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("socket option {option} returned {length} bytes"),
        ));
    }
    Ok(value)
}

impl Kernel for LiveKernel {
    fn peer_pidfd(&self, socket: BorrowedFd<'_>) -> io::Result<OwnedFd> {
        let fd = socket_option::<libc::c_int>(socket, libc::SO_PEERPIDFD, -1)?;
        if fd < 0 {
            return Err(io::Error::other("SO_PEERPIDFD returned no descriptor"));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn peer_cred_pid(&self, socket: BorrowedFd<'_>) -> io::Result<u32> {
        let empty = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let credentials = socket_option(socket, libc::SO_PEERCRED, empty)?;
        u32::try_from(credentials.pid)
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::other("SO_PEERCRED reported no peer process"))
    }

    fn pidfd_pid(&self, pidfd: &OwnedFd) -> io::Result<Option<u32>> {
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd()))?;
        let raw = info
            .lines()
            .find_map(|line| line.strip_prefix("Pid:"))
            .map(str::trim)
            .ok_or_else(|| io::Error::other("the pidfd's fdinfo names no Pid"))?;
        match raw.parse::<i64>() {
            Ok(pid) if pid > 0 => Ok(u32::try_from(pid).ok()),
            Ok(-1) => Ok(None),
            Ok(_) => Err(io::Error::other(
                "the peer process is not visible in this PID namespace",
            )),
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }

    fn pidfd_open(&self, pid: u32) -> io::Result<OwnedFd> {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_long, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = libc::c_int::try_from(fd).map_err(io::Error::other)?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn stat(&self, pid: u32) -> io::Result<Stat> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        parse_stat(&raw).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("/proc/{pid}/stat is malformed"),
            )
        })
    }

    fn executable(&self, pid: u32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
}

impl ProcessTable for LiveKernel {
    fn is_hidden(&self, pid: u32) -> bool {
        let Some(target) = libc::pid_t::try_from(pid).ok().filter(|target| *target > 0) else {
            return false;
        };
        if self.stat(pid).is_ok() {
            return false;
        }
        if unsafe { libc::kill(target, 0) } == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn facts(&self, pid: u32) -> io::Result<ProcessFacts> {
        let stat = self.stat(pid)?;
        if stat.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("process {pid} has exited"),
            ));
        }
        Ok(ProcessFacts {
            parent: stat.parent,
            session: stat.session,
            started_at: stat.started_at,
        })
    }

    fn describe(&self, pid: u32) -> (Option<String>, Option<PathBuf>) {
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|comm| comm.trim_end_matches('\n').to_string())
            .filter(|comm| !comm.is_empty());
        (name, self.executable(pid))
    }
}

pub(crate) fn read(socket: BorrowedFd<'_>) -> PeerRead {
    read_with(&LiveKernel, socket)
}

pub(crate) fn read_with(kernel: &impl Kernel, socket: BorrowedFd<'_>) -> PeerRead {
    match kernel.peer_pidfd(socket) {
        Ok(pidfd) => through_pidfd(kernel, pidfd, PeerSource::PidFd),
        Err(error) if error.raw_os_error() == Some(libc::ENOPROTOOPT) => {
            through_peer_credentials(kernel, socket)
        }
        Err(error) => PeerRead::unverified(format!("SO_PEERPIDFD failed: {error}")),
    }
}

fn peer_exited() -> PeerRead {
    PeerRead::unverified("the peer process exited before it could be identified")
}

fn through_pidfd(kernel: &impl Kernel, pidfd: OwnedFd, source: PeerSource) -> PeerRead {
    let pid = match kernel.pidfd_pid(&pidfd) {
        Ok(Some(pid)) => pid,
        Ok(None) => return peer_exited(),
        Err(error) => return PeerRead::unverified(format!("reading the peer's pidfd: {error}")),
    };
    let stat = match kernel.stat(pid) {
        Ok(stat) => stat,
        Err(error) => return PeerRead::unverified(format!("reading /proc/{pid}/stat: {error}")),
    };
    match kernel.pidfd_pid(&pidfd) {
        Ok(Some(again)) if again == pid => {}
        Ok(_) => return peer_exited(),
        Err(error) => return PeerRead::unverified(format!("reading the peer's pidfd: {error}")),
    }
    if stat.exited() {
        return peer_exited();
    }
    PeerRead::Identified(PeerIdentity {
        pid,
        started_at: stat.started_at,
        executable: kernel.executable(pid),
        source,
        _anchor: Some(pidfd),
    })
}

fn through_peer_credentials(kernel: &impl Kernel, socket: BorrowedFd<'_>) -> PeerRead {
    let pid = match kernel.peer_cred_pid(socket) {
        Ok(pid) => pid,
        Err(error) => return PeerRead::unverified(format!("SO_PEERCRED failed: {error}")),
    };
    let first = match kernel.stat(pid) {
        Ok(stat) => stat,
        Err(error) => return PeerRead::unverified(format!("reading /proc/{pid}/stat: {error}")),
    };
    let pidfd = match kernel.pidfd_open(pid) {
        Ok(pidfd) => pidfd,
        Err(error) => return PeerRead::unverified(format!("pidfd_open({pid}) failed: {error}")),
    };
    match through_pidfd(kernel, pidfd, PeerSource::PeerCred) {
        PeerRead::Identified(identity) if identity.started_at != first.started_at => {
            PeerRead::unverified(format!(
                "PID {pid} names another process than the one that connected"
            ))
        }
        confirmed => confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::fd::AsFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    struct Fallback {
        stat_reads: Cell<u32>,
        reused_after_the_first_read: bool,
    }

    impl Fallback {
        fn honest() -> Self {
            Self {
                stat_reads: Cell::new(0),
                reused_after_the_first_read: false,
            }
        }

        fn reused_after_the_first_read() -> Self {
            Self {
                stat_reads: Cell::new(0),
                reused_after_the_first_read: true,
            }
        }
    }

    impl Kernel for Fallback {
        fn peer_pidfd(&self, _: BorrowedFd<'_>) -> io::Result<OwnedFd> {
            Err(io::Error::from_raw_os_error(libc::ENOPROTOOPT))
        }

        fn peer_cred_pid(&self, socket: BorrowedFd<'_>) -> io::Result<u32> {
            LiveKernel.peer_cred_pid(socket)
        }

        fn pidfd_pid(&self, pidfd: &OwnedFd) -> io::Result<Option<u32>> {
            LiveKernel.pidfd_pid(pidfd)
        }

        fn pidfd_open(&self, pid: u32) -> io::Result<OwnedFd> {
            LiveKernel.pidfd_open(pid)
        }

        fn stat(&self, pid: u32) -> io::Result<Stat> {
            let mut stat = LiveKernel.stat(pid)?;
            let earlier_reads = self.stat_reads.replace(self.stat_reads.get() + 1);
            if self.reused_after_the_first_read && earlier_reads > 0 {
                stat.started_at += 1;
            }
            Ok(stat)
        }

        fn executable(&self, pid: u32) -> Option<PathBuf> {
            LiveKernel.executable(pid)
        }
    }

    fn connected_pair() -> (tempfile::TempDir, UnixStream, UnixStream) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peer.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        (dir, client, server)
    }

    fn identified(read: PeerRead) -> PeerIdentity {
        match read {
            PeerRead::Identified(identity) => identity,
            PeerRead::Unverified { reason } => panic!("unverified: {reason}"),
        }
    }

    #[test]
    fn a_peer_on_this_kernel_is_read_through_its_pidfd_with_start_time_and_executable() {
        let (_dir, _client, server) = connected_pair();
        let identity = identified(read(server.as_fd()));
        assert_eq!(identity.source(), PeerSource::PidFd);
        assert_eq!(identity.pid(), std::process::id());
        assert_eq!(
            Some(identity.started_at()),
            crate::process_start_time(std::process::id())
        );
        assert_eq!(
            identity.executable().map(std::path::Path::to_path_buf),
            std::env::current_exe().ok()
        );
    }

    #[test]
    fn without_so_peerpidfd_the_peer_comes_from_so_peercred_and_proc_stat() {
        let (_dir, _client, server) = connected_pair();
        let identity = identified(read_with(&Fallback::honest(), server.as_fd()));
        assert_eq!(identity.source(), PeerSource::PeerCred);
        assert_eq!(identity.pid(), std::process::id());
        assert_eq!(
            Some(identity.started_at()),
            crate::process_start_time(std::process::id())
        );
    }

    #[test]
    fn a_fallback_whose_pidfd_sees_another_start_time_is_unverified() {
        let (_dir, _client, server) = connected_pair();
        let read = read_with(&Fallback::reused_after_the_first_read(), server.as_fd());
        assert!(
            matches!(&read, PeerRead::Unverified { reason } if reason.contains("another process")),
            "{read:?}"
        );
    }

    #[test]
    fn two_connections_from_one_process_read_as_equal_identities() {
        let (_dir, _client, server) = connected_pair();
        let (_other_dir, _other_client, other_server) = connected_pair();
        assert_eq!(
            identified(read(server.as_fd())),
            identified(read(other_server.as_fd()))
        );
    }

    #[test]
    fn a_peer_that_exited_before_the_read_is_unverified_with_the_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let address = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork failed");
        if child == 0 {
            unsafe {
                let socket = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                let mut target: libc::sockaddr_un = std::mem::zeroed();
                target.sun_family = libc::AF_UNIX as libc::sa_family_t;
                for (slot, byte) in target.sun_path.iter_mut().zip(address.as_bytes()) {
                    *slot = *byte as libc::c_char;
                }
                libc::connect(
                    socket,
                    (&raw const target).cast(),
                    std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
                );
                libc::_exit(0);
            }
        }
        let (server, _) = listener.accept().unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        for kernel_read in [
            read(server.as_fd()),
            read_with(&Fallback::honest(), server.as_fd()),
        ] {
            assert!(
                matches!(kernel_read, PeerRead::Unverified { .. }),
                "{kernel_read:?}"
            );
        }
    }

    #[test]
    fn proc_stat_fields_survive_hostile_comm_names() {
        let plain = "1234 (zsh) S 1 1234 1234 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 9876543 0 0";
        let hostile =
            "1234 (a) b) c) R 77 1234 1200 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 424242 0";
        assert_eq!(
            parse_stat(plain),
            Some(Stat {
                state: 'S',
                parent: 1,
                session: 1234,
                started_at: 9_876_543
            })
        );
        assert_eq!(parse_stat(hostile).map(|stat| stat.parent), Some(77));
        assert_eq!(parse_stat(hostile).map(|stat| stat.session), Some(1200));
        assert_eq!(
            parse_stat(hostile).map(|stat| stat.started_at),
            Some(424_242)
        );
        let split_comm =
            "4321 (evil) (x) S 1 4321 4321 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 777777 123 456";
        assert_eq!(
            parse_stat(split_comm).map(|stat| stat.started_at),
            Some(777_777)
        );
        assert_eq!(parse_stat("1234 (zsh) S 1 1234"), None);
        assert_eq!(parse_stat(""), None);
    }

    #[test]
    fn a_process_whose_stat_is_readable_or_that_does_not_exist_is_not_hidden() {
        assert!(!LiveKernel.is_hidden(std::process::id()));
        assert!(!LiveKernel.is_hidden(0));
        assert!(!LiveKernel.is_hidden(u32::MAX));
    }
}
