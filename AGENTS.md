# Paneflow agent guidance

Paneflow is a native terminal multiplexer for coding agents, built with Rust
and Zed's GPUI framework. Linux, macOS, and Windows are all
shipping targets. This file is the canonical instruction source for every agent;
`CLAUDE.md` imports it.

## Gates you must not skip

Run from the repository root. The toolchain is pinned in `rust-toolchain.toml`
(1.98.0), so do not add `+stable` or `+nightly`.

```bash
cargo fmt --check                                   # mandatory before commit and push
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked --profile ci  # the CI runner; cargo test --workspace --locked runs the same suite
cargo deny check advisories licenses sources        # only when dependencies change
bash scripts/check-workflows.sh                     # when .github/** changes: every action pinned to a commit SHA (checked through gh when it is authenticated), no shell xtrace, cache saved from main only, a timeout on every job
uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github   # when .github/** changes: the Workflow audit job; set GH_TOKEN for the online audits
bash scripts/test-ci-scripts.sh                     # when a CI script changes: runs the scripts against fixtures
bash scripts/check-test-wallclock.sh                # when a test changes: no new wall-clock budget
```

**`cargo fmt --check` before every `git commit` and every `git push` that
touches Rust code.** If it reports a diff, run `cargo fmt`, re-stage, then
commit. The release pipeline does not re-run fmt, clippy or the tests: its
`ci-verdict` job requires every required `run_tests` job, `Style (fmt + clippy)`
included, to be `success` on the exact commit being released, and "Publish
GitHub Release" needs it. One mis-formatted line on that commit turns Style red
and blocks the release. A dirty tag commit is worse. Before the release is
published, delete the tag and re-create it at the fix commit; the publish job
refuses to run once the tag no longer points at the commit it built. Once the
release is published, never delete or move its tag: a re-run refuses a
published release, immutable releases lock the tag and its assets, and the fix
ships as a patch version. For a tag-push release, run
`cargo fmt --check` one final time on the exact commit you are about to tag,
and wait for `run_tests` to be green on it.

rustfmt output drifts between Rust point releases, so code that was clean last
week can need re-formatting after a toolchain bump. Always run the real
`cargo fmt`, not an editor formatter, because `cargo fmt` is the CI gate.
The `Windows x86_64 libghostty check` job runs `cargo fmt --all --check`,
which also formats path dependencies such as a vendored crate that is not a
workspace member: run it too when `vendor/**` changes, and give a vendored
copy its upstream `rustfmt.toml` rather than reformatting upstream text.

CI keeps every cargo invocation `--locked`. Never commit a change that requires
a lockfile update you did not also commit.

## `#[cfg(windows)]` code is only linted by the Windows leg

Local `cargo clippy --workspace --all-targets` runs on the host target, so every
item behind `#[cfg(windows)]` is compiled out and never linted. The same holds
for the `Style (fmt + clippy)` and `Tests (Linux x86_64)` jobs. Windows-gated
code is first seen by the `Windows x86_64 libghostty check` job in
`.github/workflows/run_tests.yml`, which clippies
`--target x86_64-pc-windows-msvc` with `-D warnings`.

Every clippy gate in `run_tests.yml` passes `--all-targets`. Do not drop it:
without it Cargo strips `#[cfg(test)]` before the HIR, so no `mod tests` is
ever linted and a whole class of lint never fires anywhere in CI. That was the
case until 2026-08-28, which is why this section used to name a job that could
not in fact catch the trap below.

The concrete trap: **when you add an item to a file that already has a
`mod tests`, declare it before that module, whatever its `cfg`.**
`clippy::items_after_test_module` fires on any non-test item that follows a test
module, and a Linux-invisible helper still trips it on the Windows leg. This
burned two Windows runs on 2026-08-26 (`e61cb61`, `b5e1139`) over two
`#[cfg(windows)]` helpers appended past `mod tests` in
`src-app/src/agent_launcher.rs`; the fix, `8022f08`, was a pure code move.

You cannot reproduce this on Linux, not even with a cross
`cargo clippy --target x86_64-pc-windows-msvc`: the target build of
`paneflow-app` needs `windows.h` and `llvm-rc` through GPUI. Read the file's
item order instead, and treat any file containing a `mod tests` as having a hard
boundary after which nothing else may be declared. Where an item genuinely must
follow the test module, add an `#[allow(clippy::items_after_test_module,
reason = "...")]` on the module.

## Unit tests assert outcomes, perf gates own latency

