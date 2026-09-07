import { readFile, stat } from "node:fs/promises";
import { countMissedFrames } from "./missed-frames.mjs";

const drawName = "Graphics.Pipeline.DrawAndSwap";
const displayedName = "Display::FrameDisplayed";
const traceLimit = 512 * 1024 * 1024;
const stderrLimit = 32 * 1024 * 1024;
const unsafeStatistic = /data_loss|packet_loss|overwritten|chunks_discarded|incremental_sequences_dropped|abi_violations|patches_failed|json_(parser|tokenizer)_failure|clock_sync_failure|invalid_clock_snapshots|packet_skipped|tokenizer_errors/;

function requireValue(condition, message) {
  if (!condition) throw new Error(`browser presentation proof: ${message}`);
}

function integer(value, name, minimum = 0) {
  requireValue(Number.isSafeInteger(value) && value >= minimum, `invalid ${name}`);
  return value;
}

function checkStatistics(value, path = "trace_processor_stats") {
  requireValue(value && typeof value === "object", `missing ${path}`);
  for (const [key, entry] of Object.entries(value)) {
    if (entry && typeof entry === "object") checkStatistics(entry, `${path}.${key}`);
    else if (unsafeStatistic.test(key)) requireValue(entry === 0, `trace loss or corruption: ${path}.${key}=${entry}`);
  }
}

