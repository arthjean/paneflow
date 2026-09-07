# Capture to DrawAndSwap trace contract

Status: instrumentation only, not M1 qualification. The first combined build rejected an unregistered category group. The patch now uses the existing `gpu.capture` category; compilation of that correction and runtime capture remain unverified.

`0005-correlate-capture-with-draw-trace.patch` adds trace events to the pinned Chromium `39c51c70dd5feca6b6aba5bb7997b595011c553d`. It changes only `components/viz/service/frame_sinks/video_capture/frame_sink_video_capturer_impl.cc` and `components/viz/service/display/display.cc`. It does not alter capture allocation, copy destinations, scheduling, pixel contents, synchronization or delivery decisions. The render-pass traversal for correlation is gated by tracing. Trace-enabled overhead still requires measurement; disabled tracing is not a claim of literally zero instructions.

Apply from the Chromium source root with `git apply --check` followed by `git apply`. The patch was extracted before the separate native-handle capture allocation correction. Keep those patches separate and record their application order. Reverse application was checked against the source immediately after this patch was created.

## Trace configuration and integer handling

Enable `gpu.capture`, `viz`, `graphics.pipeline` and `benchmark` together. The new instant events use the existing `gpu.capture` category. A combined build rejected the initially attempted three-category group, so the patch does not introduce or register a new category. Existing `CopyOutputRequest` lifetime events require `viz`; the existing `Graphics.Pipeline.DrawAndSwap` slice uses `viz,benchmark`. Enabling only `graphics.pipeline` does not supply the complete join.

All added counters and identifiers are decimal strings, except `capturer_id`, which is the exact opaque string from `copy_request_source_.ToString()`, and `empty`, which is a boolean. Preserve strings through JSON and Rust boundaries. Do not convert identifiers to JavaScript `Number`. The request track UUID is unsigned 64-bit. Chromium declares `display_trace_id` and the logical capture counter as signed 64-bit, so preserve their full textual representation. The existing frame metadata capture counter is `optional<int>`; this patch does not widen it. `metadata_capture_counter = "-1"` or `previous_capture_counter = "-1"` denotes absent metadata and is not a valid frame identity.

Identity is scoped to one archived trace, process lifetime and capturer. Neither a pointer-derived track UUID nor a counter alone is a globally unique capture identity.

## Events

| Event | Arguments and meaning |
|---|---|
| `Paneflow.CaptureCopyRequested` | `capturer_id`, logical `capture_counter`, `request_track_uuid`, `capture_version_source`, `capture_version_sub_capture`, `content_version`. Emitted after a new request has its source set, on the same explicit Perfetto track as that request's existing lifetime slice. |
| `Paneflow.CaptureCopyDraw` | `request_track_uuid`, `display_trace_id`. Emitted on that request's track, for requests in the aggregated render-pass list immediately before the actual `renderer_->DrawFrame` call. |
| `Graphics.Pipeline.DrawAndSwap` begin | Existing slice, now with explicit string `display_trace_id`. This is the reference start endpoint. It occurs after aggregation and before draw callbacks, matching the existing B metric boundary. |
| `Paneflow.CaptureCopyResult` | `capturer_id`, logical `capture_counter`, `empty`. Emitted at `DidCopyFrame` entry. A nonempty result is necessary but not sufficient for successful delivery. |
| `Paneflow.CaptureResurrected` | `capturer_id`, new logical `capture_counter`, `previous_capture_counter`, `content_version`. Emitted before overwriting the reused VideoFrame metadata. No new CopyOutputRequest exists for this capture. |
| `Paneflow.CaptureEmptyContent` | `capturer_id`, logical `capture_counter`, `content_version`. The empty content rectangle follows the existing black-frame branch; it has no copy request or fresh draw identity. |
| `Paneflow.CaptureDropped` | `capturer_id`, logical `capture_counter`, `result`. Emitted after target-version and oracle rejection decisions, before the existing failed-capture return. The result value is the pinned Chromium `CaptureResult` enum ordinal. |
| `Paneflow.CaptureDelivered` | `capturer_id`, logical `capture_counter`, `metadata_capture_counter`, `capture_version_source`, `capture_version_sub_capture`, `timestamp_micros`. Emitted immediately before `OnFrameCaptured`, using the metadata and relative media timestamp actually copied into its Mojo message. |

