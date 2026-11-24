use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use interprocess::local_socket::{Listener, Stream, prelude::*};
use paneflow_api::Principal;
use paneflow_home::PaneServer;

use crate::server::ConnectionGuard;

pub const CONNECTIONS_PER_PANE_ENDPOINT: usize = 16;

pub type ServeStream = Arc<dyn Fn(Stream, Principal) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneIdentity {
    pub session: String,
    pub workspace: Option<String>,
}

impl PaneIdentity {
    pub fn principal(&self) -> Principal {
        Principal::Pane {
            session: self.session.clone(),
            workspace: self.workspace.clone(),
        }
    }
}

pub struct PaneListener {
    endpoint: PathBuf,
    identity: PaneIdentity,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PaneListener {
    pub fn open(
        home: &Path,
        server: PaneServer,
        identity: PaneIdentity,
        serve: ServeStream,
    ) -> io::Result<Self> {
        let endpoint = paneflow_home::pane_endpoint_path(home, &identity.session, server);
        let listener = bind_pane_endpoint(home, &identity.session, &endpoint, server)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let principal = identity.principal();
        let retired = endpoint.clone();
        let thread = std::thread::Builder::new()
            .name(format!("paneflow-pane-{}-accept", server.stem()))
            .spawn(move || {
                accept_pane_connections(&listener, &flag, &principal, &serve);
                crate::server::retire_listener(listener, &retired);
            })?;
        log::debug!(
            "paneflow: the {} endpoint of sandboxed pane {} listens on {}",
            server.stem(),
            identity.session,
            endpoint.display()
        );
        Ok(Self {
            endpoint,
            identity,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    pub fn identity(&self) -> &PaneIdentity {
        &self.identity
    }

    fn endpoint_present(&self) -> bool {
        cfg!(windows) || self.endpoint.exists()
    }
}

impl Drop for PaneListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        #[cfg(windows)]
        wake(&self.endpoint);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        log::debug!(
            "paneflow: closed the endpoint {} of pane {}",
            self.endpoint.display(),
            self.identity.session
        );
    }
}

#[cfg(windows)]
fn wake(endpoint: &Path) {
    use interprocess::ConnectWaitMode;
    use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode};
    let _ = DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path_with_wait_mode(
        endpoint.as_os_str(),
        ConnectWaitMode::Timeout(std::time::Duration::from_millis(250)),
    );
}

#[cfg(unix)]
const ACCEPT_POLL: std::time::Duration = std::time::Duration::from_millis(200);

#[cfg(unix)]
fn next_stream(listener: &Listener, shutdown: &AtomicBool) -> Option<io::Result<Stream>> {
    use interprocess::local_socket::ListenerNonblockingMode;
    use std::os::fd::{AsFd, AsRawFd};
    if let Err(error) = listener.set_nonblocking(ListenerNonblockingMode::Accept) {
        return Some(Err(error));
    }
    let Listener::UdSocket(socket) = listener;
    loop {
        if shutdown.load(Ordering::Acquire) {
            return None;
        }
        let mut pollfd = libc::pollfd {
            fd: socket.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = libc::c_int::try_from(ACCEPT_POLL.as_millis()).unwrap_or(libc::c_int::MAX);
        if unsafe { libc::poll(&mut pollfd, 1, timeout) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Some(Err(error));
            }
            continue;
        }
        match listener.accept() {
            Ok(stream) => {
                return Some(stream.set_nonblocking(false).map(|()| stream));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Some(Err(error)),
        }
    }
}

#[cfg(windows)]
fn next_stream(listener: &Listener, shutdown: &AtomicBool) -> Option<io::Result<Stream>> {
    let accepted = listener.accept();
    if shutdown.load(Ordering::Acquire) {
        return None;
    }
    Some(accepted)
}

fn accept_pane_connections(
    listener: &Listener,
    shutdown: &AtomicBool,
    principal: &Principal,
    serve: &ServeStream,
) {
    let active = Arc::new(AtomicUsize::new(0));
    while let Some(connection) = next_stream(listener, shutdown) {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("paneflow: a pane endpoint accept failed: {error}");
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
        };
        let Some(guard) =
            ConnectionGuard::try_acquire(Arc::clone(&active), CONNECTIONS_PER_PANE_ENDPOINT)
        else {
            log::debug!("paneflow: a pane endpoint refused a connection over its limit");
            continue;
        };
        let serve = Arc::clone(serve);
        let principal = principal.clone();
        let spawned = std::thread::Builder::new()
            .name("paneflow-pane-conn".into())
            .spawn(move || {
                let _guard = guard;
                serve(stream, principal);
            });
        if let Err(error) = spawned {
            log::warn!("paneflow: cannot start a pane connection thread: {error}");
        }
    }
}

fn bind_pane_endpoint(
    home: &Path,
    session: &str,
    endpoint: &Path,
    server: PaneServer,
) -> io::Result<Listener> {
    if !paneflow_home::is_pane_session_name(session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{session:?} cannot name a pane endpoint"),
        ));
    }
    #[cfg(unix)]
    {
        let dir = paneflow_home::pane_dir_in(home, session);
        if server == PaneServer::Host {
            paneflow_home::create_private_dir_all(&dir)?;
        } else if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{} does not exist; the host creates it while the pane lives",
                    dir.display()
                ),
            ));
        }
        crate::server::bind_owner_only(endpoint, server.stem())
    }
    #[cfg(windows)]
    {
        let _ = home;
        let sddl = pane_pipe_sddl(session).map_err(io::Error::other)?;
        crate::server::bind_with_sddl(endpoint, server.stem(), &sddl)
    }
}

