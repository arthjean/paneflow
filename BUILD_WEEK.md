# OpenAI Build Week: Paneflow

## Project Overview

**Project name:** Paneflow

**Category:** Developer Tools

**Pitch:** A native cross-platform workspace for launching, monitoring, and orchestrating multiple coding agents in parallel.

## Project Story

**Summary of the work completed during OpenAI Build Week:** Paneflow existed before Build Week. During the event, I used Codex CLI, Codex App, and GPT-5.6 Sol to migrate its terminal backend to `libghostty-vt` on Linux, port the architecture to Windows with ConPTY, and ship the new engine on both platforms. In parallel, I redesigned the UX/UI of the app and `paneflow.dev`, along with Paneflow's entire visual identity.

### Inspiration

Before Paneflow, I was running more and more agents in parallel. Starting them was easy. But keeping track of who was waiting, who was changing what, what had failed, and how to avoid conflicts created a constant mental burden I could not eliminate. Running more terminal windows and processes also increased my PC's memory usage.

I was monitoring tmux grids and countless terminals while keeping each agent's state in my head. I wanted a workspace where every session stayed visible and I could step in at any time. That's how Paneflow was born.

### How It Works

Paneflow is a local workspace for developers who supervise multiple CLI agents on the same project. It brings Codex, Claude Code, OpenCode, Grok, Pi, and other agents into real terminals, alongside their branches, worktrees, and sessions. The interface makes their status visible, while the Review view lets you compare changes across worktrees.

Conductor is its local control plane: a human or lead agent can orchestrate the others through a CLI and a JSON-RPC socket. The read-only MCP bridge lets them inspect neighboring panes without taking control. Paneflow brings terminals, lifecycle states, orchestration, and diff review together in a native app for Linux, macOS on Apple Silicon, and Windows x64.

### Technical Design

Paneflow is written in Rust with GPUI, the native UI framework used by Zed. Each pane is a real terminal connected to a pseudoterminal (PTY). `libghostty-vt` is the default engine on Linux and Windows, with Alacritty available as a fallback. macOS still uses Alacritty until the Ghostty port is complete.

Agent hooks send lifecycle events to Paneflow as JSON-RPC over a local socket or named pipe. Paneflow matches each event to the right session and terminal, updates the UI immediately, then mirrors the normalized state onto a non-blocking event bus for subscribers such as Conductor. Conductor also uses the same JSON-RPC interface to orchestrate agents, while the MCP bridge exposes only three read-only tools: `list_panes`, `read_pane`, and `search_pane`.

### What I Built During OpenAI Build Week

Paneflow existed before Build Week. During the event, I worked across dozens of Codex sessions on Linux and Windows, covering research, PRDs, implementation, testing, debugging, design, and more.

For verification, commit `e82b3da`, released on July 10 as version 0.7.10, is the pre-event baseline. Devpost also includes the `/feedback` ID from one of the critical sessions for the build.

#### Versions and Verifiable History