The existing `CopyOutputRequest` begin is emitted by its constructor. Its end is emitted by `SendResult`, with `success` and `has_provided_task_runner`. The end marks result dispatch, not consumer callback execution or destruction. A pointer address can be reused after the request is destroyed while its result callback remains queued.

## Required joins

1. This version supports exactly one Browser capturer in the archived observation. Require tracing to start before the host browser-create command, exactly one `FrameSinkVideoCapturerImpl::Start` after that command, and exactly one `capturer_id` through the end of observation. The GPU may exist before tracing starts. The trace may stop before browser close, provided it covers all measured observations. A trace beginning after browser creation cannot establish this capturer association. Match the delivered GPUI frame's actual CEF `capture_counter` and exact relative media `timestamp_micros` to `Paneflow.CaptureDelivered`. Require equality between its logical and metadata capture counters, and exactly one matching delivery. Multi-capturer traces, duplicate matches and absent matches invalidate the join. Do not infer a capturer from temporal proximity. The capture-version fields are trace diagnostics; this contract does not assume they are exposed by the CEF public ABI.
2. For a fresh capture, match exactly one `Paneflow.CaptureCopyRequested` with that capturer and logical counter. Its explicit request track UUID must refer to the existing `CopyOutputRequest` lifetime containing that instant event.
3. Find exactly one `Paneflow.CaptureCopyDraw` inside that same request lifetime, not merely anywhere with the same pointer-derived UUID. Follow its string `display_trace_id` to the matching `Graphics.Pipeline.DrawAndSwap` begin. Require a successful request lifetime end, a nonempty `CaptureCopyResult`, and a successful matching delivery. The same draw may legitimately serve several different capture requests.
4. The capture-to-GPUI pipeline must preserve this identity through frame acceptance, paint submission and native presentation feedback. The C endpoint is native presentation of this exact painted frame. GPU intake, callback arrival, frame acceptance and `OnAcceleratedPaint` do not substitute for native presentation.
5. Compute C latency from this `DrawAndSwap` begin to that native presentation endpoint, after the clock-domain calibration required by M1. Compare against B only when B uses the same start boundary and its corresponding native presentation endpoint. Trace timestamps are not a calibration by themselves. `timestamp_micros` is the oracle-adjusted media timestamp relative to the first delivered media tick, used only as an exact identity component. It is not the capture start timestamp or a monotonic-clock calibration.

Use trace event ordering and lifetime boundaries without losing timestamp precision. In particular, do not join all events sharing a request track UUID, or choose the nearest prior draw. If a JSON conversion collapses event times, loses track identity or loses the ordering needed to distinguish reused request lifetimes, use the original Perfetto data or invalidate the ambiguous samples.

## Resurrected and empty captures

A resurrected frame shares the marked VideoFrame's existing pixel contents. Resolve `previous_capture_counter` recursively in the same capturer, requiring identical `content_version` at every edge, until a unique prior successful fresh copy is found. That originating copy establishes content provenance only. It does not create a fresh DrawAndSwap event at the resurrection timestamp. Reject missing ancestors, cycles, conflicting versions, failed origins and ambiguous metadata counter associations. A trace beginning after the source copy cannot prove this chain.

Keep resurrected deliveries in explicit counts and report their originating copy when provable. Do not mix their age from the old draw into the fresh-frame B/C latency distribution, and do not invent a new start time from the resurrection event. A scenario with too few fresh samples is incomplete. Idle repeated content is not proof of a new native presentation or missed-frame performance.

An empty-content capture is explicitly a no-copy case. It has no legitimate DrawAndSwap association. Report it separately; do not impute latency or count it as a measured GPU copy. `CaptureCopyResult.empty = true` is a different case: a real request produced an empty result, and must fail the success join.

