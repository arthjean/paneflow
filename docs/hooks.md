# Agent notification hooks (`paneflow hooks`)

Paneflow ships a tiny callback binary, `paneflow-ai-hook`, that an agent CLI
runs on lifecycle events (prompt submitted, tool use, stop, notification) to
report its turn state to a running Paneflow instance over the IPC socket. That
state drives the sidebar activity indicators and the turn-end desktop
notification (EP-004, `prd-cli-agent-orchestration`).

There are two ways the hook gets registered, and a single authority rule that
keeps them from firing twice.

## Two installers

| Installer | Scope | Where it writes | Lifetime |
|-----------|-------|-----------------|----------|
| **Ephemeral shim** (`paneflow-shim`) | project | `./.claude/settings.local.json` in the launched project | written on agent launch, swept on exit |
| **Persistent setup** (`paneflow hooks setup`) | user | `~/.claude/settings.json` | written once, survives restarts and Paneflow updates |

The shim copy references the version-pinned binary under
`cache_dir()/paneflow/bin/<VERSION>/`; the persistent copy references the
**stable, non-versioned** path under `data_dir()/paneflow/bin/paneflow-ai-hook`
(`runtime_paths::ai_hook_binary_path`), so the path written into your config
never goes stale across updates.

Both write the *byte-identical* matcher-group shape, tagged with a
`_paneflow_managed` marker, so each side recognizes the other's entries.

## Authority rule (anti double-firing)

**The persistent user-scope install wins.** When `paneflow hooks setup` has
installed managed hooks in `~/.claude/settings.json`, the shim detects them
(`persistent_claude_hooks_present`, reusing the same shape detector) and:

1. **skips** its ephemeral `./.claude/settings.local.json` injection, and
2. **sweeps** any orphan `settings.local.json` it left on a prior run.

Result: the agent fires each event exactly once (one `ai.*` frame per event, no
duplicates) and no `settings.local.json` is planted in your project tree once
you have run `hooks setup`.

If you have **not** run `hooks setup`, the shim's ephemeral injection is the
only mechanism, and it cleans up after itself on exit.

## Commands

```bash
paneflow hooks setup       # install persistent hooks for every supported agent
paneflow hooks status      # report per-agent install state
paneflow hooks uninstall   # remove only Paneflow-managed hooks (no clobber)
```

Exit codes mirror `paneflow mcp`: `0` success (or no agent detected), `1` an
agent errored, `2` usage error. Writes are atomic, backed up, and refuse to
overwrite a present-but-invalid JSON config.

`uninstall` removes only the `_paneflow_managed` matcher-groups; your own hooks
and every other key in the file are left untouched. To fully revert: run
`paneflow hooks uninstall`, then (if you never want the shim's ephemeral copy
either) there is nothing else to clean up because the shim removes its own file
on exit.

## Per-agent support

Only **Claude Code** exposes a verified, file-based user-scope notification-hook
surface, so it is the only agent that receives a persistent install
(`paneflow hooks setup`). Every other integration is EPHEMERAL: injected by
the shim when the agent launches inside a Paneflow terminal, removed when it
exits. The shim wraps all 16 `TerminalAgent` binaries; whatever has no hook
surface below still gets the universal lifecycle (`ai.exit` on crash,
`ai.session_end` on quit) plus the sidebar's "running" row from the process
scan.

| Agent | Mechanism | Where the shim writes | Events mapped |
|-------|-----------|----------------------|---------------|
| Claude Code | Claude hooks (matcher groups) | `./.claude/settings.local.json` | UserPromptSubmit, Notification, Stop, Pre/PostToolUse |
| Codex | hooks.json + TOML feature flag (Unix); JSONL tee (Windows) | `./.codex/hooks.json` | SessionStart, UserPromptSubmit, Stop, Pre/PostToolUse, PermissionRequest |
| CodeBuddy | Claude-compatible clone | `./.codebuddy/settings.local.json` | same five as Claude Code |
| Qoder | Claude-compatible clone | `./.qoder/settings.local.json` | four (no Notification) |
| Gemini CLI | matcher-group hooks in settings | `~/.gemini/settings.json` | BeforeAgent→UserPromptSubmit, AfterAgent→Stop, Before/AfterTool→Pre/PostToolUse |
| Cursor | flat hooks.json (`version: 1`) | `~/.cursor/hooks.json` | beforeSubmitPrompt, stop, pre/postToolUse |
| OpenCode | TS plugin + `plugin` entry | `~/.config/opencode/plugins/paneflow-status.ts` + `opencode.json` | chat.message, tool.execute.before/after, session.idle, permission.asked |
| Pi | TS extension (auto-loaded) | `~/.pi/agent/extensions/paneflow-status.ts` | agent_start/end, tool_execution_start/end |
| Hermes | marked YAML block | `~/.hermes/config.yaml` | pre/post_llm_call, pre/post_tool_call, pre_approval_request |
| Grok | dedicated merged hook file (wholly Paneflow-owned) | `~/.grok/hooks/paneflow.json` | UserPromptSubmit, Stop, Pre/PostToolUse |

