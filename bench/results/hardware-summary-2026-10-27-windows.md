# Real hardware protocol on Windows, after `prd-resource-efficiency`, 2026-10-27

US-020 of `tasks/prd-resource-efficiency.md`: the Windows pass over the
rendering and loop changes of EP-002 to EP-005, US-021 and US-022, on the
Windows 11 side of the dual boot that ran the Linux protocol the same day
([hardware-summary-2026-10-27.md](hardware-summary-2026-10-27.md)).

`main` at `b57da648` (`paneflow --version` reads 0.18.0), a release build
(`cargo build --release --locked -p paneflow-app -p paneflow-host`) whose
`paneflow.exe` and `paneflow-host.exe` were copied out of the target directory
and launched from there. Windows 11 Pro 10.0.26300, French locale, 2560x1440
at 240 Hz on the GeForce RTX 4070 Ti SUPER, Radeon iGPU idle. Ryzen 7 7800X3D.
The daily instance had no desktop or host running; its worker was stopped with
`paneflow serve stop` before the runs.

Each state ran in a seeded home (`~/.paneflow-hw-main-b57da648`, telemetry off,
`session.json` with no workspace, `bin` a plain file so the release build
stages no helper and registers no agent hook), default settings otherwise,
cursor blink included, window maximized. A PowerShell probe read
`GetForegroundWindow` once per second through every window: the window was
focused in every sample of the focused runs and in none of the inactive run.
Frame time is not measured: PresentMon is not installed on this machine.

- `tui-30-workspaces`: 30 workspaces opened with `paneflow new`, each in its
  own committed git repository holding one modified file, the last one selected
  and running `bench\fixtures\tui-agent.ps1 -Seconds 3000` (Windows PowerShell
  5.1). The repositories were created several minutes before the workspaces,
  so no first probe fell inside `RACY_CONFIG_WINDOW`.
- The git goal: 3 more workspaces (33), the window kept active 5 min, then a
  300 s window (label suffix `ws33-warm-w300`).
- `idle-4-panes`: one workspace in such a repository, split 2 x 2 with
  `paneflow split`, four idle PowerShell 7 shells, measured focused, then
  with the taskbar focused (label suffix `inactive`).
- `agent-thinking`: `idle-4-panes` with one pane running a fake Claude Code
  session: `paneflow-ai-hook SessionStart` and `UserPromptSubmit` with
  `PANEFLOW_AI_TOOL=claude`, then `OSC 7501 state=working`, the state
  `paneflow ps` reported as `thinking` through the window.

This is the first Windows run of the protocol. It needed three repairs, made
in this story: `scripts/perf-hardware.ps1` took the worker for a second
desktop because Windows quotes its arguments (`"serve" "run"`); `typeperf`
returned no sample because its closing line is printed in the system code page
and the harness discarded any output that was not UTF-8; and the per-thread
context switches were Linux-only, they are now read from
`NtQuerySystemInformation(SystemProcessInformation)`. The two PowerShell
fixtures also printed their braille and block glyphs as `?` until they set
the console output encoding to UTF-8.

## Measurements

CPU is percent of one core; the desktop main thread is the `main` thread.
GPU is the desktop process's 3D engines (`typeperf`), not the whole device as
on Linux.

| State | Window | Main thread | Desktop CPU | Host CPU | Worker CPU | Desktop RSS | Host RSS | Worker RSS | Root renders per s | git processes per min | `sidebar_renders` | GPU p50 / p95 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `idle-4-panes` | 62 s | 1.87 % | 2.42 % | 0.03 % | 0.03 % | 156.3 MiB | 15.9 MiB | 17.1 MiB | 1.84 | 5.8 | 0 | 0.06 / 0.06 % |
| `idle-4-panes`, window inactive | 62 s | 0.86 % | 1.21 % | 0.00 % | 0.10 % | 154.8 MiB | 15.9 MiB | 16.1 MiB | 0.00 | 0.0 | 0 | 0.00 / 0.00 % |
| `agent-thinking` | 62 s | 3.66 % | 4.80 % | 0.00 % | 0.05 % | 159.7 MiB | 16.1 MiB | 16.3 MiB | 11.13 | 5.8 | 689 | 0.31 / 0.34 % |
| `tui-30-workspaces` | 62 s | 5.17 % | 7.70 % | 1.42 % | 0.08 % | 223.6 MiB | 24.9 MiB | 16.9 MiB | 32.25 | 20.4 | 0 | 0.73 / 0.82 % |
| 33 workspaces, 5 min warm | 306 s | 4.84 % | 6.78 % | 1.55 % | 0.13 % | 230.5 MiB | 26.7 MiB | 16.6 MiB | 32.20 | 24.7 | 0 | 0.72 / 0.80 % |