A unit test asserts a result under a generous timeout: it waits for the outcome
with a deadline measured in seconds and fails only when the outcome never
comes. A latency budget (a p95, "within 200 ms") lives in the perf gates,
`scripts/perf-gates.sh` with its budgets in
`crates/paneflow-host/tests/persistent_baseline/gates.rs`, which run on one
calibrated Linux x86_64 runner class. A unit test may write its measurement for
them, as `host_status_bounds_the_memory_reads_of_sixty_four_silent_sessions`
does through `PANEFLOW_GATE_HOST_STATUS_OUT`. Wall-clock budgets in unit tests
caused most of the flaky CI failures from 2026-09-28 to 2026-11-02, on 3-vCPU
macOS and 4-vCPU Windows runners.

`scripts/check-test-wallclock.sh`, run by `Style (fmt + clippy)`, fails on any
new assertion that compares `elapsed()`, or a percentile such as `p95(...)`,
to a `Duration` in test code. The existing ones are counted per test function
in `scripts/wallclock-allowlist.txt`, and an entry may only go down.

CI runs the tests with `cargo nextest run --profile ci`: one process per test,
a 300 s timeout per test, and the JUnit report uploaded as `test-results-<leg>`
on failure. A failing test is rerun once only to tell a flake from a
regression: if it then passes it is reported FLAKY and the job still fails.
Only the tests quarantined in `.config/nextest.toml`, each with its tracking
issue, may pass on a retry, and the quarantine holds at most four.

## Cross-platform compatibility is mandatory

Every change must work on Linux (Fedora, Ubuntu/Debian, Arch, openSUSE, on both
Wayland and X11), macOS (Intel and Apple Silicon), and Windows 10/11 (x64, and
ARM64 where applicable). Concretely:

- Never hardcode POSIX-only paths, shell commands, environment variables, or
  separators. Use `std::path::PathBuf`, `std::env`, and the `dirs` crate.
- Guard platform-specific code with `#[cfg(target_os = "...")]` and always
  provide a working path for the other two platforms, at minimum a graceful
  fallback or a documented stub.
- Prefer cross-platform crates (`portable-pty`, `notify`, `dirs`, `which`) over
  POSIX-only APIs. If a POSIX-only crate is unavoidable, isolate it behind a
  trait with per-OS implementations.
- PTY, IPC, packaging, auto-update, keybindings, fonts, and file watching each
  need a Linux, macOS, and Windows path. Never Linux-only.
- If you cannot verify a platform, say so explicitly instead of assuming.

`paneflow-app`'s `cfg(windows)` branches are not compilable on a Linux host, so
locally they can only be reviewed by inspection. Windows behavior is verified on
real hardware after a push, not in a Linux development environment. Say which of
the two you did.

## Boundaries the code does not reveal

