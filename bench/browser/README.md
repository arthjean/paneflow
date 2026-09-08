# Browser measurement protocol M1

This directory contains fixtures, replay and qualification tooling. The CEF
candidate and separate native witness are described in the
[runtime contract](../../docs/browser/runtime.md). Local native presentation observations are kept separately from product and
distribution qualification. The [original GPUI investigation](../../docs/browser/qualification.md)
records the pre-implementation baseline; it is not a current test result.

## Run the fixtures

From the repository root, on Linux, macOS or Windows with Bun installed:

```sh
bun scripts/browser-qualification.mjs serve
bun scripts/browser-qualification.mjs manifest
```

The server prints its loopback origin, ephemeral port and fixture SHA-256.
It never launches a browser. An optional numeric port fixes the endpoint for a
native capture session. Stop with Ctrl+C. Only GET and HEAD on the fixture
allowlist are served; files elsewhere in the repository are not exposed.

| Route | Controlled workload |
|---|---|
| `/empty` | Empty document without JavaScript, animation or network activity |
| `/scroll` | 1,000 fixed-height text/image rows, identical local SVG, 240 CSS pixels/s ping-pong scroll |
| `/animation` | 64x64 CSS square, 576-pixel translation, 2 s per direction |
| `/webgl` | 640x360 WebGL2 full-screen triangle, time-based fragment color, one draw per animation frame |
| `/network` | Fixed 1,024-byte local payload every 250 ms; one outstanding request, failure after 200 ms |
| `/combined` | Scroll, CSS animation, WebGL and network together |
| `/ime` | Labeled input with composition/input event observation |
| `/popup` | User-initiated local 640x480 popup containing the input fixture |
| `/download` | User-initiated download of the fixed 1,024-byte payload |
| `/serviceworker` | Registers `/sw.js` at scope `/`, waits for control, then checks that `/service-worker-probe` is answered by the worker |
| `/websocket` | Eight masked text round trips against the loopback echo endpoint published by `/websocket.json` |
| `/iframe` | Same-origin `/frame` read through `contentDocument`, next to `/embedded` refusing to be framed by `X-Frame-Options: DENY` |
| `/auth` | HTTP Basic realm on `/private`: a credential-less fetch must be refused with 401 before the native prompt is answered |

The WebSocket echo listens on its own loopback port because the qualification
CLI runs under Bun, whose `node:http` upgrade socket does not write back. The
port is published by `/websocket.json` and allowed by each page's
`connect-src`. The `/auth` credential exists only in
`scripts/browser-qualification/fixtures.mjs`; no external token is imported.

Fixtures use no third-party services, fonts or randomness. Timed work is driven
by elapsed time so a dropped frame does not reduce the intended motion speed.
The empty fixture remains idle. WebGL/network failures set
`document.documentElement.dataset.fixtureState` to `failed`; captures must
retain that failure and be rejected. This flag and requestAnimationFrame are
diagnostics only, never evidence that a frame reached presentation. IME, popup,
download, the service-worker probe, the WebSocket round trips, frame reachability
and the authentication prompt require native observation; HTTP tests exercise the
server side only and do not qualify their UI.

Adding or removing a fixture changes the bundle SHA-256 that every archived M1
capture carries, so a capture stays bound to the corpus it ran against. The
current corpus is `schema_version: 2`.

## Freeze the experiment before comparing

Use three paired configurations and a pre-feature control:

| Configuration | Exact content |
|---|---|
| PRE_FEATURE | Paneflow at `fbfefd250a3f3c8d9968a23f8c358712859bc904`, with the identical measurement probe and no browser feature |
| A | Instrumented release Paneflow at the candidate commit, Browser inactive, four terminals |
| B | Minimal CEF harness, same engine pin, dedicated host and GPU backend as C, no dock or terminals |
| C | Instrumented release Paneflow at the same commit as A, four terminals and one visible browser viewport |

Freeze the replay with `bun scripts/browser-qualification.mjs replay` before
the first run. Its digest covers the [protocol](replay.json) and every emitted
event: four terminals with explicit per-pane geometry (69x17, 70x17, 69x17,
70x17 at the local 1920x1080 viewport), font and size, ASCII/Unicode/ANSI/cursor/scroll
bytes, a 50 ms output schedule, 200 ms input schedule and 1 s focus schedule.
Each repetition contains 5,950 planned events, including 300 post-warmup input
events. Archive the plan and actual delivery timestamps. Reject dropped,
changed or reordered events and delivery error above 2 ms. This is the native
replay contract; `src-app/src/terminal/bench_corpus.rs` remains the separate
CPU corpus. The native runner consumes this plan through four real PTYs.
Each worker reserves the first row for `pf-input:<terminal>:<tick>` and confines
output scrolling to rows 2..17. Input echo saves/restores the cursor and keeps
the marker visible until the next input. This setup and the per-pane geometry
were frozen before the accepted captures and are covered by the replay digest.

