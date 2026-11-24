[PRD]
# PRD: Caller identity, per-pane grants and sandboxed agent panes for Paneflow's local IPC

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-21 | Arthur Jean | Initial PRD, from the EP-008 review of the Lua scripting PRD (an automated security review flagged `layout.apply` and `layout.sweep`), a code map of the three IPC servers, and two research passes on peer credentials, process containment and agent sandboxes. Three layers: kernel-attested caller identity with one authorization point, per-pane grants held by the worker, opt-in OS sandboxes for agent panes; plus a real-hardware epic. 5 epics, 33 stories. |
| 1.1 | 2026-11-23 | Arthur Jean | EP-004: Landlock pathname-socket scoping (ABI 9) becomes an opt-in, `sandbox_unix_scope: true`, because a Landlock domain that handles file-system rights forbids mounts and so breaks the bubblewrap that Claude Code and Codex start inside the pane; ABI 6 abstract-socket scoping stays on for every sandboxed pane. US-025's second criterion is rewritten to match, and the Linux sandbox report (US-023) is added. |
| 1.2 | 2026-11-23 | Arthur Jean | EP-004 review: the Linux sandbox no longer re-exposes the D-Bus session bus, the systemd user sockets and the accessibility bus (`bus`, `systemd`, `at-spi`, `.dbus-proxy` in the runtime directory). With them a sandboxed process started a process outside the sandbox with `systemd-run --user`, measured on Fedora 45, which then saw every shared endpoint. Known remaining limits (X11, other command-running sockets, a local SSH server) are in `docs/user/scripting.md`; a filtering D-Bus proxy is an open question. |
| 1.3 | 2026-11-23 | Arthur Jean | The stories that need a Windows machine (US-027, US-030, US-033) move to a final epic EP-006, and those that need a Mac (US-026, US-029, US-032) to a final epic EP-007. EP-004 keeps the endpoints, the Linux sandbox and US-028; EP-005 keeps the Linux kernel matrix (US-031). Story ids, criteria and dependencies are unchanged. 7 epics, 33 stories. |
| 1.4 | 2026-11-23 | Arthur Jean | EP-004 closed. The Linux sandbox report is measured on five distributions in KVM virtual machines (`scripts/sandbox-matrix.sh`): Ubuntu 24.04 is unsupported as installed. The runtime directory of a sandboxed pane is an allowlist (Wayland, PipeWire, PulseAudio, GnuPG, the SSH agent), replacing v1.2's list of refused entries. The config offers `sandbox: "paneflow"` on Linux only until EP-006 and EP-007 validate Windows and macOS, so US-028 no longer waits for them; US-032 and US-033 each gain the criterion that turns their platform on. The CLI now reaches the worker through the pane endpoint from a sandboxed pane (`paneflow split` failed there). |
| 1.5 | 2026-11-23 | Arthur Jean | EP-005: the Linux kernel matrix report (US-031) is measured in KVM virtual machines (`scripts/sandbox-matrix.sh --suite identity`) on Ubuntu 22.04 (5.15, `SO_PEERCRED`) and 24.04 (6.8, `SO_PEERPIDFD`). It found that with `/proc` mounted `hidepid=2` every caller was `unverified`, because the parent chain reached a process the host cannot see (`sshd-session`, `init`); an ancestor that exists but is hidden now ends the chain, as `init` does. A8 and A12 are validated. |
| 1.6 | 2026-11-24 | Arthur Jean | EP-006, measured on Windows 11 Pro (build 26300): the Windows layer-1 report (US-030) and the Windows sandbox report (US-033). A2 and A3 are validated. The restricted token's restricting SIDs are the per-pane SID, which the pane's own data grants, a shared sandbox SID that the working directory grants, the logon SID (the desktop refuses a process without it, `0xC0000142`) and Everyone (the `NUL` device grants nothing else); the pane gets its own `TEMP` instead of a grant on the user's, whose access list every launch rewrote in more than 9 s; pane pipes grant the user's SID instead of `OWNER RIGHTS`, which a restricted token never matches. Windows stays unsupported for the sandboxed profile: a named pipe made inside the sandbox without an access list cannot be opened for writing there, so Node and Bun (Claude Code's Bash tool and MCP servers) cannot start a piped child. |
| 1.7 | 2026-11-24 | Arthur Jean | EP-007, measured on macOS 27.2.1 (build 26B131) on Apple Silicon (M4): the macOS layer-1 report (US-029) and the macOS sandbox report (US-032). A5 and A9 are validated, A7 is validated with a correction and A6 is disproved on macOS: Seatbelt refuses a second profile inside a sandboxed process (`sandbox_apply: Operation not permitted`), so neither Claude Code's nor Codex's own sandbox can start in a Paneflow pane, and macOS stays unsupported for the sandboxed profile. The Seatbelt profile now denies `signal` and `process-info*` to every process outside its own sandbox (`(target others)` skipped the processes of the caller's own session), and the host checks the profile with `sandbox-exec` before the pane starts, so a refused profile is the pane's launch failure. Not measured: an Intel Mac (US-029) and the previous macOS release (US-032). |
| 1.8 | 2026-11-24 | Arthur Jean | US-029's Intel check is deferred until after the merge to `main`, and so were the test failures the EP-007 run found outside the epic (fixed in v1.10). |
| 1.9 | 2026-11-24 | Arthur Jean | US-032 is measured on the previous macOS release as well, macOS 26.6.2 (build 25G83) in a Tart virtual machine on the same Mac: the same answers as on 27.2.1, every filter form included, and Seatbelt refuses a nested profile there too. A5 is validated on both releases. |
| 1.10 | 2026-11-24 | Arthur Jean | The test failures EP-007's run found outside the epic are fixed instead of deferred: two races between tests that share a process (the worktrees root, the editor display preferences, now a GPUI global) one test left stale by EP-009 of the Lua scripting PRD, and one flaky pane client that exited before the test read its report. The macOS CI job does run the tests (`cargo nextest`), contrary to v1.8. |
| 1.11 | 2026-11-24 | Arthur Jean | EP-007 review: a sandboxed macOS pane could rename a shared socket, or the runtime directory that holds it, and connect under the new path; the Seatbelt profile now denies `file-write*` on each shared endpoint, the home and every directory above them (macOS sandbox report). Measured on macOS 27.2.1. |
| 1.12 | 2026-11-24 | Arthur Jean | Amends TD-01 after the macOS qualification: a non-pane peer that names itself the window or the worker (hello client name, engine identity, or `layout.attach`) could approve, type with the gates closed without a journal line, or stop the host. `Window` and `Worker` now also require an executable Paneflow vouches for (this installation's binary or a byte-identical copy, the process that started the server, or the server's own binary); any other claimant is `External` and logged once (`70126a19`, `58985715`). Pane membership stays the layer-1 boundary. The Linux identity matrix (`scripts/sandbox-matrix.sh --suite identity`) still has to run on this change. |

## Problem Statement

Paneflow runs coding agents in its panes, and those agents can drive Paneflow back: the CLI, the MCP bridge and raw sockets reach three JSON-RPC servers (the window, the worker `paneflow-serve`, the terminal host `paneflow-host`). Each server accepts only the user's own processes (owner-only socket `0600`, owner-only pipe SDDL, `crates/paneflow-host/src/server.rs:44,177-222`, `src-app/src/ipc.rs:385,401`), but none of them knows **which** of the user's processes is calling. Every trust decision rests on what the caller says about itself:

1. **The host trusts a self-declared engine.** `ClientHello::attaches()` is `engine.is_some()` (`crates/paneflow-host/src/protocol.rs:116`). A client that claims an engine is treated as the window: its `session.input` is not gated (`crates/paneflow-host/src/control.rs:289-293`) and it may follow and decide write approvals (`crates/paneflow-host/src/server.rs:549-575`). A process in a pane can send that hello and approve its own `write_pane` request, whose `source_session` is also a parameter it supplies (`crates/paneflow-host/src/agent_write.rs:479`).
2. **Scope is a claim.** `scope_session` is a session id the caller supplies, checked only for being hosted; leaving it out reaches the whole instance (`crates/paneflow-host/src/control.rs:348-350,483-505`).
3. **The worker trusts any attached connection.** `layout.attach` attaches whatever connection asks, under the name it gives (`crates/paneflow-serve/src/layout_authority.rs:384-398`). An attached connection skips the spawn gate of `layout.apply` and may run `layout.sweep` with `live: true`, which stops every live session no surface holds; the diff dock's terminals are protected only by the `keep` list the window sends (`crates/paneflow-serve/src/layout_authority.rs:550-640,772-845`, `src-app/src/app/hosted_sessions.rs:300-330`). The automated security review of EP-008 (2026-11-21) flagged both.
4. **The gates disagree across processes.** Each process reads `PANEFLOW_IPC_SCRIPTING` and `PANEFLOW_IPC_ORCHESTRATION` from its own environment (`src-app/src/app/ipc_handler/gates.rs:3-20`, `crates/paneflow-host/src/control.rs:73-82`, `crates/paneflow-serve/src/launcher.rs:120-123`). A worker started by a CLI that had the variable set stays open for its whole lifetime, whatever the window's environment says.
5. **Many host methods have no gate at all:** `session.stop`, `restart`, `remove`, `resize`, `appearance`, `workspace`, `clear_history`, `reset`, `text`, `inspect`, `list`, `runtime.bind` and `host.shutdown` (`crates/paneflow-host/src/server.rs:577,900-1066`).

The only peer credential read anywhere is the window's UID check, which only logs the peer PID (`src-app/src/ipc.rs:776-827,1377-1395`); the worker and the host read none.

The user documentation states the intended limit: the gates "protect a pane from a confused agent, not from a hostile process running as your user" (`docs/user/scripting.md:121-123`). Today they do not hold even against a confused agent that copies a raw-socket recipe, because the decisive facts are claims. The people exposed are users who run coding agents in panes, which is the product's positioning, and in particular users of the conductor workflow, where one agent drives others through `paneflow send` (`docs/user/conductor.md:78`).

**Why now:** EP-008 just moved the structural layout methods to the worker and gave it `layout.launch` and `layout.sweep`, which raised the cost of trusting any attached connection; EP-009 of the Lua scripting PRD (agents ask and act) is next and gives agents more reach into Paneflow, so per-agent authority has to exist first. The OS primitives are usable on every target: `SO_PEERPIDFD` on Linux 6.5+, `LOCAL_PEERTOKEN` supported on macOS 13+, Job Objects with nesting since Windows 8 ([Research Findings](#research-findings)). Codex CLI and Claude Code both ship OS sandboxes for agent commands, which sets the reference for layer 3.

## Overview

Three layers, each usable on its own, each making the next one stronger.

**Layer 1: who is calling (EP-001) and one place that decides (EP-002).** Every connection on every server gets a **principal**, decided from the kernel and not from the hello:

- `Pane { session, workspace }`: the peer process belongs to a pane's process tree. On Linux and macOS a pane is its own session (portable-pty calls `setsid` for every child, `portable-pty-0.9.0/src/unix.rs:257`), so membership is the peer's session id or its parent chain reaching a pane leader. On Windows each pane's tree goes into its own Job Object, which a process cannot leave once breakaway is off ([Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)).
- `Window`, `Worker`, `External`: a peer that is not in a pane. Among these three, the hello (or `layout.attach`) stays the discriminator, because all three are processes of the human: the app, its worker, the user's own terminal (TD-01). The worker's in-process calls (its `ConnectionId::INTERNAL`) are `Worker`, except an `init.lua` action, which runs as the principal that started it.
- `Unverified`: the peer could not be identified. It may call only `system.whoami`, `system.capabilities` and `system.describe`.

The **host** decides pane membership: it spawned every pane and records their leaders (`crates/paneflow-host/src/host/types.rs:289,318`). The window and the worker ask it with a new `peer.classify` call, a query the server makes for itself, not a relay of the client's request (FR-03 of the Lua scripting PRD). When the host is unreachable, they classify from the session manifests the host writes in the home, which the worker already reads (`crates/paneflow-serve/src/state.rs:103`); on Windows, where membership needs the job handle, the peer is `Unverified`. The decision never defaults to "not in a pane" (TD-02). Peer identity is bound to the process, not to a reusable PID: a pidfd on Linux 6.5+ with an `SO_PEERCRED` and start-time fallback, the audit token on macOS, a process handle with its creation time on Windows.

EP-002 enforces the `Gate` each method already declares (`crates/paneflow-api/src/descriptor.rs:25-52`) plus a new **audience** in one function, `authorize(principal, descriptor, params, policy)`, called by the dispatch of the three servers. The policy comes from one `ipc` section of `paneflow.json` instead of three environments (TD-03). For a `Pane` caller:
- the gates apply whether or not its connection attached or claimed an engine;
- its scope is its own workspace, derived from the principal, and a `scope_session` it sends is ignored; reaching another workspace needs the orchestration gate, as `--scope all` does today (`docs/user/scripting.md:128-131`);
- `session.input`, `stop`, `restart`, `remove`, `reset` and `clear_history` follow that scope rule;
- its `write_pane` source is its own session.

`layout.launch`, `layout.sweep`, `approval.follow`, `approval.decide`, `runtime.bind` and `host.shutdown` answer only `Window` and `Worker`; `layout.attach` answers any non-pane caller. The worker decides when a sweep may stop live sessions (once after it loads, re-reads or adopts a layout) instead of reading `live` from the caller.

**Layer 2: what a pane may do (EP-003).** The all-or-nothing switch for panes is replaced by **grants**: records held by the worker, keyed by the `Pane` principal, carrying rights (`write`, `spawn`, `layout`), a scope (own workspace or instance) and an expiry. A grant comes from the human, through a prompt of the worker's prompt broker (EP-006 of `tasks/prd-lua-scripting.md`) or an agent profile in `paneflow.json`; its holder can delegate a narrower grant to a pane it spawned; revoking it revokes what was delegated from it. Grants are not bearer tokens (TD-04). The worker pushes the grant set to the host, whose client it already is (`crates/paneflow-serve/src/launcher.rs:81`), and the window reads grant events from the worker's event log, which it already follows for layout events (`src-app/src/app/layout_bridge/client.rs:24,651-655`).

**Layer 3: identity that cannot be escaped (EP-004).** Layer 1 holds against a confused agent, but a hostile process of the same user can leave a pane's classification (`setsid` plus a double fork on Unix). An agent profile can opt into a sandbox where Paneflow's own endpoints and other panes' processes are out of reach, and where the pane reaches each server through an endpoint that server created for that pane alone: every connection on it is that pane, after Wayland's `security-context-v1` model ([wayland.app](https://wayland.app/protocols/security-context-v1)). Linux uses bubblewrap namespaces (Codex's default, R8) with Landlock scoping where the kernel has it; macOS uses a Seatbelt profile that denies `network-outbound` to Paneflow's sockets (as Anthropic's sandbox-runtime does, R9); Windows uses a write-restricted token with a per-pane SID (Codex's unelevated approach, R8). The sandbox confines access to Paneflow and to other panes only.

EP-005 exercises the Linux kernel matrix. EP-006 holds every story that needs a Windows machine and EP-007 every story that needs a Mac, each run in one pass on real hardware; inside them the layer-1 checks stay separate stories from the sandbox checks, so they never wait for the P2 sandbox.

## Non-Goals

- **A general-purpose agent sandbox.** Layer 3 confines access to Paneflow's endpoints and to other panes' processes. File-system and network policy for agents stays with their own sandboxes (Codex, Claude Code), which must still run inside a Paneflow sandbox (US-023).
- **Defending against a hostile process outside any pane.** A process of the user outside Paneflow can read the same user's memory, environments and files; no layer of this PRD stops it, and the threat model says so (US-015).
- **Restricting reads.** `session.text`, `inspect`, `list`, `resize`, `appearance` and `workspace`, and the MCP `read_pane` and `search_pane` tools, keep their current rules; where a read already takes a scope, a `Pane` caller's scope is derived from its principal, which gives the same result as the scope the MCP bridge sends today (`crates/paneflow-mcp/src/scope.rs:95`).
- **Authentication between machines or users.** Endpoints stay owner-only and local.
- **Bearer tokens** (TD-04).
- **Per-pane cgroups or systemd scopes on Linux.** A delegated cgroup can be left by a same-user process within its subtree (R3), so it would add a D-Bus dependency without adding a boundary; layer 3 uses a cgroup namespace.
- **Changing existing parameters, results or error codes** (FR-02 of the Lua scripting PRD). New refusals carry a new `reason` on the method's existing error code.
- **Running a sandboxed profile unsandboxed.** When the sandbox cannot start, the pane does not start (US-028).

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
| R1 | `SO_PEERPIDFD` (a pidfd for the socket peer) arrived in Linux 6.5; `PIDFD_GET_INFO` (cgroup id, race-free) in 6.13. `SO_PEERCRED` reports the peer as of `connect()`. | [LPC 2024 pidfd slides](https://lpc.events/event/18/contributions/1691/attachments/1499/3166/-LPC-2024-%20PID%20FDs-%20where%20we%20were,%20where%20we%20are%20and%20were%20we%20would%20like%20to%20go.pdf), [pidfd.h v6.13](https://tangled.org/tjh.dev/kernel/blob/v6.13/include/uapi/linux/pidfd.h), [unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html) | US-001 |
| R2 | Ubuntu 22.04 GA kernel is 5.15, Debian 12 is 6.1, openSUSE Leap 15.6 is 6.4, RHEL 9 is 5.14 (a backport is unconfirmed); Ubuntu 24.04, Debian 13, RHEL 10 and Leap 16 run 6.8 to 6.12. | [Ubuntu kernel lifecycle](https://ubuntu.com/kernel/lifecycle), [Debian trixie](https://bits.debian.org/2025/08/trixie-released.html), [Red Hat kernel table](https://access.redhat.com/articles/3078), [Leap 15.6](https://en.opensuse.org/Release_announcement_15.6) | US-001 fallback is mandatory |
| R3 | In a delegated cgroup v2 subtree, the delegatee can move its processes between cgroups of that subtree; only a cgroup namespace rooted at the process's cgroup pins it. | [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) | Non-Goals, US-024 |
| R4 | `LOCAL_PEERTOKEN` (0x006) returns the peer's audit token on macOS, supported on 13+; `audit_token_to_pidversion` can collide, and `proc_pidpath_audittoken` reads the executable from the token. | [xnu sys/un.h](https://git.saurik.com/apple/xnu.git/blob/HEAD:/bsd/sys/un.h), [Apple forum](https://developer.apple.com/forums/thread/850777), [exploit-db 46648](https://www.exploit-db.com/raw/46648), [xnu test](https://git.saurik.com/apple/xnu.git/blob/HEAD:/tests/proc_pidpath_audittoken.c) | US-002 |
| R5 | `GetNamedPipeClientProcessId` exists since Vista; Microsoft does not document whether a client can set it. A process in a Job Object cannot leave it; children join unless breakaway is allowed; nested jobs since Windows 8; `KILL_ON_JOB_CLOSE` ends the tree. | [GetNamedPipeClientProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid), [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects), [Nested jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs) | US-003, US-005 |
| R6 | Windows Terminal removed its job object around ConPTY (PR #2198); third-party reports say `OpenConsole.exe` can escape assignment code that looks only for `conhost.exe`. | [microsoft/terminal#2198](https://github.com/microsoft/terminal/pull/2198), [clud#1367](https://github.com/zackees/clud/issues/1367) | US-030 |
| R7 | Landlock ABI 6 scopes abstract unix sockets; ABI 9 (`LANDLOCK_ACCESS_FS_RESOLVE_UNIX`) restricts `connect()` to pathname sockets created outside the domain and appears in the Linux 7.1 docs. seccomp cannot inspect a `connect()` address. | [Landlock docs](https://docs.kernel.org/userspace-api/landlock.html), [7.1 docs](https://docs.kernel.org/7.1/userspace-api/landlock.html) | US-025 |
| R8 | Codex on Linux defaults to bubblewrap (Landlock is a legacy opt-in) with `no_new_privs` and a seccomp filter that blocks new `AF_UNIX` sockets in proxy mode; on macOS it uses Seatbelt; on Windows it uses dedicated sandbox users or a write-restricted token with a synthetic SID, and rejected AppContainer as too narrow for shells, git and build tools. | [codex linux-sandbox README](https://raw.githubusercontent.com/openai/codex/3a067484584861606ad842de5bc4ac735a865ddf/codex-rs/linux-sandbox/README.md), [Codex sandboxing](https://learn.chatgpt.com/docs/sandboxing), [Codex Windows sandbox](https://openai.com/index/building-codex-windows-sandbox/) | US-024, US-027 |
| R9 | Claude Code sandboxes with bubblewrap on Linux and WSL2, Seatbelt on macOS, nothing on native Windows; Seatbelt needs `(deny network-outbound (remote unix-socket ...))` to block a socket; one report says literal and subpath filters stopped matching on macOS 26.6.2. | [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing), [sandbox-runtime](https://github.com/anthropics/sandbox-runtime/blob/main/src/sandbox/macos-sandbox-utils.ts), [nono seatbelt notes](https://github.com/nolabs-ai/nono/blob/main/docs/cli/internals/seatbelt.mdx) | US-026, US-032 |
| R10 | Wayland `security-context-v1`: the sandbox engine hands the compositor a listening socket tagged with the sandbox's identity; every client on it is that sandbox. | [security-context-v1](https://wayland.app/protocols/security-context-v1) | US-021 |
| R11 | `biscuit-auth` 6.0 offers offline attenuation with ed25519, p256 and protobuf; `macaroon` 0.3 is labelled not secure by its authors. | [biscuit-auth](https://docs.rs/crate/biscuit-auth/latest), [biscuit spec](https://doc.biscuitsec.org/reference/specifications), [macaroon](https://docs.rs/crate/macaroon/latest) | TD-04 |
| R12 | `libc` 0.2.189, already in `Cargo.lock`, defines `SO_PEERPIDFD` and `LOCAL_PEERTOKEN`; `paneflow-ipc-client` already links `windows-sys` 0.59 with `Win32_Security`. | `Cargo.lock`, `libc-0.2.189/src/unix/linux_like/linux/arch/generic/mod.rs:157`, `crates/paneflow-ipc-client/Cargo.toml:10-24` | No new dependency for layer 1 |

Research ran through web search and primary documentation on 2026-11-21. Unconfirmed points are listed under [Assumptions](#assumptions--constraints), each high-risk one with a validation story.

## Technical Decisions

**TD-01: the boundary of layer 1 is "in a pane or not", attested by the kernel.** Options: (a) authenticate the window with a secret, (b) classify peers by executable path, (c) classify by pane membership. A secret in a file or an environment is readable by every process of the user (`/proc/<pid>/environ`, the owner-only home); the CLI is the same executable as the window (`src-app/src/cli/mod.rs`), so (b) cannot tell an agent's `paneflow` call from the window. Pane membership is what separates an agent from the human, and the host already records every pane leader. `Window`, `Worker` and `External` stay self-declared among non-pane peers: they are the human's processes and keep the trust they have today. Amended in 1.12: a non-pane peer holds `Window` or `Worker` only when its executable is one Paneflow vouches for; any other claimant is `External`.

**TD-02: the host decides membership; the others ask it, or read its manifests, and never assume.** Only the host holds the pane leaders and, on Windows, the Job Object handles. The window and the worker call `peer.classify` once per connection and keep the answer for the connection's life. A host that does not answer is no proof that no pane process exists, since a pane process may survive its PTY closing (assumption A12): the caller then classifies from the session manifests in the home (leader PID and start time) on Linux and macOS, and treats the peer as `Unverified` on Windows, where membership needs the job handle.

**TD-03: one authorization function in `paneflow-api`, the policy in `paneflow.json`.** The gate is already declared per method; one function called from the three dispatch paths replaces the separate environment reads the code map found in the window, the host and the worker (Problem Statement item 4). The policy becomes an `ipc` section (`scripting`, `orchestration`), read by the three processes through `paneflow-config`, which the host already depends on (`crates/paneflow-host/Cargo.toml:28`). Each process re-reads the section when the file's modification time changes, checked when a gated request arrives, so no new file watcher is needed. Precedence:
- `External` caller: a gate is open when the `ipc` key is true, or the variable is set in the serving process's environment, or `ai_unrestricted` is true. This keeps today's behavior.
- `Pane` caller: a gate is open when the `ipc` key is true, or `ai_unrestricted` is true (its documented purpose is trusted AI automation, `docs/user/scripting.md:107`), or a grant covers the request (EP-003). During the deprecation release, the environment variable still opens it when the file has no `ipc` section, with a warning; from the next release it does not.
The conductor documentation tells users to set `PANEFLOW_IPC_SCRIPTING=1` for `send --submit` (`docs/user/conductor.md:78`), and the conductor runs as an agent in a pane, so removing the variable for panes without a transition would break them on update.

**TD-04: grants, not tokens.** Options: biscuit tokens, HMAC-chained caveats, server-side grants keyed by principal. A bearer token must sit where the pane can read it, so every process of the user can read it too. The token libraries (R11) solve offline verification across machines, which Paneflow does not need, and biscuit would add ed25519, p256 and protobuf next to helpers capped at 384 000 to 524 288 bytes (`scripts/check-helper-sizes.sh:8-10`). Server-side grants keyed to the kernel-attested principal give attenuation and delegation with no secret to leak, and layer 3 makes the principal unforgeable.

**TD-05: per-pane endpoints on each server, no relay.** A sandboxed pane cannot reach the shared endpoints, yet must call the worker (`grant.request`, layout methods), the host (`session.input`) and the window. One proxy endpoint forwarding to the owners would be a relay, which FR-03 of the Lua scripting PRD forbids. Each server instead listens on one extra endpoint per sandboxed pane, in a directory that only the pane's sandbox maps, and the pane's environment names those endpoints.

**TD-06: Linux sandbox through bubblewrap.** bubblewrap is packaged on every target distribution and is what Codex and Claude Code converged on (R8, R9); writing namespace setup in Paneflow would duplicate it. Landlock is added inside when the kernel has ABI 6 or 9, never required (R7).

**TD-07: Windows sandbox through a write-restricted token with a per-pane SID, not AppContainer.** Codex evaluated AppContainer and rejected it for shells, git and build tools (R8), and its interaction with ConPTY is undocumented. A write-restricted token whose restricting SIDs are not granted by the owner-only pipes cannot open them for writing; the pane's own pipes grant its SID. Validated on hardware in US-033.

## Assumptions & Constraints

| # | Assumption | Risk | Validated by |
|---|------------|------|--------------|
| A1 | Every pane's root process is a session leader on Linux and macOS (portable-pty `setsid`). | Low (code read, `portable-pty-0.9.0/src/unix.rs:257`) | US-004 tests |
| A2 | A Job Object assigned right after ConPTY spawns the child contains the pane's tree, the console host aside. | Validated by US-030: the shell and its child are in the pane's job, ConPTY's `OpenConsole.exe` is not | US-030 |
| A3 | `GetNamedPipeClientProcessId` cannot be set by an unprivileged client. | Validated by US-030: both pipe controls that would set it are refused from user mode; a handle passed to another process still names the process that opened it | US-030; US-003 also compares the impersonated token's user |
| A4 | bubblewrap can create user namespaces on Ubuntu 24.04, where AppArmor restricts unprivileged user namespaces by default. | Disproved by US-023: it cannot as installed; it can with `kernel.apparmor_restrict_unprivileged_userns=0` | US-023 |
| A5 | A Seatbelt `network-outbound` rule matches Paneflow's socket paths on every supported macOS release. | Validated on macOS 26.6.2 and 27.2.1 by US-032: `path-literal`, `literal`, `path`, `subpath` and `regex` all match, on the canonical path only (`/private/var/...`, not `/var/...`) | US-032 |
| A6 | Claude Code and Codex still run their own sandbox inside a Paneflow sandbox. | Validated on Linux by US-023; disproved on macOS by US-032: Seatbelt refuses a second profile in a sandboxed process, so both agents' sandboxed commands fail there | US-023, US-032 |
| A7 | A Seatbelt profile that allows by default can deny `process-info*` and `signal` toward processes outside the pane. | Validated with a correction by US-032: `(deny signal (target others))` lets a signal through to a process of the caller's own session, so the profile denies both operations outright and allows them `(target same-sandbox)`; `ps` cannot run at all, because Seatbelt refuses to execute a setuid program | US-032 |
| A8 | `/proc/<pid>/stat` of the user's own processes is readable by the host on Linux; a `hidepid=2` mount of `/proc` still lets a process read its own user's entries. | Validated with a correction by US-031: the user's dumpable processes stay readable, but a parent chain also crosses processes the host cannot see (other users' and non-dumpable ones), which made every caller `unverified` until the fix | US-031 |
| A9 | On macOS, `sysctl KERN_PROC` returns the parent PID and session of any process of the same user. | Validated with a correction by US-029: `kinfo_proc` gives the parent PID and start time (it agreed with `proc_pidinfo` on 462 processes and was the only source for 278 that `proc_pidinfo` refuses), but it carries no session id, which `getsid(2)` gives | US-029 |
| A10 | RHEL 9 has no `SO_PEERPIDFD` backport. | Low: the fallback path US-031 proves on 5.15 runs on any kernel without the option | Not validated separately |
| A12 | A pane process that ignores `SIGHUP` can outlive the host, and the session manifests in the home still name its leader while the host is down. | Validated by US-031 on 5.15 and 6.8 | US-031 |
| A11 | A `peer.classify` round trip per new connection does not measurably slow the CLI. No latency target is set because none is evidenced; the answer is kept for the connection's life. | Low | Debug log timing in US-007 |

Constraints from `AGENTS.md`: every change works on Linux, macOS and Windows, with Windows behavior verified on hardware after a push; no comments in source; the render thread never blocks (the window classifies on its connection thread); the helper binaries keep their size caps; every grant an agent asks for goes through the human.

## Epics & User Stories

### EP-001: Know who is calling

Every connection on the window, the worker and the host carries a principal the kernel attests, and a client can read its own.

**Definition of Done:** `paneflow api call system.whoami` run from a pane answers `pane` with that pane's session id and run from a terminal outside Paneflow answers `external`, on each of the three servers in the Linux test suite, and on the host and the worker in the macOS and Windows reports of US-029 and US-030; a raw test client in a pane that sends a hello claiming an engine is classified `pane` by the host.

#### US-001: Read a peer identity on Linux
**Description:** As the maintainer, I want one function that returns the peer process of a connection, bound to that process and not to a reusable PID, so that later decisions rest on the kernel.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given the new GPUI-free crate `crates/paneflow-peer`, when `Cargo.lock` is compared before and after, then no crate was added to it
- [x] Given `paneflow-shim` and `paneflow-ai-hook`, when their dependency trees are listed, then `paneflow-peer` is absent from both
- [x] Given a Unix socket connection on Linux 6.5 or later, when the peer is read, then the identity holds a pidfd from `SO_PEERPIDFD`
- [x] Given that identity, when it is built, then it records the peer's start time
- [x] Given that identity, when it is built, then it records the peer's executable path
- [x] Given `getsockopt(SO_PEERPIDFD)` failing with `ENOPROTOOPT`, when the peer is read, then the PID comes from `SO_PEERCRED` and the start time from `/proc/<pid>/stat`
- [x] Given that fallback, when a `pidfd_open` made after the read reports a different start time, then the identity is `Unverified`
- [x] Given a peer that exited before the read, when the peer is read, then the result is `Unverified` with the OS error
- [x] Given two connections from one process, when both are read, then the identities compare equal
- [x] Given a PID reused by a new process, when both identities are compared, then they differ

#### US-002: Read a peer identity on macOS
**Description:** As the maintainer, I want the same function on macOS, so that the principal does not depend on a reusable PID there either.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a Unix socket connection on macOS 13 or later, when the peer is read, then the identity comes from the `LOCAL_PEERTOKEN` audit token
- [x] Given that identity, when the executable is needed, then it comes from `proc_pidpath_audittoken`
- [x] Given `getsockopt(LOCAL_PEERTOKEN)` failing, when the peer is read, then the result is `Unverified` with the OS error, and the PID from `LOCAL_PEERPID` is not used alone
- [x] Given the CI macOS job, when it runs, then the macOS reader compiles and its unit tests pass

#### US-003: Read a peer identity on Windows
**Description:** As the maintainer, I want the same function for named pipes, so that Windows decisions rest on a process handle and not on a reported PID.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a named-pipe connection, when the peer is read, then the PID comes from `GetNamedPipeClientProcessId` and a process handle is opened before the first request is handled
- [x] Given that handle, when the identity is built, then it records the process creation time from `GetProcessTimes`
- [x] Given the impersonated client token, when its user SID differs from the server's, then the identity is `Unverified`
- [x] Given `OpenProcess` failing, when the peer is read, then the result is `Unverified` with the OS error
- [x] Given the code, when the `Windows x86_64 libghostty check` job runs, then it compiles and lints with `-D warnings`

#### US-004: Decide pane membership on Linux and macOS
**Description:** As the maintainer, I want the host to know whether a peer runs inside a pane and which one, so that membership is a fact the host attests.

**Priority:** P0
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [x] Given a session the host spawns, when it starts, then its manifest records the leader's PID, start time and session id
- [x] Given a process started by the pane's shell, when it is classified, then the answer is `Pane` with that session's id and workspace
- [x] Given a process of the pane that called `setsid` and kept its parent, when it is classified, then the parent chain reaches the leader and the answer is `Pane`
- [x] Given a process of the pane that double-forked without `setsid`, when it is classified, then its session id names the pane and the answer is `Pane`
- [x] Given a process unrelated to every pane, when it is classified, then the answer is `NotInPane`
- [x] Given a pane leader that exited and whose PID was reused, when a process of the new owner is classified, then the start-time mismatch makes the answer `NotInPane`
- [x] Given a parent chain that cannot be read (a `/proc` entry or `sysctl` call fails), when the peer is classified, then the answer is `Unverified`

#### US-005: Contain each pane in a Job Object on Windows
**Description:** As the maintainer, I want each Windows pane's process tree in its own job, so that membership there is a container the process cannot leave.

**Priority:** P0
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given a session spawned on Windows, when its child starts, then the child is assigned to a new Job Object with breakaway disabled
- [x] Given a process of that tree, when it is classified, then `IsProcessInJob` on the pane's job makes the answer `Pane`
- [x] Given the job assignment failing, when the session starts, then the session still starts and the failure is logged once with the OS error
- [x] Given a pane whose assignment failed, when one of its processes is classified, then membership comes from the parent chain of a process snapshot
- [x] Given the pane's session ending, when the host releases it, then the job handle is closed

#### US-006: Serve `peer.classify` from the host
**Description:** As the maintainer, I want the window and the worker to ask the host whether a peer is in a pane, so that the three servers agree on one answer.

**Priority:** P0
**Dependencies:** Blocked by US-004, US-005

**Acceptance Criteria:**
- [x] Given a `peer.classify` call with a PID and a start time, when the host answers, then the reply is `{"in_pane": true, "session": ..., "workspace": ...}` or `{"in_pane": false}`
- [x] Given a PID whose live start time differs from the one sent, when the host answers, then the reply is `{"in_pane": false, "stale": true}`
- [x] Given a caller of `peer.classify` that is itself `Pane`, when it calls, then it is refused with reason `principal_refused`
- [x] Given the host unreachable on Linux or macOS, when the worker or the window classifies a peer, then it classifies from the session manifests in the home
- [x] Given the host unreachable on Windows, when the worker or the window classifies a peer, then the principal is `Unverified`
- [x] Given `peer.classify`, when the catalog is listed, then it is declared with owner `host` and its schema is in `protocol/api-v1.json`

#### US-007: Give every connection a principal and answer `system.whoami`
**Description:** As a script author, I want to see how Paneflow identifies my process, so that a refusal can be understood.

**Priority:** P0
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given a connection accepted by the host, the worker or the window, when its first request is handled, then the connection carries one principal among `Window`, `Worker`, `Pane`, `External`, `Unverified`
- [x] Given a non-pane peer of the host, when its hello claims an engine, then its principal is `Window`
- [x] Given a non-pane peer of the host, when its hello names a worker client (`paneflow-serve-*`), then its principal is `Worker`
- [x] Given a non-pane peer of the worker, when it calls `layout.attach` with the client name `window`, then its connection's principal becomes `Window`
- [x] Given a non-pane peer of the window's socket, when it connects, then its principal is `External`
- [x] Given a `Pane` peer of the host whose hello claims an engine, when it is classified, then its principal stays `Pane`
- [x] Given `system.whoami` on any of the three servers, when it answers, then it returns the principal kind and, for `Pane`, the session id and workspace id
- [x] Given a peer whose identity read returned `Unverified`, when it calls `system.whoami`, then the answer is `unverified`
- [x] Given a classification, when it completes, then a debug line records the principal and the time the classification took

---

### EP-002: One authorization point

Every method on every server is authorized by one function from the caller's principal and one policy file, and the gaps the EP-008 review found are closed.

**Definition of Done:** the three servers call `authorize` from their dispatch; a search of the workspace finds `PANEFLOW_IPC_SCRIPTING` and `PANEFLOW_IPC_ORCHESTRATION` read only by the policy module of `paneflow-api`; a conformance test sends every catalog method as `Pane`, `External` and `Window` to its owner and every decision matches the checked-in table; the two worker paths the review flagged refuse a `Pane` caller.

#### US-008: Declare the audience and scope rule per method
**Description:** As the maintainer, I want each method's descriptor to state which principals may call it and in which scope, so that the rule exists once.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given the descriptor in `crates/paneflow-api/src/descriptor.rs`, when a method is declared, then it states an audience among `Anyone`, `NotPane` and `PaneflowOnly` (Window and Worker)
- [x] Given `layout.launch`, `layout.sweep`, `approval.follow`, `approval.decide`, `runtime.bind` and `host.shutdown`, when the catalog is listed, then their audience is `PaneflowOnly`
- [x] Given `layout.attach` and `peer.classify`, when the catalog is listed, then their audience is `NotPane`
- [x] Given `session.input`, `stop`, `restart`, `remove`, `reset` and `clear_history`, when the catalog is listed, then each declares the workspace scope rule
- [x] Given every method the host dispatches at `crates/paneflow-host/src/server.rs:549-577,900-1066`, when this story ends, then each has a descriptor
- [x] Given a descriptor declared without an audience, when the crate compiles, then compilation fails
- [x] Given `system.describe`, when it lists a method, then its audience and gate appear
- [x] Given `protocol/api-v1.json` regenerated, when it is compared with the previous one, then the parameters and results of every existing method are unchanged

#### US-009: Evaluate one `authorize` function
**Description:** As the maintainer, I want one pure function that decides a call from the principal, the descriptor, the parameters and the policy, so that the three servers cannot disagree.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given `authorize(principal, descriptor, params, policy)`, when it is called, then it returns `Allowed { scope }` or `Refused { reason }`
- [x] Given `authorize`, when its module is read, then it takes the policy as a value and calls no file, socket or environment API
- [x] Given a `Pane` caller of a `PaneflowOnly` or `NotPane` method, when it is authorized, then it is refused with reason `principal_refused`
- [x] Given a `Pane` caller that sends `scope_session` naming another session, when a scoped method is authorized, then the scope is the caller's own workspace
- [x] Given a `Pane` caller of a scoped method on another workspace with the orchestration gate closed for it, when it is authorized, then it is refused with reason `scope_refused`
- [x] Given an `External` caller that sends `scope_session`, when a scoped method is authorized, then the parameter is honored as today
- [x] Given an `Unverified` caller of any method other than `system.whoami`, `system.capabilities` and `system.describe`, when it is authorized, then it is refused with reason `unverified_caller`
- [x] Given every catalog method and each principal kind, when the decision-table test runs, then each decision matches the table checked in beside the test

#### US-010: Move the switches into `paneflow.json`
**Description:** As a user, I want one place that says what scripts and agents may do, so that it does not depend on which process started with which variable.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given `"ipc": {"scripting": true}` in `paneflow.json`, when an `External` or `Pane` request is authorized, then the scripting gate is open in all three servers
- [x] Given `"ipc": {"orchestration": true}`, when an `External` or `Pane` request is authorized, then the orchestration gate is open in all three servers
- [x] Given the section changed while Paneflow runs, when the next gated request arrives, then each server applies the new value
- [x] Given `PANEFLOW_IPC_SCRIPTING=1` in a serving process's environment, when an `External` request is authorized, then the scripting gate is open
- [x] Given that variable set, no `ipc` section, and a `Pane` request in the deprecation release, when it is authorized, then the gate is open
- [x] Given that case, when the first such request arrives, then the server logs one deprecation warning naming the `ipc` key that replaces the variable
- [x] Given an `ipc` section present and the variable set, when a `Pane` request is authorized, then only the section decides
- [x] Given `ai_unrestricted: true`, when a `Pane` or `External` request is authorized, then both gates are open
- [x] Given a malformed `ipc` section, when a `Pane` request is authorized, then both gates are closed
- [x] Given a malformed `ipc` section, when the config loads, then the error names the key

#### US-011: Authorize at the worker's dispatch
**Description:** As the maintainer, I want the worker to authorize every call before its handler runs, so that the attached-connection gaps close.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given a `Pane` caller of `layout.attach`, when the worker answers, then it is refused with reason `principal_refused` and the connection is not attached
- [x] Given a `Pane` caller whose `layout.apply` op spawns a surface carrying a command, a prompt or an environment with the orchestration gate closed for it, when the op is applied, then it is refused and the model is unchanged
- [x] Given `launcher::orchestration_enabled`, `structural::orchestration_open` and the gate check in `worktrees.rs`, when this story ends, then none of them exists
- [x] Given the worker's existing gate tests (`worker_structural.rs`, `worker_worktrees.rs`, `worker_launch.rs`), when they run against `authorize`, then they pass unchanged
- [x] Given an `init.lua` action started from a key or the palette, when it calls a method in-process, then it is authorized as `Window`
- [x] Given an `init.lua` action started by `paneflow actions run` from a pane, when it calls a method in-process, then it is authorized as that `Pane`
- [x] Given a gated method refused by the worker, when the error is returned, then the code and message equal those the method returns today with its gate closed

#### US-012: Let the worker decide a live sweep
**Description:** As a user, I want sessions stopped only by the sweep that follows a restore, so that no caller can ask for a live sweep at another time.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given the worker loading its layout from `session.json` at start, re-reading it after an outside write, or adopting a window's layout, when that happens, then it marks one live sweep as due
- [x] Given a `layout.sweep` from the `Window` with a live sweep due, when it runs, then it stops the live orphan sessions and clears the mark
- [x] Given a `layout.sweep` with no live sweep due, when it runs with `live: true`, then it forgets ended records and stops nothing
- [x] Given a live sweep that cannot list the host's sessions, when it ends, then the mark stays set
- [x] Given the window's diff-dock terminal sessions, which no layout surface holds, when a sweep from the `Window` runs, then they are kept through the `keep` list as today

#### US-013: Authorize at the host's dispatch and close the write path
**Description:** As a user, I want an agent in a pane unable to approve its own writes or reach panes of other workspaces, so that the human stays in the loop.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given a `Pane` caller whose hello claims an engine, when it calls `approval.decide`, then it is refused with reason `principal_refused`
- [x] Given a `Pane` caller whose hello claims an engine, when it calls `session.input` with the scripting gate closed for it, then it is refused as a non-attached client is today
- [x] Given a `Pane` caller of `pane.write` with a `source_session` other than its own, when it is called, then it is refused with reason `source_mismatch`
- [x] Given a `Pane` caller of `session.stop` on a session of another workspace with the orchestration gate closed for it, when it is called, then it is refused with reason `scope_refused` and a message naming that workspace
- [x] Given a `Pane` caller of `session.stop` on a session of its own workspace, when it is called, then it succeeds as today
- [x] Given a `Pane` caller of `host.shutdown`, when it is called, then it is refused with reason `principal_refused`
- [x] Given the window and the worker, when they call these methods, then they succeed as today

#### US-014: Authorize at the window's dispatch
**Description:** As the maintainer, I want the window socket to apply the same function, so that a method the window still answers is not a side door.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given a request on the window's socket, when it is handled, then `authorize` runs on the connection thread before the request reaches the GPUI thread
- [x] Given `src-app/src/app/ipc_handler/gates.rs`, when this story ends, then it reads no environment variable
- [x] Given a spawn check of the window, when it runs, then it calls `authorize`
- [x] Given a `Pane` caller of `surface.send_text` with the scripting gate closed for it, when the window answers, then the refusal code and message equal today's
- [x] Given a peer of another user, when it connects to the window's socket, then it is refused by the UID check (`src-app/src/ipc.rs:803-827`) before any classification

#### US-015: Document the threat model and the migration
**Description:** As a user, I want to know what Paneflow protects against and how to keep my setup working, so that I configure it knowingly.

**Priority:** P1
**Dependencies:** Blocked by US-011, US-012, US-013, US-014

**Acceptance Criteria:**
- [x] Given `docs/user/scripting.md`, when it is read, then it lists the principals and the `ipc` keys with their defaults
- [x] Given that page, when it is read, then it states that a hostile process of the same user outside a sandbox can escape pane classification
- [x] Given `docs/user/conductor.md:78`, when it is read, then it names `ipc.scripting` instead of `PANEFLOW_IPC_SCRIPTING`
- [x] Given `ARCHITECTURE.md`'s IPC section, when it is read, then it describes the classification by the host, `peer.classify`, `authorize` and the audience of each method family
- [x] Given `CHANGELOG.md`, when it is read, then its entry gives the exact `paneflow.json` lines that keep an agent in a pane able to orchestrate
- [x] Given the migration lines applied and `PANEFLOW_IPC_SCRIPTING` unset, when an agent pane runs `paneflow send --submit` to a pane of its workspace, then it succeeds
- [x] Given neither the migration lines nor the variable, when an agent pane runs the same command, then it is refused with the documented message

---

### EP-003: Grants a pane holds

The human grants an agent what it needs, for a scope and a time, and an agent can pass less of it to a sub-agent.

**Definition of Done:** in a debug build with every `ipc` gate closed, an agent in a pane calls `grant.request` for `write` on its workspace, the human allows it, its `paneflow send` to a pane of its workspace succeeds and one to another workspace is refused; after the grant expires the first `send` is refused.

#### US-016: Hold grants in the worker
**Description:** As the maintainer, I want grants stored in one place with one shape, so that every server can apply them.

**Priority:** P1
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given a grant, when it is stored, then it records its holder (session id and launch generation), rights among `write`, `spawn` and `layout`, a scope (`workspace` or `instance`), an expiry, and the grant it was delegated from if any
- [x] Given a `Pane` caller of a gated method, when a live grant covers the rights and scope the request needs, then `authorize` opens the gate
- [x] Given a grant whose holder's session ended or whose launch generation changed, when the next request arrives, then the grant does not apply
- [x] Given the worker restarted, when it starts, then grants of live sessions are restored from the serve directory and expired ones are dropped
- [x] Given a grant file the worker cannot parse, when it starts, then it starts with no grant and logs the cause

#### US-017: Apply grants in the host and the window
**Description:** As the maintainer, I want the host and the window to apply grants without asking the worker on every call, so that authorization stays local.

**Priority:** P1
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given a grant created, delegated, revoked or expired, when it changes, then the worker sends the full grant set to the host with a new `host.grants.set` call, whose audience is `PaneflowOnly`
- [x] Given that change, when it happens, then the worker appends a `grant.changed` event to its event log
- [x] Given the window following the worker's event log, when it receives `grant.changed`, then it reads the grant set with `grant.list` before applying the next gated request
- [x] Given the host or the window holding a grant set older than the worker's revision, when a `Pane` request needs a grant, then it is refused with reason `grants_unsynced` and a message saying the call can be retried
- [x] Given a request that needs no grant, when the host's or the window's grant set is stale, then it is authorized as without grants

#### US-018: Ask the human for a grant
**Description:** As an agent author, I want an agent to ask for the rights it needs, so that the human decides when it matters.

**Priority:** P1
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given a `Pane` caller of `grant.request` with rights, scope and duration, when the worker receives it, then it raises a prompt through its prompt broker naming the agent, its pane, the rights, the scope and the duration
- [x] Given the human allowing the prompt, when it is answered, then the grant is stored and the call returns its id and expiry
- [x] Given the human denying it, when the call returns, then it returns `granted: false` with reason `denied` and no grant exists
- [x] Given no answer before the broker's timeout or no window able to show the prompt, when the call returns, then it returns `granted: false` with reason `unanswered`
- [x] Given a request for `instance` scope or with no expiry, when the prompt is shown, then its title says so
- [x] Given a `grant.request` from a non-pane caller, when it is received, then it is refused with reason `principal_refused`

#### US-019: Grant from configuration per agent profile
**Description:** As a user who automates, I want to grant an agent profile rights in `paneflow.json`, so that a trusted setup needs no prompt each time.

**Priority:** P1
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given `agents.<profile>.grants` in `paneflow.json`, when a pane is launched with that profile, then the worker stores those grants for the pane, ending with its session
- [x] Given a pane whose agent is declared after launch by the shim, when the declaration arrives, then the profile's grants apply from that moment
- [x] Given a profile grant naming an unknown right, when the config loads, then the error names the profile and the right
- [x] Given that error, when a pane of that profile launches, then no grant of the profile is stored

#### US-020: Delegate and revoke
**Description:** As an agent author, I want an orchestrating agent to give a sub-agent less than it holds, and the human to revoke any grant, so that authority narrows down a tree and can be taken back.

**Priority:** P1
**Dependencies:** Blocked by US-017, US-018

**Acceptance Criteria:**
- [x] Given a pane holding a grant and a pane it spawned with `surface.split`, when it calls `grant.delegate` within its own rights and scope, then the child receives the grant with an expiry no later than the parent's
- [x] Given a delegation wider than the delegator's grant in rights, scope or expiry, when it is called, then it is refused with reason `grant_exceeds_holder`
- [x] Given a delegation to a pane the delegator did not spawn, when it is called, then it is refused with reason `not_spawned_by_caller`
- [x] Given a grant revoked or expired, when that happens, then every grant delegated from it is revoked in the same `grant.changed` event
- [x] Given `paneflow grants ls` and `paneflow grants revoke <id>` from a non-pane caller, when they run, then they list and revoke any grant
- [x] Given `paneflow grants revoke <id>` from a `Pane` caller on a grant it did not delegate, when it runs, then it is refused with reason `principal_refused`

---

### EP-004: Sandboxed agent panes

An agent profile can run in a sandbox where Paneflow's shared endpoints and other panes' processes are out of reach, and its pane reaches each server through an endpoint that is its identity.

**Definition of Done:** with a profile set to `sandbox: "paneflow"` on each platform the reports of US-023, US-032 and US-033 list as supported, a process in the pane cannot open the window, worker or host shared endpoints; `paneflow api call system.whoami` from the pane answers `pane` with its session on each server's pane endpoint; and Claude Code starts in the pane.

#### US-021: Give each sandboxed pane its own host and worker endpoints
**Description:** As the maintainer, I want the host and the worker to listen on one endpoint per sandboxed pane, so that identity inside a sandbox needs no peer credential and no relay.

**Priority:** P2
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given a pane launched with a sandboxed profile, when its session is created, then the host creates an endpoint for it in the home's directory `panes/<session>/`
- [x] Given that session, when the worker learns it from the host, then the worker creates its endpoint for the pane in the same directory
- [x] Given those endpoints, when the pane starts, then its environment names them in `PANEFLOW_PANE_HOST_ENDPOINT` and `PANEFLOW_PANE_SERVE_ENDPOINT`
- [x] Given a client in the pane, when it resolves the host or the worker endpoint, then `paneflow_home` returns the pane variable's path, and only when that path lies under the home's `panes/` directory
- [x] Given a connection on a pane endpoint, when it is accepted, then its principal is that `Pane` without `peer.classify`
- [x] Given the pane's session ending, when it ends, then the host and the worker close and remove their endpoints for it
- [x] Given an endpoint that cannot be created, when the session starts, then the sandboxed pane does not start and shows the launch-failure state with the OS error

#### US-022: Give each sandboxed pane its own window endpoint
**Description:** As the maintainer, I want the window to listen on one endpoint per sandboxed pane, so that window-owned methods reach it from a sandbox.

**Priority:** P2
**Dependencies:** Blocked by US-021

**Acceptance Criteria:**
- [x] Given a running window and a sandboxed pane, when the pane's session exists, then the window creates its endpoint for it in `panes/<session>/`
- [x] Given a window that starts after the pane, when it starts, then it creates that endpoint for every sandboxed pane the host lists
- [x] Given the pane's environment, when it starts, then `PANEFLOW_PANE_WINDOW_ENDPOINT` names that endpoint
- [x] Given an isolated home (`PANEFLOW_HOME` set), when a client in the pane resolves the window endpoint, then the pane variable is honored without `PANEFLOW_ALLOW_SOCKET_OVERRIDE` because its path lies under the home's `panes/` directory (`crates/paneflow-home/src/endpoint.rs:153-167` ignores a foreign `PANEFLOW_SOCKET_PATH`)
- [x] Given no window running, when the pane calls a window-owned method, then it fails with the existing "no window" reason
- [x] Given a connection on the window's pane endpoint, when it is accepted, then its principal is that `Pane`

#### US-023: Prove bubblewrap on the Linux targets
**Description:** As the maintainer, I want to know where bubblewrap works and with which agents, so that the Linux sandbox is designed on facts.

**Priority:** P2
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given Ubuntu 22.04, Ubuntu 24.04 (AppArmor's user-namespace restriction on), Fedora, Debian 13, Arch and openSUSE Leap 16, when `bwrap --unshare-all --die-with-parent` runs a shell started by Paneflow's host, then the report states per distribution whether it started
- [x] Given a distribution where it fails, when the report is read, then it gives the exact error and names the setting or AppArmor profile that would allow it
- [x] Given Claude Code and Codex started inside that sandbox with their own sandbox enabled, when each runs one sandboxed command, then the report states whether it ran
- [x] Given the report, when it is written, then it is a section of this PRD listing the supported distributions

#### US-024: Run a Linux pane under bubblewrap
**Description:** As a user, I want an agent profile to run in a sandbox that hides Paneflow and other panes, so that the agent reaches Paneflow only through its own pane.

**Priority:** P2
**Dependencies:** Blocked by US-022, US-023

**Acceptance Criteria:**
- [x] Given a profile with `sandbox: "paneflow"` on Linux, when its pane launches, then the shell runs under bubblewrap with new user, PID, mount and cgroup namespaces, `--die-with-parent` and `no_new_privs`
- [x] Given that sandbox, when a process lists `XDG_RUNTIME_DIR` and Paneflow's home, then the shared window, worker and host endpoints are absent
- [x] Given that sandbox, when a process lists the pane's endpoint directory, then the pane's endpoints are present
- [x] Given that sandbox, when a process reads `/proc`, then only processes of the pane are listed
- [x] Given the pane's working directory and the agent's configuration directories, when the pane runs, then they are writable as without the sandbox
- [x] Given `bwrap` missing or refusing, when the pane launches, then it does not start and shows the launch-failure state naming `bwrap` and its error

#### US-025: Add Landlock scoping inside the Linux sandbox
**Description:** As the maintainer, I want Landlock to scope sockets created outside the sandbox when the kernel supports it, so that a socket passed in by mistake is still unreachable.

**Priority:** P2
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [x] Given a kernel with Landlock ABI 6 or later, when the sandbox starts, then abstract unix sockets created outside it cannot be connected to
- [x] Given a kernel with ABI 9 or later and a profile with `sandbox_unix_scope: true`, when a process connects to a pathname socket created outside the sandbox, then it fails with `EACCES`; without the opt-in the connection is not scoped, so the agent's own bubblewrap still starts
- [x] Given a kernel with neither, when the sandbox starts, then it starts without Landlock and the pane's debug log names the missing ABI

#### US-028: Show sandbox state and refuse silent downgrades
**Description:** As a user, I want to see that a pane is sandboxed and never have it run unsandboxed by surprise.

**Priority:** P2
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [x] Given a sandboxed pane, when its header is drawn, then it shows a sandbox indicator built from `DESIGN.md`'s component rules
- [x] Given a sandboxed pane, when `paneflow ls` runs, then its row reports `sandboxed: true`
- [x] Given a restored session whose profile is sandboxed, when the window restores it, then it restores sandboxed or shows the launch-failure state, never an unsandboxed shell
- [x] Given a profile set to `sandbox: "paneflow"` on a platform the reports of US-023, US-032 or US-033 list as unsupported, when the config loads, then the error names the platform and the profile

---

### EP-005: Real-hardware validation

The Linux kernel matrix is exercised on real kernels. The Windows and macOS hardware checks live in EP-006 and EP-007.

**Definition of Done:** each story below has its report written as a section of this PRD; every defect it found is fixed or recorded as a blocker on the story it belongs to; the platform notes of `docs/user/scripting.md` match the reports.

#### US-031: Exercise the Linux kernel matrix
**Description:** As the maintainer, I want the fallback peer path proven on kernels without `SO_PEERPIDFD`, so that users of a kernel older than 6.5 (Ubuntu 22.04's GA kernel, Debian 12, Leap 15.6) get the same answers.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given Ubuntu 22.04 on its GA 5.15 kernel, when `system.whoami` runs from a pane and outside, then the answers are `pane` and `external`
- [x] Given that kernel, when the peer is read, then the debug log shows the `SO_PEERCRED` fallback
- [x] Given Ubuntu 24.04 on 6.8, when the same check runs, then the answers are the same
- [x] Given that kernel, when the peer is read, then the debug log shows `SO_PEERPIDFD`
- [x] Given a pane process that ignores `SIGHUP` and the host killed, when the worker classifies that process, then the report states the answer and whether the manifests still named its leader (assumption A12)
- [x] Given `/proc` mounted with `hidepid=2`, when a pane process is classified, then the report states the answer (assumption A8)
- [x] Given a process in a pane that calls `setsid` and double-forks, when it calls `system.whoami`, then the report records `external` as the documented layer-1 limit

---

### EP-006: Windows tests and validation

Everything that needs a Windows machine is run there in one pass: the restricted-token sandbox written by inspection, the layer-1 identity paths and the sandbox checks.

**Definition of Done:** each story below is proven on Windows 10 or 11 hardware and its report is a section of this PRD; every defect found is fixed or recorded as a blocker on the story it belongs to; the Windows notes of `docs/user/scripting.md` match the reports.

#### US-027: Run a Windows pane with a restricted token
**Description:** As a user on Windows, I want the sandboxed profile to keep an agent off Paneflow's shared pipes.

**Priority:** P2
**Dependencies:** Blocked by US-022

**Acceptance Criteria:**
- [x] Given a profile with `sandbox: "paneflow"` on Windows, when its pane launches, then the shell runs with a write-restricted token whose restricting SIDs are a per-pane SID and the SIDs the pane's working directory grants
- [x] Given that token, when a process in the pane opens the shared window, worker or host pipe for writing, then the open fails with access denied
- [x] Given the pane's endpoints, whose DACL grants the per-pane SID, when a process in the pane connects, then it succeeds
- [x] Given the pane's working directory, when a process in the pane writes a file there, then the write succeeds
- [x] Given token creation failing, when the pane launches, then it does not start and shows the launch-failure state with the OS error

#### US-030: Validate layer 1 on Windows hardware
**Description:** As the maintainer, I want the Windows identity paths run on Arthur's Windows 11 machine, so that the Job Object and pipe assumptions are facts.

**Priority:** P0
**Dependencies:** Blocked by US-007, US-013, US-014

**Acceptance Criteria:**
- [x] Given a pane under ConPTY, when its shell, a child of the shell and the console host are checked with `IsProcessInJob`, then the report states which are in the pane's job (assumption A2)
- [x] Given a test client that tries to present another process's PID on a named pipe, when the server reads the peer, then the report states whether the PID could be forged (assumption A3)
- [x] Given that test, when the PID was forged, then the report states whether the token-user check refused it
- [x] Given `system.whoami` sent to the host from a pane and from Windows Terminal outside Paneflow, when it runs, then the answers are `pane` and `external`
- [x] Given `system.whoami` sent to the worker from the same two places, when it runs, then the answers are `pane` and `external`

#### US-033: Validate the Windows sandbox on hardware
**Description:** As the maintainer, I want the restricted-token pane checked on Windows 11, so that it ships only where it holds.

**Priority:** P2
**Dependencies:** Blocked by US-027

**Acceptance Criteria:**
- [x] Given a sandboxed pane, when it opens the shared worker pipe, then the report states whether access was denied
- [x] Given that pane, when `git status`, `cargo build` and `node -v` run in its working directory, then the report states whether each succeeded
- [x] Given a check that fails, when the report is written, then it lists Windows as unsupported for the sandboxed profile until it is fixed
- [x] Given the report lists the platform as supported, when the story closes, then `SANDBOX_SUPPORTED_PLATFORMS` in `crates/paneflow-config/src/schema/agent_sandbox.rs` gains `windows` and `docs/user/scripting.md` says so; until then a profile with `sandbox: "paneflow"` is refused there

---

### EP-007: macOS tests and validation

Everything that needs a Mac is run there in one pass: the Seatbelt sandbox written by inspection, the layer-1 identity paths and the sandbox checks.

**Definition of Done:** each story below is proven on macOS hardware (Intel and Apple Silicon where the story says so) and its report is a section of this PRD; every defect found is fixed or recorded as a blocker on the story it belongs to; the macOS notes of `docs/user/scripting.md` match the reports.

#### US-026: Run a macOS pane under a Seatbelt profile
**Description:** As a user on macOS, I want the sandboxed profile to keep an agent away from Paneflow's shared sockets.

**Priority:** P2
**Dependencies:** Blocked by US-022

**Acceptance Criteria:**
- [x] Given a profile with `sandbox: "paneflow"` on macOS, when its pane launches, then the shell runs under a Seatbelt profile that allows by default and denies `network-outbound` to the shared window, worker and host socket paths
- [x] Given that profile, when a process in the pane connects to its pane endpoints, then the connection succeeds
- [x] Given that profile, when a process in the pane connects to the shared worker socket, then the sandbox refuses it
- [x] Given that profile, when a process in the pane sends a signal to a process outside the pane, then the sandbox refuses it
- [x] Given `sandbox-exec` missing or refusing the profile, when the pane launches, then it does not start and shows the launch-failure state with the error

#### US-029: Validate layer 1 on macOS hardware
**Description:** As the maintainer, I want the macOS identity paths run on Intel and Apple Silicon, so that nothing ships on inspection alone.

**Priority:** P0
**Dependencies:** Blocked by US-007, US-013, US-014

**Acceptance Criteria:**
- [x] Given macOS 13 or later on Apple Silicon, when `system.whoami` is sent to the host from a pane and from Terminal.app, then the answers are `pane` and `external`
- [x] Given the same machine, when `system.whoami` is sent to the worker from a pane and from Terminal.app, then the answers are `pane` and `external`
- [x] Given a raw client in a pane that claims an engine, when it calls `approval.decide`, then it is refused
- [x] Given `sysctl KERN_PROC` on a process of the same user, when the classification reads it, then the report confirms the parent PID and session are returned (assumption A9)

**Deferred (v1.8):** the Intel check ("Given macOS on Intel, when both checks run, then the answers are the same") moves out of this story's criteria. Arthur runs it on an Intel Mac after the branch merges to `main`; until then the macOS layer-1 report covers Apple Silicon only.

#### US-032: Validate the macOS sandbox on hardware
**Description:** As the maintainer, I want the Seatbelt profile checked on the current and previous macOS releases, so that the sandbox's socket rule is known to match.

**Priority:** P2
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [x] Given a sandboxed pane on the current macOS release and the previous one, when it connects to the shared worker socket, then the report states whether the sandbox refused it (assumption A5)
- [x] Given that refusal, when the report is written, then it names the filter form (literal, subpath or regex) that matched
- [x] Given that pane, when it inspects a process outside the pane with `ps` and `kill -0`, then the report states whether the sandbox refused each (assumption A7)
- [x] Given Claude Code with its own sandbox in that pane, when it runs one sandboxed command, then the report states whether it ran (assumption A6)
- [x] Given Codex with its own sandbox in that pane, when it runs one sandboxed command, then the report states whether it ran (assumption A6)
- [x] Given a release where a check fails, when the report is written, then it lists that release as unsupported for the sandboxed profile
- [x] Given the report lists the platform as supported, when the story closes, then `SANDBOX_SUPPORTED_PLATFORMS` in `crates/paneflow-config/src/schema/agent_sandbox.rs` gains `macos` and `docs/user/scripting.md` says so; until then a profile with `sandbox: "paneflow"` is refused there

## Linux sandbox report (US-023)

Measured on 2026-11-23. Fedora is the development machine; every other row is a throwaway KVM virtual machine booted from the distribution's cloud image by `scripts/sandbox-matrix.sh`, so each runs its own kernel and its own security policy. In each one the script runs `bwrap --unshare-all --die-with-parent` and then the host's sandbox tests (`crates/paneflow-host/tests/sandboxed_pane.rs`, built once against Ubuntu 22.04's glibc) with `PANEFLOW_SANDBOX_REQUIRED=1`, which turns a sandbox that does not start into a failure.

| Distribution | Kernel, bubblewrap, Landlock ABI | `bwrap --unshare-all --die-with-parent` | Paneflow sandbox (US-024), 5 tests |
|---|---|---|---|
| Fedora 45 | 7.3.4, 0.13.1, ABI 11 | Starts (`NoNewPrivs: 1`) | Pass |
| Ubuntu 22.04.5 | 5.15.0-203, 0.6.1, ABI 1 | Starts | Pass; no Landlock scoping below ABI 6, so an abstract unix socket made outside stays reachable |
| Ubuntu 24.04.5, default (`kernel.apparmor_restrict_unprivileged_userns = 1`) | 6.8.0-147, 0.9.0, ABI 4 | Fails: `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` | Fails: the pane does not start, `bwrap refused to create the sandbox (exit status: 1): bwrap: setting up uid map: Permission denied` |
| Ubuntu 24.04.5 with `sysctl kernel.apparmor_restrict_unprivileged_userns=0` | same | Starts | Pass; no Landlock scoping below ABI 6 |
| Debian 13 | 6.12.118+deb13-cloud, 0.12.0, ABI 6 | Starts | Pass |
| Arch | 7.3.4-arch1-2, 0.13.1, ABI 11 | Starts | Pass |
| openSUSE Leap 16.0 | 6.12.0-160000.41, 0.11.0, ABI 6 | Starts | Pass |

**Supported distributions:** Fedora 45, Ubuntu 22.04, Debian 13, Arch and openSUSE Leap 16.0 as installed. Ubuntu 24.04 is unsupported as installed: AppArmor refuses the user namespace to an unconfined `bwrap`, and the image ships no AppArmor profile for it (`/etc/apparmor.d` has none). The setting that allows it is `kernel.apparmor_restrict_unprivileged_userns=0` (measured above); an AppArmor profile that grants `userns` to `/usr/bin/bwrap` is the narrower alternative and was not measured. A pane whose `bwrap` cannot start does not start: its launch-failure state names `bwrap` and its stderr (US-024).

**Agents inside the Paneflow sandbox, own sandbox on** (Fedora 45, in a session created with `paneflow-host session create --sandbox`):

| Agent | Command | Result |
|---|---|---|
| Claude Code 2.1.330 | `claude -p --settings '{"sandbox":{"enabled":true}}' --allowedTools Bash`, asked to run `touch $HOME/pf-claude-sandbox-probe; echo hello-from-claude-sandbox` | Ran: `hello-from-claude-sandbox` printed, and the write outside the working directory was refused by its own sandbox (read-only file system) |
| Codex 0.165.0 | `codex sandbox -- sh -c 'echo codex-sandbox-ran; touch /usr/pf-codex-probe'` | Ran: `codex-sandbox-ran` printed, the write to `/usr` refused (read-only file system) |

The agents were run on Fedora only; the other distributions were measured for bubblewrap and the Paneflow sandbox, not for each agent. In a live debug window on Fedora, a sandboxed pane answered `system.whoami` with `pane` and its session on the window, the worker and the host, and a `paneflow split` from it opened a pane that `paneflow ls` reports `sandboxed: true`.

## Linux kernel matrix report (US-031)

Measured on 2026-11-23 by `scripts/sandbox-matrix.sh --suite identity`, in throwaway KVM virtual machines booted from each distribution's cloud image, so each runs its own kernel. In each one the script runs two test binaries of `paneflow-host`, built once against Ubuntu 22.04's glibc, as an unprivileged user, first on the default `/proc` and then with `/proc` remounted `hidepid=2` (the kernel reports it as `hidepid=invisible`; the user then sees no root process at all):

- `linux_kernel_matrix` starts the real `paneflow-host serve` binary with `RUST_LOG=debug`, creates a pane whose shell calls `system.whoami`, calls it again from outside every pane, and reads the host's debug log. The script derives the expected peer source from the kernel version alone (6.5 or later: `SO_PEERPIDFD`, R1) and the test fails when the log names another. Its second test starts a pane whose shell ignores `SIGHUP`, kills the host with `SIGKILL`, waits for the shell to be orphaned, lets it connect, and classifies it with the classifier the worker uses (`PeerClassifier`, host endpoint dead).
- `caller_identity` runs the layer-1 cases against an in-process host, including the shell that calls `setsid` and double-forks.

| Distribution | Kernel | Peer source in the host's debug log | `system.whoami` in a pane / outside | Host killed: manifest names the leader, worker's answer | `setsid` and double fork | Same, `/proc` `hidepid=2` |
|---|---|---|---|---|---|---|
| Ubuntu 22.04.5 | 5.15.0-203-generic (GA) | `SO_PEERCRED` (fallback) | `pane` with its session and workspace / `external` | Yes (lifecycle still `running`); `pane` with its session, through the session manifests | `external` | Same answers after the fix below; before it, every caller was `unverified` |
| Ubuntu 24.04.5 | 6.8.0-147-generic | `SO_PEERPIDFD` | `pane` / `external` | Yes; `pane`, through the session manifests | `external` | Same answers after the fix below; before it, every caller was `unverified` |

A log line reads, on 5.15: `paneflow host: process 2438 (SO_PEERCRED) classified pane through the host's pane leaders in 151 us`, and on 6.8 the same with `(SO_PEERPIDFD)`.

**Defect found and fixed: `hidepid=2` refused every caller.** With `/proc` mounted `hidepid=2`, the first run refused every `session.create` with `Paneflow could not identify the calling process: cannot read the parent 2021 of process 2022: No such file or directory`, on both kernels. The parent chain of any caller reaches a process the host cannot see: `sshd-session`, which runs as the user but is not dumpable, over SSH, and `init` everywhere. Membership answered `unverified` for any unreadable parent (`crates/paneflow-peer/src/membership.rs`). It now ends the chain when the parent still exists but its `/proc` entry is hidden (`kill(pid, 0)` succeeds or fails with `EPERM`, and its `stat` cannot be read), as `init` ends it, and stays `unverified` when the parent is gone. The walk has already tested every visible process for a pane leader or a pane's session id before it reaches a hidden one, so a pane process still reads `pane`. The residual is a new session started below a pane process hidden from the host, such as a `sudo` that opens its own terminal: it reads `external` under `hidepid=2`, which is the same layer-1 limit as `setsid` and a double fork, and is documented in `docs/user/scripting.md`.

**Layer-1 limit:** a pane process that calls `setsid` and double-forks reads `external` on both kernels, with and without `hidepid=2`. EP-004's sandbox is what closes it.

## Windows layer 1 report (US-030)

Measured on 2026-11-24 on Windows 11 Pro, build 26300 (10.0.26300.9818), x64, with the debug build of this branch. The job and pipe cases are integration tests of `paneflow-host` (`crates/paneflow-host/tests/windows_layer_one.rs`), which also run on the Windows CI leg; the `system.whoami` case ran the real binaries in an isolated home (`~/.paneflow-dev-ep006`): `paneflow-host serve` with `RUST_LOG=debug`, the worker through `paneflow serve start`, a pane created with `paneflow-host session create --shell cmd.exe`, and a window of Windows Terminal opened with `wt.exe -w new`, whose `cmd.exe` cleared every `PANEFLOW_*` variable but `PANEFLOW_HOME`.

| Check | Result |
|---|---|
| `IsProcessInJob` against the pane's named job (`Local\paneflow-pane-<session>`), ConPTY from the bundled `conpty.dll` | The shell (`cmd.exe`) and its child (`PING.EXE`) are in the job; the console host (`OpenConsole.exe`, started by `conpty.dll` from the host process before the shell joins the job) is not (A2) |
| `FSCTL_PIPE_SET_CLIENT_PROCESS` from the client, naming another live process | Refused, `STATUS_NOT_SUPPORTED` (`0xC00000BB`); `GetNamedPipeClientProcessId` still returns the client's own PID |
| `FSCTL_PIPE_SET_CONNECTION_ATTRIBUTE` `ClientProcessId` from the client | Refused, `STATUS_ACCESS_DENIED` (`0xC0000022`); the same attribute read with `FSCTL_PIPE_GET_CONNECTION_ATTRIBUTE` returns the real PID, which confirms the control codes |
| A client handle inherited by a child process that writes on it | The server reads the PID of the process that opened the pipe, not the writer's, and the token-user check accepts it, because it refuses only another user (A3) |
| `paneflow api call system.whoami --on host` from the pane / from Windows Terminal | `pane` with its session / `external` |
| `paneflow api call system.whoami --on worker` from the pane / from Windows Terminal | `pane` with its session / `external` |

A host log line reads `paneflow host: process 27364 (GetNamedPipeClientProcessId) classified pane through the host's pane leaders in 114 us`.

**No PID forgery from user mode.** A pipe client cannot change the PID the server reads. The token-user check does not catch a forgery inside one user, and needs not: the only way to present another process's PID is to use a pipe handle that process opened, which takes its cooperation or code running inside it, the same-user limit that `setsid` and a double fork mark on Unix. A pane process cannot hand itself an external opener's handle without first running code outside its pane.

## Windows sandbox report (US-033)

Measured on 2026-11-24 on the same machine, by the integration test `a_sandboxed_windows_pane_writes_only_where_it_is_granted_and_reaches_only_its_own_endpoints` (`crates/paneflow-host/tests/sandboxed_pane_windows.rs`): a real `SessionHost` launches a sandboxed pane through ConPTY, the sandbox helper (`paneflow-host sandbox-exec`) waits for the pane's job and starts the shell with the restricted token, and the shell is the test binary itself, which records what it can reach. With `PANEFLOW_SANDBOX_TOOLS_REPORT=<file>` it also runs the tools below and writes the report.

**The token (US-027).** `CreateRestrictedToken` with `WRITE_RESTRICTED | DISABLE_MAX_PRIVILEGE`, created by the host before the pane starts so that a failure is the pane's launch failure. Its restricting SIDs, read back with `TokenRestrictedSids` from inside the pane:

| SID | Why |
|---|---|
| The per-pane SID (`S-1-9-1347-1-...`, from the session id) | The pane's own endpoints and its data directory (its `TEMP` included) grant it; the token's default access list grants it, so the pane's own processes and objects stay writable to it |
| The shared sandbox SID (`S-1-9-1347-0`) | The working directory grants it, inheritably, since several panes can share one; a directory that already grants it is not rewritten |
| The logon SID (`S-1-5-5-...`) | The window station and desktop grant it; without it every program that loads `user32.dll` dies at start with `STATUS_DLL_INIT_FAILED` (`0xC0000142`) |
| Everyone (`S-1-1-0`) | The `NUL` device grants nothing else; without it `> NUL` is refused |

The criterion asked for "the SIDs the pane's working directory grants". Those are the user's own SID and groups, and with the user's SID among the restricting SIDs every process of the user, the Paneflow host included, would be writable from the pane (`PROCESS_VM_WRITE`), so the working directory is made to grant the shared sandbox SID instead. None of the four is granted by Paneflow's shared pipes (`OWNER RIGHTS` only, plus `SYSTEM` and Administrators for the window), and an `OWNER RIGHTS` entry never matches a restricted token. The pane's own pipes therefore grant the user's SID by name next to the per-pane SID.

| Check | Result |
|---|---|
| Open the shared host and worker pipes from the pane, for writing / for reading | Access denied (`ERROR_ACCESS_DENIED`) / access denied too |
| Open a pipe with the window's access list (`SYSTEM`, Administrators, `OWNER RIGHTS`) for writing | Access denied (added by the review) |
| Connect to the pane's host endpoint and call `system.whoami` | `pane` with its session |
| Write in the working directory, in `TEMP` (the pane's own, under its data directory), to `NUL` | Succeed |
| Read / write a file of the user outside those directories, write in `%USERPROFILE%` | Read succeeds / write denied, denied |
| Write in the data directory and the `TEMP` of another sandboxed pane | Access denied (after the review fix below) |
| Start `cmd.exe` with Rust's `Command` and read its output | Runs |
| `git init`, then `git status` in the working directory | Succeed |
| `cargo new`, then `cargo build --offline` with its target directory in the working directory | Succeed |
| `cargo build` with a target directory outside it (`CARGO_TARGET_DIR`, or `build.target-dir` in `~/.cargo/config.toml`) | Fails: `failed to open: ...\.cargo-build-lock`, access denied |
| `node -v` | Succeeds, `v24.16.0` |
| A named pipe made inside the pane with no access list, then opened for writing inside the pane | Access denied: the pipe file system gives such a pipe `SYSTEM`, Administrators and owner full control and Everyone read, and no restricting SID matches the owner |
| Node's `child_process.spawnSync` with piped output (`cmd.exe`, `bash.exe`, `git.exe`) | `EPERM` for each; with `stdio: 'inherit'` or `'ignore'` it runs |
| Claude Code 2.1.330 `claude -p` with no tool | Ran (`OK`); its plugin's `SessionEnd` hook failed with `EPERM: operation not permitted, uv_spawn` |
| Claude Code asked to run `echo` with its Bash tool | Failed: `EPERM: operation not permitted, uv_spawn 'C:\Program Files\Git\bin\bash.exe'`; its MCP servers failed to start the same way |
| `codex sandbox -- cmd /c echo` (Codex 0.165.0, its own Windows sandbox) | Failed: `windows sandbox failed: CreateRestrictedToken failed: 87` |

**Supported: no.** Every check this story names passes (the shared worker pipe is refused; `git status`, `cargo build` and `node -v` succeed in the working directory), but libuv, which Node and Bun use to start a child, creates its stdio pipes as named pipes with no access list, and no process under this token can open one for writing. Claude Code is a Bun program, so its Bash tool, its hooks and its MCP servers cannot start, which defeats the purpose of sandboxing an agent. Windows is listed unsupported for the sandboxed profile: `SANDBOX_SUPPORTED_PLATFORMS` stays `["linux"]` and a profile with `sandbox: "paneflow"` is refused on Windows. Lifting it needs a decision outside this epic, since the limit comes from the restricted token itself (TD-07): a dedicated sandbox user, as Codex's elevated mode does, gives every pipe an owner the sandbox matches; adding the user's SID to the restricting SIDs would fix pipes but expose every process of the user.

Two defects found and fixed on the way: a launch granted the shared sandbox SID on the user's whole `TEMP` (45 000 entries here), which rewrote that tree's access lists on every launch and took more than the 9 s launch budget (`LaunchPending`); and the pane endpoints granted `OWNER RIGHTS`, so even the pane's own endpoint refused it. The review of EP-006 found a third: the pane's data directory granted the shared sandbox SID, so any other sandboxed pane could write a file into it and into its `TEMP` (measured: both writes succeeded); it now grants the per-pane SID, and the same test refuses both writes. Because the working directory grants the shared SID, a sandboxed pane can still write the working directory of another sandboxed pane, which the sandbox does not confine (Non-Goals).

## macOS layer 1 report (US-029)

Measured on 2026-11-24 on macOS 27.2.1 (build 26B131), Apple Silicon (Apple M4), with the debug build of this branch. The `system.whoami` cases ran the real binaries in an isolated home (`~/.paneflow-dev-ep007`, removed afterwards), every inherited `PANEFLOW_*` variable cleared: `paneflow-host serve` with `RUST_LOG=debug`, the worker through `paneflow serve start`, a pane created with `paneflow-host session create --shell /bin/sh` whose shell called `paneflow api call system.whoami` itself, and a window of Terminal.app opened with `osascript` (Terminal, a child of `launchd`, then `login` and `zsh`).

| Check | Result |
|---|---|
| `paneflow api call system.whoami --on host` from the pane / from Terminal.app | `pane` with its session / `external` |
| `paneflow api call system.whoami --on worker` from the pane / from Terminal.app | `pane` with its session / `external` |
| The same call from a child of the pane's shell (`sh -c` under the shell) | `pane` with its session |
| A sandboxed pane (`session create --sandbox`) through its own endpoints, on the host and the worker | `pane` with its session |
| A raw client in a pane whose hello claims an engine calls `approval.decide` | Refused, `principal_refused` (`pane_authorization.rs`, `a_pane_that_claims_an_engine_cannot_decide_a_write_approval`, run on this Mac) |
| `sysctl` `KERN_PROC_PID` on a child of the user that called `setsid` | The parent PID and start time are returned; `getsid` returns the child's own PID as its session (`paneflow-peer`, `sysctl_kern_proc_names_the_parent_of_another_process_of_the_user_in_its_own_session`) |
| `kinfo_proc` against `proc_pidinfo` on every listed process | The parent and start time agreed on all 462 processes both could read; 278 more were readable through `sysctl` only, where `proc_pidinfo` answers `EPERM` |

A host log line reads `paneflow host: process 12727 (LOCAL_PEERTOKEN) classified pane through the host's pane leaders in 22 us`; classifications took 8 to 175 us.

**A9 holds with a correction.** `sysctl KERN_PROC` returns the parent PID of any process of the user, but `kinfo_proc` carries the session only as a kernel pointer (`e_sess`), so the session id comes from `getsid(2)`, which answers for any process of the user. The classifier already reads it that way (`crates/paneflow-peer/src/macos.rs`).

**Deferred: macOS on Intel.** No Intel Mac was available, and the criterion is deferred (v1.8): Arthur runs it after the merge to `main`. The code path is the same (`LOCAL_PEERTOKEN`, `proc_pidinfo`, the 648-byte `kinfo_proc` of LP64 macOS on both architectures), and the shipping targets name only `aarch64-apple-darwin`.

**Test failures found outside this epic, fixed (v1.10).** The full suite run on this Mac with `cargo test --workspace --locked --no-fail-fast` (5568 passed, 5 failed) found failures that predate EP-007 and touch none of its code. Two of them were races between tests that share one process, which `cargo test` does and CI's `cargo nextest run --profile ci` (one process per test, also on the `macOS aarch64 smoke build` job) does not:
- `paneflow-serve` `worker_worktrees` (4 tests): the worktrees root is process-wide state (`paneflow_git::worktree::set_worktrees_root`), which each test's worker sets to its own home, so a parallel test planned its worktree under another test's root and was refused as "outside Paneflow's worktree dirs". Each test of the file now holds one lock for its run (`crates/paneflow-serve/tests/worker_worktrees.rs`).
- `paneflow-app` `wrap_tests::up_and_down_walk_the_visual_rows_of_a_wrapped_line`: the editor display preferences were process-wide atomics, and `the_setting_sets_the_initial_state_and_the_toggle_stays_with_its_tab` turned word wrap on while the other test built its view, so its toggle turned wrap off. The preferences are now a GPUI global of the window's `App` (`EditorDisplay` in `src-app/src/app/diff_dock/code/controls.rs`), so each test application has its own and the window keeps one; the tests no longer save and restore a process-wide value. Measured with `cargo test -p paneflow-app --bin paneflow diff_dock::code -- --test-threads=16`: the old code failed 5 runs of 8, the new one passed 15 of 15.

The third was a stale test, failing on every platform and under nextest too: `paneflow-serve` `pane_authorization::an_action_runs_as_the_window_from_a_key_and_as_its_pane_from_a_pane` declared its actions without `agents = true`, so after EP-009 of `tasks/prd-lua-scripting.md` a `pane` caller got "no action named 'spawn'" (FR-15 of that PRD) instead of reaching the action whose principal the test checks. Its two actions are now marked `agents = true`; the refusal of an unmarked action keeps its own test (`crates/paneflow-serve/src/worker.rs`, "no action named 'secret'").

The first run of the CI gate on this Mac (`cargo nextest run --workspace --locked --profile ci`) then reported `pane_authorization::a_pane_cannot_attach_to_the_layout_nor_launch_through_it` FLAKY (failed, then passed on the retry), which fails the job: the pane client printed its report and exited, and when the test read the pane after the session ended, `session.text` answered "the session is not running". The pane client now keeps its pane open after its report, reading its terminal until the host closes the pane when the test ends.

## macOS sandbox report (US-032)

Measured on 2026-11-24 on the same machine, macOS 27.2.1, by the integration tests of `crates/paneflow-host/tests/sandboxed_pane_macos.rs`: a real `SessionHost` launches a sandboxed pane, whose shell is `/usr/bin/sandbox-exec -p <profile>` and then the test binary itself, which records what it can reach. With `PANEFLOW_SANDBOX_TOOLS_REPORT=<file>` it also runs the tools below and writes the report. The filter forms were measured with `sandbox-exec` directly, against a listening socket.

**The profile (US-026).** `(version 1)(allow default)`, then: `(deny network-outbound (remote unix-socket (path-literal ...)))` for each shared endpoint (host, worker, window) in both its spelling and its canonical form; a `subpath` deny on `panes/` in the home and a `subpath` allow on the pane's own `panes/<session>/`; a `file-write*` deny on the home except the pane's own data; a `file-write*` deny, as a `literal`, on each shared endpoint and on every directory above it and above the home (added by the review); and `(deny signal)`, `(deny process-info*)`, each followed by its allow `(target same-sandbox)`. The host runs `sandbox-exec -p <profile> /usr/bin/true` before the pane starts (about 6 ms), so a profile `sandbox-exec` refuses fails the launch with its error, as a missing `sandbox-exec` does.

| Check | Result |
|---|---|
| Connect to the shared host, worker and window sockets | Refused, `EPERM`, for each |
| Connect to another pane's endpoint under `panes/` | Refused, `EPERM` |
| Connect to the pane's host endpoint and call `system.whoami` | `pane` with its session |
| Connect to a socket outside Paneflow (in the working directory) | Connects (Non-Goals) |
| Filter forms on a socket's canonical path: `path-literal`, `literal`, `path`, `subpath`, `regex` | Each refuses the connection (A5) |
| A rule on the `/tmp/...` spelling of a socket under `/private/tmp` | Does not match; a rule on the canonical path matches both spellings |
| `kill(pid, 0)` and `SIGCONT` to a process outside the pane, `kill -0` from `sh` | Refused, `EPERM` |
| `proc_pidpath` of a process outside the pane | Refused, `EPERM` |
| `kill(pid, 0)` and `proc_pidpath` on the pane's own child | Succeed |
| `ps -p <pid>` | Cannot run: Seatbelt refuses to execute a setuid program (`/bin/ps` is setuid root) under any profile, `allow default` alone included |
| Write in the home / in the working directory | `EPERM` / succeeds |
| `git init`, then `git status` in the working directory | Succeed |
| `cargo new`, then `cargo build --offline` with its target directory in the working directory | Succeed |
| `bun -v` | Succeeds, `1.4.5` (Node is not installed on this Mac) |
| A second `sandbox-exec` profile inside the pane | Fails: `sandbox-exec: sandbox_apply: Operation not permitted` |
| Claude Code 2.1.331, `claude -p` with `sandbox.enabled` and `allowUnsandboxedCommands: false`, asked to run `echo` with its Bash tool | Did not run: its sandbox could not start, and Claude reported that macOS refuses a second sandbox in an already sandboxed process. The same command outside the pane ran and its sandbox refused the write to `$HOME` |
| `codex sandbox -- /bin/sh -c 'echo codex-sandbox-ran'` (Codex 0.165.0) | Failed: `sandbox-exec: sandbox_apply: Operation not permitted`, exit 71. Outside the pane it ran and refused a write to `/usr` |

**Signals (A7).** The profile written by inspection had `(deny signal (target others))`. Measured with `sandbox-exec`, that rule lets a signal through to a process of the caller's own session and refuses it only across sessions. A pane is its own session, so it held for the pane's shell, but only by that accident; the profile now denies `signal` and `process-info*` outright and allows them toward its own sandbox, which also refuses another pane running the same profile (measured: two `sandbox-exec` instances with one profile cannot signal each other). `process-info*` was not denied at all before, so `proc_pidpath` of any process of the user succeeded from the pane.

**Defect found by the review and fixed: a pane could move a shared socket out of its rule.** The shared host, worker and window sockets live in the runtime directory (`$TMPDIR`, `/var/folders/.../T`), which the profile left writable, and Seatbelt matches a socket rule against the path the caller connects to. Measured with `sandbox-exec`: `mv` of the socket to another name in the same directory, or of `T` itself to `T2`, then `connect` to the new path, succeeded, so a process that left its pane with `setsid` and a double fork could reach the host as `external`, which is the gap the sandbox exists to close; `rm` of the socket was allowed too. The profile now denies `file-write*` as a `literal` on each spelling of each shared endpoint and of every directory above it, and of the home and every directory above it, which refuses moving, removing or `chmod` of those entries while files and sockets made inside the runtime directory stay writable. The regression test `a_pane_cannot_move_a_shared_socket_or_its_directory_to_reach_it_under_another_name` (`crates/paneflow-host/src/sandbox/macos.rs`) runs the generated profile under `sandbox-exec`: it fails without the rules (the socket moved) and passes with them. A hard link to a socket is refused by Seatbelt already, and a symlink, a `..` or `.` spelling and a relative path all match the rule. Measured on 27.2.1 only; the 26.6.2 machine was not available to the review.

**Supported: no.** The sandbox holds on macOS 27.2.1: every shared endpoint and every other pane's endpoint is refused, the pane's own endpoints answer `pane`, and no process outside the pane can be signalled or inspected. But Seatbelt does not nest: once the pane's profile is applied, no process in it can apply another, so Claude Code's sandbox and Codex's sandbox both fail there, and an agent sandboxed by Paneflow would have to run without its own file and network policy, which the Non-Goals forbid. macOS 27.2.1 is listed unsupported for the sandboxed profile: `SANDBOX_SUPPORTED_PLATFORMS` stays `["linux"]` and a profile with `sandbox: "paneflow"` is refused on macOS. The test that proves the refusal (`a_pane_whose_seatbelt_profile_is_refused_fails_its_launch_with_the_error`) relies on the same limit: a host that itself runs under a profile cannot start a sandboxed pane, and the pane fails its launch with `sandbox_apply: Operation not permitted`. Lifting it needs a decision outside this epic: Paneflow could instead hand the agent a combined profile (its own rules plus the agent's, applied once), which means knowing each agent's sandbox, or keep a sandbox-less pane whose shared endpoints are refused by other means.

**The previous release, macOS 26.6.2.** Measured on 2026-11-24 in a Tart 2.42.0 virtual machine on the same Mac, booted from `ghcr.io/cirruslabs/macos-tahoe-vanilla:26.6.2` (build 25G83, the release where R9 reported that literal and subpath filters stopped matching). The test binary and the `paneflow-host` binary built on 27.2.1 (minimum macOS 11) ran there unchanged, with the filter-form script and Codex 0.165.0.

| Check on macOS 26.6.2 | Result |
|---|---|
| The four tests of `sandboxed_pane_macos.rs` | Pass: the shared host, worker and window sockets and another pane's endpoint are refused (`EPERM`), the pane's host endpoint answers `pane` with its session, `kill(pid, 0)`, `SIGCONT`, `kill -0` and `proc_pidpath` toward a process outside the pane are refused, `ps` cannot run, and a refused profile fails the launch with `sandbox_apply: Operation not permitted` |
| Filter forms on a socket's canonical path: `path-literal`, `literal`, `path`, `subpath`, `regex` | Each refuses the connection, as on 27.2.1; a rule on the `/tmp/...` spelling does not match |
| A second `sandbox-exec` profile inside the pane, and `codex sandbox` inside it | Fail: `sandbox-exec: sandbox_apply: Operation not permitted`. Outside the pane, `codex sandbox` ran and refused a write to `/usr` |
| Claude Code, `git`, `cargo`, `bun` | Not run: the image has no developer tools and no Claude Code credentials; on 27.2.1 they are measured above |

R9's report does not reproduce on 26.6.2 for the socket rule: every filter form matches the canonical path, so the profile's rules hold on both releases. Both releases are unsupported for the sandboxed profile for the same reason, the nesting limit.

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior | Message |
|----------|---------|-------------------|---------|
| Peer exits before classification | Short-lived client | `Unverified`; only `system.whoami`, `system.capabilities` and `system.describe` answer | "Paneflow could not identify the calling process: <OS error>" |
| Host unreachable while the worker classifies | Host restarting | Classification from the session manifests (Linux, macOS); `Unverified` (Windows) | None, debug log |
| Grant set stale in the host or the window | Window just reconnected | `grants_unsynced`, retryable | "Grants are being synchronized; retry the call" |
| Pane leader PID reused | Long-running instance | Start-time mismatch, `NotInPane` | None |
| Agent escapes with `setsid` and a double fork (Unix) | Hostile process | Classified `External`; documented limit, closed by EP-004 | None |
| An ancestor hidden by `hidepid=2` (Linux) | `/proc` mounted `hidepid=2` | The hidden ancestor ends the parent chain, as `init` does; a pane process still reads `pane` (US-031) | None |
| An agent starts its own Seatbelt sandbox in a Seatbelt pane (macOS) | Claude Code or Codex sandboxed command | `sandbox_apply` refused; the reason macOS stays unsupported (US-032) | "sandbox-exec: sandbox_apply: Operation not permitted" |
| Sandboxed profile on an unsupported platform | Config load | Config error naming the platform and profile | "Sandbox 'paneflow' is not available on <platform>" |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| The console host escapes the pane's Job Object under ConPTY | Medium (R6) | Pane processes misclassified on Windows | Parent-chain fallback (US-005), hardware report (US-030) |
| The named-pipe client PID can be forged | Medium (R5) | Window impersonation on Windows | Token-user check (US-003), hardware probe (US-030) |
| Conductor users who set `PANEFLOW_IPC_SCRIPTING` for an agent in a pane lose `send --submit` on update | Medium (the conductor docs tell them to set it, `docs/user/conductor.md:78`) | Their agents' submitted sends are refused | One deprecation release (US-010), migration lines (US-015) |
| bubblewrap blocked by AppArmor on Ubuntu 24.04 | High (A4) | No Linux sandbox there | US-023 before US-024; supported list in the report |
| Seatbelt socket filters stop matching on a macOS release | Medium (R9) | Sandbox leaks on macOS | Per-release check (US-032) |

## Open Questions

- **Length of the deprecation.** US-010 keeps the environment variable for panes during the release that ships it and drops it in the next one. A longer window would need a version named by Arthur.
- **D-Bus inside the Linux sandbox.** The runtime directory of a sandboxed pane is an allowlist (v1.4) without the session bus, so a keyring read over D-Bus, `notify-send` and `xdg-open` fail there. A filtering proxy (`xdg-dbus-proxy`, as Flatpak uses) could give back the secret service, notifications and portals while still refusing `org.freedesktop.systemd1`; it is not built.
- **Grants across a worker restart.** US-016 restores them so that a self-update does not cut an agent's authority mid-task; ending them with the worker is the stricter choice if Arthur prefers it.

## Files NOT to Modify

- `vendor/**`: upstream copies; changes follow their `Cargo.toml` headers only.
- `native/libghostty/**`: the terminal engine boundary, unrelated.
- `protocol/api-v1.json`: generated from the catalog; regenerate it, never edit it by hand.
- `crates/paneflow-shim/**`: the 512 KiB shim is not a server and stays free of identity code.
[/PRD]
