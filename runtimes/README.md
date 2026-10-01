# Runtime packages

Each built-in coding agent is described by one `runtimes/<slug>/runtime.toml` file. `paneflow-agent-config/build.rs` discovers every descriptor, validates the complete set, and generates the constant Rust registry consumed by the app and helper binaries. No runtime TOML parser is linked into shipping binaries.

Adding a detection-only runtime requires one new directory and no Rust edits. Give it at least one command alias, set lifecycle source, authority and fallback to `none`, select the `none` hook adapter, and add one suggested preset. A runtime with screen fallback also supplies captured `fixtures/working.txt` and `fixtures/idle.txt` files.

## Descriptor

The schema is strict. Unknown fields, unsupported enum values, a directory and `slug` mismatch, malformed reverse-DNS ids, duplicate ids or aliases, and inconsistent lifecycle policy fail the build with the descriptor path.

Required top-level fields:

- `schema_version = 1`
- `id`: stable reverse-DNS identity
- `slug`: identical to the package directory name
- `label`: user-facing name
- `platforms`: any of `linux`, `macos`, and `windows`

`[display]` defines the unique `order`, `tint` as optional `#RRGGBB`, `icon_asset_path`, `icon_multicolor`, and the existing `visibility_config_key`.

`[detection]` defines `command_aliases`, `process_aliases`, and normalized `script_path_signatures`. The first command alias is the canonical executable. Generic interpreters such as `node`, `sh`, and `python` must not be aliases. Script signatures identify their wrapped runtime from an argv path instead.

`[environment]` defines `strip_inherited`, the provider environment variables that must not leak into a nested launch.

`[lifecycle]` defines `source`, `authority`, `fallback`, `escape_cancels_turn`, `attention_clears_on_output`, and `anchor_start_event_to_output`. `authority` states what Paneflow can actually observe: `complete` when Paneflow installs lifecycle hooks for the runtime, `screen` when only its `[screen]` rules report turn status, and `none` otherwise. `fallback = "screen"` requires `[screen]`. `authority = "screen"` requires `fallback = "screen"`, and `authority = "none"` requires `fallback = "none"`. A runtime whose hooks reach Paneflow through another integration may declare `source = "hooks"` with `authority = "none"` when that stream is too partial to be the sole source.

Optional `[screen]` rules contain non-empty `working` and `idle_prompt` pattern arrays. Matching is case-insensitive against the rendered viewport.

`[integration]` defines a user-facing `summary`, optional `post_install_step`, and `hook_adapter`, one of `none`, `claude`, or `codex`. Only `claude` and `codex` have an installer, so `authority = "complete"` holds exactly for them and the build rejects any other pairing, naming the descriptor, the field, and the accepted adapters. A summary describes the integration the runtime really has. A new detection-only runtime uses `hook_adapter = "none"`; adding a new provider-specific hook adapter is an explicit core change that ships its installer.

Each `[[suggested_presets]]` entry defines `id` and `command`. The first preset is the built-in launch command and its id remains the persisted Paneflow agent tag. A descriptor remains available for labels, colors, icons, and detection on every target, while launch presets are offered only on declared platforms.

## Validation

Run:

```powershell
cargo test -p paneflow-agent-config --locked
cargo check -p paneflow-shim -p paneflow-ai-hook -p paneflow-app --locked
```

The catalog suite also rejects the `node`, `sh`, `python`, and `fx` false positives and exercises the Claude Code, Codex, and Gemini screen fixtures.
