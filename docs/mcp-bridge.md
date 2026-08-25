# Paneflow MCP bridge (`paneflow-mcp`)

Let an MCP-capable CLI agent running **inside a Paneflow pane** read the
terminal output of **other surfaces in the current workspace** - so you can say
*"check the logs in the cargo-run pane"* instead of selecting, copying, and
pasting by hand.

`paneflow-mcp` is a small stdio [MCP](https://modelcontextprotocol.io) server.
The agent spawns it as a subprocess; it proxies each call to Paneflow's local
JSON-RPC socket (the same one the AI-hook uses). It is **read-only** - it can
list, read, and search surfaces, but cannot type into or control them.

By default the bridge inherits `PANEFLOW_WORKSPACE_ID` from the pane that
launched the agent and filters discovery, tools, and resources to that
workspace. Set `PANEFLOW_MCP_SCOPE=all` only when instance-wide read access is
intentional.

> Source: `crates/paneflow-mcp/`. The protocol is implemented by hand (not via
> `rmcp`) to keep the dependency tree tiny and the surface fully unit-tested.

## Tools

| Tool | Arguments | Returns |
|------|-----------|---------|
| `list_panes` | - | Scoped surfaces: `surface_id`, `name`, `title`, `cwd`, `cmd`, `workspace`, plus `tab_id` / `tab_title` for the workspace tab holding the surface. Call this first to discover what to read. The result is wrapped as untrusted terminal metadata. |
| `read_pane` | `target` (name or `surface_id`), `lines?` (default 200, max 4000), `offset?` | The surface's scrollback as text, paginated. |
| `search_pane` | `target`, `pattern`, `max_matches?` (default 50, max 1000) | Matching lines with their line numbers. |

`target` resolves by exact name → case-insensitive → unique prefix, or a numeric
`surface_id`. An ambiguous name returns an error listing the candidates.

> **Security.** Returned content is wrapped in an `<untrusted_terminal_output>`
> marker. A pane may contain attacker-controlled output (a server logging a
> crafted string), and pane titles can also be terminal-controlled; the agent
> is instructed to treat bridge output as data, never as instructions to
> execute. The bridge exposes no write/keystroke tool by design.

`tab_id` is a stable identity, never a positional index, and it is omitted for
surfaces that live outside the CLI tab hierarchy (Agents threads, the bottom
dock) or when the running Paneflow predates it. Targeting stays by surface
name or `surface_id`: the tab is context for the agent, not an addressing key.

MCP resources use stable `surface_id` URIs:
`pane://surface/{surface_id}/content`. Human names and titles stay in
`list_panes`; they are display metadata, not URI syntax.

## Install (one command)

The bridge binary **ships inside Paneflow** - no build step. On every launch,
Paneflow extracts it to a stable, non-versioned path
(`~/.local/share/paneflow/bin/paneflow-mcp` on Linux,
`~/Library/Application Support/paneflow/bin/paneflow-mcp` on macOS,
`%LOCALAPPDATA%\paneflow\bin\paneflow-mcp.exe` on Windows) that survives updates.

To register the bridge with every CLI agent installed on your machine:

```bash
paneflow mcp install
```

It detects which agents are present (Claude Code, Codex, Gemini CLI, opencode),
writes the `paneflow` entry into each one's config, and reports per agent:

```text
claude-code: installed (/home/you/.local/share/paneflow/bin/paneflow-mcp)
codex: installed (/home/you/.local/share/paneflow/bin/paneflow-mcp)
gemini: skipped (not detected)
opencode: skipped (not detected)
```

The command is **idempotent** (re-running it is a no-op when nothing changed),
**no-clobber** (it only touches the `paneflow` entry, preserving every other MCP
server and setting), and **backed up** (the prior config is copied to
`<file>.bak` before any write). Run it again after a Paneflow update if `status`
reports a stale path.

```bash
paneflow mcp status      # report state per agent (read-only)
paneflow mcp uninstall   # remove only the `paneflow` entry, everywhere
```

`status` distinguishes five states per agent: *not detected*, *installed*,
*detected but not installed*, *stale path*, and *needs repair* when a
`paneflow` entry exists but is disabled or no longer matches Paneflow's managed
schema. `status` never extracts or writes the bridge binary.

> Where each agent's entry lands: Claude Code: `~/.claude.json`
> (`mcpServers.paneflow`, backed up before `claude mcp add -s user`); Codex:
> `$CODEX_HOME/config.toml` when `CODEX_HOME` is set, otherwise
> `~/.codex/config.toml` (`[mcp_servers.paneflow]`, backed up before
> `codex mcp add`); Gemini CLI: `~/.gemini/settings.json`
> (`mcpServers.paneflow`, `trust: true`); opencode: `OPENCODE_CONFIG`, or
> `OPENCODE_CONFIG_DIR`, or the global `opencode.jsonc` / `opencode.json`
> config (key `mcp`, `command` as an array, `type: "local"`).

### Not supported: aider

aider does not consume MCP. There is no bridge path for it; feed it pane output
manually (e.g. `--read <file>`).

## Manual configuration (if you prefer)

`paneflow mcp install` is the recommended path. If you'd rather wire it by hand
 -  or you're working in this repo, where `.mcp.json` already registers the
server for Claude Code - use the snippets below. Build the binary first with
`cargo build -p paneflow-mcp --release` (→ `target/release/paneflow-mcp`), and
point `command` at that absolute path.

> These config shapes are **version-volatile** for Codex, Gemini, and opencode -
> their CLIs move fast. `paneflow mcp install` tracks the current format; verify
> manual snippets against each agent's current docs.

The bridge finds the running Paneflow instance via `$PANEFLOW_SOCKET_PATH`,
injected into every pane's environment - so it must be launched from inside a
Paneflow pane (which is exactly where your agent runs).

### Claude Code

```bash
claude mcp add -s user --transport stdio paneflow -- /absolute/path/to/paneflow-mcp
```

Or directly in `~/.claude.json` under `mcpServers.paneflow`:
`{"type": "stdio", "command": "/absolute/path/to/paneflow-mcp", "args": []}`.
Claude Code consumes MCP **tools** and resources.

### Codex CLI

`$CODEX_HOME/config.toml` when `CODEX_HOME` is set, otherwise
`~/.codex/config.toml`:

```toml
[mcp_servers.paneflow]
command = "/absolute/path/to/paneflow-mcp"
args = []
```

Codex consumes **tools only** - which is why the bridge exposes everything as
tools, not MCP resources.

### Gemini CLI

`~/.gemini/settings.json`:

```json
{
  "mcpServers": {
    "paneflow": {
      "command": "/absolute/path/to/paneflow-mcp",
      "args": [],
      "trust": true
    }
  }
}
```

`paneflow mcp install` sets `trust: true` (the bridge is a local binary you
control, so per-call confirmation adds only friction). Set it to `false` if you
prefer Gemini's confirmation prompt given the untrusted-output surface.

### opencode

`opencode.jsonc` or `opencode.json` in opencode's global config location
(or `OPENCODE_CONFIG` / `OPENCODE_CONFIG_DIR`) - note the distinct schema
(key `mcp`, not `mcpServers`; `command` is an array; `type: "local"`):

```json
{
  "mcp": {
    "paneflow": {
      "type": "local",
      "command": ["/absolute/path/to/paneflow-mcp"],
      "enabled": true
    }
  }
}
```

## Example

In an agent running inside Paneflow:

> *"List my panes, then read the last 100 lines of the cargo-run pane and tell
> me why the build failed."*

The agent calls `list_panes`, sees a surface named `cargo-run`, then
`read_pane(target="cargo-run", lines=100)` - no manual copy-paste.
