#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod frame;
pub mod handshake;
pub mod limits;
pub mod message;
pub mod navigation;
pub mod profile;

pub use frame::{FrameError, decode, encode, read_message, write_message};
pub use handshake::{Hello, PROTOCOL_VERSION, Refusal, Welcome, check_version};
pub use message::{
    Command, CommandError, ConsoleLevel, Event, ExitReason, FrameDescriptor, HostMessage,
    OriginDecision, Outcome, Permission, PixelFormat, Reply, RequestId, TabId, WindowMessage,
};
pub use navigation::NavigationRefusal;
pub use profile::{InvalidProfileName, ProfileName};

pub const HOST_BINARY_NAME: &str = "paneflow-browser-host";

pub const ENDPOINT_ARG: &str = "--paneflow-endpoint";

pub const PROFILES_ARG: &str = "--paneflow-profiles";

pub const READY_LINE: &str = "paneflow-browser-host ready";
