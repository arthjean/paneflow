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
| `paneflow-serve` | `crates/paneflow-serve/` | The per-home worker: owns the layout model and `session.json`, the activity reducer, the session projection and the advertised-capability protocol Controllers speak |
| `paneflow-layout` | `crates/paneflow-layout/` | The GPU-free layout model: workspaces, tabs, split trees, ops with their events and effects, undo records, revisions and id leases, shared by the worker and the window |
| `paneflow-api` | `crates/paneflow-api/` | The method registry: each JSON-RPC method declared once with its owner (window, worker or host), audience, gate, parameter and result schemas and examples; dispatch, `authorize`, `paneflow api`, the CLI's routing and the Lua bindings derive from it |
| `paneflow-script` | `crates/paneflow-script/` | The Lua 5.4 runtime (mlua, vendored): the `paneflow` table, events, timers, `paneflow.exec`, prompts and limits, on one thread; `paneflow do` runs it in the CLI process and the worker runs it for `init.lua` |
| `paneflow-launch` | `crates/paneflow-launch/` | How a terminal session starts: the shell, its integration, its environment and the request sent to the host, shared by the window and the worker |
| `paneflow-peer` | `crates/paneflow-peer/` | The process at the other end of an IPC connection, read from the kernel, and whether it runs inside a pane (pane leaders, Windows pane jobs, parent walks) |
| `paneflow-shim` | `crates/paneflow-shim/` | PATH shim wrapping every agent CLI of the `runtimes/` catalog so Paneflow can observe its lifecycle |
| `paneflow-ai-hook` | `crates/paneflow-ai-hook/` | The hook binary agent CLIs invoke to report session events back over IPC |
| `paneflow-ipc-client` | `crates/paneflow-ipc-client/` | Blocking JSON-RPC client for the local IPC socket (shared by the MCP bridge and the CLI) |
| `paneflow-mcp` | `crates/paneflow-mcp/` | Stdio MCP server exposing pane reads (`list_panes`, `read_pane`, `search_pane`) and `write_pane`, a relay to the host's human-approved `pane.write` |
| `paneflow-mcp-install` | `crates/paneflow-mcp-install/` | GPU-free install engine behind `paneflow integrations` and `paneflow mcp`: hooks, MCP entry and conductor skill per the catalog `[integration]` section, idempotent merge written through symlinks, refuses entries another Paneflow home owns without `--force` |
| `paneflow-process` | `crates/paneflow-process/` | Bounded external-process execution (wall-clock deadline + stdout cap) shared across crates |
| `paneflow-acp` | `crates/paneflow-acp/` | Legacy Claude/Codex identity enum plus the `CLAUDECODE` environment scrub |
| `paneflow-colrv1` | `crates/paneflow-colrv1/` | COLRv1 color-glyph rasterizer (skrifa paint graph onto tiny-skia, gradients interpolated in linear light) called by the vendored `vendor/gpui_wgpu` Linux text system, whose swash backend reads only COLRv0 and CBDT emoji |
| `paneflow-highlights` | `crates/paneflow-highlights/` | Pierre's 152 lexers (`@pierre/highlights`) translated to C by wasm2c, compiled by `cc` with bounds-checked memory and no signal handlers, behind a safe live-tokenizer API with byte ranges; layer 1 of the editor's highlighting |
| `paneflow-lsp` | `crates/paneflow-lsp/` | Minimal Language Server Protocol client for semantic tokens: `Content-Length` framing with a 64 MiB message cap, one server per (server id, Files dock root) on background threads, one automatic restart, `shutdown` then `exit` then a kill after 5 s, incremental document sync in the negotiated `utf-8` or `utf-16` encoding, semantic token requests (`full` after `didOpen`, `full/delta` from the last `resultId`, `range` for the visible rows on a range-only server, one request in flight per document plus one follow-up, every open document again on `workspace/semanticTokens/refresh`, `$/cancelRequest` on close) decoded off the render thread into per-line byte ranges and never delivered for an older document revision, command resolution from `PATH` or an absolute path only, and the per-root consent store `~/.paneflow/language-server-trust.json` |
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
A detached window's bounds belong to the window, not to the layout: they are
saved in `window-state.json` keyed by the pane's layout id, beside each tab's
dock, and `session.json` never carries them. A placement whose pane id is no
longer in the layout is dropped silently at start; a placement read from a
`session.json` of an earlier release restores once by its pane id, or by
position when the saved leaf count still matches its tab.
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
        │ Backend events     │ async channel      │ channel
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
  pipe (Windows). On Unix it sleeps in `poll` on the listener until a peer
  connects, waking every 5 s to check that the socket file is still its own;
  the Windows listener blocks in `accept`. Stateless methods reply in place;
  stateful methods go to the main thread through a bounded async channel, and
  `serve_ipc_requests` wakes on the first request and drains the queued batch.
- **Layout client threads**: `paneflow-layout-client` sends the window's
  layout ops, syncs and lease requests to the worker in order over one
  connection and returns each answer through a channel the main thread drains
  on its wake; `paneflow-layout-changes` follows the worker's layout events.
  The main thread never makes a socket call for the layout
  (`src-app/src/app/layout_bridge/client.rs`, `sync.rs`).
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
runs block and inline passes; fenced code does not inject another language
in layer 2, and the editor colors it through layer 1 instead.

Captures are ordered by byte start, preserving query order at equal starts.
Each styled capture goes onto a stack. The last active capture paints until
its end or the next capture, including when it is wider than an earlier one.
Captures without a palette role do not enter the stack. Each row still caps
input at 4,096 captures. Theme changes rebuild color tables without querying
or reparsing the retained trees. Variables and namespaces use the editor's
text color; constructors share the function role.

The Changes dock builds in two passes on a background thread
(`app/diff_dock/git.rs`). The first pass reads every base text through two
`git cat-file` processes (`--batch-check` for the sizes, `--batch` for the
blobs of 512 KiB or less) instead of one `git show` per file, builds the rows,
and takes syntax runs only from the store in `diff/row_syntax.rs`. When more
than 256 KiB of shown text is missing from the store, it delivers the rows
uncolored. The second pass highlights, in parallel, the files that fit under
the 10,000-row cap and swaps the runs in on the same rows. The store is keyed
by extension and text digest, holds 16 MiB of source text, evicts the least
recently used side, and empties when the syntax palette changes, so a refresh
highlights only the sides whose text changed. A path that contains a newline,
or a batch git refuses, falls back to `git show`.

`diff/queries/MANIFEST.toml` records the upstream commit, source paths, SHA-256
hashes, license evidence and the grammar deviations. `NOTICE` attributes Zed
Industries. Git preserves LF bytes for these imports.

The queries have two sources. Fifteen come from the Zed repository
(GPL-3.0-or-later), described by the manifest's top-level `repository`,
`commit`, `license` and `license_evidence`. The Zig query, which also colors
`.zon` files, comes from the Zed Zig extension,
`https://github.com/zed-extensions/zig` (Apache-2.0, text in
`diff/queries/zig/LICENSE`): its `[[queries]]` entry carries its own
`repository`, `commit`, `license` and `license_evidence`, which override the
top-level values for that entry only. The `tree-sitter-zig` crate ships a query
written for Neovim, whose `#lua-match?` predicates the Rust runtime never
evaluates, so it is not used.

Set `ZED_DIR` to a local Zed checkout and `ZED_ZIG_DIR` to a local
zed-extensions/zig checkout, then run
`scripts/sync-zed-queries.sh --check` on Linux/macOS, or
`scripts/sync-zed-queries.ps1 -Check` in PowerShell 7 on Windows. Check mode
compares every query, the Zig license and the manifest byte for byte with the
pinned Git objects, checks the provenance notice and lists any drift. Omit the
check option to restore those bytes, including the notice's revisions. Either
variable missing stops the script before it writes anything.
To resync, pass `--commit <revision>` or `-Commit <revision>` for Zed, and
`--zig-commit <revision>` or `-ZigCommit <revision>` for the extension, then
review the upstream license evidence, compile the queries, run the parity tests
and the editor benchmark. Both scripts resolve revisions to an immutable full
SHA. No source checkout files or runtime configuration are read by the app.

Two tests in `diff/query_guard.rs` run without a checkout or the network. One
hashes every vendored query against the manifest and fails on a drifted byte or
on a `highlights.scm` the manifest does not list. The other fails when a
shipped query, vendored or taken from a grammar crate, carries a predicate the
tree-sitter runtime does not evaluate, since such a predicate never filters and
the query colors wrongly without an error. Its one named exception is the
`#has-ancestor?` of Zed's C++ query, which Zed ignores as well.

### Layer 1: the native lexers

`crates/paneflow-highlights` vendors the wasm2c translation of
`@pierre/highlights` 0.5.2, the wasm2c runtime of WABT 1.0.42 and SIMDe's
amalgamated `wasm/simd128.h` under `c/`, with their licenses and `c/NOTICE`.
`scripts/sync-pierre-highlights.sh` (or `.ps1`) writes every one of those files
and `src/tables.rs` from inputs pinned in `c/MANIFEST.toml`: the npm tarball is
checked against its `sha512` integrity, the WASM, the WABT archive, the SIMDe
archive and both license texts against SHA-256 values, and a `WASM2C` binary
must report 1.0.42. `--check` (`-Check`) regenerates into a scratch directory
and names every file that differs or is missing; nothing is written until every
input verified. A crate test rehashes the vendored files against the manifest
without the network. Token and language ids are an ABI that moves between
Pierre releases, so `tables.rs` is only ever generated, and a test reads the
WASM's own token-name table from the lexer's memory and fails when the count or
a name differs.

`build.rs` compiles the C with `-std=c11` (`/std:c11` on MSVC),
`WASM_RT_USE_MMAP=0`, `WASM_RT_MEMCHECK_BOUNDS_CHECK=1` and
`WASM_RT_SKIP_SIGNAL_RECOVERY=1`, so memory is plain `malloc`, every access is
bounds-checked and no signal handler is installed, and it refuses any target
outside the four shipping ones. Each `Lexer` owns one heap instance. The C shim
(`c/shim/`) calls `WASM_RT_SETJMP` in every entry point that runs lexer code, so
a trap returns `LexError::Trap` and poisons that instance instead of unwinding
through Rust; the two host imports are Rust `extern "C"` functions reached
through function pointers, which never call back into the shim, so a `longjmp`
never crosses a Rust frame. Each instance's linear memory is capped at
`MAX_MEMORY_BYTES` (8 x the 10 MiB editor cap) through its `max_pages`, and a
staging that would pass it is refused before memory grows. `LexDocument`
takes edits as document byte ranges, converts them to the lexer's UTF-16
columns and returns tokens as byte ranges. Its lines follow the lexer, which
also breaks on a lone `\r`, unlike the editor's ropey (`\n` only).

### How the editor combines the layers

`code/lexical.rs` owns layer 1 for one open document. Its lexer scans forward
from the first line whose state is unknown, and `LexDocument::run_before`
bounds a scan to the lines before a given one, so layer 1 lexes only what the
next frame needs. `open_blocking`, which runs off the render thread, stages a
file at or under its tree-sitter cap into a lexer and scans its first
`INITIAL_LEXICAL_ROWS`, so the first frame is colored. A larger file, up to
the 10 MiB editor cap, opens plain and is lexed whole by a background job the
view starts right after the open. The layer keeps each row's runs packed as
`u32` words (24-bit end, 8-bit token id), with blank tokens folded into their
neighbor, and maps lexer lines to editor rows through byte offsets when a
lone `\r` makes them differ. A theme switch rebuilds only the 75-entry token
color table.

`CodeHighlighter::runs_into` paints, per row, layer 2 when the row is `Fresh`
and layer 1 otherwise. Inside a nested-code region of a fresh layer-2 row,
Markdown `code_fence_content` and HTML `raw_text`
(`NESTED_CODE_REGIONS`), layer 1 shows through where it has a color. The
regions are stored in the row's run list under a reserved pass index, so the
existing interpolation and requery keep them in step with the rows.

