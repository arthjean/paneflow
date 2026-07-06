[PRD]
# PRD: Ultron Audit Remediation (2026-06-08)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-08 | Arthur Jean | Initial draft — remediation of the 2026-06-08 `/ultron` whole-codebase audit (39 confirmed findings, coverage confidence `medium`). |

## Problem Statement

The 2026-06-08 `/ultron` deep audit output (external run, not checked into this repo) ran 150 subagents across 8 dimensions with N-vote adversarial verification. It produced:

1. **Zero CRITICAL, zero HIGH** findings survived verification — the security-critical paths hold (minisign fail-closed updates, flag-gated IPC scripting, OSC52 Load sanitization, shell-metachar neutralization).
2. **3 MEDIUM crash/availability defects reachable on valid input**: `extract_scrollback_from` panics on a UTF-8 boundary when truncating multibyte scrollback (U-001); the AppImage zsync subprocess has no timeout so a stalled mirror hangs the update worker forever (U-002); `walk_jsonl_files` follows directory symlinks with no depth bound and can stack-overflow (U-003).
3. **Two systemic patterns** behind most of the 32 LOW findings. Pattern #1 — *asymmetric boundary enforcement*: write paths cap what read paths don't (scrollback capped on write not on restore, `paneflow.json` size-guarded but `session.json` read raw, IPC server caps inbound lines but the MCP client reads replies unbounded, OSC52 Load sanitizes but Store doesn't). Pattern #2 — *missing timeouts*: every external subprocess (`git`, `appimageupdatetool`, `opencode`, installer CLIs) and several socket I/O paths spawn with no deadline, and the update state machine has no watchdog — wrong default for a tool that supervises long-lived agent processes.
4. One approach defect worth fixing now: the diff review-ref allowlist drops `~` and `^`, silently corrupting the `HEAD~1` / `main^` revspecs the app generates itself, so reviews run against the wrong base (U-006).
5. **Coverage confidence is only `medium`.** Several substantial units returned zero or thin findings in a way that signals under-reading, not cleanliness — most notably the self-update pipeline (RCE-class threat profile, zero security findings), the AI shim + `agent_launcher` + `*_sessions.rs` command-construction surface (zero security findings despite a prior CWE-88 fix), the MCP-install config-file RMW surface, and `ai_hooks/extract.rs` (the untrusted-JSONL parse entry point). These warrant a focused second pass before silence is trusted as safe.

**Why now:** the audit is fresh and the finding line:numbers are still valid against `main`. The 3 MEDIUM crashes are reachable from ordinary agent output (multibyte scrollback, symlinked session dirs) and from a single stalled network mirror; they degrade availability of a tool whose whole value proposition is babysitting long-running agents. Fixing the two systemic patterns with shared helpers retires a whole class of LOW findings at once, cheaply, before the codebase grows further across cross-platform work.

## Overview

This PRD remediates the audit by *mechanism*, not by finding-ID: a single fix that closes many findings is preferred over one story per finding. Work is split into four priority phases so the high-value P0 work (crash elimination + uniform timeouts) ships standalone and the low-value P2 cleanup remains explicitly optional.

The core engineering moves are two shared disciplines. First, a new std-only `paneflow-process` micro-crate exposing a `run_with_timeout` helper (spawn → poll `try_wait` against a deadline → `kill` → `wait`, with a `Read::take` stdout cap and `Stdio::null` stdin), adopted at every external-subprocess call site — this closes U-002/U-015/U-032/U-035 and the installer/shim spawn sites with one reviewed primitive. Second, a "validate on ingress, both directions" pass that mirrors every existing write-side cap onto its read side (session.json size guard, restore-time workspace/pane caps, bounded client socket reads, schema-boundary breadth caps, OSC52 Store sanitization), plus centralizing the scattered `MAX_*` constants so the asymmetry can't silently reappear.

The remaining work is the targeted security second pass on the four under-covered units (re-run `/ultron --focus security`, land the deferred macOS Team-ID / Windows-publisher pins, harden `ai_hooks/extract.rs`, verify telemetry carries no PII) and a strictly opportunistic P2 batch of dedup/perf/cosmetic cleanups that the architecture assessment flagged as "real but bounded, no demonstrated bug — fix opportunistically, don't churn."

Key decisions: `floor_char_boundary` is used directly (stable since Rust 1.91); the timeout helper is a new zero-dependency crate (no shared lib crate exists and `paneflow-config` is a leaf); the "ACP message parsing" coverage gap is dropped because that surface was deleted with the in-app chat; telemetry PII work is verification-only because PII is excluded by construction (UUID v4 + typed-enum properties).

## Goals

| Goal | Phase-1 Target (P0) | Phase-4 Target (all) |
|------|---------------------|----------------------|
| Reachable panics / overflows on untrusted or valid input | 0 (down from ≥4: U-001, U-003, U-011, U-050) | 0 |
| External subprocess / socket calls with no deadline | 0 (down from ≥8 spawn sites) | 0 |
| Asymmetric ingress caps (write capped, read uncapped) | — | 0 (U-008, U-016, U-023, U-027, U-028, U-029, U-030) |
| Audit coverage confidence on the 4 thin units | — | raised from `medium` to `high` via `/ultron --focus security` re-run |
| Confirmed findings remaining (re-run audit) | ≤ 35 (close all P0) | ≤ 8 (only deferred cosmetics) |
| CI green on all 4 release-matrix legs | required | required |

## Target Users

### Paneflow maintainer (Arthur)
- **Role:** solo maintainer / orchestrator of the codebase; ships releases via a 4-leg CI matrix.
- **Behaviors:** runs coding agents inside Paneflow panes for hours; ships frequently; treats CI `cargo fmt --check` / clippy `-D warnings` as hard gates.
- **Pain points:** a background save panic or a hung update worker is invisible until it loses a session or freezes the app; under-covered security units are a latent unknown that blocks confident releases on the new platforms.
- **Current workaround:** none — the defects are silent until triggered.
- **Success looks like:** valid agent output can never crash a save; no external call can hang the UI or the updater; the four thin audit units have a real security verdict.

