import { expect, test } from "bun:test";
import { extractIntegratedBrowserPresentation } from "./integrated-browser-trace.mjs";

const origin = 1_000_000_000_000;
const document = { owner: { workspace: "w", session: "s" }, browser: "b", generation: 1 };

function proof() {
  const options = { origin_ns: origin, gpu_pid: 42, refresh_hz: 60,
    capturer_lifecycle: { trace_start_ns: origin - 2e9, browser_create_ns: origin - 1e9, observation_end_ns: origin + 70e9, trace_end_ns: origin + 71e9 } };
  const trace = { metadata: { "clock-domain": "LINUX_CLOCK_MONOTONIC", trace_processor_stats: { bytes_overwritten: 0, track_event_parser_errors: 0 } },
    traceEvents: [{ ph: "M", name: "process_name", pid: 42, args: { name: "GPU Process" } }] };
  const events = [{ event: "started", clock: "CLOCK_MONOTONIC", at_ns: origin - 1e9 }];
  const emit = (name, ph, ns, args = {}, track = undefined) => {
    const event = { name, ph, ts: ns / 1000, pid: 42, tid: 43, args };
    if (track) event.id2 = { local: track };
    trace.traceEvents.push(event);
    return event;
  };
  emit("FrameSinkVideoCapturerImpl::Start", "b", origin - 0.5e9, {}, "0x1");
  const add = (index, seconds, kind = "fresh", previous = undefined) => {
    const ns = origin + seconds * 1e9;
    const counter = String(index);
    const capturer = { capturer_id: "opaque-capturer", capture_counter: counter };
    const media = String(index * 16667);
    if (kind === "fresh") {
      const uuid = "18446744073709551614";
      const id = String(9007199254740995n + BigInt(index));
      emit("CopyOutputRequest", "b", ns - 1e6, { success: true, has_provided_task_runner: true }, "0xabc");
      emit("Paneflow.CaptureCopyRequested", "n", ns - 0.9e6, { ...capturer, request_track_uuid: uuid, content_version: "7" }, "0xabc");
      emit("Graphics.Pipeline.DrawAndSwap", "b", ns, { display_trace_id: id }, `0x${index + 10}`);
      emit("Paneflow.CaptureCopyDraw", "n", ns + 0.1e6, { request_track_uuid: uuid, display_trace_id: id }, "0xabc");
      emit("CopyOutputRequest", "e", ns + 2e6, {}, "0xabc");
      emit("Paneflow.CaptureCopyResult", "I", ns + 2.5e6, { ...capturer, empty: false });
    } else if (kind === "resurrected") {
      emit("Paneflow.CaptureResurrected", "I", ns, { ...capturer, previous_capture_counter: String(previous), content_version: "7" });
    } else emit("Paneflow.CaptureEmptyContent", "I", ns, { ...capturer, content_version: "7" });
    emit("Paneflow.CaptureDelivered", "I", ns + 3e6, { ...capturer, metadata_capture_counter: counter, timestamp_micros: media });
    const fields = { document, pool_generation: 1, buffer: index % 3, frame_sequence: index,
      capture_counter: index, capture_timestamp_us: Number(media), callback_ns: ns + 4e6, ready_ns: ns + 5e6, intake_ns: ns + 6e6 };
    events.push({ ...fields, event: "browser_intake", at_ns: ns + 6e6 });
    events.push({ ...fields, event: "browser_paint", feedback_id: index + 1, at_ns: ns + 8e6 });
    events.push({ ...fields, event: "browser_presented", feedback_id: index + 1, present_ns: ns + 20e6,
      native_present_ns: ns + 20e6, presentation_callback_ns: ns + 21e6, clock_id: 1, refresh_ns: 16666667,
      sequence: index + 1, flags: "Vsync | HwClock | HwCompletion", calibration: { source_ns: ns + 21e6, mapped_ns: ns + 21e6, max_error_ns: 50 } });
  };
  [1, 2, 11, 12, 69].forEach((seconds, index) => add(index, seconds));
  return { trace, events, options, add, emit };
}

