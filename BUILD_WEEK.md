# OpenAI Build Week: Paneflow

## Project overview

**Project name:** Paneflow

**Elevator pitch:** Cross-platform GPUI app for parallel coding agents.

## Project story

**Built during OpenAI Build Week:** I used Codex with GPT-5.6 Sol to move Paneflow's Linux terminals from Alacritty to `libghostty-vt`. I made Ghostty the default Linux backend, added differential tests and fuzzing, and made the pinned `libghostty-vt` archives reproducible. Every terminal pane shown in the submission screenshots is running on that new Ghostty backend. Alacritty is still available as a rollback. I also redesigned the UX/UI across the application; the matching `paneflow.dev` redesign is still in progress.

### Inspiration

I usually have several coding agents running at the same time. Starting them is easy. Remembering which one is waiting, which branch it touched, what failed, and whether two agents are about to work on the same thing gets messy fast.

I was supervising tmux grids and a pile of terminal windows, mostly from memory. I wanted a workspace where every agent stayed visible and where I could step in at any time.

That became Paneflow.

### What it does

Paneflow is a cross-platform GPUI app for parallel coding agents. It runs Codex, Claude Code, Gemini, OpenCode, and other CLI agents inside real terminal panes.

The sidebar shows which agents are thinking, running, waiting, finished, or stuck. Each workspace keeps its directory and Git branch visible. Paneflow also provides persistent layouts, an attention queue, desktop notifications, and a Review view for comparing worktree diffs side by side.

Paneflow Conductor is the local control plane behind the interface. Its CLI and JSON-RPC socket let a human or lead agent list running agents, inspect their state, read terminal output, send a prompt, and wait for lifecycle events.

Agents can also inspect another pane through a built-in read-only MCP bridge with three tools: `list_panes`, `read_pane`, and `search_pane`. Terminal output returned through MCP is wrapped as untrusted data. Prompt submission remains human-controlled by default, with automatic submission available as an explicit opt-in.

Paneflow runs locally and ships native builds for Linux, macOS Apple Silicon, and Windows x64.

### How I built it

Paneflow is written in Rust with GPUI, the native GPU-accelerated UI framework behind Zed.

Each pane is backed by a real PTY. Agent lifecycle hooks feed a local event bus, while the application tracks workspaces, branches, repository changes, notifications, and session state. Conductor exposes the same state through a local JSON-RPC protocol and public CLI. The MCP server provides a smaller read-only surface for agent-to-agent inspection.

The application ships as native packages and does not need Electron, WSL, or a hosted agent runtime.

### What I built during OpenAI Build Week

Paneflow predates Build Week, so I am using commit `e82b3da` from July 10 as the pre-event baseline. The Devpost submission contains the `/feedback` ID for the main build session, while the dated commits after `e82b3da` separate this work from the existing product.

During Build Week, I used Codex with GPT-5.6 Sol to move Paneflow's Linux terminal stack to `libghostty-vt`.

The migration started with an architecture study. I used GPT-5.6 Sol at Ultra reasoning effort to explore Ghostty and Ghostling in depth, find the parts Paneflow could safely reuse, and work out whether a cross-platform path was realistic. I chose Linux as the first target so I could validate the engine and packaging before taking on Windows and macOS.

After that initial research, I asked Codex to turn the findings into a complete, dependency-ordered [migration PRD](tasks/prd-linux-libghostty-backend-2026-Q3.md). It starts with a smoke test, then moves through the session abstraction, implementation, default switch, differential testing, and release pipeline. I supplied implementation and review skills I had written for this kind of work, then used GPT-5.6 Sol to audit those skills before running the epics.

The migration included:

- A backend-neutral terminal session layer owned by Paneflow.
- A safe Rust wrapper around the evolving Ghostty C API.
- A complete Linux PTY lifecycle covering spawn, input, resize, scrollback, search, selection, clipboard, OSC events, shutdown, and session restoration.
- Ghostty as the default Linux backend, with Alacritty preserved as an explicit rollback.
- Deterministic differential tests that feed the same terminal streams into both backends and compare normalized output.
- Fuzz targets for rendering, input, reflow, malformed sequences, and chunk boundaries.
- Reproducible, pinned static `libghostty-vt` archives for Linux x86_64 and ARM64.
- CI checks for generated bindings, dependency sources, package contents, native licenses, and cross-platform isolation.
- A broader UX/UI redesign across the application, including the sidebar, tab bar, Review and Settings surfaces, window chrome, and interaction feedback.