Fullscreen captures select [the fullscreen protocol](replay-fullscreen.json)
before creating either A or C. Its pane geometry is 70x18,71x18,70x18,71x18;
the replay events, schedules and delivery tolerance remain identical. Its
separate digest prevents pairing these captures with the windowed reference.
Actual output identity, fullscreen state and refresh rate must match within
each A/C terminal or B/C Browser pair.

`PANEFLOW_M1_LOG` opts into the release GPUI measurement probe. The additional
`PANEFLOW_IPC_SCRIPTING=1` opt-in permits `qualification.input` to register a
bounded schedule before replay begins. Its timer dispatches through the normal
terminal key handler; the measured input timestamp is handler entry, not IPC
registration. The private runner configuration fixes font, dimensions and four
PTY workloads without changing the user's saved settings.

Run each of empty, scroll, animation, WebGL and network separately, then
combined. B and C use the same fixture hash and backend. Keep the physical web
viewport at 1920x1080, independent of CSS scale and window decoration. Run
60 Hz and 120 Hz as separate conditions wherever the display/GPU is qualified
for them. Record the real frequency and scale. A missing 120 Hz reference is
missing evidence, not a 60 Hz result relabeled as 120 Hz.

For every condition, use release builds with identical instrumentation, 10 s
warm-up, then 60 s observation, five repetitions. Pair repetitions by index and
alternate A/C or B/C ordering to expose thermal/order effects. Preserve the
ordering in the raw trace. Input latency needs at least 1,000 events across the
five repetitions. Active presentation metrics require at least 1,000 samples. The empty browser
fixture instead records five complete monitored windows with zero frames after
warmup. Its result has no latency percentiles; frames are never fabricated to
reach a sample-count threshold. Run the opening/startup, memory and lifetime
experiments separately with their own trial/cycle counts.

Freeze machine identifier, exact OS/kernel/compositor versions, CPU/GPU, driver,
RAM, AC/battery state, refresh rate, scale, display backend, viewport dimensions,
Paneflow/GPUI commits, CEF/Chromium versions, cef-rs commit, engine manifest hash,
binary hashes, fixture/workload hashes and instrumentation revision. The local
machine inventory is in [Linux inventory](evidence/linux-inventory.json).
Each capture archives its actual condition. Historical
[preflight evidence](evidence/preflight.json) is preserved without alteration.

## Separate the clocks and costs

| Capture kind | Start | End | Interpretation |
|---|---|---|---|
| `terminal_cpu` | `start_ns` | `end_ns` | CPU pipeline work, not screen latency |
| `editor_cpu` | `start_ns` | `end_ns` | CPU editor work, not screen latency |
| `terminal_input_to_present` | `input_ns` | `present_ns` | Normal GPUI terminal key-handler entry to native presentation of its PTY marker |
| `browser_draw_to_present` | `draw_ns` | `present_ns` | CEF DrawAndSwap start to exactly correlated native Wayland presentation |
| `browser_idle` | No timed frame | No timed frame | Continuously monitored empty fixture with no post-warmup frame |
| `browser_callback_to_present` | `callback_ns` | `present_ns` | CEF accelerated callback to observed presentation of that frame |

Every timestamp is an integer number of nanoseconds in a documented monotonic
clock, relative to the start of its repetition. Translate GPU/compositor clocks
explicitly and archive the calibration, event/frame IDs and raw native trace.
Record the callback, submission/fence and observed presentation separately in
that trace. OnPaint, requestAnimationFrame, CPU render completion and queue
submission are insufficient endpoints. The existing debug-only
`PANEFLOW_LATENCY_PROBE` records keystroke-to-PTY work and cannot provide this
release presentation measurement. This is not a photonic scanout measurement.

Use nearest-rank percentiles: sort n durations and select the 1-based element
`ceil(p*n)` for p=0.50, 0.95 and 0.99. Calculate each repetition separately,
then report the median and maximum of the five repetition statistics. Paired
deltas are candidate minus reference for each matching repetition, followed by
their median and worst value. Do not pool unlike machines, loads or clocks.
Reject combined timestamp uncertainty above half the delta budget: 0.5 ms for
the terminal p95 budget, 1 ms for browser presentation. Report missing data
instead of interpolating it.

