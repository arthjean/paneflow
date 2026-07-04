//! `CLAUDECODE` env scrub helpers.
//!
//! The ACP agent-spawn machinery (`spawn_acp_agent`, wire tracing,
//! secret redaction) was removed with the in-app chat; only this env
//! scrub survives, called once from `main()` before any thread spawns.

/// Environment variable set by a running Claude Code process. If
/// inherited by a child `claude` / `claude-code-acp` wrapper, the
/// wrapper refuses to launch ("Claude Code cannot launch inside another
/// Claude Code session").
const CLAUDECODE_ENV: &str = "CLAUDECODE";

/// Remove `CLAUDECODE` from the current process environment so future
/// subprocesses (which inherit it by default) do not see it.
///
/// US-011 (cli-hardening-followup-2026-Q3): this helper MUST be
/// called from the very first lines of `main()`, before any
/// `std::thread::spawn`, `tokio::runtime::Builder::build`, or smol
/// executor initialization. Rust 1.85 made `std::env::remove_var`
/// `unsafe` because it races with concurrent `getenv` from any
/// other thread; the runtime sub-systems above all read env on
/// startup, so calling this before any thread exists is genuinely safe
/// by construction.
/// # Safety
///
/// Must be called before any other thread, async runtime, or foreign library
/// can concurrently read environment variables. Prefer
/// [`scrub_claudecode_from_command`] for per-child scrubbing after startup.
pub unsafe fn scrub_claudecode_env() {
    // SAFETY: called from main() before any thread::spawn or async
    // runtime init (US-011) -- no concurrent getenv possible.
    unsafe {
        std::env::remove_var(CLAUDECODE_ENV);
    }
}

/// Remove `CLAUDECODE` from one child command without mutating global process
/// environment.
pub fn scrub_claudecode_from_command(command: &mut std::process::Command) {
    command.env_remove(CLAUDECODE_ENV);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_claudecode_is_idempotent() {
        // SAFETY: test-only -- single-threaded test runner step. Sets, scrubs,
        // and re-scrubs to confirm the second call does not panic.
        unsafe {
            std::env::set_var(CLAUDECODE_ENV, "1");
        }
        // SAFETY: this test holds no extra threads and only exercises this env var.
        unsafe { scrub_claudecode_env() };
        assert!(std::env::var(CLAUDECODE_ENV).is_err());
        // SAFETY: same as above.
        unsafe { scrub_claudecode_env() };
        assert!(std::env::var(CLAUDECODE_ENV).is_err());
    }

    #[test]
    fn scrub_claudecode_from_command_is_local_to_child() {
        let mut command = std::process::Command::new("noop");
        command.env(CLAUDECODE_ENV, "1");
        scrub_claudecode_from_command(&mut command);
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == CLAUDECODE_ENV && value.is_none()),
            "child command should explicitly remove CLAUDECODE"
        );
    }
}
