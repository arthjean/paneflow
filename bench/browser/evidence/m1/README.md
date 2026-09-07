# Local Linux reference receipt

EP-001 is certified DONE against its 25 acceptance criteria in the
[review report](validation/review.json). These are observed local M1 references,
not a product GO or distribution qualification. Each row has five repetitions, 10 s warmup and 60 s observation.
The [machine-readable inventory](index.json) binds captures and inspections by
SHA-256. The native archives include raw timestamps, calibration, process
identities, release build receipts and source inventories.

The condition is Fedora 44, GNOME 50.4 Wayland, RTX 4070 Ti SUPER with NVIDIA
610.57.04, 1920x1080 physical viewport and scale 1. Both enabled monitors use
59.951 Hz for the nominal 60 Hz condition. Their original 143.973/144.006 Hz
modes are restored after qualification. Window placement is controlled by the
Wayland compositor; both outputs have the same fixed refresh condition.

| Reference | Measured samples | Median repetition p95 (ms) | Maximum repetition p95 (ms) |
|---|---:|---:|---:|
| [a-60](a-60/inspection.json) | 1,500 | 46.559 | 47.690 |
| [prefeature-60](prefeature-60/inspection.json) | 1,500 | 46.299 | 47.336 |
| [b-empty-60](b-empty-60/inspection.json) | 0 | Not applicable | Not applicable |
| [b-scroll-60](b-scroll-60/inspection.json) | 17,961 | 55.740 | 58.469 |
| [b-animation-60](b-animation-60/inspection.json) | 17,965 | 49.785 | 49.947 |
| [b-webgl-60](b-webgl-60/inspection.json) | 17,968 | 49.796 | 49.913 |
| [b-network-60](b-network-60/inspection.json) | 1,195 | 23.042 | 23.452 |
| [b-combined-60](b-combined-60/inspection.json) | 17,963 | 58.289 | 58.947 |

A and PRE_FEATURE measure normal GPUI terminal key-handler entry to native
presentation of the matching real-PTY echo marker. IPC schedule registration
is excluded. Both contain 1,500 post-warmup input samples and 29,750 verified
replay events. The [terminal probe and hooks are byte-identical](identical-terminal-instrumentation.json)
in the candidate and isolated pre-feature checkout. The reserved first marker
row, pane geometry and replay bytes were frozen before accepted capture.

B measures Chromium DrawAndSwap start to the exactly correlated native Wayland
presentation. It does not use a CPU callback as the presentation endpoint.
CEF renderer/GPU roles are established through Chromium process metadata and
joined to live sandbox evidence; every repetition observes the closing
lifecycle. Empty has no post-warmup frames and therefore no latency percentile.
Unfinished/discarded startup or drain events remain in raw diagnostics; events
intersecting the measured window cannot be silently discarded.

The [paired PRE_FEATURE/A comparison](pre-feature-vs-a.json) has a p95 delta of
+0.234 ms median and +1.576 ms worst repetition. Both are retained. This receipt
does not infer a performance GO from one statistic. The
[CPU terminal/editor witnesses](cpu/index.json) and each terminal capture's
thread-CPU records remain separate from presentation latency. Existing CPU
baselines were not updated.

The rejected GC-affected replays and the erroneous 144 Hz A condition are
retained as diagnostics. Python cyclic collection was measured at the same
replay events as the delivery overruns, then moved before the timed window;
its original state is restored on success and failure. The 2 ms delivery limit
was never relaxed. The earlier B empty capture with a 144 Hz secondary monitor
is a separate historical condition and is not mixed into this set.

The integrated configuration C, 120 Hz, X11, other distributions/compositors,
Mesa, native aarch64, macOS and Windows remain explicitly unexecuted. They do
not replace or invalidate the completed local reference work.
