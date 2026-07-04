#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unwrap_in_result,
    )
)]
//! paneflow-acp: agent identity + the `CLAUDECODE` env scrub.
//!
//! The in-app ACP chat, the PATH discovery scanner, and the auth/sign-in
//! probing were all removed when the Agents view became terminal-only
//! (each agent self-authenticates in its own launched terminal). What
//! remains is the minimal surface the app still references:
//!
//! - [`discovery::AgentKind`] - identity enum for legacy `Thread.agent`
//!   metadata.
//! - [`spawn::scrub_claudecode_env`] - unsafe process-wide startup scrub.
//! - [`spawn::scrub_claudecode_from_command`] - safe per-child scrub.

pub mod discovery;
pub mod spawn;

pub use discovery::AgentKind;
pub use spawn::{scrub_claudecode_env, scrub_claudecode_from_command};