- [v0.7.10](https://github.com/arthjean/paneflow/releases/tag/v0.7.10): the pre-Build Week baseline.
- [v0.7.11](https://github.com/arthjean/paneflow/releases/tag/v0.7.11): the first release shipped during the event, focused on workspace navigation and terminal polish.
- [v0.8.0](https://github.com/arthjean/paneflow/releases/tag/v0.8.0): Ghostty became the default terminal engine on Linux, backed by differential validation, fuzzing, reproducible static archives, and native packaging.
- [v0.8.1](https://github.com/arthjean/paneflow/releases/tag/v0.8.1): Ghostty became the default terminal engine in published Windows x64 MSVC builds, with the ConPTY host, signed MSI qualification, and Windows integration fixes.
- [Full Build Week comparison](https://github.com/arthjean/paneflow/compare/v0.7.10...v0.8.1): every commit shipped from the pre-event baseline through the final release.

The choice was simple: Ghostty was a better foundation than Alacritty across every area that mattered to Paneflow, and developers already loved it.

I started with an architecture study using GPT-5.6 Sol with Ultra reasoning. I explored Ghostty and Ghostling to determine which components Paneflow could reuse without giving up control of the rendering, then validated the FFI boundary and packaging on Linux before porting the architecture to Windows with ConPTY.

I couldn't start the macOS port during Build Week. I don't own a MacBook, and that month I couldn't afford to rent an Apple Silicon machine from Scaleway to properly test the PTY, rendering, and native packaging.

I asked Codex to turn that research into two PRDs in dependency order: a [Linux PRD](https://github.com/arthjean/paneflow/blob/main/tasks/prd-linux-libghostty-backend-2026-Q3.md), followed by a [Windows PRD](https://github.com/arthjean/paneflow/blob/main/tasks/prd-windows-libghostty-backend-2026-Q3.md). I gave it my own skills for research, writing, implementation, and review, which I audited with GPT-5.6 Sol before starting the work.

The migration includes:

- A session abstraction and safe Rust API around Ghostty's C ABI. All `unsafe` access remains confined to small FFI modules, and GPUI receives only self-contained Rust snapshots.
- A complete PTY lifecycle using `portable-pty` on Linux and ConPTY on Windows, along with search, selection, clipboard support, OSC events, and persistence. Ghostty is the default backend on both platforms, and Alacritty remains immediately available if something goes wrong.
- A 135-case differential test corpus comparing Ghostty and Alacritty, two fuzzing targets, and tests covering input, line reflow, malformed sequences, and different stream chunk boundaries.
- Pinned, reproducible static `libghostty-vt` archives for Linux x86_64, Linux ARM64, and Windows x64 MSVC. `cargo build` verifies and links them without downloading or compiling Ghostty, while CI checks their provenance, bindings, packages, and licenses.

I set the constraints up front: GPUI remains responsible for rendering, Ghostty provides the terminal state, the artifacts are statically linked, Cargo stays offline, and Alacritty provides an immediate fallback. Codex then traced behavior across Paneflow, Ghostty, Ghostling, and Zed, implemented the PRDs in batches, and diagnosed resizing, ConPTY, and native CI issues.

Throughout Build Week, I used Codex CLI on Linux. On Windows, Codex App and Computer Use let me observe Paneflow while PowerShell drove the terminals, backend switching, and resizing. This combination exposed problems that library tests couldn't catch.

In parallel, I redesigned the app's navigation, Review and Settings views, window chrome, and interaction feedback. I carried that direction over to `paneflow.dev`, then redesigned Paneflow's entire visual identity with GPT-5.6 Sol using `xhigh` reasoning, from the logo and marketing visuals to the brand guidelines. The new website is live here: https://paneflow.dev/

### Challenges I Ran Into

`libghostty-vt` exposes its rendering state through a C API whose data can be invalidated as soon as the terminal changes. Paneflow therefore confines each Ghostty instance to a dedicated thread, copies the cells, styles, and grapheme clusters while reading them, and then passes only self-contained Rust snapshots to GPUI. No Ghostty pointer ever reaches the renderer.

Ghostty handles VT parsing, the grid, and reflow. Paneflow retains responsibility for the PTY, process lifecycle, GPUI rendering, persistence, and product events. The most visible bugs appeared at this boundary: the terminal looked fine while idle, then its content jumped during a resize or when moving the scrollbar.

One of those bugs took more than an hour in a single Codex session. I kept resizing Paneflow, watching the offset, adjusting the implementation with GPT-5.6, and repeating the process until the terminal stayed stable, while making sure the code remained clean, without hacks or technical debt. That loop captures how I work with GPT-5.6 Sol in Codex: observe, isolate, fix, repeat.

On Windows, ConPTY added another trap: shutting it down can hang while the pipe is draining. Paneflow therefore closes the pseudoterminal on a dedicated thread, drains the final output, and releases the handles before signaling the end of the session.

Packaging required the same level of rigor. I didn't want to turn `cargo build` into a Ghostty build system. Cargo consumes a versioned, verified native archive without invoking Zig, bindgen, or a Ghostty checkout. Dedicated scripts and CI pipelines rebuild the Linux and Windows static archives separately, then verify their reproducibility, ABI, headers, bindings, symbols, and licenses.

Building the interactive layouts for the `paneflow.dev` landing page was just as complex. With GPT-5.6 Sol in Codex and its Computer Use and Browser Use tools, I recreated Paneflow's native Rust and GPUI interface in Next.js, aiming for near pixel-perfect fidelity. I had to translate the title bar, window controls, sidebar, panes, layout changes, session dock, and agent states into a completely different rendering system while preserving the interactions and responsive behavior.

### What I'm Proud Of

I'm proud to have taken this migration all the way to shipping. Ghostty is now the default backend on Linux and Windows, statically linked and shipped with no separate runtime. Alacritty remains immediately available if something goes wrong.

Paneflow has already recorded more than 150 opt-in users in PostHog. It is an early signal that the coordination problem it addresses extends beyond my own workflow.

I also used Paneflow to build Paneflow. Several Codex sessions with GPT-5.6 Sol were working in parallel on the Ghostty migration, the Windows port, the tests, the app, and the website. I could immediately see which session was waiting or had failed, read its output, compare its changes, and step in without losing context. Paneflow solved the problem that led me to create it.

### What I Learned

GPT-5.6 Sol is particularly effective when I frame the problem and give it a fast way to verify each hypothesis. My loop was simple: reproduce, observe, hand the investigation to Codex, review the diff, and then test the exact change. I also learned that parallelism is useful only when it remains easy to follow. Observing agents, comparing their work, and stepping in are now part of the engineering process itself.

### Next Steps

After Linux and Windows, the next platform for the Ghostty backend will be macOS. I'll start that port as soon as I have access to an Apple Silicon machine so I can properly validate the PTY, rendering, and native packaging.

I also want to connect the agents' real-time status more closely with the Review view, making the transition from implementation to validation smoother while keeping full visibility and the ability to step into any terminal.

My vision is simple: I want to ship an agentic ecosystem that adapts to this new way of building software. Paneflow is the central pillar.

[GitHub repository](https://github.com/arthjean/paneflow)

[Website](https://paneflow.dev)

## Technologies and Tools Used

- **OpenAI:** Codex CLI, Codex App, GPT-5.6 Sol (Ultra and `xhigh` reasoning), Computer Use, and Browser Use
- **Native app:** Rust and GPUI
- **Terminal:** `libghostty-vt`, Rust/C FFI, `portable-pty`, and ConPTY
- **Orchestration:** CLI, JSON-RPC 2.0, and Model Context Protocol (MCP)
- **Review:** Git worktrees, `imara-diff`, and Tree-sitter
- **Testing:** Differential tests against Alacritty and `cargo-fuzz` with libFuzzer
- **Native build and CI:** Zig and GitHub Actions
- **Website:** Next.js, React, TypeScript, and Tailwind CSS
