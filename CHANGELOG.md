# Changelog

Notable changes to Paneflow are summarized here. Release artifacts and full
notes are available on the [GitHub Releases](https://github.com/arthjean/paneflow/releases) page.

## [Unreleased]

### Added

- Review mode now has two rails and a pane grid. The Workspaces rail lists
  every repository open in Agents as a folder row that folds its checkouts
  and git worktrees and remembers the folded rows across restarts; clicking
  a checkout shows its diff in the focused pane,
  dragging one onto a pane edge opens it side by side, and dropping it on
  the center replaces that pane's subject. The Changes rail follows the focused pane
  and carries the base branch picker. Diff panes are
  regular pane cards: split, close, zoom, focus navigation and header drag
  work as in Agents, the grid persists across restarts, and it is capped at
  six panes. Their header replaces the split buttons with the `...` options
  menu of the Changes tab (layout, highlight, whitespace, collapse or expand
  all, refresh), applied to that pane's diff.

- The Changes tab highlights the words that differ inside a modified block,
  on top of the line wash, so a one-word edit on a long line reads at a
  glance. The comparison is a port of IntelliJ's pipeline on imara-diff:
  lines carrying three or more non-blank characters are matched first,
  short lines are paired in the gaps between them, block boundaries slide
  onto empty lines, and each modified block gets a word pass with
  punctuation matching. Two submenus join the dock Options menu next to
  Layout: Highlight (Words, Lines, None) and Whitespace (Default, Trim,
  Ignore), so an agent's reformat can be read with the indentation changes
  muted. Both last for the session, like Split and Unified, and a block
  whose two sides differ only by whitespace is painted muted.

- A file opened in a dock tab shows its changes against `HEAD` in the
  gutter: a bar for an added or modified block, a dot for a deletion.
  Typing shifts the blocks without a diff, and only the touched blocks are
  re-diffed off the render thread after a short pause. Clicking a marker
  opens a popup with the block's previous text, syntax highlighted, with
  Copy and Revert; a revert is a single undo step. Escape, a click outside,
  or an agent rewriting the file closes the popup.

- Hovering a block of a modified file in the Changes tab shows a Revert
  chip. It writes the `HEAD` lines back through the atomic save path and
  refreshes the dock, and refuses when the file has unsaved changes in a
  dock tab or changed on disk since the dock was built.

### Removed

- `Review with agent` and the terminal panel it opened under a Review diff
  are gone, along with the `review_prefill_delay_ms` setting; the key is
  ignored if it is still present in `paneflow.json`. The Review rail menu
  no longer offers `Open Shell in Worktree`. Launch agents from Agents mode.

### Changed

- The toolbar above the Review diff is gone, along with the Multi-project
  and Worktree scopes, the repo tabs, the hunk counter, the cost badge, the
  `Unified | Split` segment, `Collapse all` and the cross-column scroll
  sync (`s`). `u` still toggles unified and split, `[` and `]` still walk
  the hunks, collapse all lives in the Changes rail header, and the agent
  attribution is a tooltip on the pane header diffstat. A pane shows one
  worktree, so the per-column headers and the internal column arrangement
  left with the toolbar.

- The code editor and diff view use Zed's highlighting queries for Rust,
  JSON/JSONC, shell, Python, TypeScript/TSX/JavaScript, Markdown, Go, YAML,
  CSS, C and C++. Calls, parameters, fields, macros and attributes receive
  the query's specific classes. Variables and namespaces use editor text,
  punctuation is neutral, and constructors use the function color in all
  eight palettes. Rust `use` paths and `mod` names stay neutral.
- Overlapping syntax captures follow Zed's last-active-capture rule. The
  complete [stock priority audit](src-app/src/diff/fixtures/stock-priority-audit.txt)
  records every conflicting node and changed byte range on the regression
  corpus. Retained stock queries now classify TOML keys as properties,
  Java classes and methods by their specific roles, and Ruby parameters
  and possible implicit method calls by their later query patterns; HTML
  has no priority changes on the corpus. The
  [audit notes](src-app/src/diff/fixtures/README.md) explain these decisions.
- Vendored queries have a pinned source manifest, hashes and cross-platform
  sync checks. C++ uses Zed's grammar revision, including module syntax;
  JavaScript keeps the existing TSX parser. Markdown fenced-code injections
  and font styles are unchanged.

- The terminal grid is now measured on the font, the way Ghostty measures
  it: the cell is the font's widest advance by its own line height, each
  rounded to whole device pixels, and the baseline lands on a pixel row.
  The default JetBrains Mono at 13 pt used to be squeezed into a cell 8%
  shorter than its face, with descenders crossing into the next row; it now
  gets its designed spacing. `line_height` and `cell_width` are multipliers
  of those measured strides and default to `1.0` (they used to be `1.2` and
  `0.6` of the font size); a value carried over from an older config makes
  the grid taller and narrower than before, so re-check it.
- Underlines and strikethroughs are placed and sized from the font's own
  tables. Double, dotted, dashed, and curly underlines each have their own
  shape instead of collapsing to a single line, and they are painted under
  the glyphs so descenders stay legible.
- The renderer draws the whole box-drawing block itself (heavy, double,
  dashed, and mixed-weight joins, arcs, diagonals), plus the `░ ▒ ▓` shades,
  braille patterns, and the geometric Powerline symbols, at one
  font-derived stroke thickness on the device-pixel grid. Adjacent cells
  meet with no seam at any scale factor.
- The bar, underline, and hollow cursors take their thickness from the font
  metrics. The bar sits centered on the boundary between two characters and
  the hollow block is the block hollowed out by that thickness.
- The bundled font is the regular `JetBrainsMono Nerd Font` rather than the
  `Mono` variant: icons keep their designed size and the renderer constrains
  them to their cell, borrowing the empty cell after them when there is one
  (Ghostty's Nerd Font rule). Configs naming `JetBrainsMono Nerd Font Mono`
  or `JetBrainsMono NFM` keep resolving to the bundled family.
- SGR bold uses the Bold face rather than SemiBold, and SGR faint halves the
  foreground opacity (it used to keep 70%).
- The APCA contrast floor applied to the theme's ANSI colors is off by
  default and configurable as `terminal.minimum_contrast` (Zed's floor is
  `45`). Themes render their colors as designed.

## [0.11.0] - 2026-09-02

### Added

- A tab binds to its own worktree. The "New pane" palette and the tab context
  menu list the repository's branches: picking one that has no worktree creates
  it under `<workspace>.worktrees/`, picking one that already has a worktree
  reuses it, and the pane starts there. An agent that creates a branch from
  inside a pane now moves that tab alone, where it used to drag every tab of
  the workspace with it.

- A tab bound to a branch offers "Remove worktree" in its context menu. The
  sidebar could create checkouts but never take one away, so
  `<repo>.worktrees/` grew for the life of a project with no way back. The
  branch is never deleted, and the removal is refused for a checkout holding
  uncommitted changes, one without Paneflow's owner marker, or one that is
  itself an open workspace. Each refusal is a toast rather than a log line.
  The git work runs off the render thread, and the repository's Worktree-scope
  diff hosts are invalidated so a lane appears or disappears without a scope
  toggle or a restart.

- A Customize Sidebar menu on the rail header, with a switch per value: Branch,
  PR, Diffstat and the indent guide, all off by default. A branch that already
  has a pull request swaps its glyph for the PR one, in GitHub's state colors.
  Below them, Expand all / Collapse all, and the fold state of each workspace
  row now survives a restart. This rail work started from @oliviermattei's
  [#46](https://github.com/arthjean/paneflow/pull/46), which proposed returning
  the branch and the diffstat to the rail as an opt-in and tying each tab row
  to its workspace with an indent guide.

### Changed

- The terminal is markedly faster and more responsive, most visibly on Windows.
  Four things were doing redundant work. The runtime thread rebuilt and
  published a full grid snapshot for every output batch, up to a thousand a
  second, where a display shows at most a couple of hundred; it now publishes
  at most one frame per 8 ms, and still publishes the first change after any
  idle gap immediately so keystroke echo is untouched. Programs that bracket a
  screen redraw with synchronized output (DEC 2026) had those half-drawn frames
  published and painted; they are now skipped at the source, the way Ghostty
  itself does it, which removed the reason Windows and macOS were holding every
  wakeup for an extra event-batch window before drawing. A pane whose grid had
  not changed was still fully re-laid-out whenever any other pane produced
  output, so a workspace of parallel agents paid for every pane on every frame;
  each pane now reuses its layout until something in it actually changes. And
  the per-cell contrast correction, which costs a handful of transcendental
  operations per cell, is memoized on the colors instead of recomputed for
  every cell of every frame. On the project's eight-pane render benchmark this
  is 32% off the median input-to-frame time, 26% off its 95th percentile and
  35% off total CPU, in exchange for roughly 1.6 MB of retained layout per open
  pane.

- Windows holds the process timer resolution at 1 ms while the window is open.
  Windows delivers timer expirations on a 15.6 ms clock tick unless a process
  asks for better, and since Windows 10 2004 that default is per-process. Every
  short timeout in the terminal pipeline was silently rounded up to it, which
  capped the update rate and added up to 15 ms of latency to output that had
  already been parsed.

### Fixed

- Quitting an agent leaves you in your own shell. A pane opened for an agent
  started PowerShell with `-NoProfile`, so the shell you got back after the
  agent exited had none of your prompt, aliases, functions or PSReadLine
  setup: it showed a bare `PS C:\dev\project>` and read as if Paneflow had
  launched something other than PowerShell. Windows was the only platform doing
  this, since the zsh, bash and fish paths always loaded the user's rc files.
  It also defeated Paneflow's own prompt integration, which dot-sources after
  `$PROFILE` specifically to wrap a prompt you defined rather than replace it.
  An agent pane now starts the same way as any other, and the `Clear-Host`
  already prefixed to the agent command keeps the TUI's first frame clean.

- The shell picked in Settings > General is honored on Windows. Choosing
  PowerShell stored a bare `pwsh.exe`, which was only ever resolved through
  `PATH`; an app launched from Explorer inherits whatever environment Explorer
  was started with, so a stale or truncated `PATH` silently rejected the choice
  and let the fallback chain pick another shell, occasionally the Command
  Prompt. Each named shell now also resolves from its absolute install
  location, the way the unconfigured fallback already did, and Windows
  PowerShell 5.1 is found under System32 even when its own `PATH` entry is
  missing. Picking PowerShell no longer gets you Windows PowerShell, or the
  other way round: the exact shell you chose is the one that launches.

  PowerShell 7 discovery is resolved once per run instead of once per pane, so
  restoring a many-pane workspace no longer re-walks `ProgramFiles` and `PATH`
  for every pane while the disk is busy. `RUST_LOG=info` now reports the shell
  each pane actually launched next to the configured value.

- The last frame of a program is no longer dropped. The publish rate limit
  applied unconditionally, so a change landing within 8 ms of the previous
  frame waited for the runtime loop, and that loop exits as soon as the child
  is reaped: a program's final output could disappear, and the bigger the
  closing burst, the more of it was lost. The limit now applies only while more
  PTY output is already queued behind the change, and the frame preceding
  `ChildExited` is flushed explicitly on both the POSIX and the Windows
  teardown path. The synchronized-output hold is unaffected, since a torn frame
  is wrong however idle the PTY is.

- The pull request marker works at all. `gh` has no global directory flag, so
  `gh -C <repo> pr list` exited 1 with `unknown shorthand flag: 'C'` on every
  call, and each failure blacklisted the repository for the rest of the
  session because `is_stale` never asks again. The lookup now runs with the
  repository as its working directory, and a `log::debug!` on the failing path
  prints what `gh` said: the caller turns this error into a silent blacklist
  entry, so a wrong invocation used to look exactly like a checkout with no
  GitHub remote.

- Windows verbatim paths no longer break worktree detection. A `\\?\`-prefixed
  path is handed to git as an argument and compared against the paths git
  prints, and the verbatim spelling fails at both: `git worktree add` cannot
  create leading directories under it, and it never compares equal to git's
  forward-slash output, so "is this checkout the repository's own?" answered no
  for every branch on Windows. `strip_verbatim_prefix` now lives once in
  `runtime_paths`, `workspace::git::canonicalize_or` strips too, and the two
  private copies that had grown in the IPC `workspace.create` path and in
  install-method detection call the shared helper.

## [0.10.0] - 2026-08-31

### Added

- Tabs name themselves. A tab opened from the preset picker used to sit in the
  sidebar as a fourth "Claude Code" row; it now names itself after the work
  going on inside it, so a rail of agents says what each one is doing. The
  opening words of the first prompt land immediately as a placeholder, and the
  title the agent's own CLI generates for its resume picker replaces it once
  the CLI has written one (Claude Code today; Codex and Pi generate none and
  keep the placeholder). No model of our own is ever called.

  Renaming a tab yourself turns naming off for that tab for good - a name you
  typed is never overwritten - and "Reset name" in the tab's context menu hands
  it back. A tab split between several agents is left alone, since no single
  one speaks for it.

- Panes show a progress chip in their header when the running program reports
  OSC 9;4 progress: a percentage, `working`, `paused`, or `error`. The chip
  clears when the program removes the indicator or the child exits.

- "Clear scroll history" and "Reset terminal" now have default shortcuts.
  Both actions worked but nothing bound them and no menu offered them, so they
  were unreachable unless you went and bound them by hand. Clearing the
  scrollback is Ctrl+Shift+K on Linux and Windows, Cmd+K on macOS (Cmd+Shift+K
  also works), matching kitty, Ghostty, iTerm2 and Terminal.app. Resetting the
  terminal, which is what recovers a pane wrecked by dumping a binary to it, is
  Ctrl+Shift+R or Cmd+Shift+R.

- Quit now has a keyboard shortcut on every platform: Ctrl+Q on Linux and
  Windows, Cmd+Q on macOS. It was a macOS-only binding before, so there was no
  way to quit Paneflow from the keyboard elsewhere. Note that this takes the
  chord away from the shell in every pane, where bare Ctrl+Q is XON flow
  control (the key that resumes output after Ctrl+S) or readline's
  quoted-insert; rebind or unbind it in Settings > Shortcuts if you need it
  back.

- Images render in the terminal. A program that transmits through the Kitty
  graphics protocol gets its placements painted in the grid, cropped and
  scaled as it asked, under or over the text according to their z-index.
  Image storage is capped at 32 MiB per pane.

- A program can raise a desktop notification with OSC 9 or OSC 777. It is
  suppressed while the window has focus, and the title and body go through the
  same bidi and zero-width strip an agent question does.

- A pointer drag held past the edge of a pane now scrolls the viewport and
  keeps extending the selection, instead of stopping at the last visible row.
  Hold `Alt` while dragging for a rectangular (block) selection.

- **Help > System Info** shows a bug report's environment section and copies
  it with one button: Paneflow version and install format, OS, display server,
  CPU, GPU and driver, renderer, and libghostty version. It says outright when
  the GPU is a software rasterizer, which is the answer to most "why is it
  slow" reports. The panel shows the block before you copy it, because it goes
  into a public issue: no project path, no environment dump, and the bug
  templates now have a slot for it.

- Settings > Shortcuts is nine collapsible groups (Panes, Workspaces, Tabs,
  Terminal, Search, Diff, Markdown, Agents, Application) declared by the action
  registry rather than implied by table order, with an expand and collapse all
  control. A text filter matches the action description and the keystroke, and
  since chords render as Apple HIG glyphs on macOS every entry also carries an
  ASCII spelling covering both readings of `secondary`. A key-capture toggle
  turns the next pressed chord into the filter, which answers "what already owns
  this chord?". "Reset to defaults" now asks before rewriting every binding.

- Agent status is reported without hooks. An organization can disable Claude
  Code's hooks wholesale from managed settings (`disableAllHooks`,
  `allowManagedHooksOnly`), after which the sidebar knew an agent was running
  and nothing else. Two sources already reaching Paneflow are now read: the
  pane's own OSC 9;4 and OSC 777 stream, and Claude Code's session registry at
  `<CLAUDE_CONFIG_DIR|~/.claude>/sessions/<pid>.json`. `AgentStateSource` ranks
  the three writers Terminal < SessionRegistry < Hook, so a weaker source never
  talks over a live stronger one and takes over only after 20 s of its silence.
  Nothing changes on a machine where hooks work.

### Changed

- The Attention Queue moves from Ctrl+Shift+K to Ctrl+Shift+A (Cmd+Shift+A on
  macOS). Ctrl+Shift+K is what every terminal uses to clear the scrollback, and
  that action now claims it. The queue keeps its place in the UI, so the chord
  it gave up is the one that was harder to discover.

- Workspaces can no longer be renamed. A workspace is named after the folder it
  holds, and a title free to drift from that folder left the sidebar claiming a
  name nothing on disk answered to, with no way back. Double-clicking a folder
  row now just selects it.

- The statically linked Ghostty terminal engine is now the only terminal
  engine, on every platform including macOS Apple Silicon. It is always linked
  and always used.

- The pinned `libghostty-vt` archive moves to Ghostty `f2d5758f` built with Zig
  0.16.0 on all three platforms. OSC 7 and the clipboard protocols are now
  decoded by libghostty itself instead of a Paneflow-side router; clipboard
  writes keep the same 100 KiB budget.

- `read_pane` over the MCP bridge returns the screen, not just the scrollback.
  `surface.read` returned the retained history, which stops at the viewport by
  design, so a pane running a full-screen TUI (where every agent CLI lives)
  read as empty. The response is now the history followed by the screen the
  program is currently painting, read through libghostty's formatter so
  soft-wrapped lines rejoin. The two halves do not overlap.

- Clicking a `.md` row in the Files sidebar opens it in the diff dock's editor
  as source, like every other file, instead of spawning a rendered markdown
  pane. The dock's highlighter already carries the markdown grammars. The
  markdown drag-to-pane gesture goes with it: a pane accepts a session drag and
  a pane drag only. Rendered markdown panes still open from a terminal OSC path
  click and still restore from a session.

- The Files sidebar rail is scoped to the tab that opened it. It was mounted
  outside any mode branch, so it survived into Review and Settings where its
  rows open nothing, and a single app-level flag put it in front of every
  sibling tab.

- macOS: the native window material is dropped in fullscreen, which moves the
  window onto its own Space with a black backdrop, leaving AppKit's
  behind-window material nothing to sample and the blur a flat, dead tint.
  Tiled and maximized windows stay in the desktop Space and keep a live blur.

- macOS: `gpui_macos`'s text system is pinned to `error` in the default log
  filter, since it emits per-glyph fallback warnings that flooded the default
  `warn` level during normal rendering. `RUST_LOG=info` still overrides it.

- Building from source now needs Rust 1.98.0, pinned in `rust-toolchain.toml`.
  Shipped binaries are unaffected.

### Removed

- The "Close window" shortcut action is gone. Paneflow only ever opens one
  window, and the action ran exactly the same code as Quit, so it was a second
  name for quitting that no default bound and no menu offered. The window's own
  close button and the window manager still close it.

- The Alacritty terminal backend and the `terminal.backend` setting are gone.
  There is no second parser and no runtime fallback: a `"backend"` key left in
  `paneflow.json` is ignored, and a Ghostty startup failure is now reported in
  the pane instead of silently switching engines. The `--no-default-features`
  recovery build no longer exists either, so Intel Macs (`x86_64-apple-darwin`)
  and Windows ARM64 (`aarch64-pc-windows-msvc`), which have no pinned Ghostty
  archive, can no longer be built.

### Fixed

- Settings > Shortcuts no longer lags with every section open. The page laid
  out all ~90 rows on every repaint, offscreen ones included, and since each
  row highlights on hover, moving the pointer across the list was enough to
  redraw the whole thing. It now renders only the rows the viewport shows, and
  filters when the query or the fold state changes rather than once per frame.
  A repaint of the fully expanded page went from 6.2 ms to 0.3 ms here.

- A configured `scrollback_lines` is honored. The line budget was passed to the
  terminal engine but its byte budget was not, so an 80-column pane pruned at
  roughly a thousand rows whatever the setting said.

- `OSC 4` color queries answer with the active theme. The renderer painted the
  theme while the engine answered from its own built-in palette, so a program
  asking what color 1 is got an answer the screen contradicted. Indexed colors
  written by a program now resolve against the theme too, and follow a theme
  change.

- An `XTGETTCAP` query for the terminal name answers `xterm-256color` instead
  of failing, which is what the PTY exports as `TERM`.

- `CSI 0 q` resets the cursor to the configured shape and blink rather than the
  engine's built-in default.

- Reopening a closed pane restores its scrollback with colors, styling and
  hyperlinks intact, instead of replaying it as plain text.

- Double-click and triple-click selection, and the cell a drag lands on, are
  now resolved by the terminal engine: a drag ending past the middle of a cell
  includes it, as it does in every other terminal.

- The sidebar inline rename accepts keystrokes again, on every platform. Both
  the workspace row and the tab row now host a real text field with a caret,
  selection, IME and clipboard, focused when the rename opens: it commits on
  `Enter` or when the focus leaves it, cancels on `Escape`, and hands the focus
  back to the active pane. Refs #32.

- Rebinding a shortcut was broken on every platform. A recorded chord was
  persisted through `Keystroke::to_string()`, the `Display` impl, rather than
  the `-`-separated syntax `Keystroke::parse` reads back, so a rebind stored a
  chord no keypress could match while displacing the default it replaced. A
  binding you rebound in an earlier release is still stored broken: rebind it
  once to fix it.

- A pane no longer inherits the host terminal's identity. `CommandBuilder` seeds
  a pane's environment from Paneflow's own and removing a key from the assembled
  map cannot unset an inherited name, so only an `env_remove` at the spawn
  boundary fixes it. An inherited `WT_SESSION` makes Claude Code disable OSC 9;4
  outright and an inherited `TMUX` makes it wrap notifications in multiplexer
  passthrough libghostty does not unwrap, so launching Paneflow from Windows
  Terminal or from inside tmux silently lost both channels in every pane. The
  launching agent session's `CLAUDE_CODE_SESSION_ID` leaked the same way.

- The detached diff dock is keyed on the tab, not the workspace. Two tabs of the
  same folder shared one dock, so opening it in one tab opened it in its
  sibling, with that sibling's tabs and last diff snapshot. Closing a background
  tab also leaked the parked dock and the terminals it held.

- The diff dock clamps its width to the room the main panel has. Opening a right
  rail narrowed the panel under a dock sized for the wide one and the dock
  pushed its right edge past the panel's clip. The stored width is now a
  preference fitted into the live width, and nothing is written back, so the
  dock returns to full width when the rail closes.

- The mouse encoder no longer clears its motion deduplication on every event, so
  a program in mouse-tracking mode stops receiving redundant motion reports.

## [0.9.0] - 2026-08-26

### Added

- A tab level between the workspace and the layout. A workspace owns a list of
  tabs, each carrying the layout tree the workspace used to carry, with per-tab
  zoom. The CLI sidebar workspace row becomes a collapsible folder and each tab
  a child row with inline rename, hover actions and reordering by id. New
  bindings `secondary-]` (`next_tab`) and `secondary-[` (`previous_tab`);
  `secondary-alt-t`, `secondary-w` and `secondary-tab` keep their meaning.
  Workspace tabs cap at 32.
- Editable `File` tabs in the diff dock, backed by a `ropey::Rope` loaded
  off-thread, incremental tree-sitter highlighting shared with the diff, native
  input and IME, undo and redo, save with a modified marker, and a conflict path
  for a file an agent rewrites underneath. File tabs cap at 8. New bindings
  `secondary-g` (file tab) and `secondary-j` (terminal tab), scoped off shells
  and text surfaces.
- The diff dock is reachable from a CLI pane. A `git-pull-request` button in the
  pane header toggles it on the pane's workspace root.
- The files sidebar opens any file, not just markdown, and filters as you type.
  Rows the editor would refuse stay dimmed but clickable, so the refusal is
  stated inside the tab.
- Theme presets: Paneflow, Vercel, Claude and Cursor, each in a light and a dark
  variant. `theme` stores the resolved variant; pre-preset names still resolve
  through an alias table. Settings, Themes leads with three full-bleed window
  mockups and a live terminal sample painted from the active theme.
- New `reduce_motion` config key (Settings, Themes, Preferences). When enabled,
  hover transitions settle instantly and the primary sidebar toggles without the
  slide.
- Panes that do not hold focus now fade to 70% opacity when a workspace holds
  more than one pane. Tune or disable it with `unfocused_pane_opacity` (`0.15`
  to `1.0`, `1.0` disables). The tab bar, attention glow, broadcast stripe and
  Composer stay at full contrast.
- `surface.*` IPC methods export a stable `workspace_id` and accept it as an
  optional parameter; a surface outside the requested workspace is rejected with
  `invalid_params`. Omitting the parameter keeps the previous behavior.
- `list_panes` over the MCP bridge names the holding tab.
- CLI panes float as continuous-corner cards, with matching row skins and
  delayed tooltips. A Linux-only application icon ships alongside.

### Changed

- Session schema v2. A workspace session carries a list of tab sessions rather
  than a single layout tree. v1 files are migrated on load, not rejected.
- A pane holds one surface. The in-pane tab strip gives way to a card header
  with the surface name, the agent pill and the actions.
- `BoundedOutput` fails with `ProcError::OutputLimitExceeded` instead of
  returning partial data with a truncation flag. Editors and file managers
  launched from Paneflow are spawned detached, backed on Windows by a job
  object.
- Find-in-buffer in the terminal is chunked, cancellable and budgeted, and the
  agent identity is declared at launch instead of discovered by scanning.
- GPUI is pinned back to upstream `zed-industries/zed` and the Paneflow fork is
  retired; only `gpui` and `gpui_platform` remain declared. Affects builds from
  source only.

### Removed

- The Agents view, its sidebar, bottom panel, view actions and project store.
  The mode switch drops to CLI and Review, and the `secondary-shift-a` binding
  is removed. A `session.json` from an older build restores in CLI mode with its
  workspaces intact. The CLI mode tab is now named "Agents".
- The agent identity pill and the Files sidebar button from the pane header,
  Rename from the workspace context menu, word-level intra-line highlighting in
  the diff, the `paneflow-acp` crate, and the Zed markdown global-theme
  bootstrap.

### Fixed

- Windows: the title bar minimum height matches the Win11 caption strip.
- Codex 0.149.1 user turns are read correctly and subagent rollouts are dropped.
- Synthetic Claude records stay out of sidebar titles, project slugs with
  trailing separators are normalized, and bound Claude sessions are resumed
  instead of re-minted.
- The launching agent session's environment markers are stripped from child
  PTYs, and every conflict-watcher wake in the code editor stays on the view's
  thread.
- The branches popover stays anchored while its list scrolls, the delete and
  clear icons paint instead of leaving blank space, and the tab-cycling chord
  assertion is platform-aware.

### Security

- Telemetry capture is gated behind a closed event schema. `TelemetryEvent` is
  now the only way to name an event or attach properties and the client owns the
  reserved keys, so the no-PII rule is held by the type system rather than by
  review. The queue is bounded on event count and serialized bytes, with an
  explicit shutdown flush deadline.
- The agent-config lease ownership bit is stored outside the locked file.

## [0.8.1] - 2026-07-21

### Changed

- Windows 10 1809+ and Windows 11 x64 builds now select the statically linked
  Ghostty terminal engine for new sessions when `terminal.backend` is `auto`.
  Explicit `ghostty` remains available for diagnosis and `alacritty` remains
  the immediate rollback. A Ghostty startup failure can fall back once before
  child spawn; live sessions never switch backend. Linux keeps its existing
  Ghostty selection, macOS remains on Alacritty, and persisted workspace and
  session formats are unchanged.

## [0.6.2] - 2026-06-24

Patch release focused on Windows trust and cleaner agent terminal startup.

### Changed

- Windows release builds now Authenticode-sign `paneflow.exe` before packaging
  the MSI, then sign the MSI as well. This makes the installed executable
  verifiable by Smart App Control instead of relying only on installer trust.
- Agent panes spawned through `paneflow up`, `paneflow flow`, and IPC now carry
  an explicit terminal surface profile into the app.
- PowerShell agent panes now start with `-NoProfile`, avoiding user profile
  startup noise while preserving normal PowerShell behavior for regular panes.

## [0.6.1] - 2026-06-24

Patch release focused on keeping long-running multi-agent work leaner and less
surprising after the Paneflow Conductor release. It also closes two macOS/UI
paper cuts from the issue #11 feedback loop and polishes toast feedback across
the app.

### Added

- Memory-oriented terminal surface profiles. Normal terminals keep the existing
  10,000-line scrollback default, while agent, Review, and cold cached terminal
  surfaces are capped at 4,000, 2,000, and 1,000 lines respectively.
- A memory smoke-test runbook covering the 6-8 agent workload, Review and
  Agents diff navigation, IPC bursts, and the OS verification gaps that must be
  called out explicitly.

### Changed

- Multi-agent retention is now structurally bounded instead of relying on
  process memory staying friendly. Agent terminal hot cache, bottom terminal
  retention, session sidebar rows, attribution matches, closed-pane scrollback,
  diff rows, raw diff file reads, and GPUI-bound IPC requests now have explicit
  caps.
- Hidden Review columns and closed Agents diff panels release their loaded row
  models, display caches, offsets, attribution data, and exited review-terminal
  references. Running review terminals are protected: Paneflow asks you to close
  them before hiding the column instead of silently killing work.
- IPC requests headed for the GPUI thread now use a bounded queue with
  backpressure, and each UI tick drains a bounded number of live and cancelled
  requests. Busy clients get a clear retryable overload error instead of
  letting request memory grow without a cap.
- Notification toasts use icon assets, tighter sizing, clearer action buttons,
  and error-style detection for failure messages.

### Fixed

- The sidebar's CLI / Review / Agents switch now stays visible as persistent
  primary navigation, with Settings moved to a compact utility button. Switching
  modes also closes the Settings popover so the footer state stays predictable.
- macOS GUI launches that inherit the filesystem root no longer create a fresh
  implicit workspace at `/` with a generic `Terminal 1` label. New implicit
  launches fall back to the home directory, and legacy restored `Terminal N`
  root workspaces are repaired without affecting explicitly requested root
  terminals.
- macOS native menu items stay enabled across CLI, Review, and Agents modes by
  registering app-global fallback handlers for the menu actions.
- Linux shim size budgets were rebaselined after the release-min helper binary
  grew from the conductor work, keeping CI focused on real size regressions.

## [0.6.0] - 2026-06-21

Paneflow Conductor. Paneflow becomes a control plane for a fleet of CLI coding
agents running side by side in panes. A new public `paneflow` CLI discovers,
reads, drives, and waits on those agents over the local IPC socket, never by
scraping the screen, and a harness-agnostic conductor SKILL lets any agent
(Claude Code, Codex, OpenCode, Gemini, ...) drive the others. Cross-platform:
Linux, macOS, and Windows over the named-pipe transport.

### Added

- The `paneflow` fleet CLI. `ps` / `ls` discover the running agents and the
  panes themselves; `status` / `read` / `search` inspect one agent's live state
  and scrollback; `send` dispatches a prompt (pre-filled by default, `--submit`
  to auto-submit, `--broadcast` to fan out across matching panes); `wait` and
  `watch` block on events instead of polling; `up` spawns agents from a
  declarative `paneflow.workspace.toml`; and `flow run` executes a declarative
  spawn -> wait -> feed -> review pipeline. Target any pane by `surface_id`,
  name, `cmdline:<substr>`, or `cwd:<path>`. The MCP tool names (`list_panes` /
  `read_pane` / `search_pane`) are accepted as aliases, and a genuinely unknown
  verb exits non-zero with an actionable error instead of opening a stray GUI
  window.
- Hooked agents with an authoritative state machine. Agents launched through
  `up` / `flow` (or carrying a hook integration) are tracked turn by turn: their
  `state` (`thinking`, `waiting_for_input`, `finished`, `errored`, `stalled`,
  `idle`, or `unknown_running`) is real, their `ai.stop` / `ai.notification`
  events fire, and their `last_result` is exposed. A self-diagnosing hook
  installer reports `hooked` plus a `reason` in the fleet, so an agent that was
  only spotted by a process scan reads `unknown_running` rather than faking a
  derived state.
- An outbound IPC event bus. `events.subscribe` streams `ai.*` transitions and
  surface changes, which `paneflow watch` renders as one JSON event per line
  with a 30s heartbeat. `wait --idle` blocks on output quiescence via the
  monotonic `output_generation` counter; `wait --pattern` blocks on a
  baseline-aware sentinel (it matches output produced after the wait starts, not
  the prompt echo); `--all` / `--any` gate across many panes at once.
- Deterministic auto-submit. `send --submit` wraps the prompt in bracketed paste
  and sends the carriage return as a separate, calibrated write, then confirms a
  hooked agent's turn actually started via `output_generation`. If no turn start
  is confirmed it exits non-zero instead of returning a false `submitted:true`.
- Structured inter-agent context. A turn's `last_result` is backfilled
  off-thread from the Stop-hook transcript, so a full-screen (alt-screen) TUI's
  report is captured in full rather than truncated to the viewport, and
  `send --report-file <path>` appends a precise file contract so an agent writes
  its complete report to disk and prints `REPORT_DONE <path>`.
- AI access controls under Settings > AI Agent. An `ai_unrestricted` free-access
  toggle sanctions CLI auto-submit and traced writes; an `ai_injection_fence`
  wraps `read` output in an `<untrusted_terminal_output>` fence (drop it with
  `read --raw` only when you trust the source). Writes are refused with a clear,
  actionable error unless free access is on or `PANEFLOW_IPC_SCRIPTING=1` is set.
- The harness-agnostic **Paneflow conductor** SKILL: a shell-only playbook for
  driving the fleet (discover -> read -> wait -> dispatch -> hand back), plus a
  committed `examples/review-pipeline.flow.toml` worked cross-vendor
  implement -> review pipeline to copy from.

### Changed

- The event bus streams over Windows named pipes through a `PeekNamedPipe`
  liveness guard, falling back to a bounded `output_generation` clock where a
  transport cannot tick subscriptions. The `paneflow` client resolves the
  build-profile socket path and honors `PANEFLOW_SOCKET_PATH`.
- The inter-agent context blob is written owner-only (0600/0700). The
  Thinking-stuck watchdog tightened to 60s, and a pane's label is applied
  atomically at spawn so it never flashes the generic agent name first.
- Em dashes were normalized to ASCII hyphens across the repo. A new `ABOUT.md`
  project overview landed, and the fleet / events / flow control-plane surface is
  documented under `docs/`.

### Fixed

- macOS: the parent-death reaper is guarded against PID reuse, and the shim adds
  an orphan guard (Windows gets a Ctrl+C `ai.stop`).
- A stale persistent hook is now ignored in favor of a project-local install.
- The Agents environment toolbar no longer covers the embedded CLI (a top band
  is reserved), and per-file horizontal scroll is restored in the diff body.
- Windows: the console-subsystem binary is kept (only the lonely GUI console is
  shed), the workspace test suite passes natively, and the macOS DMG and process
  unit tests are made cross-platform.

## [0.5.9] - 2026-06-18

A review-workflow release. The Agents diff dock and the Review view now render
through one shared diff pipeline, the review loop is fully keyboard-driven, and
the Review attribution badge can show which agent wrote a change alongside an
estimated, fully local token cost.

### Added

- Keyboard-first review loop. `]` / `[` jump between hunks, `u` toggles the
  unified/split view, `s` toggles cross-column scroll sync, `Esc` dismisses,
  and `a` acts on the focused hunk. Bindings are scoped to
  `DiffView && !Terminal && !TextInput` so an embedded review or shell terminal
  and the base-branch filter input keep their own keystrokes, and they are
  remappable through the action registry.
- Per-hunk act-on-hunk actions in the Review view, with prompts pre-filled into
  a freshly launched review CLI rather than auto-submitted.
- Agent attribution and estimated cost on the Review badge. Per-session token
  usage is folded across assistant turns by the Claude Code, Codex, and
  OpenCode scanners, and a build-time-embedded, versioned pricing table turns
  it into an estimated cost. It is 100% local with no network lookup; unknown
  models show their token counts with no fabricated cost.
- A `review_prefill_delay_ms` setting (default 2000 ms, clamped to
  [250, 10000]) with a `-` / `+` stepper under Settings > AI Agent > Review,
  tuning how long Paneflow waits before auto-typing a prompt into a freshly
  launched review CLI. The clipboard fallback keeps any value safe.

### Changed

- The Agents diff dock now renders through the same `DiffElement`, git pipeline,
  and row model as the Review view. The bespoke horizontal-scroll state was
  replaced by a single shared scroll handle, and the monolithic diff view was
  split into focused submodules (loader, scroller, interaction, review,
  attribution, render).
- New-chat thread titles are now derived from the on-disk session ai-title
  instead of staying on the generic agent label. Each Claude thread is bound to
  a forced `claude --session-id <uuid>` minted at creation so it maps 1:1 to its
  session file (resuming the same id appends, so a restart continues the same
  session); at turn end the polished ai-title is backfilled into the sidebar row
  off the main thread. A manual rename locks the title against later OSC updates
  and backfills. Every session id is re-validated before it reaches the command
  line, so a tampered `session.json` cannot inject an argument.

## [0.5.8] - 2026-06-17

An Agents sidebar cleanup. Thread status is now driven solely by the agent
hook lifecycle (Claude Code / Codex shims), removing the output-activity
heuristic that lit a "thinking" spinner from raw PTY traffic and produced
false positives. The environment panel also sits flush against the right edge.

### Changed

- Agents thread status now comes only from `ai.*` hook frames. The fallback
  heuristic that inferred "thinking" from PTY output bursts (for agents without
  a hook integration, such as OpenCode, Pi, and Hermes) is gone: it lit false
  spinners on dev-server output streaming under a bare-shell thread and on TUI
  redraws, and never matched the precise hook lifecycle that the Claude Code and
  Codex shims already provide.

### Fixed

- The Agents environment panel now sits flush against the right edge, tightened
  from a 38px to a 12px inset now that nothing reserves the gutter.

## [0.5.7] - 2026-06-17

A macOS reliability pass. The headline is the DMG self-updater, which froze
on every attempt because the codesign Team-ID pin silently failed; this also
relights the workspace agent dot, resolves bare configured shells under a GUI
launch, and stops a spurious "shell may have exited" warning. The pid-0 guard
lands on Linux too; the rest is macOS-only or dev-only.

### Fixed

- The DMG self-updater no longer freezes on macOS. The codesign Team-ID pin
  passed its requirement as a separate `-R <req>` argument, which macOS 15+/26
  read as a *file path*: codesign tried to open the inline requirement text as
  a file and aborted, so every DMG update failed and the updater stalled at the
  three-strikes "Update keeps failing" toast. The requirement now uses the
  attached `-R=<req>` form (a single argv element) that every supported macOS
  parses as inline requirement source.
- The workspace card now lights its agent dot on macOS again. `proc_listchildpids`
  returns zero children for an unprivileged caller on modern macOS, so the
  per-node subtree walk found nothing. Agent detection now builds a
  parent-to-children map from `proc_bsdinfo.pbi_ppid` once per scan and walks
  it breadth-first, mirroring the existing Linux fallback.
- A bare configured shell name (e.g. `"pwsh"`) is now resolved under a GUI
  launch whose inherited PATH omits `/opt/homebrew/bin`, instead of silently
  falling back to `/bin/sh`. After the PATH search misses, Paneflow probes the
  well-known Unix install dirs (Homebrew prefixes and system dirs), the macOS
  parallel to the Windows well-known-location probe.
- Display-only terminals no longer probe a bogus process. A display-only pane
  has no real PTY (`child_pid == 0`); on Linux that meant reading `/proc/0/cwd`,
  and on macOS `proc_pidinfo(0, …)` targeted the kernel swapper, failed with
  EPERM, and spammed a misleading "shell may have exited" warning on every poll
  tick. The cwd probe now bails before the syscall, matching the existing
  foreground-command guards on every platform.
- Debug builds no longer warn about running outside a `.app` bundle. A
  `target/debug/` binary is never inside a bundle (the expected dev path), so
  that message is now logged at debug level in debug builds; release binaries
  running outside a bundle still warn, since that is a genuine ad-hoc extraction
  worth surfacing.

## [0.5.6] - 2026-06-16

The Agents git diff dock becomes resizable and scrolls each file on its own,
and every diff surface in the app now draws from one color source.
Cross-platform.

### Added

- The Agents git diff dock is now resizable: drag its left edge to widen or
  narrow it. The width is clamped between a readable floor and a
  window-friendly ceiling so the dock can never swallow the terminal column or
  shrink below a usable code width.
- Each file in the Agents diff dock now scrolls horizontally on its own, so a
  long line in one file no longer drags the short files into the blank. Files
  that overflow grow a per-file horizontal scrollbar (click the track or drag
  the thumb) and accept horizontal wheel scrolling, while vertical scrolling
  stays shared across the dock.

### Changed

- Every diff surface now reads its +/- colors from a single shared source:
  Codex green/red on dark themes, the theme's version-control colors on light
  themes. The Agents diff dock, the Diff/Review view, the CLI workspace sidebar
  diffstat and the diff sidebar previously each inlined their own hex and could
  drift apart; they are now guaranteed to match.
- The Agents environment toolbar's editor split-button now shares the rounded
  radius of the toggle buttons beside it.

## [0.5.5] - 2026-06-16

The Agents view gains a Codex-style environment surface, and the CLI tab strip
is restyled to match it. Cross-platform.

### Added

- An environment card in the Agents view. It carries a per-repository git branch
  picker (a live-filtered, focus-trapped search field that also names a new
  branch) and an external-editor selector that reuses the same editor list and
  logos as the General settings tab. The card is scoped to the active thread's
  working directory, so project threads and free chats can each point at their
  own repository.
- A full-width bottom terminal dock in the Agents view, toggled from the
  environment toolbar. It hosts a tab strip of shell terminals: open as many as
  you like with `+`, close each one independently, and drag the dock's top edge
  to resize it. Every terminal is a real PTY whose scrollback and I/O survive tab
  switches and closing or reopening the dock, so coming back is always warm.
- A right-side git diff dock in the Agents view. It renders an off-thread diff
  snapshot for the active thread's working directory, with a unified or
  side-by-side split view, per-file fold state that survives re-renders, and an
  uncommitted-files count surfaced from `git diff --shortstat`.

### Changed

- The CLI multiplexer's terminal tabs are now floating, rounded chips instead of
  full-height bordered tabs. The active chip lifts on a whisper of the text
  color, inactive chips wash in on hover, and the chrome separators are gone, so
  the strip melts into the terminal body and speaks the same tab language as the
  new Agents bottom dock.

## [0.5.4] - 2026-06-16

A visual polish pass on the app chrome plus two Windows session fixes. The
chrome refresh lands on every platform; the session and title-bar fixes are
Windows-only.

### Fixed

- The agent-sessions sidebar now populates on Windows. Claude Code, Codex and
  opencode sessions for the open workspace were never listed because three
  things were wrong at once: the project-directory slug kept the drive
  letter's `:` (so `C:\dev\paneflow` looked for `C:-dev-paneflow` instead of
  the real `C--dev-paneflow`), the working-directory filter was case- and
  separator-sensitive, and the active terminal's cwd was never seeded on
  Windows. All three are fixed, so the sidebar resolves the same sessions your
  agent CLIs actually wrote.
- Terminal tabs and Agents threads no longer take the shell's own path as
  their name on Windows. PowerShell and cmd briefly title their window with
  their executable path (e.g. `C:\Program Files\PowerShell\7\pwsh.exe`) before
  your profile runs; PaneFlow now ignores a title that is merely a path to an
  `.exe` and keeps the real label.

### Changed

- A chrome refresh across the sidebars, title bar, context menus and settings.
  Hovered and selected rows now share one slightly brighter translucent
  material (closer to Codex/OpenAI's soft highlights), drop-shadows are gone
  for a flatter look, and the docked sessions and files rails use the same
  native window material as the rest of the app instead of a flat dark fill.
  Corner radii are unified across cards, rows and settings controls.
- Quieter logs. A failed update check from a transient network or GitHub
  hiccup, and a diff column superseded by a newer load, now log at debug
  instead of warn; only an actionable update failure (a persistent 4xx) still
  warns.

## [0.5.3] - 2026-06-15

A Windows quality pass: new terminals now open in the right directory, the
font picker is wired end-to-end, and two stray-window/log annoyances are gone.
No changes on Linux or macOS.

### Added

- Font picker on Windows. The Settings font list was empty on Windows because
  family enumeration was never implemented; it now enumerates installed
  fixed-pitch families via GDI (`EnumFontFamiliesExW`), alongside the fonts
  PaneFlow embeds. GDI is used only for discovery; GPUI/DirectWrite still does
  the rendering.
- Cascadia Mono as the Windows default font. A fresh install now defaults to
  the system Cascadia Mono, matching Windows Terminal, instead of the embedded
  IBM Plex Mono. Linux and macOS still default to the embedded mono, which also
  stays available everywhere as the fallback. Pick any installed font (or
  return to the default) from the Settings list.

### Changed

- The font-family picker moved from the Themes page to the Terminal page, next
  to font size, line height and ligatures. Searching "font" in Settings now
  jumps to the Terminal page, and the Themes page is theme-only.

### Fixed

- New terminals open in the workspace directory on Windows. Opening a new tab,
  splitting a pane, or duplicating a tab spawned the shell in
  `C:\Program Files\PaneFlow` (the install directory) instead of the project
  folder, because Windows can't introspect a child process's working
  directory. New panes now fall back to the workspace's own root, so every new
  terminal lands where you'd expect.
- No more console window flashing on Windows. Background helpers PaneFlow runs
  (git status polling, agent CLIs, MCP probes) each briefly popped an empty
  console window; they now spawn with `CREATE_NO_WINDOW`.
- No more spurious warning when a Windows shell closes. Typing `exit` logged a
  harmless-but-noisy `TerminateProcess failed` warning on every shell close;
  PaneFlow now detects the already-exited child and skips the kill path.

## [0.5.2] - 2026-06-15

A Windows hotfix: the in-app updater now works on MSI installs. No changes on
Linux or macOS.

### Fixed

- Windows self-update. Clicking "Update" on an MSI install failed with "HOME
  environment variable is not set" and never updated. The running binary's
  install location was misdetected - `std::fs::canonicalize` returns the
  extended-length `\\?\C:\…` path on Windows, which did not match
  `%ProgramFiles%`, so the install was classified as unknown and the updater
  fell back to the Linux tar.gz path (which reads `$HOME`). MSI installs are
  now detected correctly and the update runs through msiexec end-to-end. As a
  safety net, an unknown install on Windows no longer routes to the Linux
  updater either.

  Note: because the currently-running build carries the old, broken detection,
  it cannot self-update to this fix - install the 0.5.2 `.msi` manually once
  from the releases page, and the in-app updater will work for every release
  after it.

## [0.5.1] - 2026-06-15

A Windows polish patch on top of 0.5.0: the app and installer now carry the
right icon, and the stray console window is gone. No changes on Linux or macOS.

### Fixed

- No more stray console window on Windows. paneflow.exe is now built as a
  GUI-subsystem binary, so launching it from Explorer, a shortcut or the Start
  Menu no longer opens an empty extra terminal window beside the app. The
  scriptable CLI (paneflow mcp install, paneflow ls, --version, …) still works:
  the process re-attaches to the parent console when started from a terminal.
- The paneflow.exe icon in Explorer. The bare executable embedded no Windows
  resource and fell back to the generic Windows icon; it now ships the same
  multi-resolution PaneFlow icon as the installer.
- The Windows installer icon. The 0.5.0 MSI still showed the old logo on its
  Start Menu shortcut and Add-or-Remove-Programs entry - the WiX icon was the
  one output the new-logo regeneration had missed. It is now regenerated from
  the new logo, and the icon pipeline mirrors it on every run so it can no
  longer go stale.

### Documentation

- Refreshed the Windows install docs for the signed v0.5.0 .msi: the native
  installer is now documented as an available path (WSL2 kept as the
  alternative), with a SmartScreen "Run anyway" walkthrough (publisher:
  StriveX) and signature-verification steps, replacing the stale "no native
  build / Q3 2026" framing across the docs.

## [0.5.0] - 2026-06-15

This release brings Paneflow to Windows and lands a ground-up redesign of the
app shell.

### Added

- Windows support. Paneflow now runs on Windows 10 and 11. The title bar
  carries native Windows 11 caption buttons and a full-width inset panel, new
  terminals default to PowerShell, and live agent-status updates are delivered
  reliably over named pipes.
- Inline settings. The settings window is replaced by a Codex-style settings
  surface embedded directly in the app, built on a shared set of select,
  toggle and card primitives, with every page rebuilt on those controls.
- The PaneFlow Light theme returns, paired with a light app shell, and the
  window backdrop now seeds itself from the active theme mode.
- Configurable font fallbacks. A user-editable font_fallbacks list lets you
  control the monospace fallback chain.

### Changed

- Cockpit chrome redesign. A reworked window chrome with a native backdrop,
  title-bar Files and Help menus, a Profile menu, and a sidebar toggle. The
  title bar now spans the full window width on every desktop platform.
- One menu language across the app. The title-bar dropdowns, the workspace and
  agents context menus, the theme picker, and the diff scope, project, branch
  and base pickers all share a single elevated surface and select-row style.
- The agent launcher is laid out as a grid of filled tiles, and the agents
  sidebar search field matches the settings search pill.
- The About dialog is restyled as a native app dialog, and hover backgrounds
  align with the active selected state.
- The option-as-meta default is now platform specific.

### Fixed

- Self-update reliability across platforms: the macOS app bundle relaunches
  correctly and handles translocation, AppImage installs are detected via
  $APPIMAGE with the right package-manager routing, the Fedora upgrade path
  refreshes its metadata first, and a mismatched-signature install surfaces a
  clearer hint.
- Terminal teardown is guarded against PID reuse and works on kernels built
  without CONFIG_PROC_CHILDREN.
- The GUI now adopts the login-shell PATH on launch, so tools on your shell
  PATH are found when Paneflow is started from a launcher.
- Turn-end desktop notifications carry the Paneflow icon, and widget text
  keybindings are re-registered on every keymap apply.
- Linux packages depend on fontconfig so the settings font picker is
  populated.

## [0.4.4] - 2026-06-11

### Changed

- The in-pane find bar is now a real editable field. It hosts the same text
  input the agent sidebar uses, so opening a search puts a live caret in the
  field with selection, IME and clipboard support, and the query updates the
  match list as you type. Its chrome follows the active theme (One Dark /
  PaneFlow Light) instead of a fixed palette, with search, regex, fleet,
  previous, next and close controls, and a status line that reads the match
  position, an empty result, or an invalid pattern.
- Every agent other than Claude Code now shows the same rotating arc the agent
  sidebar uses while it is thinking, in a soft neutral grey, replacing the
  Codex-style pulsing dots. Claude Code keeps its own glyph spinner and salmon
  identity colour.

## [0.4.3] - 2026-06-11

### Added

- Composer: a bottom-anchored multi-line input (secondary-shift-space) over
  the focused pane. Enter pre-fills the agent through bracketed paste
  without submitting, so the prompt is yours to review before it is sent;
  secondary-enter pre-fills and submits in one keystroke.
- Named broadcast groups: assign panes to a group (secondary-shift-b to
  toggle membership, secondary-shift-m for the picker), each marked by a
  3px coloured edge stripe. The Composer fans one prompt out to every live
  member of the active group and shows a transient recap of who received
  it, so a broadcast is never silent.
- Queued prompts for busy agents: a prompt sent to a generating agent is
  held ("1 queued" tab chip) and flushed automatically on that session's
  next idle transition, instead of being dropped or spliced into the
  running turn.
- Attention Queue (secondary-shift-k): a single overlay listing every agent
  waiting for input across all workspaces, with its question and how long
  it has waited, longest-waiting first. Enter warps straight to that pane.
- Launch Pad (secondary-shift-l): worktree, split, agent launch and
  first-prompt prefill in one gesture.
- Agent status beyond Claude Code and Codex: the sidebar states, tab dots
  and notifications now apply to any agent CLI launched through the shimmed
  PATH, identified by its binary name; an unrecognized tool is reported as
  itself instead of being mislabeled as Claude.
- Scrollbar match rail: an active search projects every match as a tick on
  the scrollbar track (decimated to the pixel grid, so 10 000 matches cost
  the same as 10), with the existing proportional track click to jump.
- Fleet grep: from any pane's find bar, the "Fleet" toggle (or Alt+F) runs
  the same query across every pane of every workspace off the render
  thread, lists the matching panes with counts, flashes a transient match
  badge on their tabs, and Enter teleports with the local search pre-armed.
- Per-pane font zoom: Ctrl+= / Ctrl+- / Ctrl+0 (Cmd on macOS) change the
  focused pane's font size by 1 px steps within 8-32 px, with the PTY grid
  reflowing like a window resize. Persisted per pane across restarts;
  panes without an override keep following the global setting.

- Fleet observability: the port/process scan now attributes results to each
  pane. Tabs show a compact identity pill for the agent CLI running inside
  (PID-detected across 16 known agents, persisted across restarts as a
  dimmed "last known" until confirmed) and per-pane port badges, clickable
  when the port belongs to a frontend dev server. When a pane announces a
  URL whose port is actually owned by another pane, its badge turns into an
  alert naming the owner.

- Errored agent state: when an agent CLI launched through the shimmed PATH
  exits non-zero, its session turns red (tab dot + sidebar badge) and the
  desktop notification says "agent exited (exit N)" instead of a false
  "agent finished". Human interrupts (Ctrl+C, pane close, external kill)
  still count as finished, never as errors.
- Stalled agent detection (on by default): a thinking agent that emits no
  hook activity for 5 minutes is flagged "stalled" in the sidebar, with one
  desktop notification per stall episode. Threshold configurable via
  `agent_stall_threshold_secs`; kill switch via `agent_stall_detection`.

### Changed

- Dev-server detection is now OS-authoritative. A port badge's clickable
  link is derived from the command line of the process that owns the
  socket, so it no longer depends on catching the dev server's banner line
  in the terminal output. The link survives an in-shell restart (nodemon, a
  plain re-run) that re-binds the port, and sustained agent output no longer
  starves the scan that picks up new ports.

### Fixed

- Agent sessions are reaped the moment their pane closes instead of
  lingering up to 30s for the periodic sweep, covering the cases where the
  exit hook never arrives (shim killed, agent started without the shim).
- A recycled process id can no longer keep a finished agent's status alive:
  a session pins its process start time, and a reused pid whose start time
  differs is treated as gone.

## [0.4.2] - 2026-06-10

### Changed

- New logo artwork. Every icon size (16-512, master 1024, .icns, .ico) is
  regenerated with a transparent keyline margin: the squircle body is
  rendered at ~80% of the canvas, the value GNOME and macOS icon grids
  converge on, so the icon no longer renders oversized next to
  spec-compliant peers in the GNOME Shell dash and macOS dock.

## [0.4.1] - 2026-06-10

### Added

- Live activity indicator on Agents thread rows: a row whose agent is
  working shows a Codex-style spinner, driven by the same `ai.*` signals as
  the pane badges.

### Changed

- Agents panel polish: stronger selected-row contrast against the rail, a
  faint hairline between rail and panel, and a 16px panel corner radius
  matching the Cli/Diff silhouette.

## [0.4.0] - 2026-06-10

### Added

- `paneflow` CLI: a scriptable control plane over the IPC socket. `ls`,
  `read` and `search` expose pane scrollback with a unified target selector;
  `new`, `select`, `split` and `focus` drive the layout; `send` feeds text to
  a pane behind a scripting gate and never auto-submits; `key` sends a single
  non-submitting keystroke; `wait` blocks on pane readiness as an
  orchestration primitive.
- `paneflow up`: declarative agent workspaces. One command builds a
  workspace from a spec (per-pane cwd, agent to launch, prompt prefill),
  backed by a `workspace.up` IPC handler.
- Worktree-per-agent: a `worktree = "branch"` field in `up` gives each pane
  its own git worktree, with `.env*` copy, an optional setup command, a
  `${port_offset}` variable for port isolation, clean teardown when the
  workspace closes and pruning of orphaned worktrees at startup.
- `paneflow flow`: a flow engine for multi-agent pipelines. `flow.toml`
  declares spawn steps with `ready.pattern` barriers, gated send steps,
  `foreach` fan-out and fan-in, `capture` to pass data between steps, plus
  validation with cycle detection, `--dry-run`, reporting, exit codes and
  state resume. Submission stays double-gated end to end.
- Attention routing: a pane whose agent waits for input glows and its tab
  shows an attention dot; the desktop notification carries the agent's
  question; `Ctrl+Shift+J` cycles to the next waiting agent across
  workspaces; hovering the pane badge peeks at the question without
  stealing focus.
- Persistent agent-notification hooks: `paneflow hooks setup` installs a
  durable hook for supported agents, `paneflow hooks status` reports each
  agent honestly, and the launch shim defers to a persistent hook instead
  of injecting an ephemeral one.
- Turn-end desktop notification when the window is unfocused.

### Changed

- Agents view rebuilt as a Codex-style cockpit: rail sections (Search,
  Pinned, Projects, Chats), free chats anchored to the home directory, a
  contextual top bar with a thread overflow menu, and unified
  selection/empty states.
- Cockpit chrome across every mode: full-height rails with a floating
  rail-confined title bar, the update call-to-action moved into the sidebar
  in Cli/Agents, a single-row Diff toolbar with the scope breadcrumb
  inline, and quieter text inputs (1px white caret).
- The sessions sidebar now follows the active workspace instead of staying
  bound to the previous one.
- "PaneFlow Light" is temporarily out of the bundled theme set pending a
  light-theme redesign; configs naming it fall back to One Dark.

### Fixed

- A literal `--update-and-exit` token passed as a CLI or hooks argument can
  no longer hijack the process into the self-updater.

## [0.3.9] - 2026-06-09

- Rebuilt the native terminal engine on upstream `alacritty_terminal` with
  rendering parity: OSC 8 hyperlinks, configurable cursor shapes, a live
  scrollbar, and faithful cursor and alt-screen input handling.
- Added PTY teardown and exit-status reporting so a closed shell reports how it
  ended, plus golden snapshot tests that lock terminal rendering against
  regressions.
- Added a Terminal settings tab and a terminal configuration block in the config
  schema and loader.
- Hardened self-update end to end: release artifacts are now signed in CI, every
  download is verified against an embedded minisign key before install, updates
  swap in atomically with crash recovery, and an unsigned build refuses to
  self-update.
- Added per-platform update verification: macOS codesign and spctl gating with
  Team ID pinning, Windows Authenticode through WinVerifyTrust, hardened tar.gz
  and AppImage extraction, and native host architecture detection for Rosetta
  and WOW64.
- Eliminated panics on untrusted input across session restore, config parsing,
  IPC, date handling, and layout, replacing defensive indexing with fail-safe
  accessors.
- Bounded every external surface against resource exhaustion: the IPC server
  caps line size, concurrency, and idle time; external subprocesses run under a
  timeout with a watchdog; ingress and DoS caps are centralized in one module.
- Moved blocking work off the render thread: session saves, git diff stats,
  config loads, font enumeration, and the recursive file watcher now run in the
  background, with a cached config feeding every frame.
- Sanitized untrusted content paths: markdown rendering strips bidi and
  zero-width characters, git refs are stripped of control bytes before they
  reach agent prompts, and session ids are validated to block argument
  injection.
- Validated and clamped all persisted config and session input, with atomic
  write-and-rename for `paneflow.json` and symmetric bounds shared across
  session, IPC, and the config schema.
- Hardened terminal and shim lifecycle: PID-reuse guards, an environment
  deny-list and scrollback sanitization on session restore, codex flock
  serialization, and correct orphan cleanup under systemd.
- Improved Windows portability: portable shell launches, correct LOCALAPPDATA
  casing, Git for Windows PATH augmentation, and `dirs`-based home resolution.
- Reduced per-frame allocations in terminal paint, sidebar recompute, and
  layout, with memoized derivations and zero-allocation leaf lookups.
- Fixed non-US keyboard input, decoupled Alt-on-arrows from the option-as-meta
  setting, and reworked the keybindings editor to be action-indexed with
  collision detection.

## [0.3.8] - 2026-06-02

- Changed the Agents view to a terminal-only model: each thread now launches a
  CLI coding agent directly in a terminal pane with a pre-filled prompt instead
  of an in-app chat, keeping the agent in its native terminal with permission
  bypass respected exactly as the tab-bar buttons do.
- Added eleven launchable agents alongside Claude Code, Codex, OpenCode, Pi, and
  Hermes: Grok, Amp, Cursor, Gemini, Kiro, Antigravity, Copilot, CodeBuddy,
  Factory, Qoder, and Openclaw, each with its own tab-bar button, icon, and
  Settings visibility toggle.
- Each Terminal Thread now remembers which agent it launches and restores it on
  the next session.
- Removed the in-app ACP chat, its conversation timeline and composer, and the
  separate agent sign-in page; agents now authenticate in their own terminal.
- Hardened the Git diff viewer with safer working-tree reads, a shared
  generated-file skip-list, and a watcher-refresh race fix.
- Polished open-source onboarding: community-health files, issue templates, and
  README positioning on the agent cockpit and cross-platform story.

## [0.3.7] - 2026-06-01

- Added an in-app Git diff viewer with file trees, sticky headers, hunk jumps,
  gutter line numbers, per-file diffstats, and word-level highlighting.
- Added branch review flows that open selected agents in real terminal panes
  with a review prompt scoped to the branch worktree.
- Added hunk/file diff copy actions for sending precise context to agents.
- Improved Worktree branch-column behavior so deselecting a branch is explicit.

## [0.3.6] - 2026-05-29

- Added docked Agent Sessions and Files sidebars.
- Added markdown-file opening from the Files panel into an adjacent pane.
- Added drag-to-reorder tabs within a pane and drag-to-move tabs between panes.

## [0.3.5] - 2026-05-29

- Added the Paneflow MCP bridge so capable agents can read pane output through
  `list_panes`, `read_pane`, and `search_pane`.
- Added `paneflow mcp install`, `uninstall`, and `status` commands.
- Added readable pane references, persistent tab renames, and clipboard copy for
  pane references.

## [0.3.4] - 2026-05-28

- Hardened the CLI-agent subsystem for long sessions: bounded caches, parser
  limits, safer IPC behavior, better logging, and reduced retained UI state.
- Improved hot paths for markdown streaming, code highlighting, persisted-item
  collection, and activity-state computation.
- Added CI audit coverage and benchmark baselines for key performance paths.
- Changed `claude_code_bypass_permissions` to default to `false` on fresh
  installs.

## [0.3.3] - 2026-05-27

- Added multi-session tracking for concurrent Claude Code, Codex, and other
  agent sessions in the same workspace.
- Added Ctrl/Cmd-click handling for `file:line:column` references in terminal
  output and assistant messages.
- Added IPC singleton protection to prevent two app instances from racing over
  the same socket.
- Improved ACP client capability declarations for richer Codex and Claude Code
  streams.

## [0.3.2] - 2026-05-26

- Added Terminal Threads as first-class sidebar entries backed by Paneflow's PTY
  stack.
- Added editable project and thread names using the same text widget as the
  composer.
- Added background thread-title generation and title cleanup for agent-provided
  titles.

## [0.3.1] - 2026-05-26

- Maintenance release. See the GitHub compare link for the full commit list.

## [0.3.0] - 2026-05-25

- Opened the 0.3.x release line. See the GitHub compare link for the full commit
  list.

[Unreleased]: https://github.com/arthjean/paneflow/compare/v0.8.1...HEAD
[0.8.1]: https://github.com/arthjean/paneflow/compare/v0.8.0...v0.8.1
[0.6.2]: https://github.com/arthjean/paneflow/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/arthjean/paneflow/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/arthjean/paneflow/compare/v0.5.9...v0.6.0
[0.5.0]: https://github.com/arthjean/paneflow/compare/v0.4.4...v0.5.0
[0.4.4]: https://github.com/arthjean/paneflow/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/arthjean/paneflow/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/arthjean/paneflow/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/arthjean/paneflow/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/arthjean/paneflow/compare/v0.3.9...v0.4.0
[0.3.9]: https://github.com/arthjean/paneflow/compare/v0.3.8...v0.3.9
[0.3.8]: https://github.com/arthjean/paneflow/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/arthjean/paneflow/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/arthjean/paneflow/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/arthjean/paneflow/releases/tag/v0.3.5
[0.3.4]: https://github.com/arthjean/paneflow/releases/tag/v0.3.4
[0.3.3]: https://github.com/arthjean/paneflow/releases/tag/v0.3.3
[0.3.2]: https://github.com/arthjean/paneflow/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/arthjean/paneflow/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/arthjean/paneflow/compare/v0.2.17...v0.3.0