function analyze(p) { return extractIntegratedBrowserPresentation(p.trace, p.events, p.options); }
function marker(p, suffix, counter = "2") { return p.trace.traceEvents.find(event => event.name === `Paneflow.Capture${suffix}` && event.args.capture_counter === counter); }
function native(p, kind, sequence = 2) { return p.events.find(event => event.event === `browser_${kind}` && event.frame_sequence === sequence); }

test("exact C join keeps 64-bit IDs and reused pointer lifetimes distinct", () => {
  const result = analyze(proof());
  expect(result.qualification).toBe("NOT_EVALUATED");
  expect(result.measurement_status).toBe("COMPLETE");
  expect(result.kind).toBe("browser_draw_to_present");
  expect(result.samples.map(sample => sample.request_generation)).toEqual([3, 4, 5]);
  expect(result.samples.map(sample => sample.draw_ns)).toEqual([11e9, 12e9, 69e9]);
  expect(result.samples[0].display_trace_id).toBe("9007199254740997");
  expect(result.samples.every(sample => sample.present_ns - sample.draw_ns === 20e6)).toBe(true);
  expect(result.uncertainty_ns).toBe(1050);
  expect(result.diagnostics.counts.outside_window).toBe(2);
});

test("copy success may remain on end instead of being merged into begin", () => {
  const p = proof();
  for (const event of p.trace.traceEvents.filter(event => event.name === "CopyOutputRequest")) event.args = event.ph === "e" ? { success: true } : {};
  expect(analyze(p).measurement_status).toBe("COMPLETE");
});

function abandonedCopy() {
  const p = proof();
  const capturer = { capturer_id: "opaque-capturer", capture_counter: "99" };
  p.emit("CopyOutputRequest", "b", origin, { success: false }, "0xdead");
  p.emit("Paneflow.CaptureCopyRequested", "n", origin + 1000,
    { ...capturer, request_track_uuid: "123", content_version: "7" }, "0xdead");
  p.emit("CopyOutputRequest", "e", origin + 2000, {}, "0xdead");
  p.emit("Paneflow.CaptureCopyResult", "I", origin + 3000, { ...capturer, empty: true });
  p.emit("Paneflow.CaptureDropped", "I", origin + 4000, { ...capturer, result: "4" });
  return p;
}

test("an empty copy abandoned before drawing is counted without inventing a draw", () => {
  const result = analyze(abandonedCopy());
  expect(result.measurement_status).toBe("COMPLETE");
  expect(result.samples).toHaveLength(3);
  expect(result.diagnostics.counts.capture_dropped).toBe(1);
  expect(result.diagnostics.counts.capture_empty_or_missing_copy_result).toBe(1);
  const record = result.capture_records.find(record => record.capture_counter === "99");
  expect(record.outcome).toBe("empty_copy_abandoned_before_draw");
  expect(record.draw_ns).toBeUndefined();
});

function pendingCopy(offset) {
  const p = proof();
  const ns = p.options.capturer_lifecycle.observation_end_ns + offset;
  p.emit("CopyOutputRequest", "b", ns, {}, "0xfed");
  p.emit("Paneflow.CaptureCopyRequested", "n", ns + 1000,
    { capturer_id: "opaque-capturer", capture_counter: "99", request_track_uuid: "123", content_version: "7" }, "0xfed");
  return p;
}

test("a request still pending after observation is retained without a fabricated latency", () => {
  const result = analyze(pendingCopy(1e6));
  expect(result.measurement_status).toBe("COMPLETE");
  expect(result.samples).toHaveLength(3);
  expect(result.diagnostics.counts.capture_pending_after_observation).toBe(1);
  expect(result.capture_records.find(record => record.capture_counter === "99").outcome).toBe("pending_after_observation");
});

test.each([-1e6, 0])("a pending request born before or at the observation end still invalidates completeness", offset => {
  expect(analyze(pendingCopy(offset)).errors.some(error => error.code === "missing_or_ambiguous_request_lifetime_join")).toBe(true);
});

