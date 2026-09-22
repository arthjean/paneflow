[PRD]
# PRD: Agent Runtime System

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-19 | Arthur Jean | Initial draft, decisions fixed after the Unpeel comparison and the Claude Code / Codex documentation review |

## Problem Statement

1. **Busy state can stick forever.** A lost `Stop` (hook timeout, hook disabled by managed settings, provider crash) leaves a pane in `Thinking` with no bound. Today the only recovery is a weaker source taking over after 20 s of silence, which exists only for Claude through an undocumented registry file (`src-app/src/claude_session_registry.rs`). Codex, Gemini and Cursor panes have no recovery at all.
2. **User interrupts are invisible.** Claude Code documents that `Stop` does not fire on a user interrupt. Paneflow detects interrupts only through the process exit code (`crates/paneflow-shim/src/main.rs`), so an Escape mid-turn keeps the spinner until a weaker source overrides it.
3. **Hooks are installed per launch and can orphan.** `crates/paneflow-shim/src/hooks/*.rs` mutates provider config at every launch and cleans it on exit, with lease files and an orphan sweep. A Paneflow crash leaves hook entries behind; two panes launching the same agent race on the same file; an agent started by a script, `tmux`, or a Makefile inside a pane is never observed.
4. **Agent-drawn menus never reach the sidebar.** Claude and Codex numbered menus fire no hook. Paneflow has no viewport detector, so a session waiting for a choice shows as busy.
5. **Adding an agent touches five places.** `WRAPPED_TOOLS`, the `TerminalAgent` enum, `install_hook_guard`, one `hooks/<tool>.rs` module and an alignment test must change together. Antigravity, Muse and dsh were each multi-file commits.
6. **Windows detection misses npm installs.** The Windows process scan reads only the image name (`src-app/src/workspace/ports.rs:612`), so an npm-installed `claude` or `codex` appears as `node.exe` and is not recognized.
7. **The renderer owns lifecycle logic.** Registry polling, terminal observations, the finished sweep and notifications live in the GPUI app (`src-app/src/app/agent_status.rs`). A phone or a second machine cannot consume Paneflow's agent state because there is no Controller protocol with advertised capabilities.

**Why now:** Unpeel (MIT, UX Themes AS) ships a reference design that solves all seven with documented invariants and a real-hardware audit of 14 runtimes. Claude Code now live-reloads hook settings and supports exec-form hooks on every platform, and Codex ships native `Interrupt` and `SessionEnd` hooks. Remote Controllers are the next Paneflow milestone and need the worker split first.

## Overview

Paneflow adopts Unpeel's agent system, ported to Paneflow's three shipping targets and to its existing detached host. Provider knowledge moves into a declarative catalog (`runtimes/<slug>/runtime.toml`) compiled by `build.rs` into const tables. Lifecycle hooks are installed once per machine into each provider's global configuration, gated by `PANEFLOW_SESSION_ID` so they are inert outside a hosted pane, and delivered to the host socket with a durable on-disk seed as fallback. The Unix hook scripts are ported from Unpeel in exec form; on Windows the same hook entries point at `paneflow-ai-hook.exe`.

The process topology splits into a frozen PTY core (`paneflow-host`: PTY, manifests, tail, viewport scan, foreground observation), a restartable per-home worker (`paneflow serve`: activity reducer, notifications, sidebar projection, Controller protocol with advertised capabilities), and Controllers (the GPUI app first, remote clients later). The reducer implements Unpeel's `HookState` model: hooks are the single authority once latched, a five-minute lease re-armed only by screen-hash changes bounds a lost `Stop`, runtime generations reject stale events, and a durable seed is replayed after restarts. Below the hooks sits a declarative screen tier per runtime and a host-side menu detector, both published as lower-confidence sources that never produce completion notifications.

Key decisions: socket transport instead of Unpeel's HTTP port registry (ordering, ACLs, one reducer); binary hook on Windows instead of shell scripts (Claude falls back to PowerShell when Git Bash is absent; exec form spawns without a shell); no Codex `notify` (native hooks now carry `Stop` and `Interrupt`); Escape fence limited to Claude, Gemini and Muse; the Claude session registry and Codex transcript reader are removed as undocumented or unstable interfaces.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Bounded busy state: no pane stays busy more than 5 min after its last screen change without a hook | 100% of hook-owned sessions in the CLI PTY matrix | 0 user reports of a stuck spinner |
| Interrupt latency: Escape in Claude settles the pane state | < 1 s in the PTY regression | < 1 s on all three platforms |
| Adding a runtime is one directory | Antigravity re-added from a single `runtime.toml` in CI | 3 community runtime packages merged |
| Hook install is idempotent and survives crashes | 0 orphan entries after `kill -9` of Paneflow in the PTY matrix | 0 support issues about stale hook config |
| Windows npm detection | `node.exe` running `@anthropic-ai/claude-code` or `@openai/codex` recognized in the unit suite | 100% of Windows sessions in telemetry carry a runtime id when an agent runs |

## Target Users

### Multi-agent developer
- **Role:** Runs 3 to 12 coding agents in parallel panes, mostly Claude Code and Codex, some Gemini, OpenCode, Cursor Agent.
- **Behaviors:** Launches agents from presets and by hand, interrupts turns with Escape, answers permission prompts and numbered menus, leaves sessions running overnight, restarts Paneflow after updates.
- **Pain points:** Spinners that never stop, no badge when an agent waits on a menu, hooks left in `~/.claude/settings.json` after a crash, npm-installed agents unrecognized on Windows.
- **Current workaround:** Clicks into each pane to check, removes hook entries by hand, installs the native Claude binary to get detection.
- **Success looks like:** The sidebar dot is right within one second of the agent's real state, and stays right after an update or a crash.

### Runtime package contributor
- **Role:** Paneflow maintainer or community contributor adding or fixing an agent CLI.
- **Behaviors:** Reads the provider's hook documentation, writes a descriptor, runs the runtime conformance tests.
- **Pain points:** Five files to edit in lockstep, no schema, no way to declare Windows support or screen rules.
- **Current workaround:** Copies the closest existing module and patches the alignment test.
- **Success looks like:** One directory, a schema error at build time when a field is wrong, and the runtime tests passing without touching core.

### Remote Controller (next milestone)
- **Role:** Paneflow on a phone or a second machine.
- **Behaviors:** Wants the same session list, states and attention queue as the desktop.
- **Pain points:** No protocol exists; the state lives in the renderer.
- **Current workaround:** None.
- **Success looks like:** A worker that advertises capabilities and serves the same projection to every Controller.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Unpeel reference checkout:** `C:/dev/unpeel` at commit `443877b` (2026-09-17). Every `Unpeel reference` line below is relative to that root; read the referenced file before implementing the story.
- **Unpeel** (github.com/unpeel-com/unpeel, MIT): three-tier activity model (hooks, screen tier, detection), global env-gated hook integrations, PTY core / worker / Controller split, `runtime.toml` catalog with `build.rs` registry, host-side menu detector, Escape cancellation fence, durable `last-hook-event.json` seed, runtime generations. macOS and Linux only; hooks are bash + curl; transport is an HTTP port registry. Paneflow reuses the model and the reusable Rust pieces (menu detector, screen classifier, reducer) under the MIT notice, and replaces the transport and the hook vehicle.
- **Paneflow today** (`crates/paneflow-host/src/agent.rs`, `crates/paneflow-ipc-client/src/agent.rs`): three ranked sources with a 20 s takeover, per-launch hook installs through the PATH shim, reducer in the host but lifecycle logic also in the GPUI app.
- **Market gap:** no cross-platform terminal workspace gives exact, bounded, restart-safe agent states on native Windows.

### Best Practices Applied
- Claude Code hooks: exec form (`command` + `args`) spawns the executable with no shell on every platform; settings edits to `hooks` apply live to running sessions; the hook process inherits the parent environment except `OTEL_*`; `Stop` does not fire on user interrupts; `PermissionRequest` ignores exit 2 and treats stdout JSON as a decision; `Stop` carries `background_tasks` and `session_crons`; managed settings can set `disableAllHooks` or `allowManagedHooksOnly` (code.claude.com/docs/en/hooks, hooks-guide, settings).
- Codex hooks: `~/.codex/hooks.json` with `Interrupt`, `SessionEnd`, `SubagentStart`, `SubagentStop`, `PermissionRequest`; `command_windows` override; `Interrupt` and `SessionEnd` default timeout 1 s, max 3 s; hook process gets `env_clear()` plus the session-start environment snapshot; hooks are spawned by the hooks engine outside the sandbox; non-managed hooks are skipped until trusted in `/hooks`, trust keyed by the hash of the definition (codex-rs/hooks/src/engine/{command_runner,discovery}.rs, learn.chatgpt.com/docs/hooks).
- Unpeel's runtime audit (docs/agents/clients/session-activity.md, 2026-09-06): native cancellation events for Codex, Kimi, Grok, Cursor, Amp, Cline; Escape fence verified for Claude, Gemini, Muse; nothing inferred for Copilot and Kiro.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Claude's `AskUserQuestion` reaches `PermissionRequest` with `tool_name == "AskUserQuestion"` (Agent SDK documents the tool on the `canUseTool` layer; the hooks page section did not surface). Validated by a logging hook in US-004.
- Exec-form hooks on native Windows deliver the JSON payload on stdin to a `.exe` exactly as on Unix (no Windows caveat documented). Validated in US-004.
- Codex hook trust survives a Paneflow upgrade when the `hooks.json` entry is byte-identical (trust is keyed by the definition hash; the exact hashed fields were not confirmed). Validated in US-005.
- OSC 9;4 progress and semantic terminal titles are emitted natively only by Claude Code and Codex among the 18 catalog runtimes (no citable evidence for the others).

