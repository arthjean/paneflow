# GPUI 703b5ac3: keystroke latency and unfocused frame rate (2026-11-15)

US-009 and US-010 of `tasks/prd-gpui-703b5ac3-upgrade.md` check that the new
frame scheduler keeps keystroke latency and caps an unfocused window at 30
frames per second. Windows was measured on this date, Linux on 2026-11-16
(US-024), and macOS is deferred until a Mac is available.

## Windows

Machine: AMD Ryzen 7 7800X3D, NVIDIA GeForce RTX 4070 Ti SUPER, one 2560 by
1440 display at 240 Hz, Windows 11 Pro build 26300. Two debug builds, each
from its own copy outside the repository with its own `CARGO_TARGET_DIR`:

- old pin: `9f606d3aa` (GPUI `37faef81`);
- new pin: `790be19aa` plus the uncommitted EP-003 changes (GPUI `703b5ac3`,
  `frame_policy::paced_window_options` on both windows).

In both copies, and only there, the two thresholds of the latency probe in
`terminal/element/mod.rs` were lowered to 0 (`paint_ms > 1.0` and
`total_ms > 8.0` become `>= 0.0`), so that every keystroke and every terminal
paint is logged instead of only the slow ones. The committed probe still logs
above 8 ms.

Each run starts the debug build on its own `PANEFLOW_HOME`, seeded with one
workspace holding one `cmd.exe` pane, `PANEFLOW_LATENCY_PROBE=1`,
`PANEFLOW_IPC_SCRIPTING=1` and `RUST_LOG=warn`. A PowerShell driver raises the
window, checks before every key that the foreground window belongs to that
process (and aborts otherwise), and types 200 letters 150 ms apart, with an
`Esc` every 40 to clear the line. It then starts
`for /L %i in (1,1,100000000) do @echo stream line %i` through
`paneflow send cmd ... --submit` and confirms with `paneflow read` that the
pane streams. Terminal paints are counted for 10 s with the window focused,
then for 10 s after the foreground moves to the taskbar. "Latency" is the
probe's `key-handler→paint-cpu`: from the start of the key handler to the end
of the CPU paint of the next terminal frame. It does not include GPU
presentation or the display.

### Focused keystroke latency

| Pin | Run | Keystrokes | Median | p95 | Max |
|---|---|---|---|---|---|
| old `37faef81` | 1 | 203 | 4.84 ms | 6.98 ms | 7.60 ms |
| old `37faef81` | 2 | 204 | 5.14 ms | 7.22 ms | 8.61 ms |
| new `703b5ac3` | 1 | 202 | 5.59 ms | 8.06 ms | 9.00 ms |
| new `703b5ac3` | 2 | 204 | 5.72 ms | 7.67 ms | 8.62 ms |
| new `703b5ac3` | 3 | 200 | 5.39 ms | 7.28 ms | 8.70 ms |

The new pin is 0.58 ms slower at the median (5.57 against 4.99 ms, means of
the run medians) and 0.62 ms at the p95 (7.67 against 7.10 ms). **Within the
2 ms budget.** The gap is smaller than one frame at 240 Hz (4.2 ms), so no
keystroke waits for an extra frame; its cause was not profiled.

### Unfocused frame rate

| Pin | Run | Focused, paints/s | Unfocused, paints/s | Unfocused, busiest second |
|---|---|---|---|---|
| old `37faef81` | 1 | 83.0 | 70.1 | 82 |
| old `37faef81` | 2 | 80.6 | 68.4 | 76 |
| new `703b5ac3` | 3 | 77.0 | 28.3 | 30 |

Runs 1 and 2 of the new pin typed the stream command through `SendKeys`,
which the shell did not run, so they carry latency only. **An unfocused window
now presents at most 30 frames per second** (28.3 on average, never more than
30 in a whole second), where the old pin presented 68 to 70 for the same
stream. The focused rate is set by the terminal's publish gate, not by GPUI,
and stays near 80 on both pins.

