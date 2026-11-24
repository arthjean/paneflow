use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::membership::{ProcessFacts, ProcessTable};
use crate::{PeerIdentity, PeerRead, PeerSource};

const LOCAL_PEERTOKEN: libc::c_int = 0x006;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4 * 1024;
const AUDIT_TOKEN_PID: usize = 5;
const KINFO_PROC_BYTES: usize = 648;
const KINFO_START_SECONDS: usize = 0;
const KINFO_START_MICROS: usize = 8;
const KINFO_PID: usize = 40;
const KINFO_PARENT: usize = 560;

#[repr(C)]
#[derive(Clone, Copy)]
struct AuditToken {
    val: [u32; 8],
}

unsafe extern "C" {
    fn proc_pidpath_audittoken(
        token: *mut AuditToken,
        buffer: *mut libc::c_void,
        size: u32,
    ) -> libc::c_int;
}

pub(crate) fn bsd_info(pid: i32) -> io::Result<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    if written == size {
        return Ok(info);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Err(io::Error::new(io::ErrorKind::NotFound, error));
    }
    Err(error)
}

pub(crate) struct KernelProcess {
    pub(crate) parent: u32,
    pub(crate) started_at: u64,
}

fn field<const N: usize>(buffer: &[u8], offset: usize) -> io::Result<[u8; N]> {
    buffer
        .get(offset..offset + N)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a short kinfo_proc record"))
}

pub(crate) fn kernel_process(pid: i32) -> io::Result<KernelProcess> {
    let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buffer = [0u8; KINFO_PROC_BYTES];
    let mut length = buffer.len();
    let status = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no process {pid}"),
        ));
    }
    if length != KINFO_PROC_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("kinfo_proc for {pid} has {length} bytes, not {KINFO_PROC_BYTES}"),
        ));
    }
    if i32::from_ne_bytes(field(&buffer, KINFO_PID)?) != pid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("kinfo_proc for {pid} names another process"),
        ));
    }
    let seconds = i64::from_ne_bytes(field(&buffer, KINFO_START_SECONDS)?);
    let micros = i32::from_ne_bytes(field(&buffer, KINFO_START_MICROS)?);
    let parent = i32::from_ne_bytes(field(&buffer, KINFO_PARENT)?);
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "a negative kinfo_proc field");
    Ok(KernelProcess {
        parent: u32::try_from(parent).map_err(|_| invalid())?,
        started_at: u64::try_from(seconds)
            .map_err(|_| invalid())?
            .saturating_mul(1_000_000)
            + u64::try_from(micros).map_err(|_| invalid())?,
    })
}

pub(crate) fn started_at(info: &libc::proc_bsdinfo) -> u64 {
    info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec
}

fn audit_token(socket: BorrowedFd<'_>) -> io::Result<AuditToken> {
    let mut token = AuditToken { val: [0; 8] };
    let mut length = std::mem::size_of::<AuditToken>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_LOCAL,
            LOCAL_PEERTOKEN,
            (&raw mut token).cast(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<AuditToken>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("LOCAL_PEERTOKEN returned {length} bytes"),
        ));
    }
    Ok(token)
}

