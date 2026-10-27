# Real hardware protocol, before and after `prd-resource-efficiency`, 2026-10-27

US-019 of `tasks/prd-resource-efficiency.md`: the last published release,
`v0.18.0` (2026-10-24, which already carries EP-001, US-001 to US-005), against
`main` at `38e873f2`, which carries EP-001 to EP-005. `v0.18.0` is the
published tarball `paneflow-0.18.0-x86_64.tar.gz`; `main` is a release build of
`38e873f2` (`cargo build --release --locked`). Fedora 44 (kernel 7.2.11),
GNOME Shell 50.6, 2560x1440 at 144 Hz. Ryzen 7 7800X3D, GeForce RTX 4070 Ti
SUPER (driver 615.78.08) driving the display, Radeon 610M idle at 0 % in every
run.

Each build ran alone from a terminal outside Paneflow, with a seeded empty home
(`~/.paneflow-hw-<label>-<state>`, telemetry off, `session.json` with no
workspace) and default settings otherwise, cursor blink included. The window
ran under X11 through XWayland (`env -u WAYLAND_DISPLAY`) as on 2026-10-20 and
2026-10-25, maximized by `xdotool` to 2560x1400, and `xdotool` checked once per
second that it kept the focus: every sample of every window was focused. Each
measurement started about 15 s after the state was reached. Frame time is not
measured: no MangoHud log.

- `idle-4-panes`: one workspace in a committed git repository holding one
  modified file, split 2 x 2 with `paneflow split`, four idle shells.
- `tui-30-workspaces`: 30 workspaces opened with `paneflow new`, each in its
  own such repository, the last one selected and running
  `sh bench/fixtures/tui-agent.sh 600` from `main`, so both builds replay the
  same fixture. Its title read `⠙ tui fixture` when each window ended.

The four protocol windows last 60 s. Because the hidden workspaces are probed
on a 5 min cycle, a 60 s window cannot settle the git goal, so `main` also ran
`tui-30-workspaces` for 300 s with 30 workspaces, and for 300 s with the 33
workspaces the goal names (label suffixes `w300` and `ws33-w300`).

This run is the first to read wakeups: the protocol now records, per process,
the context switches per second of every thread alive through the window,
voluntary and involuntary, grouped by the thread name procfs gives
(`wakeups_per_s_by_thread_name`, `bench/README.md`). Linux truncates thread
names to 15 bytes, so `paneflow-ghostt` groups the desktop's session runtime,
attached, display, control and follower threads, and `paneflow-serve-` groups
the worker's accept, core link, pump and connection threads.

## Measurements

CPU is percent of one core. The desktop main thread is the `paneflow` thread.
GPU is the NVIDIA device load (whole device, compositor included).

| Build | State | Window | Main thread | Desktop CPU | Host CPU | Worker CPU | Desktop RSS | Host RSS | Worker RSS | Root renders per s | git processes per min | `sidebar_renders` | GPU p50 / p95 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `v0.18.0` | `idle-4-panes` | 60 s | 0.60 % | 0.73 % | 0.01 % | 0.04 % | 235.4 MiB | 8.3 MiB | 15.0 MiB | 1.8 | 7.0 | no counter | 8 / 20 % |
| `main-38e873f2` | `idle-4-panes` | 60 s | 0.71 % | 0.77 % | 0.01 % | 0.02 % | 231.3 MiB | 9.9 MiB | 14.8 MiB | 1.9 | 7.0 | 0 | 7 / 19 % |
| `v0.18.0` | `tui-30-workspaces` | 60 s | 22.35 % | 23.03 % | 1.62 % | 0.18 % | 309.8 MiB | 16.4 MiB | 15.3 MiB | 24.0 | 31.0 | no counter | 31 / 41 % |
| `main-38e873f2` | `tui-30-workspaces` | 60 s | 1.69 % | 2.08 % | 1.39 % | 0.14 % | 297.6 MiB | 16.9 MiB | 15.0 MiB | 15.7 | 31.0 | 1 | 24 / 36 % |
| `main-38e873f2-w300` | `tui-30-workspaces` | 300 s | 1.72 % | 2.12 % | 1.42 % | 0.14 % | 297.2 MiB | 18.1 MiB | 14.6 MiB | 15.7 | 29.4 | 1 | 21 / 38 % |
| `main-38e873f2-ws33-w300` | 33 workspaces | 300 s | 1.71 % | 2.11 % | 1.43 % | 0.14 % | 306.4 MiB | 18.6 MiB | 14.5 MiB | 15.7 | 31.8 | 1 | 30 / 46 % |

