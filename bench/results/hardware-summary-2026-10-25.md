# Real hardware protocol, `tui-30-workspaces` with 18 workspaces, 2026-10-25

The EP-002 measurements of `tasks/prd-resource-efficiency.md`: `main` at
`d81a22d4`, before EP-002, against the same commit with US-007 alone (the two
paint layers of `src-app/src/terminal/element/mod.rs`) and with the whole of
EP-002 (US-006 to US-008, uncommitted at the time of the run). All three are
release builds of the same tree otherwise. Fedora 44 (kernel 7.2.11), GNOME
Shell 50.6, 2560x1440 at 144 Hz. Ryzen 7 7800X3D, GeForce RTX 4070 Ti SUPER
(driver 615.78.08) driving the display, Radeon 610M idle at 0 % in every run.

Each build ran alone from a terminal outside Paneflow, with a seeded empty
home. Eighteen workspaces were opened with `paneflow new`, one per committed
git repository holding one modified file, the number the PRD's reference
values use (12.8 % of a core, 5.0 ms per frame). The last one was selected and
ran `sh bench/fixtures/tui-agent.sh 600` from the EP-002 tree, so the three
builds replay the same fixture, the one whose title keeps a steady text behind
its braille spinner. The window ran under X11 through XWayland
(`env -u WAYLAND_DISPLAY`) as on 2026-10-20, and `xdotool` checked once per
second that it kept the focus: 61 of 61 samples in every window. Each window
lasted 60 s and started about 12 s after the fixture. Frame time is not
measured: no MangoHud log.

The frame cost is the PRD's metric: desktop main thread CPU over
`root_renders`. Two window sizes were measured: the maximized window of the
protocol (2560x1400), and the default window (1420x920), in which `main`
reproduces the PRD's reference values. `main` and US-007 alone were each
launched twice per size, five windows in all; EP-002 once per size, two
windows.

## Maximized window, 2560x1400

CPU is percent of one core, the mean over the windows. The desktop main thread
is the `paneflow` thread. GPU is the NVIDIA device load (whole device,
compositor included).

| Build | Windows | Main thread (range) | Frame cost | Desktop CPU | Host CPU | Worker CPU | Root renders per s | `sidebar_renders` per window | Desktop RSS | GPU p50 / p95 |
|---|---|---|---|---|---|---|---|---|---|---|
| `main-d81a22d4` | 5 | 12.95 % (12.85 to 13.12) | 5.27 ms | 13.56 % | 1.46 % | 0.15 % | 24.6 | no counter | 288 MiB | 30 / 46 % |
| `us007-d81a22d4` | 5 | 11.52 % (11.06 to 11.83) | 4.67 ms | 12.13 % | 1.50 % | 0.15 % | 24.7 | no counter | 286 MiB | 29 / 47 % |
| `ep002-d81a22d4` | 2 | 3.82 % (3.67 to 3.97) | 1.56 ms | 4.43 % | 1.45 % | 0.14 % | 24.5 | 0, 0 | 286 MiB | 28 / 45 % |

## Default window, 1420x920

| Build | Windows | Main thread (range) | Frame cost | Desktop CPU | Host CPU | Worker CPU | Root renders per s | `sidebar_renders` per window | Desktop RSS | GPU p50 / p95 |
|---|---|---|---|---|---|---|---|---|---|---|
| `main-d81a22d4` | 5 | 12.18 % (11.96 to 12.96) | 4.94 ms | 12.75 % | 1.45 % | 0.14 % | 24.6 | no counter | 281 MiB | 9 / 15 % |
| `us007-d81a22d4` | 5 | 11.35 % (10.82 to 12.46) | 4.59 ms | 11.93 % | 1.52 % | 0.15 % | 24.7 | no counter | 281 MiB | 10 / 16 % |
| `ep002-d81a22d4` | 2 | 3.50 % (3.37 to 3.63) | 1.43 ms | 4.08 % | 1.48 % | 0.14 % | 24.5 | 1, 0 | 282 MiB | 8 / 14 % |

## Verdicts

| Criterion | Target | Maximized | Default window | Verdict |
|---|---|---|---|---|
| US-007 as written in PRD 1.0, main thread against `main` | at least 15 % lower | 11.1 % lower | 6.8 % lower | not met, criterion amended in PRD 1.1 |
| US-007 as amended in PRD 1.1, main thread against `main` | at least 10 % lower, maximized | 11.1 % lower | not gated | met |
| US-007 as amended in PRD 1.1, `BoundsTree::insert` | under 1 % of the main thread, no call from the terminal paint | 0.46 %, none (profile below) | not profiled | met |
| US-008, frame cost against US-007 alone | at least 1.5 ms lower | 3.11 ms lower | 3.16 ms lower | met |
| EP-002 Definition of Done, main thread | at most 6 % | 3.82 % | 3.50 % | met |
| EP-002 Definition of Done, `sidebar_renders` | at most 2 | 0 per 60 s | at most 1 per 60 s | met |