The rest of the document is scanned in three places, all on the render
thread and all bounded. Every edit reaches layer 1 through `edit_batch`: the
row runs are shifted like layer 2's, the edits are staged into the lexer, and
the scan covers the edited rows plus `EDIT_TAIL_ROWS` within
`SYNC_PARSE_BUDGET`. Each frame's `fill_stale_rows` then scans up to the last
shown row within the frame budget, so a row is rescanned before it is painted.
Whatever is left, such as the rest of a file an unterminated `/*` comments
out, is scanned by the view's idle task (`start_lexical`), which yields to the
event loop between slices of at most `IDLE_SLICE_BUDGET` and asks for a frame
only when a slice rewrote a shown row (text area or minimap). A scan slice
reads back exactly the lines it lexed, bounded by its end line; rows not yet
reached keep their shifted runs. On any lexer error the document is dropped
and a background job relexes the current text from scratch; a job that an
edit overtakes stops early and is followed by one for the new text. `edit_batch` expects the document to be exactly the state after the
edits it receives, so undo and redo report each record's replacement as the
history applies it (`UndoHistory::undo_with`, `redo_with`). A lexer error at
open or in a job turns layer 1 off for that document with one warning; layer
2 is unaffected. When layer 2 gives up on a parse, layer 1 keeps coloring and
the "too complex" banner stays hidden.

The diff view uses layer 1 only for a side whose extension has no grammar
(`diff/lexical.rs`), under the same `highlight_cap`; the side store's key
carries the lexer language, so a `Dockerfile` and a `Makefile` with the same
text are kept apart.

Layer 3 is the semantic tokens of a language server (`code/semantic.rs`),
present only while a document is attached to a running server. The client
thread decodes each answer and delivers it through `LanguageServerHub`, whose
foreground task hands it to the view; the view installs it only when its
revision equals the count of edits it has sent, so an answer computed for older
text is never painted. Each token type maps to a capture name resolved by
`DiffSyntax::color_for_capture` (TD-09 of the layered highlighting PRD), and a
type with no mapping stays transparent, so a theme switch recolors all three
layers alike. `runs_into` overlays the mapped tokens on the layer-2 or layer-1
runs of the row. An edit drops the tokens of the rows it touches and shifts
the rows after it until the next answer replaces the whole set. A server that
leaves the `Running` state, a detached document and a turned-off
`editor.semantic_tokens` all drop the layer; with the setting off it is never
allocated. The editor's banner area reports a `Missing` server once per server
and root in a session, and a `Crashed` (last stderr line) or `Failed`
(`initialize` error) server with a Restart button.

## Files tree

The right Files rail has its own `FilesSidebar` GPUI entity, mounted with
the view cache. `PaneFlowApp` owns its placement, workspace association and
editor integration; row hover and keyboard selection update the rail entity.
The implementation is under `src-app/src/app/files_sidebar/`.

Four parts have separate lifetimes, following Zed's project panel model:

- `worker.rs` owns the directory listings and nonrecursive filesystem watches
  on a dedicated thread. It registers each watch before reading the directory,
  loads newly expanded directories, and refreshes invalidated listings. On
  macOS one recursive FSEvents watch on the root covers every loaded
  directory instead, because `notify` stops and recreates the FSEvents stream
  on each watch or unwatch, which made Expand all over 2,000 folders take
  about 30 s; the worker drops events outside the loaded directories so the
  subtree's extra events never re-read a listing or re-run git. Loaded
  collapsed directories remain searchable and watched. Deleted or ignored
  subtrees are pruned. Watch events are coalesced in the worker; unavailable
  watches fall back to background polling and registration retries. Each
  update carries only the folders that changed: a listing is an
  `Arc<[FileNode]>` shared with the panel, never copied, and a re-read listing
  comes with the positions of its removed and inserted entries, computed by a
  merge walk over the two sorted listings. A folder that did not change is not
  sent.
