# Linux libghostty backend

Ghostty is the default terminal backend in standard Linux builds. This includes
bare `cargo run`, normal development builds, tests, and official x86_64 and
ARM64 packages. `terminal.backend = auto` selects Ghostty for every new Linux
terminal. Supported Windows x64 MSVC builds use their separately qualified
native Ghostty backend. macOS Apple Silicon ships the engine as an explicit
opt-in and keeps `auto` on Alacritty; see
[macos-libghostty.md](macos-libghostty.md).

## Development build

From the repository root on Linux:

```bash
cargo run
```

No Zig installation or local Ghostty checkout is required. Paneflow versions
the verified native inputs for both supported Linux targets under
`native/libghostty/prebuilt/<target>`.

The native sys crate resolves its input in this order:

1. `PANEFLOW_LIBGHOSTTY_DIR` when a maintainer or workflow selects an archive.
2. `native/libghostty/prebuilt/<target>` for every standard build.

`build.rs` verifies and links the prepared archive. It never downloads Ghostty
and never invokes Zig or an external checksum or symbol-inspection command. The
manifest pins the reviewed archive hash for each target.

To build the Linux Alacritty-only configuration explicitly:

```bash
cargo run -p paneflow-app --no-default-features
```

## CI and release

The libghostty Linux workflow regenerates the static archive from the pinned
Ghostty SHA with Zig 0.15.2, exports its directory through
`PANEFLOW_LIBGHOSTTY_DIR`, and runs the native ABI, test, corpus, fuzz, stress,
package, notice, size, and static-link checks. These are normal regression and
supply-chain checks, not a separate product approval process.
The notice check compares the packaged third-party notice with the manifest's
reviewed SHA-256 and requires every statically bundled component marker.

The release workflow follows the same rule for Linux x86_64 and ARM64: it
generates the pinned archive, selects it explicitly, builds Paneflow with the
default features, verifies static linkage, then packages it. macOS builds with
`--no-default-features` and stays on Alacritty. Windows starts from
`--no-default-features`, explicitly enables `libghostty-windows`, and consumes
the separately verified x64 MSVC archive without requiring a local Ghostty
checkout.

## Updating the pinned native input

Update both `source_sha` in `native/libghostty/manifest.toml` and `GHOSTTY_SHA`
in the Linux and release workflows. Regenerate bindings and both native target
directories, review ABI and behavioral differences, then replace the matching
files under `native/libghostty/prebuilt/<target>`.

The committed prebuilt directory for each target must contain:

- `bindings.rs`
- `build-info.txt`
- `include/ghostty/vt.h`
- `lib/libghostty-vt.a`

The CI-generated archive overrides these committed files, ensuring that a
release validates the source pin it just built instead of silently reusing the
checkout fallback.

## Optional display runbook

When a change touches rendering, input, IME, clipboard, or PTY behavior, run a
release build under native Wayland and under X11 or XWayland. Exercise one shell
pane, one agent pane, and one alt-screen TUI. Confirm diagnostics report
`requested=Auto resolved=ghostty`.

Check Unicode rendering, keyboard and IME input, bracketed paste, mouse
reporting, clipboard copy, search, OSC 8 links, OSC 133 prompt marks, resize,
final output, exit overlays, session restore, and child teardown. Do not record
terminal bytes, commands, cwd, clipboard text, agent identifiers, or session
text.

## Rollback

Set `terminal.backend = alacritty`, then create a new terminal. Existing
sessions keep their current backend. A Ghostty failure before PTY spawn may
fallback once; a failure after PTY spawn never starts a second child.
