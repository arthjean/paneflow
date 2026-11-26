[PRD]
# PRD: Remove the MCP Bridge, Keep the CLI and Lua

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-26 | Arthur Jean | Initial PRD, from a five-part code survey (functional and security parity, agent-config installation, build and release, runtime footprint, documentation), each finding re-verified against the code, and a web research pass on code mode, Agent Skills, agent timeouts and MCP listings; revised after an independent rubric review. 8 epics, 26 stories. |

## Problem Statement

Paneflow exposes itself to coding agents through two surfaces that do the same work: the `paneflow` CLI with its Lua entry point (`paneflow do`, `init.lua` actions), and an MCP bridge, `paneflow-mcp`, a stdio server registered in five agents' configuration files. Every MCP tool calls a JSON-RPC method the CLI already reaches (R1), and every security decision behind those tools (caller identity, write approval, rate limits, the `agents = true` filter, question provenance) is taken by the host or the worker from the kernel-attested principal, not by the bridge (R2). The bridge is a second client that has to be kept in step with the first.

Keeping it costs the project today:

1. **It blocks releases.** `paneflow-mcp` is 538,248 B on `main` against a 524,288 B hard cap, so the `Tests (Linux x86_64)` job of `run_tests.yml` has been red since commit `9e477892`, and EP-005 of `tasks/prd-program-resume.md` cannot be certified (R3).
2. **It hides couplings that would break unrelated features.** The agent hooks, the status line and the conductor skill are installed by the same crate as the MCP entry, which refuses to install hooks without the bridge binary, reports an integration as installed only when the MCP entry exists, and stops refreshing hooks silently when `bin/paneflow-mcp` is missing (R4). The only reason the user's `paneflow.json` keeps its key order on every settings write is a `serde_json` feature that crate turns on (R5).
3. **It carries safe defaults the CLI lacks.** Only the bridge limits reads to the caller's workspace and fences search and list output as untrusted text (R6, R7). An agent that calls `paneflow read` or `paneflow search`, which it can already do, reads every workspace and gets search results unfenced.
4. **Agents work better through code and CLIs than through tool calls.** Anthropic reports a 150,000 to 2,000 token reduction (98.7 %) when an agent writes code against an API instead of calling tools directly, and Cloudflare argues that models have seen far more code than tool-call transcripts (R8). Paneflow already ships the code path: `paneflow do` runs Lua 5.4 against the whole registry.

The people affected are users who run coding agents in Paneflow panes, the product's positioning, and in particular users of the conductor workflow, where one agent drives others.

**Why now:** the MCP tools added by the Lua scripting PRD (`ask_human`, `list_actions`, `run_action`) are still under `[Unreleased]` in `CHANGELOG.md` (R9), so removing the bridge before the next release means they never ship. The Agent Skills format that replaces the MCP's discovery is now read by Claude Code, Codex, OpenCode and Gemini CLI (R10). And the size gate on `main` is red because of the bridge alone (R3).

## Overview

The bridge goes away; every guarantee it provided either already lives in a server or moves into one first; agents discover Paneflow through the conductor skill and the CLI.

1. **Parity before removal (EP-001, EP-002).** The servers apply the bridge's defaults to every client: reads become workspace-scoped for a pane caller, exactly as writes already are, and `surface.search` is fenced like `surface.read`. `paneflow ls` fences what it prints for a program. Three verbs replace the MCP tools that had no one-line CLI equivalent: `paneflow write` (over `pane.write`, with its pair approval), `paneflow ask` (over `prompt.request`), and `paneflow do <action> --wait` with `--resume`. The conductor skill carries the untrusted-output rule the bridge used to state, describes the Lua and CLI surface, and is installed for Codex too, into `~/.agents/skills`, which Codex and Gemini CLI read; OpenCode documents only a project-level `.agents/skills` (A2) (TD-02, TD-03, TD-09, TD-10).
2. **Integrations stand alone (EP-003).** `paneflow-mcp-install` is renamed `paneflow-agent-integrations`, loses its MCP writer and every dependency on the bridge, and declares `preserve_order` itself. A one-way purge removes the MCP entries Paneflow wrote into Claude Code, Codex, Gemini CLI, OpenCode and fx configs, in the default home and in every account and space, plus the extracted binary (TD-05, TD-06, TD-07).
3. **Removal (EP-004).** The bridge crate, its tool manifests, its embedding, its settings page and its CLI verb are deleted. `paneflow mcp` stays as a tombstone that names the replacement, and `paneflow mcp uninstall` runs the purge (TD-08). Two helpers remain embedded, under a combined cap equal to the sum of their own caps (TD-13).
4. **Documentation (EP-005).** The repository, the site (`~/dev/paneflow-web`, 6 languages) and the agent guidance describe the CLI and Lua path, and a public Lua scripting page is written, which no page covers today (R11). A guard test keeps the bridge's names out of tracked files outside history.
5. **Proof (EP-006, EP-007, EP-008).** An upgrade from an MCP install and a live agent working through the CLI alone are proven on Linux, then on Windows and macOS hardware, each platform in its own final epic.

The host protocol does not change: `HOST_PROTOCOL_VERSION` stays 1 and every change on the wire is additive (TD-14).

## Goals

| Goal | Baseline (2026-11-26) | Target | How measured |
|------|----------------------|--------|--------------|
| Paneflow MCP entries written into agent configs | 5 writers (claude, codex, gemini, opencode, fx) | 0 writers; purge leaves no entry whose command is `paneflow-mcp` | US-010 tests and US-022 |
| Embedded helper binaries | 3 (shim, ai-hook, mcp), 1,332,144 B on CI at `9e477892` (R3) | 2, combined cap 908,288 B | `scripts/check-helper-sizes.sh` |
| Read methods scoped by the caller's principal | 0 of 3 (`surface.list`, `surface.read`, `surface.search`) | 3 of 3 | `crates/paneflow-api/tests/decision_table.txt` |
| Terminal-text outputs fenced by default for an agent | `read` only | `read`, `search`, `ls` | US-002, US-003 |
| References to the bridge outside history | 61 tracked files, 149 lines (R11); 6 locales on the site | 0 outside the allowlist of US-020 | guard test of US-020 |
| `run_tests.yml` size gate on `main` | red since `9e477892` | green | CI |

## Research Findings

