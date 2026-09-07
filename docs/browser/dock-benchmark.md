# Linux dock benchmark

From the repository root, with matching release app and host binaries:

```sh
bun scripts/browser-qualification/dock-benchmark.mjs record baseline
```

Close other PaneFlow instances. Use one visible Browser tab, the same display,
scale, refresh rate, initial dock width and resize endpoints for every run.
The runner launches the app with opt-in `PANEFLOW_BROWSER_BENCH` recording,
serves a deterministic local page and prompts for loading, steady animation,
manual scrolling and resizing. Each steady/scroll/resize phase lasts 20 seconds
following 5 seconds of preparation. Close PaneFlow normally at the end.

The fixture includes a continuous small animation to keep producing frames.
It excludes network variability. The local fixture uses a fresh port per run,
so loading includes a new-origin navigation and should not be interpreted as a
warm-cache website benchmark. Repeat three times per version. Describe the
same display and dock geometry identically in the initial prompt.

Outputs under `bench/browser/dock/` include binary and fixture hashes, runtime
manifest hash, Git HEAD and tracked diff hash, phase markers, events and summary.
Binary hashes identify the executed code, including untracked source changes.
Events exclude URLs, titles, keyboard text and page content. They contain opaque
page IDs and timing/geometry information. Recording uses a bounded worker queue;
any reported dropped events invalidate comparison. A forced process exit can
lose the final queued events. Do not kill the app during a measured phase.

```sh
bun scripts/browser-qualification/dock-benchmark.mjs report <recording-directory>
bun scripts/browser-qualification/dock-benchmark.mjs compare <baseline-directory> <candidate-directory>
```

Comparison rejects different declared displays, platforms, fixture hashes or
frame-rate settings. Deltas are descriptive, not automatic performance gates.
Manual input variation and thermal/background load remain uncontrolled.

Metrics are milliseconds with count, p50, p95, p99 and maximum:

- `host_prepare_ms`: host callback timestamp to frame-ready timestamp, including
  the synchronous GPU copy. This is not an isolated GPU execution measurement.
- `ready_to_receive_ms`: ready timestamp to main-thread receipt in GPUI, including
  IPC and scheduling. Both Linux clocks use CLOCK_MONOTONIC.
- `arrival_gap_ms`: interval between received frames of the same page. This is
  neither physical presentation FPS nor a missed-frame percentage.
- `pool_import_ms`: import of a new pool. In the baseline this runs on GPUI;
  in the optimized build it runs on a worker. GPU initialization and later
  acknowledgement are outside this interval; `pool_initialize` raw events
  record initialization separately.
- `load_event_after_submit_ms`: submission to the native loaded event in the
  same phase. This does not prove the first image is physically displayed.

There is no input-to-photon measurement, compositor presentation feedback,
CPU/GPU utilization measurement or CEF standalone comparison in this collector.
No NFR certification is inferred. Use the existing qualification trace tooling
for those subsequent investigations.

Phase markers use Python 3 CLOCK_MONOTONIC directly, as the Rust collector does.
Bun process.hrtime has a different epoch in this environment and must not be
compared with host timestamps. Recordings without explicit CLOCK_MONOTONIC
phase markers are rejected; their original phases cannot be reconstructed
without a synchronization sample. Python is invoked only at phase boundaries,
never for individual frames. A phase with no received frames aborts collection.

The resize path retains the latest requested geometry and allows one transition
at a time. It sends the next size when a matching frame is received and the old
pool has retired, without waiting for pointer inactivity. The previous image
remains displayed during transitions. Keep 60 Hz for comparison.

`resize_ready_ms` measures accepted size submission to intake of a frame with
matching physical dimensions. It does not measure physical presentation or the
delay since the user's latest pointer movement. Raw `resize_host`,
`resize_capture` and `resize_pool` events carry host CLOCK_MONOTONIC timestamps
to separate command receipt, the first matching capture and pool publication.
Old recordings have no samples for this new metric; null is not zero.

The dock now disables continuous Chromium qualification tracing; the prototype
retains it. This is part of the code change under comparison. GPU pool creation
initializes all three images in one submission. Consumer import/initialization
runs in the background; reservations still count toward the two-pool limit and
late completions cannot reactivate obsolete documents or hosts.

For a diagnostic Chromium trace (not a performance comparison):

```sh
bun scripts/browser-qualification/dock-benchmark.mjs trace-resize resize-trace
```

This records startup and one 20-second resize phase using the local fixture.
Chromium categories cover CEF, layout, renderer scheduling, composition and
capture. After resizing, close the local Browser tab while keeping PaneFlow
open. The script waits up to 90 seconds for valid nonempty JSON from the hosts
participating in the resize, then asks to close PaneFlow. Artifacts are stored
in the recording's chromium directory and trace-receipt.json. Trace collection
adds overhead and can contain page URLs; use only the fixture for this run.

Normal recordings include capture_frame_rate events and a capture_rate_changes
list in summary.json. Each entry includes the page, host monotonic timestamp,
requested fps and reason (initial, resize, restore). These report CEF cadence
configuration, not measured screen refresh or presentation. Use frame arrivals
to assess the observed cadence. No Chromium trace is needed for this check.