Safety properties shared by every ephemeral installer: idempotent merge,
ownership detection by command basename (`paneflow-ai-hook`), orphan sweep on
the next launch after a SIGKILL, and refusal paths that protect user files -
a symlinked config dir, an unparseable PRIMARY config (`opencode.json`,
`~/.hermes/config.yaml` with an existing `hooks:` key), or a `.jsonc`-only
OpenCode setup all skip the install instead of clobbering. Agents without a
documented Windows-specific hook field receive a single `command` string with
Windows-safe quoting on Windows rather than an undocumented extra field. The
TS bridges are env-gated on `PANEFLOW_SOCKET_PATH`, so they are inert when the
CLI runs outside a Paneflow terminal.

Deliberately not integrated (no safe surface): **Copilot CLI** (no hooks, no
JSON stream), **Factory Droid** (dashboard-managed hooks), **Kiro** (hooks
live inside per-agent definition files - no per-session surface),
**Antigravity / Openclaw** and the remaining launchers (no stable public
hook surface). They still get the universal exit/session-end lifecycle and
the "running" row.

On Windows, Codex uses a JSONL tee rather than file hooks; the shim handles that
path at launch.

## What a hook frame carries

`paneflow-ai-hook` compacts each agent's payload before it crosses the socket,
keeping only the fields the app reads. `ai.prompt_submit` carries the opening
512 bytes of the `prompt` field, which is what names the tab after the work
asked of the agent; `ai.stop` carries `summary`, `last_result`, and
`transcript_path`. Everything else in an agent's payload is dropped at the hook
rather than sent and ignored.

An agent whose payload calls its prompt something else (Cursor's
`beforeSubmitPrompt`, for one) sends no prompt, and its tabs keep the label of
the preset that opened them. That is a missing nicety, not a broken session:
nothing else depends on the field.

Every one of those strings is UNTRUSTED terminal-adjacent text. The app treats
them as data on their way to a label - never as instructions - and a prompt
reaching a tab title goes through the same sanitizer as any CLI-written title
(`clean_sidebar_title`), which strips control, bidi, and zero-width characters.

## Parent-death and interrupt guards (cross-platform)

The shim (`paneflow-shim`) wraps each agent so two reliability gaps are closed on
every OS (EP-005 US-017):

- **Orphan guard** (Paneflow is hard-killed, e.g. `kill -9`): the agent must not
  survive and keep burning API tokens.
  - **Linux**: the agent is spawned with `PR_SET_PDEATHSIG = SIGKILL` plus a
    `getppid()` race-close, so the kernel kills it the moment the shim's parent
    dies.
  - **macOS**: kqueue `NOTE_EXIT` is not arm-able from the post-`execve` child,
    so the shim runs a tiny thread that polls `getppid()`; a reparent to
    `launchd` means Paneflow exited and the agent is `SIGKILL`ed (the AC's
    "ou équivalent").
- **Interrupt guard** (the user Ctrl+C's an agent mid-turn, which interrupts the
  turn WITHOUT the agent exiting or firing a Stop hook): the sidebar loader must
  not stick.
  - **Unix**: a blocked-`SIGINT` + `sigwait` thread emits one `ai.stop` per
    Ctrl+C.
  - **Windows**: a `ctrlc` / `SetConsoleCtrlHandler` callback emits the same
    `ai.stop`; the agent still receives `CTRL_C_EVENT` directly from the OS, so
    its turn is interrupted as usual and the shim survives to keep waiting.

The macOS and Windows branches are compile-verified from the Linux host via
`cargo check --target {x86_64-apple-darwin,x86_64-pc-windows-msvc}` (the build
gate runs all three). They still need a one-time RUNTIME smoke on real hardware:

- **macOS orphan smoke**: launch an agent in a pane, note its PID
  (`paneflow ps`), `kill -9` the Paneflow process, then confirm the agent PID is
  gone within ~1 s (`ps -p <pid>` returns nothing). PASS = no orphan.
- **Windows interrupt smoke**: launch an agent, start a turn so the sidebar shows
  the "thinking" loader, press `Ctrl+C` to interrupt mid-turn (the agent stays
  alive at its prompt), then confirm the loader clears within ~5 s. PASS = no
  stuck spinner.