I made the architectural calls up front: GPUI stays in charge of rendering, Ghostty is pinned and statically linked, `cargo build` never downloads native code, unsafe access stays confined to small wrappers, Linux gets the new backend first, and Alacritty remains available for rollback. Codex worked inside those constraints.

Codex then helped me trace behavior across Paneflow, Ghostty, Ghostling, and Zed. I used it to implement the PRD one dependency-ordered slice at a time, generate focused tests, diagnose resize and reflow bugs, review FFI boundaries, and fix failures in the native CI pipeline.

I use Codex CLI on Linux every day, but I also worked from Codex App on Windows for the hands-on validation. I used Computer Use to observe Paneflow, while CLI tools, test harnesses, and PowerShell drove the scenarios around terminal startup, backend switching, and window or pane resizing. That caught integration problems that library-level tests alone would not show.

In parallel, I reworked the visual hierarchy, spacing, navigation, Review surfaces, window chrome, and interaction feedback across the application. The `paneflow.dev` redesign follows the same direction and is still underway. I will count it as completed Build Week work only once the new site is finished and live before the deadline.

### Challenges I ran into

`libghostty-vt` exposes borrowed render data through a C API. A terminal mutation can invalidate that data, so no pointer or borrowed slice can survive a lock or frame boundary. The Rust wrapper copies the data Paneflow needs while the terminal is locked and only exposes owned snapshots to the GPUI renderer.

Ghostty provides the terminal engine, while Paneflow still owns the PTY, process lifecycle, renderer, persistence, product events, and platform integration. The most visible bugs appeared during resize, reflow, and scrollbar dragging, where the terminal could look correct at rest and then jump as its dimensions changed.

One resize bug took more than an hour of back-and-forth in the same Codex session. We kept resizing the Paneflow window, checking what the Ghostty-backed terminal did, changing the implementation, and trying again until the terminal stayed aligned. It was slow, concrete debugging, and it is one of the clearest examples of how I actually worked with GPT-5.6 during the migration.

Packaging was another large part of the work. A normal Paneflow build must work without Zig or a local Ghostty checkout, so the release pipeline produces pinned static archives ahead of time and verifies their headers, bindings, checksums, symbols, and licenses.

### What I am proud of

The Linux backend is now a real product path rather than an isolated experiment. It is the default for standard Linux builds, it has an immediate rollback, and it ships without adding a native runtime dependency for users.

I also used Paneflow to build Paneflow. Multiple Codex sessions ran side by side while I inspected their state, reviewed changes, diagnosed failures, and took over individual panes when needed.

### What I learned

Codex was most useful when the task had a clear seam, explicit invariants, and a focused way to verify the result. The productive loop was concrete: reproduce a bug, capture logs, give Codex a bounded investigation, review the diff, and run the narrowest relevant check.

The migration also reinforced why Paneflow exists. Once several agents are working across architecture, implementation, testing, and review, observing the work becomes part of the engineering problem.

### What's next

I plan to keep deepening Conductor, agent observability, and worktree review. The Ghostty port to Windows is now underway, using the same backend boundary and rollback strategy. It is still in progress, so v0.8.0 and this submission claim only the completed Linux migration.

macOS comes next. That port will resume when I can rent a real macOS test machine and validate the Ghostty backend hands-on.

Repo: https://github.com/arthjean/paneflow

Website: https://paneflow.dev

## Built with

- Rust
- GPUI
- Codex
- GPT-5.6 Sol (Ultra reasoning effort)
- libghostty-vt
- Tokio
- portable-pty
- Model Context Protocol (MCP)
- JSON-RPC 2.0
- Serde
- Tree-sitter
- Zig
- GitHub Actions
- Wayland
- X11
