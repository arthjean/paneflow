[PRD]
# PRD: OSC 7577 Program Resume Protocol in Paneflow

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-26 | Arthur Jean | Initial PRD, from the OSC 7577 specification (revision 0.5, 2026-11-25), its announcement, an ecosystem search, and five code maps of the pane spawn path, the OSC ingestion path, the restore flow, the embedded helpers and the shell integration. `paneflow-host` becomes the OSC 7577 terminal; a resume launcher in the shim crate replaces keystroke typing for every resume; real-hardware epics for Windows and macOS. 8 epics, 31 stories. |

## Problem Statement

When `paneflow-host` dies (a crash, a reboot, a forced stop, the "Stop host and restart" step of an upgrade), every program running in a pane is gone. Today Paneflow brings back three things only:

1. **A fresh shell in the pane's directory**, for every pane that is not an agent: no command, no input and no environment is replayed (`ARCHITECTURE.md:1353,1363-1373`). Neovim with its session, lazygit, a REPL, a debugger: all of them come back as an empty prompt.
2. **A catalog conversation**, for agents whose runtime declares a `[resume]` section (claude-code, codex, opencode, pi, grok, gemini, fx; `runtimes/*/runtime.toml`). Paneflow learns the session id from hook events (`src-app/src/terminal/view/conversation.rs:34-55`), builds `session_argv` from a template compiled into the app (`crates/paneflow-agent-config/build_support.rs:618-692`), then **types** it into a fresh shell: a line reset, the command, `\r` (`conversation.rs:87-94,583-584`), after a quiet heuristic (pane size known and 300 ms without output, at most 5 s) and with 250 ms between panes (`src-app/src/app/conversation_restore.rs:15`). Failure is detected by matching `failure_markers` in the screen text, and only claude and codex declare any.
3. **Nothing for an agent without a session id source.** Pi has no hook adapter (`runtimes/pi/runtime.toml:23`). The shim's `SessionStart` carries no id for it. `session_start_is_storable` (`conversation.rs:57-68`) requires `accepts_session_id`, whose `is_valid_session_id` refuses an empty id because the first character must be alphanumeric or `_` (`src-app/src/agent_resume.rs:38-45`, `src-app/src/agent_sessions.rs:423-430`). So a Pi pane does not resume its conversation, which this PRD infers from reading the code rather than from a run.

The catalog approach is the one the OSC 7577 specification names as the problem it replaces: each tool keeps per-program knowledge of how to resume, and "every program needs separate work for every terminal" (R1, "Relationship to other approaches", which cites cmux). Typing the command also builds shell source text from the resume arguments, which OSC 7577 forbids (R1, "Command execution"); Paneflow gets away with it only because the catalog limits arguments to `[A-Za-z0-9-_.=]` (`build_support.rs:618-692`).

The people who lose work are every Paneflow user whose host ends, which is the case a detached host was built to make rare but cannot make impossible, and in particular agent users, the product's positioning.

**Why now:** OSC 7577 was published on 2026-11-25 by Superlogical, Mitchell Hashimoto's company, for Rex "to resume your programs through a server restart", co-designed with Herdr and reviewed by several terminal emulator and terminal program authors (R2). Rex's architecture, a server that owns the sessions, is Paneflow's. Pi, one of Paneflow's catalog runtimes, merged native OSC 7577 reporting the same day (R4). The pinned libghostty already exposes the callback needed to receive the sequence (R5), and Paneflow already ships the sibling protocol, OSC 7501 program status, from the same specification family (`ARCHITECTURE.md:616-621`).

## Overview

Paneflow implements the terminal side of OSC 7577 in full, in `paneflow-host`, and uses the same execution mechanism for every resume it performs, catalog conversations included.

**The host is the terminal (EP-001, EP-002).** The host owns every pane's PTY, builds its environment and reads its output (`crates/paneflow-host/src/runtime.rs:1112-1156,1279-1385`), so it alone generates the per-pane token, verifies reports and keeps records (TD-01). A pure crate, `paneflow-program-resume`, holds the strict parser, the MAC and time checks and the record state machine, tested against the specification's own vectors (TD-03). The host receives the sequence through libghostty's unknown-sequence callback, which it does not enable today (`runtime.rs:1443-1447`), and a guard test fails the day a libghostty bump starts implementing 7577 itself and silences that path (TD-02). A report's owner must be a live process of the pane, checked with its start time and pane membership through the code `paneflow-peer` already uses for caller identity (TD-05). Records are written into the session manifest, which outlives the host (TD-07), and a record whose owner exits on its own is removed two seconds later, through a kernel exit notification rather than polling (TD-06).

**One way to run a resumed command (EP-003).** The host writes the command into a one-shot private file in the session directory; the shell integration runs a fixed launcher at its first prompt; the launcher, a mode of the `paneflow-shim` binary, reads and deletes the file, looks the program up on `PATH` from the shell's own environment, and executes it with every argument unchanged, so no shell ever parses an argument (TD-08). The launcher reports the attempt to the host through the existing hook path, and the host hands the record to the new process, as the specification requires (TD-09). A resume that fails to start, or that is followed by a host crash before it settles, is never run automatically again (TD-10).

**The desktop decides (EP-004).** A new `terminal.program_resume` setting chooses between asking (the default), resuming automatically and turning the feature off, with a list of trusted programs (TD-11). Agent records are governed by the existing `agents.restore_conversations` and resume automatically only when their arguments match a shape the runtime declares (TD-12). A record wins over a catalog conversation for the same pane. The shim removes the token from agents that do not report natively, so a tool an agent runs after a prompt injection cannot schedule a command for the next start (TD-14).

**Catalog conversations stop typing (EP-005).** The automatic conversation resume passes its command through the same handoff, which removes the quiet heuristic from the shells that have integration (TD-13).

EP-006 proves the whole path on Linux end to end, with a live Pi. EP-007 holds every story that needs a Windows machine and EP-008 every story that needs a Mac, each run in one pass on real hardware.

## Non-Goals

- **Implementing OSC 7577 inside libghostty.** Proposing a native option upstream, as the OSC 7501 work did (R3), is an open question for after this PRD (Q1); this PRD works with the pinned engine.
- **Resuming a record whose owner is the pane's first process.** Paneflow always starts a shell first (`runtime.rs:1112-1156`), so the owner is the shell itself only when `default_shell` is a reporting program. Such a record is kept and shown but not resumed: starting it as the first process would bypass the shell environment the specification asks for, and no evidence shows the case occurs.
- **Shells without Paneflow integration.** cmd, nushell and any other shell outside zsh, bash, fish and PowerShell get no integration today (`crates/paneflow-launch/src/shell.rs:632-639`). Their records are kept and the resume banner says why it cannot run them; their catalog conversations keep today's typed path (US-022).
- **WSL panes.** The token is not set for them: Linux processes inside WSL cannot be validated by a Windows host, and the specification asks not to forward the token across that boundary (R1, "Security").
- **Paneflow as an OSC 7577 emitter.** The shim and the hook do not send 7577 reports for catalog agents: the hook's IPC path, whose caller identity the kernel attests, already carries what they know (TD-09).
- **Restoring terminal state, shell state, scrollback or environment.** The specification restores only the reported command and its working directory (R1, "Other notes"); Paneflow's existing scrollback and cwd handling are unchanged.
- **Changing the OSC 7501 program status store** (`crates/paneflow-host/src/program_status.rs`). The two protocols keep separate records.
- **A records review page in the desktop.** The specification allows one (R1, "Threat model"); `session inspect` covers review in this version (US-008).
- **User documentation on paneflow.dev.** `docs/user/` is a mirror synced from the site repository; the user guide is written there after this PRD (Q4).
- **Changing the 250 ms spacing between resumes** (`conversation_restore.rs:15`), whose rationale is not documented.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the full suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in a unit test
- `bash scripts/check-helper-sizes.sh` - the shim, hook and MCP helpers stay under their caps (run after nextest on Linux x86_64)
- `cargo deny check advisories licenses sources` - when a dependency changes

## Research Findings