test("a late request cannot claim a delivery without its completed copy", () => {
  const p = pendingCopy(1e6);
  p.emit("Paneflow.CaptureDelivered", "I", origin + 70.002e9,
    { capturer_id: "opaque-capturer", capture_counter: "99", metadata_capture_counter: "99", timestamp_micros: "123" });
  expect(analyze(p).errors.some(error => error.code === "missing_or_ambiguous_request_lifetime_join")).toBe(true);
});

test.each([
  p => { marker(p, "CopyResult", "99").args.empty = false; },
  p => { p.trace.traceEvents = p.trace.traceEvents.filter(event => event !== marker(p, "CopyResult", "99")); },
  p => { p.trace.traceEvents = p.trace.traceEvents.filter(event => event !== marker(p, "Dropped", "99")); },
  p => { marker(p, "Dropped", "99").args.result = "1"; },
  p => { marker(p, "Dropped", "99").ts = origin / 1000 + 2; },
  p => { marker(p, "CopyResult", "99").ts = origin / 1000 + 1; },
  p => { p.trace.traceEvents.find(event => event.name === "CopyOutputRequest" && event.id2?.local === "0xdead" && event.ph === "b").args.success = true; },
  p => { p.emit("Paneflow.CaptureDelivered", "I", origin + 5000, { capturer_id: "opaque-capturer", capture_counter: "99", metadata_capture_counter: "99", timestamp_micros: "123" }); },
])("missing draw remains invalid unless the failed empty copy has a complete drop proof", mutate => {
  const p = abandonedCopy();
  mutate(p);
  expect(analyze(p).errors.some(error => error.code === "missing_or_ambiguous_request_lifetime_join")).toBe(true);
});

test("resurrection links the prior copy without inventing a new latency", () => {
  const p = proof();
  p.add(5, 69.1, "resurrected", 3);
  p.add(6, 69.2, "resurrected", 5);
  const result = analyze(p);
  expect(result.measurement_status).toBe("COMPLETE");
  expect(result.samples).toHaveLength(3);
  expect(result.diagnostics.counts.resurrected).toBe(2);
  expect(result.capture_records.find(record => record.capture_counter === "6").originating_copy_counter).toBe("3");
});

test("resurrection missing its original copy is explicit incomplete evidence", () => {
  const p = proof();
  p.add(5, 69.1, "resurrected", 99);
  expect(analyze(p).errors.some(error => error.code === "unproven_resurrection_origin")).toBe(true);
});

test("empty content, nonpainted intake and discarded native frame remain counted", () => {
  const p = proof();
  p.add(5, 69.1, "empty");
  p.events = p.events.filter(event => !(event.frame_sequence === 2 && ["browser_paint", "browser_presented"].includes(event.event)));
  const discarded = native(p, "presented", 3);
  discarded.event = "browser_discarded";
  discarded.at_ns = discarded.presentation_callback_ns;
  const result = analyze(p);
  expect(result.diagnostics.counts.empty_content).toBe(1);
  expect(result.diagnostics.counts.nonpainted).toBe(1);
  expect(result.diagnostics.counts.discarded).toBe(1);
  expect(result.samples).toHaveLength(1);
});

test("missing native feedback marks incomplete instead of selecting only remaining frames", () => {
  const p = proof();
  p.events = p.events.filter(event => event !== native(p, "presented"));
  const result = analyze(p);
  expect(result.measurement_status).toBe("INCOMPLETE");
  expect(result.diagnostics.counts.missing_feedback).toBe(1);
});

test("counter plus timestamp requires exact equality, never nearest time", () => {
  const p = proof();
  marker(p, "Delivered").args.timestamp_micros = "33335";
  expect(analyze(p).diagnostics.counts.missing_capture_join).toBe(1);
});

test("metadata counter overwrite rejects the logical capture association", () => {
  const p = proof();
  marker(p, "Delivered").args.metadata_capture_counter = "99";
  expect(analyze(p).errors.some(error => error.code === "logical_metadata_counter_mismatch")).toBe(true);
});

test("copy marker cannot join a past lifetime solely through reused pointer UUID", () => {
  const p = proof();
  const request = marker(p, "CopyRequested");
  request.ts += 4e3;
  expect(() => analyze(p)).toThrow("outside request lifetime");
});