Wakeups per second, per thread:

| Build | State | `paneflow-ipc` | `paneflow-serve-` threads | `paneflow-ghostt` threads above 1 per s | Other `paneflow-ghostt` threads |
|---|---|---|---|---|---|
| `v0.18.0` | `idle-4-panes` | 99.6 | 66.5, 10.2, 0.5, 0.0 | 4 of 12, at 10.0 each | 0.5 or less |
| `main-38e873f2` | `idle-4-panes` | 0.2 | 10.1, 2.0, 0.5, 0.0 | 0 of 12 | 0.5 or less |
| `v0.18.0` | `tui-30-workspaces` | 99.7 | 66.6, 20.4, 12.1, 0.0 | 31 of 90: 118.6 and 20.4 (the animated pane), 29 at 10.0 to 10.1 | 0.5 or less |
| `main-38e873f2` | `tui-30-workspaces` | 0.2 | 20.9, 11.8, 2.1, 0.0 | 2 of 90: 118.4 and 19.9 (the animated pane) | 0.5 or less |

The worker threads are told apart by their period, an inference the truncated
names cannot confirm: the follow connection is the one at 66.5 per s in
`v0.18.0` (its 15 ms poll, about 66 per s) and at 2.0 per s on `main`
(`FOLLOW_SHUTDOWN_POLL`, 500 ms, `crates/paneflow-serve/src/server.rs:26`); the
one at 10.1 per s in both builds is the pump, which waits `FRAME_WAIT`, 100 ms
(`crates/paneflow-serve/src/worker.rs:22`).

## Goals of the PRD

| Goal | Target | Before the PRD | `v0.18.0` | `main-38e873f2` | Verdict |
|---|---|---|---|---|---|
| CPU of the main thread, animated TUI, 18 workspaces | at most 6 % | 12.8 % (PRD), 12.95 % maximized on 2026-10-25 | 22.35 % with 30 workspaces | 1.69 % with 30 workspaces, 1.71 % with 33 | met, with more workspaces than the goal names |
| git processes per minute at rest, 33 workspaces, window active | at most 30 | 180 with 30 workspaces on 2026-10-20 | 31.0 with 30 workspaces, 60 s | 29.4 with 30 workspaces, 31.8 with 33, over 300 s | **missed** at 33 workspaces (31.8) |
| Sidebar renders during 30 s of terminal output without agent | at most 2 | about 750 (PRD) | no counter | 1 per 60 s, 1 per 300 s | met |
| Root renders per s, one agent thinking, shimmer visible | at most 12 | 11.13 (PRD) | not measured | not measured | not measured by this story: no `agent-thinking` run; `perf_gates` holds `desktop.thinking.root_renders_per_s` |
| Wakeups per s at rest, IPC thread | at most 1 | 105 | 99.6 | 0.2 | met |
| Wakeups per s at rest, worker follow thread | at most 1 | 70 | 66.5 | 2.0 | **missed** (2.0) |
| Wakeups per s at rest, idle session thread | at most 1 | 10.6 | 10.0 | 0.5 | met |
| RSS of the three processes | recorded (Non-Functional Requirements, Memory) | desktop 313 MiB, host 19 MiB on 2026-10-20 | see Measurements | see Measurements | recorded: desktop 12 MiB lower than `v0.18.0` with 30 workspaces, host and worker unchanged within 2 MiB |

## Missed goals

**git processes, 33 workspaces: 31.8 per min.** US-003 (shown workspace every
30 s, hidden ones every 5 min) and its `perf_gates` budget in US-005. The 300 s
window at 33 workspaces counts 159 git processes: 33 `config`, 42 `diff` and 84
other subcommands. The 33 `config` are one `git config --get-regexp` of the
filter-driver query (`query_filter_drivers`,
`src-app/src/git_command.rs:176`) per hidden workspace and per 5 min cycle,
although `cached_filter_drivers` (`src-app/src/git_command.rs:362`) keeps that
answer until a watched config file changes, and the setup wrote no config file after creating
the repositories. Without them the window would count 126 processes, 25.2 per min.
Why the cache misses for hidden workspaces is not established by this run.