### Paneflow end users (the 18 current + future agent-cockpit users)
- **Role:** developers running AI agents in parallel panes on Linux/macOS/Windows.
- **Behaviors:** open large multibyte scrollbacks (CJK, emoji, box-drawing TUI), restore sessions, let the app self-update.
- **Pain points:** an app that crashes on quit (main-thread truncate panic during `save_session_blocking`) or hangs on a stalled update mirror erodes trust in a supervision tool.
- **Current workaround:** restart the app; disable auto-update.
- **Success looks like:** the app degrades gracefully (truncated-but-saved session, timed-out-and-recovered update) instead of crashing or hanging.

## Research Findings

Key findings that informed this PRD (full agent reports in session context; audit in `ULTRON/`):

### Competitive Context
- This is internal hardening, not a market feature. The relevant "competitor" baseline is mainstream terminal emulators (xterm/kitty/foot/iTerm2) and Zed's terminal, against which Paneflow is at near-parity. The OSC52 Store gap (U-023) is a place where Paneflow is *behind* the de-facto hardening practice.
- **Gap addressed:** Paneflow's value is supervising long-lived agents; an unbounded subprocess or a save-path panic is a reliability regression against that promise.

### Best Practices Applied
- `str::floor_char_boundary` / `ceil_char_boundary` stabilized in **Rust 1.91.0** (2025-10-30, PR #145756) — use the stdlib method directly; no manual helper needed for our MSRV. (U-001)
- Cross-platform bounded subprocess = `Command::spawn()` + `try_wait()` poll against a wall-clock deadline + `kill()` + mandatory `wait()` to reap the zombie. Windows gotcha: `child.kill()` returns `Err(InvalidInput)` on an already-exited process (rust-lang/rust#112423) — must be guarded; process-group/job-object tree-kill is out of scope for single-process CLIs. (U-002/U-015/U-032/U-035)
- `std::fs::DirEntry::file_type()` does **not** follow symlinks (uses `lstat`), unlike `Path::is_dir()` which does; `walkdir` with `follow_links(false)` + `max_depth(n)` is the canonical safe traversal. Windows junctions report `is_symlink() == true`, so `follow_links(false)` still prevents descent. (U-003)
- Bounded socket reads: `BufReader::new(stream).by_ref().take(MAX).read_line(&mut s)` — `by_ref()` is required (the `Take` adapter doesn't impl `BufRead`); a cap hit mid-line yields a string with no trailing `\n`, which the parser must treat as a framing error. (U-029)
- OSC52 write (Store) hardening: the writer owns sanitization; strip `\r` (0x0d), ESC (0x1b), C0 controls except `\t`/`\n`, DEL (0x7f), the C1 range (U+0080–U+009F), and bidi overrides before the clipboard write. Terminals do not strip on the writer's behalf. (U-023)

*Full research sources available in session transcript.*

## Assumptions & Constraints

### Assumptions (to validate)
- The 4 thin audit units are *under-covered, not clean*. EP-005 validates this by re-running `/ultron --focus security`; if the re-run confirms zero real findings, those spikes close with a documented "verified clean" verdict (still a goal-met outcome).
- A std-only timeout helper crate adds negligible binary size to the embedded shim (the shim's dependency budget is tight — `toml_edit` was deliberately kept out). To validate as an AC, not assume.
- `floor_char_boundary` (Rust 1.91) is available on the project's toolchain. If the pinned toolchain is older than 1.91, fall back to the `is_char_boundary` reverse-scan helper.

### Hard Constraints
- **Rust + GPUI (pinned Zed fork `ArthurDEV44/zed@paneflow/markdown-append-fix`)** — never swap GPUI for a crates.io dep; never touch the fork pin as part of this PRD.
- **Cross-platform Linux + macOS + Windows** for all new code. `cfg(windows)` branches in `paneflow-app` are **not compilable on the Linux host** (GPUI needs `windows.h` / `llvm-rc`) — Windows-touching changes are verified by the CI matrix, not locally (inspection-only on the dev box).
- **Clippy lints:** `panic = "deny"`, `unwrap_used`/`expect_used` = `warn`; new `unwrap()`/`expect()` must follow the project convention (`?`, `ok_or`, `match`, or `expect("documented invariant")`).
- **`cargo fmt --check` is a CI gate on all 4 legs** — run `cargo fmt` before every commit/push touching Rust (per `CLAUDE.md` pre-commit mandate; the project's rustfmt hook reorders imports differently from canonical `cargo fmt`, so always run `cargo fmt` via Bash before clippy/commit).
- **Read-only on the audit artifacts** — `ULTRON/` is the audit output, never edited by remediation work.
- **Human-in-the-loop** — no headless auto-fix; the security re-run (EP-005) writes findings for review, it does not auto-apply.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting gate (run `cargo fmt` first if it reports a diff; CI runs this on all 4 matrix legs)
- `cargo clippy --workspace --all-targets -- -D warnings` - lint gate (no new `unwrap`/`expect` warnings; `panic`/`unimplemented`/`dbg` denied)
- `cargo test --workspace` - all workspace tests, including new regression tests added by each story
- `cargo build --workspace` - debug build compiles
- For Windows-touching stories: green CI on all 4 release-matrix legs (Linux x86_64, Linux aarch64, macOS aarch64, Windows x86_64) — the `cfg(windows)` paths cannot be compiled on the Linux dev host.

## Epics & User Stories

### EP-001: Crash & overflow hardening (Phase 1)

Eliminate every panic and integer-overflow path reachable from valid or untrusted input. Closes 2 of the 3 MEDIUM must-fix findings (the 3rd, U-002, is closed in EP-002 because it shares the timeout mechanism).

**Definition of Done:** no `truncate`/slice/`unwrap`/arithmetic panic is reachable from agent-written JSONL, multibyte scrollback, symlinked session dirs, or pathological markdown; regression tests cover each boundary input.

#### US-001: Char-boundary-safe scrollback truncation (U-001)
**Description:** As a user with large multibyte scrollback, I want session save to truncate on a UTF-8 char boundary so that quitting or auto-saving never panics.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a scrollback whose byte index `MAX_CHARS` (400_000) falls mid-codepoint (CJK/emoji/box-drawing), when `extract_scrollback_from` (`src-app/src/terminal/pty_session.rs:1093`) truncates, then it cuts at `floor_char_boundary(MAX_CHARS)` and does not panic.
- [x] Given the same input on the synchronous quit path (`save_session_blocking` on the GPUI main thread), when the app exits, then it saves a truncated-but-valid session instead of unwinding the process.
- [x] A unit test builds a `String` with a multibyte char straddling `MAX_CHARS` and asserts the truncation returns a valid `&str` of length ≤ `MAX_CHARS`.
- [x] `cargo clippy` shows no new `unwrap`/`expect` warning from the change.

#### US-002: Symlink-safe, depth-bounded JSONL directory walk (U-003)
**Description:** As a user whose agent session directory contains symlinks, I want `walk_jsonl_files` to not descend into symlinked dirs and to bound recursion depth so that a symlink cycle can't stack-overflow the app.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a directory tree with a symlink that points to an ancestor (cycle), when `walk_jsonl_files` (`src-app/src/codex_sessions.rs:81`) runs, then it terminates without stack overflow and without descending the symlink.
- [x] Traversal uses `DirEntry::file_type()` (not `Path::is_dir()`) so symlinked directories are not followed, and enforces a hard `max_depth` (≥ 8 for `YYYY/MM/DD` plus slack).
- [x] Given a legitimate deep-but-acyclic tree within the depth bound, when the walk runs, then all real `.jsonl` files are still discovered (no false truncation of valid sessions).
- [x] Behavior is identical on Windows where NTFS junctions report as symlinks (verified via CI matrix or documented as inspection-only).

#### US-003: Overflow-safe parsers for agent-written and markdown content (U-011, U-050)
**Description:** As a user whose agent writes arbitrary JSONL and markdown, I want date and table parsing to be overflow-safe so that an absurd year or pathological column count degrades gracefully instead of panicking or truncating wrongly.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a parseable-but-absurd year in agent JSONL, when `parse_iso8601_to_unix_secs` (`src-app/src/agent_sessions.rs:551`) runs, then it uses `checked_mul`/`checked_add` and returns `None` on overflow, falling through to `iso8601_safe_fallback` (the date-prefix render) instead of overflowing `i64`.
- [x] Given a markdown table with > `u16::MAX` columns, when column count is computed (`src-app/src/markdown/view.rs:1209`), then it saturates via `u16::try_from(..).unwrap_or(u16::MAX)` instead of silently truncating, and the `cols == 0` bail still holds.
- [x] Unit tests cover the overflowing year and the pathological column count.

#### US-004: Documented invariants & symmetric clamps in the paint path (U-036, U-046, U-047)
**Description:** As a maintainer, I want the production `unwrap()` documented and the geometry clamps made symmetric so that the paint path follows project convention and future edits don't introduce off-by-one panics.

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The bare `.unwrap()` in `merge_background_regions` (`src-app/src/terminal/element/mod.rs:149`) becomes `.expect("merge_background_regions: rects.len() >= 2 guaranteed by the len() <= 1 early return")`.
- [x] `desired_rows` (`element/mod.rs:491`) gains the `.max(1.0)` clamp that `desired_cols` already has.
- [x] The multi-line selection last-line rect (`element/mod.rs:1048`) uses the same saturating clamp as its sibling rects.
- [x] No behavioral change is observable; existing terminal-render tests still pass (golden unchanged).

---

### EP-002: Uniform bounded subprocess execution (Phase 1)

Introduce one reviewed `run_with_timeout` primitive and adopt it at every external-subprocess call site, plus a watchdog on the update state machine. Closes MEDIUM U-002.

**Definition of Done:** no `Command::output()/status()/wait()` for an external program runs without a deadline; on timeout the child is killed, reaped, and the caller gets a structured error; the update worker can never sit in `Downloading` forever.

#### US-005: `paneflow-process` crate — `run_with_timeout` primitive
**Description:** As a maintainer, I want a std-only shared crate that runs a child process with a wall-clock deadline, a stdout byte cap, and null stdin so that every call site can bound external processes identically and cross-platform.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] A new workspace crate `paneflow-process` (zero external deps, `std` only) exposes `run_with_timeout(cmd, deadline, stdout_cap) -> Result<Output, ProcError>` that spawns, polls `try_wait()` with a short sleep, and on deadline calls `kill()` then `wait()` (reaps the zombie) and returns `ProcError::Timeout`.
- [x] The helper reads stdout through `Read::take(stdout_cap)` so a chatty child cannot exhaust memory, and sets `Stdio::null()` on stdin so the child can never block on a prompt.
- [x] On Windows, an already-exited child's `kill()` returning `Err(InvalidInput)` is treated as success (not propagated) — guarded behind `#[cfg(windows)]` with a unit test or documented inspection note.
- [x] Unit tests cover: normal completion under deadline, a sleeping child killed at the deadline, and a child exceeding the stdout cap (output truncated, no OOM, no hang).
- [x] Crate added to `[workspace.dependencies]`; binary-size delta to the embedded shim measured and recorded as ≤ a stated ceiling.

#### US-006: Bound the self-update subprocess + add update-worker watchdog (U-002, U-015)
**Description:** As a user, I want the AppImage/installer update step to time out and recover so that a stalled mirror or hung tool returns the updater to Idle instead of freezing it forever.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given a stalled `appimageupdatetool` zsync download, when the deadline (a generous multiple of `UPDATE_HTTP_TIMEOUT`, e.g. 5–10 min) elapses (`src-app/src/update/linux/appimage.rs:419`), then the child is killed and the call returns a structured `UpdateError::Timeout`.
- [x] The macOS `.dmg` and Windows `.msi` install subprocess paths are migrated to `run_with_timeout` (or documented as not spawning a blocking external tool).
- [x] Given a `Downloading` state that hangs, when the watchdog deadline elapses (`src-app/src/app/self_update_flow.rs:318`), then status is reset to `Idle`, the retry counter is bumped, and the failure is routed through `record_update_failure` — the `EnvironmentBroken` pkexec fallback becomes reachable.
- [x] CI green on all 4 matrix legs.

#### US-007: Bound the git subprocess layer (U-035)
**Description:** As a user on a slow or broken filesystem, I want git invocations to time out and never prompt so that a hung `git` can't block workspace badges or the diff viewer.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] `git diff --shortstat` (`src-app/src/workspace/git.rs:17`) and the diff viewer's `run_git` (`src-app/src/diff/git.rs`) run through `run_with_timeout`.
- [x] Both set `Stdio::null()` on stdin and `GIT_TERMINAL_PROMPT=0` in the environment so git can never block on a credential/helper prompt.
- [x] Given a `git` that hangs (simulated), when the deadline elapses, then the call returns an error and the caller renders a "stats unavailable" state instead of blocking.

#### US-008: Bound agent enumerators, installer CLIs, and shim reapers (U-032, U-025)
**Description:** As a user, I want `opencode` enumeration, MCP-install CLIs, and shim Ctrl+C reapers to be bounded so that a stuck child can't hang session discovery or accumulate threads.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] `opencode session list` (`src-app/src/opencode_sessions.rs:70`) runs through `run_with_timeout` with a bounded stdout cap (4–8 MB).
- [x] The `run_cli` helper in `crates/paneflow-mcp-install/src/agents/support.rs:81` (used for `claude mcp add` / `codex mcp add`) is bounded by a deadline.
- [x] Per-Ctrl+C detached reaper threads in `crates/paneflow-shim/src/exec.rs:298` are bounded (single draining reaper, or an in-flight cap that drops new SIGINT-driven stops past a ceiling) so hung hooks cannot accumulate threads unboundedly.
- [x] Given a hung child, when the deadline elapses, then the enumerator returns an empty/partial result without blocking the caller.

---

### EP-003: Symmetric ingress bounds — "validate on both directions" (Phase 2)

Mirror every write-side cap onto its read side and centralize the scattered `MAX_*` constants so the asymmetry can't reappear.

**Definition of Done:** session.json, the client socket boundary, the config boundary, and the layout schema all enforce read-side caps symmetric with their write side; the `MAX_*` caps live in one module.

#### US-009: session.json read-path size guard + restore-time caps (U-008 read side, U-016)
**Description:** As a user with a hand-edited or agent-written session.json, I want the restore path to size-guard and cap workspaces/panes so that a huge or over-broad session degrades gracefully instead of allocating unbounded state.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `load_session_at` (`src-app/src/app/session.rs:244`) stats the file and rejects/falls-back over a `MAX_SESSION_SIZE_BYTES` cap before `read_to_string`, mirroring `read_config_string` (`crates/paneflow-config/src/loader.rs:91`).
- [x] `restore_workspaces` truncates iteration to `MAX_WORKSPACES` (20) and rejects/trims any layout whose `leaf_count()` exceeds `MAX_PANES` (32), mirroring the `workspace_ops/layout.rs` guard.
- [x] Given an oversized session.json, when the app starts, then it logs a warning and starts with a safe fallback session instead of OOM/hang.

#### US-010: IPC socket boundary hardening — bounded client reads, timeouts, fail-closed perms (U-029, U-027, U-028, U-031)
**Description:** As a user, I want the local socket and config boundaries to bound reads, time out connects/writes, reject non-regular files, and fail closed on a chmod error so that a stalled or hostile same-UID peer can't hang or downgrade the IPC surface.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The shared IPC client `read_line` on the IPC socket (`crates/paneflow-ipc-client/src/lib.rs`) is wrapped with `.by_ref().take(MAX_REQUEST_LEN)` (256 KiB ceiling); hitting the cap is treated as a framing error, not a partial parse.
- [x] The ai-hook IPC connect/write (`crates/paneflow-ai-hook/src/main.rs:94`) gets a write/connect deadline (250–500 ms) so a stalled peer can't block the hook or its caller.
- [x] `read_config_string` (`loader.rs:91`) rejects non-regular files (`!meta.file_type().is_file()`) before reading, closing the FIFO/device TOCTOU variant.
- [x] On Unix, if the socket `set_permissions(0600)` fails (`src-app/src/ipc.rs:452`), the server logs an error, removes the socket file, and refuses to serve (fail-closed) instead of discarding the result.

#### US-011: Schema-boundary breadth caps + legacy ratio handling (U-008 schema side, U-007)
**Description:** As a maintainer, I want the layout deserializer to cap breadth and handle the legacy single `ratio` explicitly so that a malformed layout can't allocate unbounded children or silently lose ratio data.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `validate_layout` (`crates/paneflow-config/src/loader.rs`) caps total leaf count to 32 panes per workspace and bounds children/surfaces vector length at the schema boundary (`crates/paneflow-config/src/schema.rs:988`).
- [x] Given a Split with `ratio: Some` but `ratios: None` and `children.len() != 2`, when validated (`schema.rs:755`), then a 2-child case converts to `ratios=[r, 1-r]` and an N-ary case logs a `warn!("legacy ratio ignored on N-ary split")` instead of silently discarding.
- [x] Given an over-broad layout, when loaded, then it is trimmed/rejected with a logged warning, not expanded.

#### US-012: Restore-time path-traversal re-validation (U-030, U-041)
**Description:** As a user restoring a session, I want expanded-path rehydration and diff working-text reads to re-assert containment so that an absolute or `..` path in session.json or a git-reported symlink can't escape the workspace root.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `expanded_paths` rehydration (`src-app/src/app/session.rs:338`) rejects any `rel` containing `Component::ParentDir`/`RootDir`/`Prefix`, or re-asserts `abs.starts_with(base)` after `join`, so an absolute `rel` no longer silently replaces the `cwd` base.
- [x] `load_working_text` (`src-app/src/diff/git.rs:384`) `lstat`s the resolved path and renders a stub/link-target for symlinks instead of dereferencing outside the worktree.
- [x] Given a session.json with `expanded_paths: ["/etc/passwd"]`, when restored, then the entry is dropped (not opened as an absolute path).

#### US-013: Centralize the scattered `MAX_*` caps (architectural cleanup)
**Description:** As a maintainer, I want the duplicated/scattered size constants in one module so that read and write caps stay in sync and the asymmetry pattern can't silently return.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-009, US-010, US-011

**Acceptance Criteria:**
- [x] The duplicated `MAX_LINE_BYTES` (`claude_sessions.rs:39`, `codex_sessions.rs:34`) and `MAX_OSC52_BYTES` (`pty_session.rs:808`, `view.rs:336`) are deduplicated to a single source.
- [x] A documented `limits` module (or equivalent) centralizes `MAX_REQUEST_LEN`, `MAX_LINE_BYTES`, `MAX_CHARS`, `MAX_OSC52_BYTES`, `MAX_WORKSPACES`, `MAX_PANES`, `MAX_CONFIG_SIZE_BYTES`, `MAX_SESSION_SIZE_BYTES` with comments tying each read cap to its write cap.
- [x] No behavioral change; all existing tests pass (pure refactor verified by golden/test parity).

---

### EP-004: Untrusted terminal / clipboard / markdown content (Phase 2)

Harden the paths that ingest untrusted terminal output and markdown, and fix the self-inflicted revspec corruption. Leads with P0 U-006.

**Definition of Done:** OSC52 Store is symmetric with Load; markdown body spans are bidi-stripped; the diff revspec sanitizer preserves the app's own revspecs while still neutralizing shell metacharacters.

#### US-014: Fix the diff revspec sanitizer (U-006)
**Description:** As a user running a branch/base diff, I want the ref sanitizer to preserve legitimate revspec operators so that `HEAD~1` / `main^` are not corrupted and reviews run against the correct base.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `sanitize_ref_for_prompt` (`src-app/src/diff/review_terminal.rs:72`) preserves `~` and `^` (and other valid revspec chars) — either by adding them to the allowlist or by switching to a denylist of shell-active chars (`` ` $ ; | & ( ) < > ' " \ * ? [ ] { } `` + whitespace/newline).
- [x] Given the app-generated refs `HEAD~1` and `main^`, when sanitized, then they pass through unchanged.
- [x] Given a ref containing `$(...)`, a backtick, or a newline, when sanitized, then the metacharacters are neutralized.
- [x] A unit test asserts both the pass-through and the neutralization cases.

#### US-015: Symmetric OSC52 Store sanitization (U-023)
**Description:** As a user, I want untrusted terminal output written to the system clipboard via OSC52 Store to be control-char sanitized so that a rogue program can't plant a paste-injection payload.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The OSC52 Store path (`src-app/src/terminal/pty_session.rs:806` → `view.rs:327`) applies the same control-char filter the Load path already uses (`view.rs:330-359`): strip `\r` (0x0d), ESC (0x1b), C0 controls except `\t`/`\n`, DEL (0x7f), and the C1 range (U+0080–U+009F) before `cx.write_to_clipboard`.
- [x] Given terminal output containing `\r` and ESC sequences, when OSC52 Store fires, then the clipboard receives the stripped payload.
- [x] The existing 100 KiB size cap is preserved; printable multibyte content is unaffected.

#### US-016: Markdown bidi/zero-width stripping in body spans (U-019)
**Description:** As a user viewing untrusted markdown, I want bidi/zero-width control chars stripped from all rendered text, not just `[image:]` placeholders, so that disguised content can't render.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The bidi/zero-width filter is moved into the shared text ingress (`src-app/src/markdown/parser.rs:448` `push_text`, or `build_styled_text`) so every rendered span is sanitized, not only the image placeholder (`parser.rs:218`).
- [x] The filter covers the bidi override range (U+202A–U+202E, U+2066–U+2069, U+200F, U+061C) and zero-width chars, without the placeholder-only length cap.
- [x] Given a markdown body containing a bidi override, when rendered, then the override char is removed from the displayed span.

#### US-017: Correct exactly-MAX final-line classification (U-017)
**Description:** As a user with a session whose final JSONL line is exactly `MAX_LINE_BYTES` with no trailing newline, I want it parsed instead of dropped so that a valid final record isn't lost as "oversized."

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a final line of exactly `MAX_LINE_BYTES` and no trailing `\n` (`src-app/src/claude_sessions.rs:165`), when read, then `fill_buf()` is peeked once; on EOF the line is treated as a complete final record and parsed, not discarded.
- [x] Given a genuinely truncated oversized line (more bytes follow), when read, then it is still classified as oversized and skipped.
- [x] A unit test covers both the exactly-MAX-at-EOF and the truncated-oversized cases.

---

### EP-005: Targeted security-coverage second pass (Phase 3)

Close the audit's coverage gaps on the four under-read units. These are spike-then-fix stories: re-run `/ultron --focus security`, record the verdict, and land any HIGH+ fix plus the already-known deferred items.

**Definition of Done:** each of the four thin units has a documented security verdict (real findings filed, or "verified clean"); the deferred EP-001 platform pins are landed; `ai_hooks/extract.rs` is bounds-guarded; telemetry PII-absence is asserted by a test.

#### US-018: Self-update security re-audit + deferred platform pins
**Description:** As a maintainer, I want a focused security pass on the self-update pipeline plus the deferred macOS/Windows publisher pins so that the RCE-class surface has a real verdict and defense-in-depth.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `/ultron --focus security` (or `/security-review`) is run scoped to `src-app/src/update/**` + `self_update_flow.rs`; findings are written to `ULTRON/` (new run) and triaged.
- [x] The deferred defense-in-depth gaps from the EP-001 self-update review are landed: macOS Team ID pinned in the install/verify path, and Windows publisher pinned (platform-gated; verified via CI matrix).
- [x] Any HIGH+ finding from the re-run is filed as a new P0 story appended to this PRD (scope is not silently absorbed).
- [x] If the re-run surfaces no real finding, the spike closes with a "verified clean — coverage raised to high" note recorded in the status file.

#### US-019: AI shim + agent_launcher + session-resume argument-injection pass
**Description:** As a maintainer, I want the command-construction surface audited for argument injection so that no agent-written value (session id, prompt, resume arg) can inject a CLI flag like `--dangerously-skip-permissions`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The resume-command builders in `agent_launcher.rs`, `claude_sessions.rs`, `codex_sessions.rs`, `opencode_sessions.rs` are reviewed for argument injection (building on the EP-007 CWE-88 `is_valid_session_id` fix).
- [x] Given an agent-written session id or resume value beginning with `-`/`--`, when a resume command is constructed, then it cannot be interpreted as a flag (argument separator `--` used, or value validated to start alphanumeric/`_`).
- [x] A regression test asserts a flag-shaped value is rejected or neutralized.
- [x] Findings (if any) are filed; clean result recorded as a verdict.

#### US-020: Harden `ai_hooks/extract.rs` untrusted-JSONL parsing
**Description:** As a user, I want the AI-hook JSONL parser to be bounds-guarded so that a corrupted or adversarial hook-output file can't exhaust memory or mis-parse.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `src-app/src/ai_hooks/extract.rs` (the JSONL parser reading `~/.claude/hooks/` output) gains a per-file size guard and a per-line byte cap consistent with the other session parsers.
- [x] Given an oversized or malformed hook-output file, when parsed, then it is rejected/truncated with a logged warning, not loaded unbounded.
- [x] Note in the PRD record: the "ACP message parsing" coverage gap is **dropped** — `crates/paneflow-acp` is now 3 trivial files (the protocol parser was deleted with the in-app chat), so `ai_hooks/extract.rs` is the real remaining untrusted-parse surface.

#### US-021: MCP-install config-file RMW/TOCTOU pass (U-009, U-010)
**Description:** As a user, I want the MCP-install read-modify-write on agent config files to be TOCTOU-safe and to report malformed-present configs correctly so that install/uninstall is idempotent and honest.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The RMW on `~/.claude.json` / `~/.codex/config.toml` / `~/.gemini/settings.json` is reviewed for TOCTOU (the existing backup + atomic-write contract is confirmed or hardened).
- [x] Uninstall distinguishes "absent" from "present-but-malformed" (`crates/paneflow-mcp-install/src/agents/claude_code.rs:126`): a malformed-but-present config surfaces a loud error instead of `NothingToRemove`.
- [x] `resolve_target` (`crates/paneflow-mcp/src/tools.rs:170`) accepts integral floats (e.g. `42.0`) as the `number`-typed `target`, honoring the schema.

#### US-022: Telemetry PII-absence guard test
**Description:** As a maintainer, I want a test asserting telemetry carries no PII so that a future free-form property can't silently leak user data.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [x] A test asserts the telemetry event-property surface is typed-enum-only (no free-form string fields carrying paths/usernames/hostnames), and `distinct_id` remains a UUID v4 unrelated to identity.
- [x] Note in the record: this is **verify-not-fix** — PII is excluded by construction (`crates/paneflow-telemetry/src/`), so the deliverable is a regression guard, not a scrubber.

---

### EP-006: Opportunistic cleanup & dedup (Phase 4 — do not churn)

Batch the remaining LOW efficiency, dedup, and minor-correctness findings. The architecture assessment flagged these as "real but bounded, no demonstrated bug — fix opportunistically, don't churn." These are P2 and may be deferred indefinitely; do them only alongside adjacent work.

**Definition of Done:** the batched findings are addressed with no behavioral regression, or explicitly deferred in the status file.

#### US-023: Perf hot-path batch (U-024, U-033, U-021, U-034, U-052)
**Description:** As a maintainer, I want the per-frame and per-tick hot paths memoized so that idle and rendering CPU drop without changing output.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] APCA per-cell contrast is memoized on `(fg, bg)` (and theme generation) so contiguous same-color runs short-circuit (`element/color.rs:123`); cursor blink no longer forces a full-grid relayout incl. the contrast pass (`terminal/blink.rs:31`).
- [x] `scan_output` lowercases each line once instead of joining + lowercasing the whole blob every tick (`pty_session.rs:985`); `merge_background_regions` avoids the per-frame full sort (`element/mod.rs:136`); `reported_ports` uses a bounded structure (bitset/HashSet) instead of an unbounded Vec (`pty_session.rs:280`).
- [x] Terminal-render golden tests pass unchanged (no visual regression); a micro-benchmark or before/after note records the idle-CPU reduction.

#### US-024: Dedup / over-generality batch (U-004, U-005, U-013, U-039, U-040, U-042)
**Description:** As a maintainer, I want the duplicated handler bodies and redundant work consolidated so that the code is easier to maintain, without churning working behavior.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] The three `detect_*_at_hover` methods (`terminal/view.rs:660`) share one extraction helper; the `TerminalView` constructor reads/parses `paneflow.json` once instead of 4× (`view.rs:557`).
- [x] `workspace_idx_for_terminal` uses the zero-alloc `any_leaf` helper (`event_handlers.rs:779`); the four PaneEvent drop-split handlers share a resolve+cap helper (`event_handlers.rs:554`); the nine `Ctrl+1-9` handlers collapse to one indexed action (`workspace_ops/mod.rs:649`); `DiffScope` persists via serde directly instead of three manual mappings (`diff/scope.rs:47`).
- [x] All existing tests pass; no behavioral change (verified by golden/test parity). Skip any item whose refactor is riskier than its payoff and record the skip.

#### US-025: Minor correctness / robustness batch (U-012, U-014, U-018, U-020, U-022, U-026, U-053)
**Description:** As a maintainer, I want the small correctness LOWs fixed so that edge cases (non-ASCII highlight offsets, legacy-shim PID, code-block id collisions, cwd fallback) behave correctly.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Highlight run lengths map back to original-string byte offsets for non-ASCII titles (`agents_sidebar/mod.rs:1094`); `ai.stop` auto-clear uses the resolved session key so legacy no-pid shims don't leak (`ipc_handler.rs:1015`); the git-watch refcount only increments on a successful `watch()` (`main.rs:545`).
- [x] Code-block element id mixes in positional index so identical blocks don't collide (`markdown/view.rs:1133`); rc-file write errors abort shell-integration activation instead of being ignored (`terminal/shell.rs:297`); `cwd` falls back to the user's home dir (not `/`) on `current_dir()` failure with a logged warning (`pty_session.rs:469`).
- [x] The IPC 5s dispatch timeout no longer lets a non-idempotent mutation execute after the client got an error (`ipc.rs:765`) — request marked cancelled on `recv_timeout` so duplicate workspaces/panes can't be created on retry.
- [x] Each fixed item has a unit test or a documented manual-verification note; any item skipped as too risky is recorded.

---

## Functional Requirements

- FR-01: The system must never panic on the session-save path for any valid UTF-8 scrollback, including multibyte content straddling the truncation boundary.
- FR-02: The system must never recurse without a depth bound or follow directory symlinks when walking agent session directories.
- FR-03: The system must bound every external-subprocess invocation with a wall-clock deadline and kill+reap the child on expiry.
- FR-04: The system must cap stdout read from any external subprocess and set null stdin so a child cannot block on a prompt.
- FR-05: For every write-side size/breadth cap, the system must enforce a symmetric read-side cap (session.json, IPC client reads, layout breadth, OSC52 Store).
- FR-06: The system must not corrupt the git revspecs it generates itself (`HEAD~1`, `main^`) while sanitizing refs for shell-adjacent contexts.
- FR-07: The system must re-validate path containment when rehydrating persisted paths on session restore.
- FR-08: The system must NOT auto-apply any fix surfaced by the EP-005 security re-run — findings are written for human review.
- FR-09: The system must not remove the documented resilience fallbacks the audit flagged as intentional (e.g. the triple-timer event-driven backstop).

## Non-Functional Requirements

- **Reliability:** 0 reachable panics/overflows on untrusted or valid input after Phase 1 (measured by re-running `/ultron` and confirming the U-001/U-003/U-011/U-050 finding class is gone). Update worker cannot remain in `Downloading` longer than its watchdog deadline (≤ 10 min).
- **Performance:** the `paneflow-process` poll loop uses a ≤ 50 ms sleep between `try_wait()` calls (no busy-spin). US-023 reduces idle relayout cost so cursor-blink does not trigger a full-grid APCA contrast pass (target: cursor-only repaint at 2 Hz does 0 contrast recomputations on unchanged cells).
- **Security:** OWASP/CWE posture maintained or improved; argument-injection (CWE-88) and paste-injection (OSC52) surfaces closed; no PII in telemetry (typed-enum-only property surface, asserted by test). Coverage confidence raised from `medium` to `high` on the 4 thin units.
- **Memory:** external-subprocess stdout capped at 4–8 MB; `reported_ports` bounded to a flat ceiling (e.g. an 8 KB bitset) regardless of distinct-port count; session.json rejected above `MAX_SESSION_SIZE_BYTES` before allocation.
- **Binary size:** the new `paneflow-process` crate adds ≤ a stated ceiling to the embedded shim binary (std-only, zero external deps); measured as an AC in US-005.
- **Cross-platform:** all new code compiles and behaves correctly on Linux x86_64/aarch64, macOS aarch64, Windows x86_64, verified by the 4-leg CI matrix.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Multibyte at truncation boundary | Scrollback byte 400_000 mid-codepoint | Cut at `floor_char_boundary`, save valid truncated session | — (silent, logged at debug) |
| 2 | Symlink cycle in session dir | Symlink → ancestor under `~/.codex/sessions` | Traversal terminates, symlink not descended | — |
| 3 | Stalled update mirror | zsync download stops sending bytes | Kill child at deadline, reset to Idle, bump retry, surface failure toast | "Update timed out — will retry" |
| 4 | Hung git on broken FS | `git` blocks on NFS / credential prompt | Timeout → render "stats unavailable" | — |
| 5 | Oversized session.json | Hand-edited / agent-written 50 MB file | Reject before read, start with fallback session | (logged warning) |
| 6 | Absolute path in expanded_paths | `expanded_paths: ["/etc/passwd"]` | Entry dropped on restore (containment re-asserted) | — |
| 7 | Paste-injection via OSC52 Store | Rogue program writes `\r`/ESC to clipboard | Control chars stripped before clipboard write | — |
| 8 | Self-corrupted revspec | App generates `HEAD~1` / `main^` | Passed through unchanged; diff runs against correct base | — |
| 9 | Flag-shaped session id | Agent JSONL session id `--dangerously-skip-permissions` | Neutralized (arg separator / validation), not interpreted as a flag | — |
| 10 | Exactly-MAX final JSONL line at EOF | Final record exactly 64 KiB, no trailing `\n` | Parsed as complete record, not dropped as oversized | — |
| 11 | Pathological markdown table | > 65535 columns | Column count saturates to `u16::MAX`, no truncation panic | — |
| 12 | Socket chmod fails | `set_permissions(0600)` returns Err on Unix | Fail closed: remove socket, refuse to serve | (logged error) |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Churn on a stable, shipping app (0 CRITICAL/HIGH) wastes time better spent on product/distribution | High | Med | Strict phasing: P0 (EP-001/002) ships standalone and is sufficient; P2 (EP-006) is explicitly optional/deferrable; the PRD does not mandate all 53 findings |
| 2 | New `paneflow-process` crate bloats the embedded shim binary | Low | Med | std-only, zero deps, tiny; binary-size delta measured as an AC in US-005 |
| 3 | Cross-platform timeout/kill regressions (Windows `kill()` semantics, junctions) can't be compile-checked on the Linux host | Med | Med | Windows-guard the `kill()` already-exited case; rely on the 4-leg CI matrix as the gate; AC requires green CI |
| 4 | EP-005 security re-run surfaces a real HIGH and expands scope mid-PRD | Med | High | EP-005 stories are spikes that *write* findings; any HIGH+ is filed as a new appended P0 story, triaged separately, not silently absorbed |
| 5 | A "pure refactor" dedup (EP-006) introduces a behavioral regression | Med | Low | Golden/test parity required; any item riskier than its payoff is skipped and the skip recorded |
| 6 | Centralizing `MAX_*` constants (US-013) changes a value by accident | Low | Med | US-013 is a pure move; values asserted unchanged by existing tests before/after |

## Non-Goals

- **No removal of the IPC same-UID scripting model** — local same-UID RCE is by design (opt-in, flag-gated, socket 0600); this PRD hardens the boundary, it does not change the trust model.
- **No removal of documented resilience fallbacks** — U-038 (three independent timer loops as an event-driven backstop) is intentional; dropping it trades a resilience guarantee for code reduction (a behavior change, not a simplification).
- **No fix for the cosmetic INFO findings** explicitly marked "optional / no functional change / defer" by their own remediation: U-037 (non-UTF-8 minisig UX), U-043 (IPC serialize-fail empty line — cannot occur for these `Value`s), U-044 (dual workspace addressing refactor), U-045 (markdown URL validators are dead-code until links become clickable — guarded by an allowlist test + MUST-route comment), U-048 (MAX_REQUEST_LEN comment understatement — doc only), U-049 (per-connection request serialization — optional hardening), U-051 (`file://localhost:` colon variant — clarity only, remote-file barrier already holds). Revisit if a related feature (e.g. clickable markdown links) lands.
- **No process-tree / job-object kill on Windows** — the bounded subprocesses are single-process CLIs; tree-kill is out of scope.
- **No change to the GPUI fork pin** — orthogonal to this PRD.
- **No auto-apply / headless remediation** — human-in-the-loop per project policy.

## Files NOT to Modify

- `src-app/Cargo.toml` / `Cargo.lock` GPUI git-dep pins: the Zed fork branch is managed by a separate runbook; do not touch it here.
- External `/ultron` audit output from 2026-06-08: read-only input to this PRD, not a checked-in repo artifact.
- The `alacritty_terminal` neutral-types allowlist (`terminal/types.rs` 8-file guard test) — changes there are governed by EP-003 of the terminal-neutral-types work, not this PRD.
- Theme/builtin color tables — unrelated to remediation.

## Technical Considerations

- **Shared crate placement:** `paneflow-process` as a new std-only workspace crate is recommended — engineering to confirm it should not instead be a module inside an existing reachable crate. It must be a dependency of `paneflow-shim`, `paneflow-mcp-install`, and `src-app`; `paneflow-config` is a leaf and is the wrong home (dependency inversion).
- **Async vs sync helper:** the codebase uses `smol` (via GPUI). The recommended `run_with_timeout` is a synchronous `spawn` + `try_wait` poll for call sites already off the render thread; for on-thread callers, wrap via `smol::unblock`. Engineering to confirm which call sites are render-thread vs background.
- **`floor_char_boundary` MSRV:** requires Rust ≥ 1.91. Confirm the pinned toolchain; if older, use the `is_char_boundary` reverse-scan fallback (US-001 AC accommodates both).
- **OSC52 Store filter reuse:** extract the Load-path filter (`view.rs:330-359`) into a shared function so Store and Load share one sanitizer (avoids re-introducing the asymmetry).
- **EP-005 ordering:** run the security re-run (US-018/019) before committing the platform pins so the pins are informed by the re-run's findings. Backward compatibility: the macOS Team-ID / Windows-publisher pins must not break existing signed releases' update path (verify the suffix-agnostic asset matcher still resolves).
- **Migration / rollback:** all changes are additive guards or behavior-preserving refactors; rollback is per-commit revert. The `MAX_SESSION_SIZE_BYTES` cap must be generous enough not to reject legitimate large sessions (size it from the observed `extract_scrollback` 400_000-char cap × workspaces × panes).

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Confirmed audit findings | 39 (0 C / 0 H / 3 M / 32 L / 18 I) | ≤ 35 after Phase 1; ≤ 8 after Phase 4 | Phase 1 / Phase 4 | Re-run `/ultron`, diff `AUDIT.json` finding count |
| Reachable panic/overflow paths | ≥ 4 (U-001/003/011/050) | 0 | Phase 1 | Targeted unit tests + re-run audit |
| Unbounded external subprocess sites | ≥ 8 | 0 | Phase 1 | grep audit + re-run; `run_with_timeout` adoption |
| Coverage confidence (4 thin units) | `medium` | `high` | Phase 3 | `/ultron --focus security` re-run verdict |
| CI matrix legs green | 4/4 (current) | 4/4 (maintained) | every story | release-matrix CI |
| Idle CPU (cursor blink relayout) | full-grid relayout 2 Hz | 0 contrast recompute on unchanged cells | Phase 4 | micro-bench / before-after note |

## Open Questions

- Is the pinned Rust toolchain ≥ 1.91 (for `floor_char_boundary`)? — maintainer to confirm before US-001; fallback path exists either way.
- What is the acceptable binary-size delta ceiling for the embedded shim from `paneflow-process`? — maintainer to set the number US-005 asserts against.
- Should EP-005's security re-run use `/ultron --focus security` (multi-agent, broad) or `/security-review` (single-pass, scoped)? — recommend `/ultron --focus security` for the self-update + shim units given the known thin coverage; maintainer to confirm token budget.
- Is the macOS Team-ID / Windows-publisher pinning (US-018) blocked on any pending Azure/Apple onboarding state? — check `project_windows_signing` / `project_macos_signing` before scheduling.
[/PRD]
