# Runtime packages

Each built-in coding agent is described by one `runtimes/<slug>/runtime.toml` file. `paneflow-agent-config/build.rs` discovers every descriptor, validates the complete set, and generates the constant Rust registry consumed by the app and helper binaries. No runtime TOML parser is linked into shipping binaries.

Adding a detection-only runtime requires one new directory and no Rust edits. Give it at least one command alias, set lifecycle source, authority and fallback to `none`, select the `none` hook adapter, and add one suggested preset. A runtime with screen fallback also supplies captured `fixtures/working.txt` and `fixtures/idle.txt` files. The new runtime then appears in the agent launcher at its `display.order`, gets its row in the Settings visibility table under its `display.visibility_config_key`, and receives a PATH shim. The Rust handles such as `TerminalAgent::ClaudeCode` are generated from the slugs by `runtime_identity_constants!`, so the app never repeats a runtime id.

The Windows installer (`packaging/wix/main.wxs`) and the release workflow (`.github/workflows/release.yml`) each list one shim per runtime, named after its first command alias. `cargo test -p paneflow-agent-config` fails, naming the alias, when either list misses a runtime or keeps one the catalog no longer has.

## Descriptor

The schema is strict. Unknown fields, unsupported enum values, a directory and `slug` mismatch, malformed reverse-DNS ids, duplicate ids or aliases, and inconsistent lifecycle policy fail the build with the descriptor path.

Required top-level fields:

- `schema_version = 1`
- `id`: stable reverse-DNS identity
- `slug`: identical to the package directory name
- `label`: user-facing name
- `platforms`: any of `linux`, `macos`, and `windows`

`[display]` defines the unique `order`, `tint` as optional `#RRGGBB`, `icon_asset_path`, `icon_multicolor`, and `visibility_config_key`. `order` sorts the launcher, the visibility table and the session sidebar. `visibility_config_key` is the unique `paneflow.json` key that shows or hides the launcher button: a lowercase snake_case name ending in `_button_visible`, read from the configuration whatever the runtime, so it needs no matching Rust field. When the key is absent or `null`, the button follows install detection of the canonical command alias.

`[detection]` defines `command_aliases`, `process_aliases`, and normalized `script_path_signatures`. The first command alias is the canonical executable. Generic interpreters such as `node`, `sh`, and `python` must not be aliases. Script signatures identify their wrapped runtime from an argv path instead.

`[environment]` defines `strip_inherited`, the provider environment variables that must not leak into a nested launch.

`[lifecycle]` defines `source`, `authority`, `fallback`, `escape_cancels_turn`, `attention_clears_on_output`, and `anchor_start_event_to_output`. `authority` states what Paneflow can actually observe: `complete` when Paneflow installs lifecycle hooks for the runtime, `screen` when only its `[screen]` rules report turn status, and `none` otherwise. `fallback = "screen"` requires `[screen]`. `authority = "screen"` requires `fallback = "screen"`, and `authority = "none"` requires `fallback = "none"`. A runtime whose hooks reach Paneflow through another integration may declare `source = "hooks"` with `authority = "none"` when that stream is too partial to be the sole source.

Optional `[screen]` rules contain non-empty `working` and `idle_prompt` pattern arrays. Matching is case-insensitive against the rendered viewport.

`[integration]` defines a user-facing `summary`, optional `post_install_step`, and `hook_adapter`, one of `none`, `claude`, or `codex`. Only `claude` and `codex` have an installer, so `authority = "complete"` holds exactly for them and the build rejects any other pairing, naming the descriptor, the field, and the accepted adapters. A summary describes the integration the runtime really has. A new detection-only runtime uses `hook_adapter = "none"`; adding a new provider-specific hook adapter is an explicit core change that ships its installer.

The same section drives `paneflow integrations install` and `paneflow mcp install`, which share one engine. Optional `mcp_config` names the writer of the agent's MCP config file and format, one of `claude`, `codex`, `gemini`, or `opencode`; a runtime with an installer hook adapter must declare one, because the integration installs the MCP bridge with the hooks. Optional `skills_dir` names a verified skills directory, today only `claude` (`skills/` under the Claude config directory, honoring `CLAUDE_CONFIG_DIR`), where the engine installs the versioned `paneflow-conductor` skill. Leave it out for a runtime whose skills location is not verified. Each writer is an explicit core change, and the build lists the accepted values for an unknown one.

Optional `[resume]` declares how Paneflow reopens a conversation. `session_argv` and `fork_argv` are argv arrays that contain the `{session_id}` placeholder exactly once as a whole argument, and `continue_argv` reopens the most recent conversation without it. The program of every template must be one of the runtime's `detection.command_aliases`, never the placeholder, and the other arguments use only ASCII letters, digits, `-`, `_`, `.` or `=`. `session_id_pattern` is a regex every stored id must match, on top of Paneflow's own refusal of ids that start with `-` (CWE-88); the build rejects an invalid pattern. `failure_markers` lists texts the CLI prints when it cannot find a conversation, captured from the real CLI into `fixtures/resume-failure.txt`. Declare `fork_argv` only for a fork flag verified on the installed CLI.

Optional `[sessions]` names the `reader` that lists past conversations for the session sidebar: one of `claude`, `codex`, `opencode`, `pi`, `gemini`, `kiro`, or `grok`. A reader is the Rust code that understands one provider's session store, so the build rejects any other value and lists the accepted readers; a new provider store is an explicit core change. Runtimes with the `claude`, `codex`, `opencode`, or `pi` reader can also title tabs automatically, tried in `display.order` when the tab's own agent cannot.

Each `[[suggested_presets]]` entry defines `id` and `command`. The first preset is the built-in launch command and its id remains the persisted Paneflow agent tag. A descriptor remains available for labels, colors, icons, and detection on every target, while launch presets are offered only on declared platforms.

## Validation

Run:

```powershell
cargo test -p paneflow-agent-config --locked
cargo check -p paneflow-shim -p paneflow-ai-hook -p paneflow-app --locked
```

The catalog suite also rejects the `node`, `sh`, `python`, and `fx` false positives and exercises the Claude Code, Codex, and Gemini screen fixtures.