User-visible change: a Paneflow window without the keyboard focus, for example
a detached pane on a second display, updates streaming output at most 30
times per second. GPUI lifts that cap during sustained high-rate input to the
window, such as a scroll. `CHANGELOG.md` records it.

## Linux (US-024, 2026-11-16)

Machine: the same hardware booted into Fedora 45, GNOME 51.1 on Wayland, the
NVIDIA 620.70.06 driver (open kernel modules), two 2560 by 1440 displays at
scale 1; GPUI selected the RTX 4070 Ti SUPER through Vulkan. Two debug builds
from their own worktrees with their own target directories, the probe
thresholds lowered to 0 in those copies only:

- old pin: `9f606d3aa` (GPUI `37faef81`);
- new pin: `2f8a3d376` (GPUI `703b5ac3`). `d78d92927` changes only the code
  editor's longest-line measure, which neither figure touches.

Each run starts the debug build on its own `PANEFLOW_HOME` seeded with one
workspace holding one `/bin/bash` pane, `PANEFLOW_LATENCY_PROBE=1`,
`PANEFLOW_IPC_SCRIPTING=1` and `RUST_LOG=warn`, and runs `cat >/dev/null` in
the pane through `paneflow send ... --submit`. The keys are typed with
`agent-desktop press`, which goes through Mutter's RemoteDesktop interface and
so reaches the window that holds the focus: 200 letters 130 ms apart, Enter
every 40 and at the end. GPUI windows expose no accessibility tree, so the
driver cannot check the focus before a key the way the Windows driver did;
instead it checks after every key that the test window's log gained a
`key-handler→paint-cpu` line, and aborts the run otherwise, so at most one key
could reach another window. No run aborted. It then ends `cat`, starts
`yes stream` the same way and counts terminal paints for 10 s focused, then
launches GNOME Calculator, which takes the focus, and counts 10 s more. The
calculator's AT-SPI `is_focused` state is read before and after that window,
and a run counts toward the unfocused rate only if it reads true both times.

### Focused keystroke latency

| Pin | Run | Keystrokes | Median | p95 | Max |
|---|---|---|---|---|---|
| old `37faef81` | 1 | 205 | 3.10 ms | 3.58 ms | 4.29 ms |
| old `37faef81` | 2 | 205 | 3.04 ms | 3.58 ms | 5.67 ms |
| old `37faef81` | 3 | 205 | 3.05 ms | 3.56 ms | 4.69 ms |
| old `37faef81` | 4 | 205 | 3.04 ms | 3.74 ms | 4.32 ms |
| old `37faef81` | 5 | 205 | 3.02 ms | 3.70 ms | 4.75 ms |
| new `703b5ac3` | 1 | 205 | 3.10 ms | 3.65 ms | 4.67 ms |
| new `703b5ac3` | 2 | 205 | 3.11 ms | 3.70 ms | 4.19 ms |
| new `703b5ac3` | 3 | 205 | 3.13 ms | 3.66 ms | 3.98 ms |
| new `703b5ac3` | 4 | 205 | 3.15 ms | 3.73 ms | 4.36 ms |
| new `703b5ac3` | 5 | 205 | 3.13 ms | 3.74 ms | 4.24 ms |
| new `703b5ac3` | 6 | 205 | 3.10 ms | 3.72 ms | 4.23 ms |

The new pin is 0.07 ms slower at the median (3.12 against 3.05 ms, means of
the run medians) and at the p95 (3.70 against 3.63 ms). **Within the 2 ms
budget.** Every keystroke reached the test window.

### Focused keystroke latency on the final commit (2026-11-18)

Re-measured by US-027 on the final branch commit, `650569c4`, against
`9f606d3aa`, with debug builds from two worktrees whose probe thresholds were
lowered to 0, as above. The driver followed the same procedure with three
differences: the runs alternated between the pins (old, new, then new, old,
and so on), the pane ran the login shell (`zsh`) instead of the seeded
`/bin/bash` (seen on the new pin, not checked on the old one), and the driver
unset the `PANEFLOW_*`
variables of the pane it was started from, without which the test instance
refused `paneflow send` as coming from an unknown session. Before each run the
driver waited 20 s and required 15 s without input, since the synthetic keys
reset GNOME's idle time; no run was skipped and every key reached the test
window.