| ID | Finding | Source |
|----|---------|--------|
| R1 | The bridge exposes seven tools (`list_panes`, `read_pane`, `search_pane`, `write_pane`, `ask_human`, `list_actions`, `run_action`), each a call to a registry method the CLI or Lua already reaches: `surface.list`, `surface.read`, `surface.search`, `pane.write`, `prompt.request`/`prompt.wait`, `action.list`, `action.invoke`/`action.wait`. Its resources (`pane://surface/{id}/content`) duplicate `surface.read`. | `crates/paneflow-mcp/src/tools.rs:78,351,363-496`, `resources.rs:35-95`; `crates/paneflow-api/src/methods/{surface,host,prompt,config}.rs` |
| R2 | Identity, the `pane.write` pair approval (120 s TTL, burst 3 then 1/s per pair, provenance line, 16 KiB cap), the `source_session` check, the question provenance and the action filter are enforced server-side from the principal. `paneflow do` connects from its own process, and the worker judges its process lineage: a known agent runtime is the agent, an unreadable chain counts as an agent, a fully read chain without one is the user. | `crates/paneflow-host/src/agent_write.rs:25-26,473-619`; `crates/paneflow-api/src/authorize.rs:194-211`; `crates/paneflow-serve/src/agent_asks.rs:18-19,219,257-262,321-411`; `src-app/src/cli/do_cmd.rs:240`; `crates/paneflow-serve/src/agent_caller.rs:99-121`; `crates/paneflow-serve/src/server.rs:139-155` |
| R3 | `paneflow-mcp` measures 538,248 B on CI at `9e477892` and 540,752 B locally, over its 524,288 B cap; the other helpers fit (shim 418,128 B, ai-hook 375,768 B). Nothing outside the bridge crate changed its size. | `run_tests.yml` "Helper binary size budget" log of the run on `9e477892`; `scripts/check-helper-sizes.sh:8-10` |
| R4 | `resolve_binaries` returns `None` without `bin/paneflow-mcp`, and the worker then skips the hook refresh with an info log; `install_integration_at` refuses hooks without the bridge; `installed_evidence` requires the MCP entry; `paneflow hooks setup` and `paneflow integrations install` fail when the bridge cannot be extracted; the runtime catalog rejects an installer adapter without `mcp_config`. | `crates/paneflow-serve/src/integrations.rs:11-21`, `worker.rs:264-267`; `crates/paneflow-mcp-install/src/integrations.rs:371-379,534-555`; `src-app/src/launch.rs:25-46`; `crates/paneflow-agent-config/build_support.rs:473-478` |
| R5 | Only `crates/paneflow-mcp-install/Cargo.toml:16` enables `serde_json/preserve_order`, unified across the graph; `src-app/src/config_writer.rs:28-63` round-trips the user's `paneflow.json` through `serde_json::Value`. | Files cited |
| R6 | `surface.list`, `surface.read` and `surface.search` use `ScopeRule::ClaimedSession`: a pane caller is scoped only when it sends `scope_session`, which the bridge does and the CLI never does. Writes use `Workspace` or `WorkspaceUnlessAll`. | `crates/paneflow-api/src/methods/surface.rs:315,350,374,487`; `authorize.rs:89-103,239-254`; `crates/paneflow-mcp/src/scope.rs:45-66`; `src-app/src/cli/read_cmds.rs:26,58,84` |
| R7 | `surface.search` is never fenced, in the host or the window; `surface.read` is fenced by default (`ai_injection_fence` defaults to true) unless `fenced: false`, which `paneflow read --raw` sends. The bridge fenced list, read and search output with a random id. The fence helper is shared in `paneflow-ipc-client`. | `crates/paneflow-host/src/control.rs:619-679`; `src-app/src/app/ipc_handler/surface_methods.rs:336-343,773`; `crates/paneflow-config/src/schema/config.rs:488-490`; `crates/paneflow-mcp/src/output.rs:12-33`; `crates/paneflow-ipc-client/src/scrollback.rs:88-94` |
| R8 | Code execution against an API instead of direct tool calls cut a workflow from 150,000 to 2,000 tokens (98.7 %); code lets intermediate results bypass the model; executing agent-written code needs limits, which `paneflow do` has (`--memory`, instruction deadline, `--timeout`). Cloudflare: "LLMs have seen a lot of code. They have not seen a lot of 'tool calls'." | [Anthropic, 2025-11-04](https://www.anthropic.com/engineering/code-execution-with-mcp); [Cloudflare, 2025-09-26](https://blog.cloudflare.com/code-mode/); `src-app/src/cli/do_cmd.rs` help |
| R9 | `ask_human`, `list_actions` and `run_action` are described under `[Unreleased]`; the last release is v0.20.0 (2026-11-19). The bridge itself has shipped since v0.3 and users have entries in their agent configs. | `CHANGELOG.md:6-54`; `git tag` |
| R10 | Agent Skills are read by Claude Code (`~/.claude/skills`), Codex (`~/.agents/skills`, `.agents/skills`), OpenCode (`.claude/skills`, `.agents/skills`) and Gemini CLI (`~/.agents/skills`, `~/.gemini/skills`), each loading name and description first and the body on demand. | [Claude Code](https://code.claude.com/docs/en/skills), [Codex](https://learn.chatgpt.com/docs/build-skills), [OpenCode](https://opencode.ai/docs/skills/), [Gemini CLI](https://geminicli.com/docs/cli/skills/) |
| R11 | 61 tracked files (149 lines) outside the bridge crates, `mcps/`, `tasks/`, `docs/release/qualification/` and `docs/user/blog/` match `paneflow-mcp`, `paneflow mcp install`, `MCP bridge` or `mcp_config` (`git grep -c -I -i -E`, 2026-11-26), and the site's pages in 6 locales describe the bridge; no public page documents `paneflow do`, `init.lua` or `paneflow api` beyond passing mentions. The site syncs the mirror with `bun run sync:public-docs ../paneflow/docs/user`. | `docs/user/scripting.md:1-44,506-627`; `README.md:38-48`; `/home/arthur/dev/paneflow-web/content/docs/**`, `messages/*.json`, `AGENTS.md:81` |
| R12 | When a configured stdio MCP server's command is missing, Claude Code keeps running and marks the server failed (`claude mcp list` shows "Failed to connect"); Codex fails startup only for `required = true` servers (behavior for optional ones inferred); Gemini CLI reports the server disconnected per third-party reports. | [Claude Code MCP](https://code.claude.com/docs/en/mcp); [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli); [Gemini guide](https://googlecloudplatform.github.io/vertex-ai-creative-studio/experiments/mcp-genmedia/agents/geminicli/) |
| R13 | Claude Code's Bash tool waits 120,000 ms per command by default and at most 600,000 ms. | [Claude Code env vars](https://code.claude.com/docs/en/env-vars) |
| R14 | The bridge installer writes `mcpServers.paneflow` (`~/.claude.json`), `[mcp_servers.paneflow]` (`$CODEX_HOME/config.toml`, through `toml_edit` to keep comments), `mcpServers.paneflow` (`~/.gemini/settings.json`, JSONC), `mcp.paneflow` (OpenCode config) and `mcp.paneflow` (`~/.fx/mcp.json`); legacy keys `paneflow-dev` and `paneflow-mcp` exist for Claude. The writers are unchanged since v0.20.0, so these are the shapes 0.20.0 wrote. The worker rewrites the entry at every start for each installed runtime. No function removes an account's or a space's integration. | `crates/paneflow-mcp-install/src/agents/{claude_code,support,gemini,opencode,fx}.rs`; `integrations.rs:552,1034-1046`; `crates/paneflow-serve/src/worker.rs:336-342`; `crates/paneflow-mcp-install/src/lib.rs:18-24`; `git log v0.20.0..HEAD -- crates/paneflow-mcp-install/src/agents` is empty |
| R15 | `paneflow-agent-config` is a dependency of the size-capped `paneflow-ai-hook` and `paneflow-shim`. | `crates/paneflow-ai-hook/Cargo.toml:13`, `crates/paneflow-shim/Cargo.toml:15` |
| R16 | `crates/paneflow-mcp-install` links a Windows manifest only so that Windows' installer-detection heuristic does not demand elevation for a test binary whose name contains "install". | `crates/paneflow-mcp-install/build.rs`, `paneflow-mcp-install.manifest` |
| R17 | The branch `feat/agent-browser` (11 commits ahead of `main`) adds `crates/paneflow-mcp/src/browser.rs` beside `src-app/src/cli/browser_cmd.rs`; its PRD has EP-005 IN_PROGRESS and EP-006 to EP-009 TODO. | `git log main..feat/agent-browser`; `git grep -c browser feat/agent-browser -- crates/paneflow-mcp/src`; `tasks/prd-agents-browser-status.json` |
| R18 | Glama lists a server claimed through `glama.json` and documents no removal path; awesome-mcp-servers documents none either. | [Glama](https://glama.ai/blog/2025-07-08-what-is-glamajson); [CONTRIBUTING](https://github.com/punkpeye/awesome-mcp-servers/blob/main/CONTRIBUTING.md) |

## Technical Decisions

- **TD-01. Remove, with no deprecation release.** Options: deprecate for one release, then remove; remove now. Chosen: remove now. The newest MCP tools never shipped (R9); every tool has a server-side equivalent (R1, R2); an agent whose MCP entry points at a missing binary keeps running (R12); the purge (TD-07) and the tombstone (TD-08) cover users who installed it.
- **TD-02. Defaults move into the servers, not into the CLI.** Options: make the CLI send `scope_session` by default; change the scope rule. Chosen: `surface.list`, `surface.read` and `surface.search` become `ScopeRule::WorkspaceUnlessAll`, the rule `pane.write` and `surface.send_text` already use (R6), so a Lua script or a raw `paneflow api call` is scoped too. A pane caller reaches other workspaces with `scope: "all"` when orchestration is open for it, as for writes; without the gate, `scope: "all"` stays scoped to its own workspace (`authorize.rs:96-101`). A read refused by a store for being out of scope carries a new `policy::cross_workspace_read_hint()`, the read counterpart of the write hint (`crates/paneflow-api/src/policy.rs:11-16`), appended by the window's and the host's scope checks (`control.rs:530-540`). A non-pane caller (the user's own terminal, `External`) keeps instance-wide reads, because `pane_scope` applies only to `Pane` principals (`authorize.rs:193-212`). This also narrows a human's `paneflow read` from inside a pane; that is the same rule writes already follow.
- **TD-03. Fencing.** `surface.search` gains the `fenced` parameter of `surface.read`, with the same default (`ai_injection_fence`), in the window and in the host. When fenced, the reply keeps `matches` with each `text` neutralized by `neutralize_untrusted`, and adds `text`: one `wrap_untrusted` block holding every match as `<line>: <text>`, one block as the bridge produced (`crates/paneflow-mcp/src/tools.rs:412-421`). Its `source` attribute follows each server's own `surface.read` fence: `surface:<id>` in the window, `session:<session>` in the host (`crates/paneflow-host/src/control.rs:632`); both add `pattern="<pattern>"`. The window and host reuse the shared helpers (R7). `paneflow ls` has no terminal body to fence server-side; the CLI wraps what it prints in one `wrap_untrusted` block when its standard output is not a terminal, which is how an agent's shell tool captures it, unless `--raw` is passed. `--raw` stays on `read`, `search` and `ls`: humans read from panes too, and the fence protects an agent from text it did not ask to obey, not from its own explicit choice. The skill tells agents never to pass it (US-007).
- **TD-04. Agents absent from the runtime catalog are not reclassified.** Options: treat every pane caller as an agent unless its parent is the pane's shell; keep the lineage verdict. Chosen: keep it. The first option would put a user's own scripts under the `agents = true` filter and the agent rate limit. The bridge's self-declared `agent` origin was voluntary: such an agent can already call `paneflow do` today, so the removal does not widen what it reaches. The remedy for an unrecognized agent is a runtime descriptor (`runtimes/README.md`).
- **TD-05. The integrations crate is renamed, not folded into `paneflow-agent-config`.** `paneflow-agent-config` links into the size-capped ai-hook and shim (R15), and the integration code pulls `which`, `sha2`, `anyhow` and `toml_edit`. The crate becomes `crates/paneflow-agent-integrations` through `git mv`, keeping its history. The Windows manifest exists only because of the word "install" in the test binary's name (R16); the new name has none, so `build.rs` and the manifest are deleted and the Windows epic proves the tests still start without elevation.
- **TD-06. `preserve_order` is declared where it is needed.** `src-app/Cargo.toml` and `crates/paneflow-agent-integrations/Cargo.toml` enable `serde_json/preserve_order` themselves (R5), and a test pins the key order of a rewritten `paneflow.json`.
- **TD-07. A one-way purge.**
  - **What it removes:** an entry is removed when its key is `paneflow`, `paneflow-dev` or `paneflow-mcp` and its command's file stem is `paneflow-mcp`, so a user's own `paneflow` entry pointing elsewhere survives. The command is `command` for Claude Code, Codex and Gemini CLI, and the first element of `command` for OpenCode and fx.
  - **Where:** in the five files of R14, in the default home and in every account and space target, keeping each format: JSON, JSONC through the existing JSONC editor, and TOML through `toml_edit`, so comments survive.
  - **When:**
    - at every worker start, where the hook refresh runs today;
    - at every account and space refresh at launch;
    - in `paneflow integrations install` and `paneflow integrations remove`;
    - in `paneflow mcp uninstall`.
  - **The extracted binary:** the app deletes `<home>/bin/paneflow-mcp[.exe]` at startup, where it used to extract it.
  - **Debug builds:** as today, the worker leaves integrations untouched in a debug build (`worker.rs:258`), so a debug build never purges the installed release's entries. The explicit commands purge in every build.
  - **Lifetime:** the purge's own table of the five files lives in a `legacy_mcp` module, independent of the runtime catalog, which loses `mcp_config`. It stays until a later decision (Open Questions).
- **TD-08. `paneflow mcp` becomes a tombstone.** Any `paneflow mcp ...` prints, on stderr, that the bridge was removed and that agents use the `paneflow` CLI and the conductor skill installed by `paneflow integrations install <agent>`, then exits 1. `paneflow mcp uninstall` also runs the purge, prints what it removed and exits 0, so the documented cleanup command keeps working.
- **TD-09. New verbs, with the bridge's proven waits.**
  - **`paneflow write <target> <text> [--submit] [--scope all] [--json]`:** resolves the target with the existing selector, then `surface.status` gives the target's session, as the bridge did (`bridge.rs:314-330`). It sends `pane.write` with `source_session` from `PANEFLOW_SESSION_ID`, which the server checks against the principal (R2). It prints the reply's `status`: `written` (exit 0), `approval_pending <request>` (exit 0, the human decides), or `no_window` (exit 1); `--json` prints the whole reply.
  - **`paneflow ask <question> [--option <text>]... [--multiple] [--timeout <duration>]`:**
    - It sends `prompt.request` (`kind` `input`, or `pick` with options) and keeps calling `prompt.wait` in slices of at most `MAX_WAIT_MS` (25,000 ms).
    - Origin: it declares `script` with the name `paneflow ask`, and the worker rewrites the origin to the agent for an agent caller (R2).
    - It prints the answer and exits 0.
    - When `--timeout` passes first, it prints `pending <id>` and exits `EXIT_TIMEOUT` (4); `paneflow ask --resume <id>` continues the wait.
  - **`paneflow do <action> --wait <duration>`:** `--wait` takes a required duration and has no upper bound. The CLI sends `wait_ms` of at most `MAX_ACTION_WAIT_MS` (25,000 ms, `crates/paneflow-api/src/methods/config.rs:21`) to `action.invoke`, then calls `action.wait` in slices of at most 25,000 ms until the duration passes. When it passes first, it prints `running <run_id>` and exits 4; `paneflow do --resume <run_id> [--wait <duration>]` continues. Without `--wait`, `paneflow do` blocks as today.
  - **Default wait:** 20 s for `ask`, the value the bridge used for both `ask_human` and `run_action` (`ASK_WAIT_MS`, `bridge.rs:164,278,285`), a sixth of Claude Code's 120 s Bash default (R13). `do` has no default because `--wait` is opt-in.
- **TD-10. The conductor skill replaces the bridge's discovery.**
  - **Content:** the skill gains the untrusted-output rule of `crates/paneflow-mcp/src/mcp.rs:16-22`, the `--raw` prohibition, and the `write`, `ask`, `do`, `actions` and `api describe` verbs. It loses the MCP tool names.
  - **Reach:** the runtime catalog gains `skills_dir = "agents"` (`~/.agents/skills`, `%USERPROFILE%\.agents\skills` on Windows), and `runtimes/codex/runtime.toml` declares it. A Codex integration therefore installs the skill where Codex reads it, and Gemini CLI reads the same directory (R10).
  - **Agents without an installer:** Gemini CLI, OpenCode and fx have no hook installer. Their users keep the documented `bunx skills add` path (`docs/user/conductor.md:23-41`).
- **TD-11. The removal lands on `main` before the browser branch merges.** `feat/agent-browser` adds a bridge module and has five epics unfinished, EP-005 to EP-009 of its PRD (R17). That branch drops `crates/paneflow-mcp/src/browser.rs` when it rebases and keeps `paneflow browser`. Its PRD's pending MCP criteria are rewritten to the CLI then, by the branch's own work (Non-Goals).
- **TD-12. This PRD supersedes FR-02 and the "No UX change" constraint of `tasks/prd-lua-scripting.md`** (lines 125 and 1897) for every MCP tool.
- **TD-13. Combined embed cap.** `EMBED_SIZE_LIMIT_BYTES` (`src-app/build.rs:8`) drops from 1,835,008 B to 908,288 B, the sum of the two remaining per-helper hard caps (524,288 + 384,000, `scripts/check-helper-sizes.sh:8-9`), so the combined cap can never be looser than the caps it sums.
- **TD-14. The wire stays compatible.**
  - **The host protocol:** `HOST_PROTOCOL_VERSION` stays 1 (`crates/paneflow-ipc-client/src/host_control.rs:12`).
  - **Additive changes:** the `fenced` parameter and the `text` field of `surface.search` are additions. For scoping, `authorize` decides and binds the caller's own session as `scope_session` (`authorize.rs:239-254`); the stores keep their existing scope checks, which refuse an out-of-scope target, and only gain the read hint in their refusal message (TD-02). No store gains a new parameter.
  - **An older host:** a host started by an older version, which can outlive an update, answers `surface.search` without `text`. The CLI then prints the matches as today, unfenced until that host restarts.
  - **Snapshots:** `protocol/api-v1.json`, `protocol/paneflow.d.lua` and `crates/paneflow-api/tests/decision_table.txt` are regenerated.
- **TD-15. The startup trace mark is renamed.** `startup_trace::mark("bridge_extracted")` (`src-app/src/launch.rs:380`) marks the end of all helper extraction, so it becomes `helpers_extracted`. `bench/baselines/linux-x86_64/startup.json` is regenerated with `scripts/bench-startup.sh --set-baseline` on a clean tree.

## Assumptions & Constraints

### Assumptions (to validate)
- **A1.** Codex and Gemini CLI keep running when an MCP entry's command is missing (R12 infers this for Codex and relies on third-party reports for Gemini). Low risk: the purge removes the entries on the first worker start. US-022 observes Codex starting cleanly after the upgrade; Gemini CLI is not observed.
- **A2.** OpenCode reads a user-level `~/.agents/skills`. Only project-level `.agents/skills` is documented (R10). Low risk: Gemini CLI and Codex are documented, and OpenCode users keep `bunx skills add`.
- **A3.** No external consumer fetches `mcps/paneflow/tools/*.json`. Nothing in the repository or the site reads them besides the bridge's own test (`tools.rs:613`). Low risk.
- **A4.** An agent's shell tool captures `paneflow ls` through a pipe, so standard output is not a terminal (TD-03). Claude Code and Codex run commands without a TTY for the model's output (inferred from their capture design). US-023 observes it with a live agent.
- **A5.** Few users parse `paneflow ls` or `paneflow ls --json` output through a pipe in their own scripts. Not verified: Paneflow collects no CLI usage data. Accepted with `--raw` and the upgrade note of US-018.

### Hard Constraints
- **The host protocol:** no change to the host protocol version or to `protocol/host-conformance-v1.json` cases other than additions (TD-14).
- **No comments in source:** Rust, shell and PowerShell files carry no comments (`AGENTS.md`, "No comments in source code").
- **Windows item order:** an item added to a file with a `mod tests` goes before it, whatever its `cfg` (`AGENTS.md`, `#[cfg(windows)]` section). `cfg(windows)` code is checked by inspection locally, and by the Windows CI leg and EP-007.
- **The render thread:** no file I/O on the GPUI main thread. The purge and the binary deletion run off-thread, as the extraction does today (`AGENTS.md`, "The render thread never blocks").
- **The lockfile:** every cargo invocation in CI is `--locked`, so any lockfile change is committed with its cause.
- **The size caps:** the shim and ai-hook caps are unchanged.

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior | User message |
|----------|---------|-------------------|--------------|
| User entry named `paneflow` | `mcpServers.paneflow.command` is not `paneflow-mcp` | Purge keeps it | none |
| Config file unreadable or malformed | Purge reads a broken JSON or TOML | File left untouched, warning logged naming the file, other files still purged | log only |
| Bridge binary locked (Windows) | An agent still runs `paneflow-mcp.exe` | Deletion fails, logged, retried at next start | log only |
| Old host after update | Host predates the `text` field | `paneflow search` prints matches unfenced | none |
| Cross-workspace read without orchestration | Pane caller reads a surface of another workspace | Refused, no text returned; `--scope all` without the gate stays scoped to the caller's workspace, as for writes (`authorize.rs:96-101`) | `to read across workspaces, set ipc.orchestration to true in paneflow.json and pass --scope all` |
| `paneflow write` outside a pane | `PANEFLOW_SESSION_ID` unset | Exit 1 before any call | `paneflow write needs the session of the calling pane; run it from a Paneflow pane` |
| Approval pending | Pair not yet allowed | Exit 0, prints `approval_pending <request>` | the human sees the existing approval prompt |
| `paneflow ask` with no window | `prompt.request` ends `no_client` | Exit 1 | `no Paneflow window can show the question` |
| Config file is a symbolic link | Purge finds a link in place of a config file | File not rewritten, warning names it | log only |
| Modified skill | User edited the installed `SKILL.md` | Kept, note printed, as today (`integrations.rs:599-604`) | existing note |
| Downgrade | User reinstalls 0.20 | Its worker writes the MCP entry again; the next upgrade purges it | none |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Hooks stop refreshing after the binary goes | High without EP-003 | High | US-009 precedes US-012; US-022 proves hooks stay "installed" after upgrade |
| `paneflow.json` key order reshuffled | High without TD-06 | Medium | US-008 regression test |
| Agents read other workspaces through the CLI | High today | High | US-001 before US-012 |
| A user script parsing `paneflow ls` breaks on the fence | Low | Medium | `--raw`, and the upgrade note of US-018 (A5) |
| Injected text in search results | Medium | High | US-002 |
| Unrecognized agent runs non-agent actions | Low | Medium | Accepted (TD-04); documented in the skill and `runtimes/README.md` |
| Purge deletes a user's own entry | Low | Medium | Command-stem match (TD-07), tests with a foreign entry |
| Stale Glama or awesome-mcp-servers listing | High | Low | Arthur files the delisting requests after release (Open Questions) |
| Browser branch reintroduces a bridge module | Medium | Medium | TD-11; the bridge crate no longer exists on `main`, so the rebase fails to compile until the module is dropped |

## Non-Goals

- **Changing `pane.write`, `approval.follow` or `approval.decide`.** They are generic registry methods, reachable from the CLI and Lua (`crates/paneflow-api/src/methods/host.rs:37-42`).
- **Changing the scope of other read methods** (`fleet.list`, `surface.status`, `events`). The bridge never exposed them.
- **Reclassifying agents that the runtime catalog does not know** (TD-04).
- **Forcing the fence on an agent that passes `--raw`** (TD-03).
- **Removing or merging the `paneflow hooks` and `paneflow integrations` verbs**, or the `list_panes`, `read_pane` and `search_pane` aliases (`src-app/src/cli/mod.rs:60-62`). They cost nothing and agents trained on the tool names may type them; only their MCP wording changes.
- **Removing the purge and the tombstone.** A later decision (Open Questions).
- **Changing the agents' own MCP handling,** such as `--strict-mcp-config` and `ENABLE_CLAUDEAI_MCP_SERVERS=false` in auto-naming and the usage probe (`src-app/src/auto_naming.rs:319,372-374`, `crates/paneflow-serve/src/usage/probe.rs:41,46`). They switch off the agents' own servers, not Paneflow's.
- **Work on the `feat/agent-browser` branch** (TD-11).
- **Fixing `pane.write` for `External` callers**, which may name any `source_session` (`authorize.rs:194-211`). Pre-existing and unrelated to the bridge.
- **Filing delisting requests with Glama or awesome-mcp-servers.** These are outward-facing actions; Arthur files them after the release (Open Questions).
- **Installing the skill for Gemini CLI, OpenCode or fx without a Codex or Claude integration** (TD-10).

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the full suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in a unit test
- `bash scripts/check-helper-sizes.sh` - the helpers stay under their caps (run after nextest on Linux x86_64). Until US-012 removes the bridge, this script fails on `paneflow-mcp` alone (R3); a story before US-012 passes this gate when `paneflow-mcp` is the only line over its cap
- `cargo deny check advisories licenses sources` - when a dependency or a workspace member changes
- `bash scripts/test-ci-scripts.sh` - when a CI script changes
- `bash scripts/check-workflows.sh` and `uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github` - when `.github/**` changes
- `bun run check` and `bun run build` in `/home/arthur/dev/paneflow-web` - for stories that change the site repository

## Epics & User Stories

### EP-001: Every client gets the bridge's safe defaults

The servers scope reads to the caller's workspace and fence search output, and `paneflow ls` fences what it prints for a program, so an agent using the CLI is as protected as one using the bridge.

**Definition of Done:** on Linux, from a pane of workspace A, `paneflow ls`, `paneflow read` and `paneflow search` reach no surface of workspace B, and reach it with `--scope all` only when `ipc.orchestration` is open for that pane. `paneflow search` and `paneflow ls` output captured through a pipe is inside one untrusted fence. The regenerated decision table lists the three read methods as `workspace_unless_all`.

#### US-001: Reads are scoped to the caller's workspace
**Description:** As a user running agents in several workspaces, I want an agent's reads limited to its own workspace so that it cannot read terminals it was not given.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a pane caller in workspace A and a running window, when it calls `surface.list` without `scope_session`, then the reply lists only surfaces of workspace A
- [x] Given a pane caller in workspace A and no window (the host answers), when it calls `surface.list`, then the reply lists only surfaces of workspace A
- [x] Given a pane caller in workspace A, when it calls `surface.read` on a surface of workspace B, then the call fails and returns no text of that surface
- [x] Given a pane caller in workspace A, when it calls `surface.search` on a surface of workspace B, then the call fails and returns no match
- [x] Given either refusal above, when its message is read, then it contains the new `policy::cross_workspace_read_hint()` text, which names `ipc.orchestration` and `--scope all`, in the window and in the host
- [x] Given `ipc.orchestration` open for the caller, when `paneflow read <target> --scope all` targets workspace B, then the text is returned
- [x] Given orchestration closed, when `paneflow ls --scope all` runs from a pane of workspace A, then only surfaces of workspace A are listed
- [x] Given a caller outside every pane (`External`), when it calls `surface.list`, then every workspace is listed, as today
- [x] Given a pane caller that sends a `scope_session` of another workspace, when it calls `surface.read` on its own workspace's surface, then the text is returned, because the scope is bound to its own session
- [x] Given the three read descriptors, when `protocol/api-v1.json` and `crates/paneflow-api/tests/decision_table.txt` are regenerated, then both list them as `workspace_unless_all`

#### US-002: Search results are fenced by the server
**Description:** As an agent user, I want search results marked as untrusted terminal text so that text planted in another terminal reads as data, not instructions.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `ai_injection_fence` unset, when the window answers `surface.search`, then the reply has a `text` field that is one `<untrusted_terminal_output source="surface:<id>" pattern="<pattern>" id="<16 hex>">` block holding every match as `<line>: <text>`
- [x] Given the same call with no window, when the host answers, then the reply's `text` is the same block with `source="session:<session>"`, as the host's `surface.read` fence names it
- [x] Given a matched line containing `</untrusted_terminal_output`, when it is fenced, then neither `text` nor `matches[].text` contains that sentinel unaltered
- [x] Given `"fenced": false`, when `surface.search` is called, then the reply has no `text` field and `matches[].text` is the raw line
- [x] Given `ai_injection_fence: false` and no `fenced` parameter, when `surface.search` is called, then the reply has no `text` field
- [x] Given `paneflow search <target> <pattern>` and a reply with `text`, when it prints, then it prints `text`
- [x] Given `paneflow search --raw`, when it prints, then it prints `<line>: <text>` lines unfenced
- [x] Given a reply without `text` (a host started by an older version), when `paneflow search` prints, then it prints the matches as today and exits 0
- [x] Given matches whose fenced form exceeds the IPC frame, when the reply is built, then it fits the frame and `truncated` is true

#### US-003: `paneflow ls` fences what it prints for a program
**Description:** As an agent user, I want pane titles and commands printed to an agent marked as untrusted so that a program cannot plant instructions in its title.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given standard output is not a terminal, when `paneflow ls` runs, then its whole output is inside one `wrap_untrusted` block with `source="surface.list"`
- [x] Given `--json` and a non-terminal output, when `paneflow ls` runs, then the JSON is inside the same block
- [x] Given standard output is a terminal, when `paneflow ls` runs, then the output is unchanged from today
- [x] Given `--raw`, when `paneflow ls` runs with a non-terminal output, then no fence is printed
- [x] Given a surface title containing `</untrusted_terminal_output`, when it is printed fenced, then the sentinel is neutralized

---

### EP-002: Verbs and guidance for what only the bridge offered

An agent can relay to another agent, ask the user, and run a long action from the CLI in one call each, and the conductor skill tells it how and what to distrust.

**Definition of Done:** on Linux, `paneflow write`, `paneflow ask` and `paneflow do <action> --wait` each pass their happy-path and timeout-path integration tests against a running worker and host, and the installed conductor skill (the Claude copy and `~/.agents/skills`) contains the untrusted-output rule and no MCP tool name.

#### US-004: `paneflow write` relays text to another agent
**Description:** As a conductor agent, I want one verb that relays text into another agent's pane through the pair approval so that I never fall back to typing with `paneflow send`.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given an allowed pair, when `paneflow write <target> "hello"` runs from a pane, then the host relays the text with its provenance line
- [x] Given an allowed pair, when the relay succeeds, then the CLI prints `written` and exits 0
- [x] Given a pair not yet allowed, when `paneflow write` runs, then it prints `approval_pending <request>` and exits 0
- [x] Given a pair not yet allowed, when `paneflow write` runs, then the window shows the existing approval prompt for that pair
- [x] Given no window, when `paneflow write` runs, then it prints `no_window` and exits 1
- [x] Given `--submit`, when the text is relayed, then `pane.write` receives `submit: true`
- [x] Given `--json`, when `paneflow write` runs, then it prints the `pane.write` reply as JSON
- [x] Given `PANEFLOW_SESSION_ID` unset, when `paneflow write` runs, then it exits 1 with `paneflow write needs the session of the calling pane; run it from a Paneflow pane` and sends no request
- [x] Given a target in another workspace without orchestration, when `paneflow write` runs, then it exits 1 and stderr names `ipc.orchestration`
- [x] Given text over 16 KiB, when `paneflow write` runs, then it exits 1 with the host's size message
- [x] Given its own surface as target, when `paneflow write` runs, then the host refuses it and the CLI exits 1

#### US-005: `paneflow ask` asks the user a question
**Description:** As an agent, I want to ask the user a question from my shell tool so that I can wait for a human decision without an MCP tool.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a window, when `paneflow ask "Deploy now?" --option yes --option no` runs from an agent pane, then the window shows a pick prompt attributed to that agent and pane
- [x] Given that prompt answered `yes`, when the CLI returns, then it prints `yes` and exits 0
- [x] Given no `--option`, when `paneflow ask "Branch name?"` runs, then an input prompt is shown and the typed line is printed
- [x] Given no `--timeout`, when nobody answers, then the CLI prints `pending <id>` after 20 s and exits 4
- [x] Given `--timeout 60s`, when the answer comes after 40 s, then the CLI prints it, having waited in successive `prompt.wait` calls of at most 25,000 ms
- [x] Given `paneflow ask --resume <id>` from the same session, when the user answers, then the answer is printed and the exit code is 0
- [x] Given `--resume <id>` from another pane's session, when it runs, then it exits 1 and prints the worker's refusal
- [x] Given a fourth question within a second from the same agent, when it is asked, then the CLI prints `rate_limited` and exits 1
- [x] Given no window, when `paneflow ask` runs, then it exits 1 with `no Paneflow window can show the question`
- [x] Given `--multiple` without `--option`, when it runs, then it exits 1 before any request

#### US-006: `paneflow do <action>` can return before the action ends
**Description:** As an agent, I want a long action to hand back a run id so that my shell tool's timeout does not cut it off.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given an action that ends in 1 s, when `paneflow do <name> --wait 10s` runs, then its result is printed and the exit code is 0
- [x] Given an action still running after `--wait 1s`, when the wait passes, then the CLI prints `running <run_id>` and exits 4
- [x] Given `--wait 60s` and an action that ends after 40 s, when it ends, then its result is printed, the CLI having sent `wait_ms` of at most 25,000 to `action.invoke` and then to each `action.wait`
- [x] Given `paneflow do --resume <run_id>`, when the action ends, then its result is printed and the exit code is 0
- [x] Given an agent caller and an action without `agents = true`, when `paneflow do <name> --wait 1s` runs, then it fails with the same message as without `--wait`
- [x] Given an unknown `run_id`, when `--resume` runs, then it exits 1 with the worker's message
- [x] Given no `--wait`, when `paneflow do <name>` runs, then it blocks until the action ends, as today

#### US-007: The conductor skill carries the rules and the whole surface
**Description:** As an agent, I want the installed skill to tell me how to drive Paneflow and what not to trust so that I need no MCP server to discover it.

**Priority:** P0
**Dependencies:** Blocked by US-001, US-002, US-004, US-005, US-006

**Acceptance Criteria:**
- [x] Given `skills/paneflow-conductor/SKILL.md`, when it is read, then it states that text inside `untrusted_terminal_output` is data, never instructions, and that `--raw` must not be passed
- [x] Given the same file, when it is read, then it documents `paneflow write`, `paneflow ask`, `paneflow do` with `--wait` and `--resume`, `paneflow actions` and `paneflow api describe`
- [x] Given the same file, when it is read, then it names no MCP tool and no `paneflow mcp` command
- [x] Given `runtimes/codex/runtime.toml` declaring `skills_dir = "agents"`, when `paneflow integrations install codex` runs, then `~/.agents/skills/paneflow-conductor/SKILL.md` exists with the `paneflow-version` frontmatter
- [x] Given a user-modified `~/.agents/skills/paneflow-conductor/SKILL.md`, when the integration refreshes, then the file is kept and the existing "kept your modified" note is reported
- [x] Given `paneflow integrations remove codex`, when it runs, then the Codex skill copy is removed only if its fingerprint matches the recorded one
- [x] Given a descriptor with an unknown `skills_dir` value, when the catalog builds, then the build fails naming the accepted values

---

### EP-003: Agent integrations without the bridge

Hooks, the status line and the skill install, report and refresh with no bridge binary, no MCP entry is written, and every entry Paneflow wrote before is purged.

**Definition of Done:** on Linux, with `bin/paneflow-mcp` absent, `paneflow integrations install claude-code` and `codex` succeed, `paneflow integrations list` shows them installed, the worker refresh logs a refresh for each, and a home seeded with the entries the 0.20.0 writers produce for all five agents, in the default home and in one space account, has none left after one worker start.

#### US-008: Rename the integrations crate and keep `paneflow.json` order
**Description:** As a maintainer, I want the integration engine in a crate named for what it does, with its JSON key order declared, so that removing MCP code cannot silently reorder user files.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given the workspace, when it builds, then `crates/paneflow-agent-integrations` exists, made by `git mv` from `crates/paneflow-mcp-install`, and no `Cargo.toml` names `paneflow-mcp-install`
- [x] Given `src-app/Cargo.toml` and the renamed crate's manifest, when they are read, then each enables `serde_json` `preserve_order`
- [x] Given a `paneflow.json` whose keys are not sorted, when a setting is saved through `config_writer`, then every other key keeps its position
- [x] Given a `paneflow.json` with a syntax error, when a setting is saved, then the file is left unchanged and the save reports failure, as today
- [x] Given the renamed crate, when it is listed, then it has no `build.rs` and no `.manifest` file (the Windows launch without elevation is proven by US-024)

#### US-009: Hooks install, report and refresh with no bridge
**Description:** As an agent user, I want my hooks to keep working once the bridge is gone so that agent status and prompts never stop silently.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given no `bin/paneflow-mcp`, when `paneflow integrations install claude-code` runs, then it exits 0 and the hooks, the status line and the skill are written
- [x] Given that install, when `~/.claude.json` is read, then no `mcpServers` key was added
- [x] Given installed Claude hooks and no MCP entry, when `paneflow integrations list` runs, then Claude Code is reported installed
- [x] Given a release build and no `bin/paneflow-mcp`, when the worker starts, then it refreshes each installed integration and logs `refreshed the <runtime> integration`
- [x] Given a missing `bin/paneflow-ai-hook`, when the worker starts, then it skips the refresh and logs a warning naming the missing helper
- [x] Given `paneflow hooks setup`, when it runs, then it extracts only the hook helper and succeeds without a bridge
- [x] Given the runtime catalog, when it builds, then an installer adapter without `mcp_config` is accepted and no descriptor declares `mcp_config`
- [x] Given a descriptor that still declares `mcp_config`, when the catalog builds, then the build fails with the unknown-field error naming the file

#### US-010: A purge engine for the MCP entries Paneflow wrote
**Description:** As a user upgrading from a version with the bridge, I want a purge that removes exactly Paneflow's MCP entries from every agent config format so that nothing else in those files changes.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given `~/.claude.json` with `mcpServers.paneflow`, `paneflow-dev` and `paneflow-mcp` pointing at `paneflow-mcp`, when the purge runs, then all three keys are removed and every other key keeps its value and order
- [x] Given `$CODEX_HOME/config.toml` with `[mcp_servers.paneflow]` and a comment above another table, when the purge runs, then the table is removed and the comment remains
- [x] Given `~/.gemini/settings.json` in JSONC with comments, when the purge runs, then `mcpServers.paneflow` is removed and the comments remain
- [x] Given an OpenCode config with `mcp.paneflow` whose `command[0]` is `paneflow-mcp`, when the purge runs, then the entry is removed
- [x] Given `~/.fx/mcp.json` with `mcp.paneflow` whose `command[0]` is `paneflow-mcp.exe`, when the purge runs, then the entry is removed
- [x] Given `mcpServers.paneflow` whose command is `/usr/bin/other-server`, when the purge runs, then it is kept
- [x] Given a malformed config file, when the purge runs, then that file is unchanged, a warning names it, and the other files are purged
- [x] Given a config file that is a symbolic link, when the purge runs, then the file is not rewritten and a warning names it
- [x] Given a config already purged, when the purge runs again, then no file is rewritten
- [x] Given the purge's report, when it is read, then it lists each removed entry with its file

#### US-011: The purge runs wherever Paneflow wrote entries
**Description:** As a user upgrading, I want the purge to run on its own, for my default home, my accounts and my spaces, so that I never have to clean configs by hand.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given a release build, when the worker starts, then it runs the purge once before the hook refresh
- [x] Given a debug build, when the worker starts, then it does not run the purge
- [x] Given a space account target with its own Claude and Codex config dirs holding Paneflow MCP entries, when accounts refresh at launch, then those entries are removed
- [x] Given `paneflow integrations install <runtime>` or `paneflow integrations remove <runtime>`, when it runs in any build, then the purge runs for the default home
- [x] Given `<home>/bin/paneflow-mcp`, when the app starts, then the file is deleted off the render thread
- [x] Given that deletion fails, when the app starts, then a warning is logged, startup continues, and the next start tries again

---

### EP-004: The bridge is gone

The bridge crate, its manifests, its embedding, its UI and its verb are deleted, and the build, CI and packaging serve two helpers.

**Definition of Done:** `git ls-files crates/paneflow-mcp mcps` is empty, the release build embeds exactly `paneflow-shim` and `paneflow-ai-hook`, the size gate reports two helpers under a 908,288 B combined cap, `paneflow mcp install` prints the tombstone and exits 1, and Settings has no MCP page.

#### US-012: Delete the bridge and its embedding
**Description:** As a maintainer, I want the bridge crate and every path that builds or extracts it removed so that no code keeps it alive.

**Priority:** P0
**Dependencies:** Blocked by US-001, US-002, US-003, US-004, US-007, US-011

**Acceptance Criteria:**
- [x] Given the repository, when it is listed, then `crates/paneflow-mcp` and `mcps/` do not exist and the root `Cargo.toml` has no `paneflow-mcp` member
- [x] Given `src-app/build.rs`, when the outer build runs, then the nested build compiles `paneflow-shim` and `paneflow-ai-hook` only
- [x] Given embedded helpers totalling more than 908,288 B, when the outer build runs, then it fails with the existing combined-cap message
- [x] Given the embedded assets, when `bins_embed_contains_all_staged_binaries` runs, then it expects exactly the two helpers
- [x] Given startup, when the startup trace is written, then it has a `helpers_extracted` mark and no `bridge_extracted` mark
- [x] Given a pane's `pane.write` into another workspace refused by the host, when its message is read, then it names `ipc.orchestration` and `--scope all` and not `PANEFLOW_MCP_SCOPE`

#### US-013: `paneflow mcp` names the replacement
**Description:** As a user following an old guide, I want `paneflow mcp` to tell me what replaced it so that I am not left with an unknown-verb error.

**Priority:** P0
**Dependencies:** Blocked by US-011, US-012

**Acceptance Criteria:**
- [x] Given `paneflow mcp install`, when it runs, then stderr says the bridge was removed and names `paneflow integrations install <agent>` and the conductor skill
- [x] Given `paneflow mcp install`, when it ends, then its exit code is 1
- [x] Given `paneflow mcp status`, when it runs, then it prints the same message and exits 1
- [x] Given `paneflow mcp uninstall`, when it runs, then the purge runs, each removed entry is printed with its file, and the exit code is 0
- [x] Given `paneflow mcp uninstall` with nothing to remove, when it runs, then it prints `no Paneflow MCP entry found` and exits 0
- [x] Given `paneflow --help`, when it is printed, then `mcp` is not listed

#### US-014: Settings and the palette lose the MCP
**Description:** As a user, I want the settings to show only what exists so that no switch or page promises a bridge.

**Priority:** P0
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given Settings, when its navigation is listed, then there is no `Plugins` entry
- [x] Given Settings › Agents, when an agent's integration switch is shown, then its label is `Hooks`
- [x] Given an account whose hook install fails because its config directory is not writable, when Settings › Agents shows the account, then the error reads `Hooks were not added to this account: <error>`
- [x] Given the settings search, when `MCP` is typed, then no settings page, row or header answers, and no shortcut contains `mcp` (the shortcut list's fuzzy subsequence matches, such as `Copy`, are pre-existing and unrelated to the bridge)
- [x] Given the command palette, when it is opened, then it has no `MCP servers` entry and the scripting help entry no longer matches `mcp`
- [x] Given `DESIGN.md`, when it is read, then the Plugins row is gone and the integration switch is described as `Hooks`
- [x] Given a debug build on its own tagged home, when Settings is opened and every section is visited, then no section panics and the Agents page shows the `Hooks` switch (manual check)

#### US-015: Build and packaging scripts serve two helpers
**Description:** As a release maintainer, I want every build and packaging script to know only the two helpers so that no release step looks for a binary that no longer exists.

**Priority:** P0
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given a helper directory holding only `paneflow-shim` and `paneflow-ai-hook` under their caps, when the size check runs on it, then it passes and reports exactly those two helpers
- [x] Given a helper directory without `paneflow-ai-hook`, when the size check runs on it, then it fails naming that helper
- [x] Given the helper-size cases of the CI-script test suite, when they run, then their expected totals are the two-helper totals
- [x] Given `scripts/build-windows-helpers.ps1`, `stage-windows-helpers.ps1` and `verify-windows-authenticode.ps1`, when they are read, then none names `paneflow-mcp` (their behavior is proven on Windows by US-024)
- [x] Given `scripts/candidate-manifest.sh` and `.ps1`, when a candidate manifest is written, then it has no `helper.paneflow-mcp` key
- [x] Given a candidate manifest that lacks a required helper, when it is validated, then validation fails naming the helper

#### US-016: Workflows, bench baseline and policy tests serve two helpers
**Description:** As a release maintainer, I want the workflows, the startup baseline and the dependency policy test to match the two helpers so that CI and benches stay meaningful.

**Priority:** P0
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given `.github/workflows/release-linux.yml`, when the glibc-floor step runs, then it checks the two helpers
- [x] Given `.github/workflows/run_tests.yml`, when its `rust` path filter is read, then it has no `mcps/**` line
- [x] Given `bench/baselines/linux-x86_64/startup.json`, when it is read, then it has `helpers_extracted` metrics produced by `scripts/bench-startup.sh --set-baseline` on a clean tree and no `bridge_extracted` metric
- [x] Given `src-app/tests/dependency_source_policy.rs`, when it runs, then its helper list has two entries
- [x] Given a helper added to the embed list without a policy entry, when `dependency_source_policy` runs, then it fails naming the helper

---

### EP-005: Documentation, guidance and listings

Every document, guide and listing describes the CLI and Lua path, the public site gains a Lua scripting page, and a guard test keeps the bridge from coming back into the documents.

**Definition of Done:** the guard test of US-020 passes on the repository, the site builds with no page naming `paneflow mcp`, `docs/user` is re-synced from the site, and `glama.json` is deleted.

#### US-017: Repository documents and agent guidance
**Description:** As a contributor or an agent reading the repository, I want its documents to describe what exists so that nobody installs or documents a removed bridge.

**Priority:** P0
**Dependencies:** Blocked by US-012, US-013, US-014, US-015, US-016

**Acceptance Criteria:**
- [x] Given `README.md`, when its agent section is read, then it describes `paneflow integrations install`, the conductor skill, `paneflow read`, `search`, `write`, `ask` and `paneflow do`, and no MCP install
- [x] Given `ARCHITECTURE.md`, when it is read, then no crate list or paragraph describes `paneflow-mcp`, and the integrations crate is named `paneflow-agent-integrations`
- [x] Given `ARCHITECTURE.md`, when its write-approval paragraph is read, then it describes `pane.write` as reached by `paneflow write` and Lua
- [x] Given `AGENTS.md`, when its helper bullet is read, then it lists two embedded helpers with their caps, and its `toml_edit` remark names `paneflow-agent-integrations`
- [x] Given `CLAUDE.md`, when it is read, then it points to the CLI and the conductor skill instead of `paneflow mcp install`
- [x] Given `.github/SECURITY.md`, when its scope is read, then it names the server-side fencing, the read and write scoping and the pair approval, not an MCP bridge
- [x] Given `llms.txt`, `context7.json`, `runtimes/README.md`, `bench/README.md` and `docs/release/persistent-qualification.md`, when they are read, then none describes the bridge or a `helper.paneflow-mcp` key
- [x] Given the repository root, when it is listed, then `glama.json` does not exist
- [x] Given a user who still has a Paneflow MCP entry from an older version, when they read the agent section of `README.md`, then it tells them that upgrading removes it and that `paneflow mcp uninstall` removes it by hand
- [x] Given `runtimes/README.md`, when its integration section is read, then it states that an agent the catalog does not know is treated as the user by the worker, and that a descriptor fixes it (TD-04)

#### US-018: Changelog and release metadata
**Description:** As a user reading the release notes, I want the removal and its replacement stated so that I know what to run after upgrading.

**Priority:** P0
**Dependencies:** Blocked by US-012, US-013

**Acceptance Criteria:**
- [x] Given `CHANGELOG.md` `[Unreleased]`, when it is read, then the `ask_human`, `list_actions` and `run_action` entries describe `paneflow ask`, `paneflow actions` and `paneflow do` instead
- [x] Given the same section, when it is read, then a `Removed` entry states that the MCP bridge and `paneflow mcp` were removed and that upgrading removes the entries Paneflow wrote
- [x] Given the same section, when its upgrade notes are read, then one note states that an agent reads terminals only in its own workspace unless orchestration is open and `--scope all` is passed
- [x] Given the same section, when its upgrade notes are read, then one note states that `paneflow ls` and `paneflow search` print a fence when their output is not a terminal, and that `--raw` removes it
- [x] Given `CHANGELOG.md` from `[0.20.0]` down, when it is compared with `main` before this PRD, then it is unchanged
- [x] Given a user whose agent still reports a failed `paneflow` MCP server after upgrading, when they read the `Removed` entry, then it tells them to run `paneflow mcp uninstall`
- [x] Given `assets/io.github.arthurdev44.paneflow.metainfo.xml`, when its feature list is read, then it no longer names a read-only MCP bridge, and its released notes are unchanged

#### US-019: Site documentation pages
**Description:** As a reader of paneflow.dev's documentation in any of its six languages, I want the pages to describe the CLI and Lua path so that I am never told to install a bridge.

**Priority:** P0
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [x] Given `content/docs/{features,index,settings,troubleshooting,scripting/index,scripting/reference}` in en, de, es, fr, ja and zh-Hans, when they are read, then none describes the MCP bridge, its tools or `PANEFLOW_MCP_SCOPE`
- [x] Given the same pages, when they are read, then each place that described an MCP tool describes its CLI equivalent
- [x] Given `content/docs/settings`, when its cleanup section is read, then it says upgrading removes Paneflow's entries and how to remove one by hand
- [x] Given a link to the former MCP section of `scripting/index`, when it is followed, then the page shows a short note at that anchor saying the bridge was removed and linking the CLI equivalents
- [x] Given dated blog posts, when they are compared with the site's `main`, then they are unchanged

#### US-020: Site copy, the mirror and the guard
**Description:** As a visitor of paneflow.dev, I want the marketing copy to match the product, and as a maintainer I want a test that keeps the bridge out of the documents.

**Priority:** P0
**Dependencies:** Blocked by US-018, US-019

**Acceptance Criteria:**
- [x] Given `messages/*.json` in the six locales, when they are read, then the home eyebrow, the download card, the FAQ and the about paragraph no longer promise an MCP bridge
- [x] Given `src/lib/acquisition-policy.ts`, when it is read, then the `read_only_mcp` campaign is gone
- [x] Given `bun run sync:public-docs ../paneflow/docs/user` run from the site, when the paneflow repository diff is read, then only files under `docs/user` changed
- [x] Given `src-app/tests/mcp_bridge_is_gone.rs`, when it runs over the files `git ls-files` lists, then no file outside its allowlist matches `paneflow-mcp`, `paneflow mcp install`, `MCP bridge` or `mcp_config`, case-insensitively
- [x] Given the guard's allowlist, when it is read, then it holds exactly `CHANGELOG.md`, the metainfo file, `docs/release/qualification/**`, `docs/user/blog/**`, `tasks/**`, the legacy purge module and its tests, the `paneflow mcp` tombstone, the guard file itself, and the catalog test of US-009 that asserts `mcp_config` is rejected
- [x] Given a new tracked file outside the allowlist containing `paneflow mcp install`, when the guard runs, then it fails naming the file

#### US-021: Public Lua scripting page
**Description:** As a user or an agent, I want one public page that explains `paneflow do`, `init.lua`, actions, prompts and `paneflow api` so that the code path has documentation of its own.

**Priority:** P1
**Dependencies:** Blocked by US-006, US-019

**Acceptance Criteria:**
- [x] Given the site, when `scripting/lua` is built in the six locales, then it documents `paneflow do` (file, `-e`, stdin), `init.lua` actions with `agents = true`, `paneflow.ui.input` and `paneflow.ui.pick`, `paneflow actions`, `paneflow do --wait` and `--resume`, and `paneflow api list` and `describe`
- [x] Given each example on the page, when it is run against a debug build, then it produces the output the page shows
- [x] Given the scripting index, when it is read, then it links the new page
- [x] Given an example that raises a Lua error, when the page shows it, then the page shows the error text a debug build prints for it
- [x] Given the page published on the site, when `bun run sync:public-docs ../paneflow/docs/user` runs again, then `docs/user` gains the page and only files under `docs/user` change

---

### EP-006: Linux end to end

An upgrade from an install that has the bridge, and a live agent that works through the CLI alone, are proven on Linux.

**Definition of Done:** on Linux, a tagged home whose Claude Code and Codex integrations were installed by the 0.20.0 release, with Gemini CLI, OpenCode and fx configs holding the entries the 0.20.0 writers produce, comes back after one start of the new release build with hooks installed and no MCP entry or bridge binary. A live Claude Code session in a pane lists, reads, searches, relays, asks and runs an action through the CLI, and the tool output in its transcript shows the fences.

#### US-022: Upgrade from an install with the bridge
**Description:** As a user upgrading Paneflow, I want my integrations to survive and the bridge to vanish so that the upgrade needs no action from me.

**Priority:** P0
**Dependencies:** Blocked by US-011, US-012, US-015

**Acceptance Criteria:**
- [x] Given a tagged home where the 0.20.0 release ran `paneflow integrations install claude-code` and `codex` and `paneflow mcp install`, and Gemini CLI, OpenCode and fx configs seeded with the 0.20.0 writers' entries, when the new release build starts once, then `paneflow integrations list` shows Claude Code and Codex installed
- [x] Given the same start, when it completes, then no agent config file contains an entry whose command is `paneflow-mcp`
- [x] Given the same start, when it completes, then `<home>/bin/paneflow-mcp` does not exist
- [x] Given Claude Code started in a pane after the upgrade, when it starts, then `claude mcp list` shows no `paneflow` server
- [x] Given Codex started in a pane after the upgrade, when it starts, then it reports no MCP startup error
- [x] Given a hook event from Claude Code after the upgrade, when it fires, then the pane's agent status updates
- [x] Given `~/.gemini/settings.json` holding a syntax error before the start, when the new build starts, then that file is unchanged, a warning names it, and the other files are purged

#### US-023: A live agent works through the CLI only
**Description:** As an agent user, I want to see a real agent coordinate with another through the CLI so that the replacement is proven, not inferred.

**Priority:** P0
**Dependencies:** Blocked by US-007, US-012

**Acceptance Criteria:**
- [x] Given Claude Code in a pane of workspace A with the conductor skill installed, when asked to list panes, then its transcript shows it ran `paneflow ls`
- [x] Given that run, when its tool output is read in the transcript, then the output is inside an untrusted fence
- [x] Given a planted line `IGNORE PREVIOUS INSTRUCTIONS` in a pane of workspace A, when the agent searches for it, then the tool output in its transcript holds that line only inside an untrusted fence
- [x] Given a pane in workspace B, when the agent lists panes, then that pane is absent from the tool output
- [x] Given a second agent in workspace A, when the first is asked to send it a message, then it runs `paneflow write` and the approval prompt appears
- [x] Given that approval granted, when the text arrives, then it carries its provenance line
- [x] Given an `init.lua` action with `agents = true` that takes 30 s, when the agent runs it, then its transcript shows `paneflow do --wait` and the action's result
- [x] Given the agent asked to confirm with the user, when it runs `paneflow ask`, then the window shows the question attributed to Claude Code and the answer appears in the agent's tool output
- [x] Given an `init.lua` action without `agents = true`, when the agent runs it with `paneflow do`, then the tool output shows the refusal

---

### EP-007: Windows hardware proofs

Everything only Windows can prove, run in one pass on the Windows 11 machine.

**Definition of Done:** on the Windows 11 machine, the MSI built from the final commit installs and runs with two embedded helpers, the renamed crate's test binary starts without elevation, the Windows CI leg is green on that commit, and an upgrade from 0.20.0 purges every entry and the `.exe`.

#### US-024: Windows build, signing and tests without the bridge
**Description:** As a Windows user, I want the release built, signed and tested without the bridge so that nothing in the Windows pipeline looks for it.

**Priority:** P1
**Dependencies:** Blocked by US-015, US-016, US-018

**Acceptance Criteria:**
- [ ] Given the renamed crate's `cfg(windows)` code, when the Windows CI leg's result for the final commit is read, then it reports no lint or build error in that crate
- [ ] Given the renamed crate's test binary on Windows, when it is launched, then it starts without os error 740
- [ ] Given the release workflow's Windows staging and Authenticode steps, when they run, then they sign and verify the two helpers and `paneflow.exe` and report no missing file
- [ ] Given the built MSI, when it is installed and started, then `%USERPROFILE%\.paneflow\bin` holds no `paneflow-mcp.exe`
- [ ] Given `paneflow mcp install` on Windows, when it runs, then it prints the tombstone message and exits 1

#### US-025: Windows upgrade and purge
**Description:** As a Windows user upgrading, I want my agents' configs cleaned the same way so that nothing differs from Linux.

**Priority:** P1
**Dependencies:** Blocked by US-022, US-024

**Acceptance Criteria:**
- [ ] Given a 0.20.0 install with `paneflow mcp install` run for Claude Code and Codex, when the new MSI is installed and started, then `%USERPROFILE%\.claude.json` and `%USERPROFILE%\.codex\config.toml` have no Paneflow MCP entry
- [ ] Given the same upgrade, when both files are compared with their state before it, then nothing else changed
- [ ] Given `paneflow-mcp.exe` held open by a running agent, when the app starts, then startup completes and a warning is logged
- [ ] Given that agent exited, when the app starts again, then `paneflow-mcp.exe` is deleted
- [ ] Given `paneflow integrations install codex`, when it runs, then `%USERPROFILE%\.agents\skills\paneflow-conductor\SKILL.md` exists

---

### EP-008: macOS hardware proofs

Everything only a Mac can prove, run in one pass on the rented Mac.

**Definition of Done:** on the Mac, the signed and notarized bundle of the final commit starts with two embedded helpers, the qualification candidate manifest validates without the bridge, and an upgrade from 0.20.0 purges every entry and the binary.

#### US-026: macOS build, upgrade and purge
**Description:** As a macOS user, I want the bundle, the qualification package and the upgrade to work without the bridge so that macOS ships the same removal.

**Priority:** P1
**Dependencies:** Blocked by US-015, US-022

**Acceptance Criteria:**
- [ ] Given the final commit, when `macos_qualification_package.yml` builds the candidate, then its manifest validates with no `helper.paneflow-mcp` key
- [ ] Given the signed and notarized bundle, when it starts, then `~/.paneflow/bin` holds `paneflow-ai-hook` and the shim links and no `paneflow-mcp`
- [ ] Given a 0.20.0 install with `paneflow mcp install` run for Claude Code and Codex, when the new bundle starts, then `~/.claude.json` and `~/.codex/config.toml` have no Paneflow MCP entry
- [ ] Given the same upgrade, when both files are compared with their state before it, then nothing else changed
- [ ] Given Claude Code launched in a pane after the upgrade, when a hook fires, then the pane's agent status updates
- [ ] Given `~/.codex/config.toml` replaced by a symbolic link, when the new bundle starts, then the linked file is not rewritten and a warning names it

## Open Questions

- **Q1.** When are the purge and the `paneflow mcp` tombstone removed? This PRD keeps both (TD-07, TD-08). Removing them, along with `toml_edit`, waits for evidence that upgrades from 0.20 or older are rare. It depends on release telemetry Paneflow does not collect for desktop installs, so it is Arthur's call.
- **Q2.** Arthur files the delisting requests after the release: an email to Glama (unverified address, R18) and a pull request removing the entry from awesome-mcp-servers, if one exists. They depend on US-017 being released.
- **Q3.** Should Gemini CLI, OpenCode and fx get a skill-only install, without a hook installer (TD-10)? Deferred until a user asks; A2 decides whether `~/.agents/skills` already covers OpenCode.

## Files NOT to Modify

- `native/libghostty/**`: engine archives and generated bindings
- `vendor/**`: vendored upstream crates
- `docs/user/**`: a mirror; it changes only through `bun run sync:public-docs` from the site (US-020)
- `protocol/host-conformance-v1.json` and `crates/paneflow-host/src/protocol.rs`: the host protocol stays at version 1 (TD-14)
- `crates/paneflow-host/src/sandbox/**`: no sandbox change; a change would require `scripts/sandbox-matrix.sh`
- `crates/paneflow-host/src/agent_write.rs`: the write approval is unchanged (Non-Goals)
- `crates/paneflow-shim/**`: its `mcp` mentions are the agents' own subcommands
- `src-app/src/auto_naming.rs` and `crates/paneflow-serve/src/usage/probe.rs`: their MCP flags target the agents' own servers (Non-Goals)
- `rust-toolchain.toml`: the toolchain stays at 1.100.0
- `.config/nextest.toml` quarantine list: no test of this work is quarantined
- `CHANGELOG.md` sections from `[0.20.0]` down, `docs/release/qualification/**`, and released `<release>` notes of the metainfo file: published history
- `tasks/prd-*.md` other than this file: their history stays; `tasks/prd-agents-browser.md` changes on its own branch (TD-11)
[/PRD]
