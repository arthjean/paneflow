# libghostty-vt re-pin to Ghostty v1.4.0: Linux terminal suite (2026-10-29)

Machine: Arthur's Fedora 44, AMD Ryzen 7 7800X3D, release profile, CPU share
0.99. The run comes from a clean tracked worktree at `9f939076`, which carries
the atomic re-pin (`b2e5c759`) unchanged: no Rust file, manifest or lockfile
differs between the two commits.

| Suite | GPUI bump (`92a5c29d`) | libghostty v1.4.0 (`9f939076`) |
|---|---|---|
| terminal | [20261028T112604Z-92a5c29d9902.json](20261028T112604Z-92a5c29d9902.json) | [20261029T093443Z-9f9390764146.json](20261029T093443Z-9f9390764146.json) |

The `92a5c29d` result is the committed `terminal` baseline of `linux-x86_64`,
so `scripts/bench-terminal.sh` compared against it directly. No baseline was
refreshed.

## Allocation columns

Every `alloc_bytes_per_iter` and `allocs_per_iter` is identical on both
commits, and `gate_trickle_publishes` stays at 250 frames per 1000 chunks. The
engine change moved no allocation, so `terminal-alloc.json` and the
`persistent` baseline stay as committed.

## Timings

Most timings stay within 4 %. The moves above that:

| Metric | `92a5c29d` | `9f939076` | Second run (`gates` profile) | Reading |
|---|---|---|---|---|
| `service_spaces_8192` | 38.1 us | 33.2 us (-12.8 %) | 38.1 us (+0.1 %) | noise |
| `service_text_220x60` | 19.0 us | 17.1 us (-10.0 %) | 19.6 us (+3.2 %) | noise |
| `service_spaces_220x60` | 58.8 us | 53.5 us (-9.0 %) | 61.5 us (+4.5 %) | noise |
| `render_hold_capture_200x60` | 4.2 us | 3.8 us (-9.4 %) | 4.0 us (-5.3 %) | faster |
| `line_text_at_220x60` | 360 ns | 400 ns (+11.1 %) | 350 ns (-2.8 %) | noise |
| `base_font_resolve` | 30 ns | 40 ns (+33.3 %) | 40 ns (+33.3 %) | one timer tick |

The service-output parser and the row text extraction are Paneflow code the
re-pin does not touch, and the second run puts them back within 5 % of the
baseline. `base_font_resolve` resolves a GPUI `Font` and never reaches the
engine: its median moved by one 10 ns tick of the timer, while its mean went
from 57.1 to 59.1 ns and its p95 stayed at 40 ns.
`render_hold_capture_200x60` captures a frame inside libghostty and is faster
on both runs. It matches the faster parser in the 1.4.0 release notes, but
this suite does not isolate the cause.

`pipeline_corpus_mib_s` reads 1.5 MiB/s on both commits (+1.1 %): the suite
interleaves a snapshot and a conversion with each fed batch, so the parse
speedup does not show in it.

## Golden digests

`cargo test -p paneflow-terminal-ghostty --features native --test snapshot_golden`
passes with both committed digests unchanged.

## Performance gates

`scripts/perf-gates.sh` on the same worktree: the terminal and editor suites,
the hook burst and 96 of the 107 budgets pass. The 11 others are desktop
budgets reported missing because Xvfb is not installed on this machine. The
`Performance gates (Linux x86_64)` CI job, which provides Xvfb, is green on
`9f939076` (run 40848142393).