- `model.rs` holds the panel's `TreeModel`: for every loaded folder, the
  number of visible rows under each entry (1 for a file or a collapsed folder,
  1 plus the folder's own total when it is expanded) in a Fenwick tree, so a
  prefix sum, a point update and a row lookup each cost O(log n). Expanding or
  collapsing a folder adds its total to each expanded ancestor; a listing diff
  carries the counts of unchanged entries over and computes only the inserted
  ones. Rows are never flattened: `rows(range)` descends to the first row of
  the range, then walks the listings for as many rows as the range asks.
  Path-to-row lookup binary-searches each ancestor listing, which is sorted by
  `files_tree::compare_nodes`, a total order (folders first, ASCII
  case-insensitive name, then the exact path).
- `projection.rs` turns a model row into a `FileRow` (label, icon, git status,
  element ids) only when the list asks for it, and builds the search rows,
  which are bounded by the 200 results, on GPUI's background executor. Query
  changes replace the pending search task; epoch and revision checks discard
  results from an earlier root or query.
- `panel.rs` owns the model, path-based selection, input focus and
  uniform-list scroll handle. `view.rs` renders only the requested row range.
  Hover uses the immediate squircle state and does not rebuild any row.
  Selection survives insertions; a collapsed selection returns to its visible
  ancestor. Keyboard navigation reveals the selected row.

Closing or switching workspaces cancels the worker and pending publications.
The closing animation keeps its last model until it finishes, then releases
the model on a background executor. The rail keeps Paneflow's 300 px width,
28 px rows, icons, indentation and full-width hover inside the 8 px edge insets.

## Keystroke → pixel

Windows PTYs are opened through `paneflow_host::pty::open`, shared by the host
and the app's test-only in-process runtime. It loads a pinned, embedded
Microsoft ConPTY runtime before `portable-pty` opens the PTY. Modern OpenConsole
preserves the ordering of synchronized-output markers and cursor updates; the
older system ConPTY renderer can emit the end marker before the final cursor position.
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
platform frame scheduler, under the frame policy that `frame_policy.rs` states
for every Paneflow window (`paced_window_options`): an unfocused window
presents at most 30 frames per second (33.333 ms), a window under the
platform's low-power mode at most 30 (33.333 ms), and one under serious or
critical thermal pressure at most 60 (16.667 ms). Since `703b5ac3` the
limit holds for every redraw, a notify or a keystroke echo included, where
GPUI used to limit only forced renders and next-frame callbacks (#66237).
GPUI applies the longest interval that holds and lifts the unfocused cap
during sustained high-rate input; a mandatory platform presentation bypasses all three. The values equal
GPUI's defaults at `703b5ac3` and are written out so that a later GPUI default
cannot change them silently: `every_paneflow_window_is_opened_with_the_paced_options`
fails when a new `open_window` call skips them.

Two gates decide how much of that pipeline actually runs, and both exist
because the natural rate of each stage is far above the rate a display can
show.

**`PublishGate` (`terminal/ghostty_session/publish.rs`), on the runtime thread.**
Snapshotting the grid and converting it into the neutral `Content` is the
expensive half of an output batch, and `OUTPUT_BATCH_MAX_TIME` closes a batch
every millisecond. The gate holds a publication back for two reasons: a render
hold is open, meaning the program is mid-redraw under DEC mode 2026 and the
frame would tear, or the last frame is newer than `MIN_PUBLISH_INTERVAL`. The
gate never polls the mode. The terminal installs libghostty's render hold
effect (`DisplayTerminal::enable_render_hold`), and when a program opens a
synchronized update the effect captures the finished frame into the main
render state before any of the new redraw lands
(`crates/paneflow-terminal-ghostty/src/callbacks.rs`, `begin_render_hold`).
Every ordinary publication during the hold shows that captured frame. A
publication that must show live content during the hold (a scroll, a
selection, a resize) is built from a second render state of its own
(`snapshot_live`, `RenderSlot::LiveDuringHold`), so the captured frame and its
scrollbar survive it and the next held publication shows the finished frame
again, never a half-drawn one. That second state is released once the hold
ends. A checkpoint restored with mode 2026 set has no captured frame, so
attaching resets the mode and the program's next synchronized update opens a
hold of its own. A held change is deferred, never
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
A render hold
expires after `SYNC_OUTPUT_MAX_HOLD` (150 ms, measured from the capture): the
gate releases the hold and publishes live content, so a program that opens a
frame and dies cannot freeze the pane. Resizes, scrolls, other state the user waits on, and
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

`TerminalElement::paint` draws the grid in two sibling `paint_layer` calls,
one for the base fill, overscan row, cell backgrounds, selection, search
highlights and block quads, one for sprites, text and symbols, with kitty
images of negative z-index between them. GPUI gives a layer a single draw
order and draws its primitives by kind (shadows, quads, paths, underlines,
monochrome, subpixel and polychrome sprites, surfaces), then by atlas texture,
and only then in paint order (`PrimitiveKind` in `gpui/src/scene.rs`). That
is safe here because the primitives of one layer sit in distinct cells and do
not overlap. Outside a layer, overlapping primitives keep their paint order,
and a layer opened inside another one would draw above everything its parent
paints, even what the parent paints after it; the terminal nests none.
Decorations, kitty images of non-negative z-index, the hyperlink underline,
the cursors, the scrollbar, search ticks, IME preedit and the exit overlay are
painted after the second layer, in that order.
`a_terminal_frame_paints_backgrounds_then_glyphs_in_two_layers_and_overlays_above_them`
in `terminal/view.rs` asserts the sequence of the paint phases.

Rounded corners, diagonals, Powerline separators and curly underlines are
curves, which GPUI `703b5ac3` would stroke and bin into tiles on every paint.
The terminal draws them as Ghostty draws its sprite font instead: each shape is
an SVG outline built once per cell size (`MaskKey` in
`terminal/element/paint/masks.rs`), rasterized once into GPUI's monochrome
sprite atlas through `Window::paint_svg`, and painted as a sprite in the color
of its cell. A curly underline's mask covers only the band of its wave, so it
never overlaps the row below. Straight box-drawing lines and blocks stay
quads.

The workspace sidebar is its own cached view, `SidebarView`
(`src-app/src/app/sidebar/view.rs`). It observes the app entity and calls
`PaneFlowApp::render_sidebar`, so a frame caused by a `TerminalView` alone
replays the sidebar instead of rebuilding it; the app notifies whenever state
the sidebar shows changes. The sidebar's spinner and shimmer clock
(`SpinnerClock`) lives in that view and notifies only it.
`sidebar_renders` in `system.counters` counts the rebuilds.
`SidebarView::faded_column` puts the cached view under the column whose
opacity follows the collapse; GPUI keys the view cache on the inherited
opacity, so the sidebar repaints in the frame that changes it.

A cached view that reads an entity it does not observe still wakes the window
when that entity notifies, including after a frame that drew the view from
its cache, but it keeps its cached output until it is rebuilt: a view observes
what it shows. `redraw_guard::assert_no_redraw_loop` (test-only) draws 60
frames and names any watched view that rebuilt on every one of them, so an
entity notifying each frame under a cached view fails its test instead of
pinning the UI thread.

A view drawn from the cache does not re-emit its accessibility nodes at GPUI
`703b5ac3`: the cache replays prepaint and paint ranges (`reuse_prepaint`,
`reuse_paint`) and the accessibility tree is rebuilt only from elements that
run, so the roles of a cached `SidebarView` or `TerminalView` leave the tree on
frames that reuse it and come back when it rebuilds. This is upstream
zed-industries/zed#66690, open. GPUI's test platform has no accessibility
adapter, so no unit test can observe it; on Windows the sidebar was seen
dropping from 60 to 12 accessibility references once it stopped re-rendering,
and on macOS the sidebar left the tree while the New pane palette was open.
Production code caches a view only through `ui_primitives::view_cache`,
which caches it as before while no assistive technology is connected and
renders it uncached while `Window::is_a11y_active` holds, so every frame
carries its nodes. The rebuild the cache avoids is paid only while an
accessibility client has activated AccessKit (VoiceOver, an automation tool,
some launchers and window managers). A guard test,
`production_code_caches_views_only_through_view_cache`, fails on a direct
`.cached(` outside test code.

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
       │                                                         └─ agent.event
       ├─ its own OSC 9;4, OSC 9/777 + OSC 7501 in the pane's grid   to the
       └─ the agent's own session registry on disk                   host
                                                                      │
            ┌─────────────────────────────────────────────────────────┘
            └─ one write choke point, ordered by source
                 └─ GUI: tab dots, sidebar spinners, space attention mark,
                    desktop notifications carrying the actual question
```

- **Shim**: launching an agent from Paneflow puts a shim directory first in
  `PATH`. The shim records the real PID and process start time (PID-reuse
  safe), then execs the real binary. Sixteen agent CLIs are recognized by
  name; unknown tools are reported as themselves. It emits `session_start`,
  `exit` and `session_end` on its own, so presence and exit never depend on
  the agent cooperating. Each shim hands its child `PANEFLOW_SHIM_CHAIN` (depth,
  agent launches, launch time, argument fingerprint, pane session, tool) and
  `PANEFLOW_SHIM_CHAIN_TARGETS` (the binaries already resolved for that tool);
  a chain from another pane session is ignored. A shim started again for the
  same tool with the same arguments within ten seconds was sent back by a
  third-party launcher that looked the tool up on `PATH`: it skips those
  targets, runs the next real binary, or exits 127 naming the launcher. A tool
  that runs itself with other arguments is ordinary nesting, and eight nested
  shims is the hard cap whatever the arguments. Only the outermost shim of a
  pane reports the lifecycle: a shim that inherits a chain stays silent, so an
  agent that runs another agent CLI keeps the pane's identity. A shim that
  refuses to launch declares `state=error` under the OSC 7501 record
  `paneflow-shim` when its stderr is the pane's terminal and no agent runs
  above it, so a launch that failed in a pane nobody watches still raises
  attention.
- **Hooks**: Claude Code and Codex, once their integration is installed,
  report `session_start`, `prompt_submit`, `tool_use`, `notification`, `stop`,
  `exit`, and `session_end` as `agent.event` frames on the host endpoint; the
  worker projects them and the app rebroadcasts them as `ai.*` events. The
  app socket itself answers an `ai.*` call with `-32601`. Hooks name the agent
  and carry its metadata: the provider session id and transcript path that
  make a conversation resumable, the submitted prompt that names the tab, the
  active sub-tool, the waiting message and the last result. They never move
  the turn state.
- **OSC 7501 is the only source of turn state.** The worker derives a
  session's state from the program status the agent declares (below) and from
  nothing else: no hook latch, no screen reading, no keystroke heuristic, no
  terminal bell. An agent that declares nothing shows no turn state.
- **Where the reduction happens**: in the worker, never in the core and never
  in a Controller. The core validates a hook frame against the session's
  runtime generation and records the raw `last_hook` on the manifest, and its
  viewport scan publishes the declared status; the worker
  (`crates/paneflow-serve/src/state.rs`) maps the declaration of a recognized
  agent to a state and publishes it with an `activity_source` of `declared` or
  `none`. The GPUI app subscribes and renders; it computes no lifecycle of its
  own.
- **States**: thinking (`working`), waiting for input (`blocked`, with the
  declared message), finished (`idle` or `done`), errored (`error`). Each state
  routes to the UI - and to your own tooling, since the same events are
  observable over IPC.
- **Declared program status (OSC 7501).** Any program, agent or not, can
  declare its own state with `ESC ] 7501 ; state=<working|blocked|idle|done|error|clear> ...`
  following the program status spec
  (https://www.superlogical.com/rex/docs/build/program-status, as read on
  2026-10-09). The host's terminal engine answers `OSC 7501 ; ?` and keeps up
  to 256 records per session (`crates/paneflow-host/src/program_status.rs`);
  the desktop installs no OSC 7501 callback, so the PTY never gets a second
  reply (`desktop_terminals_neither_answer_nor_report_program_status`). A
  record follows the spec's lifetime: a prompt start (OSC 133 A) or the
  program's exit releases `working`, `blocked` and `idle` and keeps `done` and
  `error`, which leave on the first keystroke typed in an attached window
  (never on input a control client sends, such as `paneflow send` or
  `write_pane`), on `clear`, on RIS, or on a manual reset. A manual reset is a
  RIS: records, OSC 9;4 progress and title are emptied. Pending status events
  are budgeted at 256 KiB per terminal; a burst past it clears every record
  and the progress instead of keeping a partial state, and logs a warning.
- **OSC 9;4 progress goes stale like Ghostty's.** The protocol has no owner
  or end, so the pane's progress chip is removed 15 s after the last OSC 9;4
  report (`PROGRESS_STALE_AFTER` in `src-app/src/terminal/pty_session.rs`),
  as in Ghostty, and each report restarts that clock. State 0 removes it at
  once, and so does the child's exit or the end of the host link. A program
  that wants the chip to stay sends an update at least every few seconds.
- **OSC 7501 capability.** Each host runtime carries one more flag,
  `program_status_capable`, that says whether the program in the pane speaks
  the protocol at all. It turns true when the engine's reply to
  `OSC 7501 ; ?` passes through `drain_engine_events` as a `WritePty` that
  starts with `ESC ] 7501 ; ?` (libghostty is not patched to report it), or
  when a record is applied. It survives the records being emptied (`clear`,
  RIS, a manual reset, a burst past the budget) and drops back to false at a
  prompt start (OSC 133 A) and with every new runtime generation, so each new
  agent process starts uncapable until its own query or first record. The
  viewport scan edge-writes it on the manifest (omitted when false), the
  worker carries it on each snapshot entry (`to_value` in
  `crates/paneflow-serve/src/state.rs`, read as false when absent), and the
  app exposes it as `AgentSession::program_status_capable`. The flag changes
  no state: the worker derives the turn state exactly as before. The app only
  uses it to rank a recognized agent that declares nothing below idle and to
  label it `No status` once it has run 3 s without the capability.
- **Agent ordering.** `src-app/src/app/sidebar/agents.rs` ranks every
  recognized agent session by urgency: waiting for input, errored, finished
  and not yet seen (the unread completion that `acknowledge_visible_completions`
  clears when its tab becomes visible), working, idle, then without the
  capability. Within a rank the most recent `last_event_at_ms`, then
  `state_seq`, comes first, and the workspace then tab order breaks the
  remaining ties. The sidebar's Agents section lists the active space's
  sessions in that order. Jump to Next Waiting (`secondary-shift-j`) cycles the
  first three ranks over every workspace, the active space first, so it works
  whether the section is shown, collapsed, or hidden.

The default loop is human-in-the-loop: Paneflow pre-fills prompts into real PTY
sessions and the user submits them. Auto-submit exists only as an explicit,
gated scripting path.

## IPC and the MCP bridge

A JSON-RPC 2.0 endpoint (Unix socket at `$XDG_RUNTIME_DIR/paneflow/`, named
pipe on Windows; an isolated `PANEFLOW_HOME` owns `paneflow-ipc-<fp>` beside
its host endpoint instead, and a debug build never binds or dials the release
one) exposes the `workspace.*`, `surface.*`, `fleet.*`, and `events.*`
namespaces - enough to script workspace creation, read panes, send text behind
the scripting gate, and subscribe to agent events, which arrive as `ai.*`
event types. The `paneflow`
CLI (`paneflow up`, `paneflow flow`, `paneflow watch`, `paneflow wait`) is
built on the same socket.

Handlers run on the GPUI thread, so none of them waits on a terminal runtime.
`surface.read` and `surface.search` resolve their pane there, then hand a
windowed read or a bounded search to a blocking worker that sends the reply
(`IpcReply::Deferred` in `src-app/src/app/ipc_handler/mod.rs`). Errors use
JSON-RPC `error` envelopes: `-32602` invalid params, `-32601` unknown or
gated method, `-32001` permission, `-32000` busy or shutting down (including a
full terminal runtime queue), `-32002` a timeout (the request did not start
within 5 s, or the pane's terminal runtime did not answer in time), `-32003`
the pane's terminal runtime is gone or failed (a read never reports any of
these as empty text), `-32005` a
close that would discard unsaved files, and `-32800`
a search superseded by a newer search on the same pane. Text budgets count the
JSON-escaped size, so a reply always fits the 256 KiB frame.

The MCP bridge re-exposes a slice of this to agents themselves:
`paneflow mcp install` registers a stdio MCP server with Claude Code, Codex,
Gemini CLI and opencode, giving any agent the ability to read other panes'
scrollback. An agent debugging a failing dev server can read the server pane's
output directly instead of asking you to paste it. Its one write tool,
`write_pane`, only relays to the host's `pane.write`: the host binds each
(source, target) pair of agent occurrences to a human approval granted from the
window over `approval.follow` / `approval.decide`, which only a client that
attaches a terminal engine may call, then sanitizes, rate-limits and prefixes
the text with its provenance (`crates/paneflow-host/src/agent_write.rs`). The
bridge binary ships embedded in the main binary and is extracted to a stable
path at launch, so there is nothing extra to install.

### Caller identity and authorization

The window's socket, the worker's endpoint and the session host each give a
connection one principal before its first request: `Window`, `Worker`,
`Pane { session, workspace }`, `External` or `Unverified`. The peer is read
from the OS (`crates/paneflow-peer`), bound to the process rather than a
reusable PID. The host decides pane membership, since it spawned every pane
and records their leaders; the window and the worker ask it with
`peer.classify`, a query they make for themselves, and fall back to the
session manifests in the home when it does not answer. On Linux and macOS the
fallback walks the caller's parents to a recorded pane leader. On Windows,
where a parent that exits leaves its children without a parent to walk, the
host places each pane's leader in a Job Object named after its session
(`Local\paneflow-pane-<session>`, `paneflow_peer::pane_job_name`), and the
fallback opens each recorded session's job by name and asks the kernel whether
the caller is in it (`IsProcessInJob`); the job grants no breakaway, so a
pane's process stays in it unless a process of the same user rewrites the
job's limits, which is outside what this layer defends. A
session whose manifest may still hold a process but whose job cannot be
opened, such as one started by a host that predates named jobs, leaves the
caller `Unverified`; with no session recorded at all, no pane exists and the
caller is outside every pane. A connection is never assumed to be outside
every pane on an incomplete answer.

Outside every pane the hello names the principal (a `paneflow-serve*` client
is `Worker`, a hello that declares a terminal engine or a `paneflow-desktop*`
client is `Window`, and on the worker `layout.attach` with the client
`window` turns `External` into `Window`), but only for a caller the server
vouches for (`paneflow_host::genuine::GenuineCallers`, built once when the
host or the worker opens). It vouches for the process that started that
server, matched by pid and start time; for a process whose executable,
canonicalized, is the server's own executable or the `paneflow[.exe]` beside
it; and for one whose executable is a byte-identical copy of either, such as
an AppImage mounted again at another path. The executable comes from the peer
read: `proc_pidpath_audittoken` on macOS, `/proc/<pid>/exe` on Linux and
`QueryFullProcessImageNameW` on Windows. Any other caller that claims
`Window` or `Worker` gets `External`, logged once per executable, and is not
refused, so honest tooling keeps working. The window and `paneflow serve` run
the `paneflow` beside the host. A test that embeds the host or the worker is
vouched for as its own executable, one that starts it as its spawner, and one
that embeds the host for a built `paneflow` names that binary with
`SessionHost::vouch_for_executable`, an in-process call no client reaches. The
check authenticates the binary, not the code path: a verb of the genuine CLI
that sends a window hello (`paneflow host stop`, `paneflow host status`,
`paneflow-host session`) still acts as the window. Code injected into a
genuine process by the same user (`LD_PRELOAD`, a debugger) is outside this
layer, and so is a process that starts the host or the worker before Paneflow
does, since it could as well serve a counterfeit endpoint. The host treats
`session.input` as typed (`InputOrigin::Typed`, not journaled) only from a
vouched `Window` or `Worker` that declared an engine.

Every request then goes through one pure function,
`paneflow_api::authorize(principal, descriptor, params, policy)`
(`crates/paneflow-api/src/authorize.rs`), which reads no file, socket or
environment and returns `Allowed { scope, gates }` or `Refused { reason }`.
Each descriptor declares an audience and a scope rule:

| Family | Audience | Scope |
| --- | --- | --- |
| `system.*`, `session.list`, `session.text` and the other reads | `Anyone` | none |
| `surface.list`, `surface.read`, `surface.search` | `Anyone` | the claimed session, if any |
| `layout.attach`, `peer.classify` | `NotPane` | none |
| `layout.launch`, `layout.sweep`, `approval.follow`, `approval.decide`, `session.runtime.bind`, `session.workspace`, `host.shutdown` | `PaneflowOnly` (window and worker) | none |
| `session.input`, `stop`, `restart`, `remove`, `reset`, `clear_history`, `resize`, `appearance` | `Anyone` | the caller's workspace |
| `surface.send_text`, `surface.send_keystroke`, `pane.write` | `Anyone` | the caller's workspace unless `scope: "all"` with orchestration |

The policy comes from the `ipc` section of `paneflow.json`
(`paneflow_host::policy::PolicySource`, re-read when the file's mtime or
size changes) plus `ai_unrestricted`. `Window` and `Worker` pass every gate.
The deprecated `PANEFLOW_IPC_*` variables, read only in
`crates/paneflow-api/src/policy.rs`, still open the gates for `External`, and
for `Pane` only while the section is absent. For a `Pane` caller,
`authorize` rewrites `scope_session` to its own session; the host then
checks the target session's workspace (`scope_refused`). The window runs
`authorize` on the connection thread, before a request reaches the GPUI
thread, and hands the resulting gates to the handler; the worker runs it in
its registry before every handler, and an `init.lua` action acts as the
window when started from a key or the palette and as its caller otherwise.
A closed gate keeps the error each method returned before, so scripts see
no new codes. `crates/paneflow-api/tests/decision_table.txt` pins every
decision for every method and principal, and the host, the worker and the
window each check their dispatch against the same table.

Grants extend the policy per pane without a secret. The worker holds them
(`crates/paneflow-serve/src/pane_grants.rs`, persisted to `serve/grants.json`)
keyed by the holder's session and launch generation, with rights (`write`,
`spawn`, `layout`), a scope (`workspace` or `instance`), an expiry and the
grant they were delegated from. A grant comes from a `grant.request` prompt
through the worker's prompt broker, an `agents.<name>.grants` profile the
worker applies when the host's runtime observation sees that agent in a pane's
foreground (a hook's self-declared `tool` is not enough), or `grant.delegate` toward
a pane the holder split. The worker records the spawner of a `surface.split` it
serves for a pane; for one the window serves, the window names the calling
pane in the `spawned_by` of the `layout.apply` it forwards, a field the worker
takes only from the attached window. The worker drops a grant whose holder ended or
restarted on the next core frame, together with every grant delegated from
it. `authorize_with` takes the caller's grants as a `GrantView`
(`crates/paneflow-api/src/grants.rs`): a live grant of the caller at its
current generation opens the gate of the right the method needs, and an
`instance` grant widens the caller's scope. Only `write` and a `spawn` on
`surface.split` can be held to one workspace: for the latter `pane_gates`
sets `Gates::spawn_confined` and the worker's `surface_split` checks the
anchor's workspace, while every other spawn or layout method needs an
`instance` grant. The store holds at most `MAX_GRANTS` grants so that the
full set fits one host control frame. After each change the worker
writes the file, sends the full set to the host with `host.grants.set`
(`PaneflowOnly`) and appends `grant.changed`; the window follows that event
and reads `grant.list`. The host and the window hold the set in a
`paneflow_host::pane_grants::GrantMirror`, which is stale while the announced
revision or the one in `serve/grants.json` is newer than the one it holds; a
stale mirror applies no grant and refuses what would need one with
`grants_unsynced`. A window process holds one mirror and one follower per
home (`ipc_grants::followed_mirror`), shared by every `PaneFlowApp` it
starts: the follower never ends, so one per app would pile up held
connections until the worker's `MAX_CONNECTIONS` cap refuses other clients.

A sandboxed pane carries its identity in the endpoint it connects to, after
Wayland's `security-context-v1`. An agent profile with `sandbox: "paneflow"`
makes the window put `PANEFLOW_SANDBOX` in the launch env; `host.create`
turns it into `CreateSession.sandbox` (and `sandbox_unix_scope`), strips it,
refuses an unknown value, and persists both in `SessionLaunch`, so a restart
re-sandboxes the pane and the window's `session.json` keeps the request for a
restore. Before the spawn the host opens `panes/<session>/host.sock`
(`crates/paneflow-host/src/pane_endpoint.rs`; a pipe with an SDDL granting the
per-pane SID on Windows) and is the only process that creates that directory;
the worker (`Worker::reconcile_pane_endpoints`) and the window
(`src-app/src/ipc_panes.rs`) open `serve.sock` and `window.sock` there for every
live sandboxed row the host lists, and all three close them when the row goes.
Every connection on such an endpoint gets the fixed principal
`Principal::Pane { session, workspace }` without `peer.classify`.
`paneflow_home` honors `PANEFLOW_PANE_*_ENDPOINT` only for a path under the
home's `panes/<session>/`. `crates/paneflow-host/src/sandbox.rs` wraps the shell:
bubblewrap plus Landlock applied by `paneflow-host sandbox-exec` on Linux
(the runtime directory comes back with only `linux::KEPT_ENTRIES`,
`KEPT_ENTRY_PREFIXES` and the SSH agent, so the session bus and the systemd
sockets, which would start a process outside the sandbox, stay out),
`sandbox-exec` with a generated Seatbelt profile on macOS, and on Windows the
same helper re-launching the shell with a write-restricted token whose
restricting SIDs are the per-pane SID and the shared sandbox SID
`S-1-9-1347-0`, which the host grants on the working directory, `%TEMP%` and
the session data. A wrap that cannot start fails the launch; nothing falls back
to an unsandboxed shell. The config offers the sandbox only on
`SANDBOX_SUPPORTED_PLATFORMS` (Linux today): macOS and Windows join the list
when their hardware reports exist, and until then only `paneflow-host session
create --sandbox` reaches their wraps. A pane a sandboxed caller spawns inherits the request, never weakened, at
each of the three servers: the worker for `surface.split`, `workspace.up`,
`workspace.create` and `workspace.restore_layout`
(`structural::inherit_sandbox`); the host for `session.create`
(`SessionHost::confine_spawn`, after authorization); and the window for
`surface.split` and `workspace.up` (`ipc_panes::confine_spawn`, which
refuses `workspace.create`, `workspace.restore_layout` and
`client.dispatch_action` from a sandboxed pane), while a `tmux.compat` split
inherits the sandbox of its team anchor. The window answers `tmux.compat`
only when the caller's verified pane principal is the pane its `surface_id`
names, so the team token alone, readable in the lead's environment, does not
let another pane or an outside process act as a team pane. The worker refuses `layout.apply`,
`layout.adopt` and `action.invoke` to a sandboxed pane
(`authority::REFUSED_IN_SANDBOX`): those either carry surface definitions it
does not rewrite or run code that reaches the window under another identity.

Ingress is treated as untrusted: session and config files are validated
structurally (layout budgets, ratio clamps, id alphabets) before they touch
app state.

## The worker between the core and its Controllers

`paneflow-serve` is the per-home worker. It sits between the PTY core and
every Controller (the GPUI app, the `paneflow` CLI, the MCP bridge) and owns
the concerns that must survive a restart of the UI but not of the terminals:
the layout model and `session.json`, the activity reducer, the session
projection, and the capability set it advertises at bootstrap.

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
  manifests under `<home>/host/sessions/`, replaying the hook frames each
  manifest holds for the agent metadata and taking the declared status it
  recorded. No terminal receives a
  signal, because the worker owns no PTY. When the app ships a newer worker it
  stops the old one with a five second drain and starts its own; the sessions
  list is identical across the swap.
- **The declared state.** For a session whose agent the worker recognizes, by
  the tool its hooks name or by the foreground runtime the host observed,
  `working` is busy, `blocked` is attention, `error` is errored, and `idle` and
  `done` are idle; an exited session is never busy or waiting. A program the
  worker does not recognize as an agent keeps `idle` and reaches the app through
  `declared_status` alone. `done` records the outcome `completed` and `error`
  records `failed` (with the declared message), `working` clears it, and every
  outcome lands in the worker's bounded activity log (`agent.activity_log`).
  `state_seq` counts each change of the reduced state, which is what
  `paneflow send --submit` and `paneflow wait --idle` follow. A turn shorter
  than the scan still counts: when `declared_status.turns` grows while the
  worker held a state other than busy and the new record is not busy either,
  the worker passes through busy first (two `agent.state_changed` events, two
  steps of `state_seq`, and the `Finished` notice of a completed turn).
- **Generations reject stale events.** A frame naming a runtime generation
  below the manifest's is refused, and a hook revision at or below the one the
  worker holds is ignored, so a snapshot recovers an event the stream lost
  without replaying an older one.
- **Background agents are tracked per child.** On Claude's `SubagentStart` the
  hook reporter atomically creates
  `<session dir>/background-hooks/<generation>/<agent_id>.json` before it
  broadcasts, and `SubagentStop` removes that one file. An `agent_id` outside
  `[A-Za-z0-9_-]` or longer than 160 bytes writes nothing.
- **The worker decides notifications.** A transition into `blocked` or `error`
  earns `Needs input`, carrying the declared message, and a transition from
  `working` or `blocked` to `done` earns `Finished`, carrying the last result
  the hooks reported or the declared message. `idle` never notifies, and a
  state the worker finds already held when it starts or discovers a session is
  not announced again. The decision rides on the `agent.event` frame as
  `notify`; the Controller decides whether the user already saw the pane and
  delivers it.
- **The host viewport scan.** Every 500 ms the host thread named
  `paneflow-host-viewport` asks each live session whose output moved for its
  title, its OSC 7501 record and capability, and its foreground process group,
  then edge-writes the manifest: `declared_status` with the session's current
  OSC 7501 record (state, kind, progress, app, title, a message bounded to
  2,048 bytes, and `turns`, the number of times the session's record entered
  `working`, counted per report by the runtime and not per scan),
  `program_status_capable`, and `observed_runtime` with the
  foreground runtime identity, dropped when the runtime declares a
  `title_prefix` the pane title does not carry.
  `declared_status` is written for every session, agent or not, and a typed key
  or a reset triggers a rescan even without new output, so a purged record
  leaves the manifest promptly. Nothing is written when nothing changed. The
  scan never reads the screen text. The worker carries
  `declared_status` on each snapshot entry, so a program without an agent
  reaches the app too: its state shows in the pane header's progress chip,
  `blocked` and `error` count toward the hidden-space attention mark, and
  `blocked` and `error` notify, naming the pane and sharing
  one notification per pane every 10 s (`src-app/src/app/declared_status.rs`).
  An `error` stops counting once its pane has been on screen: the app
  acknowledges it per surface on render, and the acknowledgment falls when the
  declared state changes, while the host record and the chip keep `error`
  until the first keystroke. Text shown outside
  the grid is stripped of bidi, zero-width and line-separator controls and
  kept to one line. No declared state ever triggers an action on a program.
- **Foreground runtime observation.** The scan matches the foreground job
  against the catalog: the process-group leader first, then the best of
  `Direct` over `Wrapper` strength, smallest ancestry depth, lowest pid. A
  `node`, `bun`, `python`, `sh -c`, `env` or `npx` wrapper is resolved through
  its script path against the runtime's `script_path_signatures`, so an
  npm-installed Claude running under `node.exe` on Windows is recognized;
  ambiguous interpreter flags (`-e`, `-c`, `-m`) never name a runtime.
  Persisted argv evidence stops at the cell that established the match, is
  bounded to 8 cells and 1 KiB, and is omitted for wrapper matches. The
  observation is refused outright when the session leader's recorded kernel
  start time no longer matches. Unix reads the PTY's foreground process group;
  Windows walks the child process tree and reads each PEB command line.
  The desktop takes a pane's agent from this observation alone:
  `apply_host_observed_agents` sets the tab badge and the sidebar's unhooked
  rows from the host rows, and the desktop's port scan (`workspace/ports.rs`)
  reads listening ports and the foreground command, never an agent identity.
- **Capabilities, not probes.** `worker.hello` answers a `WorkerIdentity`
  carrying the set in `protocol/host-capabilities-v1.json`. A Controller reads
  that set once and never discovers a feature by trying it. A capability the
  file does not list is refused by the client before a frame leaves, with
  `capability not advertised: <name>`.
- **One unread state.** A session carries `unread`, raised by the worker
  when it publishes a `finished` notification and lowered by
  `agent.acknowledge`. The desktop reads that flag off the projection and
  acknowledges the sessions the user has actually looked at, whatever its own
  local badge holds, so a restart of the window cannot strand an `unread` a
  remote Controller would keep showing.
- **A second Controller proves the protocol.** `crates/paneflow-serve/src/controller.rs`
  is the Controller client: it connects, reads the advertised set, follows the
  stream, and reconnects to a worker that restarted without replaying a row
  whose projection has not moved (recency stamps alone do not make a row
  fresh). `paneflow sessions [--follow] [--json]` is that client as a separate
  process, and `protocol/controller-conformance-v1.json` lists the cases it
  must pass; `crates/paneflow-serve/tests/controller_conformance.rs` runs every
  one of them against a real worker, and a listed case with no runner fails.
- **Restart recommendation.** A session whose core reports a
  `host_protocol_version` below the version this worker requires carries a
  `restart_recommended` token instead of failing. `host_build_id` travels
  beside it for diagnosis only; no restart path reads it.
- **Identity before any signal.** The health refresh marks a session stopped
  only when its recorded child pid with a matching kernel start time is
  absent. An unknown or recycled pid stays `non_resumable` and is never
  signaled.
- **The method families it owns, nothing more.** The worker answers the
  methods `crates/paneflow-api` declares with the worker as owner (`worker.*`,
  `agent.*`, `usage.*`, `events.*`, `config.*`, `action.*`, `keymap.*`,
  `prompt.*`, `layout.*`), plus `host.hello` as a greeting and
  `system.describe`, which lists exactly those methods. Its dispatch is the
  registry in `crates/paneflow-serve/src/registry.rs`: a worker method with no
  handler, or a handler whose reply kind differs from its declaration, fails
  the registry build, and every call is validated against its declared
  parameter schema. It still forwards nothing to the core, because it is
  replaced on every update and owns no terminal: a request relayed through it
  would add a hop and fail whenever an update swaps it. The CLI and MCP reach
  `session.*` and `surface.*` on the host endpoint directly when no desktop is
  running, and a client routes each method to its owner from the descriptor.
  A new method family also enters `protocol/host-capabilities-v1.json`, and
  the Controller refuses a method its worker does not advertise before any
  frame leaves.
- **One numbered event log.** The worker keeps the log in
  `crates/paneflow-serve/src/event_log.rs`: every event gets the next `seq`
  (from 1 for each worker start, named by an `epoch`), a `ts`, a dotted
  `type`, the ids that apply and a `data` object, and the last 1,024 stay in a
  ring. `events.follow` sends a header with the current `seq` and `epoch`,
  replays the events after `since` when they are all still in the ring, and
  otherwise refuses with an `evicted` error (-32040) carrying where the log
  stands, so a follower either sees every event once or knows it missed some.
  A follower more than 8 MiB behind is closed alone. Terminal and agent events
  are derived from the host's `agent.follow` frames by comparing each row with
  the worker's last known values, so a reconnect or a worker start announces
  only what moved; layout events are appended by the worker itself, in the
  order the op that produced them was applied by `layout.apply` or
  `layout.adopt`. `protocol/events-conformance-v1.json` lists the cases
  `crates/paneflow-serve/tests/events_conformance.rs` runs against a real host
  and worker. The window's legacy `events.subscribe` is a separate stream and
  is unchanged.
- **The layout authority.** `crates/paneflow-serve/src/layout_authority.rs`
  holds the one `paneflow_layout::Model` of a home and is the only writer of
  `session.json` while a worker runs. It loads the file at start, migrating an
  older schema, and an unreadable file leaves an empty model and is renamed
  aside on the first write so the window's recovery dialog still has it. The
  model carries a `revision` that every accepted change raises by one, written
  to the file as `layout_revision`. `layout.view` answers the model,
  `layout.attach` records a window as the layout client, `layout.apply` applies
  one op and answers its revision and events, and `layout.adopt` replaces the
  model with a client's when the worker's revision is not greater than the
  revision the client says it acknowledged, and otherwise answers the worker's
  model. A connection that sends any `layout.*` method may send frames up to
  16 MiB. The file is written 150 ms after the last change, staged and
  renamed, and a clean stop flushes it before the endpoint closes; a failed
  write keeps the model in memory and shows in `worker.status`. A
  `session.json` another process rewrote since the worker last read or wrote
  it is read again before the next `layout.attach` or `layout.adopt`. The worker
  performs the effects ops return that need no window: it stops a closed
  terminal through the host's `session.stop` when its undo hold expires (the
  5 s hold is timed by the worker), rehomes a session through
  `session.workspace`, and stops the sessions an accepted adopt removed.
- **Id leases.** Ids are never chosen by the worker on a client's behalf.
  `layout.lease_ids` grants a block of 1,024 ids and moves the persisted
  counter past it before it answers, and a worker that starts from a file
  advances that counter by one more block, so an id granted before a crash is
  never granted again. A client creates panes, surfaces, tabs and workspaces
  with ids from its blocks without waiting; `layout.apply` replays the op with
  the client's next id (`ids_from`) so the worker allocates the same ids the
  client did, and refuses an op whose ids fall outside the client's blocks
  with `invalid`, or with `ids_exhausted` when the blocks are used up.
- **The window is a layout client.** When the worker advertises
  `layout.authority`, the window attaches, adopts its restored model with the
  `layout_revision` of the file it read, and takes a lease. Every gesture is
  applied to the window's own model at once and projected in the same frame,
  then sent to the worker by `paneflow-layout-client`; a divider drag sends one
  op when it ends. An answer whose revision differs from the window's, a
  refused op, or a layout event the window did not cause makes it fetch the
  view (`layout.view` with `unless_revision` answers nothing when nothing
  moved) and reproject, keeping every pane whose id persists; a refused split
  or undo shows the toast it shows when refused locally. While attached, the
  window starts each new terminal through `layout.launch`: the worker builds
  the request with `paneflow-launch` and creates the session under the id the
  window chose. Detached, the window creates the session itself and binds it
  with a `bind_session` op. A view fetched from the worker that names
  surfaces the window has no terminal for (a workspace or a split another
  client made) gets those terminals before it is projected. While attached
  the window leaves stops to the worker and never writes `session.json`.
- **The fallback write.** When no worker answers, or the worker predates
  `layout.authority`, the window is the layout authority: it keeps applying
  ops with the ids left in its lease and writes `session.json` on the 150 ms
  debounce with its model's revision, but only while no worker holds
  `serve/owner.lock`, or when the worker that holds it predates the layout
  methods. A creating op that needs ids the lease no longer has is refused
  with the split's pane-limit toast. When a worker returns, the window adopts
  its model back with the last revision the worker acknowledged, or the
  revision the window last wrote when the worker is a new start; a worker
  whose revision is greater keeps its model, and the window shows it with one
  "layout was refreshed" toast. A quit while attached waits up to 2 s for the
  worker to acknowledge the last change. A window that quits without being
  attached writes `session.json` itself whoever holds `serve/owner.lock`, so
  its last model is on disk even when a worker holds the lock and does not
  answer; the worker reads that file again before the next window attaches.
- **Two homes for Lua.** Both run `paneflow-script`. `paneflow do` runs a
  file, a chunk or a named action in the CLI process
  (`src-app/src/cli/do_cmd.rs`), with no memory cap or deadline unless the
  user passes one; each `paneflow.*` call is routed to the method's owner
  from its descriptor, and Ctrl-C stops a busy loop through an
  instruction-count hook. `init.lua` (`<home>/init.lua`) runs inside the
  worker (`crates/paneflow-serve/src/config_runtime.rs`): it is loaded again
  300 ms after a save settles, a failed load keeps the previous configuration
  and reports its file and line, the state is capped at 256 MiB, and its
  handlers and timers live as long as the worker, not the window. Its actions
  are served by `action.invoke` with a 30 s limit, and its bindings by
  `keymap.list`, which the window applies under its own Settings shortcuts.
- **One prompt broker.** `crates/paneflow-serve/src/prompts.rs` holds the
  questions an action, a script or later an agent asks the user, in memory
  only. `prompt.request` queues one when a `prompt.follow` client can show its
  kind (`no_client` with no follower, `unsupported` when none shows it, `busy`
  at 16 pending) and waits at most `wait_ms` (25 s) before returning `pending`
  with an id that `prompt.wait` resumes, so no call outlives a client's reply
  deadline. Every follower receives a `requested` frame, the first
  `prompt.reply` wins, and every follower then receives `ended`; a later reply
  is refused with `not_pending` (-32060). A prompt ends with `timeout` after
  its `timeout` (300 s by default, at most 3,600 s), with `no_client` when the
  last follower able to show it leaves, with `failed` when its asker has not
  waited on it for 15 s or the worker stops. The window follows the stream on
  a background thread and shows one prompt at a time in the palette shell
  (`src-app/src/app/prompts/`): a prompt activated from the attention queue
  first, then the user's actions and scripts, then agents, oldest first, and
  never while the command palette or the file finder is open. In Lua,
  `paneflow.ui.input` and `paneflow.ui.pick` loop on `prompt.request` and
  `prompt.wait` from an offloaded thread and pause the action's 30 s limit
  while they wait. `protocol/prompt-conformance-v1.json` lists the
  multi-client cases `crates/paneflow-serve/tests/prompt_conformance.rs` runs
  with plain protocol clients against a real worker.
- **Only the user answers an agent.** Each `requested` frame carries a
  `token` of 128 bits from the operating system's generator, one per prompt
  and per follower, held in the broker's memory and nowhere else. A prompt
  raised by an agent, or by an action an agent runs, is never sent to a
  follower in a pane, and `prompt.reply` takes it only from a client outside
  every pane that sends back the token it was given; any other reply gets the
  `not_pending` refusal an unknown id gets.
  `paneflow_api::methods::prompt::prompt_access` holds that rule, and
  `crates/paneflow-api/tests/decision_table.txt` lists it for each principal
  against each asker. The worker also decides who is an agent from the
  process that calls, not from what its hooks reported: when a pane
  connection is classified, `paneflow_peer::LiveLineage` walks the caller's
  parents up to the pane's shell named in its session manifest, and
  `crates/paneflow-serve/src/agent_caller.rs` compares each one (name,
  executable, argv) with the runtime catalog. A known agent among them, a
  parent that cannot be read, a reused pid or a chain that never reaches the
  shell makes every `action.invoke`, `prompt.request` and
  `agent_prompt.resolve` on that connection the agent's, whatever origin it
  claims. A run an agent starts answers `running` with an opaque `run_id` of
  128 bits that only the agent's own pane session can wait on, and an agent's
  prompt stays bound to its asker after it ends.
- **A gesture grant lets an init.lua action type for the human.** The write
  gate (`PANEFLOW_IPC_SCRIPTING=1` or `ai_unrestricted`) refuses
  `surface.send_text` and `surface.send_keystroke` from every client but the
  window, the worker included. When the window runs an action from a key or
  the palette (`src-app/src/app/lua_config.rs`), it mints a random 128-bit
  grant, keeps it in memory, and passes it in the `ctx.grant` of
  `action.invoke`. The worker stores it in the run's task scope, never in the
  Lua `ctx` table, and attaches it as a top-level `grant` member of the
  JSON-RPC envelope of every window call that run makes, nested
  `paneflow.invoke` calls included; timers and handlers the run schedules run
  in their own tasks and carry none. The window admits a request whose grant
  it minted and has not revoked, and revokes it when the `action.invoke` reply
  arrives or after `ACTION_REPLY_DEADLINE`. The same action run from the CLI,
  the MCP bridge, an agent or a timer has no grant and stays gated, and an
  `origin` of `key` or `palette` sent by any other client grants nothing.

### Plan usage and the activity ledger

The worker also owns plan usage: the session and weekly limits of each Claude
Code and Codex subscription, and a daily ledger of turns, tokens and active
time per account. An account is one (agent, canonical config directory) pair;
`paneflow_agent_config::usage::accounts::inventory` lists them from the
default directories, every space's `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or
`GROK_HOME`, and the agent profiles. Grok has accounts but no usage: see the
probes below. The code lives in `crates/paneflow-serve/src/usage/`, and the
app only renders what the worker publishes (`src-app/src/app/usage/`).