### Hard Constraints
- Linux (Wayland and X11), macOS (Intel and Apple Silicon), Windows 10/11 native: every story ships a working path on all three or documents the stub.
- Helper binary caps enforced by CI: `paneflow-shim` 512 KiB, `paneflow-ai-hook` ~375 KB, `paneflow-mcp` 512 KiB. No TOML parser at runtime in any of them.
- GPUI pinned to one Zed revision; the render thread never blocks (no synchronous file I/O, subprocess, or directory walk on the main thread).
- Launches stay plain: no wrapper on PATH beyond the existing shim, no appended flag, no minted conversation id, no provider config edit at launch.
- Code copied from Unpeel keeps the MIT notice in `THIRD_PARTY_NOTICES` or the equivalent; Unpeel's name, logo and mascot are never used.
- No comments in Rust, shell or PowerShell sources; intent goes into names, types, tests and the documents in `docs/`.
- CI runs every cargo invocation `--locked`; the pinned toolchain is 1.98.0.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, mandatory before every commit and push
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint, including test modules
- `cargo test --workspace --locked` - unit and integration suites
- `cargo deny check advisories licenses sources` - only when `Cargo.lock` changes
- `cargo test --workspace --no-fail-fast` run natively on Windows for any story touching `#[cfg(windows)]` code, hook delivery, or process scanning (CI skips Windows tests)

For UI stories, additional gates:
- Arthur performs the visual pass himself on a debug build; the story reports what was verified on which platform and what was only reviewed by inspection.

## Epics & User Stories

### EP-001: Runtime Catalog

Move every per-provider fact into `runtimes/<slug>/runtime.toml` compiled by a `build.rs` in `paneflow-agent-config`, so that adding or fixing a runtime is one directory and the new reducer reads its policy from data.

**Definition of Done:** All 18 runtimes have descriptors; `WRAPPED_TOOLS`, the `TerminalAgent` enum, the `install_hook_guard` match and the alignment test are gone; a malformed descriptor fails `cargo build`; helper binary sizes are unchanged within 4 KiB.

**Unpeel reference:** `runtimes/README.md` (package layout, descriptor fields, package rules), `crates/unpeel-core/build.rs` (discovers `runtime.toml`, generates the package module), `crates/unpeel-core/src/runtime_catalog.rs` and `runtime_catalog_schema.rs` (typed registry and strict schema), `scripts/generate-runtime-client-catalog.mjs` (client-safe catalog, not needed here because Paneflow has no Swift client), `docs/agents/providers.md` §Adding a built-in agent runtime.

#### US-001: Catalog schema and generated registry
**Description:** As a runtime package contributor, I want a strict `runtime.toml` schema compiled into const tables so that a wrong field is a build error and no binary parses TOML at runtime.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None
**Unpeel reference:** `crates/unpeel-core/build.rs`; `crates/unpeel-core/src/runtime_catalog_schema.rs` (field validation, duplicate identity and alias rejection, lifecycle/capability consistency rules such as `authority = "none"` requiring `fallback = "none"` and `fallback = "screen"` requiring `[screen]`); `runtimes/README.md` §Descriptor; `runtimes/claude-code/runtime.toml` as the canonical full example.

**Acceptance Criteria:**
- [ ] Given `runtimes/<slug>/runtime.toml` files, when `paneflow-agent-config` builds, then `build.rs` discovers them without a hand-written list and emits a `pub static RUNTIMES: &[Runtime]` table with: `id` (reverse-DNS), `slug`, `label`, `platforms`, `capabilities`, `display` (tint, icon asset path), `detection` (`command_aliases`, `process_aliases`, `script_path_signatures`), `environment.strip_inherited`, `lifecycle` (`source`, `authority`, `fallback`, `escape_cancels_turn`, `attention_clears_on_output`, `anchor_start_event_to_output`, `terminal_title_signal`), optional `screen` (`working`, `idle_prompt`), `integration` (`summary`, `post_install_step`), `install.command`, `suggested_presets`.
- [ ] Given a descriptor with an unknown field, a duplicate `id`, a duplicate alias across runtimes, a `slug` that differs from its directory name, `fallback = "screen"` without a `[screen]` table, or `authority = "none"` with `fallback != "none"`, when building, then the build fails with a message naming the file and the field.
- [ ] Given the `paneflow-shim` and `paneflow-ai-hook` crates, when they consume the tables, then neither links `toml` or `serde` at runtime and the binary-size budget job reports each within 4 KiB of the current size.
- [ ] Given a descriptor declaring `platforms = ["linux", "macos"]`, when built for Windows, then the runtime is still present for presentation (label, tint, icon) but excluded from launch presets and integration install.
- [ ] Given the schema documentation in `runtimes/README.md`, when a contributor follows it for a runtime with only `runtime.toml`, then detection and, if declared, screen rules work with no Rust code.

