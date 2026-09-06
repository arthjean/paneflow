# About Paneflow

Paneflow is a native Rust/GPUI workspace where coding agents work and where
you read what they changed.

## In one sentence

A local workspace where Claude Code, Codex, Gemini, opencode, and any CLI
agent run in real Ghostty terminal panes, one session per tab on its own
branch or worktree, with a Changes view, a code editor, a Review grid for
several worktrees, a read-only MCP bridge, and scriptable orchestration.

## What makes it different

- **Native, not Electron.** Rust, GPUI, and a statically linked
  `libghostty-vt` engine, with GPU rendering through Vulkan, Metal, and
  DirectX.
- **One session per tab, on its own worktree.** A tab binds to a branch or
  worktree, names itself after its work, and shows the agent's state in the
  rail: thinking, waiting, stalled, failed, or done.
- **The diff next to the agent.** Changes shows the checkout against its base
  branch with word-level highlighting and per-block revert, Files opens the
  code with git markers in the gutter, and Review lays several worktree diffs
  side by side in one pane grid.
- **Any CLI agent, in real terminals.** No chat wrapper, no model picker, no
  hosted runtime: whatever is on `PATH` runs in a Ghostty pane you can read
  and take over.
- **Scriptable when you need it.** The `paneflow` CLI, JSON-RPC socket, MCP
  bridge, and `flow.toml` runner let humans or agents coordinate panes.
- **Cross-platform release surface.** Linux Wayland/X11, macOS Apple Silicon,
  and Windows x64 ship as native release artifacts.
- **Open by design.** GPL-3.0-or-later.

## Start

```bash
cargo run -p paneflow-app
RUST_LOG=info cargo run -p paneflow-app
```

Architecture and repo conventions: [ARCHITECTURE.md](ARCHITECTURE.md),
[AGENTS.md](AGENTS.md), and [README.md](README.md).
Site: [paneflow.dev](https://paneflow.dev).
