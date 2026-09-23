[PRD]
# PRD: Persistent Session Reliability and Host Refactor

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-19 | Arthur Jean | Persistent-session audit remediation, host ownership and resource refactor, and separate Windows, Linux, and macOS qualification epics |
| 1.1 | 2026-09-21 | Arthur Jean | Rebase on the existing host/worker/desktop topology; incorporate the Herdr and Unpeel audit; specify non-launching restoration, confirmed stop outcomes, passive ended panes, detached-session access, and generation guards for all asynchronous producers |
| 1.2 | 2026-09-22 | Arthur Jean | Amend the EP-005 Linux qualification scope: ARM64 evidence from the native CI job, Fedora as the only full native cell, other distributions as container host-only evidence, an endurance rehearsal instead of the 8-hour run, and a short manual smoke instead of the interactive passes |
| 1.3 | 2026-09-23 | Arthur Jean | Remove the 300 s, three-run W01 window from every qualification: NFR-01 is decided on the harness short window |
| 1.4 | 2026-09-23 | Arthur Jean | Drop the Stop-everything check from the Linux manual smoke: a non-root user cannot start a pane process that survives the stop, so the unresolved stop-all path is covered by automated tests |
| 1.5 | 2026-09-23 | Arthur Jean | Align the EP-006 macOS qualification with EP-004 and EP-005: the W08 evidence is an endurance rehearsal that passes every decision, and the 8-hour run is optional on every platform; NFR-11 cycle counts and NFR-12 follow |
| 1.6 | 2026-09-23 | Arthur Jean | Replace the EP-006 Scaleway rental with evidence from the hosted macOS ARM64 runner: the CI gates and render smoke, then one package workflow run with the preflight, the full protocol, the W08 endurance rehearsal, and paired baseline/candidate host profiles, decide macOS at the candidate SHA; dedicated-hardware performance, an interactive manual smoke on a physical Mac, local-display latency, and the 8-hour run are recorded `unavailable`; the rental stays an optional route |

## Problem Statement

1. **Keeping sessions running is not consistently safe across application upgrades.** Bootstrap currently retires a host whose build version differs by sending a forced shutdown before resolving the replacement executable. This can terminate retained agents merely because the desktop was updated. See finding A01 below.
2. **Memory and background work outlive their usefulness.** Attachment clones retain the initial native checkpoint after restoration. Ended records retain a terminal runtime, output tail, and timed receive loop. The displayed recent-session limit is not a limit on retained runtime resources. See A02-A04.
3. **Ordinary idle use can lose input.** The host expires the control connection after 60 seconds while the desktop caches it. The next input can be rejected against the stale connection even though output remains attached. See A05.
4. **Lifecycle transitions are not fully serialized.** Old-generation callbacks can modify a restarted manifest; a failed restart write can strand `Starting`; a timed-out spawn can later create an unregistered process; a child-wait error can be mistaken for confirmed exit. See A07-A09 and A11.
5. **Persistence lies on the PTY processing path.** Metadata callbacks synchronously write manifests under a shared writer lock. Concurrent hook seed writes also share a temporary filename without the same serialization. These paths couple terminal progress to disk latency and can publish inconsistent state. See A06 and A10.
6. **Existing terminal benchmarks do not qualify the persistent path.** The local terminal microbenchmarks do not establish the cost or reliability of the complete host, IPC, follower, mirror, and desktop lifecycle. Static risks must be reproduced, and improvements must be measured on each shipping OS.
7. **Restoring the desktop can silently launch a replacement session.** Saved panels use `Resume`, which creates a missing session or restarts a non-live record. Reopening a layout must distinguish reattachment from an explicit new generation. See A12.
8. **User-visible lifecycle decisions do not consistently preserve uncertainty or access.** Global stop-and-quit ignores failed outcomes, stale agent state can become known absence, kept-running sessions can disappear from available workspace views, and ordinary child exit closes the pane. See A13-A16 and A18.

**Why now:** Arthur has approved updating the reliability plan after a source audit of Paneflow, Herdr, and Unpeel. Paneflow now has the host/worker/desktop split and additional asynchronous producers, so the contract must cover the actual integrated topology and its user entry points. This revision authorizes planning only; it records the implementation scope without performing it.

## Overview

Preserve Paneflow's detached host and libghostty architecture. Introduce one generation-aware lifecycle authority per session, event-driven output and exit delivery, explicit startup ownership, bounded queues, and a serialized persistence service outside PTY processing. Separate a live runtime from a completed session record. Consume attachment snapshots once and release live resources after confirmed exit and final-output handling.

This is one integrated delivery with three implementation epics followed by three platform qualification epics. Every implementation story includes a working Windows, Linux, and macOS path. The platform epics exercise the completed system; they are not deferred platform ports. Windows and Linux use native test environments. macOS uses the hosted Apple Silicon CI runner: the CI gates and render smoke, then a packaged qualification run on the same kind of virtual machine, with the cells that need physical hardware recorded `unavailable` (amended in v1.6).

The scope is the persistent core, its existing `paneflow serve` consumer, and desktop attachment/lifecycle behavior. The separate **Agent Runtime System** PRD continues to own the worker architecture, activity reducer, provider catalog, and remote Controller work. The worker already exists and is part of every integrated qualification here; scoped consumer changes may be needed to adopt the persistent-core contract, without redesigning the reducer or certifying the other PRD. This delivery does not introduce another worker, a second process owner, continuous terminal journaling, process resurrection after host death, or new shipping architectures.

Saved layouts and retryable attachments never create or restart a session. Only a new-terminal action may create one, and only an explicit restart action may replace an eligible known record with a new generation. Natural exit leaves an attached pane available as a passive final view until the user closes it. Keeping a session running also preserves a visible route back to it when its workspace is closed or unavailable. An unresolved stop is shown as unresolved through the final desktop action.

## Goals

Month 1 and Month 6 below are relative to the first accepted implementation candidate. They are verification milestones, not a promised release date. Targets are proposed acceptance budgets, not measurements already achieved.

| Goal | Month-1 Target | Month-6 Target |
|------|----------------|----------------|
| Preserve live processes during desktop detach and compatible upgrade | 0 unexpected child exits in the qualification matrix | 0 regressions in the same automated matrix on release candidates |
| Eliminate lifecycle ambiguity being reported as success | 0 orphaned fixture processes and 0 false confirmed exits in fault-injection cases | Same gates retained for every lifecycle change |
| Make restoration and retained work predictable | 0 implicit launches during layout restoration; all kept-running fixture sessions remain reachable through the desktop | Same entry-point regressions retained for subsequent lifecycle changes |
| Bound idle work and retained resources | Meet NFR-01 through NFR-06 at 1, 10, and 50 sessions | No budget regression without a reviewed requirement and new evidence |
| Make qualification reproducible | Evidence bundle for all 4 shipping target triples and all 3 OS epics | Repeat affected platform evidence for subsequent changes |

## Target Users

### Developer running multiple agents

- **Role:** Uses persistent terminals and coding agents in several workspaces on Windows, Linux, or macOS.
- **Behaviors:** Leaves sessions running, closes and reopens the desktop, changes workspaces, resumes typing after long idle periods, and updates Paneflow.
- **Pain points:** Unexpected process termination or relaunch, missing first input, unexplained memory retention, final output disappearing, hidden kept-running sessions, and uncertainty about whether a session is alive or recoverable.
- **Current workaround:** Keeps the desktop open, inspects OS process tools, reconnects manually, or restarts the application. These are inferred workarounds, not measured user-research results.
- **Success looks like:** Closing the desktop preserves the intended children; returning to a session preserves terminal state; explicit stop reports what actually happened; resources return within the specified budgets.

### Maintainer qualifying a release

- **Role:** Implements and reviews session changes and verifies packaged builds.
- **Behaviors:** Runs Rust gates, native PTY integration tests, fault-injection cases, performance comparisons, and installed-app exercises.
- **Pain points:** A green aggregate CI status can hide skipped platform jobs; microbenchmarks can miss host overhead; passing compilation does not establish lifecycle or UI behavior.
- **Current workaround:** Reads logs and manually compares process counts and memory without a persistent-path evidence format.
- **Success looks like:** One repeatable scenario catalog and evidence bundle identifies the candidate, environment, measurements, failures, and untested coverage.

## Research Findings

### Evidence boundary and current checkout

- Paneflow source baseline: `f881c8bcce568c76d96ba7e691a0e26d86154215`, branch `main`, inspected on 2026-09-21. The working tree has unrelated agent-guidance, terminal contrast/fixtures, Ghostty, terminal benchmark, and theme/palette changes. Preserve them. Version 1.0 inspected `5db6249a210c035b8e571e21416fa9b90c76cefb`; it is historical context, not the current topology. A benchmark must record its actual commit and dirty-state fingerprint; neither audit commit is automatically a performance baseline.
- Herdr reference: `C:/dev/herdr`, clean checkout at `5a649142233631f8407b4099da0e8e78dfef8574`. Unpeel reference: `C:/dev/unpeel`, clean checkout at `a66359c64494bc786333821067bb1aeda73f6e16`. The references below describe inspected mechanisms and limitations, not a requirement to copy either implementation.
- Findings A01-A18 are source-inspection findings. No runtime failure reproduction, CPU measurement, heap profile, CI-run verification, or platform qualification was performed while writing or revising this PRD. Primary-source documentation was checked for the architecture and native API contracts; its examples do not qualify Paneflow.
- Earlier work already reserves ordinary restarts with `Starting` under the session lock, retains uncertainty after an unconfirmed stop timeout, and asks before closing an unknown agent. Preserve these improvements. They do not eliminate stale callbacks or the separate wait-error path.

### Audit-to-requirement traceability

| ID | Current evidence and trigger | Required resolution | Stories |
|----|------------------------------|---------------------|---------|
| A01 | [`bootstrap.rs::retire_host`](C:/dev/paneflow/crates/paneflow-host/src/bootstrap.rs:206), `ensure_host_running` at 245: build mismatch forces shutdown before replacement resolution | Non-destructive compatibility and upgrade behavior | US-002, US-014 |
| A02 | [`HostControl::new`](C:/dev/paneflow/src-app/src/terminal/ghostty_session.rs:4592) and follower clone at 4719 retain `Checkpoint.snapshot: Vec<u8>` | Consume snapshot; keep lightweight attachment identity | US-008 |
| A03 | [`runtime.rs::run`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:318) continues after exit; [`host.rs::trim_terminated_records`](C:/dev/paneflow/crates/paneflow-host/src/host.rs:566) only runs at startup and successful create | Retire live resources separately from retained records; scheduled retention | US-007, US-009 |
| A04 | [`runtime.rs`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:21) uses 20 ms ticks; [`server.rs`](C:/dev/paneflow/crates/paneflow-host/src/server.rs:43) polls followers every 15 ms | Notifications for output and exit; deadline-driven maintenance | US-006, US-007 |
| A05 | [`server.rs`](C:/dev/paneflow/crates/paneflow-host/src/server.rs:42) expires control at 60 s; [`HostControl::send_input`](C:/dev/paneflow/src-app/src/terminal/ghostty_session.rs:4633) drops input on cached-connection failure | Idle-safe control and explicit ambiguous-delivery semantics | US-010 |
| A06 | [`runtime.rs::feed`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:470) calls [`observer_for`](C:/dev/paneflow/crates/paneflow-host/src/host.rs:1331) which writes under a shared lock; snapshots are prepared before writer acquisition | Ordered, bounded persistence outside PTY processing; reject stale revisions at commit | US-011 |
| A07 | [`record_exit`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:699) publishes exit before observer; [`stop`](C:/dev/paneflow/crates/paneflow-host/src/host.rs:1061) writes after waiting; observer has no generation guard | Generation and operation tokens on every asynchronous result | US-003 |
| A08 | [`restart`](C:/dev/paneflow/crates/paneflow-host/src/host.rs:872) publishes `Starting` before fallible persistence at 945 | Recoverable, transactional restart transition | US-003 |
| A09 | [`SessionRuntime::spawn`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:157) times out without cancellation; runtime thread ignores failed startup delivery at 334 | Retain ownership until late spawn is resolved and any child reaped | US-001, US-004 |
| A10 | [`write_last_hook_event` call](C:/dev/paneflow/crates/paneflow-host/src/host.rs:739) is outside manifest writer; [`write_atomically`](C:/dev/paneflow/crates/paneflow-host/src/manifest.rs:154) uses a process-only temporary suffix | Unique temporary files, revision ordering, accepted-event recovery | US-005, US-011 |
| A11 | [`observe_exit`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:682) converts wait failure into `Exited(-1)` | Observation errors remain unverified; no fabricated exit | US-004, US-007 |
| A12 | [`attach_restored`](C:/dev/paneflow/src-app/src/terminal/view.rs:408) selects `Resume`; [`resolve`](C:/dev/paneflow/src-app/src/terminal/host_link.rs:292) creates missing sessions or restarts non-live/non-owned records; an ended [`sidebar row`](C:/dev/paneflow/src-app/src/app/sidebar/mod.rs:1643) also resumes | Restoration, ordinary row opening, and attachment retries perform no launch; explicit actions distinguish create and restart | US-002, US-009, US-010 |
| A13 | [`stop_sessions_and_shutdown`](C:/dev/paneflow/src-app/src/terminal/host_link.rs:346) does not propagate shutdown failure completely; [`quit_dialog.rs`](C:/dev/paneflow/src-app/src/app/quit_dialog.rs:233) ignores the returned stop failures | Preserve unresolved results through stop-and-quit/restart; no automatic exit after an unconfirmed stop | US-004 |
| A14 | [`agent_reading`](C:/dev/paneflow/src-app/src/app/close_policy.rs:178) filters stale worker state into `Known(None)` when no local state remains | Missing or stale activity is unknown, not confirmed absence; keep confirmation and no implicit stop | US-004 |
| A15 | [`hosted_sessions.rs`](C:/dev/paneflow/src-app/src/app/hosted_sessions.rs:130) discovers orphan sessions through a matching open workspace cwd | Expose kept-running sessions without an open or accessible workspace; preserve identity on reattachment | US-010 |
| A16 | [`should_close_on_exit`](C:/dev/paneflow/src-app/src/terminal/pty_session.rs:1383) closes on success or prior keyboard input; [`event_handlers.rs`](C:/dev/paneflow/src-app/src/app/event_handlers.rs:490) routes surface exit to stop/removal | Natural exit keeps a passive ended view; only explicit close/remove applies removal policy | US-009 |
| A17 | [`viewport_scan.rs`](C:/dev/paneflow/crates/paneflow-host/src/viewport_scan.rs:152) applies delayed observations without a generation guard; [`cancellation_scan.rs`](C:/dev/paneflow/crates/paneflow-host/src/cancellation_scan.rs:30) reads generation after draining signals | Tag observations and signals when captured, reject stale application/write, and reset generation-scoped trackers | US-003, US-005, US-011 |
| A18 | [`terminate_child`](C:/dev/paneflow/crates/paneflow-host/src/runtime.rs:792) ignores the Windows tree-cleanup result; root exit can hide unresolved descendants | Retain descendant identities/outcomes independently of root exit and block successful stop/removal/shutdown until resolved | US-004, US-007 |

