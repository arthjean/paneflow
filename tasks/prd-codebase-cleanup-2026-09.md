[PRD]
# PRD: Codebase Cleanup from the 2026-09-24 Audit

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-24 | Arthur Jean | Initial draft from the read-only audit of main at e15277a0 |

## Problem Statement

A read-only audit of main at `e15277a0` verified 309 findings across every tracked zone of the repository: 14 god files, 128 dead-code items, 89 test-only items, 35 duplicates, 17 deletable tracked files, 11 unused dependencies, and 15 over-broad lint allowances. Statuses: 199 CONFIRMED, 94 PARTIAL with a corrected action, 7 blocked by the unmerged `feat/agents-browser` branch, 7 rejected, 1 unconfirmed.

1. **A live crash path.** `clamp_text` (`crates/paneflow-host/src/agent.rs:80`, copied at `crates/paneflow-serve/src/activity.rs:60`) takes 4,096 characters, then truncates at byte 4,096. Any agent text above that size whose byte 4,096 falls inside a multibyte character (accented or CJK text) panics the persistent host or the worker.
2. **Settings and APIs that silently do nothing.**
   - `external_editor` is written by Settings but never read. Its reader left with the ACP chat removal in `caf7d7cb`.
   - `option_as_meta` never reaches the libghostty encoder, which resets Option-as-Alt to off.
   - `surface.rename` reads `new_name` (`src-app/src/app/ipc_handler.rs:804`) while the public reference documents `name` (`docs/user/scripting/reference.md:228`), so a documented call clears the pane name.
   - The headless `surface.*` fallback sends per-connection aliases over a transport that reconnects on every call.
   - The CLI and MCP skip the isolated-home endpoint rule that the desktop applies.
3. **Code that looks live but is not.**
   - The in-process PTY runtime (about 1,450 production lines in `src-app/src/terminal/ghostty_session.rs`) has no production caller on any platform, yet release qualification gates still measure it.
   - 59 `#[allow(dead_code)]` attributes hide which items are really used.
   - Agents and reviewers spend their read budget on dead paths and act on them.
4. **Files too large to work in.** Twelve Rust files range from 2,011 to 7,742 lines, and `.github/workflows/release.yml` has 3,225. Each mixes 5 to 15 responsibilities, so every change to them costs a full-file read and invites merge conflicts.
5. **Dependency and asset drift.**
   - 9 dead packages remain in `Cargo.lock` after the ACP chat removal.
   - Windows features are declared only for tests.
   - 28 SVG icons and several packaging files have no reader.

**Why now:**
- `e15277a0` just made Paneflow developable from inside a Paneflow pane, which makes the wrong-instance endpoint defect (item 2) reachable in daily use.
- The persistent-session work has shipped and left its predecessors behind: the in-process runtime, the worker front door, and duplicated host and serve primitives.
- `feat/agents-browser` (25 commits ahead, merge base `434371f5`) will rebase onto six of the god files. Sequencing the splits after that merge is the cheapest point to do them.

## Overview

This PRD turns every CONFIRMED and PARTIAL audit row into dependency-ordered stories.
- Rejected rows are excluded.
- Rows the audit resolved as "keep" are listed, not scheduled.
- Branch-blocked rows and product decisions become conditional stories or Open Questions, each with a default.

Work ships in three phases:
- **Phase A** (EP-001 and EP-002):
  - EP-001 fixes the eight behavior defects the audit exposed. `clamp_text` is the only P0.
  - EP-002 removes unused dependencies, build leftovers, and tracked files with no reader.
- **Phase B** (EP-003 and EP-004): deletes dead code crate by crate, then gates test-only items behind `cfg(test)`. This includes compiling the in-process runtime for tests only (option A), pruning the libghostty wrappers no production code calls, and replacing broad allowances with target-scoped attributes.
- **Phase C** (EP-005 and EP-006): consolidates duplicated helpers under one owner, then splits the god files by pure moves.
  - The splits run only after the dead code inside them is gone.
  - They also wait for the branch or qualification that touches each file to land.

Key decisions are made here, and Open Questions carries the ones you may want to overturn:
- The in-process runtime becomes `cfg(test)` now; deleting it is a follow-up after the Windows qualification.
- `paneflow hooks` delegates to the integrations installer.
- `packaging/winget` is deleted.
- `bench/results` is kept.
- The `external_editor` setting is wired up rather than removed.
- The worker proxy is deleted once the headless fallback is fixed.

Every story is traced to audit row IDs `R1` to `R309`. They refer to `tasks/audit/cleanup-2026-09-24.md` and `tasks/audit/cleanup-2026-09-24-rows.json`, which are **local, untracked evidence**: another agent cannot read them unless Arthur shares them. Each story therefore restates the paths and lines it needs.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Remove crash and silent no-op defects found by the audit | US-001 to US-008 DONE, each with a regression test or recorded hardware check | Zero reopened defects from EP-001 |
| Shrink the dependency and asset surface | Cargo.lock at 837 packages or fewer (846 at e15277a0); 40 or more tracked files removed | No unused dependency reintroduced (spot audit with cargo-machete) |
| Remove dead and test-only production code | Phase B complete; `#[allow(dead_code)]` count at 20 or fewer (59 at e15277a0) | 7,500 or more net lines removed since e15277a0, excluding moves |
| Make the largest files workable | EP-005 started | No production file in the split set above 2,000 lines, except `host.rs` at 2,700 or fewer |

## Target Users

### Maintainer (Arthur Jean)
- **Role:** Sole visible contributor. Reviews and ships all agent-produced changes on Linux, macOS, and Windows.
- **Behaviors:** Implements through `/implement-epic` and certifies through `/review-epic`. Validates Windows on a dual-boot machine and macOS through CI and real hardware.
- **Pain points:**
  - A 7,742-line `ghostty_session.rs` and a 5,167-line `ipc_handler.rs`.
  - 59 lint allowances that hide real usage.
  - CI legs that fail only on Windows because of cfg-gated items.
  - Qualification gates measuring a runtime no pane runs.
- **Current workaround:** Targeted greps, the traps documented in AGENTS.md, and per-change manual reading.
- **Success looks like:** Every file in the split set opens under 2,000 lines, every remaining allowance is target-scoped, and the audit's defects are closed with tests.

### Coding agents working in the repository
- **Role:** Claude Code and Codex sessions implementing stories in this workspace.
- **Behaviors:** Read files within a finite context window, and trust what compiles as live code.
- **Pain points:**
  - Dead paths such as `GhosttySession::start`, the worker proxy, and the libghostty wrappers look live, so agents extend or test them.
  - God files exhaust the read budget before the relevant function is found.
- **Current workaround:** AGENTS.md warnings and explicit file:line pointers in prompts.
- **Success looks like:** Every compiled production item has a production caller, and each concern lives in a file small enough to read whole.

### Paneflow users on Linux, macOS, and Windows
- **Role:** Developers running coding agents in Paneflow panes.
- **Behaviors:** Use accented or CJK agent output, the Settings editor picker, macOS Option shortcuts, the scripting API, and the headless CLI.
- **Pain points:**
  - A host panic on long multibyte agent text.
  - The editor setting is ignored.
  - Option+letter does not send Meta on macOS.
  - `surface.rename` as documented clears the name.
  - The headless CLI fallback misroutes.
- **Current workaround:** Use `$EDITOR`, avoid `surface.rename`, and keep the desktop running for CLI calls.
- **Success looks like:** Each of those behaves as the docs and Settings state.

## Research Findings

Key findings that informed this PRD:

### Evidence boundary
- The audit inspected main at `e15277a0` on 2026-09-24. Unmerged branches were checked with `git cherry`:
  - `feat/agents-browser`: 25 commits ahead, last commit 2026-09-14.
  - `bench/startup-compare`, `pr46`, and `test/macos-late-descendant`: small.
- Linux was compiled with `--force-warn dead_code`. macOS was cross-linted per crate where possible. Windows was read by inspection, because `paneflow-app` needs `windows.h` and `llvm-rc`.
- Every deletion candidate showed zero readers on all three platforms and on the unmerged branch.
- Stories re-verify that evidence on current main before deleting, because main moves.

