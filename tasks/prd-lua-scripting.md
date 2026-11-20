[PRD]
# PRD: A typed API, Lua scripting, and a layout that lives outside the window

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-19 | Arthur Jean | Initial PRD, from the exploration of 2026-11-19 (Rex documentation, eleven code maps of Paneflow, an mlua probe) and a second research pass the same day (routing, layout mutations, worker and approval broker, palette and events, mlua and schemars, agent client timeouts). One typed registry, a plain-data layout model, a gap-free event log, Lua scripts and `init.lua`, prompts, the layout authority in the worker, headless layout, agent prompts and actions, Windows validation. 10 epics, 75 stories. |

## Problem Statement

Paneflow can be driven from outside (a JSON-RPC socket, a CLI, an MCP bridge), but it cannot be programmed. A user who wants "split, start the tests, zoom the pane if they fail" writes a shell loop around `paneflow wait`, which polls every 250 or 500 ms (`src-app/src/cli/wait_cmd.rs:18-20`). There is no user-defined action, no way to ask the user a question from an automation, and no event for a pane being created or a program exiting (`src-app/src/ipc_events.rs:14-23` lists only `ai.*` and `surface_changed`). Rex (Superlogical), a terminal built on the same libghostty engine, documents all of this behind a Lua API and announced its prompt functions on 2026-11-18. The first reply under Mitchell Hashimoto's post about them (read on x.com on 2026-11-19) asked for exactly what Paneflow is positioned for: letting a coding agent raise its question in the terminal's own UI.

Three structural facts block the same feature set in Paneflow today:

1. **Every method exists in four hand-maintained copies.** The dispatch is a `match` on strings (`src-app/src/app/ipc_handler/mod.rs:172-195`), the advertised list is a hand-written array of 23 names (`src-app/src/app/ipc_handler/gates.rs:92-123`), parameters are pulled one by one out of untyped JSON (`src-app/src/app/ipc_handler/params.rs:7-12`), and the CLI and the user documentation repeat the list. A Lua API on top would be a fifth copy. No app-side test pins the wire format (the host and the worker each have a conformance case file under `protocol/`, the app has none).
2. **The layout belongs to the window.** The live tree holds GPUI entities (`src-app/src/layout/tree.rs:22-35`), a tab's root is assigned directly in 17 production sites across 6 files plus 3 more through templates and a `&mut` reference, focus is the GPUI focus handle, and `save_session` is called from 68 sites in 25 files (`src-app/src/app/session.rs:103-120`). With no window, every layout method is refused with "no Paneflow window is attached" (`crates/paneflow-host/src/control.rs:532-535`). The ids clients receive are GPUI entity ids (`src-app/src/app/ipc_handler/surface_methods.rs:84,102`), so `PANEFLOW_SURFACE_ID`, exported into every pane at spawn (`src-app/src/terminal/pty_session/spawn_env.rs:217`), goes stale when the window restarts.
3. **Nothing can wait for a human.** A request is abandoned after 30 seconds (`src-app/src/ipc.rs:86-88`), the app accepts 16 request connections (`src-app/src/ipc.rs:70-72`), and the one existing human-in-the-loop flow, the write approval, never waits: `write_pane` returns `approval_pending` at once and the agent must call again (`crates/paneflow-host/src/agent_write.rs:552-572`).

**Why now:** Rex's documentation now describes a complete scripting layer and a prompt protocol, which fixes a reference design and raises what users of a libghostty terminal expect. Paneflow already has the pieces Rex needed a single server for, split across two background processes: a host that owns terminals and a worker that is replaced on every update without touching them (`crates/paneflow-serve/src/bootstrap.rs:113-122`). That split lets Paneflow update its layout logic and its Lua runtime without ever stopping a terminal, which Rex's single server cannot do. Paneflow also already receives the two agent hooks that matter (`PermissionRequest` and `PreToolUse` on `AskUserQuestion`, `crates/paneflow-mcp-install/src/integrations.rs:1077-1104`), so agent-raised prompts are within reach.

## Overview

The work rewrites the control surface of Paneflow in the shape of Rex, natively in Rust, and embeds a Lua 5.4 engine. The visible application does not change: every existing shortcut, menu, drag gesture, CLI verb, JSON-RPC method and MCP tool behaves as it does today. New surfaces appear only when a script, an action or an agent asks for one.