- **One terminal engine, one boundary.** libghostty is the only terminal
  engine, statically linked on every shipping target: `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, and
  `x86_64-pc-windows-msvc`. There is no second parser and no runtime fallback,
  so a target with no pinned archive in `native/libghostty/manifest.toml` is
  not a shipping target and `src-app/build.rs` fails the build outright. The
  archives are release assets, not git content: `scripts/fetch-libghostty.sh`
  (or `.ps1`) places them under `native/libghostty/prebuilt/<target>/lib/`
  after checking the manifest hash, and every CI job that runs cargo does the
  same through `.github/actions/fetch-libghostty`.
  The engine still sits behind `TerminalSessionBackend`
  (`src-app/src/terminal/pty_session/session_backend.rs`) and
  `src-app/src/terminal/types.rs` holds the neutral mirrors: keep engine types
  out of the rest of the app so the renderer keeps a single, stable grid
  vocabulary. A guard test
  (`terminal/types.rs::alacritty_is_absent_from_the_app_crate`) fails if a
  second engine reappears.
- **One git implementation: the `git` CLI.** Every git read and write spawns
  the user's `git` through `src-app/src/git_command.rs`, and a background read
  uses `GitProfile::Probe`. Do not add libgit2, gitoxide (`gix`) or a
  hand-written reader of git's on-disk formats: an in-process engine was built
  and withdrawn in November 2026 (tag `archive/gix-engine`) because a second
  implementation trails git. ARCHITECTURE.md, "Git goes through the git CLI",
  has the reasons and the Windows residual that the CLI keeps (#124).
- **The render thread never blocks.** No synchronous file I/O, git subprocess,
  or recursive directory walk on the GPUI main thread. Push that work through
  `smol::unblock` or the bootstrap path.
- **Three embedded helper binaries have hard size caps** enforced by
  `scripts/check-helper-sizes.sh` in the `Tests (Linux x86_64)` job:
  `paneflow-shim` 512 KiB, `paneflow-ai-hook` ~375 KB, `paneflow-mcp` 512 KiB,
  and the combined cap the script reads from `src-app/build.rs`. They build
  under the `release-min` profile with every build of `paneflow-app`, so run
  the script after `cargo nextest run` on Linux x86_64 to measure them locally.
  Do not add heavy dependencies to them; `toml_edit`, for instance, is
  deliberately confined to `paneflow-mcp-install`.
- **GPUI is pinned to an exact upstream Zed revision** across four `rev` values
  in `src-app/Cargo.toml`, and `gpui_platform` must keep
  `features = ["font-kit"]` or macOS renders empty glyph bitmaps.
- The project is GPL-3.0-or-later. Keep packaging metadata in sync with the root
  `LICENSE` and `Cargo.toml`.

## Running and testing

```bash
scripts/fetch-libghostty.sh              # once per pin: places the libghostty-vt archives (not tracked by git)
cargo run                                # debug build, needs Vulkan
scripts/dev.sh                           # (or .ps1) builds paneflow + paneflow-host and runs the desktop directly, so the detached host outlives Cargo's job
RUST_LOG=info cargo run                  # structured logging
PANEFLOW_LATENCY_PROBE=1 cargo run       # keystroke to pixel latency, debug only
PANEFLOW_PIXEL_PROBE=1 RUST_LOG=debug cargo run   # log terminal cell, glyph and quad pixel positions, debug only
PANEFLOW_PIXEL_PROBE_OVERLAY=1 cargo run # paint the pixel-probe overlay over the terminal grid, debug only
PANEFLOW_DEV_INSTALL_METHOD=apt cargo run # pretend a system-package install (dnf, apt, zypper, rpm-ostree), debug only
PANEFLOW_DEV_FORCE_UPDATE=9.9.9 cargo run # fake an available update to exercise the update UI, debug only
cargo build --release                    # thin LTO, strip, codegen-units=1
cargo test -p paneflow-config            # single crate
cargo test -p paneflow-app --test flex_nchild -- --nocapture   # layout integration
```

Paneflow can be developed from inside a Paneflow pane. A debug build never
resolves the installed app's home (`~/.paneflow`) or IPC socket, even though
every pane exports them, so `scripts/dev.sh`, `cargo run` and `cargo test` stay
on the dev state. A desktop launched from a pane also sheds the pane's markers
(`PANEFLOW_SURFACE_ID`, `PANEFLOW_SESSION_ID`, and the rest of
`paneflow_host::env::PANE_CONTEXT_ENV`) before starting its host and worker.
Drive a live UI or agent test in a debug build on its own tagged home, never in
the installed app, which holds the user's live sessions.
When several agents need their own running instance, `scripts/dev.sh --tag
<name>` (`dev.ps1 -Tag <name>`) sets `PANEFLOW_HOME=~/.paneflow-dev-<name>`:
an isolated home owns its IPC endpoint (`paneflow-ipc-<fingerprint>`), host
and worker, and ignores a `PANEFLOW_SOCKET_PATH` it does not own unless
`PANEFLOW_ALLOW_SOCKET_OVERRIDE=1`, which a harness that pins both needs. A
tag's host and worker outlive the window: stop them with
`PANEFLOW_HOME=~/.paneflow-dev-<name> target/debug/paneflow serve stop` and
`... host stop` before trashing the home.

Put unit tests beside the module when the logic is self-contained; keep broader
UI and layout checks in `src-app/tests/`. Name tests descriptively, for example
`test_three_children_flex_basis`. CI is not a substitute for a manual pass on UI
changes: visual smoke jobs exist but are not exhaustive.

Performance claims need evidence: a heaptrack diff for a memory claim, a
`cargo flamegraph` profile for a CPU claim. Do not ship a perf number you did
not measure. For the terminal pipeline, `scripts/bench-terminal.sh` (or
`.ps1`) runs the reproducible suite in `src-app/src/terminal/perf_bench.rs`
and prints a comparison against `bench/baselines/<os>-<arch>/terminal.json`.
For the code editor, `scripts/bench-editor.sh` (or `.ps1`) runs the suite in
`src-app/src/app/diff_dock/code/perf_bench.rs` and compares against
`editor.json` in the same directory. For the time to first frame,
`scripts/bench-startup.sh` (or `.ps1`) launches the release binary with the
startup trace in `src-app/src/startup_trace.rs` and compares against
`startup.json`. All three share the harness and the single
`#[global_allocator]` in `src-app/src/bench_harness.rs`. A suite compares only
against its own platform's baseline, and `--set-baseline` refuses a dirty
tree; see [bench/README.md](bench/README.md).