| # | Finding | Source | Shapes |
|---|---------|--------|--------|
| R1 | OSC 7577 revision 0.5 (2026-11-25); its revision history lists five revisions, from 0.1 (2026-11-18) to 0.5. Reports are `OSC 7577 ; key=value(:key=value)*:mac=<64 hex> ST`, parsed strictly. `pid`, `time` and `mac` are required, and `mac` comes last. `argv` (base64 per element, comma-joined) or `clear=1`, optional `cwd`, optional `need`. `mac` = SHA-256 of the body before `:mac=`, then `:`, then the token. The token (22 to 64 chars of `[A-Za-z0-9_-]`, random, new per terminal start, never shared or relayed) is set in `OSC7577_TOKEN` of the first process. The time window SHOULD be 60 s; a report older than the last accepted one for the same pid is ignored. Ignored reports change nothing; rejected ones remove the owner's records. Minimum limits are 2048 bytes and 64 elements. A record is cleared when its owner exits, with "a second or two" of tolerance at terminal end. Records are kept when the app quits and discarded when the user closes the terminal. Resume runs in the user's shell after its startup files, at its first prompt, as a foreground job, with arguments passed as data used once and never as shell source. Ancestors are preferred over descendants; a failed resume is not retried. Records are as private as a startup file and saved on every change. Displayed commands MUST be escaped; OSC 7 SHOULD NOT be trusted for the cwd. | [Program Resume Protocol](https://www.superlogical.com/rex/docs/build/program-resume) | Every TD |
| R2 | Mitchell Hashimoto, 2026-11-25 22:10 UTC: Rex resumes programs through a server restart, based on OSC 7577, co-designed with Herdr and reviewed by several terminal emulator and terminal program authors. | [x.com/mitchellh](https://x.com/mitchellh/status/2125713695233970521) (read 2026-11-26) | Why now |
| R3 | OSC 7501 reached libghostty in PR #14560 (merged 2026-10-06, by mitchellh, "motivated by my work on Superlogical"): the embedder keeps the records and the terminal answers the support query only while the embedder enables it. No Ghostty PR, issue or discussion mentions OSC 7577 as of 2026-11-26. | [ghostty#14560](https://github.com/ghostty-org/ghostty/pull/14560), GitHub search | TD-02, Q1 |
| R4 | `earendil-works/pi` PR #12557, merged 2026-11-25: Pi reports `pi [--session-dir D] --session <id>` with its cwd, signed with `OSC7577_TOKEN`, and sends `clear` on quit and on `/new`. This is the catalog's Pi (`npm_package = "@earendil-works/pi-coding-agent"`, `runtimes/pi/runtime.toml`). The argv shape comes from the PR summary; the diff was not read (A3). | [pi#12557](https://github.com/earendil-works/pi/pull/12557) | TD-12, US-024 |
| R5 | The pinned libghostty-vt (`1.4.1-dev+108cacb17`) has `GhosttyTerminalUnknownSequenceFn`. It reports each complete OSC it does not implement with its raw content, a `truncated` flag and its terminator, during `ghostty_terminal_vt_write` and in order with the stream. It reports nothing until `GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES` is nonzero, and never reports a sequence cut short by CAN, SUB or another control byte. `bindings.rs` is byte-identical for the four targets. | `native/libghostty/prebuilt/x86_64-pc-windows-msvc/include/ghostty/vt/terminal.h:580-725,2462-2480`, md5 of `native/libghostty/prebuilt/*/bindings.rs` | TD-02, US-006 |
| R6 | OSC 88, the earlier Terminal Resume Protocol, has no authentication and passes arguments as one string. | [otty-sh/osc-88](https://github.com/otty-sh/osc-88) | TD-01 (why not OSC 88) |
| R7 | Embedded helper sizes, measured on the 2026-11-24 `release-min` build: shim 402,744 B (hard cap 524,288 B, margin 121,544 B), ai-hook 373,328 B (hard cap 384,000 B, margin 10,672 B, over its 307,200 B soft cap), mcp 516,960 B (margin 7,328 B). | `bash scripts/check-helper-sizes.sh` | TD-08 |

Ecosystem facts (R2 to R4) come from web search and the pages named, on 2026-11-26. Claude Code, Codex CLI and OpenCode trackers were not read directly; no emission by them was found.

## Technical Decisions

**TD-01: `paneflow-host` is the OSC 7577 terminal.** Options: the host, the desktop, the worker. The token must be set in the first process's environment and verified against reports from the same PTY. The host spawns that process (`runtime.rs:1130-1156`) and reads the PTY (`runtime.rs:1279-1299`), and it outlives the desktop. The desktop runs its own libghostty instance on the replayed stream (`src-app/src/terminal/ghostty_session/events.rs:326-328`) and never handles 7577. OSC 88 is not implemented: it is unauthenticated (R6).

**TD-02: receive reports through the unknown-sequence callback, guarded against a libghostty bump.** The callback gives the raw content, the truncation flag and the stream order the specification needs (R5). The host already caps unknown sequences at 4096 bytes (`crates/paneflow-terminal-ghostty/src/options.rs:11`), above the 2048-byte minimum (R1). A libghostty that implements 7577 would stop reporting it as unknown (R5, "Sequences libghostty-vt implements"), so a test feeds a signed report through the real engine and fails if the callback does not fire.

**TD-03: a pure, I/O-free crate, `crates/paneflow-program-resume`.** It holds parsing, MAC and time verification, validation, the record store and record choice, with injected clock and process facts. It uses `sha2` 0.10 (already used by six workspace crates) and `base64` 0.22 (a direct host dependency). Parsing reports typed malformation kinds and never decides an outcome. Verification then classifies, in the specification's order: a MAC or time failure is `Ignored` whatever else is wrong, a `mac` not last or a `?` body is `Ignored`, and a malformation in a report that passed both checks is `Rejected`. The MAC comparison is a constant-time equality. Its unit tests replay the specification's valid and invalid examples, whose MACs are real for the token `dummy-token-replace-me` (R1, "Examples"). Property tests use `proptest` 1.12.0, already in `Cargo.lock`. A `cargo fuzz` target lives beside the existing ones (`fuzz/fuzz_targets/`); the CI fuzz lane runs a fixed matrix gated on engine changes (`.github/workflows/libghostty-linux.yml:307-330`), so the new target is local only (US-004).

**TD-04: the token.** 43 characters, the unpadded base64url encoding of 32 bytes from `getrandom` 0.4 (in `Cargo.lock`, used by `paneflow-serve`, `handles.rs:9`). That is 256 bits, inside the 22-to-64-character range (R1). A new token is generated for every spawned generation (create and restart). It lives in the host session's memory only. It is injected into the spawn command, never into `spec.env`, because the manifest persists `launch.env` (`crates/paneflow-host/src/manifest.rs:122`). An inherited `OSC7577_TOKEN` is added to the strip list every pane child gets (`crates/paneflow-host/src/env.rs:73-82`). It is not set for WSL panes or when `terminal.program_resume.mode` is `off`. On Windows it stays off behind a gate until US-027 proves that ConPTY carries the sequence intact (A1), so no Windows program reports to a host that could misread it.

**TD-05: owner validation stronger than the specification's SHOULD.** A report is accepted only when its pid:
- is alive;
- runs as the host's user;
- belongs to the pane: on Unix its session id is the pane's session, since every pane child is a session leader (portable-pty `setsid`, cited at `tasks/prd-ipc-caller-identity.md`, TD-01); on Windows it is in the pane's job object (`shared.contain`, `runtime.rs:1156`).

The host records the owner's start time (`paneflow_peer::process_start_time`) and its ancestor chain up to the pane's first process, each with a start time. After the host dies, the ancestor rule (R1, "Record choice") can then still be applied from the stored chain. A tmux server or any daemon outside the pane session fails membership, so a nested multiplexer that keeps relaying Paneflow's token cannot register records.

The record's working directory comes from the first source available, in the specification's order (R1, "Working directory"):
1. the report's `cwd` key;
2. the owner's directory read from the OS at acceptance, where a documented API exists: `/proc/<pid>/cwd` on Linux and `PROC_PIDVNODEPATHINFO` on macOS, both already used by the app (`src-app/src/terminal/pty_session.rs:664-727`);
3. the pane's spawn directory.

Windows has no documented API for another process's directory, so its records use the report's `cwd` or the spawn directory. OSC 7 is never used, because any displayed text can set it (R1). The record keeps which source it came from, because only a missing `cwd`-key directory blocks a resume.

**TD-06: owner lifetime from the kernel, removal deferred two seconds.**
- **Exit notification:** `pidfd_open` and `poll` on Linux (5.3 or later, below every target kernel per `tasks/prd-ipc-caller-identity.md` R2), `kqueue` `EVFILT_PROC` `NOTE_EXIT` on macOS, a `SYNCHRONIZE` process handle on Windows.
- **Deferred removal:** when an owner exits, its record is marked and removed two seconds later, the specification's "a second or two" (R1). A host that dies inside that window leaves the record in place, which is exactly the "exited shortly before the terminal ended" case, with no clock comparison at restore.
- **Kept:** records survive every `host.shutdown`, forced or not, and the host's death. "Stop host and restart" reaches the old host as a forced `host.shutdown` (`crates/paneflow-host/src/bootstrap.rs:253`), so an upgrade keeps them.
- **Discarded:** records are discarded by `session.stop` and `session.remove`, which is how the user ends a session on purpose: closing a pane, or "Stop everything and quit", which stops sessions one by one before its shutdown (`ARCHITECTURE.md:1347`). "Stop everything and quit" departs from the specification's "keep it when the user quits the application". Paneflow's default quit keeps every program alive, so the explicit stop is an intent to end them (Q2).
- **Not touched by other sequences:** RIS, DECSTR, alternate screen, erase and OSC 133 leave records as they are (R1).

**TD-07: records live in the session manifest.**
- A new optional `program_resume` field, read with serde defaults, so older and newer hosts read each other's manifests.
- Changes go through the existing persistence service as `Metadata` writes, coalesced per session and started within 250 ms when storage is healthy (`ARCHITECTURE.md:1460-1466`), so no PTY or runtime thread writes a file.
- Manifest retention already bounds a record's life after host death: 30 days for `lost`, 24 hours for `exited` (`crates/paneflow-host/src/host.rs:262-275`).
- The manifest temporary file is created with mode 0600 instead of being chmodded after the write (`manifest.rs:232-249`), because a record's arguments may hold secrets (R1).

**TD-08: a resume launcher, not the shell, executes the command.** Options:
- (a) Type the command, as today. This builds shell source from arguments, which R1 forbids.
- (b) Have each shell read the arguments from a file and run them. Exactness then differs per shell: bash's `command` still runs builtins, and Windows PowerShell 5.1 passes native arguments with its legacy quoting.
- (c) A launcher binary started by the shell at its first prompt. It reads the arguments as data, resolves the program on the `PATH` it inherited from the shell after the shell's startup files, and on Unix replaces itself with the program (`exec`, argv[0] preserved), so the program keeps the launcher's pid and becomes the shell's foreground job. On Windows it spawns and waits, as the shim already does.

(c) is chosen: one exact implementation for every shell, and the shell only ever runs a fixed command. The launcher is a mode of `paneflow-shim`, selected by file name as the shim already selects its tool (`crates/paneflow-shim/src/detect.rs:6-14`). The shim has a 121,544-byte margin, against 10,672 for the hook (R7), and already holds the exec and spawn code (`crates/paneflow-shim/src/exec.rs`).

The handoff file is written by the host into the session directory (`PANEFLOW_SESSION_DIR`), mode 0600, under a random name. It is UTF-8 text with LF line ends, one field per line, values in standard base64 so no value can break the format:

```
paneflow-resume 1
attempt <32 lowercase hex characters>
cwd <base64 of the absolute path> <cwd source: report | owner | spawn>
arg <base64 of argv[0]>
arg <base64 of argv[1]>
...
```

The launcher refuses a file whose first line is not `paneflow-resume 1`, that has no `arg` line, or that holds an unknown field. Before starting the program it changes to the `cwd` directory. If that fails, it reports `cwd_missing` when the source is `report`, and otherwise stays in the shell's directory. The shell is also spawned in that directory, so the prompt that returns after the program exits is there. zsh, bash and fish receive its path in `PANEFLOW_RESUME_HANDOFF`. Each integration script copies the variable into a shell-local value and unsets it before the user's startup files run, since the integration runs first for these three shells (`shell.rs:6-13,55-56,611,618-621`). PowerShell gets the path inside its per-spawn `-Command` string, because its profile runs before that string (`shell.rs:685-692`).

**TD-09: attempts are reported through the hook IPC path.** The launcher spawns `paneflow-ai-hook resume-attempt`, as the shim already spawns the hook for its own events (`crates/paneflow-shim/src/main.rs:187-222`). The hook calls a new host method, `session.resume_attempt`, with:

```json
{"attempt": "<32 hex>", "outcome": "started", "pid": 4242, "start_time": 1795543201000}
{"attempt": "<32 hex>", "outcome": "failed", "reason": "not_found | not_executable | cwd_missing | handoff_invalid"}
```

`start_time` is the value `paneflow_peer::process_start_time` returns for the pid. The host authorizes it as a `Pane` principal of the same session, whose identity `paneflow-peer` attests, and binds it with the attempt id written in the handoff. On `started`, the record's owner becomes the new process (pid and start time), as R1 requires ("After a resume, the record belongs to the process the terminal started"). On `failed`, the record is dropped, except for `cwd_missing`, which keeps it blocked (TD-10). Emitting a signed 7577 report from the launcher instead would need the old owner's pid, whose validation fails once it is dead.

**TD-10: what counts as a failed resume.** R1 leaves the definition to the terminal.
- **Failed:** the launcher could not start the program (`not_found`, `not_executable`, `handoff_invalid`). The record is dropped.
- **Blocked:** the record's `report` directory is missing, found by the host before the restart or by the launcher at start (`cwd_missing`). The record is kept, is never resumed while the directory is missing (R1, "Working directory"), and the pane shows it as blocked with Dismiss.
- **Unsettled:** an attempt is unsettled from `started` until the resumed owner sends a new accepted report, or until the host receives the first `session.input` after the `started` report. The banner's typed launcher command (TD-13) is written before the launcher runs, so it can never settle its own attempt.
- **Never retried:** at the next restore an unsettled record is never resumed automatically, only offered. A host crash caused by the resumed program cannot loop.

No time threshold is used, because no evidence supplies one.

**TD-11: policy.** `terminal.program_resume.mode`:
- `ask` (default): show a banner, run nothing;
- `auto`: resume every resumable record;
- `off`: no token is set and no OSC 7577 record is resumed or offered.

The mode governs OSC 7577 records only. Catalog conversations stay governed by `agents.restore_conversations`, and use the handoff (TD-13) in every mode.

The desktop reads a session's candidate from the `SessionSummary` that the attach resolution already returns (`host_link::resolve`); the `session.inspect` IPC method and the `paneflow-host session inspect` command print the same summary. The host adds a `program_resume` object to `SessionSummary` with:
- `record` (an id), `argv`, `cwd`, `cwd_source`, `owner_pid`, `accepted_at_ms`;
- `settled` (a boolean);
- `blocked` (`null`, or one of `cwd_missing`, `shell_unsupported`, `sandbox_changed`, `first_process`).

Dismiss calls a new `session.discard_resume { record }`.

Each restored pane holds one desktop-side resume state, kept beside the existing conversation state of the terminal view:
- `None`;
- `Resuming { attempt }`;
- `Offered { record, reason }`, where `reason` is `untrusted`, `unsettled`, `agent_shape` (argv matches no `native_argv` template) or `agents_disabled`;
- `Blocked { record, reason }`, where `reason` is one of the summary's `blocked` values;
- `Failed { reason }`, with a `session.resume_attempt` failure reason.

The banner renders `Offered`, `Blocked` and `Failed`.

`terminal.program_resume.trusted_programs` lists bare program names resumed without asking in `ask` mode. A record whose argv[0] is an absolute path is never trusted by name, because `/tmp/x/nvim` would otherwise pass as `nvim`. The default is `ask` because a record is a command run as the user without the user typing it (R1, "Threat model"), and Paneflow's product rule keeps a human in the loop for agent actions.

**TD-12: agent records follow the agent rules.** A record whose argv[0] is a bare name in a runtime's `detection.command_aliases` belongs to that runtime. It is governed by `agents.restore_conversations` (default `true`, `crates/paneflow-config/src/schema/agent_panel.rs:49,61-62`) and resumes automatically only when its argv matches one of the runtime's new `[resume] native_argv` templates. Otherwise it falls back to the banner. Pi declares `["pi", "--session", "{session_id}"]` and `["pi", "--session-dir", "{absolute_path}", "--session", "{session_id}"]` (R4, A3). In a template, `{session_id}` matches an element accepted by `is_valid_session_id` (`src-app/src/agent_sessions.rs:423-430`) and by the runtime's `session_id_pattern` (`src-app/src/agent_resume.rs:38-45`), and `{absolute_path}` matches an element that is an absolute path. Every other template element must equal the record's element exactly, and the lengths must be equal. A pane holding both a 7577 record and a catalog `agent_session` resumes the 7577 record only, since the program stated its own command (R1, "Motivation").

**TD-13: catalog conversations use the handoff where a shell has integration.** The automatic conversation resume hands its catalog command to the launcher instead of typing it, keeping the 250 ms queue, the folder check and the failure markers. The `OfferResume` banner and every shell without integration keep a typed path. The banner gets a handoff from `session.prepare_resume`, which takes either `{ record }` for an OSC 7577 record or `{ argv, cwd }` for a catalog conversation, and types only the launcher's absolute path and the returned handoff path, never the arguments. Shells without integration keep today's typed catalog command.

**TD-14: agents without native reporting do not get the token.** A runtime declares `[integration] program_resume = "native"` when it reports by itself, which is only Pi today (R4). The key is optional, and its only value is `"native"`: a runtime file without it, which is every runtime other than Pi, is non-native without being edited. For every non-native runtime, the shim removes `OSC7577_TOKEN` from the agent's environment. The agent's tools, such as Claude Code's Bash tool, then cannot register a command, which closes the persistence path described in R1's "fewer rights" case. Pi keeps the token. Whether Pi strips it from its own tools is A4.

## Assumptions & Constraints

| # | Assumption | Risk | Validated by |
|---|------------|------|--------------|
| A1 | The bundled ConPTY passes OSC 7577 to the host byte for byte. Inferred from the OSC 7501 tests that emit through PowerShell (`src-app/tests/cli_delivery.rs:182`). | High | US-027 |
| A2 | The pinned libghostty reports a whole 7577 sequence through the unknown-sequence callback, in stream order, on every target (R5 states it; no test exists). | High | US-006 (Linux), US-028, US-030 |
| A3 | Pi's report passes Paneflow's validation (Pi's own pid, inside the pane session, under the shim), and its argv takes exactly the two shapes in TD-12. | High | US-024 |
| A4 | Pi removes `OSC7577_TOKEN` from the tools it runs. Unknown. TD-12's argv templates bound the damage: a forged Pi record with another shape only gets the banner. | Medium | US-024 records the observed behavior |
| A5 | 16 records per session are enough: the reporting processes alive in one pane are a wrapper, a program and nested programs. No evidence; the least recently updated record is evicted beyond 16. | Low | Revisit on a user report |
| A6 | zsh, fish and PowerShell consume the handoff as bash does. CI runners install only bash for these tests (`.github/workflows/run_tests.yml` installs no zsh or fish). | Medium | US-015 (local zsh and fish), US-029, US-031 |
| A7 | On Windows, the default ACL of the user profile keeps the handoff file and the manifest private: no explicit ACL is set today (`manifest.rs:232-249`). | Medium | US-028 |
| A8 | A same-user process can watch a non-child's exit: `pidfd_open` on Linux, `EVFILT_PROC` on macOS, `OpenProcess(SYNCHRONIZE)` on Windows. | Medium | US-009 (Linux), US-028, US-030 |

Constraints: the render thread never blocks (`AGENTS.md`), so every desktop read of a record arrives through the host link; no comments in Rust, shell or PowerShell sources (`AGENTS.md`); `#[cfg(windows)]` code is linted only by the Windows CI leg, so new Windows items go before any `mod tests`.

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior | User message |
|----------|---------|-------------------|--------------|
| Report replayed from a recording | `cat` of a captured session | Ignored: wrong token or time outside 60 s | None |
| Report from inside a nested tmux | tmux relays Paneflow's token | Rejected: the pid is not in the pane session | None |
| Report too long | More than 4096 bytes of content | Ignored as truncated; the earlier record stays | None |
| Owner exits on its own | User quits nvim without `clear` | Record removed 2 s after the exit | None |
| Host killed while owners run | `kill -9` of the host, reboot | Records kept; the next restore applies the policy | Banner or automatic resume |
| `cwd` from the report is gone | Directory deleted before restore or before the launcher starts | Record kept and blocked, never resumed while the directory is missing | "Not resumed: `<dir>` no longer exists", with Dismiss |
| Program no longer on `PATH` | Uninstalled before restore | The launcher reports `failed`; the record is dropped | "Could not resume `<cmd>`: not found" |
| Previous resume unsettled | Host crashed right after a resume | Offered, never automatic | "Resume did not complete last time" |
| Shell without integration | cmd, nushell, `shell_integration: false` | Record kept, not resumable | "This shell cannot resume programs" |
| Session was sandboxed | Record accepted in a sandboxed pane | Resumed only inside the same sandbox profile | "Sandbox changed; not resumed" |
| Bidi override in an argument | `argv` holds U+202E | Banner shows `\u{202E}` | Escaped command |
| User typed before the banner action | Keys sent to the fresh shell | Banner stays; its button resets the line first, as `OfferResume` does today | Unchanged |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| A prompt-injected agent tool schedules a command for the next start | Medium | High | TD-14 strips the token from non-native agents; TD-11 defaults to `ask`; TD-12 limits automatic agent resumes to declared shapes |
| A libghostty bump implements 7577 and silences the callback path | Medium | High | TD-02 guard test |
| The specification changes (5 revisions in 8 days, R1) | High | Medium | All protocol logic in one crate tested against the specification's vectors (TD-03); `need` keys rejected when unknown |
| ConPTY rewrites or drops the sequence | Medium | High | US-027 before Windows is turned on; the token is not set on Windows until it passes |
| Resume crash loop | Low | High | TD-10 never retries an unsettled attempt automatically |
| A record leaks secrets in its arguments | Medium | Medium | Manifest and handoff created 0600 (TD-07, TD-08); the token never persisted (TD-04) |

## Epics & User Stories

### EP-001: Protocol core

A pure crate decides, for any byte string and any record state, exactly what OSC 7577 revision 0.5 requires.

**Definition of Done:** `crates/paneflow-program-resume` exists with no I/O dependency; every valid and invalid example of the specification produces its documented result in a unit test; the record store's transitions are covered by tests.

#### US-001: Strict report parser
**Description:** As the host, I want a parser that turns a 7577 body into a typed report or a typed malformation so that verification can classify every report the way the specification prescribes.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given the specification's Neovim report body, when it is parsed, then the result carries pid 74877, time 1795543200, argv `nvim`, `-S`, `/home/user/.local/state/nvim/sessions/8f2c1d.vim`, and no cwd
- [x] Given the lazygit example body, when it is parsed, then the cwd is `/home/user/src/project`
- [x] Given a body whose last key is not `mac`, when it is parsed, then it is refused as `MacNotLast`
- [x] Given a body of `?`, when it is parsed, then it is refused as `Reserved`
- [x] Given a body with a repeated key, when it is parsed, then it is refused as `DuplicateKey` and the refusal carries the parsed pid
- [x] Given argv whose first element decodes to `./run.sh`, when it is parsed, then it is refused as `RelativeProgram`
- [x] Given argv whose first element decodes to `-x`, when it is parsed, then it is refused as `DashProgram`
- [x] Given a report holding both `argv` and `clear=1`, when it is parsed, then it is refused as `ArgvAndClear`
- [x] Given a decoded element containing U+0085, when it is parsed, then it is refused as `ControlCharacter`
- [x] Given base64 padding of the wrong length, when it is parsed, then it is refused as `BadBase64`
- [x] Given `need=later`, when it is parsed, then it is refused as `UnsupportedNeed`
- [x] Given an unknown key `foo=bar` without `need`, when it is parsed, then the key is dropped and the report parses
- [x] Given `argv=` with no element, when it is parsed, then it is refused as `EmptyArgv`
- [x] Given an empty element after the first, when it is parsed, then it is kept as an empty argument

#### US-002: MAC, time and outcome classification
**Description:** As the host, I want every report verified against the pane's token and the clock before anything else, then classified, so that untrusted output and replays never change state.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given the token `dummy-token-replace-me` and the clock at 1795543200, when each of the specification's four valid reports (Neovim, Neovim `clear`, lazygit, `resume-wrap`) is classified, then each one is `Accepted`
- [x] Given the specification's `clear` example with the right token, when it is classified, then its MAC `c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93` matches
- [x] Given a report signed with another token, when it is classified, then the outcome is `Ignored` and carries no owner
- [x] Given a `DuplicateKey` report signed with another token, when it is classified, then the outcome is `Ignored`, not `Rejected`
- [x] Given a `DuplicateKey` report with a valid MAC and time, when it is classified, then the outcome is `Rejected` with the owner pid
- [x] Given a `MacNotLast` or a `Reserved` body, when it is classified, then the outcome is `Ignored`
- [x] Given a report 61 s older than the clock, when it is classified, then the outcome is `Ignored`
- [x] Given a report 61 s ahead of the clock, when it is classified, then the outcome is `Ignored`
- [x] Given an accepted report for pid P at time T, when a report for P at T-1 arrives, then the outcome is `Ignored`
- [x] Given a `time` value larger than `u64::MAX`, when it is classified, then the outcome is `Ignored`
- [x] Given a valid report whose MAC differs in its last hexadecimal digit, when it is classified, then the outcome is `Ignored`

#### US-003: Record store and record choice
**Description:** As the host, I want one state machine that applies accepted, rejected and ignored outcomes, owner exits and resume attempts so that every record change follows the specification and the TDs.

**Priority:** P0
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [x] Given an accepted `argv` report for pid P, when it is applied, then the store holds exactly one record for P with that argv
- [x] Given a second accepted report for P, when it is applied, then it replaces the first record
- [x] Given an accepted `clear` for P, when it is applied, then P's record is removed
- [x] Given a `Rejected` outcome for P, when it is applied, then every record of P is removed
- [x] Given an `Ignored` outcome, when it is applied, then the store is unchanged
- [x] Given 16 records and a 17th accepted report, when it is applied, then the least recently updated record is evicted
- [x] Given records for a parent and its descendant, when the resume candidate is chosen, then the parent's record is chosen
- [x] Given two unrelated root records, when the candidate is chosen, then the most recently updated one is chosen
- [x] Given an owner exit at time E, when the store is queried at E+1 s, then the record is still present and marked exited
- [x] Given an owner exit at time E, when the deferred removal runs at E+2 s, then the record is gone
- [x] Given a record with an attempt that is neither failed nor settled, when its resume policy input is computed, then it is marked unsettled
- [x] Given a serialized store written by this version, when it is read with an unknown extra field, then reading succeeds

#### US-004: Property tests and fuzz target
**Description:** As a maintainer, I want the parser exercised with generated input so that no byte string makes it panic or accept what the specification refuses.

**Priority:** P1
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given arbitrary byte strings up to 4096 bytes, when the property test runs in nextest, then the parser never panics
- [x] Given generated valid reports re-encoded with one mutated byte before `:mac=`, when they are verified with the original MAC, then none is accepted
- [x] Given `fuzz/fuzz_targets/program_resume.rs`, when `cargo fuzz run program_resume -- -max_total_time=60` runs locally, then it finds no crash
- [x] Given the fuzz crate, when `cargo fuzz list` runs, then `parser`, `snapshot` and `program_resume` are listed, and the CI matrix in `.github/workflows/libghostty-linux.yml` is unchanged

---

### EP-002: The host as the OSC 7577 terminal

Every pane spawned by the host advertises OSC 7577, and the host turns verified reports into durable records with a correct lifetime.

**Definition of Done:** on Linux, a fixture process in a hosted pane sends a signed report, the record appears in `session inspect`, survives a `kill -9` of the host in the manifest, and disappears 2 s after the owner exits on its own; Windows and macOS code paths compile and are proven in EP-007 and EP-008.

#### US-005: Per-generation token in the pane environment
**Description:** As a program running in a pane, I want `OSC7577_TOKEN` in my environment so that I can tell the terminal how to resume me.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a hosted pane, when its shell prints `$OSC7577_TOKEN`, then the value has 43 characters, all in `[A-Za-z0-9_-]`
- [ ] Given two panes, when both print the variable, then the values differ
- [ ] Given a pane restarted to a new generation, when it prints the variable, then the value differs from the previous generation's
- [ ] Given the session manifest on disk, when it is searched for the token, then the token is absent
- [ ] Given `session inspect` and the host log at debug level, when their output is searched for the token, then the token is absent
- [ ] Given a host started with `OSC7577_TOKEN=inherited` in its environment, when a pane prints the variable, then the value is not `inherited`
- [ ] Given a WSL pane on Windows, when it is spawned, then the variable is not set on the `wsl` process
- [ ] Given any Windows pane before US-027 lifts the gate, when it is spawned, then the variable is not set

#### US-006: Ingest reports from the unknown-sequence callback
**Description:** As the host, I want OSC 7577 sequences delivered to the protocol core in stream order so that reports are verified as soon as they are written.

**Priority:** P0
**Dependencies:** Blocked by US-003, US-005

**Acceptance Criteria:**
- [ ] Given a hosted pane, when the fixture writes a signed 7577 report to its terminal, then the host's store holds the record
- [ ] Given the same report terminated by BEL instead of ESC `\`, when it is written, then the record is stored
- [ ] Given a report longer than 4096 bytes, when it is written, then the outcome is `Ignored` and an earlier record of the same owner remains
- [ ] Given a report cut by CAN, when it is written, then nothing is stored
- [ ] Given an unrelated unknown OSC such as `OSC 7400`, when it is written, then the store is unchanged and the pane renders as before
- [ ] Given the pinned libghostty, when the guard test feeds a signed 7577 report through the real engine, then the unknown-sequence callback receives it; the test fails if the engine handles 7577 itself

#### US-007: Owner validation and ancestry capture
**Description:** As the host, I want each report's owner checked against the pane and recorded with its identity so that impersonated, foreign or recycled pids cannot create records.

**Priority:** P0
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given a report whose pid is a live process of the pane, when it is processed, then the record stores the owner's start time and its ancestor chain up to the pane's first process
- [ ] Given a report whose pid does not exist, when it is processed, then the outcome is `Rejected`
- [ ] Given a report whose pid is a live process outside the pane session (a `setsid` child of the fixture), when it is processed, then the outcome is `Rejected`
- [ ] Given a report whose pid is the pane's first process, when it is processed, then the record is stored and marked not resumable (Non-Goals)
- [ ] Given a report without `cwd` on Linux or macOS, when it is processed, then the record's cwd is the owner's cwd read from the OS at acceptance
- [ ] Given a report without `cwd` on Windows, when it is processed, then the record's cwd is the pane's spawn directory with source `spawn`, and OSC 7 is not consulted
- [ ] Given a report with a `cwd` key, when it is processed, then the record's cwd is that directory with source `report`, even when the owner's OS directory differs
- [ ] Given a record whose owner pid was reused by a process with another start time, when a report for that pid arrives, then it is treated as a new owner and the old record is removed

#### US-008: Durable, private record persistence and inspection
**Description:** As a user whose host dies, I want records written into the session manifest as they change so that a restore finds them.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given an accepted report, when the persistence service has run, then the manifest's `program_resume` field holds the record
- [ ] Given a `clear`, when the persistence service has run, then the field no longer holds that record
- [ ] Given a manifest temporary file on Unix, when it is created, then its mode is 0600 from creation
- [ ] Given a manifest written by this version, when a host without the field reads it, then the read succeeds
- [ ] Given a manifest without the field, when this version reads it, then the session has no records
- [ ] Given `paneflow-host session inspect`, when the session has a record, then the output shows argv, cwd, owner pid and acceptance time with control and bidi characters escaped as `\u{XXXX}`
- [ ] Given two accepted reports submitted before the persistence writer takes the session's pending revision, when it writes, then one manifest write holds the second record (`Metadata` coalescing, `ARCHITECTURE.md:1462-1464`)
- [ ] Given an accepted report, when its record change is submitted, then it is submitted as a `Metadata` revision to the persistence service and the runtime thread writes no file

#### US-009: Owner lifetime through kernel exit notification
**Description:** As a user, I want a record to disappear when its program ends on its own and to stay when the host ends so that only the programs I left running come back.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given a record on Linux, when its owner exits on its own, then the exit is observed through a pidfd and the record is removed 2 s later
- [ ] Given an owner that exited 1 s before a `kill -9` of the host, when the next host reads the manifest, then the record is present
- [ ] Given a forced `host.shutdown`, when the owners die from the shutdown, then their records are kept
- [ ] Given the user closes the pane, when the session is stopped, then its records are discarded
- [ ] Given "Stop everything and quit", when it completes, then every record is discarded
- [ ] Given RIS, DECSTR, an alternate-screen switch, an erase and an OSC 133 mark written after a record, when they are processed, then the record remains
- [ ] Given a pidfd that cannot be opened (the owner exited between validation and watch), when the watch is set up, then the record is marked exited and removal is scheduled

---

### EP-003: Resume launcher and first-prompt handoff

A record or a catalog command runs in the user's shell at its first prompt, with every argument unchanged and the attempt known to the host.

**Definition of Done:** on Linux with bash, a host restart with a resume handoff starts the fixture with its exact argv, the host transfers the record to the new pid, and a missing program drops the record.

#### US-010: `paneflow-resume` launcher mode in the shim
**Description:** As the shell integration, I want a launcher that executes a handoff file so that no shell parses resume arguments.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given the shim binary named `paneflow-resume` and a valid handoff file, when it runs, then it deletes the file before starting the program
- [ ] Given a bare argv[0], when the launcher resolves it, then it uses the `PATH` of its own environment, so a `paneflow-shim` agent copy first on `PATH` is the one started
- [ ] Given an absolute argv[0], when the launcher runs, then it starts that path without a `PATH` lookup
- [ ] Given Unix, when the program starts, then it has the launcher's pid and receives argv[0] exactly as written in the handoff
- [ ] Given Windows, when the program exits with code 3, then the launcher exits with code 3
- [ ] Given a handoff whose program is not found, when the launcher runs, then it prints `paneflow-resume: <name>: not found` on stderr and exits 127
- [ ] Given a handoff file not owned by the user or with a mode other than 0600 on Unix, when the launcher runs, then it refuses it and exits 126
- [ ] Given a missing handoff file, when the launcher runs, then it exits 126 without starting anything
- [ ] Given the extract plan, when the app starts, then `paneflow-resume` is written to the helper directory beside the agent shims

#### US-011: Resume attempt reports and ownership transfer
**Description:** As the host, I want to learn when a resumed program started or failed so that the record follows the new process and failures are never retried.

**Priority:** P0
**Dependencies:** Blocked by US-008, US-010

**Acceptance Criteria:**
- [ ] Given the launcher started the program, when the host receives `session.resume_attempt` with outcome `started` and the attempt id, then the record's owner becomes the reported pid and start time
- [ ] Given the launcher could not find the program, when the host receives `session.resume_attempt` with outcome `failed` and reason `not_found`, then the record is removed
- [ ] Given the launcher could not enter the record's `report` directory, when the host receives outcome `failed` with reason `cwd_missing`, then the record is kept and its summary's `blocked` is `cwd_missing`
- [ ] Given a `session.resume_attempt` from a process outside the session, when the host receives it, then it is refused and the record is unchanged
- [ ] Given a `session.resume_attempt` with an unknown attempt id, when the host receives it, then it is refused
- [ ] Given a started attempt, when the resumed owner sends an accepted report, then the attempt is settled
- [ ] Given a started attempt, when the user sends the first input to the session, then the attempt is settled
- [ ] Given a host killed while an attempt is started and unsettled, when the next host reads the manifest, then the record is marked unsettled

#### US-012: Handoff consumption in the shell integration
**Description:** As a user, I want the resumed program to start at my shell's first prompt after my startup files so that it gets my usual environment.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given bash with integration and `PANEFLOW_RESUME_HANDOFF` set, when the first prompt is about to show, then the launcher runs once by its absolute path
- [ ] Given bash, when `~/.bashrc` runs, then `PANEFLOW_RESUME_HANDOFF` is already unset in its environment
- [ ] Given zsh, when `~/.zshrc` runs, then the variable is already unset
- [ ] Given fish, when its config runs, then the variable is already unset
- [ ] Given PowerShell, when the `-Command` string is built for a resume, then it holds the handoff path and no environment variable carries it
- [ ] Given a second prompt, when it shows, then the launcher does not run again
- [ ] Given no handoff, when the first prompt shows, then the integration runs nothing new
- [ ] Given a startup file that changes `PATH`, when the launcher resolves the program, then it uses the changed `PATH`

#### US-013: Host restart with a resume handoff
**Description:** As the desktop, I want to ask the host to restart a lost session with its record resumed so that the handoff, the token and the attempt are created in one step.

**Priority:** P0
**Dependencies:** Blocked by US-011, US-012

**Acceptance Criteria:**
- [ ] Given a lost session with a resumable record, when `session.restart` carries `resume: { record }`, then the host writes a 0600 handoff in the session directory in the TD-08 format and spawns the shell in the record's cwd with it
- [ ] Given the restart, when the manifest is written, then `launch.env` holds neither the handoff path nor the token
- [ ] Given a record whose `cwd` came from the report and no longer exists, when the restart is requested, then the host refuses with reason `cwd_missing` and starts a plain shell
- [ ] Given an unsettled record, when a restart with `resume` is requested without `confirmed: true`, then the host refuses with reason `unsettled`
- [ ] Given a record accepted in a sandboxed session, when the restart's sandbox plan differs, then the host refuses with reason `sandbox_changed`
- [ ] Given a pane whose shell has no integration, when a resume is requested, then the host refuses with reason `shell_unsupported`
- [ ] Given a lost session, when the attach resolution or the `session.inspect` IPC method returns its summary, then it carries the `program_resume` object of TD-11 with `blocked` set when one of these refusals would apply

#### US-014: Live-session and catalog handoff entry points
**Description:** As the desktop, I want handoffs for a running shell and for a catalog conversation so that the banner and the conversation resume never type arguments.

**Priority:** P0
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given a live session and `session.prepare_resume { record, confirmed }` from the window, when it runs, then the host writes a handoff and returns the launcher's absolute path and the handoff path
- [ ] Given `session.prepare_resume { argv, cwd }` from the window, when it runs, then the handoff holds that argv and cwd with source `report`
- [ ] Given `session.prepare_resume` for an unsettled record with `confirmed: false`, when it runs, then the host refuses with reason `unsettled`
- [ ] Given `session.create` with `resume: { argv, cwd }` from the window, when it runs, then the shell starts with a handoff holding that argv
- [ ] Given `session.prepare_resume` or a `resume` on `session.create` from a `Pane` principal, when it runs, then the host refuses it
- [ ] Given `session.discard_resume { record }` from the window, when it runs, then the record is removed from the store and the manifest
- [ ] Given `session.discard_resume` with an unknown record id, when it runs, then the host answers with reason `unknown_record` and changes nothing

#### US-015: Exact argv delivery across shells (validation)
**Description:** As a maintainer, I want proof that arguments reach the program unchanged so that the "never shell source" rule holds in practice.

**Priority:** P0
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given bash in CI and a record whose argv is `paneflow-session-fixture`, `argv-dump`, `$(reboot)`, `a; b`, `two words`, an empty string, `-n`, `naïve` and `*`, when it is resumed, then the fixture's dump holds those 9 elements byte for byte
- [ ] Given a bash function and an alias named like the fixture, when it is resumed, then the fixture binary runs, not the function or the alias
- [ ] Given a fixture copy named `true` first on `PATH`, when a record with argv[0] `true` is resumed under bash, then the fixture copy runs (its dump exists), not the bash builtin
- [ ] Given zsh and fish installed locally on Linux, when the same record is resumed in each, then the dump files are identical to bash's, and the run is recorded in `tasks/program-resume-evidence/` (local evidence)

---

### EP-004: Restore policy and banners in the desktop

After a host loss, each pane with a record either resumes, asks, or explains why it cannot.

**Definition of Done:** with each of the three modes, a restored pane holding a record behaves as TD-11 and TD-12 state, verified by integration tests and the manual pass of US-026.

#### US-016: `terminal.program_resume` configuration
**Description:** As a user, I want to choose whether Paneflow resumes programs, asks first, or never does so that resumes match my trust.

**Priority:** P0
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given no setting, when the config is resolved, then the mode is `ask` and the trusted list is empty
- [ ] Given `{"terminal": {"program_resume": {"mode": "auto"}}}`, when it is resolved, then the mode is `auto`
- [ ] Given `"mode": "sometimes"`, when the config loads, then it reports a validation error naming the key and falls back to `ask`
- [ ] Given `"trusted_programs": ["nvim", "/usr/bin/htop"]`, when it loads, then it reports that `/usr/bin/htop` is not a bare name and keeps `nvim`
- [ ] Given mode `off`, when a new pane prints `$OSC7577_TOKEN`, then it is unset
- [ ] Given the mode changed from `off` to `ask` at runtime, when the next pane is spawned, then the token is set

#### US-017: Restore decision for lost sessions
**Description:** As a user restarting Paneflow after a host loss, I want each pane's record handled by my policy so that the right programs come back.

**Priority:** P0
**Dependencies:** Blocked by US-013, US-016

**Acceptance Criteria:**
- [ ] Given mode `auto` and a lost session with a resumable record, when the layout is restored, then the pane's restart carries `resume` and the program starts
- [ ] Given mode `ask` and a record whose argv[0] is `nvim` in `trusted_programs`, when the layout is restored, then it resumes without a banner
- [ ] Given mode `ask` and an untrusted record, when the layout is restored, then the pane starts a plain shell and its resume state is `Offered`
- [ ] Given an unsettled record in mode `auto`, when the layout is restored, then the pane's resume state is `Offered` with reason `unsettled` and nothing is resumed
- [ ] Given a session ended `exited` with code 0 by the user, when the layout is restored, then no resume is attempted
- [ ] Given mode `off`, when the layout is restored, then no record is resumed or offered
- [ ] Given a summary whose `blocked` is `cwd_missing`, `shell_unsupported` or `sandbox_changed`, when the layout is restored, then the pane's resume state is `Blocked` with that reason and no resume is requested
- [ ] Given several panes to resume, when they restart, then they are submitted in order at least `RESUME_SPACING` apart

#### US-018: Resume offer banner with an escaped command
**Description:** As a user, I want to see exactly which command would run before I allow it so that a misleading record cannot trick me.

**Priority:** P0
**Dependencies:** Blocked by US-014, US-017

**Acceptance Criteria:**
- [ ] Given a pane whose resume state is `Blocked`, when the banner renders, then it shows the Edge Cases message for its reason and no Resume button
- [ ] Given an offered record, when the banner renders, then it shows the argv quoted element by element and the cwd
- [ ] Given an element containing U+202E, when the banner renders, then it shows `\u{202E}` instead of the character
- [ ] Given the banner, when the user clicks Resume while the shell has no input, then the desktop calls `session.prepare_resume` with `confirmed: true` and types only the returned launcher path and handoff path, quoted for the pane's shell, then `\r`
- [ ] Given the banner, when the user has typed into the shell, then Resume resets the line before typing, as `OfferResume` does today
- [ ] Given the banner, when the user clicks Dismiss, then the desktop calls `session.discard_resume` and the banner does not return on the next restore
- [ ] Given the launcher reports `failed`, when the banner state updates, then it shows "Could not resume `<cmd>`: not found"
- [ ] Given the banner and its buttons, when they render, then no element chains a second tooltip on an `accessible_control` (`AGENTS.md`)
- [ ] Given the desktop at debug logging, when a 7577 report passes through the desktop's own engine, then its log line carries the OSC number and length but not the body, so a record's arguments are only ever shown escaped by the banner

#### US-019: Agent records follow agent rules
**Description:** As an agent user, I want a native agent record resumed like a catalog conversation, and preferred over it, so that agents that report themselves need no catalog knowledge.

**Priority:** P0
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [ ] Given a runtime with `[resume] native_argv`, when the catalog is compiled, then each template is validated like `session_argv`, with `{session_id}` exactly once and `{absolute_path}` at most once
- [ ] Given a template with `{session_id}` in program position, when the catalog is compiled, then the build fails with a message naming the runtime
- [ ] Given a Pi record `pi --session abc_1`, `agents.restore_conversations` true and mode `ask`, when the layout is restored, then it resumes without a banner
- [ ] Given a Pi record `pi --extension x.js`, when the layout is restored, then the banner is shown
- [ ] Given `agents.restore_conversations` false, when a Pi record is restored, then it is not resumed automatically and the banner is shown
- [ ] Given a record `/tmp/pi --session abc` (absolute argv[0]), when the layout is restored, then the banner is shown
- [ ] Given a pane with a 7577 record and a catalog `agent_session`, when the layout is restored, then only the 7577 record is resumed and the conversation banner is not shown

#### US-020: The shim strips the token from agents without native reporting
**Description:** As a user, I want tools that agents run unable to register resume commands so that a prompt injection cannot persist past a restart.

**Priority:** P1
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given `runtimes/pi/runtime.toml` with `program_resume = "native"`, when Pi is launched through the shim, then Pi's environment holds `OSC7577_TOKEN`
- [ ] Given Claude Code, whose runtime file has no `program_resume` key, launched through the shim, when its environment is read, then `OSC7577_TOKEN` is absent
- [ ] Given an agent launched through a nested shim chain, when the outermost shim strips the token, then the inner one does not add it back
- [ ] Given an unknown `program_resume` value in a runtime file, when the catalog is compiled, then the build fails naming the runtime

---

### EP-005: Catalog conversations ride the launcher

Catalog conversation resumes stop typing their command into shells that have integration.

**Definition of Done:** with bash and zsh, an automatic Claude conversation resume starts through the handoff with no keystroke written by the desktop; cmd and nushell keep the typed path.

#### US-021: Automatic conversation resume through the handoff
**Description:** As an agent user, I want my conversation resumed at the shell's first prompt without simulated typing so that startup files and slow prompts cannot break it.

**Priority:** P1
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given a restored Claude pane in bash, when the conversation resumes, then the desktop sends `session.create` with `resume: { argv, cwd }` and writes no bytes to the PTY
- [ ] Given the handoff path, when the conversation resumes, then the 300 ms quiet wait is not used
- [ ] Given a resumed Claude conversation that prints `No conversation found with session ID`, when the failure watch runs, then the "Resume failed" banner still appears
- [ ] Given the conversation's folder is missing, when the resume is planned, then the existing `FolderMissing` banner appears and no handoff is written
- [ ] Given two restored conversations, when they start, then their creates are at least `RESUME_SPACING` apart

#### US-022: Offer and fallback paths without argument typing
**Description:** As an agent user on any shell, I want the remaining typed resume paths to stay working so that the migration removes typing only where it is safe.

**Priority:** P1
**Dependencies:** Blocked by US-021

**Acceptance Criteria:**
- [ ] Given the `OfferResume` banner in a bash pane, when the user clicks it, then the desktop types the launcher path and a handoff path, not the agent's arguments
- [ ] Given a cmd pane on Windows, when a conversation resumes, then today's typed command is used unchanged
- [ ] Given a pane with `shell_integration: false`, when a conversation resumes, then today's typed command is used
- [ ] Given the launcher reports `failed` for a conversation, when the banner updates, then the conversation is forgotten, as today's "Resume failed" path does

---

### EP-006: Linux end to end and documentation

The full path is proven on Linux with the real host, a real shell and a real Pi, and documented for maintainers.

**Definition of Done:** US-023 passes in CI; US-024 and US-026 are recorded with evidence; ARCHITECTURE.md describes the protocol path.

#### US-023: Host crash end-to-end test
**Description:** As a maintainer, I want a test that kills the host and resumes a program so that a regression anywhere on the path fails CI.

**Priority:** P0
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [ ] Given a hosted bash pane running the fixture in `resume-report` mode, when the host is killed with SIGKILL and a new host restarts the session with `resume`, then the fixture runs again with its reported argv
- [ ] Given the resumed fixture, when it reports again, then the record's owner is its new pid and the attempt is settled
- [ ] Given the resumed fixture's argv[0] removed from `PATH` before the restart, when the restart runs, then the record is removed and the pane shows a plain shell
- [ ] Given the test, when it waits for outcomes, then it uses deadlines measured in seconds and no `elapsed()` budget (`scripts/check-test-wallclock.sh`)

#### US-024: Pi live validation on Linux (validation)
**Description:** As a maintainer, I want Pi's real reports checked against Paneflow so that A3 and A4 are settled before relying on them.

**Priority:** P0
**Dependencies:** Blocked by US-019

**Acceptance Criteria:**
- [ ] Given Pi started from a Paneflow pane, when it reports, then the host accepts the report and stores Pi's own pid as owner
- [ ] Given Pi's records, when their argv is read, then each matches one of the two `native_argv` templates, or the templates are corrected and the PRD changelog says so
- [ ] Given the host killed with SIGKILL, when Paneflow restores, then Pi resumes the same session automatically
- [ ] Given Pi's `/new`, when it runs, then the previous record is cleared and a new one is accepted
- [ ] Given a Pi tool command printing `$OSC7577_TOKEN`, when it runs, then the observed value (present or absent) is recorded under A4
- [ ] Given the run, when it ends, then the evidence (Pi version, records, logs without the token) is in `tasks/program-resume-evidence/` (local evidence)

#### US-025: Architecture and threat-model documentation
**Description:** As a maintainer, I want ARCHITECTURE.md to explain the 7577 path so that later changes keep its guarantees.

**Priority:** P1
**Dependencies:** Blocked by US-023

**Acceptance Criteria:**
- [ ] Given ARCHITECTURE.md, when it is read, then a section names the token lifecycle, the record store, the persistence class, the launcher and the attempt states with their files
- [ ] Given the lifecycle table at `ARCHITECTURE.md:1344`, when it is read, then each row states what happens to resume records
- [ ] Given a lifecycle row whose event leaves records untouched (a controller pipe that closes), when it is read, then it says "records unchanged" instead of leaving the column empty
- [ ] Given the section, when it is read, then it lists the threat model's accepted risks: same-user processes, Pi's tools (A4), the "Stop everything and quit" departure (TD-06)
- [ ] Given `CHANGELOG.md`, when it is read, then an Unreleased entry describes program resume in user terms

#### US-026: Manual GUI pass on Linux
**Description:** As Arthur, I want to see the banners and resumes in the real app so that the UI meets DESIGN.md before release.

**Priority:** P0
**Dependencies:** Blocked by US-018, US-022

**Acceptance Criteria:**
- [ ] Given a tagged dev home (`scripts/dev.sh --tag resume`) with nvim, lazygit and Pi panes, when its host is stopped with `kill -9` by PID and the app restarts, then each pane behaves per its mode, recorded as screenshots
- [ ] Given the banner on Wayland and X11, when it renders, then it passes the DESIGN.md UI delivery gate
- [ ] Given a failure (program removed), when the pane restores, then the banner names the command and the reason from Edge Cases, and its Dismiss button removes it
- [ ] Given the installed app, when the pass runs, then the installed app's host is never stopped (`AGENTS.md`)

---

### EP-007: Windows hardware proofs

Everything that only Windows can prove, run in one pass on the Windows 11 machine.

**Definition of Done:** US-027 to US-029 pass on Windows 11 x64 hardware with recorded evidence; until US-027 passes, the host does not set the token on Windows.

#### US-027: ConPTY carries OSC 7577 intact (validation)
**Description:** As a maintainer, I want proof that the bundled ConPTY delivers the sequence byte for byte so that Windows can be turned on (A1).

**Priority:** P1
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given a PowerShell pane on Windows 11, when the fixture writes the specification's four valid reports, then the host receives each content byte for byte
- [ ] Given a report of 4000 bytes, when it is written, then it arrives whole and is accepted
- [ ] Given the result, when US-027 passes, then the Windows token gate is removed in the same change; when it fails, the gate stays and the failure is recorded

#### US-028: Windows owner validation, exit notification and private files
**Description:** As a Windows user, I want the host's checks to hold on Windows so that records there are as trustworthy as on Linux.

**Priority:** P1
**Dependencies:** Blocked by US-009, US-027

**Acceptance Criteria:**
- [ ] Given a report whose pid is in the pane's job object, when it is processed, then it is accepted with its creation time
- [ ] Given a report whose pid is outside the job, when it is processed, then it is rejected
- [ ] Given an owner exit, when it is observed through a process handle, then the record is removed 2 s later
- [ ] Given the manifest and a handoff file, when their ACLs are read with `icacls`, then only the user, SYSTEM and Administrators have access, or the missing ACL is added and recorded (A7)

#### US-029: PowerShell handoff and the launcher on Windows
**Description:** As a Windows user, I want resumes to run in pwsh and Windows PowerShell with exact arguments so that Windows behaves like the other platforms.

**Priority:** P1
**Dependencies:** Blocked by US-013, US-027

**Acceptance Criteria:**
- [ ] Given pwsh 7 and Windows PowerShell 5.1, when the US-015 record is resumed, then the dump holds the same 9 elements as US-015
- [ ] Given a resumed program, when the user presses Ctrl+C, then the program receives it and the launcher stays alive until the program exits
- [ ] Given the program exits with code 3, when the launcher returns, then `$LASTEXITCODE` is 3
- [ ] Given a cmd pane, when a record exists, then the banner says the shell cannot resume programs

---

### EP-008: macOS hardware proofs

Everything that only a Mac can prove, run in one pass on the rented Mac.

**Definition of Done:** US-030 and US-031 pass on macOS hardware with recorded evidence.

#### US-030: macOS owner validation and kqueue exit
**Description:** As a macOS user, I want the host's checks to hold on macOS so that records there are trustworthy.

**Priority:** P1
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given a report from a process of the pane session, when it is processed, then it is accepted with its `proc_pidinfo` start time
- [ ] Given a report from a `setsid` child, when it is processed, then it is rejected
- [ ] Given an owner exit, when it is observed through `EVFILT_PROC`, then the record is removed 2 s later
- [ ] Given a signed report in a pane, when the guard test of US-006 runs on macOS, then the unknown-sequence callback receives it (A2)

#### US-031: zsh handoff, launcher exec and Pi resume on macOS
**Description:** As a macOS user on the default zsh, I want resumes to work end to end so that macOS ships the feature.

**Priority:** P1
**Dependencies:** Blocked by US-013, US-019

**Acceptance Criteria:**
- [ ] Given zsh, the default macOS shell, when the US-015 record is resumed, then the dump holds the same 9 elements as US-015
- [ ] Given a resumed program, when its pid is read, then it equals the launcher's pid (exec)
- [ ] Given Pi on macOS, when the host is killed and Paneflow restores, then Pi resumes its session
- [ ] Given a failure to start, when the launcher reports it, then the record is removed

## Open Questions

- **Q1:** Should Paneflow propose an OSC 7577 option to libghostty-vt, on the model of the 7501 option (R3)? It would remove TD-02's guard but move MAC verification into the engine or keep it in the embedder. Depends on Ghostty maintainers' interest; to be decided after EP-006.
- **Q2:** Should "Stop everything and quit" keep records, as the specification suggests for an application quit? TD-06 discards them because the default quit keeps programs alive. Arthur may overturn; US-009's fifth criterion changes with it.
- **Q3:** Is `ask` the right default once trusted programs exist, or should catalog-free records like `nvim -S <session>` resume automatically? Depends on feedback after US-026.
- **Q4:** The user guide for paneflow.dev (settings, threat model, how a program reports) is written in the site repository after EP-006.

## Files NOT to Modify

- `native/libghostty/**`: pinned engine archives and headers; this PRD uses the existing ABI (TD-02)
- `vendor/**`: vendored upstream crates
- `docs/user/**`: a mirror synced from the site repository (Q4)
- `crates/paneflow-host/src/program_status.rs`: OSC 7501 keeps its own store (Non-Goals)
- `crates/paneflow-host/src/sandbox/**`: no sandbox change; a change would require `scripts/sandbox-matrix.sh`
- `.github/workflows/**`: no CI change is needed; the new fuzz target stays local (TD-03)
- `runtimes/*/runtime.toml` other than `runtimes/pi/runtime.toml`: catalog templates of other runtimes stay as they are
- `crates/paneflow-git/**`: unrelated
[/PRD]