**No token is ever read.** Paneflow never opens Claude Code's
`.credentials.json`, Codex's `auth.json` or a keychain item, never keeps an
OAuth token, and never sends an HTTP request for usage. Grok is the one
exception to "no credential file is opened": its CLI names the signed-in
account nowhere else, so
`paneflow_agent_config::usage::accounts::grok_account_identity` reads
`<GROK_HOME>/auth.json`, capped at 1 MiB, and keeps only the `email` and
`subscription_tier` of the `auth.x.ai` login; the tokens beside them are
parsed and dropped, never stored, logged or sent. Limits reach the worker
through three paths, all of which leave authentication to the agent's own
unmodified binary:

- **The status line relay.** When `usage.plan_limits` is on (the default) and
  the user has no status line of their own, the Claude integration installs a
  managed `statusLine` whose command is `paneflow-ai-hook StatusLine`. Claude
  Code pipes its status JSON to it after each response; the relay keeps only
  the session, the config directory, `rate_limits` and the receive time, and
  sends them to the host as an `agent.event` of kind `status_line` within a
  40 ms deadline, printing nothing. The host republishes it as a `plan_limits`
  frame on its agent bus, and the worker stores it. A foreign status line is
  never rewritten, and `usage.plan_limits: false` or removing the integration
  removes the managed one.