Never pass `--profile <run-name>` to isolate a build, a probe, or a benchmark
run. Cargo materializes a complete target tree per profile name, so one profile
per run grows `target/` without bound: by September 2026 this workspace and its
sibling had accumulated 217 throwaway profiles for roughly 290 GB. Declare the
profiles you need in `Cargo.toml` and reuse them. When a run genuinely needs an
isolated build tree, for instance two benchmark runs in parallel, set
`CARGO_TARGET_DIR` to a scratch directory outside the repository and delete it
when the run ends.

`tasks/` holds the tracked PRDs (`tasks/prd-*.md`) and their story status files
(`tasks/prd-*-status.json`). Everything else under it, including every
subdirectory, is local, untracked evidence and review scratch space: never
assume another agent can read it, and a tracked document that names such a path
must state that it is local evidence. When you implement a PRD in a separate
worktree, Arthur still reads the copy in `C:\dev\paneflow`: copy every status
change and every ticked acceptance criterion back to both files there.

## No comments in source code

Rust, shell, PowerShell, and the shim's TypeScript assets carry no comments.
Agent-written comments drift from the code they describe, so intent goes into
names, types, tests, and the documents listed below. The only exceptions are
things a tool reads: shebangs, `# shellcheck` directives, `#Requires`,
PowerShell comment-based help, and the three-line install notice at the top of
`crates/paneflow-shim/assets/*.ts`. Text a tool consumes stays code, so clap
help lives in `#[command(about = ...)]` and `#[arg(help = ...)]` attributes,
never in `///`. Generated bindings under `native/libghostty/` keep whatever
bindgen emits. Vendored crates under `vendor/` keep their upstream text; before
editing one or bumping its crates.io version, read the header of its
`Cargo.toml`, which states the Paneflow change and when to drop the copy.

## Commits and pull requests

Use `type(scope): description`, with the story ID when the work maps to a
tracked task, for example
`feat(agents): US-004 - adapt paneflow-hook for the Codex PID env var`. Keep
commits atomic per user story. Branch names look like `feat/description`.

Arthur is the only visible contributor. Never add AI attribution, a
generated-by note, or a `Co-authored-by` trailer. Never close a GitHub issue or
use an auto-closing keyword without explicit approval; default to `Refs #...`.

A PR should explain the user-visible behavior, list the validation steps you
actually ran, link the issue, and include a screenshot or short recording for UI
changes.

## Where the rest of the knowledge lives

| Topic | File |
|---|---|
| Module layout, thread model, keystroke-to-pixel path, agent lifecycle, IPC, self-update | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Visual thesis, color roles, geometry, motion, component contracts, UI delivery gate | [DESIGN.md](DESIGN.md) |
| Release mechanics: bump, tag, chain, stream verification | [docs/release/runbook.md](docs/release/runbook.md) |
| Per-platform signing runbooks | [docs/release/linux-signing.md](docs/release/linux-signing.md), [docs/release/macos-signing.md](docs/release/macos-signing.md), [docs/release/windows-signing.md](docs/release/windows-signing.md) |
| libghostty engine build and qualification per platform | [docs/release/libghostty-linux.md](docs/release/libghostty-linux.md), [docs/release/macos-libghostty.md](docs/release/macos-libghostty.md), [docs/release/windows-libghostty.md](docs/release/windows-libghostty.md) |
| Rust toolchain pin | [docs/release/rustfmt.md](docs/release/rustfmt.md) |
| Public user docs, a mirror of paneflow.dev/docs synced from the site repo | [docs/user/](docs/user/README.md) |

Everything per-user lives under one directory, `~/.paneflow`
(`%USERPROFILE%\.paneflow` on Windows, `~/.paneflow-dev` for debug builds,
`PANEFLOW_HOME` overrides it): `paneflow.json`, `session.json`,
`window-state.json`, `worktrees/`, `avatars/`, `bin/`, `shell/`, `cache/`. The layout is
owned by `crates/paneflow-home`; `src-app/src/runtime_paths.rs` only adds the
IPC socket, which stays in the runtime dir. Never resolve Paneflow's own state
through `dirs::config_dir`, `dirs::cache_dir`, or `dirs::data_local_dir`; those
remain for other tools' directories (Claude, Codex, opencode configs).