### Competitive Context
- **rust-analyzer:** keeps the code constantly refactored through small, focused PRs. We differ by batching per crate, because the audit already proved reachability. ([style guide](https://rust-analyzer.github.io/book/contributing/style.html))
- **Bun:** removed about 39k lines of dead Rust in one PR driven by reachability analysis. It noted that per-crate `dead_code` and `unreachable_pub` cannot see cross-crate use, which is why this audit added grep verification across crates. ([bun #35002](https://github.com/oven-sh/bun/pull/35002))
- **GreptimeDB:** replaced cargo-udeps with cargo-shear in CI. That CI adoption is a Non-Goal here. ([greptimedb #9294](https://github.com/GreptimeTeam/greptimedb/pull/9294))
- **rust-lang/rust:** reverted an over-aggressive dead-code analysis change, so lint output alone is not proof. ([rust #128404](https://github.com/rust-lang/rust/pull/128404))
- **Gap addressed:** per-crate lints and text-search tools miss three kinds of reachability: cfg-gated, cross-crate, and test-only. The audit combined per-target `--force-warn dead_code` with repository-wide grep across three platforms and the unmerged branch.

### Best Practices Applied
- Pure moves land in their own commit, and behavior changes land in a separate one. Unchanged passing tests are the evidence that behavior is preserved. ([atomic refactorings](https://www.codewithjason.com/refactorings-should-be-atomic/), [reviewing refactors](https://dev.to/pyor/how-to-review-a-refactor-j13))
- Target-scoped `cfg_attr(..., expect(dead_code, reason = ...))` instead of `allow`: an unfulfilled expectation fails under `-D warnings` (rustc lint levels documentation, Context7 `/rust-lang/rust`).
- Cargo resolver 2 keeps target-specific features from unifying across targets. Unification still applies within one target build, so a dependency removal is proven on each target's CI leg. ([Cargo features](https://doc.rust-lang.org/cargo/reference/features.html))
- cargo-deny warns on unused license and advisory entries by default, so stale entries are pruned in the same commit (cargo-deny docs, Context7).
- `.git-blame-ignore-revs` keeps blame usable across mechanical moves. GitHub supports it natively.

### Audit Traceability

| Story | Audit rows |
|-------|-----------|
| US-001 | R52 |
| US-002 | out-of-scope finding (`ipc_handler.rs:804` vs `reference.md:228`) |
| US-003 | R108 |
| US-004 | R142, R250, R257 |
| US-005 | R95 |
| US-006 | R217 |
| US-007 | R48 |
| US-008 | R237 |
| US-009 | R36, R38 |
| US-010 | R32, R33, R34, R35, R42, R212, R274 |
| US-011 | R20, R51, R127, R145, R147, R296 |
| US-012 | R15, R16, R23, R24, R25, R26, R27, R28, R45, R58, R59 |
| US-013 | R43, R78, R79, R80 |
| US-014 | R21, R44, R81, R82, R152, R178 (docs part), R188, R190, plus out-of-scope documentation findings |
| US-015 | R17, R19 |
| US-016 | R29, R37, R39, R40, R57, R144, R272 |
| US-017 | R83, R84, R85, R86, R87, R88, R128 |
| US-018 | R91, R92, R93, R94, R98, R99 |
| US-019 | R90, R104, R105, R106, R107, R109, R110, R113, R115, R116, R117, R118, R120, R122, R124, R125, R126, R232 |
| US-020 | R130, R132, R133, R134, R135, R136 |
| US-021 | R102, R137, R138, R139, R140, R141, R143 |
| US-022 | R149, R151, R153, R154, R155, R156, R157, R159, R162, R163, R165, R166, R168, R170, R197 |
| US-023 | R146, R148, R169, R172, R173, R176, R178, R181, R182, R183, R184, R185, R186, R194, R195, R294, R305 |
| US-024 | R41, R174, R175, R180, R187, R191, R192, R193, R196, R198, R199, R200, R201, R202, R301, R304 |
| US-025 | R247, R248, R252, R255, R259, R260, R263, R267, R268 |
| US-026 | R245, R246, R249, R251, R253, R254, R256, R258, R261, R262, R264, R265, R266 |
| US-027 | R213, R214, R216, R218, R219, R220, R222, R223, R226, R227, R229, R230 |
| US-028 | R231, R233, R236, R238, R239, R240, R241, R243 |
| US-029 | R206, R207, R208, R209, R210, R211, R270, R271 |
| US-030 | R276, R277, R278, R279, R280, R281, R282, R283, R284, R285, R292, R293 |
| US-031 | R179, R275, R286, R289, R290, R291 |
| US-032 | R297, R298, R299, R300, R302, R303, R306 |
| US-033 | R47, R53, R54, R55 |
| US-034 | R65, R67, R77 |
| US-035 | R66, R69, R70, R71, R72, R74, R76 |
| US-036 | R61, R62, R63, R64, R75 |
| US-037 | R60, R68, R73 |
| US-038 | R46, R49 |
| US-039 | R50, R129 |
| US-040 | R2 |
| US-041 | R3, R112, R119, R295 |
| US-042 | R11 |
| US-043 | R12, R13 |
| US-044 | R7 |
| US-045 | R4, R6, R9 |
| US-046 | R8, R10 |
| US-047 | R5 |

**Not scheduled:**
- **Audit verdict "keep":** R18, R22, R30, R89, R97, R100, R101, R103, R114, R121, R123, R158, R160, R161, R164, R171, R177, R189, R215, R221, R228, R234, R235, R244, R269.
- **Open Questions:** R96, R111, R131, R150, R167, R224, R225, R242, R273, R287, R288.
- **Non-Goals:** R1, R14, R56.
- **Rejected:** R31, R203, R204, R205, R307, R308, R309.

The mapping covers all 309 rows exactly once, checked by script against the local row file.

*Full research sources are listed inline above; the audit report itself is local evidence.*

## Assumptions & Constraints

### Assumptions (to validate)
- **Audit evidence still holds when each story starts.** Main moves after `e15277a0`, so each deletion first re-runs its row's reference search on current main and on `feat/agents-browser`.
- **`feat/agents-browser` will be merged or abandoned within the PRD window.** Until then, US-016, US-043, US-044, and US-045 stay blocked.
- **The Windows `cargo test` failures at `e15277a0` predate this PRD:**
  - `crates/paneflow-host/src/server.rs:2154`
  - `hyperlink.rs:894 perf_scan_200_lines_under_budget`

  They are the only red tests on that leg. Stories are measured against that baseline, not against a green leg.
- **A real macOS machine is available** for the Option key check in US-004.
- **The effects listed below were deduced by reading the code, never observed.** US-004, US-006, and US-007 reproduce or measure them first:
  - The Option-as-Alt loss (R250).
  - The headless misroute (R217).
  - The wrong-instance endpoint (R48).
  - The glyph protocol answering by default (R257).
- **No planned feature needs the libghostty wrapper modules with no production caller.** Git history keeps them recoverable at `e15277a0`.
- **`prd-persistent-session-reliability` EP-004 (Windows Qualification, IN_REVIEW) will be certified.** US-041 waits for it.

### Hard Constraints
- AGENTS.md gates apply to every story (see Quality Gates). Toolchain 1.98.0 is pinned: no `+stable` or `+nightly`.
- Any `Cargo.toml` change ships with its regenerated `Cargo.lock` in the same commit, because CI runs every cargo command with `--locked`.
- Tracked files are deleted with `gio trash <path>` followed by `git add -u`, never with `rm`.
- **No comments in source code** (Rust, shell, PowerShell, shim TypeScript assets). Intent goes into names, types, tests, and documents. Comments in TOML and YAML manifests are allowed.
- A new item in a file that has a `mod tests` is declared before that module, whatever its `cfg` (`clippy::items_after_test_module` fires on the Windows leg).
- `cfg(windows)` and `cfg(target_os = "macos")` code cannot be compiled on the Linux host. It is proven by the `windows_check` and `macos_check` jobs after push, or on real hardware. Each PR says which.
- **`docs/user/` is a mirror of paneflow-web and is never edited here.** A needed documentation change is listed in the PR description as a paneflow-web follow-up.
- **God-file splits are pure-move commits.** Any fix-up goes in a separate commit.
- **No story changes a public IPC method name, CLI command name, or accepted config key,** except where the story names it and states the compatibility behavior.
- Never pass `--profile <run-name>`. Use a scratch `CARGO_TARGET_DIR` outside the repository for parallel isolated builds.
- **Commits:**
  - Format: `type(scope): US-NNN - description`, atomic per story.
  - Arthur Jean is the only author.
  - No AI attribution and no `Co-authored-by`.
  - `Refs #...`, never an auto-closing keyword.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, run before every commit and push that touches Rust code
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - host-target lint including test modules
- `cargo test --workspace --locked` - workspace tests
- `cargo metadata --locked --format-version 1 > /dev/null` - the committed Cargo.lock matches every manifest

Additional gates by story type:
- `cargo deny check advisories licenses sources` - every story that edits a `Cargo.toml`, `Cargo.lock`, or `deny.toml`
- `windows_check` and `macos_check` jobs of `.github/workflows/run_tests.yml` green on the pushed branch - every story that edits code under `cfg(windows)`, `cfg(unix)`, `cfg(target_os = "macos")`, a target-specific dependency table, or a file with such code in its diff. A Windows `cargo test` failure present at `e15277a0` is not a regression.
- `release_build_linux` binary-size budget green - every story that edits `paneflow-ipc-client`, `paneflow-agent-config`, `paneflow-ai-hook`, `paneflow-mcp`, or `paneflow-shim`
- `scripts/bench-terminal.sh`, `scripts/bench-editor.sh`, or `scripts/bench-startup.sh` within 5% of the matching `bench/*baseline.json` - stories that touch the terminal render path, the code editor, or startup (named in each story)

For UI stories, additional gates:
- Manual GUI pass on Linux (Wayland) of every surface the story touches, with a screenshot in the PR.
- `macos_render_smoke` and `windows_render_smoke` green when the `rendering` path filter triggers them.

## Epics & User Stories

### EP-001: Latent Defects Surfaced by the Audit

The audit proved eight behavior defects while checking whether code was dead: one crash, two settings or API parameters that do nothing, two broken fallbacks, and engine options Paneflow never applies. They ship first because users hit them and none depends on cleanup.

**Definition of Done:** US-001 to US-008 are DONE. Each defect has a regression test that fails at `e15277a0`, or a recorded check on real hardware where no test can reproduce the platform effect.

#### US-001: Make agent text clamping UTF-8 safe and keep one implementation
**Description:** As a Paneflow user whose agent emits accented or CJK text, I want the host and the worker to truncate agent text on a character boundary so that a long message never panics a persistent process.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given an input of 4,096 "€" characters (12,288 bytes), when `clamp_text` runs, then it returns at most `MAX_AGENT_TEXT_BYTES` (4,096) bytes ending on a character boundary, without panicking (this input panics at `e15277a0`).
- [ ] Given an input whose 4-byte character starts at byte 4,094, when it is clamped, then the whole character is dropped and the output is 4,094 bytes long.
- [ ] Given an input containing NUL characters, when it is clamped, then every NUL is removed before the byte budget applies.
- [ ] Given ASCII input of 4,096 bytes or fewer without NUL, when it is clamped, then the output equals the input.
- [ ] `crates/paneflow-serve/src/activity.rs:60-68` no longer defines `clamp_text`; serve imports the host function, and `git grep -n "fn clamp_text"` returns exactly one hit, in `crates/paneflow-host/src/agent.rs`.
- [ ] A unit test beside `clamp_text` covers the multibyte, straddling, NUL, and ASCII cases.

#### US-002: Accept the documented `name` parameter in `surface.rename`
**Description:** As a script author following `docs/user/scripting/reference.md:228`, I want `surface.rename` to read `name` so that renaming a pane no longer clears its name.

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `{"surface_id": N, "name": "build"}`, when `surface.rename` runs, then the pane is named "build" (at `e15277a0` the name is cleared).
- [ ] Given `{"surface_id": N, "new_name": "build"}`, when it runs, then the pane is still renamed, so existing callers keep working.
- [ ] Given both `name` and `new_name`, when it runs, then `name` wins.
- [ ] Given neither key, an empty value, or a whitespace-only value, when it runs, then the name is cleared, as the reference documents.
- [ ] The control-character and bidi sanitization cases at `src-app/src/app/ipc_handler.rs:4696-4701` also pass for values sent as `name`.

#### US-003: Carry `notification_type` in the hook seed so replay restores attention
**Description:** As a user whose worker restarts while an agent waits for input, I want the replayed hook seed to keep the notification type so that the attention state survives the restart.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a hook payload carrying `notification_type`, when `paneflow-ai-hook` writes `last-hook-event.json` (`crates/paneflow-ai-hook/src/runtime.rs:204-241`), then the seed contains the same `notification_type`.
- [ ] Given a worker that replays the seed with `hook_revision == 0` (`crates/paneflow-serve/src/state.rs:613`), when the reader at `crates/paneflow-serve/src/hook_assets.rs:71` consumes it, then the attention state equals the one the live event produced (worker-level test).
- [ ] Given a payload without `notification_type`, when the seed is written and replayed, then the key is absent and replay behaves as at `e15277a0`.
- [ ] The `revision` key and the `encode_hook_seed` parameter stay unchanged in this story.

#### US-004: Apply the libghostty terminal options Paneflow exposes but never sets
**Description:** As a terminal user, I want three things applied by the engine: the glyph protocol disabled, `option_as_meta` honored, and Linux middle-click paste labeled as primary selection. That way the engine behaves as the config and the platform promise.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a new desktop or host session, when a program sends a Glyph Protocol query, then no reply is written to the PTY. `set_glyph_protocol(false)` is applied both in `configure_embedder_options` (`src-app/src/terminal/ghostty_session.rs:3751`) and in `new_terminal` (`crates/paneflow-host/src/runtime.rs:780`), and a test asserts the default no-reply behavior.
- [ ] Given `option_as_meta: true`, when a session starts or the config reloads, then the encoder receives `set_option_as_alt` with the matching value. An encoder-level test shows Option+a encoded with an ESC prefix, and `false` producing the composed character.
- [ ] Option+letter with `option_as_meta: true` is checked on a real macOS machine and the result is recorded in the PR; without that check the story stays IN_REVIEW.
- [ ] Given a Linux middle-click paste, when it reaches the runtime, then `RuntimeMessage::PasteInput` carries `ClipboardLocation::Primary`; macOS and Windows middle-click behavior is unchanged.
- [ ] `ClipboardLocation::Selection` and its `raw()` arm (`crates/paneflow-terminal-ghostty/src/terminal_ops.rs:64`, `:73`) are removed.
- [ ] The PR description lists the paneflow-web fix for the `option_as_meta` default shown as `true` at `docs/user/configuration/schema.md:70`, where the code default lives at `src-app/src/keys.rs:50`.

#### US-005: Honor the `external_editor` setting when opening files
**Description:** As a user who picked an editor in Settings > General, I want "open in editor" to launch it so that the setting stops being a silent no-op.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `external_editor` set to a preset, when a file location is opened (`src-app/src/app/event_handlers.rs:955`), then `editor::open_at_location` launches that editor at the line and column.
- [ ] Given `external_editor` unset, when a file location is opened, then the lookup order stays `$VISUAL`, `$EDITOR`, then the fixed list, as at `e15277a0`.
- [ ] Given a configured editor whose binary cannot be resolved on PATH (including `.exe` resolution on Windows), when a file location is opened, then Paneflow uses the unset lookup order and logs one warning naming the missing binary.
- [ ] Every value Settings can write, including `visual_studio`, is accepted by the `external_editor` enum in `schemas/paneflow.schema.json`, and a test fails if `EDITOR_PRESETS` and the schema enum diverge.
- [ ] Manual GUI pass on Linux: picking an editor in Settings, then opening a diff line, launches that editor.

#### US-006: Make the headless `surface.*` fallback address sessions instead of per-connection aliases
**Description:** As a CLI or MCP user with no desktop running, I want surface commands to reach the right host session so that the headless fallback works.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] A test first reproduces the failure: with a host running two sessions and no desktop, reading the second surface through `HostTransport` fails or reaches the wrong session at `e15277a0`. If it cannot be reproduced, the story stops and reports the observed behavior.
- [ ] The CLI selector (`src-app/src/cli/selector.rs`) and the MCP bridge (`crates/paneflow-mcp/src/bridge.rs`) send `session`, taken from `surface.list`, on `surface.*` calls over `HostTransport`.
- [ ] Given a session that exits between `surface.list` and the call, when the call runs, then the CLI returns its existing "surface not found" error and exit code, without a panic.
- [ ] The host `session` branch (`crates/paneflow-host/src/control.rs:197`) is kept, and the new test runs on the Linux, macOS, and Windows legs.
- [ ] The PR description lists the paneflow-web documentation of the headless behavior.

#### US-007: Resolve the controller endpoint with one rule in the desktop, CLI, and MCP
**Description:** As a developer who sets `PANEFLOW_HOME` to an isolated home from inside a pane of another instance, I want the CLI and MCP to apply the same endpoint rule as the desktop so that they never drive the other instance by mistake.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] A test first reproduces the failure: with an isolated `PANEFLOW_HOME` and an inherited `PANEFLOW_SOCKET_PATH` owned by another instance, `resolve_socket_path_or` (`crates/paneflow-ipc-client/src/lib.rs:565`) returns the inherited path at `e15277a0`.
- [ ] `paneflow-ipc-client` owns the single rule (`honored_socket_override`, `same_endpoint`, `PANEFLOW_ALLOW_SOCKET_OVERRIDE`, reserved names), and `chosen_endpoint` (`src-app/src/runtime_paths.rs`), the CLI resolver, and the MCP resolver all call it.
- [ ] Given an isolated home without `PANEFLOW_ALLOW_SOCKET_OVERRIDE=1`, when the CLI or MCP connects, then it uses the isolated home's endpoint; given the flag set to `1`, then it honors the inherited path.
- [ ] The same rule applies to `PANEFLOW_HOST_ENDPOINT` (`crates/paneflow-ipc-client/src/host_control.rs:85`).
- [ ] The tests at `src-app/src/runtime_paths.rs:349-425` move to `paneflow-ipc-client`, declared before its `mod tests`, with Windows named-pipe cases running on the Windows leg.

#### US-008: Reconcile adopted agent activity after a worker restart
**Description:** As a user whose worker restarts, I want adopted agent activity reconciled against the host session lifecycle so that finished agents do not stay busy. That behavior is described in `CHANGELOG.md:91` (0.16.0) and `docs/release/persistent-qualification.md:458`, and was lost when `c02e9ab8` removed the production call.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `reconcile_adopted(activity, &entry.lifecycle, now)` (`crates/paneflow-serve/src/activity.rs:178`) runs after the replay in `rebuild_from_home` and in `apply_core_snapshot`.
- [ ] Given an activity adopted as busy whose host lifecycle has ended, when the worker rebuilds, then the activity is reconciled as the function's unit test specifies (worker-level test).
- [ ] Given a live busy session, when the worker rebuilds, then its activity stays busy.
- [ ] Given a v0.16.0 host that reports `stale = true` in `raw["agent"]` (`crates/paneflow-serve/src/state.rs:871`), when the worker ingests it, then `stale` is unchanged.

---

### EP-002: Dependency, Build, and Tracked-File Hygiene

Remove the dependencies, build directives, tracked files, and CI steps that nothing reads, so later epics start from an accurate manifest and asset set.

**Definition of Done:**
- US-009 to US-014 are DONE.
- US-015 is DONE or CANCELLED according to OQ-2.
- US-016 is DONE, or CANCELLED with a follow-up once the `feat/agents-browser` outcome is known.
- Cargo.lock lists 837 packages or fewer.

#### US-009: Remove the unused `paneflow-app` dependencies left by the ACP chat removal
**Description:** As the maintainer, I want the dependencies `paneflow-app` never names removed so that nine dead packages leave the build graph.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `src-app/Cargo.toml` no longer declares:
  - `tokio`, `rfd`, and `syntect` (`:117-119`, `:132-139`, `:144-147`);
  - `tree-sitter-language` (`:164`);
  - `base64` (`:239`);
  - the entries at `:234-237`.

  `ignore` and its paragraph stay, and the header comment at `:99-100` no longer names tokio.
- [ ] The committed Cargo.lock no longer lists bincode, fancy-regex, linked-hash-map, plist, rfd, syntect, tokio, tokio-macros, or yaml-rust.
- [ ] Given `cargo tree --workspace --target all -i tokio`, when it runs, then it reports that the package is absent from the graph.
- [ ] Given a crate that relied on a feature only the removed dependencies unified, when a CI leg builds it, then the leg fails, the story stays IN_PROGRESS, and the PR names the consumer.
- [ ] License or advisory entries that `cargo deny` reports as unused after the removal are pruned from `deny.toml` in the same commit.

#### US-010: Prune unused crate dependencies and test-only `windows-sys` features
**Description:** As the maintainer, I want unused dependencies removed and test-only dependencies moved to dev-dependencies so that release builds link only what production code calls.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `interprocess` moves from `[dependencies]` to `[dev-dependencies]` in `crates/paneflow-ai-hook/Cargo.toml:13`.
- [ ] In `crates/paneflow-serve/Cargo.toml`, the `cfg(unix)` `libc` table (`:27-28`) and the `cfg(windows)` `windows-sys` entry (`:32-38`) are removed; `widestring` and the `windows-sys` dev-dependency stay.
- [ ] `serde` is removed from `crates/paneflow-telemetry/Cargo.toml:12`.
- [ ] In `crates/paneflow-host/Cargo.toml`, the `windows-sys` features at `:53` and `:56` are removed. The features at `:57`, `:60`, and `:61` move to `[target.'cfg(windows)'.dev-dependencies] windows-sys`. `Win32_Security` and `Win32_System_Kernel` stay in normal dependencies.
- [ ] In `src-app/Cargo.toml`:
  - the feature at `:337` is removed;
  - the test-only features at `:329`, `:340`, `:390`, and `:391` move to `[target.'cfg(windows)'.dev-dependencies] windows-sys` 0.59;
  - `Wdk_System_Threading` stays;
  - the comment at `:323-324` names `Win32_System_Diagnostics_ToolHelp` and `_Debug` (`ReadProcessMemory`, `src-app/src/workspace/ports.rs:824`).
- [ ] Given a removed feature that production code reached through feature unification, when the Windows release build step runs, then it fails, and the story stays IN_PROGRESS naming the item.
- [ ] `paneflow-ai-hook` does not grow in size.

#### US-011: Remove build-script, lint-config, and deny leftovers
**Description:** As the maintainer, I want redundant build directives and duplicated configuration removed so that each setting has one source.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] The `wxs` key (`src-app/Cargo.toml:472`) is removed; `name`, `upgrade-guid`, and `path-guid` stay.
- [ ] `src-app/build.rs:10-11` are removed and line 12 (`PANEFLOW_SKIP_EMBED_BUILD`) stays. Given an edit to a file under `src-app/assets`, when `cargo build` runs again, then the asset is re-embedded.
- [ ] The call at `crates/paneflow-libghostty-sys/src/build_support/mod.rs:16`, `emit_ghostty_native_cfg` (`:86-91`), and `ghostty_native_target` (`:93-103`) are removed. `crates/paneflow-terminal-ghostty/build.rs` still emits `ghostty_native`.
- [ ] `deny.toml:124-130` (the `exceptions` key and its comment) is removed, and `cargo deny` reports no new warning.
- [ ] The per-crate `clippy.toml` files of `paneflow-telemetry`, `paneflow-mcp`, and `paneflow-mcp-install` are trashed; the root `clippy.toml` is untouched. A diff of each copy against the root, recorded in the PR, shows no setting lost.
- [ ] Given that the `rust` path filter (`.github/workflows/run_tests.yml:86`) does not trigger on `clippy.toml`-only changes, the clippy gate is run locally and its output attached to the PR.

#### US-012: Delete unreferenced tracked assets and packaging leftovers
**Description:** As the maintainer, I want assets and packaging files that no build, package, or code path reads removed so that the repository ships only what it uses.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] The two unused hicolor PNGs under `assets/icons` (audit row R15) are deleted. The `build-icons.sh:126` loop becomes `16 32 48 128 256 512`, and `scripts/verify-libghostty-windows.ps1:373` and `:376` are removed; `build-icons.sh:159`, `:170`, and `:175` are untouched.
- [ ] Deleted outright:
  - `assets/images/social-preview.png`;
  - `src-app/LICENSE`;
  - `packaging/rpm/Packages/.gitkeep` with `packaging/rpm/README.md`;
  - `src-app/assets/icons/languages/manifest.json` with its README;
  - the 28 SVGs listed in audit row R27. `icons/moon.svg` is not one of them.
- [ ] `packaging/macos/paneflow.nightly.entitlements` is deleted, and `docs/release/macos-signing.md:42-44`, `:50`, and `:181-182` describe the release variant only.
- [ ] `packaging/winget/` (3 files) and the winget comments at `.github/workflows/release.yml:10-12`, `:1746-1747`, and `:3086-3087` are deleted, unless OQ-1 is answered "keep" before implementation. The AppStream entry at `metainfo.xml:239` is untouched.
- [ ] `packaging/wix/main.wxs:242` points at `assets/PaneFlow.ico`. `packaging/wix/paneflow.ico`, `OUT_WIX_ICO`, `build-icons.sh:18` and `:177-179`, and `verify-libghostty-windows.ps1:380` are removed, and the comments at `main.wxs:13` and `:240-241` match.
- [ ] `src-app/src/app/sidebar/mod.rs:1239` and `:1241` use `icons/folder-open.svg` and `icons/folder.svg`. The duplicate SVGs are deleted, `THIRD_PARTY_NOTICES.md` lists `folder-open.svg`, and `DESIGN.md:378-382` matches.
- [ ] Given each deleted path, when `git grep -n <basename>` runs on main and on `feat/agents-browser`, then no reference remains outside `CHANGELOG.md`; any hit keeps that path.
- [ ] Given an rpm built locally with cargo-generate-rpm, when `rpm -qlp` runs, then it still lists `/usr/share/doc/paneflow/LICENSE`.
- [ ] Manual GUI pass on Linux shows the sidebar folder icons. The MSI icon is confirmed by the next `release.yml` Windows leg, and until then the story stays IN_REVIEW.

#### US-013: Remove dead and duplicated CI workflow steps
**Description:** As the maintainer, I want CI steps that run nothing useful removed and the Linux build dependency list defined once, so that workflows stay in sync.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `release.yml:97-111` and `:227-253` use `./.github/actions/linux-build-deps`. The second use has `if: runner.os == 'Linux'` and `extra-packages`. The comments at `release.yml:255-260` and `.github/actions/linux-build-deps/action.yml:25` match.
- [ ] `libghostty-linux.yml:230-234` is removed, the step at `:223` is renamed after the stress check it keeps, and "size" is removed from `docs/release/libghostty-linux.md:38`.
- [ ] `libghostty-linux.yml:380` no longer passes `abi_layout`.
- [ ] `libghostty-windows.yml:21`, `:22`, and `:24` are removed, and the cache key at `:109` becomes `${{ env.TARGET }}-${{ hashFiles('Cargo.lock', 'native/libghostty/manifest.toml') }}`.
- [ ] The trigger comments at `release.yml:8-16` describe the current Homebrew and download-only Windows flows.
- [ ] Given each edited workflow, when it is dispatched (`libghostty-linux.yml`, `libghostty-windows.yml` with a cold cache, `release.yml` as a dry run), then the run is green; a red run keeps the story IN_REVIEW.

#### US-014: Fix documentation and repository-config drift
**Description:** As a reader of the repository documentation, I want stale paths, missing assets, and wrong names corrected so that the documents match the code.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `docs/evidence/` and `.gitignore:49-51` are removed.
- [ ] `ABOUT.md` is removed, and the PR description links the paneflow-web change that updates its `AGENTS.md:10` reference.
- [ ] In `context7.json`, the `devto-v0.3.4-hardening.md` exclusion and its trailing comma are removed, and `rules[1]` names `~/.paneflow` instead of `~/.config/paneflow`.
- [ ] In this repository:
  - `DESIGN.md:45` no longer references the missing `demo-0.9.png`;
  - `ARCHITECTURE.md:863` no longer names the nonexistent `session.ensure`;
  - `docs/release/linux-signing.md:221` no longer names `update_checker.rs`;
  - `schemas/paneflow.schema.json:193` and `DESIGN.md:948` document the coercion kept by audit row R152.
- [ ] Environment variables are documented:
  - AGENTS.md documents `PANEFLOW_DEV_INSTALL_METHOD`, `PANEFLOW_DEV_FORCE_UPDATE`, `PANEFLOW_PIXEL_PROBE`, and `PANEFLOW_PIXEL_PROBE_OVERLAY`;
  - `docs/release/runbook.md` documents `PANEFLOW_UPDATE_EXPLANATION` for packagers.
- [ ] The PR description lists the paneflow-web follow-ups:
  - the broken link at `docs/user/installation/windows.md:118`;
  - the `notify_when_agent_waiting` values at `docs/user/configuration/schema.md:140`;
  - `docs/user/scripting/reference.md:288-294`, since the hook now posts `agent.event` to the host;
  - the `#multiple-installs` anchor that `src-app/src/app/bootstrap.rs:434` links to;
  - the schema documentation for the R152 coercion.
- [ ] Given a fix whose target is under `docs/user/`, then the file is not edited in this repository.

#### US-015: Apply one retention rule to `bench/results` (conditional on OQ-2)
**Description:** As the maintainer, I want benchmark results pruned by a single written rule so that `bench/results` keeps only runs that documents cite.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] The story runs only if OQ-2 is answered "prune". Otherwise it is set to CANCELLED.
- [ ] Only the 32 unreferenced files named by audit row R17 are trashed. Kept:
  - the runs cited by `bench/README.md`;
  - the runs cited by `docs/release/persistent-qualification.md:79-80`;
  - the qualification runs of audit rows R18 and R22;
  - the two runs of row R19.
- [ ] Given a run whose file name appears anywhere under `docs/` or `bench/` (`git grep`), when the pruning runs, then that run is kept.
- [ ] `bench/README.md:5-7` states the retention rule and `:448` is corrected in the same commit.

#### US-016: Resolve the items held by `feat/agents-browser` and the active qualification
**Description:** As the maintainer, I want the branch-blocked findings resolved once the branch outcome is known so that nothing the branch uses is deleted and nothing orphaned survives.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012, US-024

**External blocker:** `feat/agents-browser` merged into main or abandoned (OQ-12). For `native/libghostty/manifest.toml`, the next libghostty pin bump or the end of the active qualification.

**Acceptance Criteria:**
- [ ] If the branch merged:
  - `src-app/Cargo.toml:190-193` (uuid) and `:279-281` (wayland-client, wayland-protocols) describe the browser use;
  - `icons/moon.svg` stays;
  - `mcps/` is removed, including the 15 `browser_*.json` manifests;
  - the drift test `static_manifests_match_runtime_specs` (`crates/paneflow-mcp/src/tools.rs`) is removed, keeping `schemas_use_safe_integer_targets_and_explicit_maxima`.
- [ ] If the branch was abandoned:
  - uuid (`:194`), wayland-client and wayland-protocols (`:283-284`), and `icons/moon.svg` are removed;
  - `mcps/` and the drift test are removed;
  - Cargo.lock is regenerated.
- [ ] `packaging/paneflow-release.asc` follows option A of audit row R57:
  - the cargo-deb asset and the `libghostty-linux.yml:72` filter point at `keys/`;
  - the copy is deleted;
  - `keys/README.md:24-35` and `docs/release/linux-signing.md:71-75` are purged.
- [ ] `native/libghostty/manifest.toml:27` (`emit_mode`) and `:2` (`source_repository`) are removed in the same commit as the next pin bump, never during an active qualification run.
- [ ] Given an asset still referenced by string on main after the merge, when this story runs, then it is kept, because rust-embed does not report a missing asset at build time.

---

### EP-003: Dead Code Removal by Crate

Delete the production items that no caller reaches on any shipping target, one crate or area per story, so each diff reviews as a pure deletion.

**Definition of Done:** US-017 to US-024 are DONE, and for every deleted item a `git grep` on main and on `feat/agents-browser` finds no remaining reference.

#### US-017: Remove dead code from `paneflow-agent-config` and `paneflow-mcp-install`
**Description:** As the maintainer, I want the unused agent-config APIs and descriptor fields removed so that the helper binaries and the runtime catalog carry only live data.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] The following are removed:
  - `render_bare_hook_command` (`claude_hooks.rs:167-184`), `hook_command.rs:43-45`, and their `pub use` entry;
  - the five `io.rs` functions with their tests and imports, with `lib.rs:19-22` reduced to `claude_config_dir` and `home_dir`;
  - `lease.rs` with `lib.rs:8-9` and `:23-24`;
  - `lock.rs:55-58`;
  - the five unused `runtime_catalog.rs` fields, `RuntimeInstall`, and `runtime_for_integration_install`, with their `build_support.rs` structs, validation, codegen, and fixture;
  - `Partial` (`build_support.rs:83`, `:603`, `runtime_catalog.rs:18`);
  - `uninstall_all` and `UninstallReport` (`paneflow-mcp-install/src/api.rs:49`, `:171-173`, `lib.rs:12-15`).
- [ ] The `config-io` feature no longer enables `tempfile`, and `tempfile` moves to dev-dependencies.
- [ ] The removed keys are deleted from every checked-in `runtime.toml`, and the generated catalog is otherwise byte-identical before and after (diff recorded in the PR).
- [ ] `None` stays as the descriptor sentinel, and the `hook_adapter` enums keep every variant (audit row R89, decision: keep as descriptor documentation).
- [ ] Given a `runtime.toml` that still declares a removed key, when the build runs, then `build_support.rs` either rejects it with an error naming the key or ignores it, and a test pins which.

#### US-018: Remove dead code from `paneflow-config`
**Description:** As a user with an existing `paneflow.json`, I want unused config fields removed without breaking my file so that the schema only promises what Paneflow reads.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] The following are removed:
  - the unused const at `crates/paneflow-config/src/loader.rs:8-13`;
  - the `agent_panel` profile fields (`schema/agent_panel.rs:7-12`), `ProfileConfig`, `ThinkingDisplayMode` with its `Deserialize`, and their consts and resolvers;
  - `tool_permissions` (`schema/config.rs:117-122`) and `ToolPermissionsEntry` (`:512-519`).
- [ ] The retired `"auto"` alternative (`schema/session.rs:143`) and its test are removed.
- [ ] The session field at `schema/session.rs:209-210` is removed, together with `:304`, `:310-314`, and `src-app/src/app/session.rs:72`.
- [ ] `schemas/paneflow.schema.json` drops the removed properties, and the schema drift test passes.
- [ ] Given a `paneflow.json` containing any removed key, when it loads, then loading succeeds and the effective config equals the same file without that key (fixture test).
- [ ] Given a `session.json` written by `e15277a0` that contains the removed session field, when it loads, then the restored layout is identical.
- [ ] Given the retired value `"auto"`, when it is read, then it resolves to `User`. This behavior change is stated in the PR description.
- [ ] The PR description lists the paneflow-web changes for the removed keys (6 locales) and notes that `feat/agents-browser` resolves its use of the loader const through paneflow-home at rebase.

#### US-019: Remove dead code from `paneflow-host`, `paneflow-ipc-client`, and `paneflow-ai-hook`
**Description:** As the maintainer, I want unused host protocol items, re-exports, and hook fields removed so that the host API and the hook frame carry only what clients read.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001, US-003, US-007

**Acceptance Criteria:**
- [ ] In `paneflow-host`, the following are removed:
  - `AgentEvent.source` and its parse (`agent.rs:131-135`, `:161`, `:215`, `AgentStateSource::parse`), keeping `SOURCE_TAKEOVER_SILENCE` and `accepts_source`;
  - `cold_text.rs:45-55`;
  - `host.rs:838-840` and `:937-939`;
  - the 21 unused names in `lib.rs:38-56`, keeping `SessionLifecycle`;
  - the revision plumbing in `persistence.rs:77-79`, `:196`, `:212`, and `:217-219`;
  - `ProcessVerdict::wire_str` (`process.rs:53-61`);
  - `protocol.rs:15` with its assert;
  - `protocol.rs:21` and `:220-224`;
  - `runtime.rs:24`;
  - `LaunchHandle::generation` (`runtime.rs:388-391`);
  - the `data` payload of incompatibility errors (`server.rs:562-578`, `:600`, `:636`, `:644`, `:648`, `:658`, `:666`).
- [ ] `ARCHITECTURE.md:869` states 32 KiB chunks inside 64 KiB frames.
- [ ] In `paneflow-ipc-client`, `ai_hook.rs:6`, `host_control.rs:19`, `:21`, `:47-49`, and `:241-243` are removed.
- [ ] In `paneflow-ai-hook` and `paneflow-ipc-client`, `AiHookParams.workspace_id` and `.surface_id`, `FrameContext` fields `:95` and `:98`, `SURFACE_ID_ENV`, `read_surface_id_from`, and their initializers and asserts are removed.
- [ ] Given a hook or client from `e15277a0` that still sends the removed `source` key or `result` alias, when the host parses the event, then the key is ignored and the event is accepted.
- [ ] Given an incompatible client handshake, when the host rejects it, then the client still receives the same error message string.

#### US-020: Remove dead code from `paneflow-serve`
**Description:** As the maintainer, I want unused worker controller methods, re-exports, and frames removed so that the worker protocol matches what the desktop sends and reads.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-008

**Acceptance Criteria:**
- [ ] The following are removed from `paneflow-serve`:
  - the two controller methods at `controller.rs:142`;
  - the accessors `controller.rs:239-241`, `server.rs:238-240`, and `core_link.rs:48-50`, with the field at `core_link.rs:22`;
  - the `lib.rs:24-37` re-exports beyond the 12 names the desktop uses.
- [ ] `drain_ms` is no longer sent by `crates/paneflow-serve/src/bootstrap.rs:342`. The `--drain-ms` client flag stays with its help text updated at `src-app/src/cli/serve_cmd.rs:19`, and `worker.hello` calls are replaced by `control.identity()`. The sender at `crates/paneflow-host/tests/persistent_baseline.rs:790` changes in US-041.
- [ ] `state.rs:860` is removed, `:899` uses `now_ms()`, and `live`, `hook_revision`, `host_instance`, and `following` are removed from the snapshot entry (`state.rs:1089`, `server.rs:866`, `:883`, `:885`). The `raw["agent"]` read is kept.
- [ ] The streaming broadcast (`worker.rs:263-264`) sends only `{"type":"snapshot","sessions":...}`, and the unread `core_disconnected` frame (`worker.rs:290-292`) is removed.
- [ ] `Controller::activity_log` (`controller.rs:171-180`) is kept pending OQ-9.
- [ ] Given a worker started by the previous release, when the updated desktop connects, then the existing `WORKER_PROTOCOL_VERSION` negotiation applies unchanged: this story does not bump the protocol version.

#### US-021: Remove dead code from `paneflow-terminal-ghostty`, `paneflow-ghostty-smoke`, and `paneflow-telemetry`
**Description:** As the maintainer, I want unused engine wrappers, the stub backend, and dead telemetry helpers removed so that every shipping target compiles only the native engine path it uses.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] `crates/paneflow-telemetry/src/tags.rs` and `pub mod tags;` (`lib.rs:14`) are removed, and the description at `Cargo.toml:8` names Paneflow correctly; `src-app/src/telemetry/tags.rs` is untouched.
- [ ] Removed from `paneflow-terminal-ghostty`:
  - `DisplayTerminal::reset` (`engine.rs:93-98`), `CallbackState::reset_working_directory` (`callbacks.rs:99-101`), and the stub twin;
  - `input.rs:45-46`;
  - `selection_gesture.rs:414-420`;
  - `tests/native_search.rs:240-262`.
- [ ] `stub.rs`, `lib.rs:65-66`, `lib.rs:165-166`, and the `UnsupportedPlatform` error variant (`error.rs:3-6`) are removed. `IntegrationState::UnsupportedPlatform` stays.
- [ ] In `paneflow-ghostty-smoke`, `mod windows` (`main.rs:129-194`) and the Windows `main` (`:205-212`) are removed, the fallback at `:214-218` becomes `#[cfg(not(unix))]`, the `Cargo.toml:29` gate lists Linux and macOS, and the `libghostty-windows.yml:53` filter matches.
- [ ] Given a build for a non-shipping target, when it runs, then `src-app/build.rs` still fails with its existing libghostty message before any missing-item error.

#### US-022: Remove dead code from the `src-app` shell, agents, IPC, and CLI
**Description:** As the maintainer, I want unused startup helpers, IPC channels, session readers, and CLI fallbacks removed so that the application shell reads as it runs.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-005

**Acceptance Criteria:**
- [ ] Removed from the application shell:
  - `bootstrap.rs:944-971`, with its name in the macOS `use` at `main.rs:82-84` and `main.rs:1859-1860`;
  - the unused `window` parameter of the two `constants.rs:164` functions, their 8 calls and 7 tests, and of `render_sessions_sidebar` and its call at `main.rs:1324`;
  - `document.rs:21-27` and `edit.rs:507-513`;
  - the `_doc` parameter of `highlight.rs:299`;
  - `event_handlers.rs:45-49` and `:118-121`, `constants.rs:110-114`, and `workspace_ops/mod.rs:1103-1108`, with `constants.rs:201-211` reduced;
  - the progress variant and its arms (`terminal/view.rs:1199-1201`, `:1540`, `:1569-1572`, `event_handlers.rs:959-961`, `pane.rs:616`);
  - `workspace_ops/mod.rs:404-423`.
- [ ] Removed from IPC:
  - the unused IPC channel `ipc_handler.rs:306-406`, with `:49`, `:95-98`, `:469`, and `:474-477`;
  - `stage_planned_pane_env`, inlined at `:1407` and `settings/tabs/workspaces.rs:1350`;
  - `IpcRequest._id` (`ipc.rs:22`) and its three initializers.
- [ ] Removed from the CLI and session readers:
  - `legacy_error_message` (`cli/wait_cmd.rs:149-155`, `cli/flow_cmd.rs:753-759`) and its definitions, with one `is_surface_gone_error` kept;
  - `read_cursor_sessions_for_cwd` and `SessionAgent::Cursor`;
  - the Hermes reader, `CommandScope`, and `SessionAgent::Hermes`.
- [ ] `worker_bootstrap.rs:88` becomes `banner(&WorkerBoot) -> Option<String>`, and `sidebar/mod.rs:748-750` is adjusted.
- [ ] The v1 session repair moves into the v1 branch of `load_session_at` (`app/session.rs:347-357`), and its call at `:459-467` is removed. A v1 fixture is repaired, and a v3 root "Terminal 1" stays intact.
- [ ] Given a value produced by `SessionAgent::index()` at `e15277a0`, when the variants are renumbered, then either the value maps to the same agent or the PR records a grep proving `index()` is never persisted.
- [ ] Given a surface-gone error from a desktop at `e15277a0`, when the CLI receives it, then it still reports "surface gone" through `is_surface_gone_error`.
- [ ] Manual GUI pass on Linux: window material, sidebar worker banner, and pane progress rendering are unchanged.

#### US-023: Remove dead code from the `src-app` terminal, markdown, widgets, theme, and diff areas
**Description:** As the maintainer, I want unused terminal plumbing, shell emitters, widget slots, and theme aliases removed so that the render path contains only exercised code.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Removed together in one commit:
  - `CalloutIcon::Info` and `icons/bulb.svg`;
  - the four unused callout variants with their slots, arms, and five allowances;
  - the stale `DESIGN.md:219`, `:282`, and `:855-857` references.
- [ ] Removed from the terminal and PTY plumbing:
  - the Unix parent-guard items (`agents/parent_guard.rs:12` and imports), the test at `:192-202`, the dispatch at `main.rs:1659-1662`, and the `pty_guard` field with its sites in `pty_session.rs`;
  - `capture_foreground_signal_mask` and the `signal_mask` parameter at its 5 callers, with the log at `main.rs:1761` corrected;
  - `Osc52Mode::CopyPaste` and `LOAD_ALLOWED` (`terminal/clipboard_gate.rs:11`, `:26-28`), with the production call becoming `set_policy(mode != Osc52Mode::Disabled)`;
  - the unused mark fields and the OSC 133 D branch in `terminal/marks.rs`;
  - `types.rs:340` (`id`) with its initializers;
  - the three hover methods at `terminal/view.rs:1131-1175`.
- [ ] The OSC 133 D emitters and their exit-code bookkeeping are removed from the zsh, bash, fish, and pwsh snippets in `terminal/shell.rs`. A, C, and `$__paneflow_last_exit` are kept, and the D literals in `selection.rs:378`, `display_terminal.rs:482`, and `bench_corpus.rs:51` stay.
- [ ] Removed elsewhere:
  - the 31 syntax aliases and the `enum` slot (`diff/syntax.rs:20`, `theme/model.rs:47`, `:898` indices 15, 18, 23 → 14, 17, 22);
  - `markdown/mod.rs:9-10`, replaced with `pub(crate) use parser::MAX_INPUT_BYTES;`;
  - `markdown/view.rs:792-813`;
  - `theme/builtin.rs:62`;
  - `ALIGNMENT_EPSILON`, `assert_pixel_aligned`, and their three tests (`pixel_probe.rs:7-8`, `:110-117`, `:123-139`).
- [ ] The text-area chip and decoration plumbing (`widgets/text_area.rs:151` and the related items) is removed. `ranges_overlap`, `split_keeping_newlines`, `LineSlice`, and their tests go in the same commit, and so does `text_area.rs:1`.
- [ ] Given any value the config schema accepts for OSC 52, when it loads, then it maps to the same clipboard policy as at `e15277a0`; the PR records a grep proving no config value produces `CopyPaste`.
- [ ] Given a config naming the removed theme alias, when it loads, then Paneflow applies its existing unknown-theme behavior without a panic (test).
- [ ] Manual GUI pass:
  - on Linux: typing, paste, IME composition, and Enter in text inputs, callouts, and syntax colors in the diff and code views;
  - on the Windows dual-boot: a pwsh prompt still produces prompt marks.

#### US-024: Remove dead platform-gated code in the Linux backdrop, updater, system info, and workspace modules
**Description:** As the maintainer, I want unreachable platform branches, the abandoned Linux blur machinery, and the unused MSI install path removed so that each target compiles only what it runs.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009, US-010

**Acceptance Criteria:**
- [ ] Linux blur machinery in `window_chrome/linux_backdrop.rs`:
  - kept: `unblurred_*`;
  - reduced: `apply_subtle_chrome_material` and `refresh_blur_region`, down to `set_background_appearance`;
  - removed: every other item, with `main.rs:905-914`, `quit_dialog.rs:400-401`, and the two dead tests;
  - `x11rb` (`src-app/Cargo.toml:285`) is removed and Cargo.lock regenerated;
  - `DESIGN.md:77` and `:951` are corrected;
  - `wayland-client`, `wayland-protocols`, and `raw-window-handle` stay (US-016).
- [ ] Platform branches:
  - `runtime_paths.rs:193-195` is removed;
  - the unreachable fallbacks in `system_info.rs:269-277` and `:351-359` are removed;
  - `any(linux, freebsd)` becomes `target_os = "linux"` in `system_info.rs` and `terminal/input.rs:936`.
- [ ] Updater:
  - the unused `install_method.rs:166` parameter is removed from `classify` and its 17 test calls;
  - `SelfUpdateStatus::Errored` becomes a unit variant (`update/mod.rs:29`, `main.rs:798`, `self_update_flow.rs:78`);
  - the unused MSI `install` path (`update/windows/msi.rs:129`) and its tests are removed, and `relaunch_paneflow` is called unconditionally at `msi.rs:397-400`;
  - `update/error.rs:23` and the `msi.rs:986` allowances become `cfg`-scoped.
- [ ] Workspace:
  - `workspace/mod.rs:99-100` and `:171` are removed;
  - the `pid_resolve.rs:84-88` stub is removed;
  - the tautological `cfg` attributes in `workspace/ports.rs` (`:17`, `:20`, `:75`, `:88`, `:1016-1023`, `:1129`, `:1147`) are removed;
  - `legacy_worktree_dir`, `legacy_worktree_dir_hashed`, their arms, and the `created_at` fallback (`workspace/worktree.rs:326-333`, `:386-387`, `:393`) are removed.
- [ ] Given a Linux session on Wayland and one on X11 (`WAYLAND_DISPLAY` unset), when Paneflow starts, then the chrome renders opaque and identical to `e15277a0` (manual GUI pass with screenshots).
- [ ] Given a Windows update from the previous release, when the MSI relay path (`msi.rs:416-490`) runs, then it installs and relaunches. This is proven by the update end-to-end job of the next `release.yml` run or on the Windows dual-boot; until then the story stays IN_REVIEW.

---

### EP-004: Test-Only Gating, libghostty Wrapper Pruning, and Target-Scoped Lint Attributes

Move items that only tests reach behind `cfg(test)` or delete them, prune the libghostty wrappers with no production caller, compile the in-process PTY runtime for tests only, and replace broad allowances with target-scoped attributes.

**Definition of Done:**
- US-025 to US-032 are DONE; US-025 and US-026 are CANCELLED if OQ-3 is answered "keep".
- The `#[allow(dead_code)]` count is 20 or fewer.
- A release build of `paneflow-app` compiles no item of the `GhosttySession::start` chain.

#### US-025: Remove libghostty wrapper modules with no production caller (default of OQ-3)
**Description:** As the maintainer, I want wrapper modules that only their own tests use removed so that the engine crate exposes only what Paneflow calls.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-021

**Acceptance Criteria:**
- [ ] Removed from `crates/paneflow-terminal-ghostty/src`, with their `native_modules!` entries and re-exports in `lib.rs`:
  - `io.rs`, `modes.rs` (with `options.rs:7` and `:147-181`), and `osc.rs`;
  - `render.rs`, after its dirty-row tests are ported to `snapshot().dirty_rows`;
  - `sgr.rs`, `tracked.rs`, and `unicode.rs`;
  - `grid_ref.rs`, with `style.rs` reduced to `style_color`.
- [ ] `formatter.rs`: the three Html methods, the `Html` variant, and its arm are removed, and the styling test is renamed without its Html block.
- [ ] The generated bindings under `native/libghostty/` are untouched.
- [ ] Given a module that `feat/agents-browser` imports (`git grep` on the branch), when the story runs, then that module is kept and the PR names it.
- [ ] Given OQ-3 answered "keep as test surface", then each module becomes `#[cfg(all(test, ghostty_native))] mod x;` instead of being deleted.

#### US-026: Prune test-only methods from the remaining libghostty wrappers
**Description:** As the maintainer, I want methods that only tests call removed or gated so that the remaining wrapper API is the API Paneflow uses.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-025, US-004

**Acceptance Criteria:**
- [ ] The following test-only items are removed, with their tests and now-unused imports:
  - in `color.rs`, the functions, the constant, and the imports `CStr`, `OnceLock`, `encode_with_buffer`, `check`, `ColorScheme`, and `Result`;
  - the 8 input variants and their mappings (`input.rs:19`, `input_map.rs:33-43`, `:104-110`, `:121-127`, `display_terminal.rs:305-318`);
  - the `input_options.rs:89` items and the backarrow field;
  - the `kitty.rs:446` methods and `PlacementLayer`;
  - `Scroll::Top` (`model.rs:257`, `engine.rs:129-132`);
  - `native_search.rs:319-321` and its stub twin;
  - the `options.rs:41` setters and the readable-clipboard accessors, with `clipboard_read` reduced to `denied_read()`;
  - the one-shot search (`search.rs:193-224`, `native_search.rs:162-168`, `:340-353`);
  - the unused selection enums and methods and `select_word`/`select_line` (`selection.rs`, `navigation.rs`, `stub.rs:90-96`);
  - the `snapshot_codec.rs` items;
  - the `sys.rs` allocator and logger hooks;
  - the `terminal_ops.rs` paste-source, compression, size-report, and `feed_until_ground` items.
- [ ] The three `color.rs:47` methods get `#[cfg(test)]` inside their existing impl, before `mod tests`.
- [ ] `set_option_as_alt` and `set_glyph_protocol` are kept, because US-004 calls them in production.
- [ ] Given the integration tests in `crates/paneflow-terminal-ghostty/tests`, when they run, then only the cases for removed methods are gone and every other case passes unchanged.
- [ ] Manual GUI pass on Linux: selection by drag, double-click, and triple-click, search, and kitty image display behave as at `e15277a0`.

#### US-027: Gate or remove test-only items in `paneflow-host`
**Description:** As the maintainer, I want host items that only tests read gated or removed so that the host binary carries production code only.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-019

**Acceptance Criteria:**
- [ ] `subscriber_count` becomes `#[cfg(test)]`, declared before `mod tests`, and `lifecycle` is removed with its asserts (`agent.rs:286`, `:396-400`, `:415-420`).
- [ ] The owner-lock `path` field and accessor are removed (`bootstrap.rs:42`, `:52`, `:62-64`), and the test compares against `paneflow_home::host_dir_in(home.path()).join(OWNER_LOCK_FILE_NAME)`.
- [ ] Removed:
  - `control.rs:107-109` and its assertion;
  - the four ack keys at `host.rs:1279-1404`, with `ARCHITECTURE.md:636-637` keys kept;
  - the `host.rs:651` field, with publication from `record.event` and its 14 readers rewritten through `subscribe_agents`;
  - `pending_launches` and `live_session_count` (5 calls → `live_sessions().len()`);
  - the four status keys at `server.rs:751-778` and `protocol.rs:54-80`.
- [ ] `manifest.rs:332-342` is moved into tests or inlined, and the `persistence.rs:21` constant becomes a private const inside `mod tests`, keeping the test name cited by `docs/release/persistent-qualification.md:373`, `:398`, and `:419`.
- [ ] `spawn` (`runtime.rs:427`) and `runtime.rs:571` get `#[cfg(test)]` in place.
- [ ] `tail.rs:37` is replaced at `stream.rs:75-78` by `.map_or(0, OutputTail::retained_bytes)`.
- [ ] Given every removed ack or status key, when a `git grep` runs across `src-app`, `crates/paneflow-serve`, `crates/paneflow-mcp`, and the CLI, then no client reads it (grep recorded in the PR).

#### US-028: Gate or remove test-only items in `paneflow-ipc-client` and `paneflow-serve`
**Description:** As the maintainer, I want client and worker items that only tests reach removed, including the unused worker front door, so that the worker exposes only what the desktop calls.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-019, US-020

**Acceptance Criteria:**
- [ ] In `paneflow-ipc-client`:
  - `Working` and `Idle` are removed (`agent.rs:112-113`, `:156-167`, test `:237-249`), and `ipc_handler.rs:4335`, `:4371`, and `:4396` use `PromptSubmit`;
  - the legacy frame builders `ai_hook.rs:246-276` and `:290-296` are removed after the ai-hook tests move to `to_agent_event_params`;
  - the two `line_wire.rs:185` wrappers are removed, and `server.rs:1862` and `:1870` pass `WRITE_DEADLINE`.
- [ ] In `paneflow-serve`:
  - the `OwnerLock` path field and accessor are removed (`bootstrap.rs:31`, `:61-63`);
  - `hook_state.rs:28` items become `#[cfg(test)]`, and `EVENT_STOP_CANCELLED` is never gated;
  - the three `notifications.rs:76-86` methods become `#[cfg(test)]` together;
  - `may_be_signaled` is removed and `menu_attention_detection` becomes `#[cfg(test)]` (`state.rs:92-94`).
- [ ] Unless OQ-10 is answered "route to the worker": the worker front door (`server.rs:504`) is removed with the proxy, `PROXIED_PREFIXES`, `WORKER_ANSWERED`, `is_proxied`, `fleet_list`, `enrich_status`, `DispatchError::Core`, `ERR_INTERNAL`, and their capabilities, and `ARCHITECTURE.md:527-530` is updated.
- [ ] Given a CLI call with no desktop running, when it targets a surface, then it succeeds through the host path from US-006, not through the worker.

#### US-029: Gate or remove test-only items in `paneflow-config`, `paneflow-home`, `paneflow-textdiff`, and `paneflow-agent-config`
**Description:** As the maintainer, I want small-crate items that only tests call gated or removed so that each crate's public API is the one production uses.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-017, US-018

**Acceptance Criteria:**
- [ ] In `paneflow-config`:
  - the function at `loader.rs:110` becomes non-public `#[cfg(test)]` in place, before the test modules at `:143`;
  - `settings/tabs/terminal.rs:107-110` calls `terminal.normalized_cursor_color()`, and `normalize_hex_color` leaves its import;
  - the `schema/terminal.rs:8` constant is removed, and `schema.rs:143` uses the literal.
- [ ] `paneflow-home` `lib.rs:114-120` and its asserts are removed or rewritten with `host_dir_in`.
- [ ] In `paneflow-textdiff`:
  - `compare_words` (`lib.rs:81-87`) and `by_word::compare` become `#[cfg(test)]`;
  - `compare_chars` and its callees become `#[cfg(test)]`, with the imports at `by_char.rs:1-3` split;
  - the README's Public API section matches.
- [ ] In `paneflow-agent-config`:
  - `build_support.rs:519-542`, `alias_pattern`, and `platform_guard` are removed with their test lines;
  - `claude_hooks.rs:59-61` and `:186-202` are removed, and the tests use `managed_group_for_command(render_hook_command(..))`.
- [ ] Given the textdiff oracle tests (`oracle/words.rs` and the character oracle), when the suite runs, then they still run and pass; they are never deleted.

#### US-030: Gate or remove test-only items in `src-app`
**Description:** As the maintainer, I want application items that only tests reach gated or removed, and the Gemini session parser fixed, so that the desktop binary carries production code only.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022, US-023

**Acceptance Criteria:**
- [ ] Removed or gated:
  - the three `agent_sessions.rs:98` functions and their allowances are removed, and `:283` and `:317` use `store_result_with_mtime`;
  - the allowances at `document.rs:116`, `highlight.rs:103`, `:263`, `:268`, `load.rs:251`, `code/view.rs:663`, `:669`, `:730`, `:735`, `:1246`, and `terminal/element/mod.rs:47` become `#[cfg(test)]`;
  - the four legacy markdown capture names and their test are removed (`diff/syntax.rs:38`);
  - `markdown/security.rs` and `markdown/mod.rs:2-3` are removed;
  - `terminal/element/font.rs:123-130` and its two tests are removed;
  - the four unused theme fields and their 40 initializers are removed, `ui_role_rows(theme).len()` goes from 27 to 24, and `DESIGN.md:220`, `:222`, `:225`, and `:1037` match;
  - `update/migrations.rs:299` becomes a production predicate `coexistence_toast_due(marker_path)`, called by `bootstrap.rs:426` and by the test.
- [ ] Gemini sessions get a dedicated parser (`command_sessions.rs:32`): the last `[...]` token is validated by `is_valid_session_id`, headers are ignored, and `allow_numeric_ids` is removed. On Windows, the list command resolves the binary with `which::which`.
- [ ] Given Gemini list output with header lines, when it is parsed, then only valid trailing ids are returned (fixture from a real Gemini CLI run); given a line without a bracketed id, then it is skipped.
- [ ] Manual GUI pass on Linux: the theme settings role list shows 24 rows and the diff view syntax colors are unchanged.

#### US-031: Compile the in-process PTY runtime for tests only (option A)
**Description:** As the maintainer, I want the in-process PTY runtime that no shipped pane runs compiled only under `cfg(test)` so that the release binary drops it, while the stress and benchmark gates that still use it keep running.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010, US-023, US-030

**Acceptance Criteria:**
- [ ] The `#[allow(dead_code)]` roots on `TerminalState::new` and `new_with_profile` are removed. The chain is compiled only under `cfg(test)`, including items currently under `cfg(any(test, windows))`: `GhosttySession::start`, `run_runtime`, the guards, and the helpers. So are `new_pending(_with_profile)`, `new_display_only(_with_profile)`, and `resolve_spawn_params` (`pty_session.rs:785`); the ignored `_profile` parameter is removed (`view.rs:468`, `:625`).
- [ ] `portable-pty` moves to `[dev-dependencies]` (`src-app/Cargo.toml:58`), and the comments at `:53-57`, `ARCHITECTURE.md:161`, and `:540` describe the test-only runtime.
- [ ] `GhosttyStartError` is reduced to `Initialization`, with `ghostty_session.rs:1203-1220` remapped and the redaction test rewritten. `failure_phase=` is kept, and `docs/release/windows-libghostty.md:191-193` and `docs/release/libghostty-linux.md:109-111` match.
- [ ] The process-group Ctrl-C and SIGHUP test from `terminal/portable_pty_probe.rs` moves to `crates/paneflow-host/tests` using `paneflow_host::pty::open`, and the rest of the probe and `terminal/mod.rs:15-16` are removed.
- [ ] The `cfg(windows)` dead items of `terminal/ghostty_stress.rs` (`:31-34`, `:327-334`, `:336-349`, `:493-503`, `:935-1054`) are removed.
- [ ] Given `cargo test --release --locked -p paneflow-app ghostty_spawn_resize_close_stress_has_no_residual_growth -- --ignored`, when it runs on Linux, then it passes. Every test name that `scripts/qualify-libghostty-windows.ps1` and `libghostty-linux.yml:235-239` invoke still exists (grep recorded).
- [ ] `scripts/bench-terminal.sh` runs and reports within 5% of `bench/baseline.json`.
- [ ] Given option B (deletion) chosen through OQ-4, when this story is done, then no gate or benchmark is removed here. Option B is a follow-up after `prd-persistent-session-reliability` EP-004 is DONE.

#### US-032: Replace broad lint allowances with target-scoped attributes
**Description:** As the maintainer, I want each remaining allowance scoped to the target where the lint fires so that newly dead code on other targets is reported.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-022, US-024

**Acceptance Criteria:**
- [ ] `detached_panes/window.rs:77` becomes `#[cfg_attr(target_os = "macos", allow(clippy::needless_update))]`.
- [ ] The allowance at `document.rs:107` and the one at `load.rs:172` are removed.
- [ ] `ipc_events.rs:1-2` are removed.
- [ ] The allowance on the `Network` field at `update/error.rs:5` is removed.
- [ ] `update/install_method.rs:1` and the blank line after it are removed.
- [ ] `workspace/pid_resolve.rs:33` becomes `#[cfg(any(target_os = "linux", test))]`.
- [ ] Given a leg that reports `unfulfilled_lint_expectations` or `dead_code` after the change, when the fix is made, then it narrows the `cfg`, never restoring a blanket allowance.

---

### EP-005: Duplicate Consolidation

Replace copied helpers with a single owner so each future fix lands once.

**Definition of Done:** US-033 to US-038 are DONE, and US-039 is DONE per OQ-5. Each consolidated helper has exactly one definition, shown by a `git grep` count recorded in its PR.

#### US-033: Share host primitives with `paneflow-serve`
**Description:** As the maintainer, I want the owner lock, JSON record reader, connection guard, owner-only bind, agent bus, and event kinds defined once in `paneflow-host` so that the worker and host cannot diverge.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-027, US-028

**Acceptance Criteria:**
- [ ] `paneflow-host` gains `OwnerLock::acquire_at(path)` and `read_json_record<T>(path, cap)`, and `acquire(home)` and `read_instance_record(home)` remain as wrappers. The serve copies (`crates/paneflow-serve/src/bootstrap.rs:29`) are removed.
- [ ] `paneflow-host` gains `ConnectionGuard::try_acquire(counter, limit)` and `bind_owner_only(endpoint, label)`, declared before `mod tests`. The serve copies (`server.rs:318`) and the desktop `ActiveCountGuard` are removed, serve keeps its `MAX_CONNECTIONS`, and `widestring` leaves serve's dependencies.
- [ ] Serve's `pub bus` is typed `paneflow_host::agent::AgentBus`, and the serve copy (`server.rs:26`, `:54-94`) is removed.
- [ ] `AiHookMethod` gains `parse` and `ends_run`. `agent.rs:20-60` becomes `pub use AiHookMethod as AgentEventKind`, and the four `wire_str` become `as_str`.
- [ ] Given the connection limit reached, when one more client connects, then it receives the same rejection as at `e15277a0` (existing tests on all three legs).
- [ ] Given the Windows named-pipe endpoint, when it is bound, then the owner-only security descriptor is identical (Windows leg test).

#### US-034: Consolidate process-identity helpers in `paneflow-host`
**Description:** As the maintainer, I want process start-time, argv, and executable-stem helpers defined once so that PID-reuse guards behave the same in every component.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022, US-024

**Acceptance Criteria:**
- [ ] `event_handlers.rs:68-121` is removed. `pid_matches` and `ipc_handler` call `paneflow_host::process::process_start_time`, and the test `proc_stat_starttime_survives_hostile_comm_names` moves to `paneflow-host`.
- [ ] `paneflow-host` makes `process_argv`, `WindowsProcessEntry`, and `windows_process_entries_named` public, and extracts `parse_procargs2` before `mod tests`, moving the tests at `ports.rs:1117` and `:1157`. The desktop copies in `workspace/ports.rs:812` are removed.
- [ ] `pub(crate) fn executable_stem` lives in `agent_launcher.rs` before `mod tests` and is called by `from_launch_command`, `agent_from_command`, and `ports.rs:707`, `:979`, and `:992`. The three copies are removed.
- [ ] Given a process whose `comm` contains `") ("`, when its start time is parsed, then the result matches `/proc/<pid>/stat` field 22.
- [ ] Given a macOS `procargs2` buffer fixture, when `parse_procargs2` runs on any host, then it returns the expected argv.

#### US-035: Consolidate `src-app` runtime helpers
**Description:** As the maintainer, I want the shell-setting, surface-read, list-command, runtime-path, update-staging, and git-stdout helpers defined once so that each behavior has one implementation.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007, US-024

**Acceptance Criteria:**
- [ ] `normalized_shell_setting` becomes `pub(crate)` and replaces `ipc_handler.rs:450-452`, and one `option_env!` pair lives in `telemetry_events.rs` before `mod tests`.
- [ ] `cli/surface_read.rs` holds the four shared items plus `text_after_baseline`, with the `new_text_since_baseline` test moved.
- [ ] `main.rs:1868-1881` becomes an `if let Err` on `ensure_ai_hook_extracted`, keeping `startup_trace::mark("bridge_extracted")`.
- [ ] `run_list_command` becomes `pub(crate)` with `program`, `args`, `cwd`, `stdout_cap`, and an agent label, is used by `read_sessions_with_program` with an 8 MiB cap, and the opencode copies are removed.
- [ ] `runtime_paths.rs:12-13` are removed and `:82` builds the path inline.
- [ ] `update/swap.rs` holds `staging_dirs` and `recover_and_clean_staging`, shared by the dmg and targz paths.
- [ ] One `pub(crate) fn git_stdout` in `workspace/git.rs` replaces the copies, including `files_git.rs:153`.
- [ ] Given an interrupted update that left `.old` and `.new` staging directories, when Paneflow starts on Linux (targz) or macOS (dmg), then recovery restores the same state as at `e15277a0` (tests for both labels).
- [ ] `scripts/bench-startup.sh` reports within 5% of `bench/startup-baseline.json`.

#### US-036: Consolidate diff and editor rendering helpers
**Description:** As the maintainer, I want text-run shaping, background spawning, plural formatting, language icons, and UTF-16 offset math defined once so that the diff view and the code editor render identically.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-030

**Acceptance Criteria:**
- [ ] `spawn_blocking_then` in `diff_dock/code/mod.rs` serves the 5 call sites, and the `#[cfg(test)] use gpui::AppContext` lines it obsoletes are removed.
- [ ] `fill_text_runs`, `text_runs`, and `shape_plain` live in `diff/element.rs` before `mod tests` (`:1570`), are re-exported from `diff/mod.rs:16`, and replace the copies in `code/element.rs:100`, `:515`, and `:586`.
- [ ] One `pub(crate) fn plural(count, one, many)` under `src-app/src/app` replaces `code/view.rs:2104` and `quit_dialog.rs:87`.
- [ ] `language_icon_path` absorbs jsonc, scss, sass, less, text, mjs, and cjs, `render.rs:157` calls it, and `file_tab_icon` is removed.
- [ ] `widgets/utf16.rs` holds `byte_offset_from_utf16`, `byte_range_from_utf16`, and `utf16_offset_from_byte`, and both widgets delegate to it.
- [ ] `scripts/bench-editor.sh` reports within 5% of `bench/editor-baseline.json`.
- [ ] Given an emoji or CJK character before the caret, when IME composition inserts text in a text input and in the code editor, then the insertion lands at the caret (test on the UTF-16 helpers plus a manual GUI pass on Linux).

#### US-037: Consolidate settings and modal UI primitives
**Description:** As a user, I want every confirmation modal and agent logo to behave and render the same so that dialogs dismiss and confirm consistently.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022

**Acceptance Criteria:**
- [ ] `settings/components.rs` provides a card shell, a parameterized confirmation list (Enter action, buttons, warning), and `modal_backdrop(id, child, on_dismiss)`. The 5 modal sites, including `worktree_remove.rs:321`, use them, and orphaned imports and constants are removed.
- [ ] `render_logo` (`settings/components.rs:292`) takes `(path, multicolor, size, tint)` and is declared before `mod tests`. The seven copies route through it, keeping each site's size and the `ui.muted` fallback of `pane.rs`.
- [ ] `agents/notifications.rs:260` uses one ungated `match urgency` inside the existing `cfg` at lines 243 and 259.
- [ ] Given each of the 5 modals, when Enter, Escape, and a backdrop click are used, then each behaves as at `e15277a0` (manual GUI pass on Linux with screenshots).
- [ ] Given a waiting agent on Linux and on Windows, when a notification fires, then its urgency matches `e15277a0`.

#### US-038: Deduplicate a config alias and the MCP-install presence check
**Description:** As the maintainer, I want the notification-screen alias mapped at parse time and the presence check shared so that each rule lives in one place.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-017, US-018

**Acceptance Criteria:**
- [ ] The unused `AllScreens` variant (`schema/agent_panel.rs:46`) is removed. `"AllScreens"` deserializes to `PrimaryScreen` (`:59`), the schema enum keeps `"AllScreens"`, `notifications.rs:199` is reduced, and the assert at `:359-362` is removed.
- [ ] `pub(crate) fn config_presence(cli, config)` in `agents/support.rs`, declared before `mod tests` (`:311`), serves both implementations, including `agents/gemini.rs:62`.
- [ ] Given a config with `"notify_when_agent_waiting": "AllScreens"`, when it loads, then it behaves as `PrimaryScreen`, without error (test).
- [ ] The PR description lists the paneflow-web alias documentation.

#### US-039: Make `paneflow hooks` delegate to the integrations installer (default of OQ-5)
**Description:** As a Claude Code user, I want `paneflow hooks` and `paneflow integrations` to manage one set of hooks so that running either command never duplicates or drops entries in `~/.claude/settings.json`.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017, US-029

**Acceptance Criteria:**
- [ ] `paneflow hooks` subcommands call `install_integration`, `remove_integration`, and `list_integrations`, and the duplicate install code in `crates/paneflow-mcp-install/src/hooks.rs` is removed.
- [ ] One ownership marker identifies Paneflow-managed hooks, and the obsolete Codex message (`hooks.rs:242`) is corrected.
- [ ] The label "pre-0.17 per-launch hook entry" (`integrations.rs:377` and its tests) describes what it matches, and `owned_events` and the retired-event lists are kept.
- [ ] Given a `settings.json` written by the old `paneflow hooks` (5 events) and one written by `paneflow integrations` (9 events), when either command runs, then both converge to the same managed set, and hooks the user added by hand are kept (fixture tests for both starting states).
- [ ] Given OQ-5 answered "retire `paneflow hooks`", then this story is rewritten before implementation to remove the command, with paneflow-web documentation first.

---

### EP-006: God-File Splits

Split the largest mixed-responsibility files into submodules by pure moves. Each split waits until the cleanup epics have removed what would otherwise be moved, and until any branch or qualification that touches the file has landed.

**Definition of Done:**
- US-040 to US-047 are DONE.
- Each split landed as a pure-move commit listed in `.git-blame-ignore-revs`.
- No production file produced by a split exceeds 2,000 lines, except `host.rs` at 2,700 or fewer.

Every story in this epic also satisfies these shared criteria, restated here once:
- The split is one pure-move commit: outside moved blocks, only `mod`, `use`, `pub use`, and visibility lines change (reviewed with `git diff --color-moved=dimmed-zebra`). Any fix-up is a separate commit.
- Every path imported from outside the module still resolves through `pub use`, so files outside the split change only in `use` lines.
- No produced file declares an item after its `mod tests`.
- A follow-up commit lists the pure-move hash in `.git-blame-ignore-revs`, creating the file if absent.
- Given a newer change to the same file on main, the split is redone from current main rather than hand-merged.

#### US-040: Split `crates/paneflow-host/src/host.rs`
**Description:** As a contributor, I want the host's tests, spawn environment, staging, and types in their own files so that the 5,141-line file becomes readable in one pass.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-027, US-033

**Acceptance Criteria:**
- [ ] `host.rs` stays the parent and gains `host/tests.rs`, `host/spawn_env.rs`, `host/staging.rs`, and `host/types.rs`.
- [ ] The public API exported at `crates/paneflow-host/src/lib.rs:45-49` is unchanged.
- [ ] `host.rs` is 2,700 lines or fewer after the split.
- [ ] Given the `cfg(windows)` and `cfg(unix)` helpers in the moved blocks, when they move, then their gates are unchanged.

#### US-041: Split `persistent_baseline.rs` and land the qualification-coupled cleanups
**Description:** As the maintainer, I want the persistent-session baseline harness split and its dead fallbacks removed once the Windows qualification is certified so that qualification evidence is never invalidated mid-run.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-019

**External blocker:** `tasks/prd-persistent-session-reliability.md` EP-004 is DONE.

**Acceptance Criteria:**
- [ ] `crates/paneflow-host/tests/persistent_baseline.rs` gains `#[path]` modules:
  - `provenance.rs` (`:74-266`);
  - `metrics.rs` (`:268-713` plus `process_sample`);
  - `processes.rs` (`:734-1219`, `:1234-1302`);
  - `report.rs` (`:714-733`, `:1303-1443`);
  - `endurance.rs` (`:1825-2023`, `:2440-2457`).

  The three tests stay at the root, re-exports serve `workloads.rs`, and the root is 900 lines or fewer.
- [ ] In separate commits after the move:
  - the `cfg` fallbacks that no target compiles are removed (`process.rs:88-91`, `:139-142`, `:224-227`, `runtime_observer.rs:853-863`, the test arms at `:1063-1064` and `:1082-1083`, and the five baseline fallbacks);
  - `server.rs:2733-2744` is removed, with `docs/release/persistent-qualification.md:372`, `:397`, `:423`, and `:439` pointing at the 61 s test and its 300 s and 1,800 s variants in POSIX and PowerShell form;
  - the `persistent_baseline.rs:273` allowance becomes `cfg_attr(..., expect(dead_code))` on `Exact` and `Prefix15`;
  - `persistent_baseline.rs:790` no longer sends `drain_ms` (audit row R134, paired with US-020).
- [ ] Given a qualification document that cites `persistent_baseline.rs` line numbers, when the split lands, then the citation is updated or pins `e15277a0`.

#### US-042: Split `src-app/src/terminal/ghostty_session.rs`
**Description:** As a contributor, I want the 7,742-line session file split by concern so that events, mailbox, commands, publication, and each runtime can be read and changed alone.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004, US-023, US-031

**Acceptance Criteria:**
- [ ] `terminal/ghostty_session/` holds:
  - `mod.rs` (API, `SessionInner`, `SharedState`);
  - `events.rs`, `mailbox.rs`, `commands.rs`, and `publish.rs`;
  - `convert.rs`, `display_runtime.rs`, and `attached_runtime.rs`;
  - tests beside their code.
- [ ] `pub(super) use` re-exports serve `pty_session.rs`, `view.rs`, and `perf_bench.rs`.
- [ ] `mod.rs` is 1,500 lines or fewer and no produced production file exceeds 2,000 lines.
- [ ] Given `scripts/bench-terminal.sh`, when it runs after the split, then it reports within 5% of `bench/baseline.json`.

#### US-043: Split `src-app/src/terminal/pty_session.rs` and `src-app/src/terminal/view.rs`
**Description:** As a contributor, I want the PTY backend facade, diagnostics, and spawn environment, and the host-attach lifecycle of the terminal view, in their own files so that each file holds one responsibility.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-023, US-031

**External blocker:** `feat/agents-browser` merged or abandoned. It adds 42 lines to `view.rs`.

**Acceptance Criteria:**
- [ ] `pty_session.rs` yields `session_backend.rs` (`:51-333`), `diagnostics.rs` (`:342-452`), and `spawn_env.rs` (`SpawnParams`, `resolve_spawn_params_with_profile`, `assemble_pty_env`, and the PATH, AI-hook, and WSLENV helpers with their tests).
- [ ] `view.rs` yields `host_attach.rs` (`impl TerminalView`) with:
  - `begin_hosted_attach`, `resume_hosted_session`, `stop_incompatible_host_and_restart`, and `render_host_link_bar`;
  - `AttachOutcome`, `spawn_error_message`, and the helpers at `:28-94` except `RENDER_WAKEUP_IMMEDIATELY`.

  `probe_enabled` and `HostedLaunch` stay in `view.rs`.
- [ ] `pty_session.rs` is 1,900 lines or fewer and `view.rs` 2,400 or fewer.
- [ ] Given that the `rendering` path filter names `terminal/view.rs`, when the branch is pushed, then `macos_render_smoke` and `windows_render_smoke` run and are green.

#### US-044: Split `src-app/src/app/ipc_handler.rs`
**Description:** As a contributor, I want the IPC handler split into JSON-RPC plumbing, method groups, gates, agent frames, transcript, and context-file modules so that adding a method no longer means editing a 5,167-line file.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-022, US-035

**External blocker:** `feat/agents-browser` merged or abandoned. It adds about 216 lines to this file, including `qualification.*` arms.

**Acceptance Criteria:**
- [ ] `app/ipc_handler/` holds `mod.rs`, `jsonrpc.rs`, `workspace_methods.rs`, `surface_methods.rs`, `gates.rs`, `agent_frames.rs`, `transcript.rs`, and `context_file.rs`, with tests moved beside their code.
- [ ] Code moves to its owners: config reload to `settings.rs`, the update check to `self_update_flow.rs`, and `fire_*_notification` to `notifications.rs`.
- [ ] The 9 external imports of `app::ipc_handler` resolve unchanged, and the `cfg(unix)` block at `:335-360` keeps its gate.
- [ ] No produced production file exceeds 2,000 lines.
- [ ] Given a moved item that an external importer can no longer resolve, when the split is compiled, then a `pub use` is added in `mod.rs` instead of editing the importer beyond its `use` line.

#### US-045: Split `src-app/src/main.rs`, `src-app/src/app/bootstrap.rs`, and `src-app/src/app/event_handlers.rs`
**Description:** As a contributor, I want the entry point, startup orchestration, and event dispatch reduced to orchestration with their clusters extracted so that startup and event flow can be followed file by file.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-022, US-024, US-034

**External blocker:** `feat/agents-browser` merged or abandoned. It edits all three files.

**Acceptance Criteria:**
- [ ] `main.rs`:
  - keeps: module declarations, re-exports, the 4 state structs, `SWAP_MODE`, and a minimal `main()`;
  - extracted: `launch.rs`, `app/window.rs`, and `app/render.rs` (with the sidebar helpers);
  - moved to `window_chrome`: the material predicates and `panel_corner_mask`;
  - size: 700 lines or fewer.
- [ ] `bootstrap.rs`:
  - `new()` keeps orchestration only;
  - `spawn_automation_tick` holds the watcher;
  - one git helper remains;
  - the macOS menu moves to `app/macos_menu.rs` under `cfg(target_os = "macos")`;
  - `system_package_update_command` moves to `self_update_flow.rs`, with the re-export at `main.rs:85` adjusted;
  - size: 650 lines or fewer.
- [ ] `event_handlers/` holds `mod.rs` (pane, title-bar, and terminal dispatch), `pane_scan.rs`, `session_reaper.rs`, and `cwd_tracking.rs`, with tests beside their code. `mod.rs` is 1,100 lines or fewer.
- [ ] The `cfg(unix)`, `cfg(windows)`, and `cfg(target_os = "macos")` blocks move with unchanged gates.
- [ ] `scripts/bench-startup.sh` reports within 5% of `bench/startup-baseline.json`.
- [ ] Given a moved `cfg(windows)` item that the Windows leg reports after a `mod tests`, when the fix is made, then the item moves above the module, never under an `allow`.

#### US-046: Split `src-app/src/app/sidebar/mod.rs` and `src-app/src/settings/tabs/workspaces.rs`
**Description:** As a contributor, I want sidebar rows, styles, drop handling, and metadata, and the workspace template model, launcher, and editor, in separate files so that each UI concern is local.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012, US-022, US-037

**Acceptance Criteria:**
- [ ] `sidebar/` holds:
  - `mod.rs` (order, filter, canary), `style.rs`, `workspace_row.rs`, `tab_row.rs`, `session_rows.rs`, `drop.rs`, and `meta.rs`;
  - the comet trail in `lane.rs`;
  - `tab_row_branch` and `tab_pull_request` in `pull_request.rs`.
- [ ] Replacing `SidebarTooltip` with `text_tooltip` happens in a separate commit after the move.
- [ ] Workspace templates:
  - the model (with `layout_tests`) moves to `app/workspace_ops/templates.rs`;
  - the launcher moves to `app/workspace_ops` with `paths_equal`, except `run_workspace_template_in_open_project`;
  - the editor lives in `settings/tabs/workspaces/{mod.rs,editor.rs}`;
  - the private widgets move to `settings/components.rs`.
- [ ] `sidebar/mod.rs` is 1,100 lines or fewer and `workspaces` `mod.rs` 1,000 or fewer.
- [ ] Manual GUI pass on Linux: sidebar rows, drag-and-drop reordering, tooltips, and the workspace template editor behave as before the split.
- [ ] Given the `text_tooltip` replacement visibly changes tooltip placement or delay, when the GUI pass runs, then that commit is reverted and `SidebarTooltip` stays.

#### US-047: Split `src-app/src/app/diff_dock/code/view.rs`
**Description:** As a contributor, I want the code editor controller split by concern so that keymap, pointer, motion, editing, disk sync, change markers, IME, and rendering each live in one file.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-022, US-030, US-036

**Acceptance Criteria:**
- [ ] `code/view/` holds `mod.rs` (struct, helper types, loading), `keymap.rs`, `pointer.rs`, `motion.rs`, `editing.rs`, `disk_sync.rs`, `change_markers.rs`, `input_handler.rs`, and `render.rs`, with tests split by concern.
- [ ] About 65 methods become `pub(super)`, `on_scrollbar_*` becomes `pub(crate)`, and `register_keybindings` and `CODE_KEY_CONTEXT` are re-exported.
- [ ] The macOS keymap block (`view.rs:164-175`) moves with its gate unchanged, and no produced production file exceeds 1,500 lines.
- [ ] `scripts/bench-editor.sh` reports within 5% of `bench/editor-baseline.json`.
- [ ] Manual GUI pass on Linux: editing, selection, scrolling, IME composition, save, and change markers behave as before the split.
- [ ] Given a caller outside `code/` that `pub(super)` does not reach, when the split is compiled, then that method becomes `pub(crate)` and the PR names the caller; no method becomes `pub`.

## Functional Requirements

- FR-01: `clamp_text` must never panic, whatever the input, and must return at most `MAX_AGENT_TEXT_BYTES` bytes, cut at a character boundary.
- FR-02: `surface.rename` must accept `name`, as documented, and keep accepting `new_name`.
- FR-03: When a user sets `external_editor`, the system must open file locations in that editor, and fall back to `$VISUAL`, `$EDITOR`, then the fixed list when it is unset or unresolvable.
- FR-04: The CLI, MCP, and desktop must choose the controller and host endpoints through one shared rule.
- FR-05: When a `paneflow.json` or `session.json` contains a key removed by this PRD, the system must load it without error and ignore that key.
- FR-06: The system must not change the name of any public IPC method or CLI command. The `ai.*` socket methods stay (OQ-6).
- FR-07: A deleted asset, file, or item must have zero references on main and on `feat/agents-browser` when it is deleted, apart from `CHANGELOG.md` history.
- FR-08: Every `Cargo.toml` change must be committed with its matching `Cargo.lock`.
- FR-09: A god-file split must not change behavior: its pure-move commit changes no function body.
- FR-10: The system must NOT edit `docs/user/` or the generated bindings under `native/libghostty/`.

## Non-Functional Requirements

- **Performance:** The terminal, editor, and startup benchmarks stay within 5% of `bench/baseline.json`, `bench/editor-baseline.json`, and `bench/startup-baseline.json` for every story that names them.
- **Binary size:** The hard caps are measured by the `release_build_linux` job after every story that touches a helper crate:

  | Binary | Hard cap (bytes) |
  |--------|------------------|
  | `paneflow-shim` | 524,288 |
  | `paneflow-ai-hook` | 384,000 |
  | `paneflow-mcp` | 524,288 |
  | Combined | 1,835,008 |

  The stripped Linux release binary of `paneflow` is no larger than at `e15277a0`.
- **Security:**
  - Cargo.lock goes from 846 packages to 837 or fewer.
  - `cargo deny check advisories licenses sources` reports 0 errors.
  - The count of `unsafe` blocks in the workspace does not increase (`git grep -c "unsafe {"` compared to `e15277a0`).
- **Reliability:**
  - No story adds a failing test on any CI leg compared to `e15277a0`, whose only red job is Windows `cargo test` (`server.rs:2154`, `hyperlink.rs:894`).
  - 0 reverts of a story commit within 30 days of merge.
- **Maintainability:**
  - `#[allow(dead_code)]` goes from 59 to 20 or fewer.
  - No production file in the EP-006 split set exceeds 2,000 lines, except `host.rs` at 2,700 or fewer.
  - 100% of split commits are listed in `.git-blame-ignore-revs`.
- **Accessibility:** UI stories keep keyboard behavior identical: Enter confirms, Escape dismisses, and Tab order is unchanged in the 5 consolidated modals. Each is verified manually in the GUI pass.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Multibyte text at the byte limit | Agent text over 4,096 bytes with a multibyte character straddling byte 4,096 | The character is dropped whole; no panic | None |
| 2 | Removed config key in a user file | `paneflow.json` still has `tool_permissions` or profile fields | Loads; the key is ignored; effective config unchanged | None |
| 3 | Retired session value | `session.json` holds `"auto"` | Resolves to `User` | None |
| 4 | Feature unification loss | A removed dependency was the only one enabling a `windows-sys` feature that production code uses | The Windows leg fails to compile; the story stays IN_PROGRESS naming the item | CI error |
| 5 | cfg-gated item after tests | A new `cfg(windows)` helper is appended after `mod tests` | `items_after_test_module` fails the Windows leg; the item moves above the module | CI error |
| 6 | Unfulfilled expectation | A `cfg_attr(..., expect(dead_code))` is fulfilled on Linux but not macOS | `unfulfilled_lint_expectations` fails the macOS leg; the `cfg` is narrowed | CI error |
| 7 | Silent missing asset | An SVG is deleted but still referenced by a string literal | rust-embed builds fine and the icon disappears; the pre-delete `git grep` on main and the branch prevents it | None |
| 8 | Wrong instance targeted | `PANEFLOW_HOME=B` with an inherited socket path from instance A | The CLI connects to B unless `PANEFLOW_ALLOW_SOCKET_OVERRIDE=1` | None |
| 9 | Session vanishes mid-call | A session exits between `surface.list` and `surface.read` in headless mode | The existing "surface not found" error and exit code | "surface not found" |
| 10 | Mixed-version worker | The desktop updates while a worker from the previous release runs | The existing protocol negotiation applies; no version bump in this PRD | Existing restart banner |
| 11 | Hook settings written by both installers | `~/.claude/settings.json` has entries from `paneflow hooks` and `paneflow integrations` | Both commands converge to one managed set; user hooks kept | None |
| 12 | Rebase conflict on a god file | `feat/agents-browser` or a new main commit edits a file being split | The split is redone from current main, not hand-merged | None |
| 13 | Asset still on the branch | `icons/moon.svg` or `mcps/` is used by `feat/agents-browser` | Kept until US-016 resolves the branch outcome | None |
| 14 | Windows update path after MSI cleanup | A user updates from the previous release on Windows | The relay installs and relaunches; proven by the update end-to-end job or the dual-boot check | Existing update UI |
| 15 | Preexisting red leg | Windows `cargo test` fails at `server.rs:2154` or `hyperlink.rs:894` | Not attributed to the story; any other new failure blocks it | CI error |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | A deletion breaks code that compiles only on Windows or macOS | Med | High | `windows_check` and `macos_check` gates; Windows items reviewed by reading `cfg` and by the dual-boot check where the story says so |
| 2 | Audit evidence goes stale as main moves | Med | Med | Each deletion re-runs its reference search on current main and on the branch before deleting |
| 3 | God-file splits conflict with `feat/agents-browser` | High | Med | US-043, US-044, and US-045 are blocked until the branch outcome; other splits avoid its files |
| 4 | Dependency removal changes feature unification | Low | High | Resolver 2 isolates targets (Cargo docs); each target leg builds; failures name the consumer |
| 5 | Behavior fixes in EP-001 regress an edge the audit did not see | Med | Med | Reproduce-first criteria (US-006, US-007), regression tests, macOS hardware check (US-004) |
| 6 | The qualification-coupled cleanup invalidates Windows evidence | Low | High | US-041 blocked until persistent-session EP-004 is DONE; option B of the runtime deferred |
| 7 | Scope fatigue across 47 stories | Med | Med | Three phases of at most 16 stories; EP-005 and EP-006 are P2 and can pause without leaving dead code behind |
| 8 | Removing a config value changes a user's effective config | Low | Med | Fixture tests load files with removed keys (US-018); the `"auto"` change is stated in the PR |
| 9 | The Windows MSI update relay regresses | Low | High | US-024 stays IN_REVIEW until the update end-to-end job or the dual-boot proves it |

## Non-Goals

What this version explicitly does NOT include:

- **Restructuring `.github/workflows/release.yml` into callable workflows (audit row R1).** The pipeline is only validated by 25-minute runs, and the matrix stays atomic by design. Revisit if release changes become frequent.
- **Splitting `src-app/src/workspace/worktree.rs` (R14).** The audit marked it optional, and its clusters are coherent.
- **Deduplicating the prebuilt libghostty bindings (R56).** Git already stores one blob, and `native/libghostty/` is off-limits.
- **Deleting the in-process PTY runtime and retargeting its qualification gates (option B: R111, R224, R225, R273, R287, R288).** Deferred to a follow-up after persistent-session EP-004 is DONE (OQ-4).
- **Wiring `commands[]` shell entries into the palette, or deprecating them (R96).** That is a feature decision (OQ-7).
- **Removing legacy migration paths for old versions (R100, R101, R103, R150, R160, R161, R164, R189, R190, R202 except the `created_at` fallback).** They wait for a support floor (OQ-8).
- **Exposing `agent.activity_log` through a new CLI command (R242).** A new feature, not cleanup (OQ-9).
- **Adding cargo-shear, cargo-machete, or `unused_crate_dependencies` to CI.** A new tool; revisit after this PRD.
- **Every rejected row (R31, R203, R204, R205, R307, R308, R309) and every row the audit resolved as "keep".**

## Files NOT to Modify

- `src-app/Cargo.toml:45`, `:46`, `:278`, `:416`: GPUI rev pins. `gpui_platform` keeps `features = ["font-kit"]` or macOS renders empty glyphs.
- `rust-toolchain.toml`: toolchain pin 1.98.0.
- `clippy.toml` (root): the single lint configuration after US-011.
- `native/libghostty/`: generated bindings and archives. `manifest.toml` changes only in US-016, with a pin bump.
- `src-app/build.rs`: size-cap constants and libghostty gating. US-011 removes only lines 10-11.
- `deny.toml`: policy entries (R307, R308 rejected). Only the exceptions block in US-011 and stale entries a removal leaves behind.
- `docs/user/`: mirror of paneflow-web.
- `packaging/linux/**/metainfo.xml`: the AppStream release entry is a CI gate (R25 excludes `metainfo.xml:239`).
- `.gitattributes` and `scripts/validate-task-artifacts.ps1`: rejected rows R203 and R31.
- `tasks/prd-*.md` and `tasks/prd-*-status.json` of other PRDs: owned by their own workflows.
- `CHANGELOG.md` historical entries.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** Should `clamp_text` and the other shared primitives (US-001, US-033) live in `paneflow-host` or move to a leaf crate that both host and serve already depend on? Recommended: `paneflow-host`, which serve already depends on. Engineering to confirm this adds no dependency edge to a helper binary.
- **Data Model:** For removed config and session fields, is serde's default unknown-field tolerance enough, or should the loader warn on retired keys? Trade-off: a warning helps users clean their files but adds a code path. Recommended: tolerate silently, as today.
- **API Design:** Should `surface.rename` document `new_name` as deprecated or keep both keys indefinitely? Recommended: accept both and document `name` only.
- **Dependencies:** Removals only; no new dependency. Is `which` already available to the Gemini runner on Windows (US-030)? Engineering to confirm before using it.
- **Migration:** Do any persisted values depend on `SessionAgent::index()` (US-022)? If they do, keep indices stable instead of renumbering. Rollback is the revert of the story commit.
- **Tooling:** Should the pure-move check be mechanized (for example a script that compares removed and added blocks), or is `git diff --color-moved` review enough for eight splits? Recommended: review, since eight commits do not justify a new script.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Audit defects closed with a regression test or hardware check | 0 of 8 | 8 of 8 | Month-1 | Status JSON for US-001 to US-008 |
| Packages in Cargo.lock | 846 | 837 or fewer | Month-1 | `grep -c '^name = ' Cargo.lock` |
| `#[allow(dead_code)]` attributes | 59 | 20 or fewer | Month-6 | `git grep -c '#\[allow(dead_code)\]' -- '*.rs'` summed |
| Tracked files | 1,251 | 1,210 or fewer | Month-1 | `git ls-files \| wc -l` |
| Net lines removed, moves excluded | 0 | 7,500 or more | Month-6 | `git diff --shortstat e15277a0..HEAD` minus pure-move commits |
| Largest production Rust file | 7,742 lines (`ghostty_session.rs`) | 2,000 lines or fewer (`host.rs` 2,700 or fewer) | Month-6 | `wc -l` over the EP-006 set |
| New failing tests per CI leg | 2 known on Windows `cargo test` | 0 new | Every story | `run_tests.yml` job results versus `e15277a0` |

## Open Questions

- **OQ-1 (Arthur):** Delete `packaging/winget/`?
  - Default: yes. There is no winget workflow, and Windows ships as a download-only .msi.
  - By: before US-012.
  - Depends: US-012.
- **OQ-2 (Arthur):** Prune `bench/results` by one rule?
  - Default: no, and US-015 is CANCELLED.
  - By: before Phase A closes.
  - Depends: US-015.
- **OQ-3 (Arthur):** Delete libghostty wrapper modules that only their own tests use, or keep them as `cfg(test)` modules?
  - Default: delete. Git history keeps them at `e15277a0`.
  - By: before US-025.
  - Depends: US-025, US-026.
- **OQ-4 (Arthur):** After persistent-session EP-004 is DONE, delete the in-process runtime (option B) by retargeting the Windows and Linux stress gates to `paneflow-host` sessions, or accept losing them?
  - Default: option A only in this PRD; option B in a follow-up PRD.
  - By: EP-004 certification.
  - Depends: a follow-up to US-031.
- **OQ-5 (Arthur):** For `paneflow hooks` versus `paneflow integrations`: delegate (B), or retire `paneflow hooks` (A)?
  - Default: B.
  - By: before US-039.
  - Depends: US-039.
- **OQ-6 (Arthur):** Keep, deprecate, or retire the `ai.*` methods on the desktop socket?
  - Default: keep (public API, rejected row R205). Only the reference documentation is fixed (US-014).
  - By: none; no story depends on it.
- **OQ-7 (Arthur):** Wire `commands[]` shell entries, `description`, `keywords`, and workspace `color` into the palette, or deprecate them?
  - Default: no change in this PRD.
  - By: next feature planning.
  - Depends: none here.
- **OQ-8 (Arthur):** What is the oldest version that must upgrade cleanly?
  - Default: keep every legacy path.
  - By: next release planning.
  - Depends: a follow-up removing R100, R101, R103, R150, R160, R161, R164, R189, R190, and R202.
- **OQ-9 (Arthur):** Expose the worker activity log (`paneflow sessions log`), as the agent-runtime PRD's US-014 intended, or delete it?
  - Default: keep as is.
  - By: none.
  - Depends: `Controller::activity_log` kept in US-020.
- **OQ-10 (Arthur):** Route the headless CLI and MCP through the worker front door instead of the host?
  - Default: no. The front door is deleted in US-028 after US-006.
  - By: before US-028.
  - Depends: US-028.
- **OQ-11 (Arthur, needs a real Kiro session):** Does Kiro print its session list on stderr (unconfirmed row R167)?
  - Default: no change until a capture exists.
  - By: none.
- **OQ-12 (Arthur):** Will `feat/agents-browser` merge, and when?
  - Default: none; the affected stories stay blocked.
  - By: before Phase C.
  - Depends: US-016, US-043, US-044, US-045.
- **OQ-13 (Arthur):** Should the qualification logs (`desktop.log`, `host.log`) that the `*.log` ignore rule excludes be tracked with their reports?
  - Default: no change.
  - By: next qualification.
  - Depends: none here.
[/PRD]