function outputProtocolState() {
  const active = new Map();
  const retired = new Set();
  const generations = new Map();
  const history = [];
  return {
    history,
    read(line, lineNumber) {
      const deleted = /wl_display#1\.delete_id\((\d+)\)$/.exec(line);
      if (deleted) {
        const id = Number(deleted[1]);
        requireValue(!active.has(id), "live wl_output ID deleted without release");
        retired.delete(id);
      }
      const bind = /-> wl_registry#(\d+)\.bind\((\d+), "wl_output", (\d+), new id (?:\[unknown\]|wl_output)#(\d+)\)$/.exec(line);
      if (bind) {
        const id = Number(bind[4]);
        requireValue(!active.has(id), `ambiguous live wl_output object ${id}; connection identity is missing`);
        requireValue(!retired.has(id), "wl_output ID reused before delete_id confirmation");
        requireValue(Number(bind[3]) >= 4, "wl_output version does not provide stable output names");
        const generation = (generations.get(id) ?? 0) + 1;
        generations.set(id, generation);
        active.set(id, { wl_output_id: id, generation, registry_id: Number(bind[1]), registry_global_name: Number(bind[2]), bind_line: lineNumber });
      }
      const event = /wl_output#(\d+)\.(name|mode|scale|done|release)\((.*)\)$/.exec(line);
      if (!event) return;
      const id = Number(event[1]);
      const output = active.get(id);
      requireValue(output, `wl_output ${id} has no unique live binding`);
      const method = event[2];
      const args = event[3];
      if (method === "release") {
        requireValue(args === "" && /->\s+wl_output#/.test(line), "invalid wl_output release");
        active.delete(id);
        retired.add(id);
        return;
      }
      if (method === "name") {
        requireValue(/^"[^"\\]+"$/.test(args), "invalid wl_output name");
        output.name = args.slice(1, -1);
        delete output.done_line;
      } else if (method === "scale") {
        requireValue(/^\d+$/.test(args), "invalid wl_output scale");
        output.scale = integer(Number(args), "wl_output scale", 1);
        delete output.done_line;
      } else if (method === "mode") {
        requireValue(/^\d+(, \d+){3}$/.test(args), "invalid wl_output mode");
        const [flags, width, height, refresh] = args.split(", ").map(Number);
        if ((flags & 1) !== 0) {
          Object.assign(output, { width_px: integer(width, "output width", 1), height_px: integer(height, "output height", 1), refresh_millihz: integer(refresh, "output refresh", 1) });
          delete output.done_line;
        }
      } else {
        requireValue(args === "", "invalid wl_output done");
        output.done_line = lineNumber;
        history.push({ ...output });
      }
    },
    snapshot(id) {
      const output = active.get(id);
      requireValue(output?.done_line && output.name && output.width_px && output.height_px && output.refresh_millihz && output.scale, `output ${id} metadata is missing or incomplete`);
      return { ...output };
    },
  };
}

export function parseWaylandPresentation(stderr, options = {}) {
  requireValue(typeof stderr === "string" && Buffer.byteLength(stderr) <= stderrLimit, "invalid or oversized Wayland log");
  requireValue(stderr.endsWith("\n"), "truncated Wayland log");
  const tagged = /\[pid=/.test(stderr);
  requireValue(tagged || !options.require_connection_identity, "missing Wayland PID and connection identity");
  if (!tagged) return parseWaylandConnection(stderr, options);
  const namespaces = new Map();
  const expression = /^(\[\s*[\d.]+\]\s*)\[pid=([1-9]\d*) connection=([1-9]\d*)\]\s*(.*)$/;
  for (const [index, line] of stderr.split("\n").entries()) {
    const match = expression.exec(line);
    if (!match) {
      requireValue(!/\[pid=/.test(line) && !/^\[\s*[\d.]+\]\s*(?:\{[^}]+\}\s*)?(?:->\s*|discarded\s*)?(?:wl_output|wl_registry|wp_presentation(?:_feedback)?)#/.test(line), `untagged or corrupt Wayland protocol line ${index + 1}`);
      continue;
    }
    const pid = integer(Number(match[2]), "Wayland process PID", 1);
    const connection = match[3];
    requireValue(BigInt(connection) <= 0xffffffffffffffffn, "Wayland connection identity exceeds uint64");
    const key = `${pid}:${connection}`;
    const group = namespaces.get(key) ?? { pid, connection_id: connection, lines: [], source_lines: [] };
    group.lines.push(match[1] + match[4]);
    group.source_lines.push(index + 1);
    namespaces.set(key, group);
    requireValue(namespaces.size <= 128, "too many Wayland connection namespaces");
  }
  const result = { presented: [], discarded: [], unfinished_feedbacks: [], output_history: [], connections: [] };
  for (const group of namespaces.values()) {
    const log = group.lines.join("\n") + "\n";
    if (!/^\[\s*[\d.]+\]\s*(?:\{[^}]+\}\s*)?(?:->\s*)?(?:wp_presentation#\d+\.(?:clock_id|feedback)|wp_presentation_feedback#\d+\.)/m.test(log)) continue;
    const parsed = parseWaylandConnection(log, options);
    const count = parsed.presented.length + parsed.discarded.length + parsed.unfinished_feedbacks.length;
    if (count > 0 && options.require_connection_identity) requireValue(Array.isArray(options.observed_pids) && options.observed_pids.includes(group.pid), "Wayland feedback PID is not an observed host or GPU process");
    const output = entry => ({ ...entry, bind_line: group.source_lines[entry.bind_line - 1], done_line: group.source_lines[entry.done_line - 1], wayland_pid: group.pid, connection_id: group.connection_id });
    for (const key of ["presented", "discarded", "unfinished_feedbacks"]) {
      for (const entry of parsed[key]) result[key].push({ ...entry, wayland_pid: group.pid, connection_id: group.connection_id,
        request_line: group.source_lines[entry.request_line - 1], ...(entry.event_line ? { event_line: group.source_lines[entry.event_line - 1] } : {}),
        ...(entry.output_metadata ? { output_metadata: entry.output_metadata.map(output) } : {}) });
    }
    result.output_history.push(...(parsed.output_history ?? []).map(output));
    result.connections.push({ wayland_pid: group.pid, connection_id: group.connection_id, feedback_count: count });
  }
  if (options.require_connection_identity) requireValue(result.connections.filter(group => group.feedback_count > 0).length === 1, "Wayland feedback connection is absent or ambiguous");
  for (const key of ["presented", "discarded"]) result[key].sort((left, right) => left.event_line - right.event_line);
  requireValue(result.connections.length > 0, "missing native presentation clock");
  return result;
}

function parseWaylandConnection(stderr, options) {
  requireValue(typeof stderr === "string" && Buffer.byteLength(stderr) <= stderrLimit, "invalid or oversized Wayland log");
  requireValue(stderr.endsWith("\n"), "truncated Wayland log");
  const outputState = options.require_output_metadata ? outputProtocolState() : null;
  const clocks = new Map();
  const active = new Map();
  const generations = new Map();
  const presented = [];
  const discarded = [];
  const managerExpression = /^\[\s*[\d.]+\]\s*(->\s*)?wp_presentation#(\d+)\.(clock_id|feedback|destroy)\((.*)\)$/;
  const feedbackExpression = /^\[\s*[\d.]+\]\s*wp_presentation_feedback#(\d+)\.(presented|discarded|sync_output)\((.*)\)$/;
  for (const [index, rawLine] of stderr.split("\n").entries()) {
    const line = rawLine.replace(/^(\[\s*[\d.]+\]\s*)\{[^}]+\}\s*/, "$1");
    outputState?.read(line, index + 1);
    const manager = managerExpression.exec(line);
    if (manager) {
      const id = Number(manager[2]);
      if (manager[3] === "clock_id") {
        requireValue(!manager[1] && /^\d+$/.test(manager[4]), `corrupt presentation clock at line ${index + 1}`);
        requireValue(!clocks.has(id) || clocks.get(id) === Number(manager[4]), "presentation clock changed");
        clocks.set(id, Number(manager[4]));
      } else if (manager[3] === "feedback") {
        const argumentsMatch = /^wl_surface#(\d+), new id wp_presentation_feedback#(\d+)$/.exec(manager[4]);
        requireValue(manager[1] && argumentsMatch, `corrupt feedback request at line ${index + 1}`);
        requireValue(clocks.get(id) === 1, "native presentation clock must be Linux CLOCK_MONOTONIC");
        const feedbackId = Number(argumentsMatch[2]);
        requireValue(!active.has(feedbackId), `ambiguous live feedback object ${feedbackId}`);
        const generation = (generations.get(feedbackId) ?? 0) + 1;
        generations.set(feedbackId, generation);
        active.set(feedbackId, { feedback_id: feedbackId, feedback_generation: generation, manager_id: id, wl_surface: Number(argumentsMatch[1]), request_line: index + 1, outputs: [], ...(outputState ? { output_metadata: [] } : {}) });
      }
      continue;
    }
    const feedback = feedbackExpression.exec(line);
    if (feedback) {
      const id = Number(feedback[1]);
      const request = active.get(id);
      requireValue(request, `feedback ${id} has no unique live request at line ${index + 1}`);
      if (feedback[2] === "sync_output") {
        const output = /^wl_output#(\d+)$/.exec(feedback[3]);
        requireValue(output, `corrupt output association at line ${index + 1}`);
        request.outputs.push(Number(output[1]));
        if (outputState) request.output_metadata.push(outputState.snapshot(Number(output[1])));
        continue;
      }
      active.delete(id);
      if (feedback[2] === "discarded") {
        requireValue(feedback[3] === "", "corrupt discarded feedback");
        discarded.push({ ...request, event_line: index + 1 });
        continue;
      }
      requireValue(/^\d+(, \d+){6}$/.test(feedback[3]), `corrupt native timestamp at line ${index + 1}`);
      const fields = feedback[3].split(", ").map(Number);
      requireValue(fields.every((value) => Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff), "native timestamp field exceeds uint32");
      const [secondsHigh, secondsLow, nanoseconds, refresh, sequenceHigh, sequenceLow, flags] = fields;
      requireValue(nanoseconds < 1_000_000_000, "invalid native nanoseconds field");
      const nativeNs = (BigInt(secondsHigh) << 32n | BigInt(secondsLow)) * 1_000_000_000n + BigInt(nanoseconds);
      requireValue(nativeNs <= BigInt(Number.MAX_SAFE_INTEGER), "native presentation timestamp exceeds safe integer range");
      presented.push({ ...request, event_line: index + 1, present_ns: Number(nativeNs), refresh_ns: refresh, output_sequence: (BigInt(sequenceHigh) << 32n | BigInt(sequenceLow)).toString(), flags });
      continue;
    }
    if (/wp_presentation(?:_feedback)?#\d+\./.test(line)) {
      requireValue(/(?:\{[^}]+\} )?discarded wp_presentation#\d+\.clock_id\(\d+\)$/.test(line), `unparsed presentation protocol line ${index + 1}`);
    }
  }
  requireValue(clocks.size > 0, "missing native presentation clock");
  return { presented, discarded, unfinished_feedbacks: [...active.values()], ...(outputState ? { output_history: outputState.history } : {}) };
}

function traceFrames(trace, startNs, endNs, expectedGpuPid) {
  requireValue(trace && Array.isArray(trace.traceEvents) && trace.traceEvents.length <= 4_000_000, "invalid or oversized trace event collection");
  requireValue(trace.metadata?.["clock-domain"] === "LINUX_CLOCK_MONOTONIC", "CEF trace must declare LINUX_CLOCK_MONOTONIC");
  checkStatistics(trace.metadata?.trace_processor_stats);
  const gpuPids = new Set(trace.traceEvents.filter((event) => event.name === "process_name" && event.ph === "M" && event.args?.name === "GPU Process").map((event) => integer(event.pid, "GPU process PID", 1)));
  requireValue(gpuPids.size === 1, "GPU process metadata is absent or ambiguous");
  const [gpuPid] = gpuPids;
  requireValue(expectedGpuPid === undefined || expectedGpuPid === gpuPid, "GPU trace PID differs from observed sandbox process");
  const relevant = trace.traceEvents.filter((event) => [drawName, displayedName].includes(event.name));
  for (const event of relevant) {
    requireValue(event.pid === gpuPid, "presentation trace event is not from the identified GPU process");
    integer(event.tid, "trace thread ID", 1);
    integer(event.ts, "trace timestamp");
    integer(event.ts * 1000, "nanosecond trace timestamp");
  }
  const ordered = relevant.map((event, index) => ({ event, index })).sort((left, right) => left.event.ts - right.event.ts || left.index - right.index);
  const active = new Map();
  const occurrences = new Map();
  const frames = [];
  const displayed = new Map();
  for (const { event } of ordered) {
    if (event.name === displayedName) {
      requireValue(event.ph === "I" || event.ph === "i", "unexpected FrameDisplayed phase");
      const key = `${event.pid}:${event.tid}:${event.ts}`;
      requireValue(!displayed.has(key), `duplicate FrameDisplayed timestamp ${event.ts}`);
      displayed.set(key, event);
      continue;
    }
    requireValue(["b", "e"].includes(event.ph) && typeof event.id2?.local === "string" && /^0x[0-9a-f]+$/i.test(event.id2.local), "invalid DrawAndSwap async event identity");
    const key = `${event.pid}:${event.id2.local.toLowerCase()}`;
    if (event.ph === "b") {
      requireValue(!active.has(key), `duplicate active frame identity ${key}`);
      const generation = (occurrences.get(key) ?? 0) + 1;
      occurrences.set(key, generation);
      active.set(key, { start: event, generation });
    } else {
      const begin = active.get(key);
      if (!begin) {
        requireValue(event.ts * 1000 < startNs || event.ts * 1000 > endNs, `frame end has no beginning inside capture: ${key}`);
        continue;
      }
      active.delete(key);
      requireValue(event.ts > begin.start.ts, `reversed or zero-duration frame ${key}`);
      frames.push({ start: begin.start, end: event, generation: begin.generation });
    }
  }
  for (const { start } of active.values()) requireValue(start.ts * 1000 > endNs, `unfinished frame overlaps capture: ${start.id2.local}`);
  return { gpuPid, frames, displayed, unfinished_frames: active.size };
}

export function extractBrowserPresentation(trace, stderr, options) {
  const origin = integer(options?.origin_ns, "capture origin");
  const warmup = integer(options.warmup_ns ?? 10_000_000_000, "warmup duration");
  const duration = integer(options.duration_ns ?? 60_000_000_000, "measurement duration", 1);
  const startNs = integer(origin + warmup, "capture start");
  const endNs = integer(startNs + duration, "capture end");
  requireValue(startNs >= origin && endNs > startNs, "invalid observation window");
  const chromium = traceFrames(trace, startNs, endNs, options.gpu_pid);
  const native = parseWaylandPresentation(stderr, { require_output_metadata: options.fullscreen === true, require_connection_identity: options.fullscreen === true, observed_pids: [chromium.gpuPid, options.host_pid].filter(pid => pid !== undefined) });
  const byMicrosecond = new Map();
  for (const feedback of native.presented) {
    const key = Math.floor(feedback.present_ns / 1000);
    const candidates = byMicrosecond.get(key) ?? [];
    candidates.push(feedback);
    byMicrosecond.set(key, candidates);
  }
  const samples = [];
  const slotPresentations = [];
  const matchedNative = new Set();
  const matchedDisplayed = new Set();
  const surfaces = new Map();
  const calibrationPoints = [];
  for (const frame of chromium.frames) {
    const drawNs = frame.start.ts * 1000;
    const traceEndNs = frame.end.ts * 1000;
    const inside = drawNs >= startNs && traceEndNs <= endNs;
    const displayKey = `${frame.end.pid}:${frame.end.tid}:${frame.end.ts}`;
    const candidates = byMicrosecond.get(frame.end.ts) ?? [];
    if (!chromium.displayed.has(displayKey) || candidates.length !== 1) {
      requireValue(!inside, `frame ${frame.start.id2.local} has missing or ambiguous native feedback / FrameDisplayed`);
      continue;
    }
    const feedback = candidates[0];
    requireValue(!matchedNative.has(feedback), "native feedback is associated with multiple frames");
    requireValue(!matchedDisplayed.has(displayKey), "FrameDisplayed is associated with multiple frames");
    matchedNative.add(feedback);
    matchedDisplayed.add(displayKey);
    surfaces.set(`${feedback.wayland_pid ?? "legacy"}:${feedback.connection_id ?? "legacy"}:${feedback.wl_surface}`, feedback);
    const hardwareClock = (feedback.flags & 7) === 7 && feedback.outputs.length > 0;
    requireValue(!inside || hardwareClock, "presentation lacks VSYNC, hardware clock, hardware completion, or output association");
    if (hardwareClock) calibrationPoints.push({ source_ns: traceEndNs, mapped_ns: feedback.present_ns });
    if (hardwareClock) slotPresentations.push({ present_ns: feedback.present_ns, refresh_ns: feedback.refresh_ns,
      output_sequence: feedback.output_sequence, fresh: true,
      content_id: `${frame.start.pid}:${frame.start.id2.local}:${frame.generation}` });
    if (!inside || feedback.present_ns >= endNs) continue;
    samples.push({ sequence: samples.length, draw_ns: drawNs - origin, present_ns: feedback.present_ns - origin, pid: frame.start.pid, tid: frame.start.tid, frame_id: frame.start.id2.local, frame_generation: frame.generation, feedback_id: feedback.feedback_id, feedback_generation: feedback.feedback_generation, wl_surface: feedback.wl_surface, ...(feedback.connection_id ? { wayland_pid: feedback.wayland_pid, connection_id: feedback.connection_id } : {}), outputs: feedback.outputs, ...(feedback.output_metadata ? { output_metadata: feedback.output_metadata } : {}), output_sequence: feedback.output_sequence, flags: feedback.flags, refresh_ns: feedback.refresh_ns });
  }
  requireValue(surfaces.size === 1, "browser presentation surface is absent or ambiguous");
  const [surfaceFeedback] = surfaces.values();
  const surface = surfaceFeedback.wl_surface;
  const sameSurface = entry => entry.wl_surface === surface && entry.wayland_pid === surfaceFeedback.wayland_pid && entry.connection_id === surfaceFeedback.connection_id;
  for (const [key, event] of chromium.displayed) {
    if (event.ts * 1000 >= startNs && event.ts * 1000 <= endNs) requireValue(matchedDisplayed.has(key), "FrameDisplayed inside capture has no complete frame association");
  }
  for (const feedback of native.presented) {
    if (sameSurface(feedback) && feedback.present_ns >= startNs && feedback.present_ns <= endNs) requireValue(matchedNative.has(feedback), "native presentation inside capture has no Chromium frame association");
  }
  const discardedBounds = [];
  for (const discarded of native.discarded.filter(sameSurface)) {
    const before = native.presented.findLast((entry) => sameSurface(entry) && entry.event_line < discarded.event_line)?.present_ns ?? -Infinity;
    const after = native.presented.find((entry) => sameSurface(entry) && entry.event_line > discarded.event_line)?.present_ns ?? Infinity;
    discardedBounds.push({ ...discarded, before_present_ns: Number.isFinite(before) ? before : null,
      after_present_ns: Number.isFinite(after) ? after : null,
      window: after < startNs ? "warmup" : before >= endNs ? "drain" : before >= startNs && after < endNs ? "measurement" : "boundary_ambiguous" });
  }
  samples.sort((left, right) => left.draw_ns - right.draw_ns);
  let previousOutputSequence = -1n;
  let measuredOutputs;
  for (const [index, sample] of samples.entries()) {
    sample.sequence = index;
    requireValue(index === 0 || sample.draw_ns > samples[index - 1].draw_ns, "ambiguous equal frame start timestamps");
    const outputs = [...new Set(sample.outputs)].sort((left, right) => left - right).join(",");
    requireValue(measuredOutputs === undefined || outputs === measuredOutputs, "presentation output changed during capture");
    measuredOutputs = outputs;
    requireValue(BigInt(sample.output_sequence) > previousOutputSequence, "native output sequence is duplicated or reversed");
    previousOutputSequence = BigInt(sample.output_sequence);
  }
  let displayEvidence;
  if (options.fullscreen === true) {
    const associated = native.presented.filter(entry => sameSurface(entry) && entry.present_ns <= endNs && entry.present_ns >= origin);
    requireValue(associated.length >= 2, "fullscreen output requires native correspondence during observation");
    let identity;
    for (const feedback of associated) {
      requireValue(feedback.output_metadata?.length === 1, "fullscreen presentation output is absent or ambiguous");
      const output = feedback.output_metadata[0];
      requireValue(output.name === options.expected_output, "native output differs from expected output");
      requireValue(output.width_px === 1920 && output.height_px === 1080 && output.scale === 1, "native output mode or scale differs from 1920x1080 scale 1");
      requireValue(Number.isFinite(options.refresh_actual_hz) && Math.abs(output.refresh_millihz / 1000 - options.refresh_actual_hz) <= 0.001, "native output mode refresh differs from display condition");
      requireValue(Math.abs(1e9 / feedback.refresh_ns - options.refresh_actual_hz) <= 0.1, "native feedback refresh differs from display condition");
      const key = JSON.stringify([output.wl_output_id, output.generation, output.registry_global_name, output.name, output.width_px, output.height_px, output.scale, output.refresh_millihz]);
      requireValue(identity === undefined || identity === key, "fullscreen output identity changed during observation");
      identity = key;
    }
    displayEvidence = { wl_surface: surface, wayland_pid: surfaceFeedback.wayland_pid, connection_id: surfaceFeedback.connection_id, output: associated[0].output_metadata[0], observation_start_ns: origin, observation_end_ns: endNs };
  }
  calibrationPoints.sort((left, right) => left.source_ns - right.source_ns);
  requireValue(calibrationPoints.length >= 2, "at least two native/CEF clock correspondence points are required, including warmup for idle fixtures");
  return {
    kind: "browser_draw_to_present", qualification: "NOT_EVALUATED", clock: "CLOCK_MONOTONIC", uncertainty_ns: 1000,
    presentation_observation: "compositor_feedback", samples, ...(displayEvidence ? { display_evidence: displayEvidence } : {}),
    missed_frames: countMissedFrames({ presentations: slotPresentations, start_ns: startNs, end_ns: endNs,
      refresh_hz: options.refresh_actual_hz, uncertainty_ns: 1000 }),
    discarded_bounds: discardedBounds,
    calibration: { clock: "CLOCK_MONOTONIC", max_error_ns: 1000, points: calibrationPoints },
    diagnostics: { gpu_pid: chromium.gpuPid, ...(surfaceFeedback.connection_id ? { wayland_pid: surfaceFeedback.wayland_pid, connection_id: surfaceFeedback.connection_id } : {}), wl_surface: surface, measured_frames: samples.length, idle_window: samples.length === 0, complete_trace_frames: chromium.frames.length, native_presentations: native.presented.length, matched_native_presentations: matchedNative.size, discarded_native_frames: native.discarded.length, discarded_bounds: discardedBounds, unfinished_trace_frames: chromium.unfinished_frames, unfinished_native_feedbacks: native.unfinished_feedbacks.length },
  };
}

export async function readBrowserPresentation(tracePath, stderrPath, options) {
  const [traceInfo, stderrInfo] = await Promise.all([stat(tracePath), stat(stderrPath)]);
  requireValue(traceInfo.size <= traceLimit && stderrInfo.size <= stderrLimit, "raw capture exceeds parser limits");
  const [trace, stderr] = await Promise.all([readFile(tracePath, "utf8"), readFile(stderrPath, "utf8")]);
  return extractBrowserPresentation(JSON.parse(trace), stderr, options);
}
