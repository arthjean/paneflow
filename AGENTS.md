# Paneflow agent guidance

Paneflow is a native Rust terminal workspace for running coding agents in
parallel, built on Zed's GPUI framework. Linux, macOS, and Windows are all
shipping targets. This file is the canonical instruction source for every agent;
`CLAUDE.md` imports it.

## Gates you must not skip

Run from the repository root. The toolchain is pinned in `rust-toolchain.toml`
(1.98.0), so do not add `+stable` or `+nightly`.

```bash
cargo fmt --check                                   # mandatory before commit and push
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check advisories licenses sources        # only when dependencies change
```

**`cargo fmt --check` before every `git commit` and every `git push` that
touches Rust code.** If it reports a diff, run `cargo fmt`, re-stage, then
commit. The release pipeline runs the same check on all four Build jobs (Linux
x86_64, Linux aarch64, macOS aarch64, Windows x86_64). One mis-formatted line
fails all four legs, skips "Publish GitHub Release", and burns a 25 minute run
for nothing. A dirty tag commit is worse: the tag has to be deleted and
re-created at the fix commit, because the original tagged build cannot be
salvaged. For a tag-push release, run `cargo fmt --check` one final time on the
exact commit you are about to tag.

rustfmt output drifts between Rust point releases, so code that was clean last
week can need re-formatting after a toolchain bump. Always run the real
`cargo fmt`, not an editor formatter, because `cargo fmt` is the CI gate.

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
reason = "...")]` on the module, as `src-app/src/diff/view.rs` does.

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
  not a shipping target and `src-app/build.rs` fails the build outright.
  The engine still sits behind `TerminalSessionBackend`
  (`src-app/src/terminal/pty_session.rs`) and `src-app/src/terminal/types.rs`
  holds the neutral mirrors: keep engine types out of the rest of the app so
  the renderer keeps a single, stable grid vocabulary. A guard test
  (`terminal/types.rs::alacritty_is_absent_from_the_app_crate`) fails if a
  second engine reappears.
- **The render thread never blocks.** No synchronous file I/O, git subprocess,
  or recursive directory walk on the GPUI main thread. Push that work through
  `smol::unblock` or the bootstrap path.
- **Three embedded helper binaries have hard size caps** enforced by the
  `Release build + binary-size budget (Linux x86_64)` job: `paneflow-shim` 512
  KiB, `paneflow-ai-hook` ~375 KB, `paneflow-mcp` 512 KiB, and a combined cap
  that must stay in sync with `src-app/build.rs`. They build under the
  `release-min` profile. Do not add heavy dependencies to them; `toml_edit`, for
  instance, is deliberately confined to `paneflow-mcp-install`.
- **GPUI is pinned to an exact upstream Zed revision** across four `rev` values
  in `src-app/Cargo.toml`, and `gpui_platform` must keep
  `features = ["font-kit"]` or macOS renders empty glyph bitmaps.
- The project is GPL-3.0-or-later. Keep packaging metadata in sync with the root
  `LICENSE` and `Cargo.toml`.

## Running and testing

```bash
cargo run                                # debug build, needs Vulkan
RUST_LOG=info cargo run                  # structured logging
PANEFLOW_LATENCY_PROBE=1 cargo run       # keystroke to pixel latency, debug only
cargo build --release                    # thin LTO, strip, codegen-units=1
cargo test -p paneflow-config            # single crate
cargo test -p paneflow-app --test flex_nchild -- --nocapture   # layout integration
```

Put unit tests beside the module when the logic is self-contained; keep broader
UI and layout checks in `src-app/tests/`. Name tests descriptively, for example
`test_three_children_flex_basis`. CI is not a substitute for a manual pass on UI
changes: visual smoke jobs exist but are not exhaustive.

Performance claims need evidence: a heaptrack diff for a memory claim, a
`cargo flamegraph` profile for a CPU claim. Do not ship a perf number you did
not measure. For the terminal pipeline, `scripts/bench-terminal.sh` (or
`.ps1`) runs the reproducible suite in `src-app/src/terminal/perf_bench.rs`
and prints a comparison against `bench/baseline.json`; see
[bench/README.md](bench/README.md).

`tasks/` is a local, untracked scratch area for PRDs and story status files. It
is not part of the repository, so never reference it from a tracked document and
never assume another agent can read it.

## No comments in source code

Rust, shell, PowerShell, and the shim's TypeScript assets carry no comments.
Agent-written comments drift from the code they describe, so intent goes into
names, types, tests, and the documents listed below. The only exceptions are
things a tool reads: shebangs, `# shellcheck` directives, `#Requires`,
PowerShell comment-based help, and the three-line install notice at the top of
`crates/paneflow-shim/assets/*.ts`. Text a tool consumes stays code, so clap
help lives in `#[command(about = ...)]` and `#[arg(help = ...)]` attributes,
never in `///`. Generated bindings under `native/libghostty/` keep whatever
bindgen emits.

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
| Per-platform signing runbooks | [docs/release/linux-signing.md](docs/release/linux-signing.md), [docs/release/macos-signing.md](docs/release/macos-signing.md), [docs/release/windows-signing.md](docs/release/windows-signing.md) |
| libghostty engine build and qualification per platform | [docs/release/libghostty-linux.md](docs/release/libghostty-linux.md), [docs/release/macos-libghostty.md](docs/release/macos-libghostty.md), [docs/release/windows-libghostty.md](docs/release/windows-libghostty.md) |
| Rust toolchain pin | [docs/release/rustfmt.md](docs/release/rustfmt.md) |
| Public user docs, a mirror of paneflow.dev/docs synced from the site repo | [docs/user/](docs/user/README.md) |

User configuration lives at `~/.config/paneflow/paneflow.json` on Linux, with
`%APPDATA%` and macOS equivalents resolved through
`src-app/src/runtime_paths.rs`.
