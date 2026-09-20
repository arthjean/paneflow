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
| `paneflow-serve` | `crates/paneflow-serve/` | The per-home worker: owns the activity reducer, the session projection and the advertised-capability protocol Controllers speak |
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

Windows PTYs are opened through `paneflow_host::pty::open`, shared by the host
and the app's local session path. It loads a pinned, embedded Microsoft ConPTY
runtime before `portable-pty` opens the PTY. Modern OpenConsole preserves the
ordering of synchronized-output markers and cursor updates; the older system
ConPTY renderer can emit the end marker before the final cursor position.
The runtime is extracted and loaded on session worker threads, with no added
publication delay. See [native/conpty/README.md](native/conpty/README.md) for the
pin, build setup and real-PTY regression test. Unix PTYs keep their native path.

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
- **Hooks own the state once a session latches.** The first hook event of a
  session makes it hook-owned, and from then on only hook events and the
  bounded lease below move it. Raw output growth never starts a busy state.
  Hooks can be switched off outside Paneflow's reach (Claude Code's managed
  settings do exactly that), so a lower-confidence screen tier backs them: a
  runtime that declares `[screen]` rules in its descriptor and has no latch
  takes the host's screen verdict, published as `activity_source = screen`. A
  screen verdict never produces a completion notification, and hooks win the
  moment they latch.
- **Where the reduction happens**: in the worker, never in the core and never
  in a Controller. The core validates a hook frame against the session's
  runtime generation, writes `last-hook-event.json` and records the raw
  `last_hook` on the manifest; the worker
  (`crates/paneflow-serve/src/hook_state.rs`) turns that stream into a state
  and `crates/paneflow-serve/src/state.rs` publishes it with an
  `activity_source` of `hooks`, `screen` or `none`. The GPUI app subscribes and
  renders; it computes no lifecycle of its own.
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

## The worker between the core and its Controllers

`paneflow-serve` is the per-home worker. It sits between the PTY core and
every Controller (the GPUI app, the `paneflow` CLI, the MCP bridge) and owns
the concerns that must survive a restart of the UI but not of the terminals:
the activity reducer, the session projection, and the capability set it
advertises at bootstrap.

The worker is the `paneflow` executable itself, started as
`paneflow serve run --home <home>` and detached through the same
`CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`
path the core uses on Windows, `setsid` on Unix. It holds
`<home>/serve/owner.lock` for its lifetime, so a second start adopts instead
of racing. `paneflow serve status` prints its pid, protocol version, home,
session count and advertised capabilities as JSON.

On Windows the bootstrap copies the executable into a content-addressed
`<home>/serve/runtime/<build-id>/` directory before detaching it. The worker
therefore never locks the application binary that Cargo, an installer or the
updater needs to replace. The build digest is part of `worker.hello`, so a
same-version development rebuild replaces the old worker instead of adopting
stale code.

- **Restarts are free.** On start the worker rebuilds every session from the
  manifests under `<home>/host/sessions/` and the durable seeds under
  `<home>/host/session-data/<id>/last-hook-event.json`. No terminal receives a
  signal, because the worker owns no PTY. When the app ships a newer worker it
  stops the old one with a five second drain and starts its own; the sessions
  list is identical across the swap.
- **The activity reducer.** `hook_state.rs` holds one latch per session.
  `Start` and `UserPromptSubmit` open a turn and arm a five-minute lease;
  `Stop` settles it as completed; `StopFailure` and `Idle` settle it without
  completion; Codex's `Interrupt` settles it as cancelled and only a new
  opening event re-arms it; `PermissionRequest` (except
  `tool_name = AskUserQuestion`) asks for input. `SessionStart`,
  `SubagentStart`, `SubagentStop` and informational notifications latch hook
  ownership and change nothing. A `Stop` whose payload counts pending
  `background_tasks` keeps the pane busy until the count reaches zero.
- **The lease bounds a lost stop.** Every busy turn carries a deadline five
  minutes past its last changed screen signal. The live reducer consumes only
  the host's `screen_changed_at_ms`; raw PTY output growth never re-arms the
  lease. The host's separate `output_changed_at_ms` timestamp may anchor a
  recovered opening seed, so output written before a worker restart is not
  mistaken for new live activity. When the deadline passes the session settles
  to idle without completion and the worker writes a generation-scoped
  watermark to `<session dir>/hook-expiry.json`, so a restart cannot revive the
  turn.
- **Generations reject stale events.** A frame naming a runtime generation
  below the manifest's is refused. An untagged settling event arriving within
  30 seconds of an in-place relaunch is quarantined until the replacement
  runtime opens a turn of its own. For sessions whose launch command is not
  hook-capable, a change in the observed foreground identity
  (`runtime_id:pid:pid_started_at`) resets the latch while keeping the
  generation; the first sighting is recorded, never treated as an edge.
