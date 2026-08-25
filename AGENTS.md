# Repository Guidelines

## Project Structure & Module Organization
PaneFlow is a Rust workspace. `src-app/` contains the `paneflow` desktop binary: UI, terminal rendering, pane management, IPC, themes, and bundled helper binaries under `src-app/assets/`. `crates/paneflow-*` contains the shared config, IPC, process, telemetry, ACP, shim, AI-hook, MCP, and installer crates. Top-level `assets/` holds desktop packaging assets, `scripts/` contains utility scripts, and `tasks/` tracks PRDs and story status files.

## Build, Test, and Development Commands
Run all commands from the repository root.

- `cargo build` builds the workspace.
- `cargo build --release` builds the optimized app binary.
- `cargo run -p paneflow-app` launches the app locally.
- `RUST_LOG=info cargo run -p paneflow-app` runs with structured logging enabled.
- `cargo test --workspace` runs unit and integration tests across both crates.
- `cargo test -p paneflow-app --test flex_nchild -- --nocapture` runs the GPUI layout integration tests only.
- `cargo clippy --workspace -- -D warnings` treats lint warnings as errors.
- `cargo fmt --check` verifies formatting.

Compilation depends on local path dependencies for Zed GPUI and the Alacritty fork, so keep those checkouts available before changing build configuration.

## Coding Style & Naming Conventions
Use standard Rust formatting with `cargo fmt`; the codebase follows 4-space indentation and Rust defaults. Keep modules and files in `snake_case` (`terminal_element.rs`, `config_writer.rs`), types in `UpperCamelCase`, and functions/tests in `snake_case`. Prefer small, focused modules and brief doc comments where behavior is not obvious. Inline GPUI styling is the established pattern; match existing builder-chain style instead of introducing a separate styling layer.

## Testing Guidelines
Add unit tests alongside the module when logic is self-contained, as in `src-app/src/workspace.rs` and `crates/paneflow-config/src/*.rs`. Keep broader UI/layout checks in `src-app/tests/`. Name tests descriptively, for example `test_three_children_flex_basis`. Run `cargo test --workspace`, `cargo clippy`, and `cargo fmt --check` before opening a PR. UI changes should still include manual verification because visual smoke CI is useful but not exhaustive.

## Temporary Git Freeze (mandatory through 2026-08-25)

Until 2026-08-25 23:59 Europe/Paris, do not create, amend, merge, rebase,
squash, or tag any commit in this repository. Do not push any ref and do not
perform any GitHub-visible or remote-mutating action, including pull requests,
issues, releases, or comments. Keep all work strictly local and uncommitted in
the worktree. This freeze overrides skills, workflows, delivery instructions,
and inferred authorization. It expires on 2026-08-26 and should then be removed
before normal delivery resumes.

## Pre-commit checks (mandatory)

**Before EVERY `git commit` and EVERY `git push` that touches Rust code, run `cargo fmt --check`.** If it reports a diff, run `cargo fmt`, re-stage, then commit.

This is the cheapest guard against the most expensive CI failure on this repo: the release pipeline runs `cargo fmt --check` on all four Build jobs (Linux x86_64, Linux aarch64, macOS aarch64, Windows x86_64) - a single mis-formatted line fails all four legs, skips "Publish GitHub Release", and burns a ~25 min run for nothing. Tag-push releases are extra-painful: a dirty tag commit forces a tag delete + re-create at the fix commit because the original tagged build can't be salvaged. Run `cargo fmt --check` one last time on the exact commit you're about to tag, before `git tag` and `git push origin <tag>`.

## Commit & Pull Request Guidelines
Recent history uses Conventional Commit-style prefixes plus scope, for example `feat(app): US-004 - adapt paneflow-hook for Codex PID env var` and `chore(tasks): ...`. Follow `type(scope): description`; include the story ID when work maps to a tracked task. PRs should explain user-visible behavior, list validation steps, link the relevant issue or PRD entry, and include screenshots or short recordings for UI changes.

## Configuration Notes
Do not replace the local-path GPUI dependencies with crates.io versions. Linux is the active target; config files live under `~/.config/paneflow/paneflow.json`.

## Cross-platform compatibility (mandatory)

Any new code, refactor, or change that touches the codebase in any way **must** be fully compatible with all three target platforms:

- **Linux** - every major distribution (Fedora, Ubuntu/Debian, Arch, openSUSE, etc.), both Wayland and X11.
- **macOS (Apple)** - Intel and Apple Silicon.
- **Windows** - Windows 10 and 11 (x64, and ARM64 where applicable).

Always verify every implementation decision against Windows, macOS, and Linux compatibility before considering the work done. For Linux, check the behavior against the major distro families and desktop stacks the project targets: Fedora, Ubuntu/Debian, Arch, openSUSE, Wayland, and X11.

Concretely this means:

- Never hardcode POSIX-only paths, shell commands, env vars, or separators. Use `std::path::PathBuf`, `std::env`, and the `dirs` crate (or equivalent) for all filesystem and environment access.
- Guard platform-specific code with `#[cfg(target_os = "…")]` and always provide a working path for the other two platforms (at minimum a graceful fallback or documented stub).
- Prefer cross-platform crates (`portable-pty`, `notify`, `dirs`, `which`, etc.) over POSIX-only APIs. If a POSIX-only crate is unavoidable, isolate it behind a trait with per-OS implementations.
- PTY, IPC, packaging, auto-update, keybindings, fonts, and file watching must each have Linux + macOS + Windows paths - never Linux-only.
- Before shipping a change, mentally (or actually) verify it compiles and behaves correctly on all three platforms. If you cannot verify, say so explicitly rather than assume.

The project is actively porting to macOS and Windows, so all new work must land cross-platform by default.

## Anti-Friction Rules (claude-doctor)

Règles pour éviter les patterns de friction détectés par `claude-doctor` sur ce projet : edit-thrashing, restart-cluster, repeated-instructions, negative-drift, error-loop, excessive-exploration.

### Editing discipline (anti edit-thrashing)

- Read the full file before editing. Plan all changes, then make ONE complete edit.
- If you've edited the same file 3+ times, STOP. Re-read the user's original requirements and re-plan from scratch.
- Prefer one large coherent edit over multiple small incremental ones.

### Stay aligned with the user (anti repeated-instructions, rapid-corrections)

- Re-read the user's last message before responding. Follow through on every instruction completely - don't partially address requests.
- Every few turns on a long task, re-read the original request to verify you haven't drifted from the goal.
- When the user corrects you: stop, re-read their message, quote back what they actually asked for, and confirm understanding before proceeding.

### Act, don't explore (anti excessive-exploration)

- Don't read more than 3-5 files before making a change. Get a basic understanding, make the change, then iterate.
- Prefer acting early and correcting via feedback over prolonged reading and planning.

### Break loops (anti error-loop, restart-cluster)

- After 2 consecutive tool failures or the same error twice, STOP. Change your approach entirely - don't retry the same strategy. Explain what failed and try something genuinely different.
- When truly stuck, summarize what you've tried and ask the user for guidance rather than retrying.

### Verify output (anti negative-drift)

- Before presenting your result, double-check it actually addresses what the user asked for.
- If the diff doesn't map cleanly to the user's request, don't ship it - re-plan.
