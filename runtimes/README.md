# Runtime packages

Each built-in coding agent is described by one `runtimes/<slug>/runtime.toml` file. `paneflow-agent-config/build.rs` discovers every descriptor, validates the complete set, and generates the constant Rust registry consumed by the app and helper binaries. No runtime TOML parser is linked into shipping binaries.

Adding a detection-only runtime requires one new directory and no Rust edits. Give it at least one command alias, set lifecycle source, authority and fallback to `none`, select the `none` hook adapter, and add one suggested preset. A runtime with screen fallback also supplies a `screen.toml` beside its descriptor and captured screens under `fixtures/screens/` (see [Screen rules](#screen-rules)). The new runtime then appears in the agent launcher at its `display.order`, gets its row in the Settings visibility table under its `display.visibility_config_key`, and receives a PATH shim unless it declares `detection.title_prefix`. The Rust handles such as `TerminalAgent::ClaudeCode` are generated from the slugs by `runtime_identity_constants!`, so the app never repeats a runtime id.

The Windows installer (`packaging/wix/main.wxs`) and the release workflow (`.github/workflows/release.yml`) each list one shim per shimmed runtime, named after its first command alias. A runtime with `detection.title_prefix` gets no shim: the shim announces a session as soon as its alias runs, before any title can confirm the program is the agent. `cargo test -p paneflow-agent-config` fails, naming the alias, when either list misses a runtime or keeps one the catalog no longer has.

## Descriptor

The schema is strict. Unknown fields, unsupported enum values, a directory and `slug` mismatch, malformed reverse-DNS ids, duplicate ids or aliases, and inconsistent lifecycle policy fail the build with the descriptor path.

Required top-level fields:

- `schema_version = 1`
- `id`: stable reverse-DNS identity
- `slug`: identical to the package directory name
- `label`: user-facing name
- `platforms`: any of `linux`, `macos`, and `windows`

`[display]` defines the unique `order`, `tint` as optional `#RRGGBB`, `icon_asset_path`, `icon_multicolor`, and `visibility_config_key`. `order` sorts the launcher, the visibility table and the session sidebar. `visibility_config_key` is the unique `paneflow.json` key that shows or hides the launcher button: a lowercase snake_case name ending in `_button_visible`, read from the configuration whatever the runtime, so it needs no matching Rust field. When the key is absent or `null`, the button follows install detection of the canonical command alias.

`[detection]` defines `command_aliases`, `process_aliases`, normalized `script_path_signatures`, and an optional `title_prefix`. The first command alias is the canonical executable. Generic interpreters such as `node`, `sh`, and `python` must not be aliases. Script signatures identify their wrapped runtime from an argv path instead. `title_prefix` makes the host accept a foreground process as this runtime only while the pane's last OSC 0/2 title starts with it; otherwise the process is not an agent and no agent row appears. An alias that another common program also uses is contested: today `fx`, also the name of the fx JSON viewer. The build refuses a contested alias without `title_prefix`. fx 0.0.12 titles its window `fx v<version> | <folder>`, so its descriptor declares `title_prefix = "fx v"`.

`[environment]` defines `strip_inherited`, the provider environment variables that must not leak into a nested launch.

`[lifecycle]` defines `source`, `authority`, `fallback`, `escape_cancels_turn`, `attention_clears_on_output`, `anchor_start_event_to_output`, and an optional `bell_attention`. `authority` states what Paneflow can actually observe: `complete` when Paneflow installs lifecycle hooks for the runtime, `screen` when only its screen rules report turn status, and `none` otherwise. `fallback = "screen"` requires a `screen.toml` with at least one `working` and one `idle` rule. `bell_attention` makes a BEL from the foreground runtime ask for attention; it defaults to true unless `authority = "complete"`, and the build refuses `bell_attention = true` beside complete hooks, which own attention. `authority = "screen"` requires `fallback = "screen"`, and `authority = "none"` requires `fallback = "none"`. A runtime whose hooks reach Paneflow through another integration may declare `source = "hooks"` with `authority = "none"` when that stream is too partial to be the sole source.

`[integration]` defines a user-facing `summary`, optional `post_install_step`, and `hook_adapter`, one of `none`, `claude`, or `codex`. Only `claude` and `codex` have an installer, so `authority = "complete"` holds exactly for them and the build rejects any other pairing, naming the descriptor, the field, and the accepted adapters. A summary describes the integration the runtime really has. A new detection-only runtime uses `hook_adapter = "none"`; adding a new provider-specific hook adapter is an explicit core change that ships its installer.

The same section drives `paneflow integrations install` and `paneflow mcp install`, which share one engine. Optional `mcp_config` names the writer of the agent's MCP config file and format, one of `claude`, `codex`, `gemini`, `opencode`, or `fx`. The `fx` writer adds a local `paneflow` entry to `~/.fx/mcp.json` with no `environment` block, because fx replaces the environment of the servers it starts, and it treats fx as present only when `~/.fx` exists; a runtime with an installer hook adapter must declare one, because the integration installs the MCP bridge with the hooks. Optional `skills_dir` names a verified skills directory, today only `claude` (`skills/` under the Claude config directory, honoring `CLAUDE_CONFIG_DIR`), where the engine installs the versioned `paneflow-conductor` skill. Leave it out for a runtime whose skills location is not verified. Each writer is an explicit core change, and the build lists the accepted values for an unknown one.

Optional `[resume]` declares how Paneflow reopens a conversation. `session_argv` and `fork_argv` are argv arrays that contain the `{session_id}` placeholder exactly once as a whole argument, and `continue_argv` reopens the most recent conversation without it. The program of every template must be one of the runtime's `detection.command_aliases`, never the placeholder, and the other arguments use only ASCII letters, digits, `-`, `_`, `.` or `=`. `session_id_pattern` is a regex every stored id must match, on top of Paneflow's own refusal of ids that start with `-` (CWE-88); the build rejects an invalid pattern. `failure_markers` lists texts the CLI prints when it cannot find a conversation, captured from the real CLI into `fixtures/resume-failure.txt`. Declare `fork_argv` only for a fork flag verified on the installed CLI. A runtime with `continue_argv` and the `none` hook adapter never reports its session id, so the desktop records it from the host's foreground observation with an empty id. After a host loss such a pane reopens in its folder and types `continue_argv`; when several panes of that runtime share the folder, none resumes on its own and each shows a banner whose button types the command in the pane the user picks.

Optional `[sessions]` names the `reader` that lists past conversations for the session sidebar: one of `claude`, `codex`, `opencode`, `pi`, `gemini`, `kiro`, or `grok`. A reader is the Rust code that understands one provider's session store, so the build rejects any other value and lists the accepted readers; a new provider store is an explicit core change. Runtimes with the `claude`, `codex`, `opencode`, or `pi` reader can also title tabs automatically, tried in `display.order` when the tab's own agent cannot.

Each `[[suggested_presets]]` entry defines `id` and `command`. The first preset is the built-in launch command and its id remains the persisted Paneflow agent tag, so it takes no other field. Each later preset is an extra launcher entry and may add a launcher `name`, a `platforms` subset of the runtime's platforms, an `env` table for the launched process, and `tmux_compat = true`. A `tmux_compat` preset runs its command with `TMUX` and `TMUX_PANE` set and with `bin/tmux-compat/` of the Paneflow home first on PATH, where `tmux` is the `paneflow` binary itself. Each pane it opens through that shim joins the same team, and the team is authorized by a token issued when the preset is launched. Such a preset cannot declare Windows. Claude Code agent teams use it to open each teammate in a Paneflow pane. A descriptor remains available for labels, colors, icons, and detection on every target, while launch presets are offered only on declared platforms.

## Screen rules

A runtime whose turns Paneflow reads from the screen ships `runtimes/<slug>/screen.toml`, engine 2. The build parses it with the same parser the host uses, refuses an invalid file with its path and line, and embeds it in the host. Each `[[rules]]` entry has:

- `id`, unique in the file, and `state`, one of `working`, `idle`, or `blocked`
- `priority`, an integer: the highest matching priority wins, and a tie keeps file order
- `region`: `all` (default), `last:N`, `first:N` over the non-blank lines of the viewport, or `title` for the last OSC title
- `all`, `any`, and `not`: regex lists of at most 64 patterns, each capped at 1 MiB of compiled program; a source holds at most 256 rules. A rule matches when every `all` pattern, at least one `any` pattern (when present) and no `not` pattern match. Patterns are case-sensitive unless they start with `(?i)`
- `progress`: optional OSC 9;4 states (`none`, `set`, `error`, `indeterminate`, `pause`) the rule requires
- `visible_blocker = true` on a `blocked` rule marks a prompt the user must answer; it raises attention and needs two consecutive misses to clear
- `disabled = true` in an override file removes the rule with that id

The host scans every 500 ms and evaluates only the rules of the pane's foreground runtime, or of the runtime its hooks declared. A `blocked` verdict keeps the last steady state for activity. For a runtime with `authority = "screen"`, the screen verdict owns the pane's status even after a hook arrives, such as the session announcement of its PATH shim.

Rules are layered by id: `~/.paneflow/runtimes/<slug>/screen.toml` (the local override, reloaded within a second of an edit) over the signed remote catalog over the built-in file. An override entry with an existing id replaces the rule, `disabled = true` removes it, and a new id adds one. An invalid override keeps the previous rules and reports the error.

The remote catalog is `screen-catalog.json` on a `screen-catalog-vN` GitHub prerelease, published by `.github/workflows/screen-catalog.yml` whenever a `screen.toml` changes on `main`. The host checks it one minute after start, then daily, retrying within the hour when GitHub cannot be reached, and applies it only when its minisign signature verifies against the key embedded in the build, its trusted comment carries this engine, and its version is strictly newer than the cached one. `agents.remote_screen_catalog = false` turns the download off. Until a catalog key is embedded, the host makes no request.

### Captures and the corpus

`paneflow agent capture <pane> --state working|idle|blocked` writes the pane's current viewport, redacted (home folder and common secret shapes), to a capture file. `paneflow agent explain <pane> [--json]` prints every rule with its match, the winner, the visible blocker, and the sources in force. A capture starts with a header:

```text
paneflow-screen-capture: 1
state: working
runtime: fx
cols: 100
rows: 30
cli: 0.0.12
paneflow: 0.17.5
captured: 2026-10-02T12:47:03Z
title: fx v0.0.12 | paneflow
---
```

`title` and `progress` are optional, and an imported screen adds an `origin` line saying where it came from. Captures live in `runtimes/<slug>/fixtures/screens/`. `cargo test -p paneflow-agent-config` classifies the whole corpus and compares it with `bench/screen-corpus-baseline.json`, which records each verdict, the accuracy, and the states still missing per runtime. After an intended rule change, regenerate the baseline with `PANEFLOW_SCREEN_CORPUS_BLESS=1` and review its diff.

## Validation

Run:

```powershell
cargo test -p paneflow-agent-config --locked
cargo check -p paneflow-shim -p paneflow-ai-hook -p paneflow-app --locked
```

The catalog suite also rejects the `node`, `sh`, and `python` false positives, refuses a contested alias without `title_prefix`, and classifies the screen corpus against its baseline.
