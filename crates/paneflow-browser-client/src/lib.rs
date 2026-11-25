#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

mod binary;
mod environment;
mod supervisor;
mod transport;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

use paneflow_browser_proto::limits::HOST_IDLE_EXIT;
use paneflow_browser_proto::{CommandError, Event, ExitReason, NavigationRefusal, Reply};

pub use binary::{
    CURRENT_RUNTIME_DIR_NAME, HOST_BINARY_ENV, dev_build_output, installed_runtime_binary,
    resolve_host_binary, resolve_host_binary_with,
};
pub use environment::host_environment;
pub use supervisor::BrowserClient;

pub const HOST_START_DEADLINE: Duration = Duration::from_secs(60);

pub const HOST_STOPPED_MESSAGE: &str = "The browser engine stopped. Reload to restart it.";

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub idle_exit: Duration,
    pub start_deadline: Duration,
    pub window_environment: Vec<(OsString, OsString)>,
}

impl HostConfig {
    pub fn for_home(home: PathBuf) -> Self {
        Self {
            binary: resolve_host_binary(&home),
            home,
            idle_exit: HOST_IDLE_EXIT,
            start_deadline: HOST_START_DEADLINE,
            window_environment: std::env::vars_os().collect(),
        }
    }

    pub fn profiles_dir(&self) -> PathBuf {
        paneflow_home::browser_profiles_dir_in(&self.home)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub pid: u32,
    pub engine: String,
    pub sandbox: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    HostStarted { pid: u32 },
    HostFailed { reason: String },
    HostExited { reason: ExitReason },
    HostCrashed { reason: String },
    Browser(Event),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabStatus {
    Starting,
    Loading {
        url: String,
    },
    Loaded {
        url: String,
        http_status: i32,
    },
    AwaitingDecision {
        origin: String,
        url: String,
    },
    Refused {
        url: String,
        refusal: NavigationRefusal,
    },
    Crashed {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

pub struct Ticket(Receiver<Result<Reply, CommandError>>);

impl Ticket {
    fn new(receiver: Receiver<Result<Reply, CommandError>>) -> Self {
        Self(receiver)
    }

    pub fn try_take(&self) -> Option<Result<Reply, CommandError>> {
        match self.0.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(lost_answer())),
        }
    }

    pub fn wait(self, deadline: Duration) -> Result<Reply, CommandError> {
        match self.0.recv_timeout(deadline) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(CommandError::HostUnavailable {
                reason: format!("no answer within {} ms", deadline.as_millis()),
            }),
            Err(RecvTimeoutError::Disconnected) => Err(lost_answer()),
        }
    }
}

fn lost_answer() -> CommandError {
    CommandError::HostUnavailable {
        reason: "the browser client dropped the request".to_string(),
    }
}