- **Restart replays the durable seed.** `last-hook-event.json` is read with its
  own file handle for both metadata and bytes, capped at 64 KiB, and applied
  with the file's mtime as the event time. An opening event's lease is anchored
  at `max(seed mtime, activity signal)`; a seed holding a stop uses only its own
  mtime, so a later repaint cannot reopen a finished turn.
  `hook-cancellation.json` is restored before the seed, so an escape fence
  survives the restart.
- **The worker decides notifications.** Only a hook-sourced completed `Stop`
  earns a `Finished` decision, and only a `PermissionRequest` or a
  `menu_prompt_active` false-to-true edge earns `Needs input`, deduplicated to
  one per ten seconds. The decision rides on the `agent.event` frame as
  `notify`; the Controller decides whether the user already saw the pane and
  delivers it. Failures, expiries and cancellations never notify, and every
  settled turn is recorded in the worker's bounded activity log
  (`agent.activity_log`) as `completed`, `failed:<reason>`, `expired` or
  `cancelled`.
- **Capabilities, not probes.** `worker.hello` answers a `WorkerIdentity`
  carrying the set in `protocol/host-capabilities-v1.json`. A Controller reads
  that set once and never discovers a feature by trying it.
- **Restart recommendation.** A session whose core reports a
  `host_protocol_version` below the version this worker requires carries a
  `restart_recommended` token instead of failing. `host_build_id` travels
  beside it for diagnosis only; no restart path reads it.
- **Identity before any signal.** The health refresh marks a session stopped
  only when its recorded child pid with a matching kernel start time is
  absent. An unknown or recycled pid stays `non_resumable` and is never
  signaled.
- **One front door.** `fleet.list` and `surface.status` are answered from the
  worker's reduced state; every other `session.*`, `surface.*`, `host.*` and
  `system.*` method is forwarded verbatim to the core.

## Local host and durable session identity