## Failure handling and limits

Fail the affected sample on missing trace segments, an unfinished request, absent or multiple draw joins, a dropped delivery, unmatched CEF identity, missing native feedback, or absent clock calibration. Report counts of each excluded class. Do not silently select only the successful subset of an otherwise failing run.

The added events prove a request was associated with one actual renderer draw invocation and identify its capture delivery. They do not prove GPU completion, CPU-readback absence, a particular full-frame copy count, resource bounds, physical display timing, sandbox correctness or an M1 budget. Those remain separate measurements and audits. Compilation, actual trace export shape, complete join coverage and tracing overhead must be verified on the final combined runtime.


## Integrated analyzer API

`extractIntegratedBrowserPresentation(trace, events, options)` in `scripts/browser-qualification/integrated-browser-trace.mjs` takes parsed Chromium trace JSON and parsed native logger JSONL events. `readIntegratedBrowserPresentation(tracePath, eventsPath, options)` performs bounded file reads and rejects a truncated JSONL tail.

Required options:

```json
{
  "origin_ns": 1000000000000,
  "gpu_pid": 42,
  "warmup_ns": 10000000000,
  "duration_ns": 60000000000,
  "refresh_hz": 60,
  "max_uncertainty_ns": 1000000,
  "capturer_lifecycle": {
    "trace_start_ns": 998000000000,
    "browser_create_ns": 999000000000,
    "observation_end_ns": 1070000000000,
    "trace_end_ns": 1071000000000
  }
}
```

These illustrative timestamps must be replaced by archived CLOCK_MONOTONIC measurements from protocol commands and trace control. They are evidence inputs, not guessed from the first and last visible trace event. Warmup and duration default to 10 and 60 seconds. The native logger must contain its `started` record with `CLOCK_MONOTONIC` before the origin.

The native schema is the current `BrowserFrame.fields()` plus `browser_intake`, `browser_paint`, `browser_presented`, `browser_discarded` and `browser_paint_unobserved`. It joins the exact document, pool generation, buffer and frame sequence across stages, then the capture counter and relative capture timestamp to Chromium delivery. Integers emitted as JSON numbers must remain safe integers; decimal strings support full-width identifiers. Missing native hardware timing flags or invalid calibration fail the analysis. Native clock calibration error plus a 1 microsecond Chromium trace quantization bound must fit `max_uncertainty_ns`. Two measured native calibration points are required, including warmup for idle scenarios.

The result retains `kind: "browser_draw_to_present"`, `configuration: "C"`, `qualification: "NOT_EVALUATED"` and origin-relative `draw_ns`/`present_ns` samples compatible with B's endpoint definition. `measurement_status: "INCOMPLETE"` exposes detected join gaps through `errors`; sample rows in an incomplete report remain diagnostic and must not be used as an accepted subset. Structural corruption, unsafe IDs, clock corruption, ambiguous capturers and missing lifecycle evidence throw an error. Callers must archive that failure rather than replace it with an empty valid measurement.

`records` classifies every intake and `capture_records` records each Chromium capture origin. `diagnostics.counts` reports measured, outside-window, nonpainted, unobserved paint, discarded, resurrected, empty-content and missing-association classes, plus dropped captures and deliveries without intake. Counts of nonpainted or discarded frames remain visible even when the exact joins themselves are complete. `COMPLETE` means the represented joins contain no detected gap; it does not assert sufficient sample count, frame loss budgets, trace overhead, physical output qualification, four-terminal load or EP-002 completion. Those require the enclosing M1 analysis.

The analyzer handles pointer reuse within explicit request lifetime boundaries, preserves negative signed DrawAndSwap IDs, accepts copy-result annotations moved onto begin events by Chromium's JSON exporter, and resolves long resurrection chains iteratively with cached origins. The targeted synthetic regression suite is `bun test scripts/browser-qualification/integrated-browser-trace.test.mjs`. Its 29 cases passed after implementation. Final runtime export and end-to-end C evidence still need separate verification.