#[cfg(windows)]
pub fn pane_pipe_sddl(session: &str) -> Result<String, String> {
    Ok(format!(
        "D:P(A;;GA;;;{})(A;;GRGW;;;{})",
        crate::sandbox::current_user_sid()?,
        paneflow_home::pane_sandbox_sid(session)
    ))
}

#[derive(Default)]
pub struct PaneEndpoints {
    open: Mutex<HashMap<String, PaneListener>>,
}

impl PaneEndpoints {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PaneListener>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_open(&self, session: &str) -> bool {
        self.lock().contains_key(session)
    }

    pub fn sessions(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    pub fn ensure(
        &self,
        home: &Path,
        server: PaneServer,
        identity: PaneIdentity,
        serve: &ServeStream,
    ) -> io::Result<()> {
        let mut open = self.lock();
        if open
            .get(&identity.session)
            .is_some_and(|listener| listener.identity == identity && listener.endpoint_present())
        {
            return Ok(());
        }
        let session = identity.session.clone();
        if let Some(previous) = open.remove(&session) {
            drop(open);
            drop(previous);
            open = self.lock();
        }
        let listener = PaneListener::open(home, server, identity, Arc::clone(serve))?;
        open.insert(session, listener);
        Ok(())
    }

    pub fn close(&self, session: &str) -> bool {
        let removed = self.lock().remove(session);
        removed.is_some()
    }

    pub fn retain(&self, live: impl Fn(&str) -> bool) -> Vec<String> {
        let mut open = self.lock();
        let ended: Vec<String> = open
            .keys()
            .filter(|session| !live(session))
            .cloned()
            .collect();
        let closed: Vec<PaneListener> = ended
            .iter()
            .filter_map(|session| open.remove(session))
            .collect();
        drop(open);
        drop(closed);
        ended
    }

    pub fn close_all(&self) {
        let closed: Vec<PaneListener> = self.lock().drain().map(|(_, listener)| listener).collect();
        drop(closed);
    }
}

pub fn remove_pane_dir(home: &Path, session: &str) {
    if !paneflow_home::is_pane_session_name(session) {
        return;
    }
    let dir = paneflow_home::pane_dir_in(home, session);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "paneflow: cannot remove the pane endpoint directory {}: {error}",
            dir.display()
        ),
    }
}

pub fn pane_dir_sessions(home: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(paneflow_home::panes_dir_in(home)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| paneflow_home::is_pane_session_name(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_whoami() -> ServeStream {
        Arc::new(|stream: Stream, principal: Principal| {
            use std::io::Write as _;
            let mut stream = stream;
            let _ = writeln!(stream, "{}", principal.kind());
        })
    }

    #[cfg(unix)]
    #[test]
    fn a_pane_endpoint_serves_its_pane_and_is_removed_when_closed() {
        use std::io::BufRead as _;
        let home = tempfile::tempdir().unwrap();
        let endpoints = PaneEndpoints::default();
        let identity = PaneIdentity {
            session: "a-session".into(),
            workspace: Some("w".into()),
        };
        endpoints
            .ensure(home.path(), PaneServer::Host, identity, &serve_whoami())
            .unwrap();
        let path = paneflow_home::pane_endpoint_path(home.path(), "a-session", PaneServer::Host);
        let stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
        let mut line = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "pane");
        assert!(endpoints.close("a-session"));
        assert!(!path.exists());
        assert!(!endpoints.is_open("a-session"));
    }

    #[cfg(unix)]
    #[test]
    fn only_the_host_creates_a_pane_directory() {
        let home = tempfile::tempdir().unwrap();
        let endpoints = PaneEndpoints::default();
        let identity = PaneIdentity {
            session: "gone".into(),
            workspace: None,
        };
        let refused = endpoints.ensure(
            home.path(),
            PaneServer::Worker,
            identity.clone(),
            &serve_whoami(),
        );
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(!paneflow_home::pane_dir_in(home.path(), "gone").exists());
        endpoints
            .ensure(
                home.path(),
                PaneServer::Host,
                identity.clone(),
                &serve_whoami(),
            )
            .unwrap();
        let window = PaneEndpoints::default();
        window
            .ensure(home.path(), PaneServer::Window, identity, &serve_whoami())
            .unwrap();
        assert!(
            paneflow_home::pane_endpoint_path(home.path(), "gone", PaneServer::Window).exists()
        );
    }

    #[test]
    fn a_session_name_that_is_not_one_token_opens_no_endpoint() {
        let home = tempfile::tempdir().unwrap();
        let refused = PaneEndpoints::default().ensure(
            home.path(),
            PaneServer::Worker,
            PaneIdentity {
                session: "../escape".into(),
                workspace: None,
            },
            &serve_whoami(),
        );
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
