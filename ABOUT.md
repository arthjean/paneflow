# About Paneflow

Paneflow is a native Rust/GPUI workspace for running coding agents in parallel.

## In one sentence

A local control plane where Claude Code, Codex, Gemini, opencode, and any CLI
agent run side by side in real terminal panes, with live status,
worktree review, a read-only MCP bridge, and scriptable orchestration.

## What makes it different

- **Native, not Electron.** Rust, GPUI, and a statically linked
  `libghostty-vt` engine, with GPU rendering through Vulkan, Metal, and
  DirectX.
- **Built for agent supervision.** Paneflow tracks which agents are thinking,
  waiting, stalled, failed, or done instead of leaving you to scan raw
  scrollback.
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
