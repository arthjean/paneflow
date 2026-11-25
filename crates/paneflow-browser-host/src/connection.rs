use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use paneflow_browser_proto::{
    CommandError, FrameError, Hello, HostMessage, Outcome, READY_LINE, Refusal, Welcome,
    WindowMessage, read_message, write_message,
};

use crate::ui::{dispatch, on_ui};

const PARENT_CONNECT_DEADLINE: Duration = Duration::from_secs(60);
const HELLO_DEADLINE: Duration = Duration::from_secs(30);

pub struct Accepted {
    stream: UnixStream,
    pub hello: Hello,
}

pub struct Running {
    pub out: Sender<HostMessage>,
    writer: JoinHandle<()>,
    stream: UnixStream,
    engine_running: Arc<AtomicBool>,
}

impl Running {
    pub fn stop_reading(&self) {
        self.engine_running.store(false, Ordering::SeqCst);
        let _ = self.stream.shutdown(std::net::Shutdown::Read);
    }

    pub fn finish(self) {
        self.stop_reading();
        drop(self.out);
        let _ = self.writer.join();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

pub fn accept_parent(endpoint: &Path) -> Result<Accepted, String> {
    if endpoint.exists() {
        std::fs::remove_file(endpoint)
            .map_err(|error| format!("cannot replace {}: {error}", endpoint.display()))?;
    }
    let listener = UnixListener::bind(endpoint)
        .map_err(|error| format!("cannot listen on {}: {error}", endpoint.display()))?;
    let _ = std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600));
    let parent = std::os::unix::process::parent_id();

    let connected = Arc::new(AtomicBool::new(false));
    let watchdog_flag = Arc::clone(&connected);
    let watched = endpoint.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("parent-watchdog".into())
        .spawn(move || {
            std::thread::sleep(PARENT_CONNECT_DEADLINE);
            if !watchdog_flag.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&watched);
                eprintln!("paneflow-browser-host: the parent window never connected");
                std::process::exit(2);
            }
        });

    {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{READY_LINE}");
        let _ = stdout.flush();
    }

    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        let peer = paneflow_peer::read_unix_peer(stream.as_fd());
        let is_parent = peer
            .identity()
            .is_some_and(|identity| identity.pid() == parent);
        if !is_parent {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            continue;
        }
        connected.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(endpoint);
        let _ = stream.set_read_timeout(Some(HELLO_DEADLINE));
        let hello = match read_message::<WindowMessage, _>(&mut stream) {
            Ok(WindowMessage::Hello(hello)) => hello,
            Ok(WindowMessage::Request { .. }) => {
                return Err("the window sent a request before its hello".to_string());
            }
            Err(error) => return Err(format!("the window's hello failed: {error}")),
        };
        let _ = stream.set_read_timeout(None);
        return Ok(Accepted { stream, hello });
    }
    Err("the endpoint stopped accepting connections".to_string())
}

pub fn refuse(accepted: &mut Accepted, refusal: Refusal) {
    let _ = write_message(&mut accepted.stream, &HostMessage::Refused(refusal));
    let _ = accepted.stream.shutdown(std::net::Shutdown::Both);
}

pub fn start(mut accepted: Accepted, sandbox: bool) -> Result<Running, String> {
    let welcome = HostMessage::Welcome(Welcome {
        protocol_version: paneflow_browser_proto::PROTOCOL_VERSION,
        host_pid: std::process::id(),
        engine: crate::app::engine_version(),
        sandbox,
    });
    write_message(&mut accepted.stream, &welcome)
        .map_err(|error| format!("cannot welcome the window: {error}"))?;

    let mut write_half = accepted
        .stream
        .try_clone()
        .map_err(|error| format!("cannot split the connection: {error}"))?;
    let mut read_half = accepted
        .stream
        .try_clone()
        .map_err(|error| format!("cannot split the connection: {error}"))?;

    let (out, outgoing) = mpsc::channel::<HostMessage>();
    let writer = std::thread::Builder::new()
        .name("protocol-writer".into())
        .spawn(move || {
            for message in outgoing {
                if send(&mut write_half, message).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| format!("cannot start the writer: {error}"))?;

    let engine_running = Arc::new(AtomicBool::new(true));
    let reading = Arc::clone(&engine_running);
    std::thread::Builder::new()
        .name("protocol-reader".into())
        .spawn(move || {
            loop {
                let message = read_message::<WindowMessage, _>(&mut read_half);
                if !reading.load(Ordering::SeqCst) {
                    return;
                }
                match message {
                    Ok(WindowMessage::Request { id, command }) => {
                        on_ui(move || dispatch(|host| host.handle(id, command)));
                    }
                    Ok(WindowMessage::Hello(_)) => {}
                    Err(FrameError::Malformed(_)) => {}
                    Err(_) => {
                        on_ui(|| {
                            dispatch(|host| {
                                host.begin_quit(paneflow_browser_proto::ExitReason::WindowGone)
                            })
                        });
                        return;
                    }
                }
            }
        })
        .map_err(|error| format!("cannot start the reader: {error}"))?;

    Ok(Running {
        out,
        writer,
        stream: accepted.stream,
        engine_running,
    })
}

fn send(stream: &mut UnixStream, message: HostMessage) -> Result<(), FrameError> {
    match write_message(stream, &message) {
        Err(FrameError::TooLarge { declared, max }) => match message {
            HostMessage::Reply { id, .. } => write_message(
                stream,
                &HostMessage::Reply {
                    id,
                    outcome: Outcome::Failed {
                        error: CommandError::Rejected {
                            reason: format!(
                                "the answer is {declared} bytes, over the {max}-byte protocol limit"
                            ),
                        },
                    },
                },
            ),
            _ => Ok(()),
        },
        other => other,
    }
}