The 300 s window at 33 workspaces counts 126 git processes, 42 `diff` and
0 `config`, the same split as Linux after US-021.

Wakeups per second, per thread. Windows reports full thread names, so unlike
Linux each worker and session thread is named, not inferred from its period:

| State | `paneflow-ipc` | `paneflow-serve-pump` | `paneflow-serve-conn` (the follower) | `paneflow-serve-core` | Desktop session threads (`paneflow-ghostty-*`) | Host session threads |
|---|---|---|---|---|---|---|
| `idle-4-panes` | 0.00 | 0.78 | 0.50 | 0.50 | follower 0.50 each, attached and control 0.00 | 0.00 |
| `idle-4-panes`, inactive | 0.00 | 0.74 | 0.52 | 0.52 | same | 0.00 |
| `agent-thinking` | 0.00 | 0.74 | 0.52 | 0.50 | same | 0.00 |
| `tui-30-workspaces` | 0.00 | 10.13 | 0.60 | 9.90 | the animated pane: attached 117.3, follower 22.1; the other 29: 0.52 or less | the animated pane 27.5, others 0.00 |

## Goals of the PRD on Windows

| Goal | Target | Linux, 2026-10-27 | Windows | Verdict |
|---|---|---|---|---|
| CPU of the main thread, animated TUI, 18 workspaces | at most 6 % | 1.69 % with 30 workspaces | 5.17 % with 30, 4.84 % with 33 | met, closer to the bound than on Linux |
| Cost of one frame, same scenario | at most 2.5 ms | 1.1 ms | 1.6 ms (5.17 % / 32.25 per s) | met |
| git processes per minute at rest, 33 workspaces, window active | at most 30 | 25.2 | 24.7 | met |
| git processes with no window active | 0 outside `.git` events | not measured | 0 in 62 s | met |
| Sidebar renders during terminal output without agent | at most 2 per 30 s | 1 per 300 s | 0 per 300 s | met |
| Root renders per s, one agent thinking, shimmer visible | between 10 and 12 | 11.10 (`perf_gates`) | 11.13 | met |
| Wakeups per s at rest, IPC thread | at most 1 | 0.2 | 0.00 | met |
| Wakeups per s at rest, worker follower and pump (US-016, US-022) | at most 1 each (at most 10 in 10 s) | 0.72, 0.5, 0.5, 0.0 (threads not told apart by procfs) | follower 0.50, pump 0.78 | met |
| Wakeups per s at rest, idle session thread (US-018) | at most 1 | 0.5 | 0.50 desktop, 0.00 host | met |
| Root renders with no window active (US-010) | at most 1 per 30 s | not measured by the protocol; `perf_gates` budget | 0 in 62 s | met |

## Visual checks

Captures are local evidence, not tracked
(`tasks/resource-efficiency-ep007/`):

- **Z-order fixture (US-007)**, `zorder-windows.png`: colored backgrounds and
  inverse under their text, block and box-drawing sprites in place, struck
  and underlined text including struck text on red, the double and curly
  underlines, the kitty image under the text (red, text readable over it) and
  the one above it (blue, text hidden under it), the block cursor after the
  prompt. ConPTY forwards the kitty APC sequences. Every line reads as the
  fixture describes; the comparison with the Linux render of the same script
  is Arthur's, from the US-007 capture. The hyperlink hover underline and the
  selection line did not respond to synthesized mouse input (`SetCursorPos`,
  then absolute `mouse_event`), so they are not checked here.
- **Cached sidebar and shimmer (US-008, US-009)**, `shimmer-steps.png`, three
  captures of the thinking row taken in sequence: the highlight moves across
  "Tab 1" and the spinner dots change between captures while the sidebar is
  served from its cached view (`sidebar_renders` equals the 11.1 per s of the
  shimmer grid while thinking, 0 during the TUI runs).
- **Blink stop (US-010)**, `caret-active-vs-inactive.png`: focused, the block
  caret blinks in the focused pane; inactive, no frame is drawn and no caret
  is shown in any pane. The second half is not Windows-specific:
  `TerminalView::render` passes `focused && window.is_window_active()` to the
  element, and an unfocused terminal paints no live cursor by design
  (`unfocused_terminal_hides_live_cursor`, since July). US-010 keeps the blink
  phase visible so the caret shows at once on reactivation, which holds.

## Stories covered by the native Windows suites

The epic review ran these on the same machine and commit, with the tracked
worktree dirty only by this story's changes:

