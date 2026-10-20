# Real hardware protocol, `tui-30-workspaces`, 2026-10-20

The reference values of `tasks/prd-resource-efficiency.md` US-002: `main` at
`3a4ca9e1`, before any fix of that PRD, against the same commit with EP-001
applied (US-001 to US-005, plus the 90 ms shimmer of US-009, uncommitted at
the time of the run). Both are release builds of the same tree otherwise.
Fedora 44 (kernel 7.2.11), GNOME Shell 50.6, 2560x1440 at 144 Hz. Ryzen 7
7800X3D, GeForce RTX 4070 Ti SUPER (driver 615.78.08) driving the display,
Radeon 610M idle at 0 % in both runs.

Each build ran alone from a terminal outside Paneflow, with a seeded empty
home (`~/.paneflow-hw-<label>`, `session.json` with no workspace). Thirty
workspaces were opened with `paneflow new`, one per committed git repository
holding one modified file, then the last one was selected and ran
`sh bench/fixtures/tui-agent.sh 600`; its title read `tui fixture 7xx` at the
end of both windows. The window ran under X11 through XWayland
(`env -u WAYLAND_DISPLAY`), because GNOME on Wayland lets no script focus or
maximize a window: `xdotool` maximized it to 2560x1400, gave it the focus, and
checked once per second during the 60 s window that it kept the focus (61 of
61 samples in both runs). Frame time is not measured: no MangoHud log.

Results:
[main-3a4ca9e1](hardware-linux-x86_64-x11-tui-30-workspaces-main-3a4ca9e1-20261020T061625Z.json),
[ep001-3a4ca9e1-dirty](hardware-linux-x86_64-x11-tui-30-workspaces-ep001-3a4ca9e1-dirty-20261020T061806Z.json).

## Measurements over 60 s

CPU is percent of one core. The desktop main thread is the `paneflow` thread.
GPU is the NVIDIA device load (whole device, compositor included).

| Build | Desktop CPU | Main thread | Host CPU | Worker CPU | Desktop RSS | Host RSS | Root renders per s | git processes per min | GPU p50 / p95 |
|---|---|---|---|---|---|---|---|---|---|
| `main-3a4ca9e1` | 22.50 % | 21.77 % | 1.27 % | 0.16 % | 313 MiB | 19 MiB | 24.9 | 180 | 30 / 42 % |
| `ep001-3a4ca9e1-dirty` | 22.86 % | 22.16 % | 1.24 % | 0.16 % | 314 MiB | 18 MiB | 24.8 | 24 | 31 / 43 % |

## Reading

The scenario reproduces the case the PRD sets out to fix: one animated agent
interface among 30 workspaces costs the desktop main thread about 22 % of a
core at 25 root renders per second. The PRD measured 12.8 % with 18
workspaces under Wayland; the backend differs, so this run does not by itself
show how the frame cost grows with the number of workspaces, which EP-002
measures with its own counters.

EP-001 divides the git processes by 7.5 (180 to 24 per minute, 3 per probe:
the shown workspace every 30 s and a tenth of the 29 others per poll) and
leaves the desktop's own CPU unchanged, as expected: the git probes run in
child processes that the desktop CPU above does not include. Root renders,
RSS, host and worker CPU are within run-to-run noise.
