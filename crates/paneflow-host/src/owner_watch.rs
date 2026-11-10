use std::time::Duration;

use crate::process::{ProcessIdentity, ProcessVerdict};

const IDENTITY_POLL: Duration = Duration::from_millis(250);

pub struct OwnerWatch {
    owner: ProcessIdentity,
    exit: Option<imp::ExitSignal>,
}

impl OwnerWatch {
    pub fn arm(owner: u32) -> Self {
        let identity = ProcessIdentity::capture(owner);
        let exit = match imp::ExitSignal::open(owner) {
            Ok(exit) => Some(exit),
            Err(error) => {
                log::warn!(
                    "paneflow-host: owner pid={owner} has no exit notification ({error}); polling its identity instead"
                );
                None
            }
        };
        Self {
            owner: identity,
            exit,
        }
    }

    pub fn owner(&self) -> u32 {
        self.owner.pid
    }

    pub fn wait_for_exit(&self) {
        if let Some(exit) = &self.exit {
            match exit.wait() {
                Ok(()) => return,
                Err(error) => log::warn!(
                    "paneflow-host: waiting on owner pid={} failed ({error}); polling its identity instead",
                    self.owner.pid
                ),
            }
        }
        while self.owner.verify() != ProcessVerdict::Gone {
            std::thread::sleep(IDENTITY_POLL);
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub struct ExitSignal(HANDLE);

    unsafe impl Send for ExitSignal {}

    impl ExitSignal {
        pub fn open(pid: u32) -> io::Result<Self> {
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }

        pub fn wait(&self) -> io::Result<()> {
            if unsafe { WaitForSingleObject(self.0, INFINITE) } == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }

    impl Drop for ExitSignal {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub struct ExitSignal(OwnedFd);

    impl ExitSignal {
        pub fn open(pid: u32) -> io::Result<Self> {
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
        }

        pub fn wait(&self) -> io::Result<()> {
            let mut readiness = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                if unsafe { libc::poll(&mut readiness, 1, -1) } > 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub struct ExitSignal {
        queue: OwnedFd,
        exited_before_arming: bool,
    }

    impl ExitSignal {
        pub fn open(pid: u32) -> io::Result<Self> {
            let queue = unsafe { libc::kqueue() };
            if queue < 0 {
                return Err(io::Error::last_os_error());
            }
            let queue = unsafe { OwnedFd::from_raw_fd(queue) };
            let change = libc::kevent {
                ident: pid as libc::uintptr_t,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let registered = unsafe {
                libc::kevent(
                    queue.as_raw_fd(),
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if registered < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(Self {
                queue,
                exited_before_arming: registered < 0,
            })
        }

        pub fn wait(&self) -> io::Result<()> {
            if self.exited_before_arming {
                return Ok(());
            }
            loop {
                let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                let received = unsafe {
                    libc::kevent(
                        self.queue.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if received > 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use std::io;

    pub struct ExitSignal;

    impl ExitSignal {
        pub fn open(_pid: u32) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no process exit notification on this platform",
            ))
        }

        pub fn wait(&self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no process exit notification on this platform",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn short_lived_child() -> std::process::Child {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "ping -n 2 127.0.0.1 >NUL"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = std::process::Command::new("sleep");
            command.arg("1");
            command
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn the_watch_returns_once_the_owner_exits_and_not_before() {
        let mut child = short_lived_child();
        let watch = OwnerWatch::arm(child.id());
        assert!(watch.exit.is_some(), "the platform notifies process exit");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            watch.wait_for_exit();
            let _ = sender.send(());
        });
        assert!(
            receiver.recv_timeout(Duration::from_millis(200)).is_err(),
            "the watch is still waiting on a live owner"
        );
        child.wait().unwrap();
        assert!(
            receiver.recv_timeout(Duration::from_secs(30)).is_ok(),
            "the watch observes the owner's exit"
        );
    }

    #[test]
    fn a_watch_on_an_owner_already_gone_returns() {
        let mut child = short_lived_child();
        let pid = child.id();
        child.wait().unwrap();
        let watch = OwnerWatch::arm(pid);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            watch.wait_for_exit();
            let _ = sender.send(());
        });
        assert!(
            receiver.recv_timeout(Duration::from_secs(30)).is_ok(),
            "an owner gone before arming ends the watch"
        );
    }
}