### Current topology and persistence guarantees

The desktop already starts both the core and worker: [`main.rs`](C:/dev/paneflow/src-app/src/main.rs:1878), [`host_agents.rs`](C:/dev/paneflow/src-app/src/app/host_agents.rs:263). The host alone owns PTYs and canonical terminal state. The worker is a replaceable consumer of manifests, seeds, and host observations; the desktop owns attachment mirrors. This PRD hardens their shared lifecycle contract without moving process ownership to the worker or desktop.

| User operation | Required guarantee | Deliberate limit |
|----------------|--------------------|------------------|
| Detach or reopen a desktop client | Same live child identities and exact native terminal checkpoint/offset continuation | A saved layout is not authority to launch |
| Restart/update the worker or a compatible desktop | Reuse the live host; rebuild projections without signaling sessions | Advertised protocol and snapshot compatibility must actually hold |
| Encounter an incompatible host | Keep it and its children running; expose the mismatch and an explicit recovery action | No live PTY transfer or seamless incompatible-engine attachment |
| Recover after host death or OS restart | Retain known identities and honest lost/unverified/ended state; show final text only when available | No process resurrection, automatic shell/agent launch, command replay, or promised full-history recovery |

Preserve Paneflow's existing strengths: atomic native snapshot plus output offset, continuation state, output-eviction resynchronization, host-instance checks, and host-only terminal-query responses. Herdr's repaint-oriented handoff and Unpeel's disk replay are not substitutes for that attachment fidelity.

### Competitive Context