## Reading

EP-002 as a whole cuts the main thread by about 70 % (12.95 % to 3.82 %
maximized) at an unchanged render rate, and the frame cost from 5.27 ms to
1.56 ms, under the PRD's 2.5 ms target. The split between the two stories
differs from the PRD's estimates: the two layers alone save 0.60 ms per frame
maximized and 0.35 ms in the default window, against 1.1 ms in the PRD's A/B
(12.8 % to 9.9 %), while the cached sidebar saves about 3.1 ms, against about
2 ms expected. This run does not show where the gap with the A/B comes from.
The first two maximized windows of US-007 alone read 15.1 % lower, the five
together 11.1 %: two windows were not enough to decide a criterion this close.

## Profile, 2026-10-26

`main` and US-007 alone, rebuilt from the same tree in release with
`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`,
`CARGO_PROFILE_RELEASE_STRIP=false` and `-C force-frame-pointers=yes`, ran the
same scenario maximized (18 workspaces, fixture running, focus 40 of 40). The
desktop main thread was sampled for 40 s with
`perf record --user-callchains -e cycles:u -F 1999 --call-graph fp -t <pid>`
(this kernel refuses kernel callchains to an unprivileged user). Percentages
are inclusive, of the main thread's samples.

| | `main` | US-007 alone |
|---|---|---|
| Main thread CPU during the profile | 12.97 % | 11.27 % |
| `BoundsTree::insert` | 12.46 % | 0.46 % |
| of which under `paneflow::terminal::element::paint::text::paint_placed_glyphs` | 11.91 % | 0 % |
| `hosted_sessions::session_belongs_to` | not split out | about 24 % |

US-007 removes every call to `BoundsTree::insert` from the terminal paint; the
0.46 % left comes from `AnimatedHover`, the title bar and the cursor. On `main`
the function weighs 12.5 % of the main thread, about 0.66 ms of the 5.27 ms
frame, against the 0.60 ms saved in the windows above: even removed entirely it
could not reach the 15 % of PRD 1.0, hence the amendment. The 22.5 % the PRD
quoted came from an earlier flat profile in which the rest of the frame weighed
less.

Outside US-007's scope, the profile also shows `session_belongs_to` comparing
paths component by component (`std::path::Components::next`, about 19 % self)
during the sidebar render. US-008's cached sidebar skips it in steady state;
every sidebar invalidation still pays it.

Results, maximized:
[main 1](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18max-20261025T110107Z.json),
[main 2](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18max-20261025T110209Z.json),
[main 3](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18max-r2-20261025T111212Z.json),
[main 4](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18max-r2-20261025T111313Z.json),
[main 5](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18max-r2-20261025T111414Z.json),
[US-007 1](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18max-20261025T110333Z.json),
[US-007 2](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18max-20261025T110434Z.json),
[US-007 3](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18max-r2-20261025T110844Z.json),
[US-007 4](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18max-r2-20261025T110945Z.json),
[US-007 5](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18max-r2-20261025T111047Z.json),
[EP-002 1](hardware-linux-x86_64-x11-tui-30-workspaces-ep002-d81a22d4-ws18max-20261025T110558Z.json),
[EP-002 2](hardware-linux-x86_64-x11-tui-30-workspaces-ep002-d81a22d4-ws18max-20261025T110700Z.json).

Results, default window:
[main 1](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18-20261025T105340Z.json),
[main 2](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18-20261025T105441Z.json),
[main 3](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18-r2-20261025T111901Z.json),
[main 4](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18-r2-20261025T112002Z.json),
[main 5](hardware-linux-x86_64-x11-tui-30-workspaces-main-d81a22d4-ws18-r2-20261025T112103Z.json),
[US-007 1](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18-20261025T105610Z.json),
[US-007 2](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18-20261025T105711Z.json),
[US-007 3](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18-r2-20261025T111536Z.json),
[US-007 4](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18-r2-20261025T111638Z.json),
[US-007 5](hardware-linux-x86_64-x11-tui-30-workspaces-us007-d81a22d4-ws18-r2-20261025T111739Z.json),
[EP-002 1](hardware-linux-x86_64-x11-tui-30-workspaces-ep002-d81a22d4-ws18-20261025T105833Z.json),
[EP-002 2](hardware-linux-x86_64-x11-tui-30-workspaces-ep002-d81a22d4-ws18-20261025T105935Z.json).