The CPU suites remain available through `scripts/bench-terminal.sh` / `.ps1`
and `scripts/bench-editor.sh` / `.ps1`. Preserve their existing baselines.
New runs and their build logs are archived separately under `evidence/m1/cpu`.
These CPU results cannot replace input-to-presentation samples.

Memory experiments include the complete browser process tree, parent delta,
private/PSS Linux memory, Windows private working set or equivalent macOS
footprint, GPU allocations and handle/fd counts. Document shared-memory double
counting. Measure 200 open/close cycles; interop pools, internal Chromium VRAM
and transient resize pools are separate quantities. Archive heaptrack/native
equivalent and CPU profiling traces before making improvement claims. These
experiments, frame misses, sandbox, accessibility and support compatibility are
not evaluated by the sample checker.

## Raw capture files and integrity checks

Archive each run in a new directory. Never overwrite a baseline. A capture
JSON uses `schema_version: 1`, the configuration, kind, scenario and identities
above, plus these fields:

| Field | Contract |
|---|---|
| `paneflow_commit`, `gpui_commit` | Full 40-character hexadecimal commits |
| `fixture_sha256`, `workload_sha256`, `binary_sha256` | Full lowercase SHA-256 digests |
| `engine` | null for inactive controls; otherwise `cef`, `chromium`, `cef_rs_commit`, `manifest_sha256` |
| `build_profile`, `terminal_count` | `release`; 4 for A/C/PRE_FEATURE, 0 for B |
| `environment` | `machine`, `os`, `kernel`, `compositor`, `cpu`, `gpu`, `driver`, `ram_mib`, `power`, `scale`, `refresh_hz`, `physical_width`, `physical_height`, `display_backend` |
| `instrumentation`, `clock`, `uncertainty_ns` | Instrumentation revision, mapped monotonic clock name, conservative absolute timestamp error |
| `calibration` | `clock` matching the capture, `max_error_ns` no larger than uncertainty, and at least two ordered `{source_ns, mapped_ns}` clock pairs; raw calibration retained in artifacts |
| `replay` | For A/C/PRE_FEATURE: workload `sha256`, matching positive `expected_events` / `observed_events`, zero `divergent_events`, `max_delivery_error_ns` at most 2,000,000; actual trace is retained for audit |
| `warmup_seconds`, `duration_seconds` | 10 and 60 |
| `load_controlled`, `fixture_failed` | true and false, corroborated by raw workload/fixture evidence |
| `presentation_observation` | `compositor_feedback` or `gpu_present_trace` for presentation metrics, null for CPU metrics |
| `artifacts` | Nonempty array of `{ "path": "relative-trace-file", "sha256": "..." }` with actual archived bytes |
| `repetitions` | Five objects, `index` 1 through 5, each with a nonempty `samples` array, except the explicitly proven `browser_idle` case |
| Each sample | `sequence` starting at 0, plus the two timestamp fields for its kind |

Samples must span the observation window, start at or after 10 s and finish at
or before 70 s. Gaps/duplicates in sample sequence, reversed or unsafe
timestamps, missing trace bytes, digest mismatch, short capture, load changes
and excessive uncertainty reject the run. Do not remove slow samples to repair
a rejection. Recapture and retain the rejected run with its explanation.

```sh
bun scripts/browser-qualification.mjs inspect path/to/capture.json
bun scripts/browser-qualification.mjs compare path/to/reference.json path/to/candidate.json
bun test scripts/browser-qualification/qualification.test.mjs
```

`inspect` returns `ACCEPTED_SAMPLES` (or `ACCEPTED_IDLE_OBSERVATION` for idle);
`compare` returns `PAIRED_SAMPLES` and refuses idle latency comparisons. Both
always return `qualification: NOT_EVALUATED`. Invalid captures exit 1 with
`REJECTED` on stderr and no report on stdout. The checker validates shape,
integrity and pairing, not the honesty of supplied measurements or their
native provenance. The dedicated terminal and Chromium/Wayland parsers perform
the native correlation before producing that structural capture. A hash alone
cannot prove a sandbox or GPU event. Synthetic
test captures exist only in temporary directories and never qualify hardware.
`compare` accepts PRE_FEATURE/A with different Paneflow commits, requiring
identical instrumentation, GPUI, workload and machine. A/C and B/C continue to
require matching commits. Calibration fields are mandatory even for older
schema-1 files; archive and recapture incomplete historical data, never invent
calibration to make a file pass.

