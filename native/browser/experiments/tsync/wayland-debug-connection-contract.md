# Wayland debug connection identity

`0007-identify-wayland-debug-connections.patch` modifies the pinned Chromium copy of libwayland. SHA-256: `b4e07e6103b17733f312148a9f7cba22b66b534402057c1404bb7106464fa28e`.

The patch applies from Chromium `src/`. It changes only the private `wl_closure_print` signature and its three callers, the client-private `wl_display`, and the client connection allocator. It does not change CEF's public API. Existing `WAYLAND_DEBUG=client` enables the client log; no new logging toggle is required. Server calls pass zero and retain the existing output format.

A client protocol line has this format:

```text
[1328941.406] [pid=785779 connection=1]  -> wl_registry#2.bind(6, "wl_output", 4, new id [unknown]#8)
[1328941.935] [pid=785779 connection=1] wl_output#8.name("DP-4")
[1328941.939] [pid=785779 connection=1] wl_output#8.done()
```

The first bracket remains the existing truncated CLOCK_REALTIME diagnostic value. It is not usable for latency, observation bounds or calibration. Native `wp_presentation_feedback.presented` timestamps and `clock_id(1)` remain the CLOCK_MONOTONIC evidence. Chromium end timestamps join those native timestamps exactly at Chromium's microsecond precision.

The second bracket contains the current process PID and a positive uint64 connection ID. A process-local atomic counter allocates that ID once per `wl_display_connect_to_fd` allocation, before any protocol request. IDs are never recycled when a display disconnects. Zero is reserved for calls without a client connection. Exhaustion aborts rather than reusing zero. The analyzer retains decimal connection IDs as strings, including values above JavaScript's safe integer range. The process PID separates forked processes' counter spaces. The contract assumes the archived process lifetime and patched libwayland implementation; a separate uninstrumented libwayland cannot be silently assigned to one of these namespaces.

Optional `{queue name}` text remains diagnostic. Multiple queues can belong to the same connection, and different connections can use the same queue name. Queue labels never establish identity.

## B association and archive lifecycle

The strict B parser uses `(PID, connection ID, object ID, object generation)` for output metadata and presentation feedback. It accepts the native feedback PID only when it is the observed CEF host or GPU process. It does not assume that the process carrying the Wayland surface also produces the GPU DrawAndSwap trace. One feedback connection and one surface must correspond exactly to the Chromium presentation endpoints. A second connection with identical numeric object IDs cannot repair missing metadata in the first. Reusing an output ID requires its release and the compositor delete_id confirmation before the next binding creates a new generation.

Each SyncOutput association snapshots the current output binding, registry global name, generation, stable connector name, current mode, integer scale and completed Done event. The requested output must match `environment.expected_output`; its mode must be 1920x1080 scale 1 at the actual refresh in the archived DisplayConfig receipt. Presentation feedback independently verifies the observed refresh. Fullscreen completion and the complete fixture viewport/visibility monitoring window are required. These establish a native fullscreen output binding; occlusion and physical photons remain unevaluated.

Windowed capture remains the default. Fullscreen B uses `browser-capture.mjs --fullscreen`, with `display_evidence_directory` and `expected_output` in the environment JSON. The witness receives `PANEFLOW_M1_FULLSCREEN=1` only for that explicit option; windowed runs remove an inherited value.

The capture archives `applied.json` before its five witness children, then writes `capture-pending.json` and returns `PENDING_DISPLAY_COMPLETION`. It does not wait for the enclosing display supervisor to finish while still running inside that supervisor. After the supervisor writes its genuine `condition-complete.json`, run:

```sh
bun scripts/browser-qualification/browser-capture.mjs --finalize --output /absolute/capture/directory
```

An optional `--display-completion /absolute/condition-complete.json` selects the completion receipt explicitly. Finalization verifies all archived digests, replays the Chromium/Wayland parser against compressed raw archives, checks native host/GPU identity, verifies the pending sample and calibration data, and validates matching display serial and layout through every complete observation interval. Only then does it emit `capture.json` and `inspection.json`. Missing completion leaves the pending capture unclosed. Neither parser nor finalization certifies EP-002.

The targeted parser and archive tests use synthetic inputs and do not establish real runtime behavior:

```sh
bun test scripts/browser-qualification/browser-trace.test.mjs scripts/browser-qualification/browser-capture.test.mjs
```

Native compilation and an actual fullscreen run must separately confirm that all relevant client libraries emit tags and that the selected primary output is DP-4. Mixed untagged/tagged protocol logs are rejected rather than heuristically merged.