- **The probes.** `usage.refresh`, sent when the sidebar bubble opens, when the
  Usage page refreshes, when Settings > Agents opens on an account whose
  identity is unknown or stale, and by the app's automatic refresh, makes the worker
  probe each requested account at most
  once per 30 s, two at a time, on background threads
  (`usage/probe.rs`): Claude Code runs as
  `claude -p --input-format stream-json --output-format stream-json` with the
  account's `CLAUDE_CONFIG_DIR`, hooks and MCP servers off, and answers the
  `get_usage` control request without any user message; Codex runs as
  `codex app-server` and answers `account/read`, then
  `account/rateLimits/read`. The same answers carry the signed-in identity
  (the `account` of Claude's `initialize` response, Codex's `account/read`),
  so the worker stores each account's email and plan name beside its limits
  and Settings > Agents lists the accounts without any sign-in step. An
  account whose limits cannot be read but whose agent names a signed-in email
  still records that identity. The real binary is
  resolved with every Paneflow shim directory removed from `PATH`, and no
  `PANEFLOW_*` variable reaches it. Grok reports no limits outside its own
  interface (`grok usage --json` refuses), so its probe only runs
  `grok models` with the account's `GROK_HOME` and reads the first line:
  "logged in" records the identity above, or the plan name `Grok account`
  when the file names nobody, and "not authenticated" clears it.
  `UsageAgent::reports_usage` is false for Grok, which keeps it out of the
  Usage page, the sidebar capsule, the automatic refresh and the activity
  import: it is probed only while Settings > Agents or Spaces is open.