## Native capture entry points

Build A and the isolated pre-feature checkout with the identical GPUI probe in
release mode. Keep their binary hashes, complete source inventory/patch and
successful build logs. The terminal runner checks the first native presentation against `--refresh-hz`
(60 by default, 120 for that separate condition), before the measured window.
It uses short private paths because a
Linux Unix socket path is limited to 108 bytes:

```sh
bun scripts/browser-qualification/terminal-capture.mjs --binary /absolute/paneflow-a --source-root /absolute/source-a --configuration A --output /tmp/new-m1-a
bun scripts/browser-qualification/terminal-capture.mjs --binary /absolute/paneflow-pre --source-root /absolute/source-pre --configuration PREFEATURE --output /tmp/new-m1-pre
bun scripts/browser-qualification/terminal-analysis.mjs /tmp/new-m1-a /absolute/metadata.json /absolute/new-archive
bun scripts/browser-qualification/browser-capture.mjs --binary target/release/paneflow-browser-host --build-evidence /absolute/release-build.json --environment /absolute/environment.json --scenario combined --raw /absolute/new-raw --output /absolute/new-archive
```

The terminal analyzer verifies all 5,950 events per repetition, unique input,
PTY echo, painted marker and matching native feedback. It checks geometry,
actual bytes, delivery timing, display refresh and clock calibration, then
archives raw JSONL plus separate thread-CPU measurements. A release build
receipt binds the binary to the source inventory and successful build log.

The browser runner performs five independent host lifecycles for the selected
fixture. It records 10 s warmup, 60 s observation and an extra drain second.
Chromium trace events are paired with native Wayland feedback by exact time
and frame identity, with required hardware clock/completion and vsync flags.
`browser_idle` additionally requires uninterrupted fixture monitoring and zero
post-warmup frames. Every run archives the engine manifest, source inventory,
release receipt, process topology, raw trace and Wayland protocol.

`scripts/browser-qualification/prototype.mjs` runs the integrated GPU path:
`paneflow browser-prototype` with a separate CEF host, DMA-BUF transfer and
GPUI composition. Its `native.json` records the host tree with sandbox flags,
X11 socket and `DISPLAY` evidence, the scripted steps (input, resize, scale,
host loss) and `frame_chain_us`: host blit-and-fence time (`callback_ns` to
`ready_ns`) and transfer-to-intake time (`ready_ns` to the application's
intake). Those are chain costs measured on the named driver, not the M1 C
presentation latency: the prototype does not pair frames with
`wp_presentation_feedback`, so `m1_presentation_feedback` stays
`NOT_MEASURED` and the C column below stays not executed. The archived runs
under `evidence/prototype/` are one Wayland run and one X11 run with the
sandbox enforced (both FAILED on the local NVIDIA driver, see the runtime
contract for the causes), one X11 diagnostic run without seccomp on the GPU
process that proves the transfer chain and is not qualification evidence, and
one `--source pattern` run that exercises the GPUI path without CEF.

Both runners retain failed captures and stop on a failed repetition. No slow
sample is deleted and no retry overwrites an earlier run. Short diagnostic
runs cannot be fed to the M1 analyzer as qualification captures.

## Baseline availability

The current local condition is Fedora 44, GNOME 50.4 Wayland, RTX 4070 Ti SUPER,
NVIDIA 610.57.04 and nominal 60 Hz (59.951 Hz actual), scale 1. Both enabled monitors are temporarily set to 59.951 Hz so compositor-controlled
window placement cannot silently select a 144 Hz output. Their original
143.973/144.006 Hz modes are restored afterward. The
[local receipt](evidence/m1/README.md) and [inventory](evidence/m1/index.json)
identify all eight accepted reference sets.

| Target | A / pre-feature raw M1 | B same-pin CEF raw M1 | C integrated raw M1 |
|---|---|---|---|
| Linux x86_64, local Wayland 60 Hz | See local capture inventory | See local capture inventory | Not executed: prototype chain costs only, see `evidence/prototype/` |
| Linux x86_64, 120 Hz or other distro/GPU/X11 | Not executed | Not executed | Not executed |
| Linux aarch64 | Not executed: GPU hardware access | Archive verified, native execution missing | Not executed |
| macOS aarch64 | Not executed | Not executed | Not executed |
| Windows x86_64 | Not executed | Not executed | Not executed |

Local reference capture does not declare a product GO, qualify the integrated
browser, or certify a distribution. All structural reports retain
`qualification: NOT_EVALUATED` until the later qualification work supplies C
and the other required release evidence.