- **Herdr:** detached server ownership, client projections, and a phased live-server handoff with rollback under Unix. Its handoff is unavailable on Windows and rebuilds terminal presentation from a new engine plus bounded ANSI/mode state; it does not transfer Paneflow-equivalent complete native terminal checkpoints. Its cold restore creates new shells or invokes provider resume, and disk history is experimental and off by default. These are separate guarantees, not evidence that one persistence system is universally superior.
- **Unpeel:** replaceable worker over a detached PTY core, event-driven shared reactor, bounded subscriber queues, disk journal, reduced resident scrollback, and explicit memory benchmarks. Its host is Unix-only. The audited checkout disables core handoff by default after a recorded regression; it lets the old core drain while new sessions use individual hosts. Paneflow adopts ownership, adoption, and resource-accounting principles without importing the reactor, journal, or weaker EOF/exit behavior.
- **tmux:** a background server owns terminal sessions independently of clients. This supports retaining Paneflow's host/client separation. [Official getting-started guide](https://github.com/tmux/tmux/wiki/Getting-Started).
- **WezTerm:** mux domains separate hosted terminals from GUI attachment. This supports a small attachment contract rather than moving process ownership into the renderer. [Official multiplexing guide](https://wezterm.org/multiplexing.html).
- **Zellij:** session resurrection saves layout and commands and can serialize viewport/scrollback. It recreates commands rather than preserving a dead process. Paneflow must distinguish reattachment from a new generation and must not silently replay commands. [Official session resurrection guide](https://zellij.dev/documentation/session-resurrection.html).
- **Product need:** reliable local multi-agent persistence with bounded resource behavior on the shipping Windows, Linux, and macOS targets. This is a product requirement, not an unsupported market-exclusivity claim.

### Relevant Unpeel references

| ID | Reference at the pinned checkout | Reuse and limits |
|----|----------------------------------|------------------|
| U01 | [`core_reactor.rs`](C:/dev/unpeel/crates/unpeel-core/src/core_reactor.rs:710); [`session_io.rs`](C:/dev/unpeel/crates/unpeel-core/src/session_io.rs:500) | Event-driven readiness and subscriber isolation. Terminal-query responses can still block the shared reactor; do not copy that write path or infer a universal child reaper from the reactor's process-event branch. |
| U02 | [`session_io.rs`](C:/dev/unpeel/crates/unpeel-core/src/session_io.rs:546) | Journal writer pressure suspends PTY reads for the affected session. Reduced resident history depends on that journal and is not an equivalent no-journal memory target for Paneflow. |
| U03 | [`session_io.rs snapshot/offset capture`](C:/dev/unpeel/crates/unpeel-core/src/session_io.rs:1290) | Snapshot plus absolute output offset is one consistent cut. Paneflow already has this invariant and must preserve it. |
| U04 | [`session_host.rs`](C:/dev/unpeel/crates/unpeel-core/src/session_host.rs:1944) | Physical journal retention is disabled when hole punching is unsupported; do not claim a universal disk bound. Continuous journaling is excluded here. |
| U05 | [`bench-memory.yml`](C:/dev/unpeel/.github/workflows/bench-memory.yml:39); [`bench-thresholds.json`](C:/dev/unpeel/scripts/bench-thresholds.json:2) | Compare idle, session/client increments, and reclamation. Reuse the measurement dimensions, not historical numbers or assertions that its latest CI passed. |
| U06 | [`pty_core_supervisor.rs`](C:/dev/unpeel/crates/unpeel-serve/src/pty_core_supervisor.rs:321); [`session_host.rs`](C:/dev/unpeel/crates/unpeel-core/src/session_host.rs:5796) | Core handoff is off by default after a documented regression; hosting is Unix-only. Retain compatible owners instead of assuming cross-platform live transfer. |
| U07 | [`SessionTeardown::run`](C:/dev/unpeel/crates/unpeel-core/src/session_io.rs:1183); [`session_ops.rs`](C:/dev/unpeel/crates/unpeel-core/src/session_ops.rs:2417) | EOF retirement can publish exit without a confirmed child status; replacement removes old artifacts before spawning. Paneflow must keep its stronger confirmed-exit and recoverable-transition requirements. |

All Unpeel paths can be resolved against the [pinned source tree](https://github.com/unpeel-com/unpeel/tree/a66359c64494bc786333821067bb1aeda73f6e16). Read the referenced symbols at implementation time; line numbers are navigation hints. Substantial copied code must retain the MIT notice from [Unpeel's LICENSE](C:/dev/unpeel/LICENSE) in the appropriate notice artifact. This planning revision copies no implementation.

### Relevant Herdr references

| ID | Reference at the pinned checkout | Reuse and limits |
|----|----------------------------------|------------------|
| R01 | [`headless.rs`](C:/dev/herdr/src/server/headless.rs:1013) | Disconnecting a client releases client controls without deleting server-owned runtimes. Preserve the same separation in Paneflow. |
| R02 | [`headless/lifecycle.rs`](C:/dev/herdr/src/server/headless/lifecycle.rs:57); [`headless/bootstrap.rs`](C:/dev/herdr/src/server/headless/bootstrap.rs:187) | Unix handoff uses staged ownership transfer and rollback. Windows explicitly refuses it at lifecycle.rs:228; live host migration stays out of this delivery. |
| R03 | [`pane.rs`](C:/dev/herdr/src/pane.rs:2280); [`handoff.rs`](C:/dev/herdr/src/server/handoff.rs:31); [`lifecycle.rs`](C:/dev/herdr/src/server/headless/lifecycle.rs:85) | Handoff creates a new terminal, restores selected modes and at most 8 KiB ANSI history, omitting history for identified agents and alternate-screen cases. Preserve Paneflow's exact native checkpoint contract instead of depending on repaint. |
| R04 | [`pane.rs`](C:/dev/herdr/src/pane.rs:2377); [`platform/windows.rs`](C:/dev/herdr/src/platform/windows.rs:2234) | Post-handoff reader completion substitutes for child wait; Windows termination uses a handle without the required terminate right and ignores the result. These inspected paths are warnings against weakening confirmed-exit requirements, not runtime qualification results. |
| R05 | [`persist/restore.rs`](C:/dev/herdr/src/persist/restore.rs:569); [`config/model.rs`](C:/dev/herdr/src/config/model.rs:1040) | Cold restore starts new shells or provider resume; experimental disk history is off by default. Restoring presentation must not be described as process survival. |
| R06 | [`pty/actor/unix.rs`](C:/dev/herdr/src/pty/actor/unix.rs:487); [`pane.rs`](C:/dev/herdr/src/pane.rs:1519); [`live_handoff.rs`](C:/dev/herdr/tests/live_handoff.rs:941) | Audit byte budgets across internal queues and preserve unresolved owners after termination failure. Reuse real process/IO survival scenarios as test ideas; no Herdr tests were executed here. |

The selected design uses Herdr as evidence for explicit ownership and staged transitions, and Unpeel as evidence for core/worker separation and resource accounting. Neither is a fidelity, platform, performance, or release-certification oracle for Paneflow.

### Best Practices Applied

- `portable-pty` 0.9.0 provides blocking `Child::wait` and `ChildKiller::clone_killer`, which permits separate waiting and signaling. Its synchronous `spawn_command` has no cancellation token. A launch deadline therefore cannot be treated as cancellation. Context7 results were checked against the locally pinned source. [Child](https://docs.rs/portable-pty/0.9.0/portable_pty/trait.Child.html), [ChildKiller](https://docs.rs/portable-pty/0.9.0/portable_pty/trait.ChildKiller.html), [SlavePty](https://docs.rs/portable-pty/0.9.0/portable_pty/trait.SlavePty.html).
- A cloned killer separates signaling from waiting; it does not promise process-tree termination or confirmed exit. The pinned Unix cloned signaller targets the child PID, while native descendant ownership remains Paneflow's responsibility. [Pinned library source](https://docs.rs/portable-pty/0.9.0/src/portable_pty/lib.rs.html).
- Windows pseudoconsole closure and child-process exit are separate facts. Close behavior differs across console versions, and output servicing must not deadlock against input or shutdown. Test the bundled ConPTY, not a guessed OS implementation. [Microsoft closure semantics](https://learn.microsoft.com/en-us/windows/console/closepseudoconsole), [pseudoconsole I/O guidance](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session).
- Windows `TerminateProcess` requires a handle with `PROCESS_TERMINATE` and is asynchronous for another process; successful signaling is not completion. Retained handles and a wait outcome establish child completion separately from descendant cleanup. [Microsoft termination contract](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess).
- Standard GitHub-hosted macOS runners are free for public repositories; larger runners are not covered by that statement. The repository currently ships only macOS ARM64 despite Intel runners being available. [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
- The hosted `macos-14` runner is an Apple M1 virtual machine (`VirtualMac2,1`, 3 vCPUs, 7 GiB) with a console graphical session. `system_profiler` reports no Metal support there, yet the packaged desktop renders its window under GPUI: preflight M03 and M13 pass with a screenshot and M04 fails, in package workflow run 35860309088 on 2026-09-23. It is a shared VM, so its timings are not dedicated-hardware evidence, and one job runs at most 6 hours. [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), [job limits](https://docs.github.com/en/actions/reference/limits).
- Scaleway offers physical Apple Silicon hosts with SSH/VNC and a 24-hour minimum rental; on 2026-09-23 the M4-S Mac mini was listed at EUR 0.22/hour, about EUR 5.28 for 24 hours before tax. Since v1.6 the rental is an optional route for the `unavailable` macOS cells, not a requirement. [Scaleway quickstart](https://www.scaleway.com/en/docs/apple-silicon/quickstart/), [pricing](https://www.scaleway.com/en/pricing/apple-silicon/), [minimum rental FAQ](https://www.scaleway.com/en/docs/apple-silicon/faq/).

## Assumptions & Constraints

### Assumptions (to validate)

| ID | Assumption | Risk | Validation owner and consequence |
|----|------------|------|----------------------------------|
| H01 | A blocking child waiter plus independent verified termination can remove periodic exit polling on every shipping target without preventing PTY drainage | High | US-001 validates the adapter on native targets; US-007 may not land a polling fallback as the completed design if validation fails. |
| H02 | Existing operation ownership can retain a synchronous launch after caller timeout and safely clean up a late child without an untracked process | High | US-001 injects a delayed launch; US-004 implements ownership through completion. A wedged launch remains explicitly pending and blocks replacement. |
| H03 | Snapshot and ended-runtime retention materially contribute to observed RAM concerns | Medium | US-001 records ownership counters and native memory; US-008/009 must prove release even if total RSS reductions are smaller than expected. |
| H04 | The initial NFR budgets are achievable without changing the terminal engine or introducing a disk journal | Medium | US-001 measures the baseline, US-013 measures the candidate. A miss requires a diagnosed fix or an explicit PRD revision, not silently raised thresholds. |
| H05 | The hosted macOS ARM64 runner runs the packaged host and desktop in its graphical session and renders the GPUI window | Low | Observed on 2026-09-23 in package workflow run 35860309088 (M03 and M13 pass, screenshot); US-020 verifies it again at the candidate SHA. A failure leaves macOS unqualified; the optional rental is then the fallback route, at Arthur's decision. |
| H06 | The existing worker and its viewport/cancellation inputs can adopt the persistent-core contract without duplicating process ownership or regressing agent projection | Medium | US-003 validates generation barriers; US-013/014 exercise worker death/restart and record consumer integration. Neither PRD is auto-certified from the other's tracker. |

### Hard Constraints

- Implement working code for Windows, Linux, and macOS in the implementation epics. No platform may be left as an unimplemented stub until its qualification epic.
- Preserve the sole libghostty engine, `TerminalSessionBackend`, neutral renderer types, existing native ConPTY pin, GPUI revision, and macOS `font-kit` feature. No parser replacement, transport pin change, or GUI-thread blocking I/O is needed by this PRD.
- Use the actual shipping archive matrix in [`native/libghostty/manifest.toml`](C:/dev/paneflow/native/libghostty/manifest.toml): Windows `x86_64-pc-windows-msvc`, Linux `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, macOS `aarch64-apple-darwin`. Preserve portability to Intel macOS and Windows ARM64, but do not add archives or claim native qualification for those currently nonshipping targets.
- All tests and benchmarks use a private `PANEFLOW_HOME`, recorded process identities, and dedicated temporary fixture directories. They must never stop a developer's real sessions or read provider credentials/transcripts for measurements.
- Preserve session IDs, recorded workspace associations, shell selection, and explicit close-policy choices. Deliberate behavior changes in this PRD replace implicit restore/row-click launches, automatic natural-exit closure, and stale-as-absent decisions. Explicit restart starts the recorded shell as a new generation; it does not replay user input or agent commands. A missing record requires an explicit new-terminal action with a fresh identity.
- Process identity includes the session, host instance, generation, and kernel start identity or retained OS process handle. PID alone is never sufficient for signaling an unowned process. A lost host cannot reattach a surviving child to a new PTY by assumption.
- Keep the Rust 1.98.0 toolchain, existing `portable-pty` 0.9.0 and `interprocess` 2.4.3 unless a demonstrated requirement justifies a scoped dependency change. No new Tokio reactor, database, or always-running service is part of this design.
- Keep all per-user state under the established `paneflow-home` layout. No new global config/cache directory and no continuous raw-output journal.
- Preserve unrelated dirty files and existing PRDs. This document and its tracker live in local, ignored `tasks/`; tracked code and documentation must not reference this local path.
- This delivery does not authorize a rental, deployment, release, tag, issue closure, or purchase. Since v1.6 macOS qualification needs no rental; an optional rental stays Arthur's decision and purchase. Qualification uses isolated fixtures and the exact candidate artifacts.
- Follow the repository's no-source-comments rule, helper size limits, locking rules, and attribution policy. Documentation and UI copy use US English.

### Cross-PRD ownership and sequencing

| Concern | Owner | Integration rule |
|---------|-------|------------------|
| PTY ownership, lifecycle generations, adoption compatibility, terminal stream, manifest persistence, runtime reclamation | This PRD | One implementation and one persistent-core contract; all new callbacks use it. |
| Existing `paneflow serve` worker, activity reducer, runtime catalog, provider installations, remote Controllers | Agent Runtime System PRD | Keep its scope and tracker unchanged. This delivery may adapt existing worker consumers to core identity/revision/uncertainty fields and test worker replacement; it does not redesign provider rules or introduce another process owner. |
| Hook seed receipt | Existing implementation plus US-005/011 hardening | Preserve current schema and event semantics; fix write ordering and delivery recovery only. |
| Overlap with its EP-003/US-009 | This PRD supplies persistent-core guarantees to the implemented topology | Inspect existing compatibility and worker paths before changing them. Integrate through the same authority and serialize shared-file edits; do not schedule a second worker migration or assume the other PRD is certified. |
| Final candidate | US-014 | Freeze the integrated desktop, host, and worker candidate. Worker death/restart/replacement while sessions continue is mandatory; omitting the existing worker is not a valid reduced qualification topology. |

The old PRD's unconditional N/N-1 compatibility expectation must not override a real incompatible wire or snapshot format. Compatibility requires advertised supported contracts, not version arithmetic. An unsupported peer remains intact and is reported as incompatible. This document does not modify or certify the other PRD.

### Prioritization decisions

| MoSCoW | Capabilities | Decision |
|--------|--------------|----------|
| Must Have | A01-A18 corrections, non-launching restoration, passive ended views, reachable detached sessions, lifecycle authority, bounded resources, and complete platform evidence | All 20 stories are release-blocking for this delivery; existing IDs are retained and dependencies determine execution order. |
| Should Have | Human-readable benchmark comparison alongside machine-readable evidence | Included in US-013; no separate dashboard or service. |
| Could Have | Additional architectures or more extensive long-term statistics | Deferred beyond this PRD. |
| Won't Have | Continuous journal, live host-process migration, remote transport, provider feature redesign | Explicitly excluded below. |

## Quality Gates

This is the only command-gate list. It applies to every relevant implementation/review bundle. Batch validation after a coherent change; do not rerun full suites after each edit. Source-inspection-only and document-only work must report that distinction. A passing local gate certifies only the exercised host target.

- `cargo fmt --check` - mandatory before every Rust commit and every Rust push, on the pinned toolchain.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint the exercised native target, including test code.
- `cargo test --workspace --locked` - workspace regression suite, including real-PTY and lifecycle integration coverage.
- `cargo build --release -p paneflow-app -p paneflow-host --locked` - build the candidate desktop and host together for integration and platform qualification.
- `cargo deny check advisories licenses sources` - required when dependencies change.
- `powershell -NoProfile -File scripts/bench-terminal.ps1` on Windows, or `bash scripts/bench-terminal.sh` on Unix - retain existing terminal regression coverage when terminal processing changes. This is not the persistent-path benchmark.
- The persistent-path harness introduced by US-001 and completed by US-013 - execute its documented native invocation and record raw results, exit code, scenario coverage, and threshold decisions. These future commands must be added to the tracked benchmark runbook, not guessed here.

Additional evidence gates:

- Native CI must execute the relevant checks for all four shipping triples. Record each required job result at the candidate SHA; skipped, cancelled, failed-first-attempt/retried, and not-run jobs are explicit outcomes, not silently green evidence. Existing Windows tests do run; stale comments or older PRDs saying they are skipped are not authority.
- Installed-package exercises use desktop and host from the same candidate artifact set, verify engine identity and bundled ConPTY, and include actual helper placement. Reuse existing MSI, Linux package, and macOS bundle scripts.
- UI changes require a native interactive pass and screenshots or a short recording. Browser automation is not needed for this native feature.
- Performance claims require the same workload and hardware before/after. Use Linux heaptrack diffs and `cargo flamegraph` for the repository's memory/CPU claim policy, with native Windows/macOS traces and counters as additional platform evidence. Allocator counters alone do not cover libghostty/native allocations.
- `/implement-epic` stops at `IN_REVIEW`. `/review-epic` alone certifies `DONE`. A missing manual or hardware check leaves the corresponding qualification story uncertified, even when scripts and CI are ready.

## Epics & User Stories

### EP-001: Baseline Evidence and Lifecycle Safety

Establish the persistent-path baseline and close process-loss, generation, launch, and accepted-event correctness gaps before the resource refactor.

**Definition of Done:** US-001 through US-005 are certified against real entry points; A01, A07-A14, A17, and A18 have regression coverage for their owned paths; restoration performs no implicit launch; stop uncertainty reaches the desktop; no controlled fixture process escapes ownership; all implementation paths exist on the shipping OS targets. Final-record opening and fallback discovery complete in US-009/010.

#### US-001: Validate waiting and launch ownership with a persistent-path baseline
**Description:** As a maintainer, I want native evidence for exit waiting, late launches, and current resource behavior so that the refactor has validated primitives and a reproducible comparison.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] A reusable deterministic fixture executable exercises idle, echo, output flood, delayed exit, blocked stdin, descendants, and split escape/UTF-8 sequences without external agents, network calls, or platform-specific shell syntax.
- [x] Baseline records identify commit, local diff fingerprint, OS/build, architecture, toolchain, engine/ConPTY identity, CPU/RAM, build profile, fixture seed, and invocation. No benchmark is represented as measured until it runs.
- [x] The baseline runner measures 1, 10, and 50 sessions through the real host IPC and attachment path using the workload definitions below; each unavailable native environment is explicitly pending.
- [x] Baselines identify host, existing worker, desktop mirrors/followers, viewport scan, and cancellation scan separately. The 500 ms viewport scan, 100 ms cancellation scan, and desktop runtime deadlines are measured alongside the 15/20 ms output/exit polls; timer presence alone is not a CPU measurement.
- [x] H01 is validated with a native blocking waiter and independent termination handle on Windows, Linux, and macOS; the evidence distinguishes child exit from output EOF and does not hold the session or registry lock while waiting.
- [x] H02 is validated by delaying launch beyond the caller deadline and dropping the requesting connection; the recorded design retains ownership of a late child and proves its cleanup path. The old behavior may fail this probe and is retained as baseline evidence.
- [x] Baseline fault evidence is limited to the late-launch probe and a paused follower, each with a bounded watchdog. Full queue, failed-wait, and denied-write injection belongs to the owning correction stories and the integrated US-013 harness.
- [x] The outputs document which initial budgets are unmeasured or missed. No expected-failure marker or fabricated zero converts the current implementation into a passing candidate.

**Execution checkpoints:** (1) add the minimal reusable fixture and record its invocation; (2) capture the baseline through existing IPC without building a new general benchmark framework; (3) validate H01/H02 with focused probes and record native CI evidence or a concrete blocker. Each checkpoint leaves reviewable artifacts. Platform-wide profiling, exhaustive faults, and the endurance harness belong to US-013 and the qualification epics.

#### US-002: Preserve sessions during restoration, adoption, and upgrades
**Description:** As a developer, I want reopening or updating the desktop to reattach retained work without silently stopping or replacing its processes.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Saved-layout restoration and attachment Retry never issue create/restart, including missing, exited, failed, lost, foreign-host, and incompatible records. They preserve the saved reference and show the typed state; `attach_restored` no longer uses a launch-capable `Resume` path.
- [x] New-terminal creation, non-launching reattachment, and explicit restart have distinct call paths. Restart submits the observed generation and cannot race into replacing a newer one; a missing record requires a distinct new-terminal action with a fresh session ID. Existing explicitly named resume/restart commands remain deliberate launch actions and are documented as such.
- [x] Application build identity is diagnostic; a build-only difference with compatible control, stream, and snapshot contracts does not cause shutdown or child replacement.
- [x] A genuinely incompatible host remains running. The desktop explains the mismatch, preserves known session identities, and offers retry or an explicit stop-and-restart flow; no bootstrap path issues an implicit forced shutdown.
- [x] Safe control operations remain available only when a compatible control contract is established; incompatible native snapshots are never decoded merely because the build mismatch was relaxed.
- [x] Replacement artifacts and required helpers are checked before any explicitly requested stop. Missing executables, access errors, or failed install staging leave live sessions intact.
- [x] The installed-app update flow defers a replacement that cannot preserve an in-use host binary, including Windows file-lock behavior; it does not terminate the host to make installation succeed.
- [x] Both ordinary desktop restart and the Windows self-update relay/MSI route obey preservation preflight. Waiting for the desktop PID alone is not proof that installation preserves the host; failed staging or an in-use core leaves the running sessions intact and the update deferred with an actionable reason.
- [x] Compatibility fixtures cover old/new same-contract builds, unsupported protocol, incompatible engine/snapshot, wrong home, missing replacement, and rollback. No fixture relies only on a host with zero sessions.
- [x] Architecture and user-facing upgrade descriptions match the implemented preservation guarantee, without promising seamless attachment to an incompatible engine.

**Execution checkpoints:** (1) separate restore/reattach from explicit create/restart and cover each saved-record state; (2) replace build-only retirement with contract-based adoption; (3) carry preservation through restart/update staging and package fixtures. Reuse current host bootstrap, attachment states, and update mechanisms; live core handoff is excluded.

#### US-003: Serialize generation transitions and recover failed restarts
**Description:** As a developer, I want each session to have one authoritative generation transition so that concurrent stop, restart, and callback delivery cannot replace or corrupt a live process.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] All lifecycle-changing requests and asynchronous completions carry the expected session, host instance, generation, and operation identity. Old completions cannot modify a newer generation or recreate a removed record.
- [x] Viewport/foreground observations, cancellation/submission signals, hook receipts, and worker-facing updates carry the identity captured with their source data through application and persistence. A delayed generation-N scan or drained signal cannot be relabeled as N+1; generation-scoped trackers reset on transition. Barrier tests cover restart/remove between capture and commit.
- [x] Two concurrent restarts can commit at most one new generation and one child. A losing request returns the current state or a conflict without spawning another child.
- [x] A stop result from generation N cannot write `Exited` into generation N+1. Deterministic barriers reproduce the old observer/stop interleavings and verify the new invariant.
- [x] Restart state is prepared and persisted through one ordered transition. A write failure before launch restores a usable prior state or a retryable failed transition; it cannot strand a process-free `Starting` record.
- [x] Remove, shutdown admission, and restart cannot race into accepting a new child after shutdown has committed or after the owning record was deleted.
- [x] Lifecycle authority does not hold the global registry lock across disk I/O, PTY operations, snapshot encoding, child wait, or user callbacks; independent sessions continue under an injected stalled operation.
- [x] Explicit restart preserves stable session/workspace identity, increments generation once, and starts only the recorded shell, with no input or command replay.

#### US-004: Keep ownership through startup, stop, and observation failures
**Description:** As a developer, I want timeouts and process-observation errors to remain recoverable without abandoned children or false success.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] A launch operation remains registered until spawn failure or child ownership transfer is conclusive, including requester disconnect and startup timeout. A second launch for that session is refused while resolution is pending.
- [x] A late child after cancellation is never published as a successful new runtime; the owner requests termination, drains or closes its I/O safely, and confirms the child outcome before releasing ownership.
- [x] The synchronous PTY spawn is not advertised as forcibly cancellable. If it has not returned, the operation is reported as pending/unverified with diagnostics and bounded admission for additional launches.
- [x] Errors from wait, process identity lookup, or termination never become a fabricated `Exited(-1)` or successful stop. Unverified ownership remains visible and prevents destructive cleanup or an unsafe host shutdown.
- [x] Every signal/reap path verifies the retained process identity or OS handle. Recycled PIDs and foreign-home records are never signaled; descendant cleanup records its own uncertainty separately from the root exit code.
- [x] Startup failures after child creation, reader/writer-thread creation failure, and runtime panic preserve a recovery owner rather than dropping a still-live child handle without supervision.
- [x] Stop and shutdown enforce the shared deadlines below, remain responsive across independent sessions, and return unresolved session/operation IDs when completion cannot be established.
- [x] Native descendant-cleanup failures and retained recovery owners remain part of the stop result after root exit. The Windows tree-termination result is consumed, and Unix group termination is not treated as proof that escaped or unresolved owned descendants exited.
- [x] Stop-everything-and-quit/restart waits for confirmed session outcomes and the host shutdown acknowledgement. Any unresolved session, failed shutdown RPC, or deadline leaves the desktop open with affected identities and Retry, Keep running and quit, and Cancel choices; only a new explicit keep-running choice may quit without claiming that all processes stopped. Already completed stops are not represented as reversible.
- [x] A final persistence/shutdown durability failure also keeps the desktop open without delaying confirmed exit or runtime retirement. When every process owner is confirmed resolved, show that result separately and label the explicit desktop-only override `Quit with unsaved final state`, alongside Retry and Cancel. It reports unconfirmed durability, does not force host termination, and leaves any surviving host's bounded pending revision owned for retry.
- [x] Missing or stale worker activity remains unknown unless a fresh authoritative observation establishes an agent state or its absence. Closing with unknown activity keeps the existing confirmation path and cannot infer a safe automatic stop from a filtered-out stale row.
- [x] Native tests cover Windows bundled ConPTY and Job Object detachment plus Unix process groups; closing a pseudoconsole or returning from `kill` is never the sole proof of successful process termination.

**Execution checkpoints:** (1) introduce the registered launch/recovery owner and late-child handoff; (2) route stop, descendant failure, and observation failure through that owner; (3) propagate the aggregate result and unknown activity into existing close/quit dialogs; (4) verify failure injection and native adapters. Reuse existing process-identity and native PTY adapters; this story does not introduce a general process supervisor, replace the runtime event loop, or implement the final-output drain, which belongs to US-007. Interactive OS qualification follows later.

#### US-005: Make accepted hook seed persistence ordered and recoverable
**Description:** As a developer, I want accepted agent events to survive concurrent delivery and storage failures without corrupting the session's persistent state.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Concurrent writes for one session use collision-free temporary files and a per-session accepted revision. Older writes cannot replace a newer-generation or newer-revision seed.
- [x] The accepted-event contract is explicit: state commit, recoverable persistence, acknowledgement, and subscriber notification have a defined order. Storage failure cannot silently acknowledge a fully durable event.
- [x] A client retry after an ambiguous acknowledgement cannot produce a duplicate completion notification; use existing event identity where available or a bounded receipt identity in the internal contract.
- [x] Delayed seed writes after session removal cannot recreate its directory. Generation checks occur at commit time as well as request parsing time.
- [x] Cancellation/submission marker writes use the same generation/deletion barrier as hook seeds. Restart or removal after signal capture cannot write a new-generation marker from old input or recreate the removed session directory.
- [x] Given injected write, rename, and notification failures, the requester receives a typed result and subscribers can reconcile from a revisioned snapshot without a permanently lost accepted state.
- [x] Parallel hook delivery, stale generations, and restart while a write is queued have deterministic regressions. Existing provider integration formats and ownership boundaries remain unchanged unless an additive revision field is required.

---

### EP-002: Event-Driven Runtime and Resource Retirement

Replace periodic output/exit interrogation with explicit events and separate live resources from retained session identity. Preserve atomic snapshot/offset semantics and terminal effects.

**Definition of Done:** US-006 through US-010 are certified; idle output/exit polling is removed; completed host sessions own zero runtime workers after final drain; checkpoints are released; ended panes remain usable; idle input and bounded reconnection work; every unattached retained live/unverified session has a visible desktop access path without requiring its original workspace.

#### US-006: Notify followers when output or lifecycle changes
**Description:** As a developer, I want idle sessions to sleep until something changes so that keeping many terminals open does not cause continuous polling.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Followers register against a sequence/offset predicate and wait for publication, exit, cancellation, or a real deadline. The 15 ms `host.output` polling loop is removed.
- [ ] Register/check/wait ordering prevents lost wakeups when output arrives during subscription or between reading the tail and sleeping; notification coalescing never changes byte offsets.
- [ ] A single append updates the authoritative tail and notifies interested followers; slow subscribers do not hold the runtime lock or block PTY parsing for other subscribers.
- [ ] Tail eviction produces an explicit resynchronization requirement. Checkpoint and resume offset remain one atomic terminal-state cut, including partial UTF-8 and control sequences.
- [ ] Keepalives and disconnection detection use deadlines, not rapid empty reads. Cancellation wakes blocked subscriptions, and their resources are reclaimed after peer closure.
- [ ] Flood, empty output, exit without output, multiple followers, paused subscriber, and notification-before-wait cases execute against the real IPC server with bounded completion.
- [ ] Existing terminal side-effect rules remain: only the host responds to terminal queries; checkpoint replay does not repeat clipboard writes, bells, or desktop notifications.

#### US-007: Deliver child completion and drain output without idle polling
**Description:** As a developer, I want process completion to wake its runtime exactly when needed while preserving the final terminal output.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-004, US-006

**Acceptance Criteria:**
- [ ] Each owned child uses the H01-validated blocking/native wait path and sends a generation-tagged completion. The 20 ms exit-check tick is removed from the normal runtime loop.
- [ ] Exit, stop, and cancellation have a reliable bounded control path that output saturation cannot starve or discard; terminal mutation remains serialized in the session owner.
- [ ] Child exit and PTY EOF are separate events. Buffered final output is consumed and a final offset is published before normal stream completion and retirement.
- [ ] A descendant retaining the PTY cannot keep an ended terminal runtime alive indefinitely: after the final-drain budget, report incomplete output and cancel owned I/O using the native adapter. Transfer unresolved verified process handles/identities to an independently counted recovery-owner record; refuse remove/restart and successful host shutdown until that ownership resolves. Held-open-descendant tests exercise those requests after the drain deadline.
- [ ] A child-wait error transitions to unverified observation, preserves identity, and schedules bounded reconciliation; it never fabricates an exit code or discards the process owner.
- [ ] Shutdown cannot join a blocked PTY closer on the GUI thread or while holding a registry lock. Windows close/drain and Unix descriptor closure are exercised natively.
- [ ] Tests cover output immediately before exit, blocked input, held-open descendant descriptors, full queues, runtime panic, and stop concurrent with natural exit. Successful cases leave no fixture descendant or host worker behind.

#### US-008: Consume attachment checkpoints and release obsolete buffers
**Description:** As a developer, I want reopening many sessions to retain only the terminal state needed for use, not duplicate serialized snapshots.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Attachment identity is separated from its one-shot checkpoint payload. Host control and followers retain only endpoint, negotiated identity, session, generation, and offset data.
- [ ] The initial checkpoint is moved into restoration and released after decoding. Ownership instrumentation reports zero retained serialized checkpoint bytes after a successful restore and quiescence.
- [ ] Reattachment permits one pending replacement checkpoint per mirror; obsolete generations or reconnect attempts release their payload and cannot restore stale state.
- [ ] Snapshot failure preserves the last valid rendered content with a typed connection state. A rejected/oversized checkpoint is released without stopping the host session.
- [ ] Redundant encoding allocations are removed where the existing native encoder API permits; all remaining transient allocations are included in the checkpoint admission budget.
- [ ] Tests exercise repeated attach/detach with filled scrollback, resize during restore, and decode failure, and retain scrollback, modes, selection behavior, and continuation correctness.

#### US-009: Retire completed runtimes and retain bounded final records
**Description:** As a developer, I want ended sessions to remain identifiable and inspectable while their live threads, PTY resources, and terminal allocations are released.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-007, US-008

**Acceptance Criteria:**
- [ ] A confirmed ended session becomes a completed record after final-output handling. Within NFR-04 it retains no host terminal engine, live tail, waiter, writer, reader, or session runtime thread.
- [ ] Natural child exit retains the attached final view for both zero/nonzero exit codes and with/without prior keyboard input. Replace `should_close_on_exit`'s current discriminator and its tests; natural exit never routes through stop/remove. Explicit user close/stop may still close the selected surface under the existing confirmed close choices.
- [ ] Passive final views preserve displayed content and user-driven selection/search in main panes, detached terminal windows, and diff-dock terminal tabs. No host worker or periodic host request is kept solely for this display; cold-text eviction cannot blank an already attached final view.
- [ ] The cold record includes exit outcome, final offset, generation, workspace identity, last title/cwd, and explicit final-output completeness. It can reference at most 512 KiB of final readable text, captured outside unrelated-session locks.
- [ ] `session.text` for a completed record returns the bounded retained text or an explicit unavailable/evicted result. It does not recreate a PTY, claim live attachment, or silently return fabricated empty output.
- [ ] Opening a completed record without an existing mirror displays its available bounded text read-only, exit outcome, and completeness, or explicit text unavailability. It does not promise the original full scrollback, native terminal modes, or a new live attachment; restart remains a separate explicit action. This output-view path is available to layout restoration and US-010 sidebar opening.
- [ ] Cold text is optional storage, limited to 64 MiB per home with oldest-ended-first eviction; evicting text does not delete the session identity. Disk-full or write failure records unavailable history and does not prevent confirmed live-resource retirement.
- [ ] Denied or stalled final-manifest writes also permit confirmed exit publication and terminal-resource retirement. Keep the latest bounded pending final revision and a visible durability error; never claim the revision is durable, and retry through the persistence service after recovery.
- [ ] Preserve existing manifest retention of 24 hours for normal completion/failure and 30 days for lost records. A maintenance deadline checks at most once per minute, including when no new sessions are created; running and unresolved-owned sessions are never expired.
- [ ] Repeated session churn proves the reclamation budgets; aged-record tests use an injected clock. Late callbacks and writes cannot resurrect removed records or obsolete cold text.
- [ ] Removing an eligible completed session through the existing removal flow releases its cold cache reference without affecting live sessions. Routine application-managed cache eviction applies only to the bounded cold cache; no new cache-management UI is required.

**Execution checkpoints:** (1) produce cold records and retire host resources; (2) retain existing mirrors passively across natural exit on every terminal surface; (3) expose bounded final-text viewing without launch; (4) verify churn, retention, and failed durability. Final text is a cache, not a continuous journal or a promise of exact cold restoration.

#### US-010: Preserve input and session access across reconnection
**Description:** As a developer, I want to return to retained sessions and type after inactivity without losing access, losing routine input, or duplicating a command.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-002, US-006, US-008, US-009

**Acceptance Criteria:**
- [ ] Valid established control connections do not expire solely because the user has not typed for 60 seconds. Handshake and partial-frame deadlines remain bounded, and disconnect/shutdown can cancel an idle read.
- [ ] Real echo fixtures receive the first key exactly once after 61 seconds, 5 minutes, and 30 minutes without input or resize while output remains attached.
- [ ] Control writes and RPC waits are isolated from the mirror's parsing/publication loop. A stalled control connection does not freeze the last valid view or block independent output processing and reconnect status.
- [ ] Only input proven unsent may be retried automatically. Partial writes or missing acknowledgements yield an explicit delivery-unknown result and are not blindly resent; queued input bytes remain within the existing cap.
- [ ] Reconnect attempts have bounded backoff and cancellation, validate host instance and generation, resume from an available output offset, and request a new checkpoint only when required by eviction or invalid state.
- [ ] A replaced host, restarted generation, incompatible snapshot, and refusal due to capacity produce distinct terminal states and release obsolete connections and buffers.
- [ ] A paused reader, control-peer crash during a paste, and repeated disconnect/reconnect have byte-count regressions that assert no accidental duplicate command and no unbounded accumulation of connection threads.
- [ ] The existing main workspace sidebar renders each unattached same-home live/unverified session exactly once. Preserve workspace grouping when applicable and add one compact `Other sessions` group for otherwise unreachable records, visible even with zero open workspaces. Reuse hosted-session rows and actions; the provider-transcript Agent sessions sidebar is not the recovery surface.
- [ ] Retained ended records without an eligible workspace remain in that fallback group under the existing bounded inactive-record display policy; ordinary opening uses the US-009 final-text view. A kept-running fallback session that exits with no workspace open remains eligible for that list, without restarting or being removed solely because it ended.
- [ ] Opening a live fallback row attaches the same session/generation in the active workspace without changing the host's recorded workspace/cwd. With no open workspace, create an ordinary empty display workspace using the existing constructor and a valid local root, then attach directly with no intermediary shell. Prefer the recorded cwd when accessible, otherwise the user's existing home directory; if neither is usable, retain the row and report the attachment prerequisite.
- [ ] An ordinary click on an ended row opens the US-009 final-text view, and a lost/unknown row exposes its state and a non-launching retry. Explicit Restart remains separate and subject to ownership checks. Discovery and row opening never increment a generation or recreate a missing record.
- [ ] Closed original workspace, deleted/moved cwd, cwd outside the original project, recents eviction, workspace-capacity refusal, duplicate attach, and host-list failure have regressions. Missing paths and capacity errors leave the row accessible; refresh failure preserves known rows as stale rather than reporting that their processes ended.

**Execution checkpoints:** (1) isolate idle-safe control and ambiguous input delivery; (2) verify bounded follower recovery; (3) reuse hosted-session rows for fallback discovery and non-launching attachment/final-output opening; (4) exercise the complete keep-running/return path. Do not add a separate session manager, redesign the sidebar, or migrate host workspace ownership.

---

### EP-003: Persistence Budgets and Integrated Qualification Readiness

Move persistence off the data path, enforce explicit resource budgets, and produce the tools and frozen candidate required for all three platform epics.

**Definition of Done:** US-011 through US-014 are certified; all A01-A18 mappings are resolved; persistent-path regression and measurement tools are available on every shipping target; an integrated desktop/host/worker candidate and complete platform runbook are ready. This is not final platform acceptance.

#### US-011: Persist revisions without blocking PTY processing
**Description:** As a developer, I want title, directory, and lifecycle persistence to stay ordered without disk latency stalling my terminals.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-003, US-005, US-009

**Acceptance Criteria:**
- [ ] A bounded persistence service receives immutable session/generation/revision records. The runtime and GUI threads do not perform manifest writes or sleep on rename retries.
- [ ] Title and cwd updates coalesce to the latest revision with the NFR-07 flush bound; a repeated identical value does not write a new manifest. A metadata storm in one session cannot monopolize the queue.
- [ ] Creation/restart commit, accepted durable hook receipt, and removal use explicit persistence completion barriers before reporting durable success. Final lifecycle writes expose a separate durability acknowledgement: confirmed process exit and resource retirement never wait for disk success. Queue exhaustion returns backpressure or a typed durability failure; critical final state retains a bounded retryable revision rather than being silently discarded.
- [ ] Writes cannot regress a session revision, resurrect a deleted generation, or race on a shared temporary name. The US-005 accepted-event semantics remain valid after the writer refactor.
- [ ] The writer contract includes viewport-derived metadata and cancellation/submission markers alongside manifests and hook seeds. No producer bypasses generation/revision/deletion barriers; provider-specific interpretation stays in the existing Agent Runtime System ownership scope.
- [ ] Critical durable acknowledgement follows the platform-specific flush/atomic-replacement contract. The runbook distinguishes process-crash consistency from power-loss durability and records any filesystem limitation.
- [ ] A stalled disk does not block output/echo in unrelated sessions. The queue remains within its byte cap, and callers receive bounded errors when critical persistence cannot finish.
- [ ] On clean host shutdown the writer drains within its budget or reports pending failures; automatic shutdown cannot discard a live or unverified process to satisfy a persistence deadline.

#### US-012: Enforce memory admission and slow-consumer isolation
**Description:** As a maintainer, I want explicit budgets for the complete persistent pipeline so that high output, large history, or slow clients cannot grow memory without bounds.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-006, US-008, US-009, US-010, US-011

**Acceptance Criteria:**
- [ ] Tail, PTY mailbox, input queue, subscriber output, checkpoint staging, and persistence queue have enforced byte budgets, not only message counts. Actual allocated capacity is reported separately from logical length.
- [ ] The tail allocation respects its physical budget without grow-then-drain doubling. GUI output buffers preserve the existing pooled path and are not replaced with an unbounded channel.
- [ ] At most two full checkpoints are being staged across the host at once; admission reserves measured transient space before encoding. Further requests wait within their deadline or receive a retryable capacity error without allocating a checkpoint first.
- [ ] A slow client cannot stall host PTY parsing through its transport write. Buffer exhaustion causes a documented disconnect/resynchronization path; healthy clients and stop/control operations retain progress.
- [ ] Oversized frames, terminal dimensions, snapshot declarations, and input use checked size arithmetic and are rejected before exceeding existing engine or protocol limits. Native snapshot decoding retains its strict compatibility and size checks.
- [ ] Resource admission supports the specified 50-session workload and reserves control capacity for inspection/stop; exhaustion returns a bounded diagnostic rather than spinning or spawning unlimited threads.
- [ ] Idle queue capacity is released where it is not required for the retained live history. No budget optimization discards canonical live terminal state or silently reduces configured history.

#### US-013: Gate the persistent path with cross-platform evidence
**Description:** As a maintainer, I want one native regression and performance harness so that every platform evaluates the actual persistent system using the same scenarios.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-004, US-005, US-007, US-009, US-010, US-011, US-012

**Acceptance Criteria:**
- [ ] The US-001 harness covers all workloads W01-W08 and emits versioned JSON plus a human-readable before/after comparison with raw repetitions, thresholds, and failure reasons.
- [ ] The measured process set includes the host, existing worker, desktop mirrors/followers, and their maintenance producers. Fixture child CPU/memory is recorded separately, preventing agent workload from being attributed to Paneflow. Wait-reason attribution distinguishes necessary observation/lease/cancellation deadlines from removable output/exit polling.
- [ ] Real desktop entry-point tests cover saved-layout restore after host loss, missing/ended-row opening, natural exit on all terminal surfaces, stale activity at close, failed stop-and-quit/restart, and discovery without an original workspace. A zero-launch assertion observes fixture process creation as well as IPC requests.
- [ ] Worker kill/restart and build replacement while terminals run preserve child identity, generation, scrollback, and the next input. The worker rebuilds its projection from accepted state without issuing PTY restart or duplicating process ownership; compare baseline/candidate behavior without certifying the separate reducer PRD.
- [ ] Native samplers record CPU time, private/physical memory as available, threads, handles/FDs, queue capacities, native terminal/checkpoint ownership, and attach/input latency. Missing metrics are unavailable, never zero.
- [ ] Existing CI runs the new functional cases on all four shipping triples. Change filters include the host, IPC, terminal mirror, process adapter, fixtures, scripts, and relevant packaging paths.
- [ ] Performance thresholds run on stable qualification machines; shared CI runners enforce deterministic correctness/resource counts and archive timing without pretending to provide noise-free performance comparisons.
- [ ] A seeded known failure in the harness proves nonzero exit status and artifact retention. Retries preserve the first failure and cannot turn flaky correctness into an unqualified pass.
- [ ] The fixture manager cleans up only processes it owns using recorded identities; crash/cancellation of the test controller does not leave background hosts on the operator's real home.
- [ ] No custom global allocator is added to a binary that already has one. Native libghostty allocations are covered through OS/native profiling rather than Rust allocator counters alone.

#### US-014: Freeze the integrated candidate and platform runbooks
**Description:** As a maintainer, I want one complete candidate and explicit platform evidence requirements so that qualification exercises the product that will actually be delivered.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-003, US-004, US-005, US-006, US-007, US-008, US-009, US-010, US-011, US-012, US-013

**Acceptance Criteria:**
- [ ] One candidate SHA and artifact manifest identify desktop, host, helper binaries, engine archives, and Windows ConPTY. All implementation stories have completed review before OS qualification begins.
- [ ] The existing Agent Runtime System integration is recorded without editing its tracker. Desktop, host, worker, viewport/cancellation producers, and packaged update routes use the same identity/uncertainty contract. Worker death/restart/replacement and consumer compatibility cases are mandatory candidate evidence.
- [ ] Tracked architecture, benchmark, and user-facing session documentation describe lifecycle states, compatibility, bounded final text, deadlines, and recovery accurately, without linking to local `tasks/`.
- [ ] Package preparation verifies host/helper placement, execute permissions, process detachment, and upgrade staging on the existing packaging routes; a missing helper fails preflight before a retained host is stopped.
- [ ] The platform runbook includes the matrix, workload inputs, thresholds, evidence format, known environment requirements, and native UI exercises below, with no dependence on an unstated developer machine setup.
- [ ] A coverage ledger maps every A finding, FR, NFR, and unhappy path to automated cases and/or platform evidence. Missing coverage is a release blocker rather than an inferred pass.
- [ ] A candidate change during qualification records its affected boundaries and invalidates relevant earlier evidence. Shared lifecycle/IPC/engine changes require renewed core evidence on all OSes.

---

### EP-004: Windows Qualification

Exercise the fully implemented system on native Windows 11 x64 with the actual bundled ConPTY. Windows 10 x64 is assumed equivalent and not exercised: no Windows 10 machine is available, and the report records this assumption. Scope amended by Arthur on 2026-09-22: the qualification uses the native automated suites, the harness workloads, and a short manual smoke; installer and update behavior is covered by the existing `release.yml` MSI smoke at the next release.

**Definition of Done:** US-015 and US-016 are reviewed with native Windows 11 evidence at the integrated candidate; every ownership or resource failure found is dispositioned through a fix and rerun, not a waived green summary.

#### US-015: Qualify Windows lifecycle and installed-app behavior
**Description:** As a Windows user, I want the application to preserve, stop, and recover sessions with native process and console semantics.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Native Windows 11 x64 runs `cargo test --workspace --locked` green on the candidate, with the bundled ConPTY identity recorded; Windows 10 x64 is recorded as assumed equivalent, not tested.
- [ ] Descendants, blocked input, delayed EOF, denied process access, recycled-identity simulation, host crash, breakaway denial, named-pipe cancellation, and paused clients are covered by the native automated suites without touching unrelated processes, and handles are reclaimed.
- [ ] Desktop forced termination with live sessions reattaches every pane without a create or restart, through the harness `-WithDesktop` restoration.
- [ ] Arthur runs a short manual smoke of the release desktop: close with Keep running then reopen, kill the host then reopen (lost state with Retry, no shell launched), and Stop everything with a session that ignores termination (desktop stays open). The result is recorded in the report.
- [ ] Installer and self-update relay behavior is not qualified here; the `release.yml` MSI install and update-relay smoke of the next release covers it.
- [ ] The report records Windows edition/build, architecture, executable paths and versions, the ConPTY identity, and each case result.

#### US-016: Qualify Windows CPU, memory, and endurance
**Description:** As a Windows user, I want the persistent system to meet its budgets under idle, output, and repeated session churn.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [ ] The full harness protocol runs on Windows 11 in the host+worker and native-desktop topologies with native CPU, resident-memory, thread, handle, and queue evidence; each threshold decision is pass, fail, or unavailable with its reason.
- [ ] Idle CPU at 50 sessions is taken from the harness short window.
- [ ] A W08 endurance rehearsal passes every decision (idle first input echoed once, retained identities unchanged, no orphan, no ownership-counter growth); the 8-hour run is optional.
- [ ] A threshold miss or flaky lifecycle failure causes a recorded finding and a rerun; it is not hidden by baseline replacement or repeated retries.
- [ ] Evidence is archived with the candidate SHA and Windows environment, and residual OS/architecture limits are stated explicitly.

---

### EP-005: Linux Qualification

Verify the complete host and desktop on Linux x64 and ARM64, including distribution packaging and both supported desktop protocols.

Scope amended by Arthur on 2026-09-22. Native Linux ARM64 hardware is not available: ARM64 evidence is the native `ubuntu-22.04-arm` CI job (clippy, workspace tests, release build) at the candidate SHA, with no ARM64 render smoke, performance matrix, or endurance run. Fedora x64 on the qualification machine is the only full native cell: GNOME Wayland, and X11 through XWayland, recorded as XWayland and not as a native Xorg session. Ubuntu, Debian, Arch, and openSUSE run host lifecycle checks from the portable tarball in containers and are host-only evidence; their Wayland and X11 application smokes are recorded `unavailable`. The 8-hour W08 run is replaced by a rehearsal, and the interactive D-cell passes by a short manual smoke that Arthur runs on Fedora.

**Definition of Done:** US-017 and US-018 are reviewed; required native target, distribution, and desktop coverage is explicit; core lifecycle and performance budgets pass without extrapolating from Windows or from a headless-only run.

#### US-017: Qualify Linux lifecycle, distribution, and desktop paths
**Description:** As a Linux user, I want persistent sessions to behave consistently across supported distributions, Wayland, and X11.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Native Fedora x64 runs `cargo test --workspace --locked` green at the candidate SHA, and the native `ubuntu-22.04-arm` CI job runs clippy, workspace tests, and the release build green at the same SHA; no emulator run is presented as ARM64 evidence.
- [ ] Fedora, Ubuntu, Debian, Arch, and openSUSE execute host lifecycle checks from the candidate's portable tarball, in containers for every distribution other than the qualification machine's, with exact distribution, kernel, libc, and installation route recorded. Container results are host-only evidence.
- [ ] Fedora receives a native application run in both a GNOME Wayland session and an X11 session through XWayland, each covering attach, output, resize, and reopen through the harness `--with-desktop` restoration. The Wayland and X11 smokes of the other distributions and any native Xorg session are recorded `unavailable`.
- [ ] Arthur runs a short manual smoke of the release desktop on Fedora: close with Keep running then reopen, and kill the host then reopen (lost state with a non-launching Retry or an explicitly labeled Restart, no shell launched). The unresolved stop-all path (desktop stays open) is covered by `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures` and `a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving`, because a non-root user cannot start a pane process that survives the stop on Linux. The result is recorded in the report; the remaining D-cells stay as the reference checklist.
- [ ] Unix ownership checks cover socket permissions, owner locks, process groups, detached host lifetime, held-open PTY descriptors, interrupted I/O, and host/desktop termination without assuming that EOF proves all descendants exited.
- [ ] Unsupported/missing package dependencies, an occupied endpoint, corrupt state, and a denied manifest directory preserve unrelated/live processes and return recoverable errors.
- [ ] Every cell outside the amended scope (ARM64 render and attach, per-distribution Wayland and X11, native Xorg) remains visibly `unavailable` in the report and is never counted as passed.

#### US-018: Qualify Linux CPU, memory, and endurance
**Description:** As a Linux user, I want measured resource behavior and reclamation on both shipping Linux architectures.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [ ] W01-W08 run natively on the Fedora x64 qualification machine with its own baseline; ARM64 performance is recorded `unavailable` and no cross-architecture ratio is presented as an optimization result.
- [ ] Heaptrack before/after captures include native terminal allocations; CPU flamegraphs cover idle and output workloads. RSS/PSS, private memory where available, threads, and FDs are recorded alongside ownership counters.
- [ ] Wayland and X11 (XWayland) desktop runs distinguish GPU/rendering work from host/IPC work. A software-rendered CI environment is identified and not used as the hardware performance reference.
- [ ] A W08 endurance rehearsal on Fedora x64 passes every decision (idle first input echoed once, retained identities unchanged, no orphan, no ownership-counter growth); the 8-hour run is optional and the ARM64 2-hour run is not required.
- [ ] Threshold misses, accumulating FDs, unconfirmed descendants, or unavailable native metrics remain explicit blockers or missing evidence, and the report identifies the affected case and next action.

---

### EP-006: macOS Qualification on the Hosted Apple Silicon Runner

Validate the completed Apple Silicon application, host, and resource budgets from the packaged candidate on the hosted `macos-14` runner, with every cell that needs physical hardware or a person at the Mac recorded explicitly.

Scope amended by Arthur on 2026-09-23, matching EP-004 and EP-005. v1.5: the W08 evidence is an endurance rehearsal that passes every decision, and the 8-hour run is optional. v1.6: no rental is required. The macOS evidence is the CI gates and render smoke at the candidate SHA, plus one run of the package workflow that builds the qualification package, preflights it, and runs the full host, worker, and desktop protocol, the W08 endurance rehearsal, and paired baseline/candidate host profiles on the same hosted VM. Its verdicts are the macOS decisions of record on a shared virtual machine. Dedicated-hardware performance, an interactive manual smoke on a physical Mac, local-display latency, and the 8-hour run are recorded `unavailable`. The rental runbook stays as an optional route that can add those cells later.

**Definition of Done:** US-019 and US-020 are reviewed; the candidate has macOS ARM64 CI gate, render, lifecycle, and resource evidence from the hosted runner at the candidate SHA; the `unavailable` cells are explicit in the report; the raw evidence is committed with it. No Intel macOS or physical-hardware qualification is implied.

#### US-019: Prepare the macOS qualification package
**Description:** As the operator, I want the build, baseline, scenarios, and on-machine checks packaged and rehearsed, so macOS qualification runs the exact candidate artifacts without compiling on the measuring machine.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] macOS ARM64 CI has actually executed candidate build, lint, tests, and available render smoke, and artifacts include matching desktop/host/helpers, engine identity, fixture, sampler, and checksums.
- [ ] The pre-refactor baseline and candidate hosts run from the package in separate isolated homes without rebuilding; the profiling builds keep line tables and packed debug symbols.
- [ ] A launchable on-machine preflight checks architecture, OS, graphical session, the Metal report, disk, package integrity, quarantine, signature, competing processes, socket path, the packaged host, the packaged desktop render, and evidence export, and writes a ledger.
- [ ] The package workflow runs the preflight, the full protocol, the W08 endurance rehearsal, and the paired host profiles on the hosted runner within one job's 6-hour cap, and fails when any of them fails.
- [ ] Use existing signed artifacts when available. If ad-hoc signing is used for isolated runtime qualification, record that Gatekeeper/notarized distribution is not thereby certified; no production signing secret enters the package or the job.
- [ ] The optional rental route (quote check, access preflight, schedule, teardown) stays documented in the runbook. Buying it remains Arthur's action, and no rental result is assumed.

#### US-020: Execute and report macOS qualification
**Description:** As a macOS user, I want the Apple Silicon application to pass the same persistence and resource guarantees as Windows and Linux.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-019

**Acceptance Criteria:**
- [ ] The macOS CI legs (fmt, clippy, workspace tests, release build, render smoke) are green at the candidate SHA, or at a later SHA whose changes the candidate change rules classify as invalidating nothing.
- [ ] One package workflow run with the full protocol at that SHA records the runner hardware, OS/build, package and candidate identities, and a preflight ledger in which M07, M12, M13, and M14 pass; the M04 Metal report and the signature status are recorded as observed.
- [ ] W01-W07 run from the package with the worker and the desktop on the hosted runner and every harness decision passes. `NFR-08.throughput_ratio` stays `unavailable` without a matched baseline; paired baseline/candidate host memory (resident, physical footprint, threads, descriptors) and CPU sample profiles from the same runner stand in for the comparison. The W08 endurance rehearsal passes every decision (idle first input echoed once, retained identities unchanged, no orphan, no ownership-counter growth); the 8-hour run is optional.
- [ ] Desktop evidence is the render smoke, the preflight screenshot, and the harness desktop cycles (attach, output, resize, reopen through restoration). The functional matrix and the Desktop lifecycle and recovery actions are decided by the macOS workspace tests; the interactive manual smoke on a physical Mac, local-display input latency, and dedicated-hardware performance are recorded `unavailable`.
- [ ] A failed lifecycle case, render failure, or decision miss remains unqualified. A miss is a recorded finding and a rerun; the first failure stays in the report and is never attributed to the shared VM without evidence. Fixes use a newly identified candidate and rerun the affected checks.
- [ ] The report records the run URL and the package SHA-256, and commits the raw result documents, the preflight ledger, the screenshot, and the profiles it cites, because workflow artifacts expire after 30 days.
- [ ] Report Apple Silicon qualification on the hosted VM only. macOS Intel, physical hardware, peripherals, local-display latency, and production notarization remain separate unless independently exercised and evidenced.

## Functional Requirements

- **FR-01:** A host owns at most one live or unresolved-launch runtime for a given session generation. Replacement and record removal are refused while an earlier launch, root process, or owned descendant has unresolved ownership. Retiring terminal resources does not discard a recovery owner.
- **FR-02:** Desktop, follower, or worker disconnection must not implicitly stop session processes. Build mismatch must not trigger forced retirement.
- **FR-03:** Live terminal state and output offset form one atomic checkpoint boundary. Resume never omits or duplicates bytes within the retained tail; eviction explicitly requires resynchronization.
- **FR-04:** Lifecycle changes are serialized per session and tagged with generation and operation identity. The identity is captured with viewport/foreground observations, cancellation/submission input, hook receipts, and asynchronous results, then checked again at commit. No old producer, tracker, or queued disk write may overwrite a newer generation or recreate a removed record.
- **FR-05:** The system distinguishes confirmed exit, pending stop/start, observation failure, host loss, and incompatible attachment. It never derives confirmed death solely from a timeout, a closed pipe, or a failed wait.
- **FR-06:** Stop and shutdown report unresolved root, descendant, and launch identities. No automatic cleanup signals a foreign/recycled process or discards a live owner to produce a successful response. Root exit alone cannot erase an unresolved descendant owner.
- **FR-07:** Established idle control remains usable. Proven-unsent input may be retried; ambiguous delivery is surfaced without automatic replay.
- **FR-08:** Terminal parsing and output publication do not wait for metadata persistence or control RPC completion. Critical durable transitions have explicit acknowledgement semantics.
- **FR-09:** Completed records retain stable identity and bounded final readable output while releasing live host resources. Natural exit keeps attached final content and selection/search usable until explicit close, on main panes, detached windows, and diff-dock terminals. Reopening a completed record is a read-only bounded-text view, not a new PTY or an exact reconstruction of a discarded mirror.
- **FR-10:** Memory limits apply to all queues and transient checkpoints. Slow consumers cannot impose unbounded memory use or block unrelated sessions.
- **FR-11:** Normal complete/failed record retention remains 24 hours; lost-record retention remains 30 days; running/unverified-owned records are never age-deleted. Cold text may be evicted earlier under its separate byte budget.
- **FR-12:** All implementation behavior has native Windows, Linux, and macOS paths. Platform qualifications use the fully integrated desktop, host, and existing worker candidate, include worker replacement, and report the actual target matrix.
- **FR-13:** Host/OS death is not transparent live-process migration. Surviving identities are reconciled without assuming ownership; explicit restart creates a new generation and never replays commands.
- **FR-14:** Diagnostics expose bounded counters and typed errors for state, pending ownership, queue use, retention, and persistence failures, without terminal contents, full inherited environments, tokens, or provider transcripts in routine logs.
- **FR-15:** Layout restoration, attachment retries, discovery, and ordinary row opening never create a process or increment a generation. Explicit new-terminal creation uses a fresh identity; explicit restart of an eligible known record uses the observed generation and retains stable identity. Missing or lost records are not implicit launch requests.
- **FR-16:** Every unattached same-home live/unverified session remains visible and reachable without its original workspace, an existing cwd, or a recents entry. Existing workspace grouping is retained where possible; the main sidebar provides one fallback group. Display attachment cannot rewrite host workspace ownership or create an intermediary shell.
- **FR-17:** The desktop preserves uncertainty through close, stop, quit, restart, and update decisions. Stale/missing activity is not confirmed absence. An unresolved stop-all or failed host shutdown keeps the desktop open until a new explicit user choice; neither a toast nor a logged error counts as confirmed termination. A durability-only failure is shown separately from already confirmed process exit and uses `Quit with unsaved final state` for its explicit desktop-only override.

### Desktop lifecycle and recovery actions

| Entry point | Required action | Failure or boundary behavior |
|-------------|-----------------|------------------------------|
| Open a saved layout or retry an attachment | Inspect and reattach an existing compatible live generation | Missing/ended/lost/incompatible stays visible with typed state; no create/restart RPC |
| Click an unattached live session | Attach the same ID/generation using the existing terminal surface | Duplicate attachment focuses its existing surface; ownership or generation change requires refresh, never automatic replacement |
| Click an ended record | Open bounded final text read-only through US-009 | Missing/evicted/corrupt text is explicit; no fabricated blank success or command launch |
| Click a lost/unverified record | Show uncertainty and offer non-launching retry or an explicitly labeled eligible restart | An unresolved owner blocks restart; absence of a PTY attachment is not proof of death |
| Explicit Restart / existing deliberate resume command | Start the recorded shell only after authority validates expected generation and absence of unresolved ownership | Conflict refreshes state; a missing record offers a separate new-terminal action with a fresh ID |
| Keep running when closing a workspace | Detach views and preserve the process plus a visible row | The fallback `Other sessions` group works with no open workspace and missing cwd; no dependency on the recents cache |
| Open a fallback live row with no workspace | Create only an empty ordinary display workspace, then attach directly | Use accessible recorded cwd or existing user home; unavailable roots or workspace limits leave the row visible and show the prerequisite |
| Natural child exit | Finish drain, retire host resources, and keep the existing passive view | Both exit-code branches and prior-input branches follow this policy; explicit close/removal is separate |
| Stop everything and quit/restart | Resolve each owned session and obtain host shutdown acknowledgement | Stay open with unresolved identities and Retry, Keep running and quit, or Cancel; no automatic relaunch or false successful-stop result |
| Final persistence fails after all process owners resolve | Report confirmed process exit and unconfirmed durability separately | Stay open with Retry, Quit with unsaved final state, or Cancel; the explicit override closes only the desktop and does not force host termination or discard its bounded pending revision |
| Installer/update relay | Complete host-preservation preflight before exiting or replacing artifacts | Defer a replacement that would require killing the retained host; desktop exit alone is not an installer safety proof |

The fallback group reuses existing hosted-session rows and actions. It is independent of provider-transcript browsing. An ordinary open action never implies restart, including after a failed refresh; preserved rows are marked stale until the next authoritative list. Existing inactive-record display limits may apply to ended rows, but must not hide live or unverified-owned sessions.

### Lifecycle and resource contract

The names below define logical states, not a mandatory public Rust enum or a second state machine. Use the existing schema where representable and version/add fields where necessary.

| Logical state | Owner and allowed progress | Required failure behavior |
|---------------|----------------------------|---------------------------|
| Starting | One registered launch operation, expected generation, persistence revision, and cancellation state | Timeout returns pending/unverified while ownership remains; persistence failure before spawn is retryable and does not strand the record |
| Running | One runtime owns canonical terminal state and verified child identity; clients only attach | Detached clients do not change process ownership |
| Stopping | The same owner drains output and requests termination against verified identity | Deadline/wait failure remains unresolved and blocks unsafe replacement |
| Draining | Root child exit is confirmed; final PTY bytes/descendant closure are still being reconciled | On budget expiry publish incomplete output and explicitly unresolved descendants; no invented clean completion |
| Completed | No live runtime or unresolved recovery owner; metadata plus optional bounded final text | Missing/evicted/corrupt cold text does not prevent inspect, remove, or explicit restart; pending final persistence remains a visible durability error |
| Failed | Launch ended conclusively without an owned child | Explicit Restart is allowed after correction and ownership/generation validation; attachment Retry remains non-launching |
| Lost/unverified | Host or observation evidence is insufficient for live attachment or confirmed exit | Do not signal/reap on PID alone or treat this as permission to forget an owned unresolved child |

Creation/restart and durable receipt/removal transitions prepare immutable revisions, obtain their required persistence acknowledgement, and commit through the session authority. Observed process truth is separate from durability: confirmed exit is published immediately, and terminal resources retire after final-output handling even if the final manifest cannot be written. Retain one latest pending final revision per affected session within the persistence budget, report its durability failure, and retry after storage recovers. Reserve that bounded final-state capacity when admitting a session so simultaneous exits cannot require unbounded allocation. An operation may not hold a global lock while waiting for acknowledgement. Queued metadata uses revision checks so it cannot overwrite a later lifecycle transition.

When descendants remain unresolved after final drain, retire the terminal engine and I/O only when the native adapter has released them, while retaining a separate recovery-owner record with verified handles/identities and reconciliation state. The session remains lost/unverified, not Completed. Count recovery resources separately in diagnostics and benchmarks; they are never hidden as successful reclamation. Normal removal, retention expiry, restart, and successful shutdown cannot erase or bypass that owner. A blocked native closer remains explicit unresolved ownership and an unmet resource deadline rather than an invented zero-worker result.

### Retained history and resource budgets

| Resource | Budget / policy | Exhaustion behavior |
|----------|-----------------|---------------------|
| Canonical live scrollback | Preserve the existing default 10,000-line behavior and effective host byte cap of 10,240,000 bytes; keep the existing absolute 128 MiB safety ceiling for any future supported larger budget | Reject unsupported requests explicitly; do not silently reduce configured history to meet a benchmark |
| Host live output tail | 8 MiB allocated byte storage per session, plus at most one 32 KiB in-flight chunk | Evict oldest raw bytes with monotonic offsets; followers resynchronize when needed |
| PTY output mailbox | At most 64 chunks of 32 KiB per live session | Backpressure the reader for that session; reserve independent lifecycle progress |
| Host input writer queue | At most 2 MiB per live session | Reject excess before claiming acceptance; do not drop accepted bytes silently |
| GUI queued input | Preserve the existing 1 MiB cap | Surface unsent/rejected input; no unbounded buffering during reconnect |
| Subscriber pending output | At most 8 MiB per connection, including staging retained while a write is stalled | Cancel a write at its existing 5-second deadline and disconnect/resynchronize the slow follower |
| GUI output pool | Preserve 4 pooled 32 KiB buffers on the normal path | Producer backpressure; lifecycle/control cancellation must still progress |
| One checkpoint | Existing 64 MiB encoded limit | Typed refusal before unbounded allocation; live session continues |
| Host checkpoint staging | At most 2 simultaneous captures and 256 MiB aggregate native/encoded/transient staging | Queue by deadline or return retryable capacity error; release on disconnect/cancellation |
| Mirror checkpoint ownership | At most one pending payload per mirror and zero initial checkpoint bytes retained after restore | Supersede/release obsolete attempts; preserve last valid display on error |
| Persistence queue | 8 MiB aggregate serialized/estimated payload budget per home | Coalesce metadata; reserve critical transition admission; typed failure when it cannot be accepted |
| Cold final text | At most 512 KiB per ended session and 64 MiB aggregate per home, including reserved writes | Keep newest complete readable text; oldest-ended-first cache eviction; record unavailable text on storage failure |
| Connection admission | Preserve the bounded 128-connection limit unless measurement justifies a reviewed adjustment; reserve at least 8 slots for control/inspection/stop | Reject excess with retryable busy status; no unbounded thread creation |
| Concurrent unresolved launches | At most 8 per home | Further creates receive busy without creating a process; existing live sessions remain usable |

Native terminal memory, image resources, allocator overhead, and thread stacks are measured in addition to these payload budgets. Existing engine safety limits continue to apply. This table does not pretend that adding payload limits equals total RSS.

### Platform coverage

| Platform | Required automated evidence | Required native interactive/performance evidence |
|----------|-----------------------------|----------------------------------------------------|
| Windows x64 | Native Rust/lifecycle gates and packaged helper checks | Windows 11 native suites, harness workloads, and manual smoke; Windows 10 assumed equivalent; 8-hour soak optional |
| Linux x64 | Native gates on Fedora; Ubuntu, Debian, Arch, openSUSE host lifecycle cases from the tarball in containers | Fedora Wayland and X11 (XWayland) harness desktop runs and manual smoke; x64 performance host and endurance rehearsal; 8-hour soak optional |
| Linux ARM64 | Native `ubuntu-22.04-arm` CI gates: clippy, workspace tests, release build | None required; render smoke, performance matrix, and 2-hour soak recorded unavailable |
| macOS ARM64 | Actual executed macOS CI gates and available render smoke | Hosted `macos-14` runner (Apple M1 VM): packaged preflight, full protocol with worker and desktop, W08 endurance rehearsal, paired baseline/candidate host profiles; dedicated-hardware performance, interactive manual smoke, and 8-hour soak recorded unavailable |
| macOS Intel / Windows ARM64 | Preserve applicable portable source paths | Not shipping with the current archive manifest; no qualification claim in this delivery |

macOS qualification run, one package workflow job with the full protocol enabled, within the 6-hour job cap:

| Step | Work |
|------|------|
| Build | Candidate app bundle, host, fixture, harness, and candidate manifest; profiling hosts at the baseline and the candidate; `SHA256SUMS` |
| Preflight | The extracted package checked on the runner (M01-M14) with a ledger, screenshot, and evidence export |
| Protocol | Quick run, then the full host, worker, and desktop protocol |
| Endurance | W08 endurance rehearsal with worker and desktop cycles and the idle first-input check |
| Profiles | Memory and CPU profiles of the baseline and candidate hosts on the same runner |
| Archive | Package, evidence, and profiles uploaded for 30 days; the report commits what it cites |

The optional rental: if Arthur buys a physical Mac session to fill the `unavailable` cells, `docs/release/qualification/macos-rental.md` holds the quote check, access preflight, schedule, and teardown. Its results add cells to the report; they do not rewrite the hosted-runner verdicts.

## Non-Functional Requirements

All thresholds below are acceptance targets to validate on release builds, not claimed baseline results. A failure requires a fix or an explicit reviewed PRD change. Units use MiB = 1,048,576 bytes. CPU percent is normalized to one logical CPU: `100 * process_cpu_seconds / elapsed_wall_seconds`, summed over the measured Paneflow process set. Agent/fixture workload is excluded and reported separately.

| ID | Requirement | Measurement and pass rule |
|----|-------------|---------------------------|
| NFR-01 | For 50 quiet live sessions, host-only CPU <= 1.0% of one CPU; desktop+host+worker <= 5.0% with one visible pane and 49 background attachments | W01 harness short window (4 s settle, 10 s sample) in each topology; the 50-session value passes on the designated native qualification machine. Cursor animation is disabled in the fixture profile and reported. |
| NFR-02 | Zero periodic 15/20 ms output/exit polls in the persistent path; no completed session has a timed runtime loop | Instrumented wait reasons under W01/W05; only real data, lifecycle, cancellation, GUI interaction, and documented maintenance/keepalive deadlines wake the relevant worker. |
| NFR-03 | At 50 empty 80x24 sessions, incremental private/resident host memory <= 3 MiB/session over an empty-host baseline, and desktop+host+worker incremental memory <= 8 MiB/session over the same empty topology | W01, report raw platform metric, per-process attribution, and native allocation evidence. These are empty-session budgets, not filled-history budgets. |
| NFR-04 | Within 5 s after confirmed exit and the final-drain decision, 0 host workers, 0 PTY handles/FDs, 0 live terminal allocations, and 0 live tail bytes remain owned by that completed runtime | W03/W05 resource identities and native counters; attached passive desktop content is counted separately. Unresolved children are not falsely classified as completed. |
| NFR-05 | After 10 batches of 50 create/flood/end/detach sessions and 60 s quiescence, private/resident memory is <= warmed baseline + max(16 MiB, 10%); threads and handles/FDs are <= warmed baseline + 2; last 5 batches show <= 1 MiB/batch retained-memory slope | W05; exclude bounded cold files from RAM and keep metadata/cache counts explicit. Allocator caching requires native evidence and cannot excuse growing live-allocation counts. |
| NFR-06 | 0 serialized initial checkpoint bytes retained 1 s after restore; all queue/checkpoint/cold-text budgets in the resource table are enforced | W02/W04 with filled history, repeated reconnects, cancellation, and oversized frames; peak native/encoded staging is reported. |
| NFR-07 | Metadata coalescing flush starts within 250 ms when storage is healthy; critical persistence waits <= 5 s and return typed failure on deadline; no metadata write in the PTY feed or GUI thread | W06 with healthy storage and injected 5 s stalls/permission failures; queued size stays <= 8 MiB. |
| NFR-08 | Local input-to-echo publication p95 <= 30 ms and p99 <= 100 ms at idle; with 10 other sessions emitting 1 MiB/s each, focused echo p95 <= 50 ms; persistent-path throughput >= 90% of the matched baseline | W03/W07, at least 1,000 sequenced echo samples per run. Use local monotonic timestamps through the rendered-content publication boundary, not VNC or network display time. |
| NFR-09 | A live 80x24 session with 10,000 populated history lines attaches through checkpoint+first publication in p95 <= 1,000 ms for 10 repetitions; 10 simultaneous attachments complete in <= 5 s on the qualified machine | W02 with fixed corpus and bounded admission. A larger admitted snapshot is reported separately and never mistaken for this workload. |
| NFR-10 | Startup response deadline 10 s, stop action budget 5 s, abnormal final drain <= 2 s; deadline expiry returns pending/unverified where necessary rather than false completion | W04/W06; responsiveness is measured separately from eventual ownership resolution. Every late-created fixture child remains tracked and is reconciled. |
| NFR-11 | 0 unexpected child exits or generation changes across 100 desktop detach/reopen cycles and 100 worker kill/restart or replacement cycles; 0 duplicate committed launches across 1,000 deterministic concurrent-transition schedules; 0 lost/duplicated first echo after 61 s, 5 min, and 30 min idle | W04/W08 plus barrier-based fault tests. Worker cycles include both crash recovery and build replacement. The 100-cycle counts apply to the optional 8-hour W08 run; the required W04 runs and endurance rehearsal record their own cycle counts, each with 0 identity or generation changes. Every failed repetition remains in the evidence. |
| NFR-12 | 0 orphan fixture processes, ownership leaks, or deadlocks during each required endurance run; short-lived fixture effects recover within the existing resource deadlines | W08 endurance rehearsal on the Windows and Linux qualification machines and on the hosted macOS runner; the 8-hour run is optional and Linux ARM64 runs none. |
| NFR-13 | 0 foreign or identity-mismatched processes signaled in the process-safety suite; preserve 64 KiB control-frame and 64 MiB checkpoint limits; unauthorized cross-user endpoint access is rejected | Native protocol/security fixtures, including malformed lengths, concurrent requests, and permission changes. |
| NFR-14 | 100% of required scenario/platform cells have an explicit result at the candidate SHA; 0 missing/skipped required cells are counted as passed | US-014 ledger and the three qualification reports. A changed shared core invalidates corresponding prior evidence. |
| NFR-15 | 0 implicit launches from restoration/retry/ordinary row opening; 0 successful stop-all results while owned processes or launches remain unresolved; 100% of unattached live/unverified sessions represented exactly once after each successful list publication | W04/W06/W07 and desktop entry-point assertions, including no open workspace, missing cwd, recents eviction, stale activity, final-view retention, and shutdown RPC failure. Known rows survive failed refreshes with explicit stale state. |

### Workload and measurement protocol

Record individual run samples and report median, p95, p99, maxima, sample count, and failures where meaningful. Do not compute a percentile from fewer than the stated sample count or merge different machines into one baseline. Use three repeated short runs, matched power settings and dimensions, and separate cold from warmed measurements. CPU/RAM baselines are captured on the same machine during the same qualification session whenever possible.

| ID | Workload | Required evidence |
|----|----------|-------------------|
| W01 | Empty host/worker/desktop baseline, then 1/10/50 idle fixtures; host-only, host+worker without desktop, and fully attached modes; 80x24, one visible pane, remaining attachments in background | CPU, memory, thread/handle/FD deltas, wait reasons, mirror/follower and viewport/cancellation attribution; identical process topology for each baseline/candidate pair |
| W02 | 10,000 lines of deterministic ANSI/Unicode history, 10 reattachments, 10 concurrent attachments, then 100 attach/detach cycles | Checkpoint ownership/peak bytes, attach latency, content equivalence, continuation and effects behavior |
| W03 | One throughput fixture; 10 fixtures at 1 MiB/s for 60 s; focused echo alongside; separate bounded graphics/long-line corpus | Throughput, latency, fairness, buffer admission, UI responsiveness and engine limits |
| W04 | Slow/paused follower, control disconnect mid-paste, GUI force quit, same-instance reconnect, generation change, output eviction, worker replacement, and saved-layout restoration after host loss | Byte sequence integrity, explicit uncertainty, resynchronization, unchanged surviving identities, zero implicit launches/replay |
| W05 | Ten batches of 50 create/fill/stop or natural-exit/detach operations, plus cold record retention with injected time | Live resource reclamation, memory slope, stale callback rejection, cold cache eviction |
| W06 | Disk full/denied/slow, corrupt manifest/cold text, delayed spawn, wait failure, PID mismatch, descendant-held PTY, failed descendant cleanup, occupied endpoint, runtime panic, delayed old-generation scans/signals, and stop-all/shutdown RPC failure | Typed state/result through the desktop, ownership through failure, no stale commits or false successful stop, independent-session progress, bounded watchdogs |
| W07 | Native desktop usage after idle; resize, paste, search, natural exit with zero/nonzero code and prior/no input, ended-row opening, stale activity during close, fallback discovery with zero/closed workspaces, missing cwd, recents eviction, and workspace-limit refusal | Passive final content on all terminal surfaces, zero implicit launch, unchanged live ID/generation, exactly-once row visibility, explicit limits/unavailability, local timestamped echo and no clipboard/bell replay |
| W08 | Endurance at 10 live fixtures, periodic bursts and completed-session churn, 100 desktop detach/reopen cycles, 100 worker crash/restart or build-replacement cycles over the run, and a 30-minute untouched control connection | Child identities, lifecycle ledger, resource trends, final cleanup, no accumulated errors; worker cycles do not run during the uninterrupted idle-input interval |

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Empty home | First launch, no records | Start one owner; zero terminal runtimes until create | No extra error UI |
| 2 | Launch still pending | Spawn has not returned within 10 s | Keep operation owned, reject duplicate launch, allow inspection | "Session is still starting. Its process is being checked." |
| 3 | Host compatibility mismatch | Unsupported wire/engine contract on reopening | Keep old host and agents running; refuse unsafe attach | "These sessions are still running in an incompatible host. Retry with a compatible version or stop them explicitly." |
| 4 | Replacement unavailable | Missing host/helper or denied update staging | Leave retained work intact | "The update could not be prepared. Your sessions are still running." |
| 5 | Restart persistence failure | Disk full, denied rename | Retryable state with previous identity preserved; no stranded Starting | "Could not save the restart. Check storage access and retry." |
| 6 | Ambiguous input delivery | Partial write or lost acknowledgement | Do not replay; retain last valid view | "Input delivery could not be confirmed. Check the terminal before sending it again." |
| 7 | Idle control | No input for more than 60 s | First input delivered once through usable control | No error in healthy idle case |
| 8 | Slow output client | Paused consumer reaches byte/deadline limit | Disconnect/resynchronize this client, preserve session | "Reconnecting to the running session." |
| 9 | Output eviction | Resume offset older than retained tail | Atomic checkpoint refresh; no side-effect replay | Existing reconnecting state |
| 10 | Stop cannot be confirmed | Wait/identity lookup failure | Retain owner and unresolved state; do not delete record or force successful shutdown | "The session has not been confirmed stopped." |
| 11 | Child exits before EOF | Buffered output or descendant retains PTY | Drain within budget; report incomplete final output if necessary | "Session ended. Some final output could not be collected." when applicable |
| 12 | Old callback/write | Delayed operation from earlier generation | Reject stale commit and release payload | Diagnostic only |
| 13 | Host dies | Crash or OS restart | Restore presentation as lost/unverified; no shell/agent launch, command replay, or PID-only kill | "The session could not be reattached." with non-launching Retry and a separately labeled eligible Restart/new-terminal action |
| 14 | Cold text unavailable | Cache eviction, corrupt file, write failure | Metadata remains usable; final-text API reports explicit absence | "Final output is no longer available." |
| 15 | Over limit | Oversized frame, snapshot, input, or excessive pending launches | Reject before excessive allocation, preserve other sessions | "The host is at capacity. Retry when an operation finishes." or size-specific error |
| 16 | Concurrent create/restart/remove | Multiple desktop/CLI callers | One authority commits one valid transition; losers get conflict/current state | "The session changed elsewhere. Refresh and retry." |
| 17 | Permission revoked | Endpoint or state-directory access changes | No destructive fallback, bounded error, retry available | "The session host cannot access its state directory." |
| 18 | Unknown agent during close | Missing/stale activity data | Keep activity unknown unless fresh authoritative evidence resolves it; never infer safe stop from filtered-out stale state | Existing close confirmation |
| 19 | Hosted macOS runner unavailable or failing | Queued job, runner image change, render failure in the VM, 6-hour cap exceeded | Keep qualification incomplete; archive available evidence; the optional rental is the fallback route | Operator report, no product UI |
| 20 | Runtime panic | Session runtime thread failure after child launch | Supervision retains recovery ownership and reports session uncertainty | "Session monitoring failed. Process state is being checked." |
| 21 | Restore absent/ended session | Layout restoration or ordinary row click | Keep typed missing/ended state; open retained text when available; no create/restart | Final-output view or missing-session state with separate explicit actions |
| 22 | Stop-all is incomplete | Failed stop, unresolved descendant, shutdown RPC failure, or unacknowledged final durability | Keep desktop open; distinguish unresolved process ownership from durability-only failure after confirmed exit | Unresolved processes: Retry, Keep running and quit, Cancel. Durability-only: Retry, Quit with unsaved final state, Cancel |
| 23 | Kept-running workspace is unavailable | Workspace closed, cwd deleted/moved, recents evicted, or no open workspace | Show one fallback row; attach existing live identity through an empty display workspace if needed | Existing session status in `Other sessions`; missing-path indication |
| 24 | Fallback attachment cannot open | Workspace limit or no valid display root | Keep row and process; show prerequisite; do not create a shell or change ownership | Existing capacity/path error with retry after correction |
| 25 | Natural exit | Clean/error exit with/without prior input | Drain then keep a passive final view on every terminal surface | Exit outcome and final-output completeness; explicit Close and Restart |
| 26 | Worker restarts | Crash, deliberate restart, or compatible build replacement | Rebuild projection; keep host processes and stream intact; activity remains unknown until fresh | Existing reconnecting state, followed by reconciled session state |
| 27 | Detached list fails | Worker/core unavailable during refresh | Keep known rows marked stale; reject destructive conclusions from absence | Existing reconnecting/unavailable state and Retry |

All ten planning categories are covered by these 27 cases: empty, loading, errors, IPC degradation, permission changes, concurrency, limits, reversal/retry, interrupted flows, and external dependencies. An explicit stop cannot be undone; confirmation and accurate outcomes protect that boundary rather than promising reversal.

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|-------------|--------|------------|
| 1 | Refactor loses final output or double-applies a snapshot | Medium | High | Atomic cut, exit/EOF distinction, continuation corpus, slow-reader and simultaneous-exit tests in US-006/007/008 |
| 2 | Synchronous late spawn cannot be cancelled immediately | Medium | High | H02/US-001 validation; registered pending owner, bounded admission, late-child cleanup, no false cancellation claim |
| 3 | Windows PTY teardown differs from Unix and can block | High | High | Keep independent I/O; test actual bundled ConPTY, Windows 10/11 native stop/drain paths, explicit unresolved state |
| 4 | Memory budgets hide native allocations or allocator caching | High | High | Native profiles plus ownership/capacity counters; same-machine baselines; no Rust-allocator-only claim |
| 5 | Async persistence reorders critical state | Medium | High | Per-session revisions, deletion/generation barriers, crash-consistency and write-failure injection |
| 6 | Core changes bypass existing worker/scan consumers or overlap their ownership | High | High | Explicit producer inventory, capture/commit generation barriers, mandatory integrated worker tests; no duplicate service, reducer redesign, tracker edits, or automatic cross-certification |
| 7 | Source baseline changes while implementation is in progress | High | Medium | Preserve local work, record commit/diff identity, recheck affected findings, invalidate stale qualification evidence |
| 8 | Existing green CI skipped required jobs or retried a flake | Medium | High | Per-job result ledger, first-failure retention, required candidate matrix rather than aggregate status |
| 9 | The hosted macOS runner is a shared VM: timing noise, no Metal report, image drift, 6-hour job cap | Medium | Medium | Package built and preflighted in the same job, runner identity recorded, every miss kept as a finding and rerun, physical-hardware cells recorded unavailable, optional rental route documented |
| 10 | New cold text is mistaken for durable complete history | Medium | Medium | Explicit size/eviction/completeness metadata; no journal or host-death restoration promise |
| 11 | Snapshot capture still pauses its own session | Medium | Medium | Bound capture size/admission, measure attach/input latency; do not add speculative copy-on-write engine changes unless the documented budget fails |
| 12 | Core correctness is hidden by launch-capable restore, automatic exit closure, or ignored quit errors | High | High | Explicit entry-point table, zero-launch assertions, passive-view tests, and aggregate stop outcomes through GUI and installer paths |
| 13 | Kept-running processes remain unreachable after workspace closure or path removal | Medium | High | Reuse global host list with one fallback sidebar group; qualify zero-workspace, missing-cwd, recents eviction, duplicate attach, and capacity/error cases |

Devil's advocate decisions: the top three architecture risks remain final-output correctness, ownership of late/failed launches, and persistence ordering. Each has deterministic failure coverage before platform acceptance; desktop entry points must preserve the same invariants. Herdr-style live transfer was considered and excluded because it adds a different ownership protocol, lacks a demonstrated Windows path, and weakens the exact-state attachment comparison. The scope remains 20 stories with explicit execution checkpoints; no additional service, session manager, protocol family, or journal is hidden inside a performance story.

## Non-Goals

- Continuous raw terminal journaling or lossless unlimited history on disk. Only bounded final readable text is added to support runtime retirement.
- Transparent survival of host death, machine reboot, or live PTY migration between host versions, including a Herdr-style Unix-only handoff. Explicit eligible restart creates a new generation; provider resume and command resurrection are not added here.
- A new activity reducer, provider hook format redesign, runtime catalog migration, replacement worker architecture, or remote/mobile Controller. The existing worker is integrated and qualified as a consumer; its feature development remains owned by the Agent Runtime System PRD.
- Replacing libghostty, changing its pin to hide a host bug, adopting an alternate terminal parser, or redesigning GPUI rendering.
- A shared epoll/kqueue/IOCP runtime rewrite, Tokio migration, binary transport redesign, or telemetry backend. Existing mechanisms can satisfy the selected contract; measured failure is required before expanding that scope.
- New shipping macOS Intel or Windows ARM64 archives, or a claim that Apple Silicon measurements qualify them.
- Public release/tagging, cloud provisioning, paid agent usage, signing-account setup, or changes to production user homes as part of this PRD-writing task.

## Files NOT to Modify

- Existing unrelated changes in `AGENTS.md`, terminal element contrast/golden fixtures, `src-app/src/terminal/ghostty_session.rs`, `src-app/src/terminal/perf_bench.rs`, and theme/palette files. Recheck Git status before implementation; this list is the audit snapshot, not permission to discard other edits. If necessary session work intersects a dirty file, preserve unrelated edits and inspect the task-owned diff.
- [`tasks/prd-agent-runtime-system.md`](C:/dev/paneflow/tasks/prd-agent-runtime-system.md) and its status tracker. This PRD references their scope and must not silently rewrite their requirements or status.
- `runtimes/*/runtime.toml`, provider setup assets, and provider configuration files, except when a separately authorized integration task owns the change. Hook storage ordering here is provider-neutral.
- `native/libghostty/manifest.toml`, prebuilt archives, generated bindings, GPUI revision pins, and `rust-toolchain.toml`. A newly proven blocker requires a separately justified scope decision.
- Code editor and diff rendering, browser/CEF, theme redesign, and unrelated sidebar/command-palette features. Narrow exceptions are the existing diff-dock terminal exit/final-view path for US-009 and hosted-session sidebar rows/fallback discovery for US-010; they authorize no adjacent redesign.
- Existing real user state under `.paneflow`, `.paneflow-dev`, or `.unpeel`, signing keys, and provider secrets. Test data belongs in private fixture homes.

Expected implementation surfaces are `crates/paneflow-host`, `crates/paneflow-ipc-client`, `crates/paneflow-home` for scoped state paths, existing `crates/paneflow-serve` consumers where the core contract requires adaptation, terminal attachment/runtime/final-view modules, saved-layout restoration, hosted-session/sidebar recovery, existing close/quit/update routes, test fixtures, benchmark scripts, relevant CI/package checks, and tracked architecture/runbooks. Split modules at ownership boundaries when necessary; do not perform adjacent cleanup. This document update itself changes only this PRD and its tracker.

## Technical Considerations

These questions guide engineering validation within the fixed functional contract. A different internal technique is acceptable only if it preserves the same acceptance criteria, cross-platform behavior, and evidence requirements.

- **Architecture:** Can the current runtime mailbox and a per-session transition guard provide one authority without a new global actor? Recommended: retain terminal serialization per session, introduce explicit operation/generation ownership, and keep registry locks short. Confirm with deterministic concurrency tests.
- **Exit delivery:** Can a blocking `Child::wait` worker plus `clone_killer` remove polling on each native adapter? Recommended: yes, subject to H01. Keep independent PTY reader/writer service and explicit EOF handling; one extra sleeping waiter is acceptable if thread and memory budgets pass.
- **Output notification:** Can a condition variable or equivalent sequence-predicate wake replace follower sleeps without changing the existing wire? Recommended: reuse current offset protocol and bounded buffers. Verify missed-wakeup, disconnect, and shutdown ordering before considering transport changes.
- **Data model:** Which existing manifest states can represent pending/unverified ownership without a breaking schema change? Recommended: preserve external identity, add versioned operation/revision fields only where needed, and test old-record loading plus downgrade rejection. Never reinterpret unknown as exited for compatibility.
- **Persistence:** Can one bounded writer with per-session revisions and critical completion barriers serve manifests and hook seeds? Recommended: use unique temporary files and native atomic replacement, with critical flush semantics tested per OS. Avoid introducing a general event-sourcing database.
- **Completed output:** Can the existing bounded text extraction produce a <=512 KiB final artifact without a new snapshot format? Recommended: bounded readable text and metadata, with explicit truncation/unavailable status. Existing displayed final content remains in the passive desktop view until closed.
- **Desktop intent:** Can existing attachment intents and hosted-session rows express restore, inspect, and explicit restart without a new manager? Recommended: make restoration/Retry/ordinary row opening non-launching, retain native mirrors on natural exit, and reuse one fallback sidebar group plus empty-workspace construction. Update the current exit-discriminator and row-resume tests to the stated product behavior.
- **Stop outcomes:** Can the existing quit dialog consume an aggregate result that distinguishes root completion, unresolved descendants/launches, shutdown failure, and persistence failure? Recommended: propagate typed outcomes through the final action, retain the window on unresolved stop-all, and require a new explicit keep-running choice before quitting without confirmation. Successful signaling alone is never success.
- **Control I/O:** How should cancellation and partial delivery be represented without blocking mirror publication? Recommended: a bounded control worker using the existing client protocol; keep established control alive and report ambiguous delivery. Do not promise exactly-once delivery through arbitrary connection loss without an acknowledgement/deduplication contract.
- **Compatibility:** Which protocol capabilities and snapshot identity actually require refusal? Recommended: compatible build changes attach; unsupported contracts preserve the old host and return an actionable mismatch. Do not claim universal N/N-1 compatibility or downgrade safety without fixtures.
- **Existing worker:** Which viewport/cancellation and worker-consumer paths need identity/revision adaptation? Recommended: inventory all producers, capture generation with source data, reject obsolete commits, and qualify worker restart separately from core death. Keep existing provider semantics and record any required consumer changes without altering the other tracker.
- **Migration:** Can current records load without eagerly allocating terminal state? Recommended: lazy cold records, retain existing manifest retention, treat missing final text as unavailable, and keep active old hosts alive. Incompatible persisted state must fail visibly without deleting user data.
- **Dependencies:** Are the existing Rust synchronization and pinned PTY/IPC crates sufficient? Recommended: yes. Add no new runtime dependency merely to match Unpeel's implementation. Document any demonstrated blocker before revising this choice.
- **Observability:** Which counters establish release rather than only a lower RSS? Recommended: live runtime/terminal/tail/checkpoint ownership, pending operation counts, queue bytes/capacity, and native memory/CPU/handle traces, using bounded diagnostic snapshots and rate-limited errors.

### Dependency and execution map

IDs are local to this PRD; always pass this PRD path together with an EP/US identifier to downstream workflows. The other PRD uses overlapping numeric IDs and must not be confused with this tracker.

| Stage | Stories | Blocking relationship |
|-------|---------|-----------------------|
| Primitive/baseline validation | US-001 | First, including native H01/H02 evidence |
| Safety fixes | US-002 through US-005 | US-002 can proceed independently of US-003 after baseline; US-004/005 depend on US-003 |
| Runtime refactor | US-006 through US-010 | Follow their explicit dependencies; snapshot ownership and lifecycle-adapter work can proceed independently once their prerequisites pass |
| Integrated persistence and budgets | US-011 through US-014 | Consolidate existing fixes; all implementation stories reviewed before US-014 qualification handoff |
| OS qualifications | US-015 through US-020 | All depend transitively on US-014; the three OS epics can run independently against the same frozen candidate |

No platform qualification story is a substitute for implementing a missing OS path. If parallel engineering is later used, assign disjoint module ownership and serialize shared-file integration before US-014.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|--------------------|--------|-----------|-------------|
| Implicit process termination on build mismatch | Present in inspected bootstrap path; runtime frequency unmeasured | 0 in 100 preservation cycles and compatibility matrix | EP-001 safety fix; reconfirm all OS epics | Process identity ledger, exit events, packaged upgrade fixtures |
| Implicit launches during restore or row opening | Launch-capable `Resume` exists in saved-layout and ended-row paths | 0 create/restart/process launches from non-launching entry points | US-002/009/010, then all OS epics | IPC request ledger plus fixture child-creation counters for every record state |
| Stop-all uncertainty reaching the desktop | Some stop/shutdown failures are ignored; stale activity can become known absence | 0 false confirmed-stop decisions or automatic quit/restart after unresolved outcomes | US-004, then all OS epics | Failed wait/tree cleanup/RPC and stale-state injection through real close/quit actions |
| Kept-running session access | Discovery depends on an eligible open workspace; runtime incidence unmeasured | 100% live/unverified unattached records reachable exactly once after successful publication | US-010, then all OS epics | Zero-workspace, missing-cwd, recents eviction, capacity, and failed-refresh cases |
| Final-view retention | Ordinary natural exits close surfaces under the current tested discriminator | 0 automatic final-view closures across zero/nonzero exit and prior/no-input combinations | US-009, then native GUI qualification | Main pane, detached window, diff-dock terminal, and read-only ended-row cases |
| Duplicate retained initial checkpoint | Deep clones present in inspected control/follower construction; bytes unmeasured | 0 retained initial payload bytes 1 s after restore | EP-002 and final qualification | Ownership counters plus native heap evidence |
| Completed host runtime retention | Runtime continues until record removal; 24-hour retention and opportunistic sweep | NFR-04 resource release and NFR-05 churn recovery | EP-002 and all performance qualifications | Workers, handles/FDs, allocations, memory slope |
| Idle persistent-path CPU | Unmeasured; 20 ms runtime tick, 15 ms follower poll, 500 ms viewport scan, and 100 ms cancellation scan are source facts; existing worker/mirrors add work | NFR-01/02 across the actual process set | Baseline in US-001; acceptance in EP-004/005/006 | Native CPU time and attributed wait reasons at 1/10/50 sessions |
| Empty-session memory | Unmeasured on current candidate and target hardware | NFR-03 | Same-machine qualification baseline/candidate runs | Private/resident memory, native heap evidence |
| Idle-first-input correctness | Stale cached control path identified; no reproduction recorded during PRD drafting | 0 missing/duplicate first input at all 3 idle durations | US-010, then each OS epic | Sequenced echo fixture and byte ledger |
| Concurrent/failed lifecycle correctness | A07-A11, A17, and A18 source paths identified | 0 orphan/false-exit/duplicate-launch/stale-commit results in NFR-10/11/12 scenarios | EP-001 through final qualification | Deterministic schedule barriers, injected faults, owned-process cleanup |
| Worker independence | Existing worker/core split is implemented; no qualification run in this PRD | 0 child-identity or generation changes caused by 100 worker crash/restart or replacement cycles | US-013 and all OS epics | Child/stream ledger, accepted-state projection reconstruction, same candidate artifacts |
| Input, attach, and throughput | Existing local microbenchmarks do not establish full persistent-path baseline | NFR-08/09 and >=90% matched throughput | US-013 and each native performance story | Monotonic timestamped fixtures and raw repeated measurements |
| Qualification completeness | No runs performed for this PRD | 100% required cells recorded; 0 required unverified cells counted as passed | Before PRD DONE | Candidate-bound evidence ledger and independent epic review |

## Open Questions

No unresolved product-scope question blocks this PRD from being READY. The following are owned validation inputs, not permission to invent results:

| Question | Owner and deadline | Required outcome |
|----------|--------------------|------------------|
| Do blocking wait and late-launch ownership behave as expected on all native adapters? | Implementer, US-001 before US-004/007 | H01/H02 evidence or an explicit blocker and revised adapter decision |
| What are the measured release baselines on the selected machines? | Implementer, US-001; operator for each OS qualification | Raw baseline files, hardware identities, and unavailable fields explicitly marked |
| Which existing worker and scan consumers need adaptation to the revised core contract? | Implementer, US-003/011; integration owner, US-014 | Producer inventory and candidate integration recorded; worker cases mandatory; no duplicate ownership or automatic cross-PRD status changes |
| Which Linux/Windows environments and macOS runner image will be used? | Qualification operator, before the corresponding OS execution story | Exact supported matrix, access preflight, package identities, and explicit missing coverage |
| Is a physical Mac rental worth buying for the unavailable macOS cells? | Arthur, after US-020 | Not required since v1.6; if bought, a verified quote and teardown per the rental runbook; no purchase performed by PRD generation |
| Are all performance budgets feasible without extending scope? | Implementer/reviewer, US-013 before qualification freeze | Pass evidence or documented bottleneck; any changed target requires a PRD changelog entry and retained original measurements |

### Pre-save validation record

| Check | Satisfying section |
|-------|--------------------|
| Problem and why now | Problem Statement, numbered findings and Why now |
| Objective performance language | NFR-01 through NFR-15 and Workload and measurement protocol |
| Explicit exclusions | Non-Goals, including journal, resurrection, worker redesign, and new target archives |
| Edge/error coverage | Edge Cases & Error States, 27 cases across all 10 categories |
| Unhappy path in every story | Each US includes an injected failure, missing-access, capacity, stale-state, or threshold-miss criterion |
| Baseline, target, timeframe | Success Metrics; unmeasured baselines labeled explicitly |
| Numerical NFRs | Non-Functional Requirements and Retained history and resource budgets |
| Personas and workarounds | Target Users, with inference identified |
| Risk probability and impact | Risks & Mitigations |
| Determinate behavior | FR-01 through FR-17, Desktop lifecycle and recovery actions, lifecycle table, budgets, matrix, dependency map |
| Story sizes | 20 stories, each <= XL (8 points) |
| Bounded story count | 6 epics, 20 stories, implementation then qualification |
| Assumptions and validation | H01-H06 with owners; high-risk H01/H02 in US-001 |
| Engineering questions | Technical Considerations, recommendations within fixed acceptance contracts |
| Version and audit record | Changelog, versions 1.0/1.1; current source SHAs, A01-A18 mappings, Herdr/Unpeel comparison and limits |

Revision 1.1 planning validation on 2026-09-21: 18 required sections, 6 epics, 20 stories, 159 acceptance criteria, 18 audit mappings, 17 functional requirements, 15 numerical NFRs, 27 edge cases, and 58 valid local file links. Story IDs, titles, sizes, dependencies, and epic totals match the JSON tracker; dependencies are acyclic and every story includes failure coverage. A separate read-only coherence review checked restoration, final views, fallback access, stop uncertainty, and durability. All story/epic statuses remain TODO, with no execution or certification timestamps. No Rust build, runtime test, benchmark, or native qualification was run for this revision.

READY means the agreed scope is specified and can enter the implementation workflow. It does not mean implementation, performance acceptance, release approval, or OS qualification has already occurred. All stories start TODO; certification follows the shared status workflow.
[/PRD]
