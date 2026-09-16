# Paneflow Architecture

Paneflow is a native GPU-accelerated terminal workspace for running CLI coding
agents in parallel. One user-facing Rust binary, no web runtime: the UI is
built on a pinned Paneflow branch of
[Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui),
terminal emulation is provided by a pinned, statically linked
[`libghostty-vt`](https://github.com/ghostty-org/ghostty) engine on every
shipping target. It is the only engine: there is no second parser and no
runtime fallback. Paneflow owns PTY lifecycle orchestration, rendering, and
integration with agent tracking, IPC, the MCP bridge, and self-update.

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
| `paneflow-host` | `crates/paneflow-host/` | GPU-free local host library and executable: owns PTYs, child processes, canonical libghostty state and the durable session manifests under `~/.paneflow/host/` |
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

## Detached panes

`app/detached_panes` opens a lightweight native GPUI window around the existing
`Entity<Pane>`. The workspace remains the logical owner: IPC lookup, terminal
processes, surfaces, and session serialization retain their original identity.
The main layout projects only docked leaves and normalizes their visible ratios.
Returning a pane restores its original logical position without spawning a PTY.

Terminal and code-view focus subscriptions follow the current native window.
The child observes ownership changes instead of reading the app during render,
because native window creation can synchronously render while the app is borrowed.
Window bounds are debounced before session snapshot construction. A detached
pane restores only when the saved layout leaf count still matches its tab.
Closing a child returns its pane; closing the main window still quits the app.

Detachment is exposed through pane controls, the pane context menu, and
`toggle_detached_pane` (`Cmd/Ctrl+Alt+Shift+D`). Native cross-window drag is not
implemented. It requires portable native drag support beyond the pinned GPUI API.

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
│ Ghostty        │  │ socket server  │  │ git state        │
│                │  │                │  │                  │
└────────────────┘  └────────────────┘  └──────────────────┘
```

- **Main thread**: the GPUI event loop. All UI state lives in `Entity<T>`
  values mutated through GPUI contexts; there are no locks around UI state.
- **Terminal workers**: each session owns a `paneflow-ghostty-runtime` worker
  and a PTY reader; Windows also uses a dedicated ConPTY closer so pipe
  drainage cannot block teardown. The worker publishes backend-neutral events
  to the view and owned render snapshots through `TerminalSessionBackend`.
- **IPC thread**: accepts connections on a Unix socket (Linux/macOS) or named
  pipe (Windows). Stateless methods reply in place; stateful methods are
  dispatched to the main thread through a bounded channel and drained by the
  50 ms app poll loop.
- Blocking work (git subprocesses, filesystem walks, fleet-wide search) is
  pushed to background executors - registering a recursive file watcher or
  scanning a monorepo on the render thread is how you get a
  "not responding" window, so the codebase treats the main thread as
  render-only.

## Editor and diff syntax highlighting

The editor's `CodeHighlighter` and the diff view's `highlight_lines` share
`diff/highlighter.rs`: grammar selection and capture resolution use one path.
Fifteen Zed highlighting queries are compiled with `include_str!` from
`diff/queries/`. TOML, HTML, Java and Ruby keep their grammar's stock queries.
JavaScript uses the JavaScript query on the existing TSX grammar. Markdown
runs block and inline passes; fenced code does not inject another language.

Captures are ordered by byte start, preserving query order at equal starts.
Each styled capture goes onto a stack. The last active capture paints until
its end or the next capture, including when it is wider than an earlier one.
Captures without a palette role do not enter the stack. Each row still caps
input at 4,096 captures. Theme changes rebuild color tables without querying
or reparsing the retained trees. Variables and namespaces use the editor's
text color; constructors share the function role.

`diff/queries/MANIFEST.toml` records the upstream commit, source paths, SHA-256
hashes, license evidence and the JavaScript grammar deviation. `NOTICE`
attributes Zed Industries. Git preserves LF bytes for these imports.

Set `ZED_DIR` to a local Zed checkout, then run
`scripts/sync-zed-queries.sh --check` on Linux/macOS, or
`scripts/sync-zed-queries.ps1 -Check` in PowerShell 7 on Windows. Check mode
compares every query and the manifest byte for byte with the pinned Git
objects, checks the provenance notice and lists any drift. Omit the check
option to restore those bytes, including the notice's revision.
To resync, pass `--commit <revision>` or `-Commit <revision>`, then review the
upstream license evidence, compile the queries, run the parity tests and the
editor benchmark. Both scripts resolve revisions to an immutable full SHA.
No source checkout files or runtime configuration are read by the app.

## Files tree

The right Files rail has its own `FilesSidebar` GPUI entity, mounted with
the view cache. `PaneFlowApp` owns its placement, workspace association and
editor integration; row hover and keyboard selection update the rail entity.
The implementation is under `src-app/src/app/files_sidebar/`.

Three states have separate lifetimes, following Zed's project panel model:

- `worker.rs` owns the directory snapshot and nonrecursive filesystem watches
  on a dedicated thread. It registers each watch before reading the directory,
  loads newly expanded directories, and refreshes invalidated listings. Loaded
  collapsed directories remain searchable and watched. Deleted or ignored
  subtrees are pruned. Watch events are coalesced in the worker; unavailable
  watches fall back to background polling and registration retries.
- `projection.rs` prepares ordered rows, labels, icons, filter highlights and
  a path-to-index map on GPUI's background executor. Tree, expansion and query
  changes replace the pending projection task. Epoch and revision checks
  discard results from an earlier root, fold state or query.
- `panel.rs` owns the current projection, path-based selection, input focus and
  uniform-list scroll handle. `view.rs` renders only the requested row range.
  Hover uses the immediate squircle state and does not flatten, filter or sort
  the tree. Selection survives insertions; a collapsed selection returns to
  its visible ancestor. Keyboard navigation reveals the selected row.

Closing or switching workspaces cancels the worker and pending publications.
The closing animation keeps its last snapshot until it finishes, then releases
the snapshot on a background executor. The rail keeps Paneflow's 300 px width,
28 px rows, icons, indentation and full-width hover inside the 8 px edge insets.

## Keystroke → pixel

The full input/output pipeline, end to end:

```
KeyDownEvent
  → TerminalView::handle_key_down()
  → Ghostty structured input
  → backend writer → PTY → shell / agent CLI
  → output bytes → libghostty-vt engine
  → PublishGate → owned neutral Content in SharedState
  → TerminalBackendEvent → sync() → cx.notify()
  → TerminalSessionBackend::render_content() → Arc clone of that Content
  → TerminalElement::prepaint()  - memoized on Content::generation
  → TerminalElement::paint()     - quads + shaped glyph runs
  → GPU (Vulkan on Linux, Metal on macOS, DirectX on Windows)
```

The leading Ghostty wakeup notifies GPUI immediately. Presentation follows the
platform frame scheduler.

Two gates decide how much of that pipeline actually runs, and both exist
because the natural rate of each stage is far above the rate a display can
show.

**`PublishGate` (`terminal/ghostty_session.rs`), on the runtime thread.**
Snapshotting the grid and converting it into the neutral `Content` is the
expensive half of an output batch, and `OUTPUT_BATCH_MAX_TIME` closes a batch
every millisecond. The gate holds a publication back for two reasons: DEC mode
2026 is set, meaning the program is mid-redraw and the frame would tear (the
same check Ghostty's renderer makes in `src/renderer/generic.zig`), or the last
frame is newer than `MIN_PUBLISH_INTERVAL`. A held change is deferred, never
dropped: `PublishGate::next_wake` shortens the runtime loop's block to the
interval's end, and the loop's `poll` publishes it then. The rate limit applies
whether or not more output is queued behind the change, because a program that
prints a line every couple of milliseconds (which is what ConPTY delivers for
most output) never builds a backlog yet would otherwise be snapshotted hundreds
of times a second for frames no display shows. The first change after an idle
gap always publishes at once. Input arms a one-shot interactive publication
for the next output arriving within 100 ms: it bypasses the ordinary rate
limit, including while another output stream is active, but still respects
DEC 2026. This is an input hint, not identification of the echoed character.
A DEC 2026 hold
expires after `SYNC_OUTPUT_MAX_HOLD` so a program that opens a frame and dies
cannot freeze the pane. Resizes, scrolls, other state the user waits on, and
the last frame before `ChildExited` bypass both, which is also what keeps a
deferred change from being lost when the loop exits.
The conversion behind a publish is incremental: the binding reports which rows
changed since the previous snapshot (`Content::dirty_rows`), and `CellMirror`
converts only those, alternating two cell buffers so the one the render thread
reads is never the one being written. A keystroke echo converts one row; a
full-viewport scroll converts every row. The neutral snapshot carries stable
`row_versions` so a renderer that skips publications can still identify every
changed row. Cell buffers are reused when no reader retains them.
With nothing pending, the runtime loop blocks for `RUNTIME_QUIET_TICK` once a
pane has been silent for a second, and drops back to `RUNTIME_IDLE_TICK` while
output flows, a drag is held, or a child is winding down.
Because a wakeup is queued only when a frame is actually published, this also
stops the UI thread being woken for frames it would discard.

Native terminal search uses Ghostty's total, selected match, and viewport-match
APIs. The runtime publishes only visible highlights to GPUI; navigation does
not invalidate the cell snapshot. Global scrollbar offsets are cached until
terminal contents, dimensions, or the result count change, and their pixel
projection is cached separately from the terminal layout. During output,
global offsets refresh at most every 100 ms; visible matches and selection
continue updating immediately. A pending rail refresh keeps a trailing wakeup
even after output stops. Changing the query refreshes the rail immediately.
Held navigation
keys queue in order, with one command in flight. A publication acknowledges
that command by generation before the next frame dispatches the next one;
an unrelated output publication cannot acknowledge a navigation command.

**The layout memo (`terminal/element/`), on the render thread.** Terminal
views use GPUI's view cache. Within an active view, `build_layout` first checks
its complete frame key, then uses a per-row cache when the content generation
changed. Each retained row owns its prepared text runs, decorations, sprites
and background regions. Unchanged row versions reuse that data without walking
the row's cells. Background regions are still merged across rows for painting.
Theme, font, geometry, selection, search highlights and viewport changes
invalidate the relevant cache; cursor and other frame metadata are rebuilt
independently. No cached layout retains the input cell buffer, preserving the
worker's double-buffer reuse.

Font settings are resolved from the configuration loaded during bootstrap and
published on configuration reload or an in-app settings change. Rendering reads
only that in-memory snapshot, with no configuration-file polling. The service
output detector tracks line length and non-whitespace characters incrementally,
so whitespace-heavy output does not repeatedly rescan the accumulated line.

On Windows both budgets depend on the process holding a `timeBeginPeriod(1)`
for the lifetime of the GUI (`app::win_timer`): without it every millisecond
timeout in the pipeline, including the event batch window and the mailbox idle
tick, rounds up to the default 15.6 ms clock tick.

`TerminalElement` (`src-app/src/terminal/element/`) is the one place Paneflow
implements GPUI's low-level `Element` trait directly instead of composing
divs: terminal rendering wants per-cell control over background quads, glyph
runs, cursor shapes, underlines and hyperlink hitboxes. Everything else in the
app (sidebar, tabs, settings, diff viewer) is regular GPUI flex layout.

The debug `PANEFLOW_LATENCY_PROBE=1` measures input-handler time and time to
the end of CPU painting. It does not identify the echoed character or observe
GPU presentation, and therefore does not measure input-to-visible-pixel latency.

## One terminal engine behind one boundary

`TerminalSessionBackend` is the renderer-facing facade over libghostty. Every
shipping build links the pinned static archive: there is no `terminal.backend`
setting, no portable build, and no runtime fallback. A target with no pinned
archive in `native/libghostty/manifest.toml` is not a shipping target, and
`src-app/build.rs` fails the build rather than producing a binary that cannot
run a shell.

Ghostty's raw ABI and static archive linking live in
`paneflow-libghostty-sys`; `paneflow-terminal-ghostty` exposes the safe Rust
interface. The facade still matters even with a single engine: no borrowed
terminal state reaches GPUI, and the rest of the app consumes Paneflow-owned
points, mode flags, cells, events, and `Content` snapshots from
`src-app/src/terminal/types.rs`.

A startup failure is reported in the pane, not routed around. Once a shell
child has been spawned, Paneflow never starts a second child for that
session.

## Agent lifecycle tracking

The feature that makes Paneflow more than a tiling terminal: it knows what
the agents inside its panes are doing.

```
agent CLI (claude, codex, opencode, …)
  └─ launched through a PATH shim (paneflow-shim)
       ├─ agent hooks fire paneflow-ai-hook on lifecycle events ─┐
       ├─ the shim fires session_start / exit / session_end ─────┤
       │                                                         └─ ai.*
       ├─ the agent's own OSC 9;4 + OSC 9/777 in the pane's grid     JSON-RPC
       └─ the agent's own session registry on disk                   socket
                                                                      │
            ┌─────────────────────────────────────────────────────────┘
            └─ one write choke point, ordered by source
                 └─ GUI: tab dots, sidebar spinners, attention queue,
                    desktop notifications carrying the actual question
```

- **Shim**: launching an agent from Paneflow puts a shim directory first in
  `PATH`. The shim records the real PID and process start time (PID-reuse
  safe), then execs the real binary. Sixteen agent CLIs are recognized by
  name; unknown tools are reported as themselves. It emits `session_start`,
  `exit` and `session_end` on its own, so presence and exit never depend on
  the agent cooperating.
- **Hooks**: agents that support lifecycle hooks (Claude Code, Codex, …)
  report `session_start`, `prompt_submit`, `tool_use`, `notification`, `stop`,
  `exit`, and `session_end` through the `ai.*` IPC namespace. Richest source,
  and the only one that names the active sub-tool or carries a turn summary.
- **Three sources, one order**: hooks can be switched off outside Paneflow's
  reach (Claude Code's managed settings do exactly that), so they are not the
  substrate. Two hook-free sources back them: the escape sequences the agent
  writes into its own pane, and the status file it maintains for its own peer
  discovery. `ai_types::AgentStateSource` ranks them
  (`Terminal < SessionRegistry < Hook`) and `upsert_session_state` enforces the
  rank, so a weaker observer never talks over a live stronger one - and a
  stronger one that falls silent hands over instead of freezing the sidebar.
- **States**: thinking, waiting for input (with the actual prompt text),
  finished, errored (non-zero exit). Each state routes to the UI - and to your own tooling, since
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

## Local host and durable session identity

`paneflow-host` is a GPU-free crate (library plus `paneflow-host` executable)
that owns terminal execution independently of a GPUI entity: the PTY pair, the
child process handle, the canonical `libghostty` terminal, an 8 MiB output
tail with monotonic byte offsets and the session manifests. Nothing in it
links GPUI. The desktop still runs its in-process terminal path today; the
host becomes the runtime owner when the desktop attaches to it in a later
increment.

Identity is durable and lives in `paneflow-config`: `WorkspaceId` and
`SessionId` are hyphenated UUIDs persisted in `session.json` (schema version
3, migrated from v2 by assigning ids without touching the layout), never a
GPUI entity id, a PID, a cwd string or a connection id. A `HostInstanceToken`
identifies one running owner per state home and a `SessionGeneration` counts
explicit restarts. Host records sit under `~/.paneflow/host/`:
`instance.json` for the running owner and `sessions/<SessionId>.json` for
each manifest (identity, cwd, launch metadata, lifecycle, process identity
with kernel start time, agent summary). A manifest or PID alone never proves
ownership: a new host instance marks inherited running records `lost` and
refuses to signal them.

The host endpoint is derived from the state home (`\\.\pipe\paneflow-host-<fp>`
on Windows, `<runtime dir>/paneflow-host-<fp>.sock` on Unix), so an isolated
`PANEFLOW_HOME` never shares an endpoint or a record directory with the normal
one. The protocol is JSON-RPC 2.0 lines on that endpoint: `host.hello` must
open every connection and verifies the protocol version and the terminal
engine identity (libghostty source sha and API version) before any effect;
`session.list/create/ensure/inspect/stop`, `session.attach` (native snapshot
checkpoint plus its output offset, captured in one runtime operation),
`session.output` (contiguous bytes from an offset, optionally followed),
`session.input`, `session.resize` and `agent.snapshot` follow. Control frames
are capped at 64 KiB, data chunks at 1 MiB, a checkpoint at 64 MiB; an
oversized frame or checkpoint is refused without unbounded allocation and
without stopping the session. The host is the only responder to terminal
queries; clipboard, bell and notification effects are not replayed from
history.

## Self-update

A single background thread polls the GitHub releases feed at launch and then
every four hours (thirty minutes after a failure), handing each result to the
GPUI tick through a shared slot; a manual `Check for Updates…` wakes the same
thread through an mpsc channel and the next result also drives the title bar
check pill. A result never displaces a download in flight or a staged binary
waiting for restart. Each install format has its own update
path (apt/dnf repos, AppImage swap, tarball swap, macOS app replacement,
Windows MSI relay), all driven by one in-app updater. Update artifacts are verified with
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
| Terminal engine | `libghostty-vt`, statically linked | `libghostty-vt`, statically linked (Apple Silicon) | `libghostty-vt`, statically linked (x64 MSVC) |
| PTY | `portable-pty` | `portable-pty` | ConPTY via `portable-pty` |
| IPC | Unix socket | Unix socket | Named pipe |
| Packaging | `.deb` / `.rpm` / AppImage / tarball | signed + notarized `.dmg` | signed `.msi` |

Linux, macOS Apple Silicon, and Windows x64 ship as release artifacts today.
macOS Intel and Windows ARM64 are not in the current release matrix; see
[`README.md`](README.md#install) and
[`docs/user/installation/windows.md`](docs/user/installation/windows.md) for
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
