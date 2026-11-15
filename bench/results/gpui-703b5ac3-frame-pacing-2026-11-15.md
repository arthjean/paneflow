# GPUI 703b5ac3: keystroke latency and unfocused frame rate (2026-11-15)

US-009 and US-010 of `tasks/prd-gpui-703b5ac3-upgrade.md` check that the new
frame scheduler keeps keystroke latency and caps an unfocused window at 30
frames per second. Windows was measured on this date; Linux is still to be
measured, and macOS is deferred until a Mac is available.

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

## Linux: to be measured

Not measured: this host has no Linux desktop session (WSLg is an RDP-backed
compositor, not one). On Fedora 44 (GNOME, Wayland), same hardware:

1. Build both copies as above (`git archive 9f606d3aa` and the branch commit),
   lower the two probe thresholds in each, `cargo build -p paneflow-app -p
   paneflow-host --locked` with a private `CARGO_TARGET_DIR`.
2. Seed a `PANEFLOW_HOME` per pin with `session.json` (one workspace, one
   terminal pane), `paneflow.json` (`{"default_shell": "/bin/bash"}`),
   `window-state.json` and `telemetry_id`.
3. Launch with `PANEFLOW_LATENCY_PROBE=1 PANEFLOW_IPC_SCRIPTING=1
   RUST_LOG=warn`, stderr to a file, keep the window focused and type 200
   characters at about 7 per second (by hand or with `ydotool type --delay 150`).
4. Start `yes stream` through `paneflow send <pane> 'yes stream' --submit`,
   count `[latency] paint:` lines for 10 s focused, then 10 s with another
   window focused.
5. Pass: medians within 2 ms of each other, unfocused paints at most 30 in any
   whole second on the new pin.

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