`paneflow-host` is a GPU-free crate (library plus `paneflow-host` executable)
that owns terminal execution independently of a GPUI entity: the PTY pair, the
child process handle, the canonical `libghostty` terminal, an 8 MiB output
tail with monotonic byte offsets and the session manifests. Nothing in it
links GPUI. The desktop no longer spawns a PTY of its own: every terminal view
resolves a hosted session and attaches to it (see "Attachment and the client
mirror" below). The in-process runtime in `ghostty_session.rs` remains only
for the perf bench and unit tests.

### Host lifetime

One host owns one state home. The desktop (`host_bootstrap.rs`, on a
background thread) and `paneflow host start` share
`paneflow_host::bootstrap::ensure_host_running`: take
`<home>/host/bootstrap.lock`, `host.hello` the endpoint, adopt a compatible
host that serves the same home, otherwise spawn `paneflow-host --home <home>
serve` and wait up to ten seconds for it to answer. The host holds
`<home>/host/owner.lock` for its lifetime, so a second `serve` on the same
home exits instead of becoming a second owner. The executable is resolved
next to the controller binary (`paneflow-host[.exe]` beside `paneflow[.exe]`),
never from the per-user cache and never from the embedded helper bundle: it
links libghostty and stays outside the three capped helpers.

Every shipping artifact therefore has to carry the host beside the desktop, or
the installed app opens dead panes with no way to recover: `Contents/MacOS/` in
the `.app`, `paneflow.app/bin/` in the tar.gz, `/usr/bin/` in the `.deb` and
`.rpm`, `usr/bin/` in the AppImage, and the install root beside `paneflow.exe`
in the MSI. On macOS the host is a second Mach-O inside a signed bundle, so
`scripts/sign-macos.sh` signs `Contents/MacOS` inside-out before sealing the
bundle; an unsigned helper there fails `codesign --verify --deep --strict` and
will not execute on Apple Silicon at all.

The spawn is detached on every platform. Windows uses
`CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`, so
the host leaves the desktop's kill-on-close Job Object and gets no console, and
the spawn goes through `CreateProcessW` with an explicit
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` because `std::process::Command` always
passes `bInheritHandles=TRUE` on Windows: without the list the host inherits
every inheritable handle of the controller, keeps a shell pipeline such as
`paneflow host start | jq` open forever and pins the desktop's log files;
Unix calls `setsid()` in the child before exec, so the host has no controlling
terminal. Stdin and stdout are null and stderr appends to
`<home>/host/host.log`. A denied breakaway (`ERROR_ACCESS_DENIED`), a missing
executable, an early exit or a startup timeout is reported as a bounded error;
nothing falls back to a desktop-owned PTY. Breakaway only leaves the
controller's immediate job: a controller started under `cargo run` or
`cargo test` sits in Cargo's kill-on-close job and so does its host, which is
why `scripts/dev.ps1` and `scripts/dev.sh` build both binaries and run
`target/<profile>/paneflow` directly.

| Event | Host | Sessions | Records |
|---|---|---|---|
| Quit keeping sessions (the default of the quit dialog), crash or `taskkill /F` | keeps running | keep running | unchanged |
| Quit with no live session, or "Stop everything and quit" | `host.shutdown` once the stops are done | stopped one by one | `lifecycle: exited` |
| Controller pipe or CLI connection closes | keeps running | keep running | unchanged |
| Explicit `session.stop` | keeps running | that session's owned process tree is terminated within a 5 s budget, exit recorded | `lifecycle: exited` |
| `paneflow host stop` with live sessions | refused, lists them | untouched | unchanged |
| `paneflow host stop` when idle | exits, removes `instance.json` | none live | manifests kept |
| Host process death or reboot | gone | processes gone | next host marks running records `lost`, never signals them |
| `session.restart` on an exited or lost record | same host | new generation, recorded shell with no arguments, no input or command replay | `generation + 1`, current owner |
| Incompatible host on the endpoint | left running | untouched | unchanged; the controller reports the mismatch |

Reconnection classification lives in `SessionSummary::reconnection`: `live`,
`starting`, `exited`, `failed`, `host_replaced` (the record's owner is not the
current instance) or `lost`. `paneflow host status` and `paneflow-host session
inspect` print it. Development recovery commands, all scoped by
`PANEFLOW_HOME`:

```bash
paneflow host start                       # start or adopt the host for this home
paneflow host status                      # identity, live count, per-session reconnection state
paneflow host stop                        # refused while sessions live
paneflow-host session list|create|inspect|stop|restart
paneflow-host --home <dir> serve          # foreground host for an isolated home
```

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

### Attachment and the client mirror

`src-app/src/terminal/host_link.rs` is the desktop's only entry to the host.
`TerminalView::open` (new terminal), `attach_restored` (saved layout) and
`attach_existing` (hidden session or explicit restart) each build an
`AttachRequest` with a `SessionIntent`: `Create` may create, `Reattach` never
creates and reports a missing or ended record as an explicit ended state,
`Resume` attaches when live, restarts when ended and creates when missing.
`host_link::resolve` runs on the background executor: `session.inspect`, then
create or restart as the intent allows, then `session.attach`, which returns
the native libghostty snapshot and its output offset captured in one runtime
operation together with the host instance token and the session generation.
`ERR_CHECKPOINT_TOO_LARGE` and an engine identity mismatch surface as an attach
failure that leaves the running session untouched.

The view keeps one `GhosttySession` runtime per attachment
(`start_attached`, thread `paneflow-ghostty-attached`). It decodes the
checkpoint through the pinned `SnapshotDecoder` with continuation retention
enabled before any byte is fed, so a checkpoint cut inside a UTF-8 or control
sequence resumes parsing correctly. The decoded terminal is the client
mirror: it renders, selects, searches and scrolls locally, and it encodes key,
mouse, focus and paste input into bytes that go to the host through
`session.input`. The mirror has no PTY writer: any reply the mirror's parser
would emit to a terminal query is dropped, because the host's canonical
terminal is the only responder. Clipboard, title, cwd and notification events
still reach the desktop live; they are never replayed from a restored
snapshot.

A follower thread (`paneflow-ghostty-follower`) streams `session.output` from
the checkpoint offset with `follow=true`. The host emits a keepalive frame
every two seconds while idle; the follower skips overlapping bytes, treats a
gap or `ERR_OUTPUT_EVICTED` as a request for a fresh checkpoint
(`RuntimeMessage::RestoreCheckpoint` replaces the mirror), reconnects every
500 ms after a connection loss, and verifies the host instance token on every
reconnect so a replaced host is reported as `host_replaced` instead of being
followed. Frames from a previous generation or attach attempt are discarded
by the runtime's stop flag. When the stream ends with `live: false`, the
follower classifies the end through `session.inspect` (exited with code or
signal, failed, lost, restarted, missing).

`TerminalState.host_link` carries `HostLinkState`: `Attaching`, `Attached`,
`Reconnecting`, `Ended(HostLinkEnd)` or `Unavailable`. Input is accepted only
while attaching or attached; in every other state the input dispatcher
rejects it and pending input is cleared, never queued for a later resend. A
`session.input` call that fails after a disconnect is reported as rejected
and not retried. `render_host_link_overlay` draws the reconnecting, ended and
unavailable states over the last rendered frame; Enter on an ended or
unavailable terminal runs `resume_hosted_session`, which re-resolves the same
`SessionId` with the `Resume` intent.

### Close versus hide

Dropping a `TerminalState` only shuts the local runtime down; nothing in a
`Drop` implementation calls `session.stop`, so a crash or `taskkill /F` leaves
every session running on the host. Every close path resolves one shared policy
in `app/close_policy.rs` before anything is removed: a `CloseTarget` names what
the action closes, `session_close_decision` answers `Stop` for a session with
no agent or a finished or errored one, `Ask` for a thinking or waiting agent,
and `Unknown` when that session's host link is unavailable. `Ask` opens one
dialog for the whole action; `Keep running` (Enter, the default) removes the
views with the `Detach` intent and leaves the sessions listed, `Stop` removes
them with the `Stop` intent, Escape cancels and nothing is removed. Every stop,
from a close path or from the quit dialog, goes through the single call site
`host_link::stop_session`, which a guard test in `terminal/host_link.rs`
enforces.

| Action | Sessions |
|---|---|
| Close pane (shortcut, pane menu, detached window shortcut), close surface tab, close diff dock terminal | stopped and its record removed, or asked for when an agent is thinking or waiting |
| Close tab, close workspace | every contained session stopped and its record removed, one dialog for the whole action |
| Child exit | the surface goes and its record with it; the last surface closes its pane through the same stop path |
| Hide pane from layout (`hide_pane` action, pane menu) | kept running, the `Detach` intent skips the stop, never asks |
| Any close while the local host is unreachable | the views are removed, no stop is attempted and a toast says the session state is unknown |
| Return a detached pane to its window | untouched |
| Quit (`Quit` action, main window close, title bar close) with live sessions | asks: "Keep sessions running" (Enter, default) leaves them and the host untouched; "Stop everything and quit" stops each live session, then `host.shutdown`; the `on_quit` setting skips the dialog |
| Quit with no live session | the idle host receives `host.shutdown` and the app exits |

A stop whose outcome is unknown (connection lost mid-request) shows a toast
and is reconciled from the next `session.list`; the desktop never marks
sessions stopped optimistically. The workspace session list is every owned
record of `session.list`, live or ended, whose id no attached view carries;
the sidebar lists them under the workspace's last tab, live rows first and
then ended rows by the most recent lifecycle change. A record the desktop
cannot parse is skipped with a log line and the rest of the list renders; a
failed listing keeps the previous rows and marks them stale. Ended rows are
dimmed, carry no agent lane, and the ones beyond `sidebar_ended_sessions`
(default 5) collapse under one row. Left-click reopens a live row through
`attach_existing` and resumes an ended one; right-click offers Open in layout
and Stop session for a live row, Resume and Remove from list for an ended one.
`Remove from list` calls `session.remove`, which the host refuses with
`ERR_SESSION_LIVE` while the session runs and which otherwise deletes its
manifest. Every stop the desktop issues chains that call, so a close forgets
the session it stopped and leaves no row behind. The row is held back from the
moment the stop is issued until its outcome lands, so a close never flashes
one; a record whose stop failed comes back and is reconciled from the next
listing. An ended row therefore comes from a session nobody stopped: one a
hidden pane left running, or one that outlived the app.
`resume_ended_sessions` brings back every ended restartable pane of a
workspace in layout order, resumability being decided by the host link alone
because a reattach that lands on an ended session never promotes an
attachment, and a window that opens with at least two of them offers it once
through a toast. Worktree teardown asks the host for the live session cwds
first: a worktree that still contains a live session, hidden or not, is kept;
an unreachable host proceeds with the current rules; any other host error
skips the teardown.

The host endpoint is derived from the state home (`\\.\pipe\paneflow-host-<fp>`
on Windows, `<runtime dir>/paneflow-host-<fp>.sock` on Unix), so an isolated
`PANEFLOW_HOME` never shares an endpoint or a record directory with the normal
one. The protocol is JSON-RPC 2.0 lines on that endpoint: `host.hello` must
open every connection and verifies the protocol version and the terminal
engine identity (libghostty source sha and API version) before any effect;
`session.list/create/ensure/inspect/stop/restart`, `session.attach` (native
snapshot checkpoint plus its output offset, captured in one runtime
operation), `session.output` (contiguous bytes from an offset, optionally
followed), `session.input`, `session.resize`, `agent.snapshot` and
`host.shutdown` (refused with the live session list while any session runs)
follow. Control frames
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
