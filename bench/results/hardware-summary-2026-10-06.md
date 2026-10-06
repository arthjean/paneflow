# Real hardware protocol, first execution, 2026-10-06

v0.17.5 (the published `/usr/bin/paneflow`) against `main` at `762ac880`
(release build), Fedora 44, GNOME Shell 50.5 under Wayland, 2560x1440 at
144 Hz, window seeded at 1920x1080. Ryzen 7 7800X3D, GeForce RTX 4070 Ti
SUPER (driver 615.71.09) driving the display, Radeon 610M idle at 0 % in every
run. Each build ran alone, from Ghostty, with a seeded empty home
(`~/.paneflow-hw-<label>`): one workspace of 2 x 2 idle shells in this
repository. In `agent-thinking`, the first pane ran
`claude -p "<6000-word essay, no tools>" >/dev/null`; the sidebar reported
`thinking` at the start and at the end of the window in both builds.
Frame time is not measured: MangoHud is not installed. X11 and Windows were
not run, as PRD 1.4 allows.

Results:
[v0.17.5 idle](hardware-linux-x86_64-wayland-idle-4-panes-v0.17.5-unix1791290597.json),
[v0.17.5 thinking](hardware-linux-x86_64-wayland-agent-thinking-v0.17.5-unix1791290678.json),
[main idle](hardware-linux-x86_64-wayland-idle-4-panes-main-762ac880-unix1791290773.json),
[main thinking](hardware-linux-x86_64-wayland-agent-thinking-main-762ac880-unix1791290853.json).

## Measurements over 60 s

CPU is percent of one core, GPU is the NVIDIA device load (whole device,
compositor included).

| State | Build | Desktop CPU | Host CPU | Worker CPU | Desktop RSS | Host RSS | GPU p50 / p95 | Root renders |
|---|---|---|---|---|---|---|---|---|
| `idle-4-panes` | v0.17.5 | 0.99 % | 0.01 % | 0.05 % | 219 MiB | 9 MiB | 5 / 12 % | not measured (no `system.counters`) |
| `idle-4-panes` | main | 0.70 % | 0.03 % | 0.04 % | 220 MiB | 17 MiB | 9 / 17 % | 113 |
| `agent-thinking` | v0.17.5 | 25.04 % | 0.01 % | 0.05 % | 219 MiB | 9 MiB | 39 / 39 % | not measured (no `system.counters`) |
| `agent-thinking` | main | 2.79 % | 0.01 % | 0.04 % | 221 MiB | 17 MiB | 12 / 20 % | 780 |

## Verdicts on the candidate regressions

| Candidate | Verdict | Evidence |
|---|---|---|
| Tab badges rebuilt every frame, driven by the sidebar spinner (v0.17.1) | Confirmed in v0.17.5, fixed on `main` | `agent-thinking`: desktop CPU 25.04 % to 2.79 % (9x), GPU p50 39 % to 12 % |
| Worker snapshot every 2 s (v0.17.0) | Consistent with the fix, small at this scale | `idle-4-panes`: desktop CPU 0.99 % to 0.70 %; the decisive proof stays the US-008 counters |
| Extra git config process per probe (v0.17.5) | Fixed on `main` | 279 git processes for 93 diff-stat probes: 3 per probe, down from 5 |
| Full process listing, viewport scan, per-session broadcasts | Not decided here | No pane prints in these two states; host CPU stays at 0.01 to 0.03 % in both builds; decided by the EP-002 counters |
| Startup sleep on a stale socket | Not this protocol | `bench/baselines/linux-x86_64/startup.json`: `stale_socket_step_ipc_server_started` 0.2 ms |

The idle GPU p50 (5 % to 9 %) is device wide and moves with the compositor and
other clients; it is not decidable at this resolution. The host's resident
memory grew from 9 to 17 MiB between the builds; it is recorded, not judged.

## New findings

1. **Git probes run about 1.6 times per second in an idle repository.** On
   `main`, the desktop spawned 279 git processes per minute (93 diff-stat
   probes) in both states, and a second check with a single pane in the same
   repository counted 47 probes in 30 s: the cadence follows the repository, not
   the number of panes, while `.git/index` did not change. `perf_gates`
   measures one 30 s poll on a repository that does not change, so this
   trigger escapes it. Per Risk 8 of the PRD it is a new cause, to become a
   story.
2. **Root renders on a focused real window exceed the PRD bounds that the gates
   hold under Xvfb.** `main` drew 113 root renders in 60 s at rest (1.9 per
   second, bound: 3 per 30 s) and 780 while thinking (13.0 per second, bound:
   12). The extra 1.9 per second matches a blinking cursor in the focused pane,
   which the unfocused Xvfb window of the gates does not have; this is an
   assumption, not verified.