Added to the PRD as US-021, "Servir les sondes des workspaces cachés depuis
le cache des filtres". Likely cause, to confirm there: the first probe follows
`git init` by less than `RACY_CONFIG_WINDOW` (2 s), so nothing is cached and the
next cycle queries once more, which would make this a cost of the first cycle
only. US-021 measures a 300 s window opened after a full cycle.

**Worker follow thread at rest: 2.0 wakeups per s.** US-016 bounded the
follower's wait at `min(keepalive due, 500 ms)` so that a shutdown ends the
stream within 600 ms, and its own criterion (at most 25 context switches in
10 s) holds at 20; the goal of at most 1 per s needs the shutdown to wake the
follower instead of the follower polling for it. The worker's pump, outside
the PRD's scope, still wakes 10 times a second on its 100 ms `FRAME_WAIT`.

Added to the PRD as US-022, "Réveiller le suivi et la pompe du worker à
l'arrêt au lieu de les faire sonder": the shutdown wakes the follower and the
pump, which then wait for their next frame or deadline, so no worker thread
wakes more than once per s at rest.

## After US-021 and US-022

The same day, a release build of `38e873f2` with US-021 and US-022 in the
working tree (label `main-38e873f2-ep006`; `paneflow --version` still reads
0.18.0) ran the same setup.

- **git processes, 33 workspaces: 25.2 per min, met.** The window stayed
  active 5 min after the last workspace opened, then the protocol measured
  300 s (label suffix `ws33-warm-w300`): 126 git processes, 42 `diff`,
  0 `config`. US-021 found no cache defect: the extra 33 queries of the first
  run were the first cycle after `git init`, which `bench/README.md` now says
  how to step over.
- **Worker threads at rest (`idle-4-panes`): 0.72, 0.5, 0.5 and 0.0 wakeups
  per s, met** (10.1, 2.0, 0.5 and 0.0 before). Under the fixture with
  33 workspaces two worker threads wake 11.6 and 10.6 times a second, the
  frames they carry (20.9 and 11.8 with 30 workspaces before).
- Desktop main thread under the fixture with 33 workspaces: 1.66 %, root
  renders 14.1 per s, `sidebar_renders` 1 per 300 s.

## Outside the goals

- At rest (`idle-4-panes`, nothing printing, cursor blinking), the desktop
  main thread still wakes about 150 times a second in `epoll_wait`, and
  another thread named `paneflow` about 100 times a second in a futex wait, in
  both builds, while the root renders 1.9 times a second. Neither is among the
  PRD's goals, and this run does not identify the second thread; both deserve a
  look for the laptop user, under Wayland as well as X11.
- Root renders under the fixture fell from 24.0 per s on `v0.18.0` to 15.7 per s
  on `main`, below the fixture's own 20 frames per s. This run does not show
  which story changed it or whether the spinner looks less smooth; a visual
  check of the animated pane settles the second question.
- `async-io` wakes 58 times a second under the fixture on `main`, 108 on
  `v0.18.0`, and 0.8 at rest, 15.9 on `v0.18.0`.

Results:
[v0.18.0 idle-4-panes](hardware-linux-x86_64-x11-idle-4-panes-v0.18.0-20261027T073014Z.json),
[main idle-4-panes](hardware-linux-x86_64-x11-idle-4-panes-main-38e873f2-20261027T073310Z.json),
[v0.18.0 tui-30-workspaces](hardware-linux-x86_64-x11-tui-30-workspaces-v0.18.0-20261027T073146Z.json),
[main tui-30-workspaces](hardware-linux-x86_64-x11-tui-30-workspaces-main-38e873f2-20261027T073438Z.json),
[main tui-30-workspaces, 300 s](hardware-linux-x86_64-x11-tui-30-workspaces-main-38e873f2-w300-20261027T073716Z.json),
[main 33 workspaces, 300 s](hardware-linux-x86_64-x11-tui-30-workspaces-main-38e873f2-ws33-w300-20261027T074315Z.json),
[after US-021 and US-022, idle-4-panes](hardware-linux-x86_64-x11-idle-4-panes-main-38e873f2-ep006-20261027T111130Z.json),
[after US-021 and US-022, 33 workspaces, 300 s after a cycle](hardware-linux-x86_64-x11-tui-30-workspaces-main-38e873f2-ep006-ws33-warm-w300-20261027T110551Z.json).
