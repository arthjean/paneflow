//! Per-agent config writers (EP-002 trait + registry; EP-003 fills in the
//! concrete writers).
//!
//! Each supported agent (Claude Code, Codex, Gemini CLI, opencode)
//! implements [`AgentConfigWriter`]. The orchestrator in
//! [`crate::cli`] iterates [`default_writers`], gating each call on
//! [`AgentConfigWriter::presence`] so an absent agent is reported as
//! skipped without ever touching the filesystem.
//!
//! The trait deliberately splits the three verbs so each is independently
//! testable, and returns structured outcomes (not pre-formatted strings) so
//! the CLI owns presentation and the exit-code policy in one place.

use std::path::Path;

use anyhow::Result;

use crate::detect::Presence;

pub mod claude_code;
pub mod codex;
pub mod gemini;
pub mod opencode;
mod support;

#[cfg(test)]
pub(crate) mod testutil;

/// Result of an `install` on one agent. "Skipped because absent" is *not*
/// here - the orchestrator decides that from [`AgentConfigWriter::presence`]
/// before calling `install`, so writers only model the write itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// A `paneflow` entry was created where none existed.
    Installed,
    /// An existing `paneflow` entry's command path was updated.
    Updated,
    /// The entry already pointed at the current bridge path - no write.
    AlreadyCurrent,
}

/// Result of an `uninstall` on one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallOutcome {
    /// The `paneflow` entry was removed.
    Removed,
    /// No `paneflow` entry was present - nothing to do.
    NothingToRemove,
}

/// Result of a read-only `status` probe on one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusOutcome {
    /// A `paneflow` entry exists and points at the current bridge path.
    Installed { path: String },
    /// A `paneflow` entry exists but points at a different path than the
    /// current `bridge_binary_path()` - typically a stale path left by an
    /// older Paneflow install that moved data dirs.
    StalePath { found: String, expected: String },
    /// A `paneflow` entry exists but does not match the managed schema.
    NeedsRepair {
        path: Option<String>,
        reason: String,
    },
    /// The agent is present but carries no `paneflow` entry.
    NotInstalled,
}

/// One agent's config surface: how to detect it and how to install /
/// uninstall / inspect the `paneflow` MCP entry.
///
/// Implementations live in `agents/<name>.rs` (EP-003). They must be
/// idempotent and no-clobber - see [`crate::merge`] and [`crate::io`] for
/// the shared safe-write primitives every writer is expected to use.
pub trait AgentConfigWriter {
    /// Stable machine id, e.g. `"claude-code"`. Used in scriptable output.
    fn id(&self) -> &'static str;

    /// Human-readable label, e.g. `"Claude Code"`.
    fn label(&self) -> &'static str;

    /// Is this agent installed on the machine (CLI on PATH or config
    /// present)? The orchestrator skips absent agents.
    fn presence(&self) -> Presence;

    /// Register or refresh the `paneflow` entry pointing at `bridge_path`.
    /// Must be idempotent (re-run with the same path = `AlreadyCurrent`).
    fn install(&self, bridge_path: &Path) -> Result<InstallOutcome>;

    /// Remove only the `paneflow` entry, leaving every other entry intact.
    fn uninstall(&self) -> Result<UninstallOutcome>;

    /// Inspect the current `paneflow` entry without writing. `bridge_path`
    /// is the path the entry should point at, used to flag a stale path
    /// when the caller can resolve it.
    fn status(&self, bridge_path: Option<&Path>) -> Result<StatusOutcome>;
}

/// The registry of concrete writers driven by `paneflow mcp <cmd>`.
///
/// EP-003 wires all four supported agents. The orchestrator skips any that
/// are not present on the machine, so listing them here unconditionally is
/// safe - detection happens per-agent at run time.
#[must_use]
pub fn default_writers() -> Vec<Box<dyn AgentConfigWriter>> {
    vec![
        Box::new(claude_code::ClaudeCode::new()),
        Box::new(codex::Codex::new()),
        Box::new(gemini::Gemini::new()),
        Box::new(opencode::OpenCode::new()),
    ]
}