test("overlapping pointer lifetimes are rejected", () => {
  const p = proof();
  const request = p.trace.traceEvents.find(event => event.name === "CopyOutputRequest" && event.ph === "b");
  p.trace.traceEvents.push({ ...request, ts: request.ts + 1 });
  expect(() => analyze(p)).toThrow("overlapping reused request");
});

test("request UUID mismatch and missing draw ID invalidate exact joins", () => {
  const p = proof();
  const draw = p.trace.traceEvents.find(event => event.name === "Paneflow.CaptureCopyDraw");
  draw.args.request_track_uuid = "1";
  draw.args.display_trace_id = "999";
  const result = analyze(p);
  expect(result.errors.some(error => error.code === "request_uuid_mismatch")).toBe(true);
  expect(result.errors.some(error => error.code === "missing_or_reversed_draw_join")).toBe(true);
});

test("missing copy result and empty result cannot certify a delivered capture", () => {
  const p = proof();
  marker(p, "CopyResult").args.empty = true;
  expect(analyze(p).errors.some(error => error.code === "delivery_without_successful_copy")).toBe(true);
});

test.each([
  p => { p.trace.metadata["clock-domain"] = "UNKNOWN"; },
  p => { p.trace.metadata.trace_processor_stats.bytes_overwritten = 1; },
  p => { delete p.trace.metadata.trace_processor_stats; },
  p => { native(p, "presented").calibration.mapped_ns += 1; },
  p => { native(p, "presented").calibration.max_error_ns = 1_000_001; },
  p => { native(p, "presented").flags = "Vsync"; },
  p => { p.events.push({ event: "fatal", reason: "evidence queue overflow" }); },
  p => { marker(p, "Delivered").args.timestamp_micros = 9007199254740992; },
  p => { p.options.gpu_pid = 99; },
  p => { p.options.capturer_lifecycle.trace_start_ns = origin; },
  p => { p.trace.traceEvents = p.trace.traceEvents.filter(event => event.name !== "FrameSinkVideoCapturerImpl::Start"); },
  p => { marker(p, "Delivered").args.capturer_id = "another-capturer"; },
])("clock, trace integrity and lifecycle corruption fail closed %#", mutate => {
  const p = proof();
  mutate(p);
  expect(() => analyze(p)).toThrow();
});


test("negative signed display IDs preserve the Chromium random u64 bit pattern", () => {
  const p = proof();
  for (const event of p.trace.traceEvents) if (event.args?.display_trace_id) event.args.display_trace_id = String(-BigInt(event.args.display_trace_id));
  expect(analyze(p).samples[0].display_trace_id).toBe("-9007199254740997");
});

test("missing callback result cannot masquerade as a merely undelivered frame", () => {
  const p = proof();
  p.trace.traceEvents = p.trace.traceEvents.filter(event => !(["Paneflow.CaptureCopyResult", "Paneflow.CaptureDelivered"].includes(event.name) && event.args.capture_counter === "2"));
  expect(analyze(p).errors.some(error => error.code === "missing_copy_result")).toBe(true);
});

test("reversed output sequences reject compositor evidence", () => {
  const p = proof();
  native(p, "presented", 2).sequence = 4;
  native(p, "presented", 3).sequence = 3;
  expect(() => analyze(p)).toThrow("sequence reversed");
});

test("copy callback after consumer delivery marks the trace incomplete", () => {
  const p = proof();
  marker(p, "CopyResult").ts += 1000;
  expect(analyze(p).errors.some(error => error.code === "reversed_copy_result_delivery")).toBe(true);
});


test("long idle resurrection chains resolve without recursion or fabricated latencies", () => {
  const p = proof();
  const initialEvents = [...p.events];
  for (let index = 5; index < 1505; index += 1) p.add(index, 69 + index / 1000, "resurrected", index - 1);
  p.events = initialEvents;
  const result = analyze(p);
  expect(result.measurement_status).toBe("COMPLETE");
  expect(result.capture_records.at(-1).originating_copy_counter).toBe("4");
  expect(result.samples).toHaveLength(3);
  expect(result.diagnostics.counts.delivered_without_intake).toBe(1500);
});