The design rests on six decisions, each argued in [Technical Decisions](#technical-decisions):

1. **One registry, declared once.** A new GPUI-free crate, `paneflow-api`, declares each method once: name, summary, typed parameters, typed result, examples, owner process, gate. Dispatch, validation, `system.capabilities`, `system.describe`, the `paneflow api` CLI, the Lua bindings, a checked-in schema snapshot and editor stubs all derive from it. The 23 existing methods migrate without a wire change, pinned first by an app conformance file.
2. **The layout is plain data.** A new GPUI-free crate, `paneflow-layout`, holds workspaces, tabs, split trees, panes and surfaces as data with stable `u64` ids, and changes only through `apply(op)`. The GPUI tree becomes a projection of it. It lives in the window first (EP-002), then in the worker (EP-007), then works with no window at all (EP-008).
3. **New subsystems live in the worker from day one.** The event log, the `init.lua` runtime, the action table and the prompt broker are built in the worker, their final home. The host protocol does not change at any point, because an incompatible host is left running until the user agrees to stop every terminal (`ARCHITECTURE.md:863`, `crates/paneflow-host/src/protocol.rs:135-146`).
4. **Clients route, processes do not forward.** Each method has one owner (window, worker or host). A shared client picks the endpoint from the descriptor. No process relays a request for another, which keeps the worker's rule "forwards nothing to the core" (`ARCHITECTURE.md:692-696`) while amending "no front door" to "answers the method families it owns".
5. **Lua 5.4 through mlua 0.12.2.** Scripts run in the CLI process (`paneflow do`), the configuration runs in the worker. A probe showed that a suspended call works under `pcall` and inside a metamethod on Lua 5.4, both refused on the Lua 5.1 that Rex documents.
6. **Agents are first-class askers.** The prompt broker serves an MCP tool, the agent hooks, and Lua actions exposed as agent tools, behind visible provenance, a rate limit and an allow-list.

The construction order is: registry, layout model in the window, events, scripts, `init.lua` and actions, prompts, layout authority in the worker, headless, agents, then one Windows hardware epic.

## Goals

| Goal | Baseline (2026-11-19) | Target | How measured |
|------|----------------------|--------|--------------|
| Hand-maintained copies of the method list | 4 (dispatch match, capabilities array, CLI wiring, docs) | 1 declaration per method | No string `match` on method names remains in `src-app/src/app/ipc_handler/mod.rs`; `protocol/api-v1.json` drift test |
| App methods with a machine-readable schema | 0 of 23 | 100 % of registered methods | `system.describe` returns input and output schemas for every method |
| Direct assignments to a tab's `root` or `saved_layout` in production code | 20 | 0 | Guard test over `src-app/src` |
| `save_session` call sites in the window | 68 in 25 files | 1, the fallback path used when no worker runs | Guard test |
| Layout methods that answer with no window | 0 | every structural method | Conformance cases run against a worker with no window |
| Ids that survive a window restart | workspace id, session id | workspace, tab, pane and surface ids | Restart test on a saved layout |
| Lifecycle and terminal event types | 0 (only `ai.*` and `surface_changed`) | the types listed in FR-07 | Event conformance cases |
| User-defined actions runnable from key, palette, CLI and agent | 0 | all four entry points | Story criteria of EP-005 and EP-009 |
| Host protocol version bumps caused by this work | n/a | 0 | `HOST_PROTOCOL_VERSION` unchanged at the end of every epic |

## Target Users

- **The user who automates their terminal.** Today: shell scripts around `paneflow ls`, `read`, `send`, `wait`, and TOML flow files (`docs/user/scripting.md`). Pain: polling, no events, no question to the user, no reuse from a key or the palette.
- **The user running several coding agents.** Today: watches the attention queue (`src-app/src/app/attention.rs:94-116`) and answers each agent in its own pane. Pain: a question asked by an agent in a pane that is not visible waits until the user finds it.
- **A coding agent in a pane.** Today: four MCP tools (`crates/paneflow-mcp/src/tools.rs:33-92`) and the CLI through a skill. Pain: it cannot ask the user anything outside its own TUI, and it cannot trigger a workflow the user has prepared.
- **The maintainer.** Today: adds a method in four places and tests layout through a GPUI context with a fake terminal (`src-app/src/layout/mutations.rs:156-159`). Pain: drift between copies, layout logic that cannot be tested as data.

## Research Findings

Rex facts come from the documentation dump at <https://www.superlogical.com/rex/docs/llms-full.txt>, read on 2026-11-19 (10,924 lines). `L.n` is a line number in that dump, which is local evidence and not tracked in this repository. Page names refer to the same site.

| # | Finding | Source |
|---|---------|--------|
| R1 | Rex reads `init.lua` in the server; a script started with `rex do` runs in the CLI process. | L.65-66, L.1590 |
| R2 | Every server method carries a name, a description, an input schema, an output schema and examples; the CLI and the Lua API derive from them. | L.2612-2614, page "Server API" |
| R3 | Rex runs Lua 5.1 with the full standard library except `io.popen` and `os.execute`. | L.10124-10127 |
| R4 | A configuration action has 30 seconds to finish; time spent in a prompt does not count. `rex.invoke` nests up to 16 deep. | L.884-887, page "Rex Lua API" |
| R5 | Prompts: every capable app receives `client_prompt_requested`, the first `client.reply` wins, the others receive `client_prompt_ended`. Default timeout 300 s, at most 3,600 s. At most 2,000 items. Reasons for no answer: `cancelled`, `timeout`, `no_client`, `unsupported`, `busy`, `failed`. | L.1154-1157, L.7868-7870, page "Custom UI" |
| R6 | Rex refuses a prompt under `pcall`, in an event handler, in an action run by `rex.invoke`, in a function bound to a key and in a `table.sort` comparator. | L.1115-1128, page "Custom UI" |
| R7 | Events carry a gap-free `seq`; a connection more than 8 MiB behind is closed rather than skipping events. | L.8285-8287 |
| R8 | Handlers run one at a time; a script has at most 1,024 timers. `rex.on` does nothing in the configuration file. | L.1951, L.2058, page "Lua Events", page "Lua Scripts" |
| R9 | View synchronization sends no diff: each change increments a revision and the client refetches the whole view. An id "never changes and is never reused". | L.9015-9017, L.1202-1203 |
| R10 | No file watching: the user runs `rex config reload`. | L.57-59 |
| R11 | Agents use the CLI and a skill; changing the app from the CLI needs Remote Control turned on. | L.1363-1366, L.1381 |
| R12 | mlua 0.12.2 is MIT, needs Rust 1.88, and its `vendored` feature builds Lua 5.4.9 from `lua-src` 551.0.2 (MIT). `serialize` is an alias of `serde`. `Lua::sandbox` exists only on Luau. `set_memory_limit` and instruction hooks exist on Lua 5.4. | Crate sources in the local cargo registry (`mlua-0.12.2/Cargo.toml`, `lua-src-551.0.2/lua-5.4.9/lua.h`); Context7 `/mlua-rs/mlua` |
| R13 | Probe, an async Rust function of 200 ms called from Lua through mlua 0.12.2: direct and nested calls work on Lua 5.1, 5.4, 5.5 and Luau; under `pcall` it works on 5.4, 5.5 and Luau and fails on 5.1; inside `__index` it works on 5.4 and 5.5 only; inside a `table.sort` comparator it fails everywhere with "attempt to yield across a C-call boundary"; inside a coroutine the script created, no error is raised and the script receives an internal mlua value; two pending calls and a computation finish in 200 ms total. | Throwaway probe run on 2026-11-19 outside the repository (local evidence) |
| R14 | `schemars` 1.2.2 is already in `Cargo.lock` (pulled by GPUI), no workspace type derives a schema, and it emits JSON Schema 2020-12 by default. | `Cargo.lock:6137,6151`; Context7 `/gresau/schemars` |
| R15 | Claude Code: a `PreToolUse` hook on `AskUserQuestion` answers by returning `permissionDecision: "allow"` with `updatedInput` holding the original `questions` and an `answers` object; a `PermissionRequest` hook returns `decision.behavior` allow or deny; command hooks default to a 600 s timeout. | <https://code.claude.com/docs/en/hooks> (sections "Tools that require user interaction", "PermissionRequest decision control", common fields), read 2026-11-19 |
| R16 | Claude Code: a per-server `"timeout"` in the MCP configuration is a hard wall-clock limit per tool call. The Paneflow installer writes `30000`. | <https://code.claude.com/docs/en/mcp>, read 2026-11-19; `crates/paneflow-mcp-install/src/agents/claude_code.rs:174` |
| R17 | Codex CLI: MCP tool calls time out after 60 s by default (`mcp_servers.<id>.tool_timeout_sec`). A `PermissionRequest` hook can allow, deny or decline to decide. No hook answers a question. Hooks default to a 600 s timeout. | <https://learn.chatgpt.com/docs/config-file/config-reference>, <https://learn.chatgpt.com/docs/hooks>, read 2026-11-19 |

Paneflow facts, each read in the code on 2026-11-19 at `31d8ce34`:

| # | Finding | Source |
|---|---------|--------|
| P1 | Three endpoints exist: the window's socket, the host's, the worker's. A client picks the window's socket when it listens and the host otherwise; the host never forwards to the window. | `crates/paneflow-home/src/lib.rs:178-287`, `crates/paneflow-ipc-client/src/host_control.rs:100-114` |
| P2 | The worker is the `paneflow` binary started as `serve run`. It is replaced when its version, protocol or build id differs, with 5 s to drain and 10 s to start. It uses OS threads and no async runtime. It holds `serve/owner.lock` while it runs. It writes `serve/instance.json` and the usage ledger, nothing about layout. | `crates/paneflow-serve/src/bootstrap.rs:16-17,113-122,263-272`, `crates/paneflow-serve/Cargo.toml`, `crates/paneflow-serve/src/worker.rs:103-111,149-180` |
| P3 | The worker follows the host through `agent.follow` and holds, per session, lifecycle (including exit), title, working directory and `bell_at_ms`. | `crates/paneflow-serve/src/core_link.rs:45-56,99-113`, `crates/paneflow-host/src/agent.rs:168-198` |
| P4 | The window already follows a stream on the worker endpoint (`agent.follow`), on every platform. The window's own socket closes the connection after each reply on Windows and its event stream is refused on a named pipe. | `src-app/src/app/host_agents.rs:535-562`, `src-app/src/ipc.rs:949-950`, `crates/paneflow-ipc-client/src/lib.rs:566-576` |
| P5 | The write-approval broker keeps pending requests in memory with a 120 s lifetime and a rate limit of one per second with a burst of three; the window follows `approval.follow` and shows a banner. | `crates/paneflow-host/src/agent_write.rs:22-28,125-132`, `src-app/src/app/write_approvals.rs:74-116`, `src-app/src/terminal/view/conversation.rs:97-109` |
| P6 | The hook helper sends `agent.event` to the host and nothing else: it never writes to stdout and always exits 0. Its hooks are installed with a 5 s timeout. Hard size caps: `paneflow-ai-hook` 384,000 bytes, `paneflow-mcp` 524,288 bytes. | `crates/paneflow-ai-hook/src/main.rs:5`, `crates/paneflow-ai-hook/src/transport.rs:9-41`, `crates/paneflow-mcp-install/src/integrations.rs:1077-1104`, `scripts/check-helper-sizes.sh:8-12` |
| P7 | The palette reads a static table; a dynamic entry needs a new `Source` and a new `Apply` variant. `quick_pick.rs` holds only styling helpers; a one-line `TextInput` and a modal one-field prompt (the branch prompt) exist. Toasts are the only general notice. An invalid `paneflow.json` is only logged. | `src-app/src/app/command_palette/mod.rs:55-60,472-518`, `src-app/src/app/command_palette/catalog.rs:156,781`, `src-app/src/ui_primitives/quick_pick.rs:13-103`, `src-app/src/app/branch_prompt.rs:9-78`, `src-app/src/app/notifications.rs:140-238`, `crates/paneflow-config/src/loader.rs:109-124` |
| P8 | Key bindings are rebuilt by `apply_keybindings` on config reload, on a saved shortcut and on a keyboard layout change. Settings writes shortcuts into `paneflow.json`. | `src-app/src/keybindings/apply.rs:73-128`, `src-app/src/app/settings.rs:127-153,548-568`, `src-app/src/app/config_writer.rs:100-149` |
| P9 | Undo close keeps at most 5 records and holds a closed terminal's session for 5 s before stopping it. A detached pane stays a leaf of its tab with a `detached` field; its saved location is positional. | `src-app/src/app/constants.rs:301-303`, `src-app/src/app/hosted_sessions.rs:222-262`, `src-app/src/app/detached_panes/window.rs:233-238`, `crates/paneflow-config/src/schema/session.rs:63-79` |
| P10 | A plain-data mirror of the tree already exists for persistence, and the window can already conform to a document by reusing live panes. | `crates/paneflow-config/src/schema/layout.rs:125-139`, `src-app/src/layout/serde.rs:97-106`, `src-app/src/app/ipc_handler/workspace_methods.rs:747-791` |
| P11 | Session launch (pane environment, host link) is built in the window crate. | `src-app/src/terminal/pty_session/spawn_env.rs`, `src-app/src/terminal/host_link.rs` |
| P12 | The repository withdrew a second git engine in November 2026 because a second implementation trails the reference. | `AGENTS.md`, "One git implementation: the `git` CLI" |

## Assumptions & Constraints

### Assumptions (to validate)

- **A1.** mlua 0.12.2 with vendored Lua 5.4.9 compiles on the four shipping targets, MSVC included, and passes `cargo deny`. The probe ran on Linux x86_64 only. Validated by US-026.
- **A2.** The host's `agent.follow` stream covers plain shells as well as agent sessions, so the worker can derive terminal events for every pane. Read from the snapshot type, not tested. Validated by US-023.
- **A3.** A local-socket round trip to the worker is short enough that a client-predicted layout op never shows a correction to the user. Not measured. Validated by US-050, which records the numbers; the design does not depend on the result because every op is applied locally first (D7).
- **A4.** A `PreToolUse` hook that answers `AskUserQuestion` works in an interactive Claude Code session, not only in the non-interactive mode whose documentation section describes it. Validated by US-064 before any hook work.
- **A5.** Streams on the worker's named pipe behave on Windows as they do on Linux. Read on Linux only. Validated by US-071.
- **A6.** Session launch can move out of the window crate without behavior change. `spawn_env.rs`, `host_link.rs` and `agent_launcher.rs` were located, not read in depth. Validated by US-058.
- **A7.** Design constants chosen for this version, each a named constant with a test, none a performance target: the event ring keeps 1,024 events (the bound already used by `src-app/src/ipc_events.rs:12` and `crates/paneflow-serve/src/core_link.rs`); a follower more than 8 MiB behind is disconnected (R7); at most 16 prompts wait at once; an id lease holds 1,024 ids; `ask_human` waits at most 20 s per call (below the 30 s of R16 and the 60 s of R17); the worker's Lua state is capped at 256 MiB; the window keeps at most 1,024 unsent layout events; a permission prompt shows at most 400 characters of the tool's input.
- **A8.** The operation inventory of 2026-11-19 is complete for user-triggered paths and incomplete for IPC split, `workspace.close`, workspace creation with the picker and templates. Completed by US-007.
- **A9.** `serve/owner.lock` (`OwnerLock`, `crates/paneflow-serve/src/worker.rs:103-111`, test at `:656`) is seen as free as soon as its worker dies, with or without a clean shutdown. The lock's behavior after a crash was not read. Validated by US-051.
- **A10.** GPUI accepts, at runtime, a key binding to an action that carries data (an action name and its arguments), through the `cx.bind_keys` call that `apply_keybindings` already makes. Not tried. Validated by the first criterion of US-041.
- **A11.** `paneflow-ai-hook` can call a second endpoint and print a reply while staying under its 384,000-byte cap. Not built. Validated by US-064.

### Hard Constraints

- **No UX change.** Every existing shortcut, menu, drag gesture, palette entry, CLI verb, JSON-RPC method, event frame and MCP tool keeps its behavior and its wire shape. New UI appears only for prompts and for user-defined actions.
- **The host protocol does not change.** `HOST_PROTOCOL_VERSION` and the host's method set stay as they are. Nothing in this PRD may require a user to stop a terminal.
- **Helper size caps hold.** `paneflow-shim` 524,288 bytes, `paneflow-ai-hook` 384,000 bytes, `paneflow-mcp` 524,288 bytes. Neither `paneflow-api`, `paneflow-layout`, `paneflow-script`, `schemars` nor `mlua` is linked into a helper.
- **Cross-platform.** Every change has a Linux, macOS and Windows path. `cfg(windows)` code is linted only by the Windows CI leg, and any item added to a file with a `mod tests` goes before that module (`AGENTS.md`). No Mac is available before 2027: macOS is validated by CI.
- **The render thread never blocks.** No socket call, no Lua and no file I/O on the GPUI main thread.
- **No comments in source code.** Intent goes into names, types and tests. Clap help lives in attributes.
- **Tests assert outcomes, perf gates own latency.** No new wall-clock assertion in a unit test (`scripts/check-test-wallclock.sh`).
- **Rust 1.98.0** (`rust-toolchain.toml`), every cargo invocation `--locked`.
- **One engine per job.** Lua is embedded through mlua; no home-made interpreter (P12).
- **Security boundary unchanged.** Gates protect from a confused agent, not from a hostile process of the same user (`docs/user/scripting.md:121-123`). A script runs with the user's rights.
- **Commits** follow `type(scope): US-NNN - description`, atomic per story, no AI attribution.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints on the host target
- `cargo nextest run --workspace --locked --profile ci` - the test suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget, when a test changes
- `cargo deny check advisories licenses sources` - when `Cargo.toml`, `Cargo.lock` or `deny.toml` changes
- `bash scripts/check-helper-sizes.sh` - on Linux x86_64 after the test run, when a helper crate or a crate it depends on changes

Every story is validated locally on Fedora. Real-hardware Windows checks are grouped in EP-010; until then each story writes its Windows path, reviews its `cfg(windows)` items by inspection and says so.

For stories that change a latency-sensitive path (EP-002, EP-007), additional gate:
- `bash scripts/perf-gates.sh` - the perf gates on the calibrated Linux x86_64 runner class

For UI stories (US-039, US-043, US-045, US-046, US-049, US-066), additional gates:
- The `DESIGN.md` UI delivery gate: Dark, Light and one other preset; `reduce_motion` on and off; the 800 by 500 minimum window; every control state.
- The story lists the surfaces and states it changed for Arthur's manual visual pass. The implementing agent drives a debug build on its own tagged home (`scripts/dev.sh --tag <name>`), never the installed app.

## Epics & User Stories

### EP-001: One typed, self-describing API

Every method the app serves is declared once, with types and a schema, and everything else derives from the declaration. Nothing a client can observe changes.

**Definition of Done:** `src-app/src/app/ipc_handler/mod.rs` contains no `match` on method-name strings, `system.describe` returns an input and an output schema for 100 % of registered methods, the app conformance file passes unchanged from US-001 to the end of the epic, and `paneflow api list` prints every method with its owner.

**Rex reference:** self-describing API (L.2612-2614), `rex api list | describe | call` (page "Server API").

#### US-001: Pin the app's wire format in a conformance case file
**Description:** As the maintainer, I want every existing app method pinned by request and response cases so that the registry migration cannot change a wire shape unnoticed.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Paneflow sites:** `protocol/host-conformance-v1.json` and `crates/paneflow-host/tests/conformance.rs:14-49` (the pattern to follow); `src-app/tests/cli_controller.rs`, `src-app/tests/ipc_endpoint_fallback.rs`; `src-app/src/app/ipc_handler/jsonrpc.rs:10-17,112-157`.

**Acceptance Criteria:**
- [x] Given `protocol/app-conformance-v1.json`, when it is read, then it holds at least one success case for each of the 23 methods listed in `gates.rs:92-123`, with the full result field set of each
- [x] Given the case file, when it is read, then it holds one invalid-parameter case per method that takes a required parameter, asserting code `-32602`
- [x] Given a gated method (`surface.send_text`, `surface.send_keystroke`) with its gate closed, when it is called, then the case asserts code `-32601`
- [x] Given a case listed in the file with no runner, when the test suite runs, then it fails naming the case id
- [x] Given an unknown method name, when it is sent, then the case asserts code `-32601` and the exact current message
- [x] The runner drives a real app IPC endpoint on a per-process socket or pipe, as the host conformance runner does, and needs no display

#### US-002: Create `paneflow-api` and migrate the read methods
**Description:** As the maintainer, I want a registry crate where a method is one declaration so that dispatch and validation stop being written by hand.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Paneflow sites:** `src-app/src/app/ipc_handler/mod.rs:45-49,172-195`, `params.rs:3-57`, `surface_methods.rs:255-267,396-401,484-486,585-663`, `workspace_methods.rs:694-712`.

**Acceptance Criteria:**
- [x] Given the new crate `crates/paneflow-api`, when its dependencies are listed, then it depends on neither `gpui`, `mlua` nor any `src-app` module
- [x] Given a method declaration, when it is read, then it carries a name, a one-line summary, a parameter type and a result type that both derive `serde` and `schemars::JsonSchema`, at least one example, an owner (`window`, `worker` or `host`) and a gate
- [x] Given `system.ping`, `system.identify`, `system.counters`, `system.capabilities`, `surface.read`, `surface.search`, `surface.list`, `surface.status`, `fleet.list`, `workspace.list` and `workspace.current`, when they are called, then each is served through the registry and the US-001 cases pass unchanged
- [x] Given a request whose parameter has the wrong JSON type, when it is dispatched, then the reply is `-32602` and its message names the parameter
- [x] Given a request with an unknown extra parameter, when it is dispatched, then it is accepted as today (no new rejection)
- [x] Given the three reply kinds `Ready`, `Deferred` and `Async`, when a handler is registered, then it declares its kind and the 5 s dispatch and 30 s reply timeouts of `ipc.rs:86-88` still apply
- [x] Given two declarations with the same method name, when the registry is built, then the build of the registry fails in a test, not at runtime in production

#### US-003: Migrate the remaining methods and delete the hand-written dispatch
**Description:** As the maintainer, I want the write, layout and compatibility methods on the registry so that the string dispatch and the hand-written capability list can be deleted.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002

**Paneflow sites:** `gates.rs:3-20,92-123,145-156`; `split.rs:37-46,306-445`; `workspace_methods.rs:25-44,319-436,556,714-791`; `surface_methods.rs:1034-1200`; `tmux_compat.rs:78`; `src-app/src/ipc.rs:887-931`.

**Acceptance Criteria:**
- [x] Given `surface.rename`, `surface.focus`, `surface.send_text`, `surface.send_keystroke`, `surface.split`, `workspace.create`, `workspace.up`, `workspace.select`, `workspace.close`, `workspace.restore_layout` and the tmux compatibility method, when they are called, then each is served through the registry and the US-001 cases pass unchanged
- [x] Given `events.subscribe`, when the registry is listed, then it has a descriptor marked as a stream, and its frames (`subscribed`, `heartbeat`, `dropped`) are unchanged
- [x] Given `system.capabilities`, when it is called, then its `methods` array is computed from the registry and equals the array of US-001 byte for byte
- [x] Given the source tree, when `src-app/src/app/ipc_handler/mod.rs` is searched, then no `match` arm compares a method name string, and `capabilities_value` holds no literal list
- [x] Given `PANEFLOW_IPC_SCRIPTING` and `PANEFLOW_IPC_ORCHESTRATION` unset, when a gated method is called, then it is refused exactly as before, and the gate is read from the descriptor
- [x] Given a `layout` parameter with more leaves than `MAX_PANES`, when `workspace.restore_layout` is called, then it is refused with the same code and message as before

#### US-004: `system.describe` and a checked-in schema snapshot
**Description:** As a client author, I want to ask a running Paneflow what each method takes and returns so that I can call it without reading source.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given `system.describe` with no parameter, when it is called, then it returns every method this endpoint serves with name, summary, owner, gate, stream flag, input schema, output schema and examples
- [x] Given `system.describe` with `method`, when it is called with a known name, then it returns that one descriptor
- [x] Given `system.describe` with an unknown `method`, when it is called, then the reply is `-32602` and names the method
- [x] Given a method owned by another process, when the full list is requested, then it appears with its owner and without being callable on this endpoint
- [x] Given `protocol/api-v1.json`, when a test regenerates the description from the registry, then the two are equal, and the failure message tells how to regenerate the file
- [x] The emitted schemas are JSON Schema 2020-12 and each validates its own examples in a test

#### US-005: `paneflow api list`, `describe` and `call`
**Description:** As a user, I want to explore and call the API from the shell so that I can script without a reference page.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Paneflow sites:** `src-app/src/cli/mod.rs:22-25,570-624`.

**Acceptance Criteria:**
- [x] Given a running app, when `paneflow api list` runs, then it prints one line per method with its owner and summary, and `--json` prints the descriptors
- [x] Given `paneflow api describe surface.read`, when it runs, then it prints the parameters with type, required or optional and default, the result fields, and one example invocation
- [x] Given `paneflow api call surface.list`, when it runs, then it prints the JSON result and exits 0
- [x] Given `paneflow api call surface.read lines=50 fenced=true`, when it runs, then `key=value` pairs are typed from the method's schema (`50` as an integer, `true` as a boolean), and `--args '{...}'` or `--args @file.json` passes raw JSON
- [x] Given an unknown method or a parameter the schema rejects, when `paneflow api call` runs, then it prints the registry's message and exits with the existing error exit code
- [x] Given no running window, worker or host, when any `paneflow api` verb runs, then it exits with the existing "no instance" code and message
- [x] No existing subcommand changes its flags, output or exit code

#### US-006: Route each call to its owner
**Description:** As a client author, I want one client that sends each method to the process that owns it so that scripts do not hard-code endpoints.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Paneflow sites:** `crates/paneflow-ipc-client/src/host_control.rs:64-114`; `src-app/src/cli/mod.rs:570-624`; `crates/paneflow-host/src/control.rs:26-49`; `crates/paneflow-serve/src/protocol.rs:20-31`.

**Acceptance Criteria:**
- [x] Given the registry, when it is listed, then the host control methods and the worker's `worker.*`, `agent.*` and `usage.*` methods have descriptors with owner `host` or `worker`, and no host or worker code changes in this story
- [x] Given a router built on the registry, when a method owned by the window is called and the window listens, then the request goes to the window's socket
- [x] Given a method owned by the window and no window, when the host serves a control answer for it, then the router falls back to the host exactly as `resolve_control_target` does today
- [x] Given a method owned by the worker and no worker reachable, when it is called, then the error names the worker and the method, and nothing is sent to another endpoint
- [x] Given `paneflow api list` with a window and a worker running, when it runs, then it shows the union of the methods, each tagged with its owner
- [x] The router lives outside `paneflow-ipc-client`, so no helper binary links it, and each helper's `release-min` size is unchanged by the story

---

### EP-002: The layout becomes plain data with stable ids

Workspaces, tabs, split trees, panes and surfaces become one plain-data model with a single entry point for change. The GPUI tree is recomputed from it. The model stays in the window in this epic. The app looks and behaves exactly as before.

**Definition of Done:** `src-app/src` holds 0 direct assignments to a tab's `root` or `saved_layout` outside the projection module (guard test), every structural operation of the US-007 inventory goes through `paneflow_layout::Model::apply`, the model's tests run with no GPUI context, workspace, tab, pane and surface ids survive a restart, and Arthur's visual pass over the US-020 checklist reports no difference.

**Rex reference:** the server owns sessions, windows and blocks (L.8897, L.8539); ids never change and are never reused (L.1202-1203); zoom is a display flag (L.9038).

#### US-007: Complete the operation inventory
**Description:** As the maintainer, I want the full list of structural operations with their rules so that the model's ops are specified from facts.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Paneflow sites:** `src-app/src/layout/mutations.rs:77,121`, `close.rs:8`; `src-app/src/app/workspace_ops/mod.rs:644-1095`, `layout.rs:15-196`, `tab.rs:51-662`, `focus.rs:20`; `src-app/src/app/event_handlers/mod.rs:200-575`; `src-app/src/app/close_policy.rs:134-250`; `src-app/src/app/ipc_handler/split.rs:170-445`, `workspace_methods.rs:340-791`; `src-app/src/app/template_launch.rs:140-141`; `src-app/src/app/detached_panes/`.

**Acceptance Criteria:**
- [x] Given the section "Layout Operation Inventory" of this PRD, when this story ends, then each operation has its entry points, the function that mutates, the focus rule, the session effect (none, spawn, hold, stop, rehome) and whether it is refused while zoomed, with `file:line`
- [x] Given the paths the 2026-11-19 inventory left untraced (IPC `surface.split`, IPC `workspace.close`, `create_workspace_with_picker`, `open_pane_in_new_workspace_tab`, `split_pane_at_edge`, templates, `detached_panes/persistence.rs`), when the section is read, then each is traced, or named as out of scope with a reason
- [x] Given an operation with more than one entry point, when the section is read, then it lists every entry point and says whether they behave identically
- [x] Given the two behaviors reported as possible defects (undo of a pane holding several surfaces restores one; a non-active surface resolves to a terminal but not to a pane), when the section is read, then each is confirmed or refuted with the code path that decides it
- [x] No production code changes in this story

#### US-008: Pin today's behavior: create, close, undo, focus, rename
**Description:** As the maintainer, I want tests that state what the creating and closing operations do today, written against outputs the refactor keeps, so that the model can be proven equivalent.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given split, new surface, new tab, new workspace, close surface, close pane, remove a given pane, close tab, close workspace, undo close, focus, select and rename, when the suite runs, then at least one test per operation asserts its resulting tree shape, ratios, focus target and session effect through the serialized `LayoutNode` and the requests sent to a fake host
- [x] Given a split requested while a tab is zoomed, or at `MAX_PANES`, when the test runs, then it asserts the current refusal
- [x] Given a close that empties a tab, when the test runs, then it asserts that the tab receives a fresh pane
- [x] Given undo within and after the 5 s hold, and a sixth record pushed on a full history, when the tests run, then they assert reattachment, a fresh terminal, and the eviction with its stop
- [x] Given each suspected defect that US-007 confirmed, when the suite runs, then one test pins the current behavior under a name that says so
- [x] Given the new tests, when they are read, then each drives the entry point a user or an IPC client triggers, and none reads a private field of `LayoutTree`, so the same test compiles after the refactor

#### US-009: Pin today's behavior: move, reorder, resize, zoom, presets, restore
**Description:** As the maintainer, I want the same pinning tests for the operations that rearrange an existing layout so that every op of the model has a reference.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given swap, move pane to an edge, move surface, drop an agent session, move pane to a new tab, move tab to a workspace, reorder tabs, reorder workspaces, divider resize, equalize, each preset, zoom, restore layout and templates, when the suite runs, then at least one test per operation asserts its resulting tree shape, ratios, focus target and session effect as US-008 does
- [x] Given a move or a preset on a zoomed tab, when the test runs, then it asserts whether zoom is left first or the operation is refused, per operation
- [x] Given a reorder to an index past the end, and `workspace.up` with an invalid pane in the middle of its list, when the tests run, then they assert today's outcome
- [x] Given restore layout with fewer panes than the tab holds, when the test runs, then it asserts which panes close and with which session effect
- [x] Given an operation that cannot be driven in a test without a display (a pointer drag, a detached window), when the inventory is read, then it is listed with the reason and added to the checklist of US-020
- [x] Given the new tests, when they are read, then none reads a private field of `LayoutTree`

#### US-010: `paneflow-layout`: the model, ids and the view
**Description:** As the maintainer, I want the layout as plain data in its own crate so that it can be tested, serialized and later moved to another process.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008, US-009

**Paneflow sites:** `crates/paneflow-config/src/schema/layout.rs:125-139,326-349`, `schema/session.rs:14-30,63-79,200-222`; `src-app/src/workspace/mod.rs:101-128`, `tab.rs:90-100`; `src-app/src/pane.rs:29-40`.

**Acceptance Criteria:**
- [x] Given the new crate `crates/paneflow-layout`, when its dependencies are listed, then it depends on neither `gpui` nor any `src-app` module, and its tests need no display
- [x] Given the model, when it is read, then it holds workspaces (id, title, space, ordered tabs, active tab), tabs (id, title, split tree, focused pane, zoomed pane), panes (id, ordered surfaces, active surface) and surfaces (id, kind terminal or markdown, name, the fields of `SurfaceDefinition`), plus the active workspace and a `revision`
- [x] Given any id, when it is allocated, then it is a `u64` taken from one counter held by the model, strictly increasing, and never reused after a close
- [x] Given a `SessionState` written by the current release, when the model loads it, then it allocates ids for every workspace, tab, pane and surface, and exporting the model back yields an equal `SessionState` for every existing field
- [x] Given a `SessionState` already carrying ids and a counter, when the model loads it, then every id is kept and the counter is at least one above the highest id
- [x] Given a corrupt or duplicated id in the input, when the model loads it, then the offending element gets a fresh id, one warning is logged, and loading succeeds
- [x] Given the model, when `view()` is called, then it returns a plain-data snapshot with the `revision`, and two calls with no op in between are equal

#### US-011: Operations, part 1: create, close, focus, rename
**Description:** As the maintainer, I want the creating, closing and focusing operations expressed as ops on the model so that their rules live in one place.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given `Model::apply(op)`, when an op succeeds, then it returns the events it caused and the effects it requests (spawn a session for a surface, hold a session, stop a session, rehome a session to a workspace), and increments `revision` by exactly 1
- [x] Given an op that fails, when it returns, then the model is unchanged, `revision` is unchanged, and the error is a typed reason (`not_found`, `zoomed`, `pane_limit`, `invalid`)
- [x] Given the ops split (direction, target pane, edge), new surface in a pane, new tab, new workspace, close surface, close pane, close tab, close workspace, focus pane, select tab, select workspace, rename surface, rename tab, rename workspace and bind session (a surface and a host session id), when each is applied, then the resulting tree, ratios, focus and effects equal those pinned by US-008 and US-009
- [x] Given a close that empties a tab, when it is applied, then the tab receives a fresh pane as today
- [x] Given bind session for a surface that does not exist, when it is applied, then it fails with `not_found`; given a surface that already names a live session, then it fails with `invalid`
- [x] Given a split while the tab is zoomed, or at `MAX_PANES`, when it is applied, then it fails with `zoomed` or `pane_limit`
- [x] Given a sequence of 1,000 random valid ops in a property test, when it runs, then after each op every id is unique, every tab's focused pane exists in its tree, every split has at least two children, and ratios of each split sum to 1 within 1e-6

#### US-012: Operations, part 2: move and reorder
**Description:** As the maintainer, I want the moving and reordering operations on the model so that drag-and-drop and tab moves share one rule set.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given the ops swap panes, move pane to an edge, move surface to a pane or an edge, move pane to a new tab, move tab to another workspace, reorder tabs and reorder workspaces, when each is applied, then the result equals what US-008 and US-009 pinned
- [x] Given a move that empties its source tab, when it is applied, then the source tab is closed as today
- [x] Given a move that takes a terminal to another workspace, when it is applied, then one rehome effect is requested per terminal session moved
- [x] Given a move whose source or target does not exist, when it is applied, then it fails with `not_found` and the model is unchanged
- [x] Given an op that today leaves zoom first (move pane to a new tab, a detached transfer), when it is applied on a zoomed tab, then zoom is cleared as part of the same op; given an op that today is refused while zoomed, then it fails with `zoomed`
- [x] Given a reorder to an index past the end, when it is applied, then the result equals what US-008 and US-009 pinned for that case

#### US-013: Operations, part 3: resize, zoom, presets, restore
**Description:** As the maintainer, I want resizing, zoom, presets, restore and templates on the model so that no caller needs to touch the tree.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given the ops set divider ratio, equalize, apply preset (even horizontal, even vertical, tiled, main vertical), set zoom, restore layout from a `LayoutNode` and launch template, when each is applied, then the result equals what US-008 and US-009 pinned
- [x] Given zoom, when it is set on a pane, then the tree is unchanged and only the tab's `zoomed` field changes, and the exported `SessionState` equals what today's tree swap would have saved
- [x] Given a preset applied on a zoomed tab, when it is applied, then zoom is cleared as part of the same op
- [x] Given a divider ratio outside the floor that `sanitize_ratios` enforces, when it is applied, then it is clamped as today
- [x] Given restore layout with fewer panes than the tab holds, when it is applied, then the surplus panes are closed with the same session effect as today
- [x] Given restore layout with more leaves than `MAX_PANES`, when it is applied, then it fails with `pane_limit` and the model is unchanged

#### US-014: Undo close in the model
**Description:** As a user, I want undo close to behave exactly as today so that the refactor is invisible.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Paneflow sites:** `src-app/src/app/workspace_ops/mod.rs:37-51,117-120,181-244,886-940`; `src-app/src/app/hosted_sessions.rs:222-262`; `src-app/src/app/constants.rs:301-303`.

**Acceptance Criteria:**
- [x] Given a closed pane or tab, when the op is applied, then the model keeps a closed record and requests a hold effect of `CLOSED_SESSION_GRACE_MS` for each terminal session it held
- [x] Given more than `MAX_CLOSED_PANES` records, when a new one is pushed, then the oldest is evicted and a stop effect is requested for its held sessions
- [x] Given undo within the hold, when it is applied, then the surface comes back bound to the same session
- [x] Given undo after the hold expired, when it is applied, then the surface comes back with no session and a spawn effect is requested
- [x] Given a removal that today stops the session at once and records nothing (`PaneEvent::Remove`, the tmux compatibility close), when it is applied, then no closed record is created and a stop effect is requested
- [x] Given undo with an empty history, when it is applied, then it fails with `not_found` and nothing changes
- [x] The model takes the current time as an input, so the tests advance a fake clock and assert no wall-clock duration

#### US-015: The window projects the model
**Description:** As the maintainer, I want the GPUI tree recomputed from the model through one entry point so that the keyboard, menu and palette paths stop mutating the tree.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012, US-013, US-014

**Paneflow sites:** `src-app/src/layout/serde.rs:97-106` (pane reuse); `src-app/src/layout/tree.rs:22-35`; `src-app/src/app/workspace_ops/`; `src-app/src/main.rs:204-209,316`.

**Acceptance Criteria:**
- [x] Given one function `apply_layout_op` on the app, when a keyboard, menu or palette path changes the structure, then it calls that function and nothing else mutates workspaces, tabs or trees
- [x] Given a model change, when the projection runs, then every pane and terminal view whose id is still present is reused, not recreated, and its terminal keeps its scrollback and selection
- [x] Given a spawn, hold, stop or rehome effect, when the op returns, then the window performs it through the code paths used today
- [x] Given a model focus change that did not come from the user's click or key, when the projection runs, then GPUI focus moves to that pane
- [x] Given a click or key that moves GPUI focus to another pane, when it happens, then a focus op is applied and the model's focused pane matches
- [x] Given an op that fails, when it returns, then the user sees the same toast or no-op as today, and no partial change is rendered
- [x] The projection performs no I/O and runs on the main thread only as a pure rebuild

#### US-016: Drag-and-drop and dividers go through the model
**Description:** As the maintainer, I want pointer gestures to produce model ops so that the tree is no longer edited from event handlers.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

**Paneflow sites:** `src-app/src/app/event_handlers/mod.rs:231-575`; `src-app/src/layout/render.rs:176-182`; `src-app/src/app/render.rs:384-386`.

**Acceptance Criteria:**
- [x] Given a drag-and-drop of a pane, a surface or an agent session, when it lands, then the change is one model op with the result pinned by US-008 and US-009
- [x] Given a drop that is refused today (a detached pane, a zoomed tab), when it lands, then no op is applied and the user sees the same refusal
- [x] Given a divider drag, when the mouse moves, then the ratio changes only in the window's view state, and exactly one set-ratio op is applied when the drag ends
- [x] Given a divider drag cancelled by a pane transfer, when it is cancelled, then no op is applied and the divider returns to the model's ratio
- [x] Given two dividers of nested splits dragged one after the other, when each drag ends, then each produces one op that names its own split
- [x] The drag path performs no I/O and takes no lock shared with a background thread

#### US-017: Detached panes are keyed by pane id
**Description:** As a user, I want a detached pane to find its place again after any layout change so that detaching stays reliable once the layout can change from outside.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

**Paneflow sites:** `src-app/src/app/detached_panes/mod.rs:13-117`, `window.rs:147-281`; `crates/paneflow-config/src/schema/session.rs:63-79`.

**Acceptance Criteria:**
- [x] Given a detached pane, when its placement is saved and restored, then it is keyed by pane id
- [x] Given a session file that holds a positional `DetachedPaneLocation`, when it is loaded, then the location is converted to the pane id found at that position
- [x] Given a positional location that points at no pane, when it is loaded, then it is dropped with one warning and loading succeeds
- [x] Given a pane removed from the model while detached, when the projection runs, then its window closes as today
- [x] Given a detached window closed by the user, when it closes, then the pane is shown in its tab again and no structural op is applied
- [x] Given a transfer of a detached pane, when it happens, then in-progress divider drags are cancelled and zoom is cleared as today

#### US-018: IPC paths go through the model and direct assignments end
**Description:** As the maintainer, I want the last mutation paths on the single entry point so that the direct assignments can be deleted for good.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016, US-017

**Paneflow sites:** `src-app/src/app/ipc_handler/split.rs:223-252`, `workspace_methods.rs:747-791`; `src-app/src/app/template_launch.rs:140-141`.

**Acceptance Criteria:**
- [x] Given `surface.split`, `workspace.create`, `workspace.up`, `workspace.close`, `workspace.restore_layout` and the tmux compatibility method, when they are called, then each applies model ops and the US-001 cases pass unchanged
- [x] Given a template launch, when it runs, then it applies model ops
- [x] Given `workspace.up` with an invalid pane in the middle of its list, when it is called, then the outcome equals what US-008 and US-009 pinned for that case
- [x] Given `src-app/src`, when a guard test searches it, then no assignment to a tab's `root` or `saved_layout` exists outside the projection module, including through a `&mut` reference
- [x] Given the mutators of `src-app/src/layout/mutations.rs` and `src-app/src/layout/close.rs`, when the story ends, then each is deleted or private to the projection module

#### US-019: Stable ids on the wire and one save path
**Description:** As a script author, I want the ids I receive to stay valid across a restart so that a saved id or `PANEFLOW_SURFACE_ID` keeps meaning the same pane.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-018, US-003

**Paneflow sites:** `src-app/src/app/ipc_handler/surface_methods.rs:41-155,787-833`; `src-app/src/terminal/pty_session/spawn_env.rs:217`; `src-app/src/tmux_compat/mod.rs:118-121`; `src-app/src/app/session.rs:22-28,103-120,1196-1216`; `crates/paneflow-host/src/agent_write.rs:415`.

**Acceptance Criteria:**
- [x] Given `surface.list`, `surface.status`, `surface.split`, `workspace.up`, `fleet.list` and the MCP tools, when they return a `surface_id`, `tab_id` or a workspace id, then it is the model's id, still a JSON number
- [x] Given an incoming `surface_id`, when it is resolved, then the lookup goes through the model, finds a surface that is not the active one of its pane, and returns its pane
- [x] Given a layout saved with ids, when the app restarts, then `surface.list` returns the same `surface_id` for each surviving terminal session, and a new pane spawned afterwards exports that same id as `PANEFLOW_SURFACE_ID`
- [x] Given `session.json`, when it is written, then it carries the ids and the counter as added fields, and no existing field is renamed or removed
- [x] Given one subscriber on model changes, when the model's revision changes, then a save is scheduled with the existing 150 ms debounce (`SAVE_DEBOUNCE_MS`, `src-app/src/app/session.rs:22`), and `save_session` has no other call site for structural changes (guard test)
- [x] Given an id that no longer exists, when a method receives it, then it replies with the same not-found error as today
- [x] Given a session file with no ids and a restored terminal whose host record exposes the `PANEFLOW_SURFACE_ID` it was spawned with (to be confirmed in this story from `crates/paneflow-host/src/agent_write.rs:415` and `session.inspect`, with no host change), when ids are first allocated, then the surface takes that value when no other surface claims it, and the counter starts above the highest id
- [x] Given the host does not expose that value, when ids are first allocated, then they are fresh, and the changelog entry says that a pane opened before the upgrade keeps a stale `PANEFLOW_SURFACE_ID` until it is restarted, as it does today after every window restart

#### US-020: Prove parity
**Description:** As Arthur, I want evidence that nothing changed for the user so that I can accept the refactor.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-019

**Acceptance Criteria:**
- [x] Given a benchmark case added to the terminal or startup suite, when it runs before and after the epic on the same machine, then the time from a split request to the model and projection being updated is recorded in `bench/results/`, with both values
- [x] Given `crates/paneflow-host/tests/persistent_baseline/gates.rs`, when the epic ends, then no budget in it was raised
- [x] Given a session file with 3 workspaces, 8 tabs and 20 panes saved by the previous release, when the new build opens it, then the restored layout, ratios, titles, zoom and detached windows equal the original, asserted through the exported `SessionState`
- [x] Given the checklist written by this story, when Arthur reads it, then it lists every gesture to try (each split, each close, undo, each drag, zoom, presets, detach and reattach, rename, reorder) with the expected result
- [x] Given a model op that panics in a test build, when it is caught, then the window keeps its last projected tree and logs the op, and the suite has a test for a failing op leaving the UI intact

---

### EP-003: A gap-free event log

The worker keeps one numbered log of what happens (terminals, layout, agents, configuration) and any client can follow it without losing or repeating an event.

**Definition of Done:** `events.follow` delivers the terminal, agent and layout event types of FR-07 (those produced by US-023 and US-024) with strictly consecutive `seq` values to two concurrent followers in a conformance test, a follower that reconnects with `since` receives exactly the events it missed or an explicit `evicted` error, and the legacy `events.subscribe` frames on the window's socket are unchanged.

**Rex reference:** gap-free `seq` and the 8 MiB rule (L.8285-8287); event names (page "Lua Events").

#### US-021: The worker serves its methods through the registry
**Description:** As the maintainer, I want the worker's dispatch on the registry so that new worker method families are declared once.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Paneflow sites:** `crates/paneflow-serve/src/server.rs:599-700`, `protocol.rs:20-71`; `protocol/controller-conformance-v1.json`; `ARCHITECTURE.md:692-696`.

**Acceptance Criteria:**
- [x] Given `worker.hello`, `worker.status`, `worker.shutdown`, `agent.snapshot`, `agent.follow`, `agent.activity_log`, `agent.acknowledge` and the `usage.*` methods, when they are called, then each is served through `paneflow-api` and `crates/paneflow-serve/tests/controller_conformance.rs` passes unchanged
- [x] Given `system.describe` on the worker endpoint, when it is called, then it returns the worker's methods
- [x] Given a client that has not sent `worker.hello`, when it calls another method, then it is refused as today
- [x] Given `ARCHITECTURE.md`, when this story ends, then its "No front door" paragraph is rewritten: the worker answers the method families it owns, still forwards nothing to the core, and the reason (it is replaced on every update and owns no terminal) is stated
- [x] Given the worker's capability file `protocol/host-capabilities-v1.json`, when a new method family is added later, then a client that reads an older set refuses the call before a frame leaves, as today

#### US-022: The event log and `events.follow`
**Description:** As a script author, I want to follow events with a sequence number so that I never miss or repeat one.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-021

**Acceptance Criteria:**
- [x] Given the log, when an event is appended, then it gets the next `seq` (starting at 1 for a worker start), a `ts` in milliseconds, a `type`, the ids that apply (`workspace_id`, `tab_id`, `pane_id`, `surface_id`, `session`) and a `data` object
- [x] Given `events.follow` with no parameter, when it is called, then the stream sends a header with the current `seq` and the worker's `epoch`, then every later event in order
- [x] Given `events.follow` with `since` and the same `epoch`, when the events after `since` are still in the ring, then the stream replays them first, with no gap and no duplicate
- [x] Given `since` older than the ring, or an `epoch` that is not the worker's, when `events.follow` is called, then it replies with a typed `evicted` error carrying the current `seq` and `epoch`
- [x] Given `types` (exact names or a `prefix.*` form) and `workspace_id`, when they are passed, then only matching events are sent, and the `seq` values of sent events stay those of the log
- [x] Given a follower whose unsent backlog exceeds 8 MiB, when the next event is queued, then the worker closes that connection and other followers are unaffected
- [x] Given the ring, when more than 1,024 events exist, then the oldest are dropped from the ring and `seq` keeps increasing
- [x] Given no frame for the keepalive interval `agent.follow` uses (`FOLLOW_KEEPALIVE`, 2 s, `crates/paneflow-serve/src/server.rs:31`), when a follower is connected, then it receives a keepalive frame

#### US-023: Terminal and agent events from the host stream
**Description:** As a script author, I want events for directory changes, title changes, bells and program exits so that scripts react without polling.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022

**Paneflow sites:** `crates/paneflow-serve/src/core_link.rs:45-155`, `state.rs:94-118,473-528`, `worker.rs:268-332`; `crates/paneflow-host/src/agent.rs:168-198`.

**Acceptance Criteria:**
- [x] Given a session whose working directory changes, when the host frame arrives, then one `terminal.cwd_changed` event is appended with the new `cwd` and the session id
- [x] Given a title change, a new `bell_at_ms` value, or a lifecycle that becomes exited, when the frame arrives, then one `terminal.title_changed`, `terminal.bell` or `terminal.exited` (with `exit_code` when the host recorded one) is appended
- [x] Given an agent session whose projected state changes, when the worker's reducer applies it, then one `agent.state_changed` event is appended with the tool, the previous state and the new state
- [x] Given a plain shell with no agent, when it changes directory and exits, then the events above are appended for it (validates A2)
- [x] Given the worker reconnecting to the host after a dropped stream, when it takes a fresh snapshot, then it appends events only for fields that differ from its last known values, and none for unchanged sessions
- [x] Given a worker start, when it rebuilds from the host manifests, then no event is appended for the initial state
- [x] The host crate is not modified by this story

#### US-024: Layout events from the window
**Description:** As a script author, I want events for panes, tabs and workspaces being created, closed, focused and renamed so that scripts follow the layout.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022, US-019

**Acceptance Criteria:**
- [x] Given a model op applied in the window, when it returns events, then the window sends them to the worker with `events.publish` on a background thread, in the order the ops were applied
- [x] Given `workspace.created`, `workspace.closed`, `workspace.renamed`, `workspace.selected`, `tab.created`, `tab.closed`, `tab.renamed`, `tab.selected`, `pane.created`, `pane.closed`, `pane.focused`, `surface.created`, `surface.closed`, `surface.renamed` and `layout.changed` (with the revision), when the matching op is applied, then exactly one event of that type is appended with the model ids
- [x] Given the worker unreachable, when ops are applied, then their events are kept in a bounded window-side queue of 1,024 and sent in order on reconnect, and if the queue overflows the window sends one `layout.resync` event instead of the lost ones
- [x] Given a divider drag, when it ends, then exactly one `layout.changed` is published for it
- [x] Given `events.publish` with an event type outside the layout family, when it is called, then it is refused with `-32602`
- [x] Given the legacy `events.subscribe` on the window's socket, when it is used, then its frames and types are unchanged

#### US-025: Follow events from the CLI
**Description:** As a user, I want to watch the event log from a shell so that I can see what a script would receive.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-022, US-006

**Acceptance Criteria:**
- [x] Given `paneflow events`, when it runs, then it prints one line per event with `seq`, type and ids until interrupted, and `--json` prints one JSON object per line
- [x] Given `--types terminal.*,pane.closed` and `--since <seq>`, when they are passed, then they map to the `events.follow` parameters
- [x] Given the worker restarting while the command runs, when the stream ends, then the command reconnects, prints one line saying the sequence restarted, and continues
- [x] Given no worker, when the command runs, then it exits with the existing "no instance" code
- [x] `paneflow watch` and `paneflow wait` are unchanged

---

### EP-004: Lua scripts from the CLI

A user runs a Lua script against Paneflow with `paneflow do`. The script can call every method, wait for events, set timers and run commands. First user-visible benefit of the PRD.

**Definition of Done:** the example scripts of US-029, US-031 and US-032 run from a shell on Linux against a debug build, the workspace compiles with mlua on the four shipping targets in CI, `cargo deny` passes, and `paneflow api stubs` produces a file that the Lua language server loads without error.

**Rex reference:** pages "Lua Scripts", "Lua Events", "Rex Lua API".

#### US-026: Validate the engine on every target
**Description:** As the maintainer, I want the engine choice proven on the four targets before any code depends on it so that a build failure does not surface in the last epic.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given the new crate `crates/paneflow-script` depending on `mlua = "=0.12.2"` with features `lua54`, `vendored`, `async` and `serialize` (and without `send`), when the workspace builds, then it compiles on Linux x86_64 locally
- [ ] Given the branch pushed for this story, when `run_tests` completes, then the Linux aarch64, macOS and `Windows x86_64 libghostty check` legs compile and lint the workspace with the new crate
- [x] Given `deny.toml`, when the story ends, then no license outside MIT was added to it for mlua or its dependencies, and no advisory is ignored for them
- [x] Given the seven probe cases of R13, when the suite runs, then each is a test: direct call, nested call, call under `pcall`, call inside `__index`, two concurrent calls with a computation, a call inside a `table.sort` comparator (asserts an error), a call inside a coroutine the script created (asserts today's behavior, to be replaced by US-033)
- [x] Given a release build before and after the dependency, when their sizes and the clean build time are measured, then both deltas are recorded in the story report, and no helper binary links `mlua`
- [x] Given a target that fails to compile, when the story ends, then it is BLOCKED with the compiler output, and no later story of this epic starts

#### US-027: The script runtime core
**Description:** As a script author, I want a predictable Lua environment so that scripts written against the documentation run.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [x] Given a new state, when it is created, then the standard libraries `base`, `table`, `string`, `utf8`, `math`, `coroutine`, `io`, `os` and `package` are loaded, the `debug` library is absent, and C modules cannot be loaded
- [x] Given `io.popen` or `os.execute`, when a script calls it, then it raises an error that names `paneflow.exec`
- [x] Given `require("name")`, when it runs, then it searches the script's directory and `<home>/lua/`, where the home is resolved by `paneflow-home`
- [x] Given `paneflow.json.encode` and `paneflow.json.decode`, when they convert, then a table whose keys are 1 to n becomes an array, any other table including an empty one becomes an object, JSON `null` becomes `nil`, integers stay integers, and a function or a self-containing table makes `encode` return `nil` and a message
- [x] Given `paneflow.log([level,] message)`, when it is called, then the level is one of `debug`, `info`, `warn`, `error`, defaults to `info`, and an unknown level raises an error
- [x] Given a Lua error, when it reaches the runtime, then the reported message carries the file name and line
- [x] The crate depends on neither `gpui` nor `src-app`, and its tests need no running Paneflow

#### US-028: Bind the API into Lua
**Description:** As a script author, I want `paneflow.surface.read{...}` and every other method available as a function so that I never build JSON by hand.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-027, US-006

**Acceptance Criteria:**
- [x] Given a connection, when the runtime starts, then it builds `paneflow.<namespace>.<method>` for every method returned by `system.describe` on the reachable endpoints, so a method added to the registry needs no change in the script crate
- [x] Given `paneflow.call(method, args)`, when it is called, then it calls any method by full name
- [x] Given a successful call, when it returns, then the function returns the result as a table; given a failed call, then it returns `nil`, the message and the numeric code, and raises no error
- [x] Given a method whose schema has an optional `surface_id` or `workspace_id`, when the script omits it and runs inside a pane, then the value from `PANEFLOW_SURFACE_ID` or the pane's workspace is filled in, and `paneflow.ctx` exposes `surface_id`, `pane_id`, `tab_id`, `workspace_id` and `origin`
- [x] Given an argument table with a key the schema does not declare, when the call is made, then it returns `nil` and a message naming the key and listing the declared ones, without sending the request
- [x] Given an API call in progress, when a timer of the same script comes due, then the timer runs before the call returns (the call suspends the coroutine, it does not block the thread)
- [x] Given `paneflow.layout.pane{...}`, `paneflow.layout.horizontal(ratio, a, b)` and `paneflow.layout.vertical(ratio, a, b)`, when they are called, then they return a table that `workspace.create` accepts as `layout`, and they call nothing
- [x] Given the method list unavailable at start (no instance), when a script calls any `paneflow.<namespace>` function, then it returns `nil` and the "no instance" message

#### US-029: `paneflow do`
**Description:** As a user, I want to run a Lua file, an inline chunk or stdin against Paneflow so that automation is one command.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-028

**Rex reference:** page "Lua Scripts", sections "Running a Script", "Arguments", "Results and Errors".

**Acceptance Criteria:**
- [x] Given `paneflow do ./script.lua`, `paneflow do -e '<chunk>'` and `paneflow do -`, when each runs, then the chunk executes in the CLI process
- [x] Given an argument that ends in `.lua` or contains a path separator, when `paneflow do` parses it, then it is a file; anything else is an action name (served from EP-005, and until then refused with "no action named")
- [x] Given `key=value` pairs after the script, when it runs, then `paneflow.args` holds them typed by spelling (`n=2` a number, `on=true` a boolean, `name=x` a string), and `--args '{...}'` or `--args @file.json` replaces them with raw JSON
- [x] Given a script that returns a value, when it ends, then the value is printed as one line of JSON, `--json` indents it, and a script that returns nothing prints nothing
- [x] Given a script that raises an error, when it ends, then `Error: <message>` is printed to stderr and the exit status is 1
- [x] Given `paneflow.stop(3)`, when it is called, then the script ends at once and the exit status is 3
- [x] Given a file that does not exist or does not parse, when `paneflow do` runs, then it prints the path or the syntax error with its line and exits 1 without contacting Paneflow
- [ ] Given this example, when it runs inside a pane of a debug build, then a second pane opens beside the first and the script prints its id: `local s = assert(paneflow.surface.split{ direction = "right" }) return { surface_id = s.surface_id }`

#### US-030: Sleep and timers
**Description:** As a script author, I want `sleep`, `after` and `every` so that a script can act later without blocking.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-029

**Rex reference:** page "Lua Scripts", section "Timers".

**Acceptance Criteria:**
- [x] Given `paneflow.sleep(seconds)`, when it is called with a fractional value, then the script resumes after it and other timers of the script run meanwhile
- [x] Given `paneflow.after(seconds, fn)` and `paneflow.every(seconds, fn)`, when they are called, then each returns a timer with `t:cancel()` and `t:active()`, and `fn` receives a `ctx` whose `origin` is `timer`
- [x] Given a script with an active timer, when its top level ends, then `paneflow do` keeps running, prints one status line naming the count of timers, and exits when none is active
- [x] Given a tick that comes due while the script is running other Lua, when the script becomes free, then `fn` runs once, and missed ticks are dropped
- [x] Given an error inside a timer function, when it is raised, then it is printed and the timer keeps running
- [x] Given `paneflow.every(0, fn)`, when it is called, then it raises an error
- [x] Given a script that holds 1,024 active timers, when it creates one more, then the script stops with status 1, even inside `pcall`
- [x] The tests drive a fake clock and assert no wall-clock duration

#### US-031: React to events: `paneflow.on` and `paneflow.wait`
**Description:** As a script author, I want to wait for an event or handle every occurrence so that I stop polling.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-030, US-023, US-024

**Rex reference:** page "Lua Events"; `rex.wait` (page "Lua Scripts", "Sleeping and Waiting").

**Acceptance Criteria:**
- [x] Given `paneflow.on(event, fn)` at the top level, when a matching event arrives, then `fn(ctx, ev)` is called with the event's `data` fields in `ev` and its ids in `ctx`, and handlers run one at a time in `seq` order
- [x] Given `paneflow.on` called from inside a handler or a timer, when it runs, then it raises an error
- [x] Given `paneflow.wait(event, filter, timeout)`, when a matching event arrives, then it returns `ev` and `ctx`; when `timeout` seconds pass first, then it returns `nil` and `"timeout"`; `filter` is a table of fields to equal or a function `filter(ev, ctx)`
- [x] Given an event name that is a prefix form (`terminal.*`), when it is registered, then it matches every type under that prefix
- [x] Given a script run inside a pane, when it follows events, then it hears its own workspace only, and `--all` hears every workspace
- [x] Given `--for 10m`, when the time runs out, then the script exits 0; given `--fail-fast`, when a handler raises, then the script exits 1; without it the error is printed and the script continues
- [x] Given the worker restarting, when the stream ends, then the runtime reconnects with `since`, and on an `evicted` reply it calls handlers registered for `events.resync` and continues from the current `seq`
- [ ] Given this example, when it runs with `PANEFLOW_IPC_ORCHESTRATION=1` in a debug build, then it prints a non-zero `exit_code`: the script splits with `command = "false"`, calls `paneflow.wait("terminal.exited", { surface_id = id }, 600)` and returns `{ exit_code = ev.exit_code }`

#### US-032: Run commands: `paneflow.exec`
**Description:** As a script author, I want to run a program and read its output so that scripts can use git and other tools without a shell.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-029

**Rex reference:** `rex.exec` and `rex.exec.start` (page "Rex Lua API").

**Acceptance Criteria:**
- [x] Given `paneflow.exec(argv, opts)`, when the program exits, then it returns a table with `code`, `signal`, `stdout`, `stderr`, `timed_out` and `ok`; a non-zero exit is a result, not an error
- [x] Given a program that does not exist, when `paneflow.exec` runs, then it returns `nil` and a message
- [x] Given `opts`, when it is passed, then `cwd`, `env` (with `false` removing a variable), `clear_env`, `stdin`, `timeout`, `stdout = false`, `stderr = false` and `text` are honored, and an unknown option logs one warning
- [x] Given `argv`, when it is run, then no shell interprets it on any platform, and a program name without a separator is resolved through `PATH` with `PATHEXT` on Windows
- [x] Given `timeout`, when it expires, then the process tree is terminated (a termination signal then a kill on Unix, a job-object or tree kill on Windows) and `timed_out` is true
- [x] Given `paneflow.exec.start(argv, opts)`, when it is called, then it returns a handle with `h:wait([seconds])`, `h:write(data)`, `h:close_stdin()`, `h:kill([signal])`, `h:running()` and `h.pid`, and function-valued `stdout` or `stderr` options receive output as it arrives
- [x] Given a command started by a handler, a timer or the top level, when that scope ends, then the command is stopped, unless it was started with `detach = true`
- [x] Given a command waiting for output, when a timer of the same script comes due, then the timer runs

#### US-033: Limits and guard rails
**Description:** As a user, I want a runaway or misused script to stop with a clear message so that it cannot hang my shell or return garbage.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-030

**Acceptance Criteria:**
- [x] Given a script in a loop that never yields, when the user presses Ctrl-C (or the process receives a console interrupt on Windows), then an instruction-count hook stops it and `paneflow do` exits with the interrupted status
- [x] Given a runtime created with a deadline, when Lua code runs past it without yielding, then the hook raises a `timeout` error in that coroutine and other coroutines of the state keep running
- [x] Given a runtime created with a memory limit, when an allocation exceeds it, then the running chunk fails with a memory error and the runtime stays usable
- [x] Given an async function (an API call, `sleep`, `exec`, a prompt) called from a coroutine the script created with `coroutine.create` or `coroutine.wrap`, when it runs, then it raises an error naming the limitation, and never returns an internal value
- [x] Given an async function called from a `table.sort` comparator or another C-boundary callback, when it runs, then the error message names the limitation and suggests moving the call out of the callback
- [x] Given `paneflow do` with no limit flags, when a script runs, then no memory limit and no deadline apply (the process belongs to the user)

#### US-034: Editor stubs and a generated reference
**Description:** As a script author, I want completion and types in my editor so that I do not guess parameter names.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-028

**Acceptance Criteria:**
- [x] Given `paneflow api stubs`, when it runs, then it prints a Lua language server definition file (`---@meta`) with one typed function per registry method and the hand-declared runtime functions (`call`, `on`, `wait`, `sleep`, `stop`, `after`, `every`, `exec`, `json`, `log`, `layout`, `args`, `ctx`)
- [x] Given `paneflow api docs`, when it runs, then it prints a Markdown reference with one section per method (parameters, result, example), generated from the registry
- [x] Given a registry change, when the suite runs, then a drift test fails until `protocol/paneflow.d.lua` is regenerated
- [x] Given a method with an optional parameter, when the stub is read, then the parameter is marked optional
- [x] Given a method or parameter name that is a Lua keyword (such as `end`) or is not a valid identifier, when the stub is generated, then it is emitted in bracket form and the file still loads in the Lua language server
- [x] Given no running instance, when either verb runs, then it still prints the output from the registry linked into the binary

---

### EP-005: `init.lua`, actions and key bindings

A user writes `~/.paneflow/init.lua`, defines named actions in Lua, binds keys to them, finds them in the command palette and runs them from the CLI. The file is loaded by the worker, so its handlers and timers keep running when the window is closed.

**Definition of Done:** an action defined in `init.lua` runs from a key, from the palette and from `paneflow do <name>` in a debug build; a syntax error in `init.lua` is shown in the app with its file and line and leaves the previous configuration active; every built-in shortcut and palette entry behaves as before with and without an `init.lua`.

**Rex reference:** pages "Custom Actions", "Rex Lua API" (key bindings, actions).

#### US-035: The worker hosts the configuration runtime
**Description:** As a user, I want Paneflow to load `init.lua` in the background process so that my configuration does not depend on the window.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-027, US-021

**Paneflow sites:** `crates/paneflow-serve/src/worker.rs:103-180,268-332`, `server.rs:38-46`; `crates/paneflow-home/src/lib.rs:76-118,429`.

**Acceptance Criteria:**
- [ ] Given `paneflow_home::init_lua_path()` and its `_in(home)` variant, when they are called, then they return `<home>/init.lua`, and the home layout test lists the file and the `lua/` directory
- [ ] Given a worker start, when `init.lua` exists, then it is executed on one dedicated thread that owns the Lua state and a single-threaded executor, and no other worker thread touches the state
- [ ] Given no `init.lua`, when the worker starts, then nothing is loaded, no warning is logged, and every existing worker behavior is unchanged
- [ ] Given `config.status`, when it is called, then it returns the path, whether a file was found, the load time, the count of actions, bindings, handlers and timers, and the list of load problems with file, line, severity and message
- [ ] Given an `init.lua` that raises at load, when the worker starts, then the worker still starts, serves all its other methods, and `config.status` reports the error
- [ ] Given the runtime, when it is created, then it has the 256 MiB memory cap of A7 and no load-time deadline other than the instruction hook used to stop it at shutdown
- [ ] Given `worker.shutdown`, when it arrives while Lua is running, then the runtime thread stops within the worker's existing drain window and the shutdown still completes
- [ ] Given a release build and a debug build, when each loads its file, then each reads only its own home (`~/.paneflow` or `~/.paneflow-dev`)

#### US-036: Define actions
**Description:** As a user, I want to declare a named action with a title and typed arguments so that it can be run from anywhere.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-035

**Rex reference:** `rex.action{ ... }` (page "Custom Actions", sections "Defining an Action", "Arguments").

**Acceptance Criteria:**
- [ ] Given `paneflow.action{ name, title, run }`, when it is called at load, then the action is registered; `description`, `category` (default `Actions`), `keywords`, `args` and `agents` (default `false`) are optional
- [ ] Given a `name` that is not lowercase letters, digits and underscores, or that contains a dot, when it is declared, then the load reports an error for that declaration and the rest of the file still loads
- [ ] Given `args = { file = "string", ratio = "number?" }`, when it is declared, then the types `string`, `number`, `integer`, `boolean`, `object`, `array` and `any` are accepted, a trailing `?` marks an optional argument, and a table with a `type` key is used as a JSON Schema as written
- [ ] Given the same `name` declared twice, when the file loads, then one warning is reported and the later declaration wins
- [ ] Given `paneflow.action` called after load (inside an action, a handler or a timer), when it runs, then it raises an error
- [ ] Given `action.list`, when it is called, then it returns each action's name, title, description, category, keywords, argument schema, `agents` flag and source (`init.lua:12`)
- [ ] Given `paneflow actions`, when it runs, then it prints the loaded actions, and `--json` prints the `action.list` result

#### US-037: Run actions
**Description:** As a user, I want to run an action with arguments and get its result so that actions are real commands.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-036, US-029, US-033

**Rex reference:** page "Custom Actions", sections "The Run Function", "Running an Action", "Calling Other Actions", "Trying an Action Before Reloading"; 30 s limit (L.884-887).

**Acceptance Criteria:**
- [ ] Given `action.invoke { name, args, ctx }`, when the action's `run(ctx, args)` returns, then the method returns its value; returning nothing, `nil` or `true` yields no result
- [ ] Given arguments that do not match the declared schema, when `action.invoke` is called, then it is refused before `run` starts with a message such as `no argument named "bogus"; the action takes file, ratio?`
- [ ] Given `run` raising an error, when it ends, then `action.invoke` returns an error carrying the message with file and line
- [ ] Given two actions invoked at the same time, when one waits on an API call, a command or a sleep, then the other runs meanwhile, and only one runs Lua at any instant
- [ ] Given an action still running 30 seconds after it started, not counting the time it spent waiting in a prompt (EP-006), when the limit is reached, then it is stopped with a `timeout` error; time spent in API calls, commands and sleeps counts
- [ ] Given `paneflow.invoke(name, args)` inside an action, when it is called, then the other action runs with the same `ctx`; nesting deeper than 16 returns `nil` and a message
- [ ] Given `ctx`, when `run` reads it, then it has `origin` (`key`, `palette`, `cli`, `api`, `script`, `agent`, `timer`, `event`) and the ids known at the call (`surface_id`, `pane_id`, `tab_id`, `workspace_id`, `client_id`), and API calls inside `run` default to them
- [ ] Given `paneflow do dev_layout file=app.log`, when it runs, then it calls `action.invoke`, prints the result as JSON and exits 0, or prints the error and exits 1
- [ ] Given `paneflow do ~/.paneflow/init.lua --action dev_layout file=app.log`, when it runs, then the file is loaded in the CLI process and the action runs from it without touching the worker's loaded configuration
- [ ] Given a script run with `paneflow do`, when it calls `paneflow.invoke` with a name it does not define, then the worker's action of that name runs

#### US-038: Reload, check, and always-on handlers
**Description:** As a user, I want my edits to apply when I save, a broken file to change nothing, and handlers that keep running so that the configuration is safe to iterate on.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-036, US-031

**Paneflow sites:** `crates/paneflow-config/src/watcher.rs:13-15`.

**Acceptance Criteria:**
- [ ] Given `init.lua` or a file it required being saved, when 300 ms pass with no further change, then the worker loads the file into a new state
- [ ] Given a reload that succeeds, when it completes, then the new actions, bindings, handlers and timers replace the old ones in one step, one `config.reloaded` event is appended, and actions still running on the old state run to completion
- [ ] Given a reload that fails, when it completes, then the previous configuration stays fully active, `config.status` reports the error, and one `config.reloaded` event with `ok = false` is appended
- [ ] Given `paneflow config reload`, when it runs, then it forces a reload and prints the status; given `paneflow config check [path]`, then the file is loaded in the CLI process, problems are printed with file and line, and the exit status is 1 when any error exists
- [ ] Given `paneflow.on(event, fn)` in `init.lua`, when a matching event arrives, then the handler runs in the worker, including while no window is open
- [ ] Given `paneflow.after` and `paneflow.every` in `init.lua`, when they are called at load or inside an action or handler, then they behave as in US-030 and are cancelled by the next successful reload
- [ ] Given a handler or a timer that raises, when it fails, then the error is logged, counted in `config.status`, and the handler or timer keeps running
- [ ] Given the file deleted, when the watcher fires, then the configuration is cleared as a successful reload of an empty file

#### US-039: Actions in the command palette
**Description:** As a user, I want my actions listed in the palette so that I can find and run them by title.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-037, US-038

**Paneflow sites:** `src-app/src/app/command_palette/mod.rs:55-60,135-320,472-518`, `catalog.rs:32-81,137-156,237,781`.

**Acceptance Criteria:**
- [ ] Given actions loaded in the worker, when the palette opens, then each appears under its category with its title, and its description, name and keywords match the filter
- [ ] Given a palette entry for an action, when it is chosen, then the palette closes and `action.invoke` is called from a background thread with `origin = "palette"` and the focused ids
- [ ] Given an action that fails, when its error returns, then one toast shows the action title and the first line of the message
- [ ] Given a `config.reloaded` event, when the palette opens next, then it shows the new action list
- [ ] Given the worker unreachable, when the palette opens, then it shows the built-in entries only, with no error and no delay on the render thread
- [ ] Given no `init.lua`, when the palette opens, then it is identical to today's, entry for entry
- [ ] Given an action with required arguments chosen from the palette, when it is invoked, then it runs with no arguments and the schema check of US-037 reports what is missing in the toast

#### US-040: `paneflow.bind` and the keymap the worker serves
**Description:** As a user, I want to declare key bindings in `init.lua` so that my actions are one keystroke away.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-036

**Rex reference:** `rex.bind(key, action[, args])`, `rex.unbind`, `rex.bindings` (page "Rex Lua API").

**Acceptance Criteria:**
- [ ] Given `paneflow.bind(key, name[, args])`, when it is called at load, then the binding is recorded with its source (`init.lua:14`), and `key` is kept as written for the window to parse with the syntax of `paneflow.json` shortcuts
- [ ] Given `keymap.list`, when it is called, then it returns each binding with key, action name, arguments, source, and whether the name is a Lua action of this file or a name left for the window to resolve as a built-in action
- [ ] Given a binding to a Lua action whose arguments do not match the action's schema, when the file loads, then one warning is reported and the binding is left out of `keymap.list`
- [ ] Given `paneflow.unbind(key)`, when it is called, then this file's bindings of that key are removed and the key is listed as unbound for the window to drop its default; `paneflow.bindings()` returns the bindings made so far
- [ ] Given `paneflow.bind(key, function)`, when it is called, then the load reports an error that says to define an action and bind its name
- [ ] Given `paneflow.bind` called after load, when it runs, then it raises an error

#### US-041: The window applies `init.lua` bindings
**Description:** As a user, I want the keys I bound in `init.lua` to work in the app, and the keys I set in Settings to keep winning, so that the two never surprise me.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-040

**Paneflow sites:** `src-app/src/keybindings/apply.rs:44-52,73-128`, `registry.rs:73-81,644,672-686`; `src-app/src/app/settings.rs:94,127-153,548-568`; `src-app/src/app/bootstrap.rs:31-35`; `crates/paneflow-config/src/schema/config.rs:13`.

**Acceptance Criteria:**
- [ ] Given an action type that carries an action name and arguments, when a test binds it to a key at runtime through `cx.bind_keys` and presses the key, then the handler receives the name and the arguments (validates A10; on failure the story is BLOCKED with the finding)
- [ ] Given the window, when it applies key bindings, then the order is defaults, then `init.lua` bindings, then `paneflow.json` shortcuts, so a key set in Settings always wins and Settings keeps showing what the key does
- [ ] Given a key bound in both files, when bindings are applied, then one warning names the key and both sources and is counted in the toast of US-043
- [ ] Given a key bound to a Lua action, when it is pressed, then `action.invoke` is called from a background thread with `origin = "key"` and the bound arguments, and the key is not sent to the terminal
- [ ] Given a binding whose name is an entry of the `ACTIONS` table, when the key is pressed, then that built-in action is dispatched as its default binding would
- [ ] Given a binding whose name is neither a Lua action nor a built-in action, or whose key does not parse, when bindings are applied, then one warning is reported and the binding is skipped
- [ ] Given a `config.reloaded` event, a saved shortcut or a keyboard layout change, when it happens, then the window re-applies bindings in the same order
- [ ] Given the worker unreachable at window start, when bindings are applied, then defaults and `paneflow.json` shortcuts apply at once, and `init.lua` bindings are added when the worker answers
- [ ] Given no `init.lua`, when the Settings shortcut list is opened, then it equals today's

#### US-042: Built-in actions from Lua
**Description:** As a script author, I want to trigger a built-in action such as zoom from Lua so that my actions can compose what the app already does.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-037, US-003

**Rex reference:** `rex.client.queue(name[, args])` (page "Rex Lua API").

**Acceptance Criteria:**
- [ ] Given `client.list_actions` (owned by the window), when it is called, then it returns the name, description and group of every entry of the `ACTIONS` table
- [ ] Given `client.dispatch_action { name }`, when it is called, then the window dispatches that action on the GPUI thread as the palette does, and replies once it is dispatched
- [ ] Given an unknown name, when `client.dispatch_action` is called, then it replies `-32602` and names the action
- [ ] Given `paneflow.client.queue(name)` inside an action, when `run` returns without error, then the queued actions are dispatched in order; when `run` fails, they are discarded
- [ ] Given `paneflow.client.queue(name)` in a `paneflow do` script, when it is called, then the action is dispatched immediately
- [ ] Given no window, when `paneflow.client.queue` is called, then it raises an error that says no window is attached

#### US-043: Show configuration problems in the app
**Description:** As a user, I want to see that my `init.lua` failed and where so that I do not wonder why a key does nothing.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-038, US-041

**Paneflow sites:** `src-app/src/app/notifications.rs:52-78,140-238`; `src-app/src/worker_bootstrap.rs:61-83`.

**Acceptance Criteria:**
- [ ] Given a `config.reloaded` event with `ok = false`, when the window receives it, then one toast shows `init.lua:<line>: <first line of the message>` with an action that opens the file in the dock's editor, at that line when the editor can reveal a line
- [ ] Given a load error present when the window starts, when the worker answers `config.status`, then the same toast is shown once
- [ ] Given warnings only (a duplicate action, a key conflict), when a reload completes, then one toast gives the count and the same open action, and nothing is shown when the count is 0
- [ ] Given the same error reported twice in a row, when the second arrives, then no second toast is queued
- [ ] Given a key bound to a Lua action pressed while the worker is unreachable, when it is pressed, then one toast says scripting is unavailable, shown at most once per outage
- [ ] The toast uses the existing toast primitive and adds no new permanent chrome

---

### EP-006: Prompts any client can answer

An action or a script stops, asks the user for a line of text or a choice, and continues with the answer, while keys, other actions and events keep working. The request is held by the worker and shown by every attached client that can show it; the first answer wins.

**Definition of Done:** the "new branch" action of US-047 runs in a debug build from a key, shows a prompt, and types the command with the answer; a test with two protocol clients proves that the first reply wins and the other receives `ended`; a prompt asked with no window attached returns `no_client`.

**Rex reference:** page "Custom UI"; prompt protocol (L.1154-1157, L.7868-7870).

#### US-044: The prompt broker
**Description:** As the maintainer, I want one place that holds pending questions and delivers the first answer so that any asker and any answering client can meet.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-021

**Paneflow sites:** `crates/paneflow-host/src/agent_write.rs:22-28,73-132,183-366` (the pattern, not modified); `crates/paneflow-serve/src/server.rs:38-46,664-700`.

**Acceptance Criteria:**
- [ ] Given `prompt.request` with `kind` (`input` or `pick`), `title`, the fields of FR-10 and `wait_ms`, when an answer arrives within `wait_ms`, then it returns `{status: "answered", answer}`
- [ ] Given `wait_ms` elapsing with the prompt still open, when `prompt.request` returns, then it returns `{status: "pending", id}`, and `prompt.wait { id, wait_ms }` continues waiting on that id with the same three outcomes
- [ ] Given a prompt that ends without an answer, when the asker's call returns, then `status` is the reason: `cancelled`, `timeout`, `no_client`, `unsupported`, `busy` or `failed`
- [ ] Given `prompt.follow` with the kinds the client can show, when it is called, then the stream sends the pending prompts, then `requested` frames (id, spec, origin, remaining time) and `ended` frames (id, reason)
- [ ] Given `prompt.reply { id, answer }` or `prompt.reply { id, cancel: true }`, when it is the first for that id, then the asker receives the answer or `cancelled` and every follower receives `ended`; a second reply for the same id is refused with `not_pending`
- [ ] Given no follower able to show the kind, when a prompt is requested, then the status is `no_client` (no follower at all) or `unsupported` (followers exist, none shows this kind), and nothing is queued
- [ ] Given every follower disconnecting while a prompt is pending, when the last one leaves, then the asker receives `no_client`
- [ ] Given `timeout` seconds (default 300, at most 3,600) passing with no reply, when it expires, then the asker receives `timeout` and followers receive `ended`
- [ ] Given 16 prompts already pending, when another is requested, then its status is `busy`
- [ ] Given a `pick` request with more than 2,000 items, two items with the same `id`, a missing `title`, or `allow_other` together with `multiple`, when it is requested, then it is refused with `-32602` naming the field
- [ ] Given a worker shutdown, when prompts are pending, then each asker receives `failed` with a detail saying the worker stopped
- [ ] The broker holds its state in memory only, takes the current time as an input in tests, and adds no method to the host

#### US-045: The text prompt in the window
**Description:** As a user, I want a question from an action to appear as a text field in the palette so that I answer it where I already type commands.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-044

**Paneflow sites:** `src-app/src/ui_primitives/quick_pick.rs:13-103`; `src-app/src/widgets/text_input.rs`; `src-app/src/app/branch_prompt.rs:9-100`; `src-app/src/app/command_palette/mod.rs:73-92,388-398`.

**Acceptance Criteria:**
- [ ] Given a window attached, when it starts, then a background thread follows `prompt.follow` and reconnects as the write-approval follower does
- [ ] Given an `input` prompt, when it is next in the order of FR-11, then the palette shell opens with `title`, a field holding `value` with `placeholder`, and one row labelled `confirm`, or `Use "<typed text>"` when `confirm` is absent
- [ ] Given Enter, when it is pressed, then the typed text is sent unchanged (spaces and an empty string included) and the prompt closes
- [ ] Given Escape or a click outside, when it happens, then `cancel` is sent and the prompt closes
- [ ] Given an `ended` frame for the shown prompt, when it arrives, then the prompt closes without sending anything, and the next prompt in the order of FR-11 is shown
- [ ] Given the command palette already open, when a prompt arrives, then the prompt waits until the palette closes, and the palette is never replaced under the user's hands
- [ ] Given the prompt's origin, when it is shown, then one line under the title names it: the action's title, the script's file name, or the agent and its pane
- [ ] The prompt is built from the existing palette shell, row and text-input primitives and adds no new color or radius token

#### US-046: The choice prompt in the window
**Description:** As a user, I want to pick one or several items from a list so that an action can ask me which project, branch or file.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-045

**Rex reference:** page "Custom UI", sections "Asking for a Choice", "Icons", "Choosing Several Items", "Accepting Typed Text".

**Acceptance Criteria:**
- [ ] Given a `pick` prompt, when it is shown, then each item shows `title` (default `id`), `subtitle`, `icon` and `detail`, in the given order, and typing filters them with the palette's matching and highlighting
- [ ] Given an item that is a plain string, when it is shown, then its `id` and `title` are that string
- [ ] Given Enter on an item, when `multiple` is false, then that item's `id` is sent
- [ ] Given `multiple = true`, when the user toggles items and confirms, then the ids are sent in the order of `items`, items with `selected = true` start ticked, the count of ticked items shows beside `confirm`, and confirming with none ticked sends an empty list
- [ ] Given `allow_other = true`, when the user confirms text that matches no item, then `{ text = "<typed>" }` is sent
- [ ] Given `icon`, when it is one of `document`, `folder`, `branch`, `terminal`, `code`, `server`, `database`, `package`, `globe`, `link`, `person`, `tag`, `clock`, `star`, `gear`, `plus`, then the matching Paneflow icon is drawn; any other name draws no icon and logs one warning
- [ ] Given an empty `items`, when the prompt is shown, then the list shows the palette's empty state and Escape cancels
- [ ] Given 2,000 items, when the prompt opens and the user types, then filtering runs off the render thread as the file finder's does

#### US-047: `paneflow.ui.input` and `paneflow.ui.pick`
**Description:** As a script author, I want to ask a question from Lua and get the answer as a return value so that actions can be interactive.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-044, US-037, US-041, US-045

**Acceptance Criteria:**
- [ ] Given `paneflow.ui.input{ title, placeholder, value, confirm, timeout }`, when the user answers, then it returns the text; given `paneflow.ui.pick{ ..., items, multiple, allow_other }`, then it returns the id, the list of ids, or `{ text = ... }`
- [ ] Given no answer, when the call returns, then it returns `nil`, a reason among `cancelled`, `timeout`, `no_client`, `unsupported`, `busy`, `failed`, and a detail string only for `failed`
- [ ] Given a mistake in the call (missing `title`, duplicate ids), when it is made, then it raises an error
- [ ] Given a prompt open from one action, when another action is invoked or a timer comes due, then it runs, and the waiting action's 30 s limit does not advance
- [ ] Given a call made under `pcall`, inside a function called by the action, inside a handler, a timer or a `paneflow do` script, when it runs, then it works
- [ ] Given a call made at the top level of `init.lua` during load, when it runs, then it returns `nil` and `failed` without asking anyone
- [ ] Given a call made inside a `table.sort` comparator or a coroutine the script created, when it runs, then it raises the error of US-033
- [ ] Given this action bound to a key, when the key is pressed and "fix/login" is typed, then `git switch -c fix/login` followed by Enter is sent to the focused terminal: the action calls `paneflow.ui.input{ title = "New branch", value = "feature/" }` and then `paneflow.surface.send_text{ text = "git switch -c " .. name, submit = true }`

#### US-048: Several clients, one answer
**Description:** As the maintainer, I want the multi-client behavior proven without a second app so that a future client can rely on it.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-044

**Acceptance Criteria:**
- [ ] Given `protocol/prompt-conformance-v1.json`, when it is read, then it lists the cases below, and a listed case with no runner fails the suite
- [ ] Given two followers and one prompt, when the first replies, then the asker gets the answer, the second receives `ended` with reason `answered`, and a late reply from the second is refused with `not_pending`
- [ ] Given two followers, when one cancels, then the asker gets `cancelled` and the other receives `ended`
- [ ] Given one follower that supports `input` only and a `pick` request, when it is requested, then the status is `unsupported`
- [ ] Given a follower that disconnects while another stays, when it leaves, then the prompt stays pending
- [ ] Given three prompts requested in order, when a follower connects afterwards, then it receives the three as pending, oldest first
- [ ] The runner uses two plain protocol clients against a real worker on a per-process endpoint and needs no display

#### US-049: Pending prompts in the attention queue
**Description:** As a user with many panes, I want a waiting question to show where I already look for things that need me so that I do not miss it.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-045

**Paneflow sites:** `src-app/src/app/attention.rs:94-137`; `src-app/src/app/write_approvals.rs:40-62`.

**Acceptance Criteria:**
- [ ] Given a pending prompt whose origin names a surface, when the attention items are computed, then one item is placed on that surface and its workspace, with its waiting time, as pending write approvals are
- [ ] Given that item, when the user activates it, then the prompt is shown, even when another prompt is older
- [ ] Given a prompt with no surface (a script outside a pane), when the items are computed, then no item is created and the prompt is shown directly
- [ ] Given the prompt ending for any reason, when the next computation runs, then the item is gone
- [ ] Given no pending prompt, when the attention items are computed, then the list equals today's

---

### EP-007: The layout authority moves to the worker

The layout model moves from the window to the worker. The window sends ops and draws the view it receives. The worker writes `session.json`. A layout method still needs an attached window to create a terminal; that last tie is cut in EP-008.

**Definition of Done:** the window writes `session.json` only on the fallback path of US-054 (guard test), a protocol client that applies an op while a window is attached sees the window draw it, the window keeps accepting every layout gesture while the worker is replaced and converges afterwards, and Arthur's visual pass over the US-020 checklist reports no difference.

**Rex reference:** revision and refetch (L.9015-9017); shared and per-client state (L.8539, L.7779, L.9038).

#### US-050: Measure before moving
**Description:** As the maintainer, I want the cost of a round trip and of a worker replacement measured so that the design's assumptions are numbers, not guesses.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-020

**Paneflow sites:** `scripts/perf-gates.sh`; `crates/paneflow-host/tests/persistent_baseline/gates.rs`; `crates/paneflow-serve/src/bootstrap.rs:240-330`.

**Acceptance Criteria:**
- [ ] Given a measurement that sends 1,000 no-op requests to a worker over its local endpoint, when it runs on the perf-gate runner class, then the median and the 95th percentile are written to a results file under `bench/results/`
- [ ] Given a worker replacement triggered as an update does, when it is measured 20 times, then the time during which the endpoint does not answer is recorded with its median and maximum
- [ ] Given the two measurements, when the story report is read, then it states them beside assumption A3 and says whether any op's acknowledgement could exceed one 60 Hz frame
- [ ] Given a failed or noisy run (two runs differing by more than 10 %), when it is detected, then the run is discarded and repeated, and the report says so
- [ ] No budget is added to `gates.rs` by this story; a budget is proposed in the report for Arthur to accept

#### US-051: The worker hosts the model and writes the session file
**Description:** As the maintainer, I want the worker to own the layout document and persist it so that it outlives the window.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-050, US-021

**Paneflow sites:** `src-app/src/app/session.rs:22-28,1196-1216`; `crates/paneflow-home/src/lib.rs:84`; `crates/paneflow-serve/src/worker.rs:103-180`.

**Acceptance Criteria:**
- [ ] Given a worker start, when `session.json` exists, then the worker loads it into a `paneflow_layout::Model` and serves `layout.view`, which returns the view and its `revision`
- [ ] Given `layout.attach`, when a client calls it, then it returns the view, the `revision` and the worker's `epoch`, and the client is recorded as attached until its connection closes
- [ ] Given `layout.apply { op }`, when the op succeeds, then the reply carries the new `revision` and the events, and the op's failure reasons are returned as typed errors
- [ ] Given no `layout.attach` and no accepted op since the worker started, when time passes, then the worker never writes `session.json`
- [ ] Given a window listening on the home's IPC endpoint that has not called `layout.attach` (a window of an earlier build during an update), when `layout.apply` is called, then it is refused with `window_owns_layout`, and the worker reloads `session.json` before its next accepted op
- [ ] Given a change, when 150 ms pass with no further change, then the worker writes `session.json` through a staged write and rename (debounce and write as in `src-app/src/app/session.rs:22,1196-1216`), and a clean shutdown flushes a pending write before the endpoint closes
- [ ] Given a write that fails (read-only file, full disk), when it fails, then the model stays authoritative in memory, the failure is logged once per cause and reported by `worker.status`
- [ ] Given a missing or unreadable `session.json`, when the worker starts, then it starts with an empty model and renames an unreadable file aside rather than overwriting it
- [ ] Given a worker killed without a shutdown, when another process tests `serve/owner.lock`, then the lock is seen as free (validates A9; if it is not, the story records the finding and the ownership rule of US-054 uses the worker endpoint's liveness instead)
- [ ] Until US-053 lands, no window calls `layout.attach`, so the window stays the only writer of `session.json` and its behavior is unchanged

#### US-052: Id leases
**Description:** As the maintainer, I want a client to allocate ids from a block the worker granted so that creating a pane never waits for a reply.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-051

**Acceptance Criteria:**
- [ ] Given `layout.lease_ids`, when it is called, then it returns a contiguous block of 1,024 ids and the worker's counter moves past the block before the reply is sent
- [ ] Given the counter, when it moves, then the new value is persisted with the next session write, and a worker that starts from a file never grants an id at or below a value it once granted
- [ ] Given an op that creates an element with a client-chosen id, when the id lies inside a block leased to that client and is unused, then the op is accepted; otherwise it fails with `invalid`
- [ ] Given a client that has used half of its block, when it asks again, then it receives a second block and may use both
- [ ] Given a worker crash between a lease and the next session write, when it restarts, then its counter is advanced by one full block beyond the persisted value before any id is granted

#### US-053: The window becomes a layout client
**Description:** As a user, I want every gesture of the US-020 checklist to give the same result while the layout now lives in the background process.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-052

**Paneflow sites:** `src-app/src/app/host_agents.rs:535-562` (the follow pattern); `src-app/src/app/session.rs:662-665`; `src-app/src/app/render.rs:384-386`.

**Acceptance Criteria:**
- [ ] Given a window start, when the worker answers, then the window calls `layout.attach`, takes an id block, projects the view, and reattaches each terminal by its session id as restore does today
- [ ] Given a user gesture, when `apply_layout_op` runs, then the op is applied to the window's own copy of the model at once, the projection runs in the same frame as today, and the op is sent to the worker from a background thread
- [ ] Given the worker's acknowledgement, when its `revision` equals the window's, then nothing is redrawn; when it differs, then the window fetches the view and reprojects
- [ ] Given a `layout.changed` event with a revision the window did not produce, when it arrives, then the window fetches the view and reprojects, reusing every pane whose id persists
- [ ] Given an op the worker refuses after the window applied it, when the refusal arrives, then the window adopts the worker's view and shows the toast that op shows today when refused
- [ ] Given a divider drag, when it is in progress, then no op is sent, and one set-ratio op is sent when it ends
- [ ] Given a new surface created by the window, when its terminal session is created, then the window sends a bind-session op, and until EP-008 the window is the only process that creates sessions
- [ ] Given `src-app/src`, when a guard test searches it, then the window has no write of `session.json` for structural state while it is attached to a worker
- [ ] Given the worker unreachable at window start, when the window opens, then it loads `session.json` itself as today and works as the layout authority (the handover back is US-054)
- [ ] The render thread performs no socket call

#### US-054: Keep working while the worker is away
**Description:** As a user, I want every layout gesture to keep working during an update or a worker failure so that the background process is never something I notice.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-053

**Paneflow sites:** `crates/paneflow-serve/src/bootstrap.rs:16-17,240-330`; `src-app/src/worker_bootstrap.rs:42-83`; `crates/paneflow-serve/src/worker.rs:103-111`.

**Acceptance Criteria:**
- [ ] Given the worker unreachable, when the user splits, closes, moves or resizes, then each op is applied to the window's model with leased ids and drawn as usual
- [ ] Given the worker reachable again, when the window reconnects, then it sends `layout.adopt` with its model and the last revision the worker acknowledged, and the worker accepts it when its own revision is not greater than that revision
- [ ] Given the worker's revision greater than the window's acknowledged revision, when `layout.adopt` is sent, then it is refused, the window adopts the worker's view, and one toast says the layout was refreshed
- [ ] Given the worker unreachable and `serve/owner.lock` not held, when the model changes, then the window writes `session.json` on the 150 ms debounce with the model's revision, and stops doing so as soon as a worker holds the lock
- [ ] Given a window that quits while the worker is unreachable, when it exits, then its last model is on disk
- [ ] Given an id block exhausted while the worker is unreachable, when a creating op is requested, then it fails with the toast used for a refused split
- [ ] Given a test that replaces the worker while ops are applied, when it ends, then the worker's view equals the window's model, asserted on the exported `SessionState`

#### US-055: Window-only state leaves the session file
**Description:** As the maintainer, I want state that belongs to one window stored by the window so that the worker's file has one writer.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-053

**Paneflow sites:** `crates/paneflow-config/src/schema/session.rs:63-79,200-222`; `src-app/src/window_state.rs:28-43`; `src-app/src/main.rs:152-154`.

**Acceptance Criteria:**
- [ ] Given the dock state and the detached pane placements (window and bounds, keyed by pane id), when they change, then the window writes them to `window-state.json`, and the worker never writes them
- [ ] Given a `session.json` written by the previous release that holds dock and detached fields, when the new window starts, then it reads them once, moves them to `window-state.json`, and the next session write omits them
- [ ] Given the previous release opening files written by this build, when it starts, then the layout restores and the dock opens in its default state, and the changelog entry of this story says so
- [ ] Given a pane id in `window-state.json` that no longer exists in the view, when the window starts, then the entry is dropped silently
- [ ] Given a test that lists every field of `SessionState`, when it runs, then each field is assigned to the worker's file or to `window-state.json` as FR-12 says, and a field added later without an assignment fails the test

#### US-056: Effects and structural methods move to the worker
**Description:** As a script author, I want layout methods answered by the process that owns the layout so that a change made from the CLI and one made from the keyboard follow the same path.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-053

**Paneflow sites:** `src-app/src/app/hosted_sessions.rs:222-262`; `src-app/src/app/workspace_ops/tab.rs:551-565`; `src-app/src/app/ipc_handler/split.rs`, `workspace_methods.rs`, `surface_methods.rs:1034-1087`.

**Acceptance Criteria:**
- [ ] Given hold, stop and rehome effects, when an op requests them, then the worker performs them through the host's existing `session.stop` and `session.workspace` methods, and the 5 s hold of undo close is timed by the worker
- [ ] Given `surface.split`, `surface.focus`, `surface.rename`, `workspace.create`, `workspace.up`, `workspace.select`, `workspace.close` and `workspace.restore_layout`, when the registry is listed, then their owner is `worker`, and their parameters and results are unchanged
- [ ] Given a raw JSON-RPC client that sends one of these methods to the window's socket, when the window receives it, then the window answers with the same shape as before by submitting ops, and the US-001 cases pass unchanged
- [ ] Given one of these methods sent to the worker while a window is attached, when it creates a pane, then the attached window creates the terminal session and binds it, and the reply waits for the binding as the `Async` reply does today
- [ ] Given one of these methods that must create a terminal while no window is attached, when it is sent to the worker, then it is refused with the existing "no Paneflow window is attached" error
- [ ] Given the read methods that need terminal content (`surface.read`, `surface.search`), when the registry is listed, then their owner is unchanged
- [ ] The host crate is not modified

#### US-057: Events, actions and documents follow the new owner
**Description:** As the maintainer, I want the temporary plumbing removed and the architecture document rewritten so that the code has one story.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-056, US-037, US-024

**Acceptance Criteria:**
- [ ] Given a model op applied in the worker, when it returns events, then the worker appends them to the log itself, and `events.publish` and the window-side queue of US-024 are deleted
- [ ] Given an action in `init.lua` that calls a structural method, when it runs, then the call is served inside the worker with no socket round trip
- [ ] Given `ARCHITECTURE.md`, when it is read, then the worker section, the session persistence section and the thread model describe the worker as the layout authority, the id leases, the adopt rule and the fallback write, and no sentence describes the window as the owner of the layout
- [ ] Given `CHANGELOG.md`, when it is read, then it has an entry for the file split of US-055 and for ids that are now stable
- [ ] Given the legacy `events.subscribe` on the window's socket, when it is used, then its frames and types are unchanged
- [ ] Given a worker and a window of different builds during an update, when the window attaches, then the capability set decides whether the window uses the layout methods, and a window that finds them missing falls back to the local authority of US-054

---

### EP-008: Layout with no window

The worker creates terminal sessions itself. The CLI, `init.lua` and scripts can build and change a layout while no window is open, and a window opened afterwards draws what it finds. This epic is P1: the PRD ships without it.

**Definition of Done:** with no window running, `paneflow up` followed by `paneflow split` and `paneflow ls` succeed against a debug home and their terminals run; opening the window then shows that layout with live terminals; the quit dialog's two choices behave as before.

**Rex reference:** the server creates blocks (`session.new_split`, `session.new_window`, page "Rex Lua API").

#### US-058: Inventory session launch
**Description:** As the maintainer, I want to know exactly what starting a terminal needs from the window so that the extraction is planned on facts.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-057

**Paneflow sites:** `src-app/src/terminal/pty_session/spawn_env.rs`; `src-app/src/terminal/host_link.rs`; `src-app/src/agent_launcher.rs`; `src-app/src/app/ipc_handler/split.rs:37-46`; `crates/paneflow-host/src/env.rs:31-43`.

**Acceptance Criteria:**
- [ ] Given the section "Session launch inventory" added to this PRD by this story, when it is read, then it lists every input of a session creation (shell, working directory, environment, each `PANE_CONTEXT_ENV` variable, shell integration, launch profile, agent command, prompt, initial size, managed worktree) with where it is computed today, `file:line`
- [ ] Given each input, when the section is read, then it says whether it depends on GPUI, on window state, on configuration, or on nothing
- [ ] Given an input that depends on window state with no equivalent in the worker, when the section is read, then it proposes where the value comes from in a headless start and names the user-visible difference, if any
- [ ] Given the section, when US-059 starts, then its scope is the list of functions the section names, and anything the section marks as blocking makes US-059 BLOCKED rather than improvised
- [ ] No production code changes in this story

#### US-059: Extract session launch into a shared crate
**Description:** As the maintainer, I want session launch in a crate the worker can link so that both processes start terminals the same way.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-058

**Acceptance Criteria:**
- [ ] Given the new crate `crates/paneflow-launch`, when its dependencies are listed, then it depends on neither `gpui` nor `src-app`
- [ ] Given the window, when it creates a terminal session, then it builds the request through `paneflow-launch`, and the request sent to the host is byte-identical to the one built before the story for the same inputs (asserted on a set of recorded cases: shell, agent with prompt, managed worktree, custom environment)
- [ ] Given an input the worker cannot know (the pane's size in cells), when the request is built without it, then the documented default is used
- [ ] Given an environment key that the host strips or refuses, when it is passed, then the behavior is unchanged
- [ ] The existing tests of `spawn_env.rs` and `host_link.rs` pass, moved with the code they test

#### US-060: The worker starts terminals
**Description:** As a script author, I want a pane created by a script to get its terminal even when no window is open.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-059

**Acceptance Criteria:**
- [ ] Given a surface with no session after an op, when the worker handles the op's spawn effect, then the worker creates the session through the host's existing `session.create` and applies the bind-session op
- [ ] Given a window attached, when a pane is created from the keyboard, then the worker creates the session and the window only attaches to it, so exactly one process creates sessions
- [ ] Given the pane's size unknown at creation, when a window later attaches, then it sends the real size with the host's existing `session.resize`
- [ ] Given a `session.create` that fails, when the host refuses, then the surface stays in the layout with no session and shows the pane's existing launch-failure state, and the op's caller receives the host's message
- [ ] Given the host unreachable, when a creating op arrives, then it is refused with the host's connection error and the model is unchanged
- [ ] Given a pane created headless, when its environment is read inside the terminal, then `PANEFLOW_SURFACE_ID` equals its surface id
- [ ] The host crate is not modified

#### US-061: Layout methods answer with no window
**Description:** As a user, I want `paneflow up`, `new`, `split`, `select` and `focus` to work before I open the app so that I can prepare a workspace from a script.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-060

**Paneflow sites:** `src-app/src/cli/control_cmds.rs:25-64`, `up_cmd.rs:89-135`, `flow_cmd.rs:74-674`; `crates/paneflow-host/src/control.rs:532-535`.

**Acceptance Criteria:**
- [ ] Given a worker and a host running and no window, when `paneflow up`, `paneflow new`, `paneflow split`, `paneflow select` and `paneflow focus` run, then each succeeds with the result shape it has with a window
- [ ] Given `paneflow ls` and `paneflow status` with no window, when they run, then they list the surfaces of the worker's layout with their ids
- [ ] Given `paneflow read` and `paneflow search` with no window, when they run, then they are served by the host as today
- [ ] Given a method that needs a window (`client.dispatch_action`, a prompt with no follower), when it is called with no window, then it fails with its existing "no window" reason
- [ ] Given no worker running, when a layout verb runs, then the CLI starts the worker as `paneflow sessions` does today, or fails with the existing message when it cannot
- [ ] Given `PANEFLOW_IPC_ORCHESTRATION` unset, when a headless method carries a command, a prompt or an environment, then it is refused as it is with a window

#### US-062: A window attaches to a layout it did not build
**Description:** As a user, I want to open the app and find the layout a script prepared, with its terminals running.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-060

**Paneflow sites:** `src-app/src/app/hosted_sessions.rs:68-89`; `src-app/src/app/session.rs:662-665`.

**Acceptance Criteria:**
- [ ] Given a layout created with no window, when the window starts, then it draws every workspace, tab and pane of the view and attaches each terminal with its scrollback
- [ ] Given a terminal started headless at the default size, when the window attaches, then it is resized to its pane before the first frame that shows it
- [ ] Given a session of the host that no surface of the layout names, when the orphan sweep runs, then it runs in the worker with the rules of `orphan_sweep`, and the window no longer runs it
- [ ] Given a surface whose session the host reports lost or exited, when the window attaches, then the pane restores as it does today (a fresh shell in its directory, or the agent's resume offer)
- [ ] Given two windows started against the same home, when the second starts, then the existing single-instance refusal applies

#### US-063: Quitting keeps its meaning
**Description:** As a user, I want the quit dialog's choices to do what they do today so that the background layout never surprises me.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-061, US-062

**Paneflow sites:** `ARCHITECTURE.md:852-866`; `src-app/src/app/quit_dialog.rs:496,527`.

**Acceptance Criteria:**
- [ ] Given "quit keeping sessions", when the window exits, then the worker keeps the layout, the terminals keep running, and reopening shows the same layout
- [ ] Given "stop everything and quit", when it is confirmed, then every session is stopped through the worker, the layout keeps its panes with no session, and the next start opens a fresh shell per pane as today
- [ ] Given a stop that cannot be confirmed, when it happens, then the window stays open with the existing Retry, Keep running and quit, and Cancel choices
- [ ] Given held sessions in the undo history, when the window quits, then they are released as today
- [ ] Given a quit with no live session, when the window exits, then the host is shut down as today and the worker's lifetime rules are unchanged
- [ ] `ARCHITECTURE.md`'s host lifetime table is updated where the actor changed and nowhere else

---

### EP-009: Agents ask and act

A coding agent in a pane can ask the user a question through Paneflow, have its own native questions and permission requests answered from Paneflow when the user opts in, and run the actions the user marked for agents. This epic is P1: the PRD ships without it.

**Definition of Done:** in a debug build, a Claude Code session calls the `ask_human` tool and receives the answer typed in the Paneflow prompt; with the opt-in on, a native `AskUserQuestion` is answered from Paneflow and falls back to the agent's own prompt when declined; an action without `agents = true` is invisible to `list_actions`; the helper size check passes.

**Rex reference:** agents use the CLI and a skill only (L.1363-1366); the public request under Mitchell Hashimoto's post of 2026-11-18 for agent-raised prompts.

#### US-064: Prove that a hook can answer
**Description:** As the maintainer, I want a live check that an agent accepts an answer supplied by a hook so that the hook work does not rest on a documentation reading.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a throwaway hook script outside the repository and an interactive Claude Code session in a pane of a tagged debug home, when the agent calls `AskUserQuestion` and the hook returns the `updatedInput` of R15 with an `answers` object, then the report states whether the agent proceeded with that answer and no native prompt appeared
- [ ] Given the same hook exiting 0 with no output, when the agent calls `AskUserQuestion`, then the report states whether the native prompt appeared unchanged
- [ ] Given a `PermissionRequest` hook returning `decision.behavior` `allow`, then `deny`, when a tool needs permission, then the report states the observed result for each, for Claude Code and for Codex
- [ ] Given the hook sleeping 120 s before answering, when the agent waits, then the report states whether the answer was still accepted and what the agent's UI showed meanwhile
- [ ] Given a throwaway build of `paneflow-ai-hook` that makes one extra request to the worker endpoint and prints the reply, when it is built under the `release-min` profile, then the report records its size beside the 384,000-byte cap (validates A11)
- [ ] Given the section "Hook answer validation" added to this PRD by this story, when it is read, then it holds the agent versions tested, each result, and a go or no-go for US-067 and for US-068, each of which becomes CANCELLED on a no-go
- [ ] No production code changes in this story

#### US-065: The `ask_human` tool
**Description:** As a coding agent, I want to ask the user a question and get the answer as a tool result so that I can continue with a decision that is theirs.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-048

**Paneflow sites:** `crates/paneflow-mcp/src/tools.rs:33-92,269-291,478-484`, `bridge.rs:191-214`, `main.rs:33-43`; `crates/paneflow-host/src/agent_write.rs:25-28,37-69,422-424`.

**Acceptance Criteria:**
- [ ] Given the MCP tool `ask_human` with `question`, an optional `options` list, `multiple`, and an optional `prompt_id`, when an agent calls it, then the bridge calls `prompt.request` on the worker with `wait_ms` of 20,000 and an origin built from `PANEFLOW_SESSION_ID` in the bridge's environment
- [ ] Given an answer within the wait, when the call returns, then the result is `{status: "answered", answer}`; otherwise it is `{status: "pending", prompt_id}`, and a later call with that `prompt_id` continues the same prompt
- [ ] Given a tool argument that names another session as the asker, when the tool is called, then it is ignored, as the forged-source test of `write_pane` requires
- [ ] Given the broker's outcome, when the tool returns, then `status` is one of `answered`, `pending`, `cancelled`, `timeout`, `no_window` (the broker's `no_client` and `unsupported`), `busy`, `rate_limited` or `failed`, and no other value
- [ ] Given no window attached, when the tool is called, then the result is `{status: "no_window"}` at once
- [ ] Given more than 3 requests in a burst or more than 1 per second after it from the same agent session, when the tool is called, then the worker refuses it before the broker is asked, the result is `{status: "rate_limited"}`, and nothing is shown
- [ ] Given the agent's session ending while its prompt is pending, when the worker observes it, then the prompt ends with reason `cancelled`
- [ ] Given a question longer than 16 KiB (the relay limit of `agent_write.rs:25`) or more than 2,000 options, when the tool is called, then it is refused with a message naming the limit
- [ ] Given `paneflow-mcp` built under the `release-min` profile, when its size is read, then it is under 524,288 bytes, and the bridge links neither `paneflow-api`, `schemars` nor `mlua`

#### US-066: Show who is asking
**Description:** As a user, I want a question from an agent to say which agent and which pane so that a manipulated agent cannot pass for Paneflow itself.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-065, US-049

**Acceptance Criteria:**
- [ ] Given a prompt whose origin is an agent, when it is shown, then the origin line reads the agent's tool name and its pane title, is drawn by Paneflow, and cannot be altered by the question's text
- [ ] Given the question text, when it is shown, then control characters and bidirectional override characters are stripped as the markdown viewer strips them, and it is drawn as plain text
- [ ] Given a prompt from an agent, when it is pending, then its attention item sits on the agent's pane (US-049) and activating it shows the prompt and focuses nothing else
- [ ] Given a prompt from an action or a script, when it is shown, then its origin line never uses the agent wording
- [ ] Given an agent prompt and a user-initiated prompt pending together, when the window picks one to show, then the user-initiated one is shown first

#### US-067: Answer an agent's own question from Paneflow
**Description:** As a user who opted in, I want the question Claude Code asks with its own tool to appear in Paneflow so that I can answer it from whichever pane I am in.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-064, US-066, US-046

**Paneflow sites:** `crates/paneflow-ai-hook/src/event.rs:154-186`, `runtime.rs:20-108`, `transport.rs:9-41`, `main.rs:5`; `crates/paneflow-mcp-install/src/integrations.rs:1077-1104`.

**Acceptance Criteria:**
- [ ] Given the configuration key `agent_prompts` in `paneflow.json` with values `native` (default) and `paneflow`, when it is absent, then every agent behaves exactly as today
- [ ] Given `agent_prompts = "paneflow"` and a `PreToolUse` hook call for `AskUserQuestion`, when the helper runs, then it first sends `agent.event` to the host as today, then calls the worker method `agent_prompt.resolve { event: "ask_user_question", agent, session, payload }`, where `payload` is the hook's input JSON
- [ ] Given `agent_prompt.resolve`, when it replies, then the reply is `{status: "answered", output}`, where `output` is the exact JSON for the helper to print, or `{status: "declined", reason}`; the worker holds one deadline of 300 s (the prompt default of R5) for the whole call, shared by all its questions and below the 600 s hook timeout (R15); when it elapses, the open prompt ends and the reply is `declined`
- [ ] Given the questions, when the worker receives them, then it requests one `pick` prompt per question, in order, with each option's label and description, `multiple` from `multiSelect`, `allow_other` on, and one extra item "Answer in the terminal"
- [ ] Given every question answered, when the worker replies, then `output` is the JSON of R15 (`permissionDecision: "allow"`, the original `questions`, and `answers` mapping each question text to the chosen label, labels joined by commas for a multiple choice), and the helper prints it unchanged and exits 0
- [ ] Given "Answer in the terminal", a cancel, a timeout, no window, the worker unreachable, or `agent_prompts = "native"`, when the helper returns, then it writes nothing to stdout and exits 0, so the agent shows its own prompt
- [ ] Given the worker unreachable, when the helper tries it, then it gives up within the connection deadline the helper already uses for the host (350 ms, `crates/paneflow-ai-hook/src/transport.rs:9`) and never delays the agent beyond that
- [ ] Given `paneflow-ai-hook` built under the `release-min` profile, when its size is read, then it is under 384,000 bytes
- [ ] Given Codex, when its hooks run, then no question is intercepted (R17 documents no such hook), and this is stated in the changelog entry

#### US-068: Answer a permission request from Paneflow
**Description:** As a user who opted in, I want an agent's permission request to appear in Paneflow so that I can allow or deny it without finding its pane.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-067

**Acceptance Criteria:**
- [ ] Given `agent_prompts = "paneflow"` and a `PermissionRequest` hook call, when the helper calls `agent_prompt.resolve` with `event: "permission_request"`, then the worker requests one `pick` prompt with the items "Allow once", "Deny" and "Decide in the terminal", titled with the tool name
- [ ] Given the tool's input, when the prompt is shown, then a summary of at most 400 characters is shown as plain text, with the stripping rule of US-066
- [ ] Given "Allow once" or "Deny", when the worker replies, then `output` is the `decision.behavior` JSON of R15 for Claude Code and of R17 for Codex, and the helper prints it unchanged and exits 0
- [ ] Given "Decide in the terminal", a cancel, a timeout or any failure, when the helper returns, then it writes nothing and exits 0
- [ ] Given any choice, when the helper returns, then it never writes a permission rule, an `updatedInput` or anything that allows a later call
- [ ] Given an agent whose pane is focused and visible, when it requests a permission, then the same prompt is shown (no visibility heuristic in this version), and this is listed in Open Questions

#### US-069: Actions as agent tools
**Description:** As a user, I want an agent to run the workflows I prepared, and only those, so that I delegate without handing over the whole API.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-037, US-065, US-066

**Acceptance Criteria:**
- [ ] Given the MCP tool `list_actions`, when an agent calls it, then it returns only the actions declared with `agents = true`, each with name, title, description and argument schema
- [ ] Given the MCP tool `run_action` with `name` and `args`, when the action is marked for agents and the arguments match its schema, then it runs with `ctx.origin = "agent"` and the agent's surface ids, and the tool returns its result
- [ ] Given an action not marked for agents, or an unknown name, when `run_action` is called, then both are refused with the same "no action named" message, so the tool does not reveal unmarked actions
- [ ] Given an action that asks a prompt while run by an agent, when the prompt is shown, then its origin line names the agent and the action
- [ ] Given `action.invoke` with `wait_ms`, when the action is still running after it, then the method returns `{status: "running", run_id}`, and `action.wait { run_id, wait_ms }` returns the result or the same status
- [ ] Given `run_action` still running after 20 s, when the wait elapses, then the tool returns `{status: "running", run_id}` and a later call with `run_id` returns the result or the same status
- [ ] Given the rate limit of US-065, when an agent exceeds it with `run_action`, then the result is `{status: "rate_limited"}`
- [ ] Given `PANEFLOW_MCP_SCOPE` and the existing four tools, when they are used, then their behavior is unchanged

#### US-070: Install and retire the new integrations
**Description:** As a user, I want `paneflow mcp install` and the automatic hook refresh to set up the new pieces and remove them cleanly so that my agent configuration stays correct.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-067, US-069

**Paneflow sites:** `crates/paneflow-mcp-install/src/integrations.rs:1077-1157`, `agents/claude_code.rs:174-195`; `crates/paneflow-serve/src/worker.rs:182-262`.

**Acceptance Criteria:**
- [ ] Given the Claude Code hook entries for `PreToolUse` on `AskUserQuestion` and for `PermissionRequest`, when the integration is written with `agent_prompts = "paneflow"`, then their `timeout` is 600; with `native` it stays 5
- [ ] Given `agent_prompts` changing in `paneflow.json`, when the worker observes the reload, then it rewrites the managed hook entries, and entries not marked as managed are untouched
- [ ] Given the Codex integration, when it is written, then only `PermissionRequest` follows the opt-in, with `command_windows` kept
- [ ] Given a settings file that is invalid JSON or read-only, when the integration is written, then it is left untouched and the existing failure report is used
- [ ] Given an agent configuration written by this build, when the previous release refreshes it, then it restores its own entries without error (the managed marker is unchanged)
- [ ] Given the installed MCP server entry, when it is read, then its per-server timeout is unchanged at 30,000 ms, because no tool call waits longer than 20 s

---

### EP-010: Windows validation and release readiness

Everything the earlier epics wrote for Windows by inspection is run on Arthur's Windows 11 machine, every defect is fixed, and the release records are written. The epic starts once every epic that will ship has reached IN_REVIEW. EP-008 and EP-009 are P1: a check about an epic that was cancelled is recorded as not applicable, and nothing here waits for it.

**Definition of Done:** every check of US-071 to US-073 has a recorded result on Windows 11 x64 with the build's commit, every defect they found is fixed and re-verified, the four CI legs are green on the final commit, and `CHANGELOG.md` and `ARCHITECTURE.md` describe the shipped state.

#### US-071: Windows: scripts, events and commands
**Description:** As Arthur, I want the scripting CLI verified on Windows hardware so that the first user-visible epic works on the three platforms.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005, US-031, US-032, US-033

**Acceptance Criteria:**
- [ ] Given a Windows 11 x64 build, when `paneflow api list`, `paneflow do -e "return paneflow.surface.list{}"` and a `paneflow do` script that registers `paneflow.on("pane.*", ...)` run, then each succeeds, and the report records the output
- [ ] Given that script following the worker's named pipe for 10 minutes, when panes are created, renamed and closed, then every event arrives with consecutive `seq` values (validates A5)
- [ ] Given `paneflow.exec{ "git", "status" }` and a program resolved through `PATHEXT`, when they run, then they return the output with `\r\n` normalized unless `text = false`
- [ ] Given `paneflow.exec` with a `timeout` on a program that starts a child, when the timeout expires, then the whole process tree is gone
- [ ] Given Ctrl-C on a `paneflow do` script in a busy loop, when it is pressed in Windows Terminal and in a Paneflow pane, then the script stops
- [ ] Given a failing check, when it is recorded, then it becomes an item of US-074 with its reproduction

#### US-072: Windows: configuration, keys and prompts
**Description:** As Arthur, I want `init.lua`, key bindings and prompts verified on Windows hardware.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-047, US-046, US-043

**Acceptance Criteria:**
- [ ] Given `%USERPROFILE%\.paneflow\init.lua` with an action, a binding and a prompt, when the app runs, then the action runs from its key, from the palette and from `paneflow do`, and the prompt returns the typed text
- [ ] Given a saved edit of `init.lua`, when 300 ms pass, then the reload applies, and a syntax error shows the toast with the file and line
- [ ] Given an app update that replaces the worker (the runtime copy under `serve\runtime\<build-id>\`), when it completes, then `init.lua` is loaded by the new worker and its bindings work without restarting the window
- [ ] Given a key bound in `init.lua` with a Windows-specific modifier spelling, when it is pressed, then it triggers, and the report lists the spellings tried
- [ ] Given the prompt UI, when it is shown at 100 %, 150 % and 200 % display scale, then the report attaches one screenshot per scale for Arthur's visual pass
- [ ] Given a failing check, when it is recorded, then it becomes an item of US-074

#### US-073: Windows: layout authority, headless and agents
**Description:** As Arthur, I want the moved layout, the headless mode and the agent integrations verified on Windows hardware.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-057

**Acceptance Criteria:**
- [ ] Given the US-020 checklist, when it is run on Windows, then every gesture gives its expected result
- [ ] Given the worker replaced during a series of splits and closes, when it returns, then the layout on screen and the worker's view are equal
- [ ] Given EP-008 shipped and no window, when `paneflow up` then `paneflow split` run and the window is opened, then the layout shows with live ConPTY terminals at the right size
- [ ] Given EP-009 shipped, and Claude Code and Codex in panes with the integrations installed, when `ask_human` is called and, with the opt-in on, a native question and a permission request are raised, then each is answered from Paneflow, and declining shows the agent's own prompt
- [ ] Given the helper binaries built for Windows, when their sizes are read, then the report records them beside the caps
- [ ] Given EP-008 or EP-009 cancelled, when the story ends, then its checks are recorded as not applicable with the cancellation named
- [ ] Given a failing check, when it is recorded, then it becomes an item of US-074

#### US-074: Fix what Windows found
**Description:** As Arthur, I want every defect found on Windows fixed and re-verified so that the feature ships on three platforms.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-071, US-072, US-073

**Acceptance Criteria:**
- [ ] Given the defects recorded by US-071, US-072 and US-073, when the story ends, then each is fixed with a test where one can be written, or listed as accepted with Arthur's decision
- [ ] Given each fix, when it is made, then the check that found it is re-run on Windows hardware and its new result is recorded
- [ ] Given a fix that touches shared code, when it lands, then the US-001 and US-048 conformance cases pass unchanged on Linux
- [ ] Given `cfg(windows)` items added to a file with a `mod tests`, when the file is read, then each is declared before that module
- [ ] Given no defect found, when the story ends, then it records that and changes no code

#### US-075: Release records
**Description:** As Arthur, I want the changelog, the architecture document and the generated reference to describe what shipped so that the release and the site documentation can be written from them.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-074

**Acceptance Criteria:**
- [ ] Given `CHANGELOG.md`, when it is read, then it has an entry for each of these that shipped: the `paneflow api`, `paneflow do`, `paneflow events`, `paneflow actions` and `paneflow config` verbs, for `init.lua`, for prompts, for stable ids, for headless layout, and for the three agent tools and the `agent_prompts` key
- [ ] Given `ARCHITECTURE.md`, when it is read, then it describes the registry, the three owners and client-side routing, the layout crate and its authority, the event log, the script runtime's two homes and the prompt broker, and its module list names the four new crates
- [ ] Given `protocol/api-v1.json`, `protocol/paneflow.d.lua`, `protocol/app-conformance-v1.json` and `protocol/prompt-conformance-v1.json`, when the suite runs on the final commit, then their drift tests pass
- [ ] Given `paneflow api docs`, when it is run on the final commit, then its output is attached to the story as the source for the site's reference page, and `docs/user/**` is not edited
- [ ] Given the CI run on the final commit, when it completes, then every required leg is green, with no test added to the nextest quarantine for this work
- [ ] Given a statement in either document that cannot be verified on the final commit, when it is found, then it is removed rather than left

## Layout Operation Inventory

Completed by US-007 on 2026-11-20 from the code at `a5d8a705`. Every path is under `src-app/src/`; `wo` is `app/workspace_ops`, `eh` is `app/event_handlers/mod.rs`, `ipc` is `app/ipc_handler`. "Hold" exists only where an undo record is pushed: every close goes through `perform_close` (`app/close_policy.rs:161-207`), and with `CloseIntent::Hold`, `hold_closed_sessions` keeps only the sessions named by an undo record and stops the others at once (`app/hosted_sessions.rs:222-241`). The grace is `CLOSED_SESSION_GRACE_MS = 5_000` and the history keeps `MAX_CLOSED_PANES = 5` records (`app/constants.rs:301-303`). The seed table of 2026-11-19 was wrong on four rows: closing a surface, closing a workspace, closing a pane from its menu or header, and IPC `restore_layout` all stop their sessions at once.

| Operation | Entry points | Mutator | Focus rule | Session effect | Zoom / limits |
|---|---|---|---|---|---|
| Split (key, palette, fork) | `wo/mod.rs:780-795` → `split` 693; palette `app/pane_palette.rs:439` through `PaneEvent::Split` eh:404-437; fork `app/conversation_restore.rs:393` | `split_with_target` wo/mod.rs:729 → `LayoutTree::split_at_pane` `layout/mutations.rs:77` | the new pane (wo/mod.rs:774) | spawn (757-759) | refused when zoomed (702, 741) and at MAX_PANES (709, 747) |
| Split (IPC `surface.split`, tmux) | `ipc/split.rs:306`; tmux `ipc/tmux_compat.rs:314` | `place_split` split.rs:223 | unchanged | spawn (404-413) | refused when zoomed (188), at MAX_PANES (196), at the tab cap for a new worktree tab (216-219) |
| Close focused pane | `ClosePane` wo/mod.rs:797 → `request_close` | `remove_focused_pane` wo/mod.rs:818 → `close_focused` `layout/close.rs:8` | previous sibling's last leaf, else the next sibling's first leaf (close.rs:75-83); a fresh pane when the tab empties (859-880) | hold the active surface + undo record; the pane's other surfaces stop | leaves zoom and removes the zoomed pane (832-843) |
| Close pane (menu, header, detached window) | `app/context_menu.rs:597-599`, `pane.rs:850,1357`, `app/detached_panes/window.rs:420` → `PaneEvent::CloseRequested` eh:157 | `remove_pane_from_layout` eh:525 | none set; a fresh pane when the tab empties (563-575) | stop at once (Hold intent, but no record) | prunes `saved_layout` or `root` (548-560); closing the zoomed pane ends zoom |
| Remove a given pane | `PaneEvent::Remove` eh:231 (last surface closed, `pane.rs:877-879`); tmux `ipc/tmux_compat.rs:368` | `remove_pane_from_layout` eh:525, Stop intent | same | stop at once, no record | same |
| Close surface | `PaneEvent::CloseSurfaceRequested` eh:160 (`pane.rs:854`); tmux 366 | `Pane::remove_surface` (`close_policy.rs:180` → `pane.rs:873`) | the active index shifts (`pane.rs:882-884`) | stop at once, no record | ignores zoom |
| Close tab | `CloseTab` wo/tab.rs:391; `app/context_menu.rs:446`; `app/keyboard.rs:370`; `app/tab_row.rs:206`; palette `app/pane_palette.rs:361`; bulk `app/selection.rs:837` | `remove_workspace_tab` wo/tab.rs:243 → `Workspace::close_tab` `workspace/mod.rs:378` | first pane of the new active tab (wo/tab.rs:266-268) | hold one surface per leaf + undo record; the rest stop | ignores zoom (the record reads `saved_layout` or `root`, wo/mod.rs:296) |
| Close workspace (UI) | `CloseWorkspace` wo/mod.rs:964; `app/context_menu.rs:428`; `macos_menu.rs:98`; bulk `app/selection.rs:850` | `remove_workspace` wo/mod.rs:1054 | first pane of the new active workspace, else the welcome screen (1084-1089) | stop at once + worktree teardown (1067-1069) | ignores zoom |
| Close workspace (IPC) | `workspace_close` `ipc/workspace_methods.rs:737` | `remove_workspace` through `close_without_prompt` (`close_policy.rs:122`) | none (no window) | stop at once | refuses the last workspace (745); unsaved files answer `confirmation_required` (751-753) |
| New tab | `NewTab` wo/tab.rs:106 → palette → `app/pane_palette.rs:432` | `open_tab_with_surface` wo/tab.rs:51 (fills an empty active tab at 82, else `Workspace::open_tab` `workspace/mod.rs:343`) | first pane through `focus_workspace_tab` (wo/tab.rs:100) | spawn | MAX_TABS_PER_WORKSPACE toast (88-90) |
| New surface in a pane | `PaneEvent::NewTab` eh:170, `NewTabPreset` eh:198; detached "+" `app/detached_panes/window.rs:423` | `Pane::push_surface` `pane.rs:824` | the pane (`pending_pane_focus`, eh:186, 227) | spawn | MAX_PANE_TABS = 8 (`pane.rs:301`, eh:172, 200); ignores zoom |
| New workspace (UI) | `NewWorkspace` wo/mod.rs:955, `window_chrome/title_bar_menus.rs:92`, `app/sidebar/mod.rs:424,584`, `app/welcome.rs:169`, `app/status_groups.rs:219`, `macos_menu.rs:92` → `create_workspace_with_picker` wo/mod.rs:649; folder drops `app/drop.rs:128`, `app/sidebar/mod.rs:910`; `app/welcome.rs:132`; `app/clone_bubble.rs:624` | `open_workspace_folders` wo/mod.rs:610 → `add_workspace` `app/spaces.rs:330` | `active_idx` set with no focus call (644); a folder already open goes through `activate_workspace_without_window` (640-642) | none (an empty tab, 632) | none |
| New workspace (IPC `workspace.create`) | `ipc/workspace_methods.rs:342` | `finish_workspace_create` 383, plus `apply_layout_from_json` when a layout is given | none | spawn | a layout holds at most MAX_PANES (`wo/layout.rs:90`) |
| New workspace (IPC `workspace.up`) | `ipc/workspace_methods.rs:321` | `finish_workspace_up` 441, `Workspace::restored_with_id` | the requested pane through `pending_pane_focus` (540-542) | spawn | MAX_PANES per batch (84); tabs above MAX_TABS_PER_WORKSPACE truncated silently (`workspace/mod.rs:275-281`) |
| Swap panes | `PaneEvent::DropPaneMove { edge: None }` eh:336 | `swap_panes` eh:368 (`layout/mutations.rs:121`) | the source pane (400) | none | no explicit check; the source is searched in `root` only (359-365), so it never matches a hidden pane while zoomed |
| Move pane to an edge | `PaneEvent::DropPaneMove { edge }` eh:369-395 | `remove_pane` then `split_pane_at_edge`, falling back to a first-leaf split (381-389) | the source pane | none | as swap; the pane count is unchanged |
| Move surface | `PaneEvent::DropSurfaceMove` eh:129 → `move_surface_to_pane` 441 | `push_surface` or `split_pane_at_edge` (497-519) | the destination | none | refused silently when zoomed (460) or detached (474); MAX_PANES (492); MAX_PANE_TABS (489); same tab only (466-473) |
| Drop an agent session | `PaneEvent::DropSessionSplit` eh:249 | `split_pane_at_edge` (316) or `open_pane_in_new_workspace_tab` (327) | the new pane (323, 330) | spawn with a resume command (298-306) | zoom refused only for an edge drop (269-277); MAX_PANES refused with no toast (278-285) |
| Move pane to a new tab | `app/drop.rs:87` | `move_pane_to_new_tab` wo/tab.rs:589 | first pane of the destination tab (711) | rehome (673, 696) | leaves zoom (628-635); tab cap (619-626); refused for the only pane of the same workspace (610-617) |
| Move tab to a workspace | `app/drop.rs:83` | `move_tab_to_workspace` wo/tab.rs:499 | `focus_workspace_tab` (546) | rehome (535-536, 551-565) | zoom moves with the tab; destination tab cap (520) |
| Reorder tabs | `app/drop.rs:81`, `app/keyboard.rs:225` | `reorder_workspace_tab` wo/tab.rs:475 → `Workspace::reorder_tab` `workspace/mod.rs:365` | unchanged | none | an index past the end is a no-op when `to > len`, and is clamped otherwise (`workspace/mod.rs:366-370`) |
| Reorder workspaces | `app/drop.rs:99`, `app/keyboard.rs:242` | `reorder_workspace` wo/mod.rs:1095 → `reorder_in_space` `app/spaces.rs:207` | unchanged | none | the move is confined to the workspace's space |
| Divider resize | `layout/render.rs:176-238` | ratio cells written in place (220-223); a save on drag end `app/render.rs:384-386` | unchanged | none | only the visible `root` |
| Equalize | `SplitEqualize` `wo/layout.rs:196` | `equalize_ratios` `layout/queries.rs:191` | unchanged | none | while zoomed it acts on the single zoomed leaf: a silent no-op |
| Presets | `wo/layout.rs:131-194` | `apply_layout_preset` 52 (`layout/presets.rs`); main vertical 157 | first leaf (73); the main pane (182) | none | leaves zoom (58, 163); ratios rebuilt |
| Zoom | `ToggleZoom` `wo/layout.rs:15` | tree swap (43-45); `Tab::exit_zoom` `workspace/tab.rs:191` | the zoomed pane | none | needs at least two leaves (34) |
| Detach and reattach | `ToggleDetachedPane` `app/detached_panes/mod.rs:47`; `app/context_menu.rs:596`; `PaneEvent::ToggleDetached` eh:138 | the `pane.detached` field (`detached_panes/window.rs:233-239`, `mod.rs:124`); the tree is untouched | the source window's first leaf (window.rs:240-245); reattach focuses that pane (mod.rs:138-155) | none | leaves zoom (`mod.rs:104`); MAX_DETACHED_PANE_WINDOWS (window.rs:47-51) |
| Undo close pane | `UndoClosePane` wo/mod.rs:886 (`app/render.rs:564`) | `split_at_focused`, else `split_first_leaf`, horizontal (934-941); focus is first moved to the tab's first docked pane by `activate_workspace_at` (914) | the restored pane (949) | reattach while held, else spawn (`surface_reopen` 181-187); a session from another worktree is stopped (922-926) | refused when zoomed (264) and at MAX_PANES (267); lands next to the first docked pane, not at its old position |
| Undo close tab | `UndoClosePane` when the newest record is a tab (894-896); toast action `app/notifications.rs:268` → wo/tab.rs:281 | `restore_closed_tab` wo/tab.rs:293 → `LayoutTree::from_layout_node` | `focus_workspace_tab` (361) | reattach or spawn; fresh spawns do not get the tab's worktree (330) | tab cap (310) |
| Rename tab | `begin_tab_rename` wo/tab.rs:405 (`app/context_menu.rs:444`, `app/keyboard.rs:366`, `app/tab_row.rs:289`) → `commit_rename` wo/mod.rs:1192 → `commit_tab_title` 1310; reset wo/tab.rs:426; process title wo/tab.rs:447 | `Tab::set_title` | unchanged | none | none |
| Rename surface (IPC) | `surface_rename` `ipc/surface_methods.rs:1040` | the terminal's `custom_name` (1055) | unchanged | none | none |
| Rename workspace | none: the only write to a workspace title is a repair during session load (`app/session.rs:1020`) | none | none | none | none |
| Restore layout | IPC only, `ipc/workspace_methods.rs:757` | closes the surplus panes (781-798), then `apply_layout_from_json` `wo/layout.rs:79` | none | stop the surplus at once; spawn the missing leaves | leaves zoom (`wo/layout.rs:95`); 1 to MAX_PANES leaves (775) |
| Templates | `app/context_menu.rs:411` → `wo/template_launch.rs:34`; Settings `app/settings/tabs/workspaces/mod.rs:403` | direct `root =` (`wo/template_launch.rs:140-141`) | `pending_pane_focus` (153-155) | spawn; the replaced panes are dropped with no close | refused when zoomed (90) and above MAX_PANES (93) |
| Focus in a direction | `wo/focus.rs:64-104` | none | the neighbor | none | zoomed: one leaf only |
| Select tab | `select_workspace_tab` wo/tab.rs:156; next and previous tab 117-154 | `Workspace::set_active_tab` | first pane (187) | none | none |
| Select workspace (UI) | `select_workspace` wo/mod.rs:487, `activate_workspace_at` 496 | `point_at_workspace` | first docked pane, or a given pane after `reveal_pane` (510-520) | none | `reveal_pane` leaves zoom when the pane is hidden (`workspace/tab.rs:203-213`) |
| Select workspace, focus (IPC) | `workspace_select` `ipc/workspace_methods.rs:721`; `surface_focus` `ipc/surface_methods.rs:1062` | `activate_workspace_without_window` wo/mod.rs:550; `reveal_surface` 371 | `workspace.select`: none; `surface.focus`: deferred pane focus (1078-1089) | none | `surface.focus` leaves zoom |

### Operations with several entry points

- **Split, not identical.** The key always uses the Normal profile and no command; the palette passes a preset's profile, environment and command; the IPC split never moves focus and refuses an anchor `surface_id` that is not its pane's active surface (defect b below).
- **Pane close, not identical.** The `ClosePane` key pushes an undo record and holds the session for 5 s. The context menu, the header button and the detached window's close push no record and stop the session at once.
- **Workspace close, not identical.** The UI opens the unsaved-files dialog and moves focus. IPC refuses the last workspace, answers `confirmation_required` instead of a dialog, and moves no focus.
- **New workspace, not identical.** The UI creates an empty, de-duplicated workspace with no session. `workspace.create` spawns a terminal and, given a layout in the same space, leaves the new workspace active (`ipc/workspace_methods.rs:418-424`); without a layout it does not select it (`app/spaces.rs:335-343`). `workspace.up` selects the new workspace without a window.
- **Workspace select, not identical.** The UI focuses a pane; IPC moves no focus.
- **Close tab, identical** across its six entry points: each ends in `remove_workspace_tab`.
- **Reorder tabs and reorder workspaces, identical** between drag and keyboard.

### Paths left untraced on 2026-11-19

- **IPC `surface.split`.** `prepare_split` (`ipc/split.rs:318`) resolves the anchor through `find_pane_by_surface_id` (287), or takes the active tab when there is no anchor. It checks first (347), resolves the working directory and worktree off the main thread (170-175), checks again in `finish_split` (398-402), and `place_split` then splits the anchor, splits the first leaf (245), or opens a new worktree tab that becomes active (250-255). The terminal is created before `place_split` (404-420); if placement then fails the view is dropped.
- **IPC `workspace.close`.** `ipc/workspace_methods.rs:737-755` → `close_without_prompt` (`close_policy.rs:122`) → `perform_close(Workspace, Hold)` → `remove_workspace` with no window. No record is pushed, so every session stops at once.
- **`create_workspace_with_picker`.** `wo/mod.rs:649-672` opens a folder prompt, then `open_workspace_folders` (610), which calls `path.is_dir()` on the main thread (618). This is a pre-existing breach of "the render thread never blocks", out of scope for this epic.
- **`open_pane_in_new_workspace_tab`.** `eh:35-60` calls `Workspace::open_tab(Tab::new("", Leaf))`, which replaces an empty shell tab (`workspace/mod.rs:344-348`) and shows a toast at the tab cap. Its callers are eh:327 and wo/tab.rs:680.
- **`split_pane_at_edge`.** `eh:19-33` is `split_at_pane` followed by a swap when the edge puts the new pane first (`pane_drag.rs:74-81`). It checks neither zoom nor limits; its callers (eh:316, 380, 509) do.
- **Templates.** `wo/templates.rs` only builds `LayoutNode` values (19-181). `wo/template_launch.rs:139-141` replaces the active tab's tree and clears `saved_layout`; the old panes get no `perform_close`, so their sessions are neither held nor stopped by this path.
- **`detached_panes/persistence.rs`.** The save keys each detached pane by position (workspace index, tab index, leaf index) over `saved_layout` or `root` (13-18, 62-85). The restore skips an entry whose leaf count changed (45-47) and calls `detach_pane` (55).

### Suspected defects

- **a. Undo of a pane holding several surfaces restores one: confirmed.** `capture_closed_surface_record` reads only `pane.surface()`, which is `surfaces[active_surface]` (`wo/mod.rs:199-205`, `pane.rs:790-792`); `ClosedPaneRecord` holds a single `surface` (`app/window.rs:217-222`), and undo rebuilds a one-surface pane (`wo/mod.rs:931`). Tab undo does the same, one surface per leaf (296-301, 326-334). The close still hands every terminal of the pane to `hold_closed_sessions` (`close_policy.rs:79`), which stops each one that no record names (`app/hosted_sessions.rs:237-241`): the other surfaces' sessions are stopped at close time. Kept for parity by this PRD (Non-Goals).
- **b. A non-active surface resolves to its terminal but not to its pane: confirmed.** `find_terminal_in_tree` scans every terminal (`ipc/surface_methods.rs:99-106`), while `find_pane_in_tree` checks only `active_terminal_opt()` (151-155). IPC split answers "Surface not found" (split.rs:287-288), `surface.focus` answers not found (1070-1072), and attention (`app/attention.rs:176`), fleet search (`app/fleet_search.rs:214`), the sessions sidebar (`app/sessions_sidebar.rs:582`) and jump-to-waiting (`wo/focus.rs:119`) skip the surface. `surface.read`, `rename` and `send` work because they resolve the terminal (795). US-019 removes the defect.

### Direct assignments to a tab's `root` or `saved_layout`

31 in production code: 11 `take()` calls (eh:372, 514, 549, 557; `wo/focus.rs:15`; `wo/layout.rs:43`, 104-105; `wo/mod.rs:836`, 845; `wo/tab.rs:639`; `workspace/tab.rs:193`) and 20 assignments (eh:392, 515 through `*root`, 552, 554, 559, 573; `wo/focus.rs:20`; `wo/layout.rs:44`, 45, 124; `wo/mod.rs:839`, 848, 875, 940; `wo/tab.rs:82`, 646, 662; `wo/template_launch.rs:140`, 141; `workspace/tab.rs:194`). Seven more mutate a tree in place through `&mut` (eh:314, 355/368, 506; `ipc/split.rs:237`; `wo/mod.rs:761`, 934; `wo/tab.rs:684`), and `workspace/mod.rs:345` replaces a whole tab. `save_session` has 68 call sites in 25 files (defined at `app/session.rs:103`).

### Operations that a test cannot drive without a display

A pointer drag (pane, surface or agent session drop, divider drag) and a detached window need a real window and pointer. Their model ops are pinned through the event the drop emits (`PaneEvent::DropPaneMove`, `DropSurfaceMove`, `DropSessionSplit`) and through the model's own tests; the gesture itself is on the US-020 checklist.

## Functional Requirements

- FR-01: The system must declare each method once, with typed parameters, a typed result, a summary, an example, an owner and a gate, and must derive dispatch, validation, `system.capabilities` and `system.describe` from that declaration.
- FR-02: The system must NOT change the name, parameters, result fields, error codes or gates of any method, event frame, CLI verb or MCP tool that exists at `31d8ce34`.
- FR-03: The system must send each call to the process that owns the method, and no process may relay a request to another process on a client's behalf.
- FR-04: The system must hold workspaces, tabs, split trees, panes and surfaces in one plain-data model that changes only through `apply(op)`, with `u64` ids that are never reused.
- FR-05: When a client applies a layout op, the system must draw it without waiting for another process, and must converge on the authority's view.
- FR-06: The system must keep a numbered event log in the worker and deliver it to any follower with consecutive `seq` values, replay from `since`, and an explicit `evicted` error.
- FR-07: The event log must carry these types: `workspace.created`, `workspace.closed`, `workspace.renamed`, `workspace.selected`, `tab.created`, `tab.closed`, `tab.renamed`, `tab.selected`, `pane.created`, `pane.closed`, `pane.focused`, `surface.created`, `surface.closed`, `surface.renamed`, `layout.changed`, `layout.resync`, `terminal.cwd_changed`, `terminal.title_changed`, `terminal.bell`, `terminal.exited`, `agent.state_changed`, `config.reloaded`. The script runtime adds one local event, `events.resync`, delivered to handlers when its stream had to restart from the current `seq`.
- FR-08: The system must run Lua 5.4 scripts in the CLI process with `paneflow do`, and `<home>/init.lua` in the worker, with the same `paneflow` table in both.
- FR-09: The `paneflow` table must provide: `call`, one function per registry method under its namespace, `action`, `invoke`, `client.queue`, `bind`, `unbind`, `bindings`, `on`, `wait`, `sleep`, `stop`, `after`, `every`, `exec`, `exec.start`, `ui.input`, `ui.pick`, `json.encode`, `json.decode`, `log`, `layout.pane`, `layout.horizontal`, `layout.vertical`, `args` and `ctx`.
- FR-10: A prompt must carry `kind`, `title`, and optionally `placeholder`, `value`, `confirm`, `timeout` (default 300 s, at most 3,600 s), and for a choice `items` (at most 2,000, each with `id`, `title`, `subtitle`, `icon`, `detail`, `selected`), `multiple` and `allow_other`.
- FR-11: When several clients can show a prompt, the system must deliver the first answer to the asker and close the prompt on the others. A client shows one prompt at a time, in this order: a prompt the user activated from the attention queue, then prompts raised by the user's own action or script, oldest first, then prompts raised by an agent, oldest first.
- FR-12: Shared state held by the layout authority: split trees, ratios, tab order, workspace order, focused pane per tab, active tab, active workspace, zoom, titles, custom names, space, worktree link, surface definitions and session bindings, undo records. Window-only state: divider drag in progress, scroll position, selection, search, sidebars and panel widths, the palette, the dock (git, files, editor), detached window placement and bounds.
- FR-13: The system must NOT change the host's protocol version or method set.
- FR-14: When `init.lua` fails to load, the system must keep the previous configuration active and must show the file and line in the app.
- FR-15: A prompt or an action run raised by an agent must show the agent and its pane, must be rate limited to 1 per second with a burst of 3 per agent session, and must reach only actions declared with `agents = true`.
- FR-16: When `agent_prompts` is `native` or absent, the hook helper must write nothing to stdout and must exit 0, as it does today.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected behavior | User message |
|---|----------|---------|-------------------|--------------|
| 1 | No `init.lua` | First run, or the file was deleted | Nothing is loaded, the app equals today's | None |
| 2 | `init.lua` syntax error | Saved file does not parse | Previous configuration stays active | Toast: `init.lua:<line>: <message>`, with an open action |
| 3 | Worker replaced during an update | Version, protocol or build id differs | Layout gestures keep working locally, built-in keys work, Lua keys show one toast, state converges on return | "Scripting is unavailable" at most once per outage |
| 4 | Worker fails to start | `WorkerBoot::Failed` | The window is the layout authority and writes `session.json`; the existing banner and retry apply | The existing worker banner |
| 5 | Another client changed the layout | A script applies an op while the user drags | The window refetches the view on the next revision it did not produce; the drag in progress commits afterwards | None |
| 6 | Prompt with no window | A headless script calls `paneflow.ui.input` | Returns `nil`, `no_client` | None |
| 7 | Prompt while the palette is open | An action asks during a palette session | The prompt waits for the palette to close | None |
| 8 | Runaway action | `while true do end` in `run` | Stopped at the 30 s limit; other actions keep running | Toast with the action title and "timeout" |
| 9 | Async call in a `table.sort` comparator | A script sorts with a comparator that calls the API | Error naming the limitation | The Lua error |
| 10 | Slow event follower | A script blocks in a handler for minutes | Disconnected at 8 MiB of backlog; on reconnect it gets `evicted` and an `events.resync` | Printed by `paneflow do` |
| 11 | Stale surface id | A script kept an id of a closed pane | The method replies not found; the id is never reused for another pane | The existing not-found error |
| 12 | Key bound in both files | `init.lua` and Settings bind the same key | Settings wins | Warning toast with the count, details in `config.status` |
| 13 | Agent floods prompts | A looping agent calls `ask_human` repeatedly | Rate limited after a burst of 3 | None shown; the tool returns `rate_limited` |
| 14 | Agent question, user away | No answer within the prompt timeout | The hook writes nothing; the agent shows its own prompt | None |
| 15 | Old host running | A host older than the app | Unaffected: nothing in this work calls a new host method | None |
| 16 | Downgrade | The previous release opens files written by this build | Layout restores; the dock opens in its default state | Changelog entry |
| 17 | Headless pane, later window | A pane was created with no window | Resized to its pane before its first visible frame | None |
| 18 | Session creation fails | The host refuses `session.create` | The surface stays with the pane's existing launch-failure state | The host's message |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|-------------|--------|------------|
| 1 | The layout refactor changes a behavior nobody listed | High | High | US-008 and US-009 pin behavior in tests that outlive the refactor before any model code; US-020 gives Arthur a gesture checklist; the model is property-tested |
| 2 | The projection recreates terminal views and loses scrollback or selection | Med | High | Reuse by id is an explicit criterion (US-015); the pane-reuse path of `layout/serde.rs:97-106` is the starting point |
| 3 | Two writers corrupt `session.json` during the transition | Med | High | One ownership rule keyed on `serve/owner.lock` (US-051, US-054), tested with both processes present |
| 4 | mlua does not build on a target or fails `cargo deny` | Low | High | US-026 runs first in its epic and blocks it; Luau through the same crate is the recorded fallback (D5) |
| 5 | The worker becomes a single point of failure for keys and layout | Med | High | Built-in keys never go through it; the window is a full fallback authority (US-054); Lua keys degrade with one toast |
| 6 | A hook that waits for a human blocks the agent | Med | Med | Opt-in only; fail open to the native prompt on every failure path (US-067); live validation first (US-064) |
| 7 | A manipulated agent uses prompts to phish the user | Med | High | Provenance drawn by Paneflow, text stripped and plain, rate limit, user prompts shown first (US-066); no prompt can grant a standing permission (US-068) |
| 8 | The method migration alters a wire shape | Med | High | Conformance file written before the migration (US-001) and required unchanged by every later story that touches a method |
| 9 | Session launch cannot leave the window crate cleanly | Med | Med | Inventory first (US-058), which may block US-059; EP-008 is P1 and nothing after it in EP-009 depends on it |
| 10 | The scope is too large to land as one stream | High | Med | Each epic delivers on its own and leaves the app shippable; EP-004 gives user value after three epics; EP-008 and EP-009 are P1 |
| 11 | Scripts written against this version break when the API changes | Med | Med | `protocol/api-v1.json` and its drift test make every API change explicit in review |
| 12 | Windows defects pile up in the last epic | Med | Med | Every story writes and reviews its Windows path; the worker endpoint already streams on Windows (P4); US-074 is sized for a batch |

## Non-Goals

- **A second window or a remote client.** The protocol allows several clients and the broker is tested with two, but the single-instance refusal (`src-app/src/ipc.rs:143-163`) stays and no mobile, web or remote client is built.
- **A sandbox for scripts.** A script has the user's rights, like a shell script. Lua 5.4 has no sandbox mode in mlua (R12), and the existing boundary already excludes a hostile process of the same user.
- **Functions bound directly to keys** (`rex.bind(key, fn)`), **key modes and key sequences declared from Lua.** GPUI resolves keys through its keymap; binding to a named action covers the need (Rex itself redirects to named actions for prompts, L.1126-1128).
- **Any change to the host.** No new host method, event or protocol version.
- **A home-made Lua interpreter, or a second scripting language.**
- **Removing or changing the legacy surfaces:** `events.subscribe`, `paneflow watch`, `paneflow wait`, flow files, workspace TOML files and the tmux compatibility layer stay as they are.
- **Output-change events in the new log.** `surface_changed` stays on the window's socket; the worker does not see terminal output.
- **Moving the dock (git, files, editor) or the diff view into the layout model.** They read the local disk and stay window state.
- **Arbitrating terminal size between several clients.** With one window, the last resize wins as today (`crates/paneflow-host/src/host.rs:2318-2325`).
- **Answering Codex questions through a hook, or answering MCP elicitation.** No documented Codex hook answers a question (R17).
- **Editing `docs/user/**`.** It is a mirror generated from the site repository; US-075 produces the source material.
- **A marketplace, a package manager or a plugin format for scripts.**
- **Settings in Lua.** `init.lua` declares actions, key bindings, handlers and timers. Theme, fonts, shell and every other setting stay in `paneflow.json` and the Settings screen.
- **Changing how undo restores a pane that held several surfaces.** If US-007 confirms that only one surface comes back, the model keeps that behavior. The other suspected defect (a non-active surface that does not resolve to its pane) disappears with US-019, where every lookup goes through the model.

## Files NOT to Modify

- `crates/paneflow-host/src/**`: the host protocol and behavior are frozen for this work (FR-13).
- `protocol/host-conformance-v1.json`: the host's contract.
- `crates/paneflow-host/tests/persistent_baseline/gates.rs`: no existing budget is raised; a new budget needs Arthur's approval (US-050 only proposes one).
- `native/libghostty/**`: engine archives and generated bindings.
- `vendor/**`: vendored crates keep their upstream text.
- `crates/paneflow-shim/**`: size-capped helper with no part in this work.
- `src-app/src/terminal/pty_session/session_backend.rs` and `src-app/src/terminal/types.rs`: the engine boundary and its neutral types.
- `src-app/src/git_command.rs`: the single git boundary.
- `docs/user/**`: generated mirror of the site's documentation.
- `rust-toolchain.toml`: the toolchain stays at 1.98.0.
- `.config/nextest.toml` quarantine list: no test of this work is quarantined.

## Technical Decisions

Each decision names the options considered and the evidence for the choice.

- **D1. The first work is the registry, not Lua.** Options: bind Lua to the existing handlers; or declare methods once and derive. Chosen: derive. A method exists in four hand-kept copies today (Problem Statement, point 1) and Rex's own CLI and Lua API derive from descriptors (R2). The JSON-RPC transport stays: the CLI, the MCP bridge, the hook helper, the host and the worker all need it.
- **D2. Schemas come from `schemars` 1.2.2.** Options: hand-written schemas; `schemars`. Chosen: `schemars`, already in the lockfile through GPUI (R14), so no new source or license enters the tree.
- **D3. Clients route by owner; processes do not forward.** Options: a single front door that proxies; client-side routing. Chosen: client-side routing. The CLI already chooses between the window and the host (P1), the worker's rule "forwards nothing to the core" stays true, and no request gains a hop. The cost is that a raw JSON-RPC user must address the right endpoint, which `system.describe` states for every method.
- **D4. New subsystems live in the worker, and the host does not change.** Options: host, worker, or window. Chosen: worker. An incompatible host is left running until the user stops every terminal (`ARCHITECTURE.md:863`) and compatibility is an exact protocol match (`crates/paneflow-host/src/protocol.rs:135-146`), so logic placed in the host freezes at its version and a protocol change costs users their sessions. The worker is replaced on every update (P2), owns no terminal, outlives the window, and already streams to the window on every platform (P4), which removes the Windows limits of the window's own socket. This reverses the earlier idea of generalizing the host's approval broker: its design is reused (P5), its code is not touched.
- **D5. Lua 5.4.9 through mlua 0.12.2, features `lua54`, `vendored`, `async`, `serialize`.** Options: Lua 5.1 (what Rex runs), Lua 5.4, Lua 5.5, Luau, piccolo, a home-made interpreter. Chosen: 5.4. The probe (R13) shows 5.4 lifts two of the limits Rex documents for prompts (R6). Luau removes `io` and `package` and fails the metamethod case. piccolo's last release is from June 2024 and calls itself experimental. A home-made interpreter repeats the withdrawn second git engine (P12). Lua 5.5 passes the same probe and is one feature flag away; it is left as an open question because 5.4 is the version scripts, tools and editor support are written for today. Fallback if US-026 fails on a target: Luau through the same crate.
- **D6. Scripts in the CLI process, configuration in the worker.** Options: everything in the window; everything in the worker; Rex's split. Chosen: Rex's split (R1), with the worker in place of Rex's server. `init.lua` handlers and timers then survive the window, which Rex cannot offer in its configuration file (R8). The Lua state stays on one thread (no `send` feature) with a single-threaded executor, because the worker has no async runtime (P2).
- **D7. Every layout op is applied locally first, with leased ids.** Options: wait for the authority's reply (Rex's client does, R9); predict non-creating ops only; predict everything. Chosen: predict everything. The window links the same `paneflow-layout` crate, applies the op at once and converges on revision mismatch. Creating ops would need an id from the authority, so the worker leases id blocks. This removes the split latency risk by construction and gives the window a full local authority while the worker is replaced.
- **D8. Ids are `u64` from one persisted counter.** Options: UUIDs as Rex uses; `u64`. Chosen: `u64`, because `surface_id` is already a JSON number in 23 methods, in the MCP tools and in `PANEFLOW_SURFACE_ID`, so the wire does not change and the ids simply stop changing at restart.
- **D9. The window stays a fallback authority.** Options: block layout changes without a worker; queue them; let the window persist. Chosen: the window persists when no worker holds `serve/owner.lock`, and hands its model back with `layout.adopt`. The "no UX change" constraint leaves no room for a blocked gesture.
- **D10. Settings wins over `init.lua` for keys.** Options: last writer wins; Lua wins; Settings wins. Chosen: Settings wins, so the Settings screen keeps telling the truth about what a key does (P8). Conflicts are reported.
- **D11. `ask_human` long-polls and resumes.** Options: block until answered; return at once and poll. Chosen: wait up to 20 s, then return `pending` with an id the next call resumes. A blocking call would be cut by the 30 s per-server limit the installer writes (R16) and by Codex's 60 s default (R17), and the existing `write_pane` already uses call-again semantics (P5).
- **D12. Native agent prompts are intercepted only on opt-in, and fail open.** A `PreToolUse` hook holds the agent's own prompt until it returns, so intercepting by default would change how every agent session behaves. Every failure path writes nothing, which is what the helper does today (P6).

## Open Questions

- **Lua 5.4 or 5.5?** Both pass the probe (R13) and differ by one feature flag. 5.5 avoids a later migration of user scripts; 5.4 is what tooling targets today. Arthur, before US-026; changes one line of the manifest and the version named in the documentation.
- **Budget for the layout round trip.** US-050 proposes a number from its measurements. Arthur decides whether it becomes a perf gate.
- **Default of `agent_prompts`.** `native` in this PRD. Should it become `paneflow` once US-064 and a period of use confirm it? Arthur, after EP-009.
- **Permission prompts when the agent's pane is visible.** US-068 shows the Paneflow prompt regardless. A rule such as "only when the pane is not visible" needs a definition of visible. Arthur, at US-068.
- **Memory cap of the worker's Lua state.** 256 MiB is a chosen constant (A7), not a measured need. Arthur, at US-035.
- **Undo of a pane with several surfaces.** If US-007 confirms that only one surface is restored, is that kept for parity (this PRD) or fixed? Arthur, after US-007.
- **Rex in operation.** Rex was read, not run, and its demonstration videos were not watched. A behavior the documentation does not state (session persistence across a server restart, the exact shape of `session.view`) may differ from what this PRD assumes it inspires.
[/PRD]
