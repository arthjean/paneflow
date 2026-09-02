# Linux libghostty engine

Ghostty is the only terminal engine, and Linux builds link it unconditionally:
bare `cargo run`, normal development builds, tests, and the official x86_64 and
ARM64 packages. Windows x64 MSVC and macOS Apple Silicon consume their own
separately qualified archives; see
[windows-libghostty.md](windows-libghostty.md) and
[macos-libghostty.md](macos-libghostty.md).

## Development build

From the repository root on Linux:

```bash
cargo run
```

No Zig installation or local Ghostty checkout is required. Paneflow versions
the verified headers, bindings, and build metadata for both supported Linux
targets under `native/libghostty/prebuilt/<target>`; the archives are assets
of the GitHub pre-release the manifest names, and
`scripts/fetch-libghostty.sh` places them there after verifying their hashes.

The native sys crate resolves its input in this order:

1. `PANEFLOW_LIBGHOSTTY_DIR` when a maintainer or workflow selects an archive.
2. `native/libghostty/prebuilt/<target>` for every standard build.

`build.rs` verifies and links the prepared archive. It never downloads
anything and never invokes Zig or an external checksum or symbol-inspection
command; a missing archive fails with a pointer to the fetch script. The
manifest pins the reviewed archive hash for each target.

## CI and release

The libghostty Linux workflow regenerates the static archive from the pinned
Ghostty SHA with Zig 0.16.0, exports its directory through
`PANEFLOW_LIBGHOSTTY_DIR`, and runs the native ABI, test, corpus, fuzz, stress,
package, notice, size, and static-link checks. These are normal regression and
supply-chain checks, not a separate product approval process.
The notice check compares the packaged third-party notice with the manifest's
reviewed SHA-256 and requires every statically bundled component marker.

The release workflow follows the same rule for Linux x86_64 and ARM64: it
generates the pinned archive, selects it explicitly, builds Paneflow, verifies
static linkage, then packages it. macOS and Windows do the same with their own
verified archives, without requiring a local Ghostty checkout.

## Updating the pinned native input

Update both `source_sha` in `native/libghostty/manifest.toml` and `GHOSTTY_SHA`
in the Linux and release workflows. Regenerate bindings and both native target
directories, review ABI and behavioral differences, then replace the matching
files under `native/libghostty/prebuilt/<target>`.

The prebuilt directory for each target must contain (the `lib/` entry is
fetched, the rest is committed):

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
`backend=ghostty failure_phase=none`.

Check Unicode rendering, keyboard and IME input, bracketed paste, mouse
reporting, clipboard copy, search, OSC 8 links, OSC 133 prompt marks, resize,
final output, exit overlays, session restore, and child teardown. Do not record
terminal bytes, commands, cwd, clipboard text, agent identifiers, or session
text.

## When the engine fails to start

There is no rollback: Ghostty is the only engine. A startup failure is reported
in the pane with its `failure_phase`, `reason_code`, and OS error, and a failure
after PTY spawn never starts a second child. Report the diagnostic line rather
than switching backends.
