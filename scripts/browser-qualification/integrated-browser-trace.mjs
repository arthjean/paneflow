import { readFile, stat } from "node:fs/promises";
import { countMissedFrames } from "./missed-frames.mjs";

const prefix = "Paneflow.Capture";
const drawName = "Graphics.Pipeline.DrawAndSwap";
const requestName = "CopyOutputRequest";
const startName = "FrameSinkVideoCapturerImpl::Start";
const lossPattern = /data_loss|packet_loss|overwritten|discarded|dropped|abi_violations|patches_failed|parser_failure|tokenizer_failure|clock_sync_failure|invalid_clock|packet_skipped|parser_errors|tokenizer_errors|missing_timestamp|invalid_track|conflicting_reservation/;

function requireValue(value, message) {
  if (!value) throw new Error(`integrated browser presentation proof: ${message}`);
}

function integer(value, name, minimum = 0) {
  requireValue(Number.isSafeInteger(value) && value >= minimum, `invalid ${name}`);
  return value;
}

function identifier(value, name, trace = false) {
  if (!trace && Number.isSafeInteger(value) && value >= 0) return String(value);
  requireValue(typeof value === "string" && /^(0|[1-9][0-9]*)$/.test(value) && BigInt(value) <= 0xffffffffffffffffn, `invalid exact ${name}`);
  return value;
}

function drawIdentifier(value) {
  requireValue(typeof value === "string" && /^-?(0|[1-9][0-9]*)$/.test(value) && value !== "-0"
    && BigInt(value) >= -(1n << 63n) && BigInt(value) < (1n << 63n), "invalid exact signed display trace ID");
  return value;
}

function statistics(value, path = "trace_processor_stats") {
  requireValue(value && typeof value === "object" && Object.keys(value).length > 0, `missing ${path}`);
  for (const [key, entry] of Object.entries(value)) {
    if (entry && typeof entry === "object") statistics(entry, `${path}.${key}`);
    else if (lossPattern.test(key)) requireValue(entry === 0, `trace loss or corruption: ${path}.${key}`);
  }
}

function asyncTrack(event) {
  const local = event.id2?.local;
  requireValue(typeof local === "string" && /^0x[0-9a-f]+$/i.test(local), "missing exported async track identity");
  return `${event.pid}:${local.toLowerCase()}`;
}

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map(key => [key, stable(value[key])]));
  return value;
}

function frameIdentity(event) {
  const document = event.document;
  requireValue(document && typeof document.browser === "string" && document.owner && typeof document.owner.workspace === "string" && typeof document.owner.session === "string", "missing browser document identity");
  const normalized = { ...document, generation: identifier(document.generation, "document generation") };
  return JSON.stringify([stable(normalized), ...["pool_generation", "buffer", "frame_sequence"].map(key => identifier(event[key], key))]);
}

function captureIdentity(event) {
  return `${identifier(event.capture_counter, "capture counter")}:${identifier(event.capture_timestamp_us, "capture timestamp")}`;
}

function sameFrame(left, right) {
  return frameIdentity(left) === frameIdentity(right) && captureIdentity(left) === captureIdentity(right)
    && ["callback_ns", "ready_ns", "intake_ns"].every(key => left[key] === right[key]);
}