#### US-002: Migrate consumers to the catalog
**Description:** As a maintainer, I want the shim, the app, the hook binary and the installer to read the catalog so that the five hand-maintained lists disappear.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001
**Unpeel reference:** `crates/unpeel-core/src/runtime_observer.rs::runtime_id_for_executable` and `runtime_id_from_script_path` (catalog aliases and `script_path_signatures` as the only detection source); `crates/unpeel-core/src/screen_activity.rs::rules_for_runtime` (catalog-driven policy lookup); `crates/unpeel-serve/src/runtime_presentation.rs` (label, tint, lifecycle policy read from the catalog by the worker); `crates/unpeel-core/src/runtime_catalog.rs` tests `runtime_catalog` (run by `bun run validate:runtimes`).

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-shim/src/detect.rs`, when the shim resolves the wrapped tool, then it uses the catalog's `command_aliases` and `WRAPPED_TOOLS` no longer exists.
- [ ] Given `src-app/src/agent_launcher.rs`, when the app needs a runtime's label, icon, tint or launch detection, then it reads the catalog and the `TerminalAgent` enum is replaced by a catalog runtime reference.
- [ ] Given `crates/paneflow-ai-hook`, when it detects the tool from `PANEFLOW_AI_TOOL` or the payload, then it matches catalog aliases.
- [ ] Given the test `wrapped_stems_match_shim_detect_list`, when the migration lands, then it is replaced by a catalog conformance test that asserts every descriptor has at least one alias and one false-positive negative case (`node`, `sh`, `python`, `fx` as JSON viewer).
- [ ] Given a runtime not present in the catalog, when a user launches it, then the pane reports the command as itself with no runtime identity and no error.

#### US-003: Descriptors for the 18 shipped runtimes
**Description:** As a multi-agent developer, I want every currently supported agent described in the catalog with verified policy so that behavior is unchanged where it worked and declared honestly where it did not.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001
**Unpeel reference:** `runtimes/*/runtime.toml` for the 14 shipped packages (`claude-code`, `codex`, `gemini` carry `[screen]` rules; `grok` carries `attention_clears_on_output = false` and `anchor_start_event_to_output = false`; `pi`, `fx`, `antigravity` carry `authority = "none"`); `docs/agents/providers.md` (per-provider hook details and verified behavior); `docs/agents/clients/session-activity.md` §Runtime cancellation coverage (audit table dated 2026-09-06); `runtimes/claude-code/fixtures/approval-menu.txt` and `crates/unpeel-core/src/screen_activity.rs` tests (captured working and idle screens).

**Acceptance Criteria:**
- [ ] Given the 18 tools currently in `WRAPPED_TOOLS`, when the catalog is complete, then each has a descriptor with `platforms` set to `["linux", "macos", "windows"]` only for Claude, Codex, Gemini, GitHub Copilot and Amp, and `["linux", "macos"]` otherwise.
- [ ] Given Claude and Codex, when their descriptors are read, then `strip_inherited` lists `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PID` and `CODEX_CI`, `CODEX_SHELL`, `CODEX_THREAD_ID`, `CODEX_TUI_RECORD_SESSION`, `CODEX_TUI_SESSION_LOG_PATH` respectively, `script_path_signatures` carry `@anthropic-ai/claude-code` and `@openai/codex`, `lifecycle.fallback = "screen"` with `working = ["… (", "esc to interrupt"]` / `idle_prompt = ["❯"]` for Claude and `working = ["esc to interrupt", "• Working"]` / `idle_prompt = ["›"]` for Codex, and `terminal_title_signal = true`.
- [ ] Given Gemini, when its descriptor is read, then `fallback = "screen"` with `working = ["esc to cancel"]`, `idle_prompt = ["Type your message", "> Type your message"]`, `escape_cancels_turn = true`.
- [ ] Given Grok, when its descriptor is read, then `attention_clears_on_output = false` and `anchor_start_event_to_output = false`.
- [ ] Given Pi, fx, Antigravity, dsh and any runtime without a hook contract, when their descriptors are read, then `authority = "none"` and `fallback = "none"`, and `paneflow integrations install <slug>` is refused with a message naming the missing capability.
- [ ] Given a descriptor whose `[screen]` rules are wrong for a captured viewport, when the package tests run, then a fixture under `runtimes/<slug>/fixtures/` fails, so each of Claude, Codex and Gemini ships one working and one idle fixture.

---

### EP-002: Global Hook Integrations

Replace per-launch hook installation with one user-triggered install per machine into each provider's global configuration, inert outside a hosted pane, delivered to the host socket with a durable seed fallback.

**Definition of Done:** `crates/paneflow-shim/src/hooks/` and its lease and orphan-sweep code are deleted; a `kill -9` of Paneflow leaves provider config unchanged; a hand-typed `claude` in a pane reports through hooks; the same conformance suite passes for the Unix scripts and the Windows binary.

**Unpeel reference:** `AGENTS.md` §Hard invariants ("Hooks are the busy/idle authority", "Launches are plain; integrations are explicit"); `docs/agents/providers.md` §Provider Hook Details; `crates/unpeel-core/src/integrations/install.rs` (install, adoption, refresh, MCP shim); `runtimes/<slug>/assets/hooks/` (per-provider reporters); `runtimes/hook_reporter_tests.rs` and `runtimes/test_support.rs` (transport conformance).

#### US-004: Hook reporter with env gate, generation, seed and bounded delivery
**Description:** As a multi-agent developer, I want the hook reporter to be inert outside Paneflow, tag every event with the runtime generation, write a durable seed when the host is unreachable, and return within the provider's timeout so that state is exact and no provider is slowed down.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002
**Unpeel reference:** `runtimes/claude-code/assets/hooks/lifecycle.sh` (line 5 env gate; `post_hook_payload` with `--connect-timeout 0.2 --max-time 1`; `add_runtime_generation_to_payload`; `record_last_hook_event` temp-file plus `mv -f`, never creating the session dir; `SessionStart → HookSeen` rewrite; Grok `GROK_SESSION_ID` guard); `runtimes/codex/assets/hooks/` (notify normalizer mapping raw Codex events onto `Start`/`Stop`/`PermissionRequest`); `runtimes/hook_reporter_tests.rs` (stalled ports, proxy variables, duplicate ports, generation tags, restart seeds, silent outside a session); `crates/unpeel-core/src/hook_assets/scripts.rs` (`write_executable_script`, locked atomic replacement); `crates/unpeel-serve/src/activity.rs::normalize_event_name` and `is_latch_only` (which events must be latch-only, including `SubagentStart`/`SubagentStop`).

**Acceptance Criteria:**
- [ ] Given no `PANEFLOW_SESSION_ID` in the environment, when any hook event arrives on stdin, then the reporter exits 0 with no output, no file write and no socket connection, within 50 ms.
- [ ] Given `PANEFLOW_SESSION_ID`, `PANEFLOW_SESSION_DIR`, `PANEFLOW_HOST_ENDPOINT` and `PANEFLOW_RUNTIME_GENERATION` set, when a `Stop` event arrives, then the reporter posts a frame carrying `runtime_generation` as a number and `received_at` is stamped by the host on the complete frame.
- [ ] Given the host endpoint unreachable, when a lifecycle event arrives, then the reporter atomically writes `<session dir>/last-hook-event.json` with `hook_event_name`, `tool_name` and `runtime_generation`, using a temp file plus rename, and never creates the session directory.
- [ ] Given a `PermissionRequest` event, when the reporter handles it, then it writes nothing to stdout, so Claude's permission flow proceeds unchanged.
- [ ] Given a Codex `Interrupt` or `SessionEnd` event, when the reporter runs, then total wall time is under 500 ms with the host reachable and under 800 ms with a stalled endpoint, measured in the conformance suite.
- [ ] Given `SubagentStop`, `SubagentStart` and `SessionStart`, when the reporter forwards them, then the host reduces them as latch-only (`HookSeen`) and never as `Stop`; the current `SubagentStop → Stop` mapping in `crates/paneflow-ai-hook/src/event.rs` is removed.
- [ ] Given the Unix `lifecycle.sh` script ported from Unpeel and the Windows `.exe`, when the conformance suite runs (stalled ports, duplicate endpoints, proxy variables set, generation tag, seed on failure, silent outside a session), then both transports pass the same test cases.
- [ ] Given a real Claude session on Windows launched with the exec-form hook, when a prompt is submitted, then the reporter receives the JSON payload on stdin and the PTY regression asserts the `UserPromptSubmit` frame; and given `AskUserQuestion` is invoked, then the test records which hook events fired with which `tool_name`, and the reducer's latch-only rule is adjusted to that finding.

#### US-005: Claude and Codex installers
**Description:** As a multi-agent developer, I want `paneflow integrations install claude|codex` to merge byte-stable hook entries and the MCP shim into the provider's global config, idempotently and without touching my other entries, so that every launch reports without per-launch mutation.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004
**Unpeel reference:** `runtimes/claude-code/adapter/setup.rs` (`reconcile_claude_hooks` line 214: event list, `async: false`, `timeout: 5`, matcher `*` on `PermissionRequest`, migration of owned `async: true` entries; `reconcile_claude_mcp_servers` line 55: user-scope entry in `~/.claude.json`, pruning of Unpeel-owned legacy names); `runtimes/codex/adapter/setup.rs` (`reconcile_codex_hooks_json` line 380: reconciliation preserving side-by-side `UNPEEL_HOME` instances and pruning obsolete entries; `reconcile_codex_config_toml` line 176: `[mcp_servers.unpeel]` merge with `toml_edit` preserving comments); `crates/unpeel-core/src/integrations/install.rs` (`write_mcp_shim` line 106, `install_in` line 247, `adopt_legacy_installs_in` line 299 with the `adopted_from` marker and `legacy_evidence`, `refresh_installed_in` line 338); `crates/unpeel-core/src/hook_assets/mod.rs` (`read_mergeable_json_object`, `write_file_atomic`, flock on `<file>.lock`); `docs/agents/providers.md` §Claude and §Codex.

**Acceptance Criteria:**
- [ ] Given `~/.claude/settings.json` with foreign hooks, when the Claude installer runs, then it adds exec-form entries (`command` = the stable reporter path under `~/.paneflow/bin`, `args` = `["<event>"]`, `timeout: 5`, `async: false`, matcher `*` on `PermissionRequest`) for `SessionStart`, `UserPromptSubmit`, `Stop`, `StopFailure`, `PermissionRequest`, `SubagentStart`, `SubagentStop`, `Notification`, preserves every foreign entry byte for byte, and a second run produces no diff.
- [ ] Given `~/.claude.json`, when the Claude installer runs, then the `paneflow` MCP server is registered in user scope pointing at the stable shim, and Paneflow-owned legacy names are pruned while user names are kept.
- [ ] Given `~/.codex/hooks.json`, when the Codex installer runs, then it adds entries for `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Stop`, `Interrupt`, `SubagentStart`, `SubagentStop`, `SessionEnd` with `command` (Unix) and `command_windows` (the `.exe`) both absolute and version-free, `timeout` 1 for `Interrupt` and `SessionEnd`, and does not write `notify` or `features.hooks`.
- [ ] Given `~/.codex/config.toml`, when the Codex installer runs, then `[mcp_servers.paneflow]` points at the shim with no `env` table, and unrelated tables are preserved including comments.
- [ ] Given an installed integration and a Paneflow upgrade, when the worker starts, then it re-runs the installer and the resulting Codex entry is byte-identical so the trust hash is unchanged; the test asserts the file content before and after.
- [ ] Given hook entries written by the current per-launch shim (`_paneflow_managed` marker), when the worker starts after the upgrade, then it adopts them as an installed integration, rewrites them to the global form once, and records `adopted_from` in the integration marker.
- [ ] Given a config file that is a symlink or is not valid JSON/TOML, when the installer runs, then it refuses with the path and the reason and changes nothing.
- [ ] Given two concurrent installer runs, when they race, then an exclusive lock on `<file>.lock` serializes them and the final file is identical to a single run.

#### US-006: Integration management surface
**Description:** As a multi-agent developer, I want a CLI verb and a Settings ▸ Agents panel that show each runtime's integration state and let me install or remove it, including the Codex trust step, so that the one-time setup is discoverable.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005
**Unpeel reference:** `docs/agents/cli.md` (`unpeel integrations install|list`, refusal for detection-only runtimes such as Pi); `crates/unpeel-core/src/controller_api.rs` (`integrations.install` Host verb behind Settings ▸ Agents ▸ Install integration); `runtime.toml` `[integration]` block (`summary`, `manual_command` with `{shim}`, `legacy_evidence`) in `runtimes/claude-code/runtime.toml` and `runtimes/codex/runtime.toml`; `clients/native/UnpeelNative/Sources/UnpeelNative/Views/SettingsView.swift` (Agents section: install state per runtime, summary copy, install and remove actions).

**Acceptance Criteria:**
- [ ] Given the `paneflow` CLI, when `paneflow integrations list` runs, then it prints one row per catalog runtime with `installed`, `not installed`, `not supported on this platform`, or `no integration (detection only)`, and exits 0.
- [ ] Given `paneflow integrations install codex`, when it completes, then the output states that Codex requires trusting the new hooks once in `/hooks` and the Settings panel shows the same `post_install_step` text from the descriptor.
- [ ] Given `paneflow integrations remove claude`, when it completes, then only Paneflow-owned entries are removed and the hook script is left in place if another `PANEFLOW_HOME` instance still references it.
- [ ] Given Settings ▸ Agents, when it renders, then each row uses the existing `squircle_skin`, `ROW_RADIUS` and semantic colors, shows the `integration.summary` text, and offers Install or Remove; no ad hoc border or radius is introduced.
- [ ] Given a runtime with `authority = "none"`, when the user opens its row, then Install is absent and the row explains that the runtime offers detection only.
- [ ] Given the installer fails, when the panel shows the result, then the exact path and reason appear and the row stays `not installed`.

#### US-007: Retire per-launch hooks, the Claude registry and the Codex transcript reader
**Description:** As a maintainer, I want the shim to stop mutating provider config and the app to stop reading undocumented provider files so that one code path owns lifecycle.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005
**Unpeel reference:** `AGENTS.md` §"Launches are plain; integrations are explicit" (what a launch may and may not do); `docs/agents/providers.md` first section ("Launching is provider-neutral": the exact generic environment exported, no wrapper, no flag, no minted id); `docs/agents/session-model.md` §Resume recipes (`transcripts::transcript_provider_for_manifest` resolves launch command → captured runtime → live observation, and the hook-captured `transcript_path` / `provider_session_id` as the only transcript anchors); `docs/agents/clients/session-activity.md` ("Known hook-capable tools do not use raw output growth to enter busy while waiting for the first hook event").

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-shim/src/hooks/` and `hooks.rs`, when the story lands, then the per-launch install, `with_last_lease`, `with_orphan_lease`, `sweep_orphan_hook_config` and `cleanup_hook_config_file` are deleted and the shim only records the real PID and start time, sets the session environment, execs, and emits `session_start`, `exit`, `session_end`.
- [ ] Given `src-app/src/claude_session_registry.rs` and the 400 ms registry poll in `src-app/src/app/agent_status.rs`, when the story lands, then both are removed and no code reads `~/.claude/sessions`.
- [ ] Given `src-app/src/codex_sessions.rs`, when the story lands, then transcript discovery uses the `transcript_path` captured from Codex hooks and no longer parses rollout files by directory scan.
- [ ] Given a `kill -9` of the Paneflow process while three Claude panes run, when the PTY matrix inspects `~/.claude/settings.json`, then the file is byte-identical to its post-install state.
- [ ] Given a hand-typed `claude` in a blank pane with the integration installed, when a prompt is submitted, then the pane latches as hook-owned; and given the integration is not installed, then the pane stays neutral with runtime identity only.

#### US-008: Host session environment and seed on receipt
**Description:** As a multi-agent developer, I want the host to export the session variables into every pane shell and to persist each accepted hook event, so that hooks find their session and state survives a host or worker restart.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004
**Unpeel reference:** `crates/unpeel-core/src/session_host.rs` (session environment export: `UNPEEL_SESSION_ID`, `UNPEEL_SESSION_DIR`, `UNPEEL_APP_PORT`, `UNPEEL_HOST_BIN`, `UNPEEL_RUNTIME_GENERATION`; `strip_inherited` applied from the catalog before the PTY starts; `HERDR_*` and `CODEX_*` hygiene); `crates/unpeel-serve/src/hook_listener.rs` (200 only for a session whose manifest exists in this home, 404 otherwise so foreign instances never swallow events; `MAX_BODY_BYTES`, `IO_TIMEOUT`, `received_at` stamped on the complete request); `crates/unpeel-core/src/hook_assets/expiry.rs` and `lifecycle.sh::record_last_hook_event` (the shared seed file shape `{"hook_event_name","tool_name","unpeel_runtime_generation"}`).

**Acceptance Criteria:**
- [ ] Given a hosted session, when its shell starts, then `PANEFLOW_SESSION_ID`, `PANEFLOW_SESSION_DIR`, `PANEFLOW_HOST_ENDPOINT`, `PANEFLOW_RUNTIME_GENERATION` and `PANEFLOW_HOME` are exported, and every variable listed in any catalog runtime's `strip_inherited` is removed from the child environment.
- [ ] Given an accepted hook frame, when the host stores it, then it writes `<session dir>/last-hook-event.json` atomically with the same shape as the reporter's fallback, so either writer produces an identical file.
- [ ] Given a frame with `runtime_generation` lower than the manifest's, when the host receives it, then it is rejected, logged with the reason, and not persisted.
- [ ] Given the socket is a Unix socket, when created, then its mode is 0600; given a Windows named pipe, then its ACL grants access only to the current user; a test asserts each.
- [ ] Given a frame larger than 64 KiB or with a session id whose manifest does not exist in this home, when received, then it is refused with a 404-equivalent error and the connection is closed.

---

### EP-003: PTY Core, Worker and Controller Split

Freeze `paneflow-host` as the PTY core and move every restartable concern into a per-home `paneflow serve` worker that serves Controllers over an advertised-capability protocol.

**Definition of Done:** A worker restart leaves every terminal and its scrollback intact; the GPUI app runs against the worker with no lifecycle logic left in `src-app/src/app/agent_status.rs`; the protocol advertises capabilities at bootstrap and a conformance case file exists.

**Unpeel reference:** `AGENTS.md` §Hard invariants ("Hosts and Controllers, one protocol", "Terminals survive restarts through the Host, not the renderer", "Process identity before any signal", "The state bus"); `docs/agents/pty-core.md` (one detached PTY core per workspace, sessions as threads); `docs/agents/serve.md` (the workspace worker); `crates/unpeel-serve/src/service.rs`, `driver.rs`, `pty_core_supervisor.rs`; `protocol/host-capabilities-v1.json` and `protocol/host-conformance-v1.json`; `crates/unpeel-core/src/state_bus.rs`.

#### US-009: Freeze the PTY core protocol
**Description:** As a maintainer, I want the PTY core to expose a versioned, minimal protocol so that it never needs a restart when the worker or the app evolves.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008
**Unpeel reference:** `docs/agents/pty-core.md`; `crates/unpeel-core/src/pty_core.rs` (core process, handoff and adoption); `crates/unpeel-core/src/session_host.rs` (`HostedSessionManifest` with `host_protocol_version`, `host_build_id`, `pid_started_at`; `reap_dead_sessions` verifying pid against kernel start time; identity-guarded kill of the verified foreground group); `docs/agents/session-model.md` §Restart Recommendation API (`host_protocol_version` as the only restart lever, `host_build_id` diagnostic only) and §Session Model (`pid_started_at` incident of 2026-07-09); `protocol/host-conformance-v1.json`; PTY cases `crates/unpeel-cli/tests/cases/pty_core_adopt.py`, `pty_core_handoff.py`, `pty_core_parity.py`, `host_launch_conformance.py`.

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-host`, when the story lands, then its responsibilities are exactly: PTY pair, child process and kernel start time, libghostty terminal and viewport, output tail, session manifests, hook ingress with seed on receipt, viewport scan (EP-005), foreground observation; the activity reducer, notifications, sidebar projection and auto-archive are no longer compiled into it.
- [ ] Given a manifest, when written, then it carries `host_protocol_version` and `host_build_id`; `host_build_id` is diagnostic only and no restart UI reads it.
- [ ] Given a worker built against protocol version N, when it connects to a core reporting N-1, then it publishes a `restart recommended` token per session instead of failing, and the token changes only when the required version changes.
- [ ] Given `protocol/host-conformance-v1.json`, when `cargo test -p paneflow-host` runs, then every listed case (bootstrap, session list, output stream, input, resize, agent snapshot, hook ingress rejection) passes against the real core.
- [ ] Given a core that crashed, when the worker's health refresh runs, then a session is marked stopped only when its recorded child pid with matching start time is absent; an unknown or recycled pid stays non-resumable and is never signaled.

#### US-010: `paneflow serve` worker lifecycle
**Description:** As a multi-agent developer, I want a per-home worker that owns the reducer and can restart freely so that updates and crashes never touch my terminals.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009
**Unpeel reference:** `crates/unpeel-serve/src/service.rs` (`run_service` line 485, `run_workspace_worker` line 578, `spawn_workspace` line 421: one worker per workspace home, detached spawn); `crates/unpeel-serve/src/driver.rs` (`run` line 2036: the worker main loop; `spawn_integrations_refresh` line 1973: re-running installers of installed integrations at start; `spawn_reap` line 1407); `crates/unpeel-serve/src/pty_core_supervisor.rs` (`spawn` line 753: supervising the PTY core without restarting it on worker restart); `crates/unpeel-serve/src/auto_archive.rs` (the sweep that moved from the app into the worker); `docs/agents/serve.md`; `crates/unpeel-core/src/state_bus.rs` (`announce`, `flush` before one-shot CLI exit); PTY cases `serve_install.py`, `compat_serve.py`, `state_bus.py`.

**Acceptance Criteria:**
- [ ] Given a home directory, when the app or `paneflow serve start` boots, then one worker owns `<home>/serve/owner.lock`, a second start exits 0 with `already running`, and the worker is spawned detached on every platform (Windows: breakaway from the job object, no console, explicit inherited-handle list).
- [ ] Given a running worker, when it is killed and restarted, then every session's state is rebuilt from manifests and seeds within 2 s and no terminal receives a signal.
- [ ] Given the worker, when it starts, then it re-runs the installers of installed integrations (US-005) and adopts legacy per-launch installs once.
- [ ] Given an app update that ships a newer worker, when the app starts, then it stops the old worker after a 5 s drain and starts the new one, and the sessions list is identical before and after.
- [ ] Given the worker cannot bind its endpoint, when the app boots, then it shows the exact error and offers Retry; no pane opens dead.
- [ ] Given `paneflow serve status`, when run, then it prints the worker pid, protocol version, home, session count and the advertised capabilities as JSON.

#### US-011: The GPUI app as a Controller
**Description:** As a maintainer, I want the app to consume the worker's projection instead of computing lifecycle itself so that every Controller sees the same state.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010
**Unpeel reference:** `docs/agents/clients/session-activity.md` first paragraph (since 2026-09-03 the app ingests no hook events; `HookEvent`/`handleHookEvent`, the busy sweep and `persistActivitySnapshot` were deleted; `SessionActivity.swift` is a read-only startup seed of `activity-state.json` / `last-hook-event.json`); `crates/unpeel-serve/src/activity_snapshot.rs` (`activity_status`, `raw_status`, `activity_source` words; `status_words` line 140); `crates/unpeel-serve/src/sessions.rs` (session summary JSON near line 1855: `status`, `activity`, `activitySource`, `updatedAtUnixMs`, `capabilities`); `docs/agents/session-model.md` §Recent dropdown source ("the native disk scan is only the startup seed"); `clients/native/UnpeelNative/Sources/UnpeelNative/SessionActivity.swift` and `UnpeelStore.swift` (`observedForegroundIdentities`, `restartRecommendations`).

**Acceptance Criteria:**
- [ ] Given `src-app/src/app/agent_status.rs`, when the story lands, then `apply_observed_agent_state`, `schedule_finished_sweep`, `sweep_claude_session_registry` and the OSC-to-lifecycle mapping are removed; the app subscribes to worker `agent.snapshot` and `agent.event` frames.
- [ ] Given a bootstrap from the worker, when the app connects, then it reads the advertised capabilities and never probes for a feature by trying it.
- [ ] Given a worker frame with `activity_source` `hooks`, `screen`, or `none`, when the sidebar renders, then the spinner tint uses the runtime's `spinner_tint`, screen-sourced busy renders the same spinner, and only hook-sourced `Finished` triggers the completion notification.
- [ ] Given the app is closed and reopened while agents run, when the sidebar first renders, then the states shown equal the worker's snapshot with no flash of idle; the startup seed path is the worker snapshot, not a local disk scan.
- [ ] Given the worker connection drops, when the app detects it, then panes keep rendering from the core and the sidebar shows a muted `reconnecting` state until the worker returns; no agent state is invented locally.

---

### EP-004: Activity Reducer

Implement Unpeel's `HookState` reducer in the worker: hooks own state once latched, a screen-re-armed lease bounds a lost stop, generations reject stale events, and the durable seed replays after restarts.

**Definition of Done:** The reducer unit suite ports Unpeel's `activity.rs` cases; the CLI PTY matrix proves bounded busy, stale generation rejection and restart recovery on Linux and Windows.

**Unpeel reference:** `crates/unpeel-serve/src/activity.rs` (the whole reducer and its `mod tests`); `crates/unpeel-serve/src/sessions.rs::derive_status_with_source` line 299 (how the worker combines hook state, screen tier and menu flag); `docs/agents/clients/session-activity.md` §Session Activity State (every edge documented).

#### US-012: HookState reducer with lease, generation and foreground identity
**Description:** As a multi-agent developer, I want busy, idle and attention to follow hooks exactly, with a bounded lease, so that a lost stop never sticks and a stale process never speaks for a new one.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010
**Unpeel reference:** `crates/unpeel-serve/src/activity.rs` (`HOOK_IDLE_TIMEOUT` line 14; `LEGACY_GENERATION_STOP_GUARD` line 22; `Entry` fields lines 32-83; `normalize_event_name` line 93; `is_latch_only` line 127; `apply_hook_event_unchecked` and the match on canonical names; `apply_hook_event_for_runtime` line 244; `hook_owned_state` line 305; `observe_runtime_launch` line 427; `observe_foreground_runtime` line 465; `attention_has_new_output` line 482; `note_output_and_sweep` line 496; tests `a_weaker_source_never_talks_over_a_live_stronger_one` and following); `crates/unpeel-serve/src/sessions.rs::derive_status_with_source` line 299 (foreground identity string `runtime_id:pid:pid_started_at`, `activity_signal` from `screen_changed_at`, the 100 ms viewport re-check before clearing Attention); `docs/agents/clients/session-activity.md` §Hook-driven sessions and §Background agents (Claude `background_tasks` is a Paneflow addition on top of this design).

**Acceptance Criteria:**
- [ ] Given `Start` or `UserPromptSubmit`, when applied, then state is `Busy`, `completed = false`, and `deadline = now + 5 min`; given `Stop`, then `Idle` with `completed = true`; given `StopFailure` or `Idle`, then `Idle` without completion; given `StopCancelled`, then `Idle` with `native_cancelled` set until the next opening event; given `PermissionRequest` (except `tool_name == AskUserQuestion`), then `Attention`.
- [ ] Given `SessionStart`, `SubagentStart`, `SubagentStop`, `Notification` (except `permission_prompt`, which deduplicates against a `PermissionRequest` received within 10 s) and unknown names, when applied, then the session latches as hook-owned and no state changes.
- [ ] Given a `Busy` session whose screen hash has not changed for 5 min, when the sweep runs, then state becomes `Idle` without completion and `hook-expiry.json` records the generation and the last transition time; given the screen hash changes, then the deadline is re-armed; given output grows but the hash is unchanged, then nothing is re-armed.
- [ ] Given an event whose `runtime_generation` is lower than the manifest's, when applied, then it is rejected; given an untagged `Stop` within 30 s of a relaunch whose replacement has not yet sent an opening event, then it is quarantined.
- [ ] Given a session whose launch command is not hook-capable, when the observed foreground identity `runtime_id:pid:pid_started_at` changes, then the latch and output baseline are reset while the runtime generation is kept; the first sighting after a worker start is recorded and is not an edge.
- [ ] Given a `Stop` whose payload carries a non-empty `background_tasks`, when applied, then the session stays `Busy` with `activity_source = hooks` until a `Stop` with an empty list, a later `Idle`, or the lease expiry.
- [ ] Given `Attention` and changed output, when `attention_clears_on_output` is true for the runtime, then the worker reads the live viewport with a 100 ms timeout and clears to `Busy` only if no menu is detected; a failed read keeps `Attention` and the previous output baseline.
- [ ] Given the ported Unpeel reducer tests (out-of-order frames, legacy generation guard, cancellation rearm, background lease), when `cargo test -p paneflow-serve` runs, then all pass.

#### US-013: Durable seed replay, expiry and cancellation sync
**Description:** As a multi-agent developer, I want the worker to rebuild exact state from disk after any restart so that sessions mid-turn while the app was closed show busy and finished ones stay idle.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012
**Unpeel reference:** `crates/unpeel-serve/src/activity.rs::seed_from_disk` line 570 (same-inode metadata and bytes, 64 KiB cap, `not_before`, lease anchored at `max(seed mtime, output.bin mtime)` only for openers, event ordering by seed mtime); `sync_cancellation_from_disk` line 374 and `observe_cancellation` (pending opener retained until the submission marker lands); `crates/unpeel-core/src/hook_assets/expiry.rs` (`hook_turn_expired` line 25, `record_hook_expiry` line 30: generation-scoped watermark under an exclusive lock, announced on the state bus); `crates/unpeel-core/src/hook_cancellation.rs::read_in` line 20; `docs/agents/clients/session-activity.md` §"The latch survives app restarts via a durable seed".

**Acceptance Criteria:**
- [ ] Given `last-hook-event.json` newer than the latest accepted live transition, when a rescan runs, then its event is applied with the file's mtime as the event time; given it is older, then it is ignored.
- [ ] Given a seed holding `UserPromptSubmit` (or `Start` when `anchor_start_event_to_output` is true), when replayed, then the lease is anchored at `max(seed mtime, output.bin mtime)`; given a seed holding `Stop`, then only the seed mtime is used and later terminal repaints cannot reopen the turn.
- [ ] Given `hook-expiry.json` for the current generation with a watermark at or after the seed's opening event, when replayed, then the session is `Idle` and no lease is created.
- [ ] Given a seed larger than 64 KiB or unparsable, when read, then it is ignored and a trace line is written; metadata and bytes are read from the same open file handle.
- [ ] Given `hook-cancellation.json` for the current generation, when replayed, then the cancellation fence is restored before the seed is applied, and a pending opener recorded after the submission time is applied afterwards.
- [ ] Given a worker restarted 6 min after a session went `Busy` with no seed update and no screen change, when the first sweep runs, then the session is `Idle`.

#### US-014: Notifications and completion rules
**Description:** As a multi-agent developer, I want a desktop notification only for a real completion or a real need for input so that I trust every notification.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012
**Unpeel reference:** `crates/unpeel-serve/src/notifications.rs` (worker decides when a Session needs attention or finished; `Done` body "Finished"; delivery through platform adapters, missing adapter is a no-op); `crates/unpeel-serve/src/activity.rs::is_completed` line 325 (hook-seen, Idle, completed, no expired background); `crates/unpeel-serve/src/driver.rs::derive_unread` line 2131 (settles while unobserved → unread); `crates/unpeel-core/src/activity_log.rs` (shared Recent/unread history); `docs/agents/clients/session-activity.md` ("Only a successful `Stop` counts as completed work"; menu edge and `PermissionRequest` deduplicate whichever arrives second; screen verdicts never send completion notifications; App `alert` never enters the reducer).

**Acceptance Criteria:**
- [ ] Given a hook-sourced `Busy → Idle` with `completed = true`, when the session's pane is not under the user's eye and the workspace is not muted, then one `Finished` notification carries the runtime label and `last_assistant_message` truncated to 200 characters; given the pane is visible, then none.
- [ ] Given a screen-sourced `working → idle` edge, when it happens, then no notification is sent and the row is not marked unread.
- [ ] Given `StopFailure`, lease expiry, `StopCancelled` or an Escape cancellation, when the session settles, then no `Finished` notification is sent and the activity log records the outcome (`failed:<matcher reason>`, `expired`, `cancelled`).
- [ ] Given `PermissionRequest` or a `menu_prompt_active` false→true edge, when either arrives, then exactly one `Needs input` notification is sent per generation, and the second signal within 10 s is deduplicated.
- [ ] Given a `Notification` hook with `notification_type = idle_prompt`, when received, then it does not notify (the `Stop` already did) and does not change state.
- [ ] Given the platform notifier fails (missing `notify-send`, Windows AUMID not registered), when a notification is due, then the failure is logged once per session and the attention queue still updates.

---

### EP-005: Host Viewport Scan

Add the host-side 500 ms viewport scan that stamps real screen changes, classifies the screen tier per runtime, detects agent-drawn menus, and observes the foreground runtime through wrappers on every platform.

**Definition of Done:** `screen_changed_at`, `screen_activity`, `menu_prompt_active` and `runtime.current_observation` are edge-written into manifests; an npm-installed Claude on Windows is recognized; Claude and Codex menus set attention without hooks.

**Unpeel reference:** `crates/unpeel-core/src/session_host.rs` (the 500 ms `menu_job` lines 5560-5660 and the runtime observer job around line 5395, `SESSION_MENU_SCAN_INTERVAL_MS` line 69); `crates/unpeel-core/src/screen_activity.rs`; `crates/unpeel-core/src/menu_prompt.rs`; `crates/unpeel-core/src/runtime_observer.rs`; `HostedSessionManifest` fields `menu_prompt_active`, `screen_activity`, `screen_changed_at`, `runtime.current_observation`.

#### US-015: Screen change stamp and declarative screen tier
**Description:** As a multi-agent developer, I want a recognized agent without an installed integration to show a lower-confidence busy state from its own screen so that the sidebar animates without inventing work from output growth.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009, US-003
**Unpeel reference:** `crates/unpeel-core/src/session_host.rs` (`ScreenChangeTracker` line 458 with the minimum spacing between `screen_changed_at` writes; the scan job line 5584: hash of `current_screen_text()`, classification only when the hash changed, `None` keeps the previous verdict, a runtime without rules clears it); `crates/unpeel-core/src/screen_activity.rs` (`BOTTOM_WINDOW_LINES` = 15, `classify` line 60, tests `claude_working_line_wins_over_the_always_visible_prompt`, `unknown_screens_leave_the_previous_verdict_alone`, `codex_prompt_and_working_shapes`); `crates/unpeel-serve/src/sessions.rs::screen_fallback_status` line 289; PTY case `crates/unpeel-cli/tests/cases/screen_fallback.py`; `docs/agents/clients/session-activity.md` §"Recognized agent sessions without a hook latch".

**Acceptance Criteria:**
- [ ] Given a hosted session, when the scan thread runs every 500 ms, then it hashes the visible viewport text and writes `screen_changed_at` only when the hash changes, coalesced to at most one manifest write per second.
- [ ] Given the observed runtime declares `[screen]` rules, when the screen changes, then the bottom 15 non-blank lines are classified: any `working` substring (case-insensitive) wins, else an `idle_prompt` prefix on a trimmed line gives idle, else the previous verdict stands; the verdict is edge-written to `screen_activity`.
- [ ] Given a runtime without `[screen]` rules, when the scan runs, then `screen_activity` is cleared and no classification runs.
- [ ] Given a session with a hook latch, when the worker derives status, then `screen_activity` is ignored; given no latch, then the verdict maps to `Busy`/`Idle` with `activity_source = screen`.
- [ ] Given a working marker 20 lines above the bottom, when classified, then it does not count (fixture from Unpeel's `screen_activity.rs` tests ported).
- [ ] Given an idle TUI repainting identical content at 30 Hz, when the scan runs for 10 s, then `screen_changed_at` does not advance and zero manifest writes occur.

#### US-016: Agent-drawn menu detector
**Description:** As a multi-agent developer, I want a session waiting on a numbered menu to show attention even though no hook fires so that I answer it instead of waiting on a spinner.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015
**Unpeel reference:** `crates/unpeel-core/src/menu_prompt.rs` (`NAV_MARKERS`, `SELECT_MARKERS`, `CONFIRM_MARKERS`, `CANCEL_MARKERS`, `AMEND_MARKERS`, `PASSIVE_MARKERS`, `PASSIVE_SELECTOR_PREFIXES`, `INTERACTIVE_QUALIFIERS`; `viewport_has_menu_prompt` with the adjacent-row window and whitespace collapse; `has_selected_choices` with the 12-line lookback; the regression tests in its `mod tests`); `runtimes/claude-code/fixtures/approval-menu.txt`; `clients/ios/UnpeelIOS/Sources/UnpeelIOS/RemoteGhosttyTerminalView.swift` (`menuPromptActive`, the Swift twin whose marker lists are kept aligned); PTY case `crates/unpeel-cli/tests/cases/claude_permission_redraw.py`; `docs/agents/clients/session-activity.md` §"Agent-drawn select menus".

**Acceptance Criteria:**
- [ ] Given Unpeel's `menu_prompt.rs` ported with its MIT notice, when the visible text has a navigation hint plus a select hint on the same or adjacent rows, or a confirm key next to a cancel key, or `esc to cancel` plus `tab to amend` beside a selected numbered list, then `viewport_has_menu_prompt` returns true.
- [ ] Given Claude's passive subagent footer `↑/↓ to select · Enter to view`, including its partially painted prefix, when scanned, then the result is false.
- [ ] Given the Claude approval fixture `runtimes/claude-code/fixtures/approval-menu.txt` and a Codex `Press enter to confirm or esc to cancel` fixture, when scanned, then both return true.
- [ ] Given a transcript that quotes `to navigate` in prose 30 lines above a shell prompt, when scanned, then the result is false.
- [ ] Given the detector flips, when the scan runs, then `menu_prompt_active` is edge-written and the worker overrides `Busy`/`Idle` to `Attention` while `menu_attention_detection` is enabled in settings (default on).
- [ ] Given a narrow viewport that wraps the footer across two rows, when scanned, then whitespace runs are collapsed and the detection still fires.

#### US-017: Wrapper-aware foreground observation
**Description:** As a multi-agent developer on Windows, I want an npm-installed Claude or Codex to be recognized under `node.exe` so that identity, tint and screen rules apply regardless of install method.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009, US-003
**Unpeel reference:** `crates/unpeel-core/src/runtime_observer.rs` (`MAX_OBSERVED_ARGV_ITEMS`, `MAX_OBSERVED_ARGV_BYTES`, `PID_START_TOLERANCE_MS` lines 19-21; `ActiveRuntimeObservation` line 29 with `PartialEq` as the edge-write key; `observe_foreground_runtime` line 262 with the double `pid_start_matches` check; `identify_runtime_in_job` line 305: leader first, then strength, depth, pid; `match_process` line 383; `runtime_from_wrapper_argv` line 447 and the `node`/`bun`/`python`/`sh -c`/`env`/`npx` branches; `runtime_id_from_script_path` line 522; `bounded_evidence_argv` line 605; `platform::foreground_job` per OS, to be extended with a Windows implementation that reads the PEB command line); `crates/unpeel-core/src/session_host.rs` observer job around line 5395 (fail closed when the PTY runtime no longer names the captured child).

**Acceptance Criteria:**
- [ ] Given the foreground process group of the PTY, when observed, then the group leader is matched first; otherwise the best match by `Direct` over `Wrapper` strength, then smallest ancestry depth, then lowest pid.
- [ ] Given `node`, `bun`, `python`, `sh -c`, `env`, `npx` wrappers, when their argv names a script path whose components match a catalog `script_path_signatures` entry, then the runtime is identified as `Wrapper` strength; ambiguous flags (`-e`, `-c` for node, `-m`/`-c` for python) yield no match.
- [ ] Given Windows, when observing, then the PEB command line already read in `src-app/src/workspace/ports.rs` feeds the argv matcher, and a `claude.cmd → cmd.exe → node.exe` chain running `@anthropic-ai\claude-code\cli.js` is recognized as `claude`.
- [ ] Given a match, when persisted, then `argv` is bounded to 8 cells and 1 KiB, stops at the cell that established the match, and is omitted for `Wrapper` matches.
- [ ] Given the session leader pid's start time no longer matches the recorded one (tolerance 10 s), when observing, then the observation is `None` and nothing is written.
- [ ] Given the observation is unchanged, when the scan runs, then no manifest write occurs; a change writes `runtime.current_observation` once.

---

### EP-006: Escape Cancellation and Background Agents

Cover the two lifecycle edges that hooks miss: the user interrupt on runtimes whose `Stop` does not fire, and child agents whose stops must not settle the parent.

**Definition of Done:** Escape in Claude, Gemini and Muse settles the pane within 1 s and the next prompt re-arms; a Claude subagent finishing never completes the main turn; both proven in the PTY matrix.

**Unpeel reference:** `docs/agents/clients/session-activity.md` §"Escape cancellation and hook delivery" and §"Background agents"; `crates/unpeel-core/src/hook_cancellation.rs`; `crates/unpeel-core/src/session_input.rs`; `crates/unpeel-core/src/hook_assets/background.rs`; `lifecycle.sh::record_subagent_activity`.

#### US-018: Escape cancellation fence
**Description:** As a multi-agent developer, I want Escape in a Claude, Gemini or Muse pane to settle the busy state immediately so that the sidebar reflects what I just did.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-013
**Unpeel reference:** `crates/unpeel-core/src/session_input.rs` (input parser tracking only successfully delivered user input across Write commands and StreamInput frames; 150 ms wait to distinguish a bare Escape from a fragmented sequence; bracketed paste, modified keys and Kitty escape release ignored; parser state surviving core handoff); `crates/unpeel-core/src/session_host.rs` `cancellation_job` line 5312 (100 ms timer, `escape_cancels_turn` read from the launch command's integration, `hook_input.take_pending`); `crates/unpeel-core/src/hook_cancellation.rs` (`Cancellation { runtime_generation, cancelled_at, submitted_at }`, locked write, state bus announce); `crates/unpeel-serve/src/activity.rs::observe_cancellation` and the `cancelled_at`/`submitted_at`/`pending_opener` branch of `apply_hook_event_unchecked`; PTY cases `crates/unpeel-cli/tests/cases/hook_cancellation.py`, `gemini_cancellation.py`, `muse_cancellation.py`, `grok_cancellation.py`, `ctrlc_fallback.py`; attach test `lone_escape_reaches_host_without_a_second_key_or_eof` in `crates/unpeel-attach/tests`; `Integration::with_escape_cancellation()` in `crates/unpeel-core/src/integrations/`.

**Acceptance Criteria:**
- [ ] Given a runtime with `escape_cancels_turn = true` launched from a preset, when the host sees a bare `ESC` byte delivered as user input with no continuation within 150 ms, then it writes `hook-cancellation.json` (`runtime_generation`, `cancelled_at`) under the session lock and announces it.
- [ ] Given bracketed paste, a modified key, a CSI or SS3 sequence starting with `ESC`, or an unmodified Kitty escape release, when parsed, then no cancellation is recorded; a Kitty `ESC` press is recognized.
- [ ] Given a cancellation, when the next Enter is delivered, then `submitted_at` is recorded in the same file; the paste recipe's second Enter does not replace the first.
- [ ] Given a cancellation with no later submission, when a `Stop`, `PermissionRequest` or `UserPromptSubmit` arrives, then the reducer keeps the session `Idle` without completion; given an opening hook after `submitted_at`, then the turn re-arms; a fast opener arriving before the Enter timer persists is retained and applied when the submission lands.
- [ ] Given a hand-typed agent in a blank pane, when Escape is pressed, then no fence applies (policy follows the launch binding).
- [ ] Given a Codex pane, when Escape is pressed, then no fence applies and the native `Interrupt` hook settles the state.
- [ ] Given the PTY matrix cases ported from Unpeel (`hook_cancellation`, `gemini_cancellation`, `muse_cancellation` with the deterministic substitute), when run on Linux and Windows, then all pass.

#### US-019: Background agent markers
**Description:** As a multi-agent developer, I want subagents tracked separately from the main turn so that a child finishing never marks the session done and a busy child keeps it busy.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012
**Unpeel reference:** `runtimes/claude-code/assets/hooks/lifecycle.sh::record_subagent_activity` (id validation `[A-Za-z0-9_-]`, 160-character cap, generation directory, atomic create on start and removal of only that file on stop, untagged launches stay metadata-only); `crates/unpeel-core/src/hook_assets/background.rs::read_background_hook_activity` line 19; `crates/unpeel-serve/src/activity.rs` (`BackgroundActivity` line 85, `sync_background_from_disk`, the background loop in `note_output_and_sweep`, `hook_owned_state` returning Busy while a child is unexpired and Attention taking precedence); PTY case `crates/unpeel-cli/tests/cases/background_hooks.py`; `docs/agents/clients/session-activity.md` §Background agents (directory mtime advancing the shared recency clock; a new generation never inherits children).

**Acceptance Criteria:**
- [ ] Given a Claude `SubagentStart` with `agent_id`, when the reporter handles it, then it atomically creates `<session dir>/background-hooks/<generation>/<agent_id>.json` before broadcasting; given `SubagentStop`, then it removes only that file.
- [ ] Given an `agent_id` longer than 160 characters or containing characters outside `[A-Za-z0-9_-]`, when received, then no file is created and the event is still forwarded as latch-only.
- [ ] Given at least one unexpired marker, when the main `Stop` arrives, then the session stays `Busy`; given the last marker is removed, then the session settles to the main turn's outcome and the directory mtime advances the shared recency clock.
- [ ] Given a marker older than 5 min with no screen change, when the sweep runs, then it is marked expired, the session settles, and no completion notification is sent.
- [ ] Given a new runtime generation, when markers exist under the previous generation directory, then they are ignored.
- [ ] Given `Attention` on the main turn, when children are busy, then `Attention` is reported.

---

### EP-007: Remote Controller Proof

Prove that the worker protocol is consumable by a second Controller so that the phone and remote-machine milestone starts from a verified contract.

**Definition of Done:** The `paneflow` CLI, running as a separate process, renders the same session list and states as the app over the worker protocol, with capabilities read from bootstrap.

**Unpeel reference:** `AGENTS.md` §"Hosts and Controllers, one protocol"; `protocol/host-capabilities-v1.json`; `crates/unpeel-core/src/controller_protocol.rs` and `controller_api.rs`; `docs/agents/clients/remote-control.md`; `clients/shared/UnpeelShared` (the Host protocol client shared by Mac and iOS).

#### US-020: CLI Controller over the worker protocol
**Description:** As a remote Controller developer, I want `paneflow sessions --follow` to consume the worker's bootstrap and event stream so that the protocol is proven before any mobile work.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011
**Unpeel reference:** `crates/unpeel-core/src/controller_protocol.rs` (`host.bootstrap` line 26; native and headless Host descriptors advertised in bootstrap lines 138-147); `protocol/host-capabilities-v1.json` (capabilities are advertised, never guessed from Host kind or a 404 probe); `protocol/host-conformance-v1.json` (native and headless Hosts run the same cases); `docs/agents/cli.md` (the `unpeel` CLI as a peer client of the same server); `crates/unpeel-cli/tests/pty_suites.rs` and cases `mobile.py`, `compat_serve.py`, `compat_bridge.py`, `host_launch_conformance.py`; `docs/agents/session-model.md` §"Recent ordering" (`updatedAtUnixMs` published so Controllers never reconstruct recency from local files).

**Acceptance Criteria:**
- [ ] Given a running worker, when `paneflow sessions --follow --json` runs, then it prints the bootstrap with `capabilities`, then one JSON line per `agent.event`, with `status`, `activity`, `activity_source`, `runtime_id`, `unread` matching the app's sidebar for the same session within 500 ms.
- [ ] Given a worker that does not advertise `session.runtime.resume`, when the CLI is asked to resume, then it refuses with `capability not advertised` and never probes.
- [ ] Given the worker restarts during `--follow`, when it comes back, then the CLI reconnects within 3 s and resumes from a fresh bootstrap without duplicate rows.
- [ ] Given an unauthenticated connection attempt from another user account on the same machine, when it connects, then the socket permissions reject it and the CLI reports `permission denied`.
- [ ] Given `protocol/controller-conformance-v1.json`, when `cargo test -p paneflow-serve` runs, then the CLI Controller passes every listed case.

## Functional Requirements

- FR-01: The system must derive busy, idle and attention from hook events only once a session is hook-latched; output growth must never start a busy state.
- FR-02: The system must bound any hook-owned busy state to 5 minutes without a screen-hash change, and persist the expiry so a restart cannot revive it.
- FR-03: The system must reject hook events whose runtime generation is older than the session's current generation.
- FR-04: The system must install provider hooks only on explicit user action, into the provider's global configuration, preserving foreign entries byte for byte, idempotently, under an exclusive file lock.
- FR-05: The system must keep hook reporters inert when `PANEFLOW_SESSION_ID` is absent.
- FR-06: The system must persist the last lifecycle event per session on disk, written by the worker on receipt and by the reporter when the host is unreachable, in one shared file shape.
- FR-07: The system must NOT wrap executables, append flags, mint conversation ids, or edit provider configuration at launch or upon observing a hand-typed agent.
- FR-08: The system must publish `activity_source` (`hooks`, `screen`, `none`) with every running session and must NOT send a completion notification for a screen-sourced edge.
- FR-09: The system must detect agent-drawn menus from the rendered viewport and report attention while `menu_attention_detection` is enabled.
- FR-10: The system must recognize a runtime executed through `node`, `bun`, `python`, `sh -c`, `env` or `npx` by script path signature on Linux, macOS and Windows.
- FR-11: The system must record Escape as cancellation intent only for runtimes declaring `escape_cancels_turn`, only for preset launches, and must never send a signal to the process.
- FR-12: The system must keep the PTY core running across worker restarts and app updates; no terminal receives a signal because of a worker or app lifecycle event.
- FR-13: The system must advertise Controller capabilities at bootstrap; Controllers must NOT infer capabilities from probes or host kind.
- FR-14: The system must verify a live pid against its recorded kernel start time before sending any signal.
- FR-15: The system must fail a build when a runtime descriptor violates the schema.
- FR-16: The system must NOT read undocumented provider state files (Claude `~/.claude/sessions`, Codex rollout directory scans) as a lifecycle source.

## Non-Functional Requirements

- **Performance:** Hook reporter wall time under 50 ms when inert, under 500 ms with a reachable host, under 800 ms with a stalled endpoint. Viewport scan tick under 2 ms per session at 200x60 on the release build. Worker state rebuild after restart under 2 s for 50 sessions. Sidebar state within 500 ms of the hook event. Zero manifest writes per second in steady state.
- **Security:** Unix socket mode 0600; Windows named pipe ACL restricted to the current user; hook frames over 64 KiB or for unknown session ids refused; hook payload persisted only as `hook_event_name`, `tool_name`, `runtime_generation` (no prompt text); installers refuse symlinked or unparsable config files; no credentials or argv beyond the matching cell persisted.
- **Accessibility:** Settings ▸ Agents fully keyboard-navigable; every state has a text label next to its color (spinner, dot, badge).
- **Scalability:** 200 sessions per home with the scan thread total CPU under 3% of one core on the release build; 16 Controllers subscribed to one worker without frame loss (subscriber queue 256 slots, slow subscriber dropped, never blocking).
- **Reliability:** Zero orphan provider config entries after `kill -9` in the PTY matrix; worker crash recovery with no state divergence in 100 consecutive restarts of the matrix; every kill path verifies pid and start time; hook delivery finishes before the reporter returns (no fire-and-forget without the seed written).
- **Binary size:** `paneflow-shim` under 512 KiB, `paneflow-ai-hook` under 375 KB, `paneflow-mcp` under 512 KiB, combined cap unchanged in `src-app/build.rs`.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Hook fires with no listener | Worker down, host down | Reporter writes the seed and exits 0 within 800 ms | none |
| 2 | Lost Stop | Hook timeout or disabled | Lease expires after 5 min of unchanged screen; state Idle, no notification | none, activity log `expired` |
| 3 | Stale event after relaunch | Old process posts Stop after Resume Agent | Rejected by generation, or quarantined 30 s if untagged | none, trace line |
| 4 | Escape mid-turn (Claude) | Bare ESC delivered | Fence written, state Idle, re-armed on next Enter plus opener | none |
| 5 | Escape on Codex | Bare ESC delivered | No fence; native `Interrupt` settles | none |
| 6 | Menu with no hook | Numbered choice drawn | `menu_prompt_active` true, Attention, one notification | "Needs input" |
| 7 | Passive subagent footer | `↑/↓ to select · Enter to view` | No attention | none |
| 8 | Codex hook untrusted | Fresh install, `/hooks` not visited | Session stays neutral or screen-sourced; Settings row shows the trust step | "Trust the Paneflow hooks once in Codex: /hooks" |
| 9 | Managed settings block hooks | `disableAllHooks` or `allowManagedHooksOnly` | Screen tier carries state with `activity_source = screen` | none |
| 10 | Provider config symlinked or invalid | Installer run | Refuse, no change | "Refused: <path> is a symlink / not valid JSON" |
| 11 | Concurrent installs | Two instances | Serialized by lock, identical result | none |
| 12 | npm Claude on Windows | `node.exe` foreground | Recognized through script path signature | none |
| 13 | Pid recycled | Manifest pid points at another process | Never signaled; session cleaned without kill | none |
| 14 | Worker cannot bind | Port or pipe in use | App shows error and Retry, no dead panes | "Worker failed to start: <reason>" |
| 15 | Seed unparsable or oversized | Corrupt file | Ignored, trace line | none |
| 16 | Subagent id malformed | `agent_id` invalid | No marker, event latch-only | none |
| 17 | Descriptor invalid | Wrong field in `runtime.toml` | Build fails naming file and field | compiler error |
| 18 | Unknown `SessionEnd.reason` | New provider value | Treated as `other` | none |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Codex trust gate makes hooks silently inert after install | High | High | Byte-stable definition, explicit post-install step in CLI and Settings, screen tier as fallback, US-005 asserts file identity across upgrades |
| 2 | `AskUserQuestion` routes differently than assumed | Medium | Medium | US-004 records the real events in the PTY regression and adjusts the latch-only rule |
| 3 | Core/worker split regresses terminal reattach | Medium | High | Conformance cases in `protocol/host-conformance-v1.json`, reattach keyed on pid plus start time, worker restart matrix of 100 cycles |
| 4 | Windows exec-form stdin behaves differently | Low | High | US-004 real Claude and Codex PTY run on Windows before EP-002 closes |
| 5 | Screen rules drift with provider TUI updates | High | Low | Fixtures per runtime, lower-confidence source that never notifies, hooks always win |
| 6 | Story count and scope (20 stories, 7 epics) | High | Medium | Epics ordered by dependency; EP-001 and EP-002 ship value alone; EP-007 is a proof, not the mobile client |
| 7 | MIT attribution omitted on ported code | Low | Medium | `THIRD_PARTY_NOTICES` entry in EP-001, checked by the notices CI job |

## Non-Goals

- No HTTP port registry or multi-listener broadcast; the host socket is the single ingress.
- No PowerShell hook scripts; Windows uses the hook binary in exec form.
- No Codex `notify` reporter and no `features.hooks` write; native hooks are the source.
- No reading of Claude's session registry or Codex rollout directories; removed in US-007.
- No Escape inference for Codex, Copilot, Kiro, Cursor, Amp, OpenCode, Cline, Kimi, Grok; native events or nothing.
- No mobile or remote-machine client in this PRD; EP-007 proves the protocol with the CLI only. The mobile client is the next PRD.
- No transcript viewer, resume rewriting, or archive semantics changes beyond what hook-captured ids already provide; Resume Agent parity with Unpeel is a later PRD.
- No downloadable third-party runtime packages; the catalog is a source contribution boundary.

## Files NOT to Modify

- `native/libghostty/**` and `crates/paneflow-libghostty-sys/**` - engine pin and generated bindings; a second engine or a changed archive breaks every target.
- `src-app/src/terminal/types.rs` - the neutral grid vocabulary and the `alacritty_is_absent_from_the_app_crate` guard.
- `rust-toolchain.toml` - pinned 1.98.0; rustfmt output drifts across releases.
- `.github/workflows/release.yml` and `.github/actions/fetch-libghostty/**` - release chain; only `run_tests.yml` may gain a job for the new conformance cases.
- `src-app/build.rs` size caps - may be read, never raised.
- `crates/paneflow-terminal-ghostty/**` - render path; the viewport scan reads `current_screen_text()` through the existing host API only.

## Technical Considerations

- **Architecture:** Should the worker be a new crate `paneflow-serve` or a mode of `paneflow-host`? Recommended: new crate with a `serve` subcommand on the existing `paneflow` CLI, so the PTY core binary stays frozen and small. Engineering to confirm the packaging impact on the MSI, `.deb`, `.rpm`, AppImage and `.app` bundles.
- **Data Model:** Manifest additions (`screen_changed_at`, `screen_activity`, `menu_prompt_active`, `runtime.current_observation`, `runtime_launch_generation`) as additive optional fields, or a manifest version bump? Recommended: additive with `#[serde(default)]`, so old manifests decode. Trade-off: no way to distinguish "never scanned" from "no verdict" without an explicit `Option`.
- **API Design:** One `agent.snapshot` plus `agent.event` stream, or a per-session subscription? Recommended: workspace-wide stream with bounded subscriber queues (256 slots, drop slow subscribers) as today. Capability advertisement as a JSON document mirrored in `protocol/host-capabilities-v1.json`.
- **Dependencies:** `toml` as a build dependency of `paneflow-agent-config` only; `same_file` already present for the shim. No new runtime dependency in the capped helpers. Engineering to confirm `cargo deny` passes.
- **Migration:** Adoption of `_paneflow_managed` per-launch entries into global integrations runs once at worker start and is idempotent. Backward compatibility with manifests from 0.16.0: yes. Rollback: removing the integration restores the provider file to its pre-install content minus Paneflow entries; there is no automatic rollback of the worker split, so EP-003 ships behind a feature flag until EP-004 lands.
- **Ported code:** Which Unpeel modules are copied verbatim (`menu_prompt.rs`, `screen_activity.rs`, the reducer tests) versus rewritten? Recommended: copy with the MIT notice where the logic is provider-neutral, rewrite where it touches Paneflow's socket or manifest types.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Sessions busy longer than 5 min without a hook or screen change | unbounded (no lease) | 0 | Month-1 | PTY matrix assertion plus opt-in telemetry counter `agent.busy_expired` |
| Escape-to-settled latency on Claude | 20 s (source takeover) | < 1 s | Month-1 | PTY regression timing |
| Files edited to add a runtime | 5 | 1 directory | Month-1 | Antigravity re-add PR diff |
| Orphan hook entries after crash | present (lease sweep) | 0 | Month-1 | PTY matrix `kill -9` case |
| Windows sessions with runtime identity while an agent runs | unknown (image name only) | 100% of npm and native installs in the matrix | Month-1 | Windows native test run |
| Completion notifications that were not a real `Stop` | present (`SubagentStop`, OSC idle) | 0 | Month-1 | Activity log audit in the matrix |
| Controllers consuming the worker protocol | 1 (app, no protocol) | 2 (app and CLI) | Month-6 | `protocol/controller-conformance-v1.json` passing |

## Open Questions

- Which hook events fire for `AskUserQuestion` with which `tool_name`? Answered by the US-004 regression before EP-004 starts; changes the latch-only rule only.
- Does the exact set of fields hashed by Codex's trust gate include `timeout`? Answered by the US-005 upgrade test; changes only whether `timeout` may ever be edited after install.
- Should EP-003 ship behind a feature flag in a minor release before EP-004, or should both land in one release? Arthur decides at EP-002 close; affects the 0.17 versus 0.18 boundary.
- Which of Gemini, Copilot and Amp receive Windows-native hook conformance runs in CI versus manual verification on hardware? Arthur decides at EP-001 close; affects the `platforms` list in US-003.
[/PRD]
