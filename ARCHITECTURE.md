# Paneflow Architecture

Paneflow is a native GPU-accelerated terminal workspace for running CLI coding
agents in parallel. One user-facing Rust binary, no web runtime: the UI is
built on a pinned Paneflow branch of
[Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui),
terminal emulation is provided by a pinned, statically linked
`libghostty-vt` backend by default on Linux and Windows x64 MSVC, and by
explicit opt-in on macOS Apple Silicon. Upstream
[`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) remains the
macOS default and the explicit cross-platform rollback. Paneflow owns backend
selection, PTY lifecycle orchestration, rendering, and integration with agent
tracking, IPC, the MCP bridge, and self-update.

This document describes how the pieces fit together. It is aimed at
contributors and at anyone curious how you build a multiplexing terminal app
without Electron.

## Workspace layout

The repo is a Cargo workspace with one binary crate and a set of small,
focused library crates:

| Crate | Path | Purpose |
|---|---|---|
| `paneflow-app` | `src-app/` | The GPUI application and `paneflow` CLI entrypoint: UI, panes, PTY sessions, IPC server, self-update |
| `paneflow-libghostty-sys` | `crates/paneflow-libghostty-sys/` | Raw Ghostty ABI plus verification and linking of the pinned static archive |
| `paneflow-terminal-ghostty` | `crates/paneflow-terminal-ghostty/` | Safe Rust interface over Ghostty terminal state, input, search, selection, and owned render snapshots |
| `paneflow-ghostty-smoke` | `crates/paneflow-ghostty-smoke/` | Package-level native smoke binary for Ghostty, PTY I/O, resize, and shutdown verification |
| `paneflow-config` | `crates/paneflow-config/` | Config schema, tolerant JSON loader, file watcher |
| `paneflow-shim` | `crates/paneflow-shim/` | PATH shim wrapping 16 known agent CLIs so Paneflow can observe their lifecycle |
| `paneflow-ai-hook` | `crates/paneflow-ai-hook/` | The hook binary agent CLIs invoke to report session events back over IPC |
| `paneflow-ipc-client` | `crates/paneflow-ipc-client/` | Blocking JSON-RPC client for the local IPC socket (shared by the MCP bridge and the CLI) |
| `paneflow-mcp` | `crates/paneflow-mcp/` | Stdio MCP server exposing read-only pane access (`list_panes`, `read_pane`, `search_pane`) |
| `paneflow-mcp-install` | `crates/paneflow-mcp-install/` | GPU-free install engine for the MCP bridge: per-agent detection, idempotent config merge, backup + atomic write |
| `paneflow-process` | `crates/paneflow-process/` | Bounded external-process execution (wall-clock deadline + stdout cap) shared across crates |
| `paneflow-acp` | `crates/paneflow-acp/` | Legacy Claude/Codex identity enum plus the `CLAUDECODE` environment scrub |
| `paneflow-telemetry` | `crates/paneflow-telemetry/` | Opt-in telemetry plumbing (no event leaves the machine unless consent resolves to `true`) |

`src-app` is the default workspace member, so bare `cargo run` starts the
desktop app instead of becoming ambiguous across helper binaries. The split is
deliberate: anything that runs *outside* the GUI process (shim, hook, MCP
bridge, MCP installer logic) must stay GPU-free and tiny, so it lives in its
own crate and never links GPUI.

## Thread model

```
┌─────────────────────────────────────────────────────────┐
│ Main thread - GPUI event loop                           │
│   owns all Entity state, rendering, input dispatch      │
└─────────────────────────────────────────────────────────┘
        ▲                    ▲                    ▲
        │ Backend events     │ mpsc (50ms poll)   │ channel
┌───────┴────────┐  ┌────────┴───────┐  ┌─────────┴────────┐
│ Terminal       │  │ IPC thread     │  │ Watcher threads  │
│ workers        │  │ JSON-RPC 2.0   │  │ config, theme,   │
│ Ghostty or     │  │ socket server  │  │ git state        │
│ Alacritty      │  │                │  │                  │
└────────────────┘  └────────────────┘  └──────────────────┘
```

- **Main thread**: the GPUI event loop. All UI state lives in `Entity<T>`
  values mutated through GPUI contexts; there are no locks around UI state.
- **Terminal workers**: the backend is selected before the shell child is
  created. Each Ghostty session owns a `paneflow-ghostty-runtime` worker and a
  PTY reader; Windows also uses a dedicated ConPTY closer so pipe drainage
  cannot block teardown. Alacritty sessions retain their `EventLoop` I/O
  thread and shared terminal grid. Both implementations publish
  backend-neutral events to the view and owned render snapshots through
  `TerminalSessionBackend`.
- **IPC thread**: accepts connections on a Unix socket (Linux/macOS) or named
  pipe (Windows). Stateless methods reply in place; stateful methods are
  dispatched to the main thread through a bounded channel and drained by the
  50 ms app poll loop.
- Blocking work (git subprocesses, filesystem walks, fleet-wide search) is
  pushed to background executors - registering a recursive file watcher or
  scanning a monorepo on the render thread is how you get a
  "not responding" window, so the codebase treats the main thread as
  render-only.

## Keystroke → pixel

The full input/output pipeline, end to end:

```
KeyDownEvent
  → TerminalView::handle_key_down()
  → Ghostty structured input or Alacritty escape-sequence input
  → selected backend writer → PTY → shell / agent CLI
  → output bytes → libghostty-vt engine or Alacritty VTE / Term grid
  → TerminalBackendEvent → sync() → cx.notify()
  → TerminalSessionBackend::render_content() → owned neutral Content
  → TerminalElement::prepaint()
  → TerminalElement::paint()     - quads + shaped glyph runs
  → GPU (Vulkan on Linux, Metal on macOS, DirectX on Windows)
```

The first Ghostty wakeup on Linux can render immediately. Windows Ghostty and
Alacritty wakeups are coalesced into the 4 ms event batch. This keeps the
renderer backend-independent without imposing the same scheduling policy on
different PTY implementations.

`TerminalElement` (`src-app/src/terminal/element/`) is the one place Paneflow
implements GPUI's low-level `Element` trait directly instead of composing
divs: terminal rendering wants per-cell control over background quads, glyph
runs, cursor shapes, underlines and hyperlink hitboxes. Everything else in the
app (sidebar, tabs, settings, diff viewer) is regular GPUI flex layout.

Debug builds can trace the whole pipeline: `PANEFLOW_LATENCY_PROBE=1` stamps a
keystroke at ingress and reports time-to-pixel.

## Dual terminal engines behind one boundary

`TerminalSessionBackend` is the renderer-facing facade for both engines.
Standard Linux builds and supported Windows x64 MSVC builds resolve
`terminal.backend = auto` to Ghostty. macOS Apple Silicon builds resolve `auto`
to Alacritty and switch engines only on an explicit
`terminal.backend = ghostty`; promoting that default is a separate decision.
Builds without a verified native Ghostty feature and explicit rollback sessions
use upstream `alacritty_terminal`. The choice applies to new sessions only.

Ghostty's raw ABI and static archive linking live in
`paneflow-libghostty-sys`; `paneflow-terminal-ghostty` exposes the safe Rust
interface. Alacritty imports remain confined to an explicit allowlist. Neither
engine leaks borrowed terminal state into GPUI: the rest of the app consumes
Paneflow-owned points, mode flags, cells, events, and `Content` snapshots.

The two engines are interchangeable for everything the renderer draws, but
Ghostty decodes a few sequences Alacritty does not. OSC 9;4 progress reporting
is one: on a Ghostty session the pane header shows the running program's
progress chip, and on an Alacritty session the pane simply never reports
progress. Any such extra must degrade to silence, never to a broken pane.

A Ghostty startup failure may fall back to Alacritty only before the shell
child exists. Once a child has been spawned, Paneflow never starts a second
child or switches the live session to another engine.

## Agent lifecycle tracking

The feature that makes Paneflow more than a tiling terminal: it knows what
the agents inside its panes are doing.

```
agent CLI (claude, codex, opencode, …)
  └─ launched through a PATH shim (paneflow-shim)
       └─ agent hooks fire paneflow-ai-hook on lifecycle events
            └─ ai.* JSON-RPC notifications over the local socket
                 └─ GUI: tab dots, sidebar spinners, attention queue,
                    desktop notifications carrying the actual question
```

- **Shim**: launching an agent from Paneflow puts a shim directory first in
  `PATH`. The shim records the real PID and process start time (PID-reuse
  safe), then execs the real binary. Sixteen agent CLIs are recognized by
  name; unknown tools are reported as themselves.
- **Hooks**: agents that support lifecycle hooks (Claude Code, Codex, …)
  report `session_start`, `prompt_submit`, `tool_use`, `notification`, `stop`,
  `exit`, and `session_end` through the `ai.*` IPC namespace. Agents without
  hooks fall back to process-tree and terminal-activity detection.
- **States**: thinking, waiting for input (with the actual prompt text),
  finished, errored (non-zero exit), stalled (no hook activity past a
  threshold). Each state routes to the UI - and to your own tooling, since
  the same events are observable over IPC.

The default loop is human-in-the-loop: Paneflow pre-fills prompts into real PTY
sessions and the user submits them. Auto-submit exists only as an explicit,
gated scripting path.

## IPC and the MCP bridge

A JSON-RPC 2.0 endpoint (Unix socket at `$XDG_RUNTIME_DIR/paneflow/`, named
pipe on Windows) exposes `workspace.*`, `surface.*`, `fleet.*`, `events.*`,
and `ai.*` namespaces - enough to script workspace creation, read panes, send
text behind the scripting gate, and subscribe to agent events. The `paneflow`
CLI (`paneflow up`, `paneflow flow`, `paneflow watch`, `paneflow wait`) is
built on the same socket.

The MCP bridge re-exposes a read-only slice of this to agents themselves:
`paneflow mcp install` registers a stdio MCP server with Claude Code, Codex,
Gemini CLI and opencode, giving any agent the ability to *read* (never write)
other panes' scrollback. An agent debugging a failing dev server can read the
server pane's output directly instead of asking you to paste it. The bridge
binary ships embedded in the main binary and is extracted to a stable path at
launch, so there is nothing extra to install.

Ingress is treated as untrusted: session and config files are validated
structurally (layout budgets, ratio clamps, id alphabets) before they touch
app state.

## Self-update

Each install format has its own update path (apt/dnf repos, AppImage swap,
tarball swap, macOS app replacement, Windows MSI relay), all driven by one
in-app updater. Update artifacts are verified with
[minisign](https://jedisct1.github.io/minisign/) signatures and the client
**fails closed**: an unsigned or tampered artifact is rejected, never installed.
macOS builds add Developer ID / notarization checks with Team ID pinning;
Windows MSI updates add `WinVerifyTrust` before `msiexec` runs.

## Telemetry (opt-in, fail-closed)

Telemetry is **disabled by default**. A first-run modal asks for consent; no
event is sent unless the answer is an explicit yes. `PANEFLOW_NO_TELEMETRY=1`,
`DO_NOT_TRACK`, or `NO_TELEMETRY` override everything unconditionally. The full
client lives in `crates/paneflow-telemetry/`; app-level emitters live in
`src-app/src/app/telemetry_events.rs`. The event surface covers app lifecycle,
update funnel, telemetry re-enable, and session-corruption events, with no
terminal content, no paths, and no prompts.

## Cross-platform strategy

One codebase, three first-class targets. Platform-specific code is gated
behind `#[cfg(target_os)]` with a working path (or a documented stub) for the
other two platforms:

| Concern | Linux | macOS | Windows |
|---|---|---|---|
| GPU | Vulkan | Metal | DirectX |
| Windowing | Wayland + X11 | AppKit | Win32 |
| Terminal engine | `libghostty-vt` by default, Alacritty rollback | Alacritty by default, `libghostty-vt` as an opt-in on Apple Silicon | `libghostty-vt` by default on x64 MSVC, Alacritty rollback |
| PTY | `portable-pty` for Ghostty, `alacritty_terminal::tty` for rollback | `portable-pty` for Ghostty, `alacritty_terminal::tty` otherwise | ConPTY via `portable-pty` for Ghostty, `alacritty_terminal::tty` for rollback |
| IPC | Unix socket | Unix socket | Named pipe |
| Packaging | `.deb` / `.rpm` / AppImage / tarball | signed + notarized `.dmg` | signed `.msi` |

Linux, macOS Apple Silicon, and Windows x64 ship as release artifacts today.
macOS Intel and Windows ARM64 are not in the current release matrix; see
[`README.md`](README.md#install) and [`docs/WINDOWS.md`](docs/WINDOWS.md) for
the support matrix.

## Performance discipline

Perf claims in release notes are backed by reproducible procedures, not
vibes: heaptrack diffs for memory work, `cargo flamegraph` for CPU work,
criterion benchmarks for hot paths, and a keystroke-latency probe in debug
builds. The render thread never does blocking I/O; scans and searches that
touch the filesystem or many panes run on background executors and report
back through events.

## Building

```bash
cargo build --release    # LTO thin, strip, codegen-units=1
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

See the [README](README.md#build-from-source) for per-platform build
instructions and system dependencies.