function parseTrace(trace, options, bounds) {
  requireValue(Array.isArray(trace?.traceEvents) && trace.traceEvents.length <= 4_000_000, "invalid or oversized trace");
  requireValue(trace.metadata?.["clock-domain"] === "LINUX_CLOCK_MONOTONIC", "trace clock must be LINUX_CLOCK_MONOTONIC");
  statistics(trace.metadata.trace_processor_stats);
  const pids = new Set(trace.traceEvents.filter(event => event.name === "process_name" && event.ph === "M" && event.args?.name === "GPU Process").map(event => event.pid));
  requireValue(pids.size === 1 && pids.has(integer(options.gpu_pid, "observed GPU PID", 1)), "GPU trace PID absent, ambiguous or differs from observed process");
  const relevant = trace.traceEvents.filter(event => event.name?.startsWith(prefix) || [requestName, drawName, startName].includes(event.name));
  const ordered = relevant.map((event, ordinal) => {
    requireValue(event.pid === options.gpu_pid, "capture trace event belongs to another GPU process");
    integer(event.tid, "trace thread ID", 1);
    integer(event.ts * 1000, "trace nanosecond timestamp");
    requireValue(event.ts * 1000 >= bounds.trace_start_ns && event.ts * 1000 <= bounds.trace_end_ns, "trace event outside archived trace bounds");
    return { ...event, ordinal, ns: event.ts * 1000 };
  }).sort((a, b) => a.ns - b.ns || a.ordinal - b.ordinal);
  const starts = ordered.filter(event => event.name === startName && event.ph === "b");
  requireValue(starts.length === 1 && starts[0].ns >= bounds.browser_create_ns, "missing or ambiguous capturer Start after browser create");
  const active = new Map();
  const generations = new Map();
  const requests = [];
  const draws = new Map();
  const captures = new Map();
  const capturers = new Set();
  for (const event of ordered) {
    if (event.name === requestName) {
      const track = asyncTrack(event);
      requireValue(["b", "e"].includes(event.ph), "invalid request lifetime phase");
      if (event.ph === "b") {
        requireValue(!active.has(track), "overlapping reused request pointer lifetime");
        const generation = (generations.get(track) ?? 0) + 1;
        generations.set(track, generation);
        const request = { start: event, track, generation, markers: [] };
        active.set(track, request);
        requests.push(request);
      } else {
        const request = active.get(track);
        requireValue(request && event.ns >= request.start.ns, "request end without beginning");
        request.end = event;
        const flags = [request.start.args?.success, event.args?.success].filter(value => value !== undefined);
        requireValue(flags.length > 0 && flags.every(value => typeof value === "boolean" && value === flags[0]), "missing or conflicting copy result success flag");
        request.success = flags[0];
        active.delete(track);
      }
      continue;
    }
    if (event.name === drawName && event.ph === "b") {
      const id = drawIdentifier(event.args?.display_trace_id);
      requireValue(!draws.has(id), "duplicate display trace ID");
      draws.set(id, event);
      continue;
    }
    if (!event.name.startsWith(prefix)) continue;
    requireValue(["I", "i", "n"].includes(event.ph), "invalid capture marker phase");
    requireValue(event.ns >= starts[0].ns, "capture marker precedes capturer Start");
    const type = event.name.slice(prefix.length);
    requireValue(["CopyRequested", "CopyDraw", "CopyResult", "Resurrected", "EmptyContent", "Dropped", "Delivered"].includes(type), "unknown capture marker");
    if (["CopyRequested", "CopyDraw"].includes(type)) {
      const request = active.get(asyncTrack(event));
      requireValue(request, "capture copy marker outside request lifetime");
      request.markers.push(event);
      event.request = request;
    }
    if (type === "CopyDraw") continue;
    const capturer = event.args?.capturer_id;
    requireValue(typeof capturer === "string" && capturer.length > 0, "missing capturer identity");
    capturers.add(capturer);
    const counter = identifier(event.args.capture_counter, "trace capture counter", true);
    const key = `${capturer}:${counter}`;
    const capture = captures.get(key) ?? { key, capturer, counter, first_ns: event.ns };
    requireValue(!capture[type], `duplicate ${type} for capture`);
    capture[type] = event;
    captures.set(key, capture);
  }
  requireValue(capturers.size === 1, "capturer identity absent or ambiguous across observation lifecycle");
  return { requests, draws, captures, capturer: [...capturers][0] };
}

function checkPresentation(event, paint, maximumError, refreshHz) {
  for (const key of ["present_ns", "native_present_ns", "presentation_callback_ns", "clock_id", "refresh_ns"]) integer(event[key], key);
  identifier(event.sequence, "native output sequence");
  requireValue(typeof event.flags === "string" && ["Vsync", "HwClock", "HwCompletion"].every(flag => new RegExp(`\\b${flag}\\b`).test(event.flags)) && !event.flags.includes("Unknown"), "presentation lacks hardware clock/completion and VSYNC flags");
  requireValue(event.refresh_ns > 0 && (refreshHz === undefined || Math.abs(1e9 / event.refresh_ns - refreshHz) <= 0.1), "native refresh differs from capture condition");
  const calibration = event.calibration;
  requireValue(calibration && ["source_ns", "mapped_ns", "max_error_ns"].every(key => Number.isSafeInteger(calibration[key]) && calibration[key] >= 0), "missing measured presentation clock calibration");
  requireValue(calibration.max_error_ns <= maximumError, "presentation clock uncertainty exceeds budget");
  requireValue(event.present_ns === event.native_present_ns + calibration.mapped_ns - calibration.source_ns, "native clock mapping differs from calibration");
  requireValue(paint.at_ns <= event.present_ns && event.present_ns <= event.presentation_callback_ns + calibration.max_error_ns, "paint and native presentation are causally reversed");
  return calibration;
}