- **US-012, US-013 (scroll row reuse)**: `scripts/bench-terminal.ps1`
  ([20261027T160701Z-b57da648a84b.json](20261027T160701Z-b57da648a84b.json),
  no Windows baseline yet) reads `layout_scroll_cached_220x60` at 95.9 us
  (target at most 150) and `publish_scroll_220x60` at 86.8 us (target at
  most 100). `cargo test -p paneflow-terminal-ghostty --features native`
  passes, including the `row_shift` snapshot tests.
- **US-014 (hidden panes)**: the `PublishGate` tests of
  `ghostty_session::publish` pass on Windows, including
  `a_hidden_gate_publishes_at_most_once_a_second_even_for_urgent_output`. The
  runtime-level hidden pane tests of `pty_runtime.rs` drive a POSIX shell and
  are `cfg(unix)`, so no Windows run publishes from a hidden pane under
  output.
- **US-017 (git event channel)**: the `git_watch` tests pass on Windows. The
  main thread wakes about 1,050 times a second for GPUI (see below), so this
  pass cannot isolate the absence of the 200 ms timer on that thread.
- **US-011 (glibc arenas)**: Linux gnu only; on Windows `cap_malloc_arenas`
  is the empty fallback (`src-app/src/launch.rs`), and the build runs.

## Windows gaps found

- **The in-process runtime keeps a 100 ms quiet tick on Windows.**
  `idle_wakeups_shell_per_s` of the terminal bench reads 10.0 per s on Windows
  against 0.0 on Linux after US-018, which asks for at most 1. US-018 kept it
  on purpose (`5f1b1af95`): ConPTY does not signal the child's exit through
  EOF, so `run_runtime` (`pty_runtime.rs`, `quiet_wait`) polls its lifecycle.
  That runtime is reached only by `#[cfg(test)]` `GhosttySession::start`,
  which the bench uses; the attached runtime every real pane runs measured
  0.00 wakeups per s above. No user-facing wakeup, but the bench metric cannot
  meet its US-018 bound on Windows until the child exit wakes the mailbox.
  Accepted on 2026-10-27 as a bench limit, with no story: fixing it would add
  a ConPTY child exit waiter to test-only code for no user-facing gain.
  The bench note also counts these waits as `gate waits`, because
  `count_runtime_loop_wait` only tells a `None` wait and the 10 ms tick apart.

- **The `notify` watcher loop wakes 10 times a second per watcher.** Two
  `notify-rs windows loop` threads wake 9.97 times a second each in every
  state, active or not. `notify` 7.0.0 (the version in `Cargo.lock`) and 8.2.0
  wait on their wakeup semaphore with a 100 ms timeout
  (`WaitForSingleObjectEx(self.wakeup_sem, 100, 1)` in `src/windows.rs`); on
  Linux the inotify backend blocks. FR-12 asks that the git watcher not wake
  without an event. Fixing it means patching or replacing the `notify` Windows
  backend, a dependency decision: recorded as US-023 in the PRD, not fixed
  here.
- **Root renders under the TUI fixture: 32.2 per s against 15.7 on Linux.**
  The fixture writes a frame every 50 ms and a title every 100 ms on both
  platforms, so 32 per s is one render per frame, per title and per blink, none
  coalesced. The frame cost stays within the PRD's bounds (1.6 ms, main thread
  at 5.17 %), so no goal is missed; the 240 Hz display against 144 Hz on Linux
  is a likely contributor this run does not isolate.

## Outside the goals

- The desktop main thread wakes about 1,050 times a second in every state,
  including with the window inactive and 0 renders, and `VSyncProvider` about
  250 times a second (the 240 Hz display); an unnamed thread wakes 96 times a
  second. GPUI's Windows platform owns all three. The inactive main thread
  costs 0.86 % of a core with nothing drawn, against 0.71 % active at rest on
  Linux. Not among the PRD's goals; worth a look for laptop users.
- The desktop's resident memory is 156 MiB at rest and 224 to 231 MiB with 30
  or 33 workspaces, against 231 and 298 MiB on Linux.

Results:
[idle-4-panes](hardware-windows-x86_64-windows-idle-4-panes-main-b57da648-20261027T155027Z.json),
[idle-4-panes, window inactive](hardware-windows-x86_64-windows-idle-4-panes-main-b57da648-inactive-20261027T155224Z.json),
[agent-thinking](hardware-windows-x86_64-windows-agent-thinking-main-b57da648-20261027T155555Z.json),
[tui-30-workspaces](hardware-windows-x86_64-windows-tui-30-workspaces-main-b57da648-20261027T153540Z.json),
[33 workspaces, 300 s after a cycle](hardware-windows-x86_64-windows-tui-30-workspaces-main-b57da648-ws33-warm-w300-20261027T154235Z.json).