fn token_executable(token: &AuditToken) -> io::Result<PathBuf> {
    let mut token = *token;
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let written = unsafe {
        proc_pidpath_audittoken(
            &raw mut token,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if written <= 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(OsString::from_vec(buffer)))
}

pub(crate) fn read(socket: BorrowedFd<'_>) -> PeerRead {
    let token = match audit_token(socket) {
        Ok(token) => token,
        Err(error) => return PeerRead::unverified(format!("LOCAL_PEERTOKEN failed: {error}")),
    };
    let pid = token.val[AUDIT_TOKEN_PID];
    let Some(signed) = i32::try_from(pid).ok().filter(|pid| *pid > 0) else {
        return PeerRead::unverified("the audit token names no peer process");
    };
    let started = match bsd_info(signed) {
        Ok(info) => started_at(&info),
        Err(error) => return PeerRead::unverified(format!("reading process {pid}: {error}")),
    };
    let executable = match token_executable(&token) {
        Ok(path) => path,
        Err(error) => {
            return PeerRead::unverified(format!(
                "the peer process exited before it could be identified: {error}"
            ));
        }
    };
    PeerRead::Identified(PeerIdentity {
        pid,
        started_at: started,
        executable: Some(executable),
        source: PeerSource::AuditToken,
        _anchor: None,
    })
}

pub(crate) struct LiveTable;

impl ProcessTable for LiveTable {
    fn facts(&self, pid: u32) -> io::Result<ProcessFacts> {
        let signed = i32::try_from(pid)
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no process {pid}")))?;
        let (parent, started) = match bsd_info(signed) {
            Ok(info) if info.pbi_status == libc::SZOMB => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("process {pid} has exited"),
                ));
            }
            Ok(info) => (info.pbi_ppid, started_at(&info)),
            Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                let process = kernel_process(signed)?;
                (process.parent, process.started_at)
            }
            Err(error) => return Err(error),
        };
        let session = unsafe { libc::getsid(signed) };
        if session < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Err(io::Error::new(io::ErrorKind::NotFound, error));
            }
            return Err(error);
        }
        Ok(ProcessFacts {
            parent,
            session: session as u32,
            started_at: started,
        })
    }

    fn describe(&self, pid: u32) -> (Option<String>, Option<PathBuf>) {
        let Ok(signed) = i32::try_from(pid) else {
            return (None, None);
        };
        let name = bsd_info(signed).ok().and_then(|info| {
            let bytes: Vec<u8> = info
                .pbi_comm
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| *byte as u8)
                .collect();
            (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
        });
        let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
        let written = unsafe {
            libc::proc_pidpath(
                signed,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                PROC_PIDPATHINFO_MAXSIZE as u32,
            )
        };
        let executable = usize::try_from(written)
            .ok()
            .filter(|written| *written > 0)
            .map(|written| {
                buffer.truncate(written);
                PathBuf::from(OsString::from_vec(buffer))
            });
        (name, executable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn a_peer_is_read_from_its_audit_token_with_start_time_and_executable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peer.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        let PeerRead::Identified(identity) = read(server.as_fd()) else {
            panic!("this process is identified");
        };
        assert_eq!(identity.source(), PeerSource::AuditToken);
        assert_eq!(identity.pid(), std::process::id());
        assert_eq!(
            Some(identity.started_at()),
            crate::process_start_time(std::process::id())
        );
        let expected = std::env::current_exe().unwrap().canonicalize().unwrap();
        let read = identity.executable().unwrap().canonicalize().unwrap();
        assert_eq!(read, expected);
    }

    #[test]
    fn a_descriptor_that_is_no_local_socket_is_unverified_without_a_pid_fallback() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let read = read(file.as_fd());
        assert!(
            matches!(&read, PeerRead::Unverified { reason } if reason.starts_with("LOCAL_PEERTOKEN failed")),
            "{read:?}"
        );
    }

    #[test]
    fn the_kernel_process_record_agrees_with_proc_pidinfo() {
        let pid = std::process::id();
        let record = kernel_process(pid as i32).unwrap();
        assert_eq!(record.parent, std::os::unix::process::parent_id());
        assert_eq!(Some(record.started_at), crate::process_start_time(pid));
    }

    #[test]
    fn launchd_owned_by_root_is_still_read_as_the_root_of_every_chain() {
        let facts = LiveTable.facts(1).unwrap();
        assert_eq!(facts.parent, 0);
        assert!(facts.started_at > 0);
    }

    #[test]
    fn sysctl_kern_proc_names_the_parent_of_another_process_of_the_user_in_its_own_session() {
        use std::os::unix::process::CommandExt as _;
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30");
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        let record = kernel_process(pid as i32);
        let facts = LiveTable.facts(pid);
        let start = crate::process_start_time(pid);
        let _ = child.kill();
        let _ = child.wait();
        let record = record.unwrap();
        assert_eq!(record.parent, std::process::id());
        assert_eq!(Some(record.started_at), start);
        let facts = facts.unwrap();
        assert_eq!(facts.parent, std::process::id());
        assert_eq!(facts.session, pid);
    }

    #[test]
    fn sysctl_kern_proc_agrees_with_proc_pidinfo_on_every_process_both_can_read() {
        let mut pids = vec![0i32; 8192];
        let bytes = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        assert!(count > 0, "{}", io::Error::last_os_error());
        pids.truncate(count as usize);
        let (mut compared, mut sysctl_only) = (0, 0);
        for pid in pids.into_iter().filter(|pid| *pid > 0) {
            let Ok(record) = kernel_process(pid) else {
                continue;
            };
            let read = bsd_info(pid);
            let unchanged = kernel_process(pid).is_ok_and(|again| {
                again.parent == record.parent && again.started_at == record.started_at
            });
            if !unchanged {
                continue;
            }
            match read {
                Ok(info) => {
                    compared += 1;
                    assert_eq!(record.parent, info.pbi_ppid, "parent of {pid}");
                    assert_eq!(record.started_at, started_at(&info), "start of {pid}");
                }
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => sysctl_only += 1,
                Err(_) => {}
            }
        }
        assert!(compared > 0);
        eprintln!(
            "kinfo_proc agreed with proc_pidinfo on {compared} processes; {sysctl_only} were readable through sysctl only"
        );
    }

    #[test]
    fn this_process_has_parent_session_and_start_time_facts() {
        let facts = LiveTable.facts(std::process::id()).unwrap();
        assert_eq!(facts.parent, std::os::unix::process::parent_id());
        assert_eq!(
            Some(facts.started_at),
            crate::process_start_time(std::process::id())
        );
    }
}