export function extractIntegratedBrowserPresentation(trace, events, options) {
  const origin = integer(options?.origin_ns, "capture origin");
  const start = integer(origin + integer(options.warmup_ns ?? 10e9, "warmup"), "measurement start");
  const end = integer(start + integer(options.duration_ns ?? 60e9, "duration", 1), "measurement end");
  const maximumError = integer(options.max_uncertainty_ns ?? 1_000_000, "uncertainty budget", 1000) - 1000;
  const bounds = options.capturer_lifecycle;
  requireValue(bounds && ["trace_start_ns", "browser_create_ns", "observation_end_ns", "trace_end_ns"].every(key => Number.isSafeInteger(bounds[key]) && bounds[key] >= 0), "missing archived capturer lifecycle bounds");
  requireValue(bounds.trace_start_ns <= bounds.browser_create_ns && bounds.browser_create_ns <= origin && bounds.observation_end_ns >= end && bounds.trace_end_ns >= bounds.observation_end_ns, "trace does not cover create and observation lifecycle");
  requireValue(Array.isArray(events) && events.length <= 4_000_000, "invalid or oversized native evidence");
  requireValue(!events.some(event => event.event === "fatal"), "fatal or lost native evidence");
  const started = events.filter(event => event.event === "started");
  requireValue(started.length === 1 && started[0].clock === "CLOCK_MONOTONIC" && integer(started[0].at_ns, "native logger start") <= origin, "native logger clock/start missing or ambiguous");
  const chromium = parseTrace(trace, options, bounds);
  const errors = [];
  const counts = Object.fromEntries(["measured", "outside_window", "nonpainted", "paint_unobserved", "discarded", "resurrected", "empty_content", "missing_feedback", "missing_capture_join", "missing_draw_join", "delivered_without_intake", "capture_dropped", "capture_not_delivered", "capture_empty_or_missing_copy_result"].map(key => [key, 0]));
  const records = [];
  const count = name => { counts[name] = (counts[name] ?? 0) + 1; };
  const fail = (code, identity) => { errors.push({ code, identity }); };
  const deliveryByIdentity = new Map();
  const captureRecords = [];
  for (const capture of chromium.captures.values()) {
    const classes = ["CopyRequested", "Resurrected", "EmptyContent"].filter(type => capture[type]);
    const record = { capture_counter: capture.counter, capturer_id: capture.capturer, kind: classes[0] ?? "missing_origin", first_ns: capture.first_ns };
    capture.record = record;
    captureRecords.push(record);
    count(`capture_${record.kind}`);
    if (classes.length !== 1) fail("missing_or_ambiguous_capture_origin", capture.key);
    for (const type of classes) identifier(capture[type].args.content_version, "capture content version", true);
    if (capture.CopyRequested) {
      const request = capture.CopyRequested.request;
      const markers = request.markers.filter(event => event.name === `${prefix}CopyDraw`);
      const requests = request.markers.filter(event => event.name === `${prefix}CopyRequested`);
      record.request_generation = request.generation;
      const abandonedBeforeDraw = markers.length === 0 && request.end
        && request.success === false && capture.CopyResult?.args.empty === true
        && capture.CopyResult.ns >= request.end.ns && capture.Dropped
        && capture.Dropped.ns >= capture.CopyResult.ns
        && ["2", "3", "4", "5"].includes(capture.Dropped.args.result)
        && !capture.Delivered;
      const pendingAfterObservation = request.start.ns > bounds.observation_end_ns
        && !request.end && markers.length <= 1
        && !capture.CopyResult && !capture.Delivered && !capture.Dropped;
      if (requests.length !== 1) fail("missing_or_ambiguous_request_lifetime_join", capture.key);
      else if (pendingAfterObservation) {
        identifier(capture.CopyRequested.args.request_track_uuid, "request UUID", true);
        count("capture_pending_after_observation");
        record.outcome = "pending_after_observation";
      }
      else if (!request.end || (markers.length !== 1 && !abandonedBeforeDraw)) fail("missing_or_ambiguous_request_lifetime_join", capture.key);
      else if (abandonedBeforeDraw) {
        identifier(capture.CopyRequested.args.request_track_uuid, "request UUID", true);
        count("capture_empty_or_missing_copy_result");
        record.outcome = "empty_copy_abandoned_before_draw";
      }
      else {
        const marker = markers[0];
        const uuid = identifier(capture.CopyRequested.args.request_track_uuid, "request UUID", true);
        if (identifier(marker.args.request_track_uuid, "draw request UUID", true) !== uuid) fail("request_uuid_mismatch", capture.key);
        const drawId = drawIdentifier(marker.args.display_trace_id);
        const draw = chromium.draws.get(drawId);
        if (!draw || draw.ns > marker.ns || marker.ns < capture.CopyRequested.ns || marker.ns > request.end.ns) fail("missing_or_reversed_draw_join", capture.key);
        else { record.draw_ns = draw.ns; record.display_trace_id = drawId; }
        if (!request.success || !capture.CopyResult || capture.CopyResult.args.empty !== false) {
          count("capture_empty_or_missing_copy_result");
          if (capture.Delivered) fail("delivery_without_successful_copy", capture.key);
          if (!capture.CopyResult) fail("missing_copy_result", capture.key);
        }
        if (capture.CopyResult && (capture.CopyResult.ns < request.end.ns || (capture.Delivered && capture.CopyResult.ns > capture.Delivered.ns))) fail("reversed_copy_result_delivery", capture.key);
      }
    }
    record.dropped = !!capture.Dropped;
    record.delivered = !!capture.Delivered;
    if (capture.Dropped) count("capture_dropped");
    if (capture.Delivered) {
      const delivery = capture.Delivered;
      if (capture.Dropped) fail("dropped_capture_delivered", capture.key);
      if (identifier(delivery.args.metadata_capture_counter, "metadata capture counter", true) !== capture.counter) fail("logical_metadata_counter_mismatch", capture.key);
      const identity = `${capture.counter}:${identifier(delivery.args.timestamp_micros, "delivered media timestamp", true)}`;
      requireValue(!deliveryByIdentity.has(identity), "duplicate delivered capture identity");
      deliveryByIdentity.set(identity, capture);
    } else count("capture_not_delivered");
  }
  const originCache = new Map();
  function sourceCopy(capture) {
    const chain = [];
    const seen = new Set();
    let current = capture;
    let originCopy;
    while (current && !seen.has(current.key)) {
      if (originCache.has(current.key)) { originCopy = originCache.get(current.key); break; }
      chain.push(current);
      seen.add(current.key);
      if (current.CopyRequested) {
        if (current.record.draw_ns !== undefined && current.CopyRequested.request.success && current.CopyResult?.args.empty === false) originCopy = current;
        break;
      }
      if (!current.Resurrected) break;
      const previous = current.Resurrected.args.previous_capture_counter;
      if (previous === "-1") break;
      identifier(previous, "resurrected previous counter", true);
      const ancestor = chromium.captures.get(`${current.capturer}:${previous}`);
      if (!ancestor || ancestor.first_ns >= current.first_ns || (ancestor.CopyRequested ?? ancestor.Resurrected)?.args.content_version !== current.Resurrected.args.content_version) break;
      current = ancestor;
    }
    for (const item of chain) originCache.set(item.key, originCopy);
    return originCopy;
  }
  for (const capture of chromium.captures.values()) {
    if (!capture.Resurrected) continue;
    const ancestor = sourceCopy(capture);
    if (ancestor) capture.record.originating_copy_counter = ancestor.counter;
    else { count("capture_resurrection_missing_origin"); fail("unproven_resurrection_origin", capture.key); }
  }
  const intakes = new Map();
  const paints = new Map();
  const feedbacks = new Map();
  const unobserved = new Set();
  for (const event of events.filter(event => event.event?.startsWith("browser_"))) {
    if (!["browser_intake", "browser_paint", "browser_presented", "browser_discarded", "browser_paint_unobserved"].includes(event.event)) continue;
    const key = frameIdentity(event);
    captureIdentity(event);
    for (const field of ["callback_ns", "ready_ns", "intake_ns"]) integer(event[field], field);
    requireValue(event.callback_ns <= event.ready_ns && event.ready_ns <= event.intake_ns, "callback, readiness and intake are reversed");
    if (event.event === "browser_intake") {
      requireValue(!intakes.has(key), "duplicate native intake identity");
      intakes.set(key, event);
    } else if (event.event === "browser_paint") {
      requireValue(!paints.has(key), "duplicate painted frame identity");
      integer(event.at_ns, "paint timestamp");
      requireValue(event.at_ns >= event.intake_ns, "paint precedes intake");
      paints.set(key, event);
    } else if (event.event === "browser_paint_unobserved") unobserved.add(key);
    else {
      requireValue(!feedbacks.has(key), "duplicate or contradictory native feedback");
      feedbacks.set(key, event);
    }
  }
  for (const key of new Set([...paints.keys(), ...feedbacks.keys(), ...unobserved])) if (!intakes.has(key)) fail("native_event_without_intake", key);
  const samples = [];
  const calibration = [];
  const usedDeliveries = new Set();
  const usedFeedbacks = new Set();
  const sequences = new Set();
  const nativeOrder = [];
  const slotPresentations = [];
  const clockIds = new Set();
  for (const [key, intake] of intakes) {
    const record = { frame_identity: key, capture_identity: captureIdentity(intake), intake_ns: intake.intake_ns };
    records.push(record);
    const inWindow = intake.intake_ns >= start && intake.intake_ns < end;
    const classify = (status, invalid = false) => { record.status = status; count(status); if (invalid && inWindow) fail(status, key); };
    const capture = deliveryByIdentity.get(record.capture_identity);
    if (!capture) { classify("missing_capture_join", true); continue; }
    if (usedDeliveries.has(capture.key)) { classify("duplicate_capture_intake", true); continue; }
    usedDeliveries.add(capture.key);
    if (capture.Delivered.ns > intake.callback_ns) { classify("capture_delivery_after_callback", true); continue; }
    const paint = paints.get(key);
    const feedback = feedbacks.get(key);
    if (!paint) { classify(unobserved.has(key) ? "paint_unobserved" : "nonpainted", unobserved.has(key) || !!feedback); continue; }
    if (!sameFrame(intake, paint)) { classify("paint_identity_mismatch", true); continue; }
    const feedbackId = identifier(paint.feedback_id, "feedback ID");
    requireValue(!usedFeedbacks.has(feedbackId), "reused native feedback ID");
    usedFeedbacks.add(feedbackId);
    if (!feedback) { classify("missing_feedback", true); continue; }
    if (!sameFrame(paint, feedback) || identifier(feedback.feedback_id, "feedback ID") !== feedbackId) { classify("feedback_identity_mismatch", true); continue; }
    if (feedback.event === "browser_discarded") { classify("discarded"); continue; }
    const point = checkPresentation(feedback, paint, maximumError, options.refresh_hz);
    calibration.push(point);
    clockIds.add(feedback.clock_id);
    nativeOrder.push(feedback);
    slotPresentations.push({ present_ns: feedback.present_ns, refresh_ns: feedback.refresh_ns,
      output_sequence: String(feedback.sequence), fresh: !!capture.CopyRequested && capture.record.draw_ns !== undefined,
      content_id: capture.record.display_trace_id });
    const sequence = identifier(feedback.sequence, "output sequence");
    requireValue(!sequences.has(sequence), "duplicate native output sequence");
    sequences.add(sequence);
    if (capture.Resurrected) { classify("resurrected"); record.originating_copy_counter = capture.record.originating_copy_counter; continue; }
    if (capture.EmptyContent) { classify("empty_content"); continue; }
    if (capture.record.draw_ns === undefined) { classify("missing_draw_join", true); continue; }
    if (capture.record.draw_ns > intake.callback_ns || capture.record.draw_ns > feedback.present_ns) { classify("draw_after_capture_or_presentation", true); continue; }
    if (capture.record.draw_ns < start || feedback.present_ns >= end) { classify("outside_window"); continue; }
    classify("measured");
    samples.push({ sequence: samples.length, draw_ns: capture.record.draw_ns - origin, present_ns: feedback.present_ns - origin,
      capture_counter: capture.counter, capture_timestamp_us: intake.capture_timestamp_us, capturer_id: capture.capturer,
      display_trace_id: capture.record.display_trace_id, request_generation: capture.record.request_generation,
      frame_identity: key, feedback_id: feedbackId, output_sequence: sequence, flags: feedback.flags, refresh_ns: feedback.refresh_ns });
  }
  for (const capture of deliveryByIdentity.values()) if (!usedDeliveries.has(capture.key)) {
    capture.record.delivered_without_intake = true;
    count("delivered_without_intake");
  }
  requireValue(clockIds.size === 1, "native presentation clock changed during capture");
  nativeOrder.sort((a, b) => a.present_ns - b.present_ns);
  requireValue(nativeOrder.every((event, index) => index === 0 || BigInt(event.sequence) > BigInt(nativeOrder[index - 1].sequence)), "native output sequence reversed across presentations");
  requireValue(calibration.length >= 2, "at least two measured native clock calibration points required");
  calibration.sort((a, b) => a.source_ns - b.source_ns);
  requireValue(calibration.every((point, index) => index === 0 || point.source_ns > calibration[index - 1].source_ns), "duplicate or reversed calibration points");
  samples.sort((a, b) => a.draw_ns - b.draw_ns);
  for (const [index, sample] of samples.entries()) sample.sequence = index;
  const cohorts = { before_observation: {}, warmup: {}, measurement: {}, drain: {} };
  for (const record of records) {
    const time = record.intake_ns;
    record.window = time < origin ? "before_observation" : time < start ? "warmup" : time < end ? "measurement" : "drain";
    const cohort = cohorts[record.window];
    cohort[record.status] = (cohort[record.status] ?? 0) + 1;
  }
  const captureCohorts = { before_observation: {}, warmup: {}, measurement: {}, drain: {} };
  for (const record of captureRecords) {
    const time = record.first_ns;
    record.window = time < origin ? "before_observation" : time < start ? "warmup" : time < end ? "measurement" : "drain";
    const cohort = captureCohorts[record.window];
    for (const label of [record.kind, ...(record.dropped ? ["dropped"] : []), ...(record.delivered_without_intake ? ["delivered_without_intake"] : [])]) {
      cohort[label] = (cohort[label] ?? 0) + 1;
    }
  }
  const uncertainty = 1000 + Math.max(...calibration.map(point => point.max_error_ns));
  return { kind: "browser_draw_to_present", configuration: "C", qualification: "NOT_EVALUATED",
    measurement_status: errors.length ? "INCOMPLETE" : "COMPLETE", clock: "CLOCK_MONOTONIC", uncertainty_ns: uncertainty,
    presentation_observation: "compositor_feedback", samples, errors, records, capture_records: captureRecords,
    calibration: { clock: "CLOCK_MONOTONIC", max_error_ns: uncertainty, points: calibration.map(({ source_ns, mapped_ns }) => ({ source_ns, mapped_ns })) },
    missed_frames: countMissedFrames({ presentations: slotPresentations, start_ns: start, end_ns: end,
      refresh_hz: options.refresh_hz, uncertainty_ns: uncertainty }),
    diagnostics: { gpu_pid: options.gpu_pid, capturer_id: chromium.capturer, counts, intake_cohorts: cohorts, capture_origin_cohorts: captureCohorts, measured_frames: samples.length,
      idle_window: samples.length === 0, observation_start_ns: start, observation_end_ns: end,
      trace_requests: chromium.requests.length, unfinished_requests: chromium.requests.filter(request => !request.end).length } };
}

export async function readIntegratedBrowserPresentation(tracePath, eventsPath, options) {
  const sizes = await Promise.all([stat(tracePath), stat(eventsPath)]);
  requireValue(sizes[0].size <= 512 * 1024 * 1024 && sizes[1].size <= 128 * 1024 * 1024, "raw evidence exceeds parser limits");
  const [trace, events] = await Promise.all([readFile(tracePath, "utf8"), readFile(eventsPath, "utf8")]);
  requireValue(events.endsWith("\n"), "truncated native JSONL evidence");
  return extractIntegratedBrowserPresentation(JSON.parse(trace), events.trim().split("\n").map(line => JSON.parse(line)), options);
}