- **Codex rollouts.** At each Codex turn end, and every five minutes while a
  Codex session runs, the worker reads the tail of the newest
  `<CODEX_HOME>/sessions/**/rollout-*.jsonl` for its last `rate_limits`.

**Several accounts, no credential swap.** An account is a config directory, so
Paneflow never copies or rewrites an agent's credentials to change accounts.
**Add account** in Settings > Agents opens one dialog that names the folder:
`~/.claude-`, `~/.codex-` or `~/.grok-` followed by a suffix, prefilled with
the first free number (from 2) and editable (`~/.claude-work`). The suffix
takes letters, digits, hyphens and underscores, 32 characters at most, and is
refused when the folder exists or an account already names it. The name is
chosen at creation only, because the directory is the account's identity
everywhere else (plans, ledger, default account, spaces, and on macOS Claude
Code's keychain service name). Paneflow then creates the folder with an agent
profile named `Claude Code <suffix>` pointing at it, installs Hooks and MCP
there when the agent's default directory has them, and signs in without
leaving the page.
`paneflow_serve::usage::login` drives the agent's own sign-in on a background
thread: Claude Code runs `claude auth login`, which opens the browser itself,
and Paneflow confirms the result with `claude auth status --json`; Codex goes
through `codex app-server`, whose `account/login/start` returns the page that
Paneflow opens and whose `account/login/completed` reports the outcome; Grok
runs `grok login`, which without a terminal prints a device-code page that
Paneflow opens, and `grok models` confirms the result. The
account row shows the wait with **Open the page** and **Cancel**. For Claude,
**Open the page** opens the manual link the CLI prints, and a field relays the
code that page shows to the CLI's standard input; Paneflow never keeps it, and
never reads the credentials the CLI writes. **Cancel** on an account that
**Add account** has just created removes it whole: its profile goes, a space it
was assigned to returns to its previous account, and the sign-in thread deletes
the folder Paneflow created, with its hooks record, once the CLI has exited.
Cancel on an existing account only stops the sign-in. An account row carries
no menu: its only control is **Sign in** while it is signed out, and the CLI's
other sign-in modes are run by hand in a pane. Once the account's email is
known, from the probe or Claude's `.claude.json`, a profile that still carries
that generated name is renamed to the email. `agents.default_accounts`
(runtime slug to directory) names the account new panes start on; nothing in
Settings writes it since the row menu went, so it is set in `paneflow.json`.
`space_env::space_env` layers it under the space's own variables, so every new
pane, plain shells included, starts on that account unless its space names its
own directory. Open panes keep the account they started with, and two panes can
run two accounts of the same agent at once.

A space names its account the same way, without anyone typing a variable. The
`Accounts` card of a space page (`settings/tabs/space_accounts.rs`) reads the
space's `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `GROK_HOME` back into a choice
and writes the choice as a `~`-relative path, or removes the key for `Default`. It never
offers an agent's default directory: setting `CLAUDE_CONFIG_DIR` to `~/.claude`
moves Claude Code's global config and, on macOS, its keychain service name, so
that account stays reachable only as `Default`. **Add account…** in the menu
opens the same dialog and assigns the new directory to the space before the
sign-in starts. The Environment editor hides those keys and merges them
back on save, so the menu stays their only editor.

The automatic refresh belongs to the app, not the worker, so nothing is probed
once the window is gone: `start_usage_auto_refresh`
(`src-app/src/app/usage/mod.rs`) ticks 10 s after launch and then every 60 s
while the sidebar capsule is shown or the Usage page is open. A tick does no
I/O; `stale_accounts` (`usage/model.rs`) picks the accounts whose newest
observation and last automatic request are both at least five minutes old, and
only those go out in one `usage.refresh`. An account the status line relay
keeps fresh is therefore never probed, an account that does not answer is
asked again five minutes later rather than at the next tick, and an account
billed through an API key or hidden by `usage.sidebar_agents` is not asked at
all unless the Usage page is open. `usage.sidebar_agents` is an app-side
filter over what the capsule and its bubble show; the worker keeps storing
every account.

The newest observation of an account wins. `<home>/usage/plans.json` holds one
snapshot per account (windows, reset times, observation time, agent version,
or a state such as `no_codex_data` or `no_limits_reported`), written at most
every 10 s and on stop; the ledger below is written at most once a minute. Each change goes out as a `usage` frame on the
`agent.follow` stream, carrying the plans and the ledger revision; the app
fetches the ledger itself with `usage.snapshot` only when the revision moved
and the Usage page is open.

`<home>/usage/ledger.json` is the activity ledger, kept for 400 days and
capped at 8 MiB (an unreadable file is moved aside as `ledger.json.bad-<ms>`).
Turns and active time come from the hook frames the worker already reduces;
tokens come from the bytes a Claude transcript gained at each `Stop` and from
the growth of the Codex cumulative totals, excluding cache tokens, with at
most 8 MiB read per file per pass and per-file offsets under `cursors`. Each
day counts in the local calendar of the entry's timestamp.

**The import.** At the worker's first start with an empty ledger, and on
`usage.import` (the page's `Import past activity`), a low-priority background
thread walks every account's Claude `projects/**/*.jsonl` transcripts and
Codex rollouts, newest first, up to 10,000 files or 2 GB
(`usage/backfill.rs`). It counts a turn per user prompt (not a tool result,
meta line or sidechain; a Codex `task_started`) and the same tokens as the
live path, then books the result in one step. A resumed Claude session copies
its earlier lines into a new transcript, so the import counts each prompt
`uuid`, assistant message `id` and Codex `turn_id` once per account across
all files. Only days before today that the
ledger does not already hold are booked, and each one is flagged per account
under `imported`, so a second import adds nothing and a live read of an old
transcript never adds to an imported day. When a limit stops the walk, the
days the unread files could still cover are dropped and the import record
(`import`) keeps `limited_days`, which the page shows as `Imported the last
{n} days`. A stop request interrupts the walk and books nothing. Progress
rides on the `usage` frame as `import.running` and `import.files`.

## Local host and durable session identity

`paneflow-host` is a GPU-free crate (library plus `paneflow-host` executable)
that owns terminal execution independently of a GPUI entity: the PTY pair, the
child process handle, the canonical `libghostty` terminal, an 8 MiB output
tail with monotonic byte offsets and the session manifests. Nothing in it
links GPUI. The desktop no longer spawns a PTY of its own: every terminal view
resolves a hosted session and attaches to it (see "Attachment and the client
mirror" below). The in-process runtime in `ghostty_session/pty_runtime.rs` is compiled
only under `cfg(test)`, for the perf bench, the Ghostty stress gates and unit
tests, so `portable-pty` is a dev-dependency of the app and the release binary
carries none of it.

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

A caller that must not leave a host behind, which today means the host's own
integration tests, starts it through `ensure_host_running_for` with
`HostLifetime::EndsWithCaller`. The spawn then adds `--exit-with <caller pid>`,
and `serve` arms `owner_watch::OwnerWatch` before it binds the endpoint, while
the caller is still blocked waiting for the first `host.hello`, so the pid
cannot have been recycled. The watch waits on a process handle on Windows, a
pidfd on Linux and an `EVFILT_PROC` kqueue event on macOS. Where none is
available it polls the caller's `ProcessIdentity`. When the caller exits, the
host runs a forced `host.shutdown` and stops serving. This covers exit paths
that run no destructor, such as nextest killing a test at its timeout. The
desktop and `paneflow host start` keep `HostLifetime::OutlivesCaller`.

| Event | Host | Sessions | Records |
|---|---|---|---|
| Quit keeping sessions (the default of the quit dialog), crash or `taskkill /F` | keeps running | keep running | unchanged |
| Quit with no live session, or "Stop everything and quit" | `host.shutdown` once the stops are done | stopped one by one | removed once every stop is confirmed and saved; the window unbinds each terminal's `session` and `agent` in the worker's layout, which the worker writes to `session.json`, so the layout reopens with a new shell per pane |
| Controller pipe or CLI connection closes | keeps running | keep running | unchanged |
| Explicit `session.stop` | keeps running | that session's owned process tree is terminated within a 5 s budget, exit recorded | `lifecycle: exited` |
| `paneflow host stop` with live sessions | refused, lists them | untouched | unchanged |
| `paneflow host stop` when idle | exits, removes `instance.json` | none live | manifests kept |
| Exit of the caller of a `serve --exit-with` host | forced `host.shutdown`, then exits | stopped | those of a forced `host.shutdown` |
| Host process death or reboot | gone | processes gone | next host marks running records `lost`, never signals them, while a shell that died first under a shutdown is recorded `exited` with a non-zero code; on the next layout restore each non-agent pane in either state comes back with a fresh shell in its cwd, and an agent pane offers its conversation resume |
| `session.restart` on an exited or lost record | same host | new generation, recorded shell with no arguments, no input or command replay | `generation + 1`, current owner |
| Incompatible host on the endpoint | left running | untouched | unchanged; the controller reports the mismatch, the pane offers Retry and an explicit "Stop host and restart" |
| Stop or shutdown that cannot be confirmed (wait failed, descendants unresolved, runtime panic) | keeps running and keeps owning the process | `lifecycle: unverified`, never `exited` | the desktop keeps its window open with Retry / Keep running and quit / Cancel |

Reconnection classification lives in `SessionSummary::reconnection`: `live`,
`starting`, `exited`, `failed`, `host_replaced` (the record's owner is not the
current instance), `lost` or `unverified`. `paneflow host status` and
`paneflow-host session inspect` print it.

The desktop creates or restarts a process implicitly in exactly one case: the
first resolution of a pane rebuilt from a saved session id by
`PaneFlowApp::build_restored_surface`, which serves both `restore_workspaces` at
launch and `workspace.restore_layout`. When that resolution ends `lost`, `host_replaced`, or `exited` with a
non-zero code or a signal, the pane submits one
`Restart` of the generation it observed; when it ends `missing`, the pane
submits one `Create` under a fresh `SessionId`. Both launch the recorded shell
with no arguments and replay no command, input or persisted environment
(`HostLinkEnd::restore_recovery`, armed by `TerminalView::recover_when_restored`
and consumed by the first outcome). A pane whose recorded agent conversation is
pending restore is excluded, because the conversation resume already reopens it.
An unclean exit counts as lost because a Windows shutdown or `host stop --force`
ends the shell with `STATUS_CONTROL_C_EXIT` or code 1 while the host is still
recording, and on Unix the shell dies on a signal; the conversation resume uses
the same rule (`HostLinkEndKind::lost_by_the_host`). A clean `exited` (code 0,
the user typed `exit`), `failed` and every other end keep their button, a pane that ends
later in the same app run never recovers on its own, and a recovered pane that
ends again shows its button, so no pane is relaunched twice. `host_link::resolve`
carries one of three intents: `Create` (a new pane, a fresh `SessionId`),
`Reattach` (a restored layout, a sidebar row, a Retry) which only attaches to a
live owned session and otherwise ends the pane with the host's reconnection
state, and `Restart { expected }` which is the explicit Restart button, the
sidebar context menu or the "Resume Ended Sessions" command and submits the
generation the desktop observed so two restarts of one generation commit at
most one new generation. A record the host does not know ends the pane as
`missing`; its action is "New terminal" with a fresh id, never a silent create
under the old id. Build identity (`host_build_id`) is diagnostic only: the
handshake accepts a client from another release when the protocol version and
the terminal engine agree, logs the drift, and the compatibility fixtures in
`crates/paneflow-host/src/server.rs` pin that contract.

Every lifecycle request and every asynchronous completion names its generation.
A launch is registered as an operation until it is conclusive: the requester
may disconnect, the startup deadline may pass, and the host keeps the
`LaunchHandle` under a `paneflow-host-launch-owner` thread until the child
reports or fails, then re-checks the record and the operation at commit. A stop
that raced a restart drops its outcome when the generation moved on, so a stop
of generation N never writes `exited` into N+1. A restart is transactional:
the previous manifest is restored when its persist fails and no `starting`
record is stranded. Viewport scans and hook events capture the generation they
observed and are re-checked when they commit (`SessionHost::commit_scan`,
`ingest_agent_event`). At most eight launches are unresolved at once
(`MAX_PENDING_LAUNCHES`): a `session.create` or `session.restart` beyond that
waits in the host for a slot up to `LAUNCH_ADMISSION_DEADLINE` (5 s) instead of
being refused, and only a wait that outlives that deadline answers `-32000`.
`-32000` is therefore always transient: it names a capacity that frees by
itself (a launch slot, a checkpoint staging slot, a connection slot or a
streaming slot), while an oversized checkpoint answers
`ERR_CHECKPOINT_TOO_LARGE`. The desktop is the only layer that retries it,
with jittered exponential backoff for up to 60 s. A pending launch is reported
as `starting`, never as a verified process. No registry lock is held across PTY
I/O, process waits or observer callbacks: the map lock brackets record lookups
and commits only.

Agent events require the generation captured by their producer. They commit,
attempt persistence, and publish a nonblocking notification under the session
ledger before the acknowledgement is written. Parallel requests therefore publish
in revision order even when a requester stops reading its acknowledgement.
The ack carries `revision`, `durable` and `persistence_error`; a retry of the
same event after an ambiguous ack is acknowledged as `duplicate` from a bounded
per-session receipt ledger and never notified twice. The manifest retains the
latest event and latest activity transition as two bounded frames; ingress rejects
an event before commit if its manifest would exceed the 64 KiB reload limit.
Workers reject older revisions and recover newer accepted state from snapshots or
disk, including when the compact seed write fails. A full worker queue reconnects
for a new snapshot instead of silently dropping accepted state. Hook seeds are
written on the persistence writer thread as exclusive jobs that
re-check the session ledger `session.remove` marks removed, so a delayed write
cannot recreate a removed session directory. The fixtures behind these guarantees are
`crates/paneflow-host/src/bin/paneflow-session-fixture.rs` (idle, echo, flood,
paced stream, deterministic history, delayed exit, blocked stdin, descendants,
split escape and UTF-8 sequences) and
`crates/paneflow-host/tests/ownership_probes.rs`. Development recovery commands, all scoped by
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

A workspace has one `WorkspaceId`, held by the layout model in
`extras.durable_id`. The client that creates the workspace assigns it (the
window, or the worker for `workspace.create` and `workspace.up`), every
launch path passes it to the host as the session's workspace, and a window that
draws a workspace another client opened takes the model's id
(`layout_bridge/project.rs`) rather than minting its own. Workspace-scoped
checks (`pane.write`, `grant.delegate`) compare these ids, so two ids for one
workspace refuse writes between its own panes.
The window keeps the git location of every workspace it draws (repository
root, worktree root, branch), which feeds the branch label, the Changes dock and
the branch row of the New pane palette. The model carries only the cwd, so the
projection probes git off the render thread (`spawn_initial_git_stats`) for each
workspace it mints from another client's op and for each one whose cwd the
model moves, as the window's own create paths and session restore do.

### Persistence service and resource budgets

Nothing on the PTY path, the runtime thread, or a control connection writes a
manifest. `crates/paneflow-host/src/persistence.rs` owns one writer thread,
`paneflow-host-persist`, and three write classes. `Metadata` (viewport scans,
resize, bind, activity) is coalesced to the latest revision per session and
served round-robin, so a chatty session cannot starve another; a write starts
within 250 ms of submission when storage is healthy. `Critical` (create,
restart, adoption, accepted agent events) is queued in order and the caller
waits at most five seconds for the acknowledgement, then receives a typed
`HostError::Durability`. `Final` (exit, unverified) is never dropped: a final
revision that cannot be written is retained and retried every five seconds
until storage recovers, and the per-session ledger keeps the failure visible
through `session.inspect` as `durability_error` until a later revision lands.
A hook seed only copies the hook the manifest already holds, so a seed whose
session data directory is gone is skipped with one warning per session instead
of failing the revision: it never keeps a final state pending or a quit
waiting. A restart recreates that directory, which a record written before
0.17 never had.
Critical and final revisions are fsynced before the atomic rename; the parent
directory is synced on Unix. Every revision carries a per-session number
assigned under the manifest lock, and the writer discards a revision older than
the one already on disk or submitted after the session was marked removed. The
queue is bounded at 8 MiB with a 16 KiB final-revision reservation per live
session: when the budget is exhausted, metadata is rejected with
`QueueFull`, critical waits, and final still fits its reservation.
`session.remove` and record trimming use the writer as a barrier, so a manifest
deleted by the host is never resurrected by a job queued earlier.

Every other queue has a byte budget and a typed refusal. The output tail is
allocated exactly and never doubles past its 8 MiB cap. Input queued toward a
child that stopped reading is capped at 2 MiB, after which `session.input`
returns busy instead of buffering. Checkpoint capture admits two concurrent
captures and 256 MiB of staged bytes, waits five seconds for a slot, and the
staged bytes are released when the attachment result is dropped. Terminal
dimensions above 4,194,304 cells are refused before reaching the engine. Of
the 128 accepted connections, eight are reserved for control: the 121st
streaming follower is refused with `ERR_BUSY` while `session.inspect` and
`session.stop` still answer. `host.status` reports all of these under
`resources` (queued and peak persistence bytes, pending final revisions,
rejected metadata, staged checkpoints, live runtimes, pending launches,
connection counts, and per-session tail and input usage), and
`paneflow host status` prints them. `resources.sessions` lists every session,
with or without a runtime, and carries each one's terminal memory (resident,
virtual, compressed and image bytes, and whether compression is supported).
A session without a runtime reports `runtime` and `memory` as null with
`memory_unavailable` set to `not_started` or `exited`; a runtime that does
not answer reports `timeout`. The memory reads go out to every runtime
before any reply is awaited, under one shared 500 ms deadline
(`MEMORY_USAGE_BUDGET`), so the reply stays bounded however many sessions
stall. Bounded final text (up to 512 KiB) is
larger than one 64 KiB control frame, so `session.text` answers in frames of
at most 48 KiB of text with `next_offset` and `total_bytes`;
`HostClient::text` reads them back in order, and any other reply that would
exceed the frame limit is refused with `ERR_FRAME_TOO_LARGE` instead of
closing the connection. On Windows, descendant discovery ignores a process
whose creation time is later than the exit time of the parent handle it hangs
from: after the root exits its PID can be recycled, and the children of the
new owner are not the root's descendants. The qualification runbook in
[docs/release/persistent-qualification.md](docs/release/persistent-qualification.md)
maps each budget to the test or workload that proves it.

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
the checkpoint offset with `follow=true`. On the host, followers wait on the
session's `OutputStream` (a mutex and condvar in
`crates/paneflow-host/src/stream.rs`) for a publication past their offset, the
end of the stream, or a keepalive deadline; nothing polls. The host emits a
keepalive frame every two seconds while idle; the follower skips overlapping
bytes, treats a gap or `ERR_OUTPUT_EVICTED` as a request for a fresh
checkpoint (`RuntimeMessage::RestoreCheckpoint` replaces the mirror, and a
replacement still queued is overwritten rather than stacked), reconnects with
an exponential backoff from 500 ms to 5 s that a stop cancels, and verifies
the host instance token on every reconnect so a replaced host is reported as
`host_replaced` instead of being followed. The attachment checkpoint itself
is a one-shot `CheckpointPayload`: it is decoded once into the mirror and
dropped, so no serialized snapshot stays resident after a restore. Frames from a previous generation or attach attempt are discarded
by the runtime's stop flag. When the stream ends with `live: false`, the
follower classifies the end through `session.inspect` (exited with code or
signal, failed, lost, restarted, missing). Once a child exits, the host
drains the PTY for at most two seconds, retires the terminal, releases the
tail after a one second grace, and keeps a completed record: exit outcome,
final offset, completeness, and up to 512 KiB of final screen text in a
`final-output.txt` cold file capped at 64 MiB per home. `session.text`
serves that text, from memory until its cold write lands, or says it is
unavailable, without any launch; a pane
restored onto an ended record shows it read-only. A maintenance thread runs
retention (24 h for finished records, 30 d for interrupted ones) at most
once a minute.

`TerminalState.host_link` carries `HostLinkState`: `Attaching`, `Attached`,
`Reconnecting`, `Ended(HostLinkEnd)` or `Unavailable`. Input is accepted only
while attaching or attached; in every other state the input dispatcher
rejects it and pending input is cleared, never queued for a later resend.
Control requests (input, runtime binding, resize) leave the mirror loop
through a bounded queue to a dedicated `paneflow-ghostty-control` thread, so
a slow host never stalls rendering. When the stream ends, the mirror releases
that control connection, so a passive final view holds no host connection and
no host thread. A `session.input` call that fails after
a disconnect is reported as delivery unknown, never resent: only input the
host refused before reading it is known to be unsent. Control connections
have no idle expiry; the host bounds the handshake and partial frames, not
silence, so the first key after an hour of idling goes through the same
connection exactly once. `render_host_link_overlay` draws the reconnecting, ended and
unavailable states over the last rendered frame; Enter on an ended or
unavailable terminal runs `resume_hosted_session`, which re-resolves the same
`SessionId` with the `Resume` intent.

### Closing views

Dropping a `TerminalState` only shuts the local runtime down; nothing in a
`Drop` implementation calls `session.stop`, so a crash or `taskkill /F` leaves
every session running on the host. Every close path goes through
`app/close_policy.rs`: a `CloseTarget` names what the action closes, and the
close happens at once, without a confirmation dialog. The only prompt left on
that path is the unsaved files dialog, for a tab or workspace whose diff dock
holds edited files. What is closed is never reopened, so its sessions are
stopped and their records removed. Every stop, from a close path or from the
quit dialog, goes through the single call site `host_link::stop_session`,
which a guard test in `terminal/host_link.rs` enforces.

| Action | Sessions |
|---|---|
| Close pane by shortcut, close tab | stopped and removed after a 5 second undo window; undo (`Cmd/Ctrl+Shift+T`, or the tab toast) within that window reattaches the still running session |
| Close pane by button or pane menu, close surface tab, close diff dock terminal, close workspace, worktree removal | every contained session stopped and its record removed at once |
| Child exit | nothing closes: the surface stays as a passive final view (zero or nonzero code, with or without prior input) until the user closes it through one of the rows above |
| Any close while the local host is unreachable | the views are removed, no stop is attempted and a toast says the session state is unknown |
| Return a detached pane to its window | untouched |
| Quit (`Quit` action, main window close, title bar close) with live sessions | asks: "Keep sessions running" (Enter, default) leaves them and the host untouched; "Stop everything and quit" stops each live session, then `host.shutdown`; the `on_quit` setting skips the dialog |
| Quit with no live session | the idle host receives `host.shutdown` and the app exits |

A stop whose outcome is unknown (connection lost mid-request) shows a toast;
the desktop never marks sessions stopped optimistically. Every stop the
desktop issues chains `session.remove`, which the host refuses with
`ERR_SESSION_LIVE` while the session runs and which otherwise deletes its
manifest. A stop the host answers with `ERR_PROCESS_UNVERIFIED` (a `Lost`
record that has no process left) falls back to that removal alone. A second
stop for a session whose stop is still in flight is dropped.

The sidebar lists no hosted session: a session is reachable only through the
view that holds it. The desktop keeps a reduced listing of `session.list`
(owned live sessions, for the quit dialog and worktree teardown) and, after
every listing, asks the worker for an orphan sweep with `layout.sweep`,
passing the sessions it holds outside the layout (dock terminals, stops in
flight) as `keep`. The worker runs the sweep against the host itself
(`paneflow_serve::sweep`). Every ended record (`Exited`, `Failed` or `Lost`,
owned or left by a previous host) whose id no surface of the layout, no undo
hold, no pending `layout.launch` and no kept session names is removed with
`session.remove`; a record that may still own a process (`Starting`,
`Unverified`) is never touched. The first sweep after a successful restore,
and the first one after the user starts a new session from a failed restore,
is sent with `live`, and also stops and removes every owned live session
nothing holds; a live request the worker cannot serve stays pending for the
next listing. The desktop sends no sweep while a failed restore is
unresolved, and the worker refuses the live part while its own
`session.json` was unreadable, because the preserved file may still reference
those records. With no worker attached, no sweep runs. Worktree teardown runs after the
workspace is closed: it polls the host until the closed workspace's sessions
have ended (100 polls 50 ms apart at most), then keeps every worktree that a
tab, a workspace root or a live session (attached or not) of an open
workspace still sits in; an unreachable host proceeds with the current rules;
any other host error skips the teardown
(`paneflow_git::worktree::close_teardown`). A worktree kept for any reason,
automatic removal turned off included, is listed again by an open workspace
(the one whose tab uses it, else one on the same repository, else the active
one), so Settings still shows it and the keep limit still trims it. When no
workspace is left to list it, the app keeps it in `kept-worktrees.json` in the
state home, validated against the owner marker on load, so Settings and the
keep limit still see it after the last workspace closes and after a restart.
The keep-limit trim spares any worktree a live session runs in, since a kept
worktree is bound to no tab.

The host endpoint is derived from the state home (`\\.\pipe\paneflow-host-<fp>`
on Windows, `<runtime dir>/paneflow-host-<fp>.sock` on Unix), so an isolated
`PANEFLOW_HOME` never shares an endpoint or a record directory with the normal
one. The protocol is JSON-RPC 2.0 lines on that endpoint: `host.hello` must
open every connection and verifies the protocol version and the terminal
engine identity (libghostty source sha and API version) before any effect;
`session.list/create/inspect/stop/restart`, `session.attach` (native
snapshot checkpoint plus its output offset, captured in one runtime
operation), `session.output` (contiguous bytes from an offset, optionally
followed), `session.input`, `session.resize`, `session.workspace` (files
a session under the workspace that now holds it, sent when a tab or pane
moves between workspaces and again on every attach), `agent.snapshot` and
`host.shutdown` (refused with the live session list while any session runs,
and answered only once every exited runtime has saved its completed record
and final output) follow. Control frames
are capped at 64 KiB, and output travels in 32 KiB data chunks inside those
64 KiB frames; a checkpoint is capped at 64 MiB; an
oversized frame or checkpoint is refused without unbounded allocation and
without stopping the session. The host is the only responder to terminal
queries; clipboard, bell and notification effects are not replayed from
history.

Every server that listens on a Paneflow endpoint (the detached host, the
in-process host server, and the worker) retires it through
`server::retire_listener`. On Unix it unlinks the socket path first, so a new
connect finds no endpoint, then accepts and closes whatever still reaches the
listener until 5 ms pass with no arrival (250 ms at most), and only then closes
it. On macOS, a connect still inside the kernel when its listener closes stays
attached to an orphaned peer that never sends an end of stream, even after the
server process exits (#127).

The host and worker probes (`paneflow_host::bootstrap::probe` and
`paneflow_serve::bootstrap::probe`) never stake a whole budget on one
handshake either. Both split it into the answer windows of
`host_control::answer_windows` (1, 2, 4 s, then the remainder) and retry only a
connection that did not answer, so an orphan costs one second and a slow but
live peer still gets a 4 s window. `probe_within` bounds those windows by the
caller's own deadline, which keeps `stop_worker`'s drain, the startup waits and
`stop_incompatible_host` to the time they announce. The worker probe reports
an endpoint as unreachable only when it is missing or refuses connections; one
that accepts and never answers is faulted, so it is never mistaken for a
stopped worker and doubled by a second one. The worker being replaced still
runs the previous release, which closes its listener without draining, so this
client side is what covers the first upgrade.

## Git goes through the git CLI

Paneflow has one git implementation: the user's `git` binary. Every read and
every write spawns it through `crates/paneflow-git/src/command.rs`, off the GPUI main
thread, so a repository behaves in Paneflow as it does in the user's terminal,
whatever its layout (reftable references, SHA-256 objects, sparse checkouts,
submodules) and whatever git ships next.

- **Two profiles.** `GitProfile::UserAction` runs git as the user would: clone,
  branch switch, worktree add and remove. `GitProfile::Probe` is for background
  reads: the diff badge, the Files sidebar statuses, the diff dock listing and
  the check that guards a worktree removal. A probe sets `GIT_OPTIONAL_LOCKS=0`
  so it never takes `index.lock`, a C locale, `core.fsmonitor=false`, no
  external diff, no textconv and an empty hooks directory, and it blanks the
  filter drivers that the repository's own config declares. A probe therefore
  runs no program the repository configures, while a system or global driver
  such as git-lfs runs as it does for the user.
- **One crate for the three processes.** `paneflow-git` holds the command
  builder, its spawn counters, the per-directory subprocess environment and the
  worktree code, so the window, the worker and the CLI spawn git the same way.
  A `workspace.up` or `surface.split` pane with a `worktree` request has its
  worktree planned, created and, when the layout refuses the call, removed
  again by the process that serves the call; a `setup` command in the request
  needs the orchestration gate, like any other command. That process records
  the worktree in the workspace's `managed_worktrees` (the model op
  `record_worktree` on the worker) unless an open workspace already lists it,
  and the window copies every record of the model into its own list when it
  draws a workspace. A `workspace.close` the worker serves runs the same close
  teardown as the window, on a thread of its own.
- **Known residual on Windows (#124).** Git for Windows memory-maps a worktree
  file while it hashes it, and Windows refuses a truncating write
  (`CREATE_ALWAYS`, `TRUNCATE_EXISTING`) to a mapped file with OS error 1224.
  `git status`, which the Files sidebar and the removal check run, maps a
  stat-dirty file of 32 KiB or more whose size did not change.
  `diff-index --raw` and `diff --name-status` map only a racily clean file,
  within about a second of an index write. The diff badge counts lines itself
  (`diff-index --raw`, base blobs through `cat-file --batch`, worktree files
  read in Rust) instead of running `git diff --shortstat`, which mapped every
  changed file at any size. A writer that replaces a file by rename is not
  affected, since git opens worktree files with `FILE_SHARE_DELETE`.
- **No second implementation.** An in-process engine built on gitoxide answered
  the four probes on `main` from 2026-11-06 to 2026-11-10 and was removed
  before any release; the tag `archive/gix-engine` keeps it, and
  `tasks/prd-in-process-git-status.md` and
  `tasks/prd-incremental-git-status.md` record what it measured. It closed the
  residual above and made a one-file status cheap on large repositories, but a
  second implementation trails git: gix 0.90 could not resolve `HEAD` in a
  reftable repository, the default reference format of Git 3.0, and the engine
  needed four vendored gitoxide crates and a parity oracle to stay equal to the
  CLI. Zed made the same move away from libgit2 for the same reason.

## Self-update

A single background thread polls the GitHub releases feed at launch and then
every four hours (thirty minutes after a failure), handing each result to the
GPUI tick through a shared slot; a manual `Check for Updates…` wakes the same
thread through an mpsc channel. Every known release drives the title bar update
pill, whichever check found it; a manual check only adds its transient
checking, up-to-date, and failed states. A result never displaces a download in flight or a staged binary
waiting for restart. Each install format has its own update
path (apt/dnf repos, AppImage swap, tarball swap, macOS app replacement,
Windows MSI relay), all driven by one in-app updater. Update artifacts are verified with
[minisign](https://jedisct1.github.io/minisign/) signatures and the client
**fails closed**: an unsigned or tampered artifact is rejected, never installed.
macOS builds add Developer ID / notarization checks with Team ID pinning;
Windows MSI updates add `WinVerifyTrust` before `msiexec` runs.

Every restart into a new version goes through the quit dialog's stop-everything
path (`quit_dialog.rs`), including the Windows MSI route: the staged MSI is
preflighted (`UpdatePreflight`: the replacement exists on disk, the host still
serves sessions), the desktop stops every hosted session through the single
`host_link::stop_session` call site, waits for the confirmed outcomes and the
`host.shutdown` acknowledgment, and only then spawns the MSI relay. An
unconfirmed stop keeps the window open with Retry, Keep running and quit, and
Cancel. A shared five-second action deadline reports unresolved identities while
the host retains pending ownership. A storage-only failure offers Quit
without updating, which exits only the desktop and never installs the update. The relay itself does not rely on the desktop PID
alone: after the parent exits it probes the host endpoint for up to thirty
seconds and, if a host still serves, skips `msiexec`, logs the deferral in its
relay log and relaunches the current version, because the host binary next to
`paneflow.exe` may still be in use. It preserves the staged MSI on deferral and
disables Installer Restart Manager shutdown. The relay runs from a copy of
`paneflow.exe` in `%TEMP%` that cannot delete its own image, and a
delete-on-reboot needs administrator rights, so every desktop start removes the
`paneflow-msi-relay-*.exe` copies no process still runs and keeps the relay and
`msiexec` logs for diagnosis. The relay never probes the installed
binary itself: it runs with the desktop's unelevated token, and a file under
Program Files is never writable by that token, so a write-access probe deferred
every update. Releasing the binary belongs to the MSI, whichever updater started
it. During an upgrade the `StopInstalledHost` custom action in
`packaging/wix/main.wxs` runs before `InstallValidate`, finds a
`paneflow-host.exe` running from the install folder, asks it to stop through the
installed `paneflow host stop --force`, and terminates what is left of that exact
image, never a same-named host elsewhere. The 0.16 updater starts `msiexec` with
the host still serving and Restart Manager enabled, and Restart Manager cannot
close a windowless process, so without that action the upgrade stalls on error
1611. User-facing recovery choices are documented in
[Persistent sessions and updates](docs/persistent-sessions.md).

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