| Pin | Run | Keystrokes | Median | p95 | Max |
|---|---|---|---|---|---|
| old `9f606d3aa` | 1 | 201 | 3.32 ms | 3.78 ms | 4.60 ms |
| old `9f606d3aa` | 2 | 201 | 3.21 ms | 3.90 ms | 4.57 ms |
| old `9f606d3aa` | 3 | 201 | 3.26 ms | 3.86 ms | 4.44 ms |
| old `9f606d3aa` | 4 | 201 | 2.88 ms | 3.52 ms | 4.05 ms |
| old `9f606d3aa` | 5 | 201 | 3.37 ms | 3.67 ms | 4.10 ms |
| new `650569c4` | 1 | 201 | 3.45 ms | 3.80 ms | 4.64 ms |
| new `650569c4` | 2 | 201 | 3.35 ms | 3.87 ms | 4.57 ms |
| new `650569c4` | 3 | 201 | 3.11 ms | 3.81 ms | 6.14 ms |
| new `650569c4` | 4 | 201 | 3.04 ms | 3.54 ms | 4.13 ms |
| new `650569c4` | 5 | 201 | 3.51 ms | 3.72 ms | 4.14 ms |

The new pin is 0.08 ms slower at the median (3.29 against 3.21 ms, means of
the run medians) and equal at the p95 (3.75 ms). **Within the 2 ms budget.**

### Unfocused frame rate

| Pin | Run | Focused, paints/s | Unfocused, paints/s | Unfocused, busiest second |
|---|---|---|---|---|
| old `37faef81` | 1 | 100.5 | 92.3 | 100 |
| old `37faef81` | 3 | 98.6 | 91.5 | 99 |
| old `37faef81` | 5 | 99.3 | 90.8 | 95 |
| new `703b5ac3` | 5 | 99.8 | 25.4 | 26 |
| new `703b5ac3` | 6 | 99.8 | 25.7 | 27 |

**An unfocused window presents at most 30 frames per second on Linux too**
(25.4 and 25.7 on average, never more than 27 in a whole second), where the
old pin presented 91 to 92. The other runs carry latency only: the calculator
stayed open from an earlier run, so the new launch did not take the focus and
those runs read `is_focused` false.

Not measured on Linux: KDE Plasma, X11 and Mesa. Only GNOME on Wayland with the
NVIDIA driver was run.

## macOS: deferred until a Mac is available

Not measured. No Mac is available before 2027, so US-010 is cancelled (PRD
1.2): macOS is validated by CI only and the latency risk of
zed-industries/zed#66954 is accepted on trust in the upstream code. The
procedure below is kept for the first hardware run. On a Mac with
macOS 14 or later and a 120 Hz display (ProMotion, refresh set to
ProMotion):

1. Build both pins as above on the Mac (the archives come from
   `scripts/fetch-libghostty.sh`).
2. Run steps 2 to 4 of the Linux procedure, typing with
   `osascript -e 'tell application "System Events" to keystroke "a"'` in a
   loop at 150 ms, or by hand.
3. The probe stops at the end of the CPU paint, and upstream
   zed-industries/zed#66954 reports the regression after it, between the
   drawable and the display (`CAMetalDisplayLink`, `0373366f43`). So also film
   the screen at 240 fps and count frames from the key press to the glyph, 20
   keystrokes per pin, as the key-to-present figure.
4. Trace startup on both pins: `scripts/bench-startup.sh` from each copy, or
   `PANEFLOW_STARTUP_TRACE=<file>` on the release build.
5. Pass: key-to-present on the new pin within 4.2 ms of the old one. Otherwise
   the macOS release of the bump is held, the finding is recorded here with a
   link to zed-industries/zed#66954, and the options are to wait for an
   upstream fix and re-pin, or to carry a vendored patch.
