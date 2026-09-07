import { createHash } from "node:crypto";

const integer = value => Number.isSafeInteger(value) && value >= 0;
const positive = value => integer(value) && value > 0;
const assert = (condition, message) => { if (!condition) throw new Error(message); };
const canonical = value => JSON.stringify(value, function (_key, item) {
  return item && typeof item === "object" && !Array.isArray(item)
    ? Object.fromEntries(Object.entries(item).sort(([left], [right]) => left.localeCompare(right))) : item;
});
const equal = (left, right) => canonical(left) === canonical(right);
const close = (left, right, tolerance) => Number.isFinite(left) && Math.abs(left - right) <= tolerance;
const outputIdentity = output => Object.fromEntries(Object.entries(output).filter(([key]) => key !== "done_ns"));

export { inspectReceipt as inspectDisplayReceipt };

export function waylandDisplayUuid(name) {
  const bytes = createHash("sha1").update(Buffer.from("6ba7b8109dad11d180b400c04fd430c8", "hex")).update(name, "utf8").digest().subarray(0, 16);
  bytes[6] = (bytes[6] & 15) | 80;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

const physicalOutputIdentity = output => Object.fromEntries(Object.entries(output).filter(([key]) => !["done_ns", "wl_output_id"].includes(key)));

function inspectReceipt(receipt, condition, label) {
  assert(receipt?.schema_version === 1 && integer(receipt.monotonic_ns), `${label} receipt or timestamp is invalid`);
  assert(positive(receipt.state?.serial) && receipt.serial_guard === receipt.state.serial, `${label} display serial guard is invalid`);
  assert(Array.isArray(receipt.monitor_change_events) && receipt.monitor_change_events.length === 0, `${label} contains display changes`);
  const plan = receipt.verified_plan;
  assert([1, 2].includes(plan?.layout_mode) && Array.isArray(plan.outputs) && plan.outputs.length === 2, `${label} must describe two configured outputs`);
  assert(receipt.state.properties?.["layout-mode"] === plan.layout_mode, `${label} raw layout mode differs from its plan`);
  const { monitors, logical_monitors: logical } = receipt.state;
  assert(Array.isArray(monitors) && monitors.length === 2 && Array.isArray(logical) && logical.length === 2, `${label} raw output inventory is invalid`);
  assert(new Set(plan.outputs.map(output => output.connector)).size === 2 && plan.outputs.filter(output => output.primary === true).length === 1, `${label} output identity or primary is invalid`);
  for (const output of plan.outputs) {
    assert(typeof output.connector === "string" && Array.isArray(output.spec) && output.spec.length === 4 && output.spec[0] === output.connector, `${label} lacks a physical output identity`);
    assert(output.scale === 1 && output.transform === 0 && Number.isSafeInteger(output.x) && Number.isSafeInteger(output.y) && typeof output.primary === "boolean", `${label} output geometry is invalid`);
    assert(close(output.refresh_actual_hz, condition.refresh_actual_hz, 0.0001), `${label} actual refresh differs from the condition`);
    const physical = monitors.filter(monitor => equal(monitor.spec, output.spec));
    assert(physical.length === 1 && Array.isArray(physical[0].modes), `${label} physical output does not match its plan`);
    const modes = physical[0].modes.filter(mode => mode.properties?.["is-current"]);
    assert(modes.length === 1 && modes[0].id === output.mode && modes[0].width === 1920 && modes[0].height === 1080
      && close(modes[0].refresh_hz, condition.refresh_actual_hz, 0.0001)
      && (modes[0].properties?.["refresh-rate-mode"] ?? "fixed") === "fixed" && !output.mode.includes("+vrr"), `${label} current physical mode differs from fixed 1920x1080`);
    const matches = logical.filter(item => item.monitors?.length === 1 && equal(item.monitors[0], output.spec));
    assert(matches.length === 1 && ["x", "y", "scale", "transform", "primary"].every(key => matches[0][key] === output[key]), `${label} logical output is mirrored or differs from its plan`);
  }
  const [left, right] = plan.outputs;
  assert(left.x + 1920 <= right.x || right.x + 1920 <= left.x || left.y + 1080 <= right.y || right.y + 1080 <= left.y, `${label} output rectangles overlap`);
  for (const name of [condition.terminal_output, condition.browser_output].filter(Boolean)) {
    assert(plan.outputs.some(output => output.connector === name), `${label} does not contain requested output ${name}`);
  }
  return plan;
}

function inspectOutput(output, condition, names, time) {
  assert(output && names.has(output.name) && positive(output.wl_output_id) && positive(output.registry_global_name), "native output identity is invalid");
  assert(output.width_px === 1920 && output.height_px === 1080 && output.scale === 1
    && close(output.refresh_millihz / 1000, condition.refresh_actual_hz, 0.001), "native output mode, scale or actual refresh differs from the condition");
  assert(integer(output.done_ns) && output.done_ns <= time, "native output metadata timestamp is invalid");
}

function inspectRole(events, role, name, condition, names, origin, end, completionTime) {
  const prefix = role === "browser" ? "browser_" : "";
  const types = { viewport: `${prefix}viewport`, paint: `${prefix}paint`, presented: `${prefix}presented` };
  const viewports = events.filter(event => event.event === types.viewport).sort((left, right) => left.at_ns - right.at_ns);
  assert(viewports.every(event => integer(event.at_ns)), `${role} viewport has an invalid timestamp`);
  const initial = viewports.filter(event => event.at_ns <= origin).at(-1);
  assert(initial && positive(initial.wl_surface_id), `${role} lacks an initial Wayland viewport`);
  const surface = initial.wl_surface_id;
  const displayUuid = waylandDisplayUuid(name);
  const displayId = initial.display_id;
  assert(positive(displayId) && initial.display_uuid === displayUuid, `${role} GPUI display identity does not match its requested output`);
  const identity = event => {
    assert(event.role === role && event.wl_surface_id === surface && event.fullscreen === true, `${role} fullscreen or wl_surface identity changed`);
    assert(event.display_id === displayId && event.display_uuid === displayUuid, `${role} GPUI display identity changed`);
  };
  for (const viewport of [initial, ...viewports.filter(event => event.at_ns > origin && event.at_ns <= end)]) {
    identity(viewport);
    assert(viewport.width_px === 1920 && viewport.height_px === 1080 && viewport.scale === 1, `${role} viewport differs from 1920x1080 scale 1`);
  }
  const ready = events.filter(event => event.event === "presentation_ready" && event.role === role);
  assert(ready.every(event => integer(event.at_ns)), `${role} presentation observer has an invalid timestamp`);
  assert(ready.some(event => event.wl_surface_id === surface && event.clock_id === 1 && event.at_ns <= origin), `${role} lacks a CLOCK_MONOTONIC presentation observer`);
  assert(ready.filter(event => event.at_ns >= origin && event.at_ns <= end).every(event => event.wl_surface_id === surface && event.clock_id === 1), `${role} presentation observer clock or surface changed`);
  const outputs = events.filter(event => event.event === "output" && event.role === role).sort((left, right) => left.at_ns - right.at_ns);
  assert(outputs.every(event => integer(event.at_ns)), `${role} output event has an invalid timestamp`);
  const initialOutputs = new Map();
  for (const event of outputs.filter(event => event.at_ns <= origin)) initialOutputs.set(event.output?.name, event);
  assert(initialOutputs.has(name), `${role} lacks initial metadata for its requested output`);
  for (const event of [...initialOutputs.values(), ...outputs.filter(event => event.at_ns > origin && event.at_ns <= end)]) {
    assert(event.wl_surface_id === surface, `${role} output event identifies a different wl_surface`);
    inspectOutput(event.output, condition, names, event.at_ns);
    const baseline = initialOutputs.get(event.output.name)?.output;
    assert(baseline && equal(outputIdentity(event.output), outputIdentity(baseline)), `${role} output metadata changed during capture`);
  }
  const removals = events.filter(event => event.event === "output_removed" && event.role === role);
  assert(removals.every(event => integer(event.at_ns)), `${role} output removal has an invalid timestamp`);
  assert(!removals.some(event => event.at_ns >= origin && event.at_ns <= end), `${role} output was removed during capture`);
  const paints = events.filter(event => event.event === types.paint);
  assert(paints.every(event => integer(event.at_ns) && positive(event.feedback_id)), `${role} paint identity or timestamp is invalid`);
  const paintMap = new Map();
  for (const paint of paints) {
    assert(!paintMap.has(paint.feedback_id), `${role} duplicate paint feedback`);
    paintMap.set(paint.feedback_id, paint);
    if (paint.at_ns >= origin && paint.at_ns <= end) identity(paint);
  }
  const selectedPaints = paints.filter(event => event.at_ns >= origin && event.at_ns <= end);
  assert(selectedPaints.length > 0, `${role} has no paint during capture`);
  const feedbacks = new Set();
  const resolvedIds = new Set();
  const globalOutputs = events.filter(event => event.event === "output").sort((left, right) => left.at_ns - right.at_ns);
  let presented = 0;
  let presentedInInterval = 0;
  for (const event of events.filter(event => event.event === types.presented)) {
    assert(integer(event.present_ns) && positive(event.feedback_id), `${role} presentation identity or timestamp is invalid`);
    const paint = paintMap.get(event.feedback_id);
    const selected = (event.present_ns >= origin && event.present_ns <= end) || (paint?.at_ns >= origin && paint.at_ns <= end);
    if (!selected) continue;
    assert(paint && !feedbacks.has(event.feedback_id), `${role} presentation does not uniquely identify a paint`);
    feedbacks.add(event.feedback_id);
    identity(paint);
    identity(event);
    const callback = role === "browser" ? event.presentation_callback_ns : event.callback_ns;
    const calibration = event.calibration;
    assert([event.native_present_ns, callback, event.sequence].every(integer) && event.clock_id === 1 && positive(event.refresh_ns)
      && calibration && [calibration.source_ns, calibration.mapped_ns, calibration.max_error_ns].every(integer)
      && calibration.max_error_ns <= (role === "browser" ? 1_000_000 : 500_000)
      && event.present_ns === event.native_present_ns + calibration.mapped_ns - calibration.source_ns
      && paint.at_ns <= event.present_ns && event.present_ns <= callback + calibration.max_error_ns, `${role} presentation clock or causal timestamps are invalid`);
    assert(completionTime == null || callback <= completionTime, `${role} presentation callback exceeds display completion`);
    assert(close(1e9 / event.refresh_ns, condition.refresh_actual_hz, 0.1), `${role} native presentation actual refresh differs from the condition`);
    assert(typeof event.flags === "string" && !event.flags.includes("Unknown")
      && ["Vsync", "HwClock", "HwCompletion"].every(flag => new RegExp(`\\b${flag}\\b`).test(event.flags)), `${role} presentation lacks Vsync, HwClock or HwCompletion`);
    assert(Array.isArray(event.unknown_output_ids) && event.unknown_output_ids.every(positive)
      && Array.isArray(event.outputs) && event.outputs.length > 0
      && Array.isArray(event.sync_output_ids) && event.sync_output_ids.every(positive), `${role} lacks resolved SyncOutput metadata`);
    const named = new Map();
    for (const output of event.outputs) {
      inspectOutput(output, condition, names, callback);
      assert(!named.has(output.name) || equal(named.get(output.name), output), `${role} conflicting duplicate SyncOutput metadata`);
      named.set(output.name, output);
    }
    assert(named.size === 1 && named.has(name), `${role} SyncOutput differs from its requested output`);
    const output = named.get(name);
    const ids = new Set(event.sync_output_ids);
    assert(ids.size > 0 && event.outputs.every(output => ids.has(output.wl_output_id))
      && event.unknown_output_ids.every(id => ids.has(id) && !event.outputs.some(output => output.wl_output_id === id)), `${role} SyncOutput object IDs differ from resolved metadata`);
    const recorded = outputs.filter(item => item.output?.name === name && item.at_ns <= callback).at(-1)?.output;
    assert(recorded && equal(recorded, output), `${role} SyncOutput metadata does not match an observed wl_output`);
    for (const id of ids) {
      const observations = globalOutputs.filter(item => item.output?.wl_output_id === id && item.at_ns <= callback);
      const observed = observations.at(-1);
      let resolved = id === displayId;
      if (observed) {
        assert(integer(observed.at_ns), `${role} SyncOutput alias has an invalid observation timestamp`);
        inspectOutput(observed.output, condition, names, observed.at_ns);
        assert(observed.output.name === name && equal(physicalOutputIdentity(observed.output), physicalOutputIdentity(output)), `${role} SyncOutput alias identifies a different output`);
        assert(!events.some(item => item.event === "output_removed" && item.output?.wl_output_id === id
          && item.at_ns >= observed.at_ns && item.at_ns <= callback), `${role} SyncOutput alias was removed before presentation`);
        resolved = true;
      }
      assert(resolved, `${role} unresolved SyncOutput alias ${id}`);
      resolvedIds.add(id);
    }
    presented++;
    if (event.present_ns >= origin && event.present_ns <= end) presentedInInterval++;
  }
  let discarded = 0;
  if (role === "browser") {
    for (const event of events.filter(event => event.event === "browser_discarded")) {
      assert(integer(event.at_ns) && positive(event.feedback_id), "browser discard identity or timestamp is invalid");
      const paint = paintMap.get(event.feedback_id);
      const selected = (event.at_ns >= origin && event.at_ns <= end) || (paint?.at_ns >= origin && paint.at_ns <= end);
      if (!selected) continue;
      assert(paint && !feedbacks.has(event.feedback_id), "browser discard does not uniquely identify a paint");
      identity(paint);
      identity(event);
      assert(event.at_ns >= paint.at_ns && (completionTime == null || event.at_ns <= completionTime), "browser discard has reversed or late timing");
      feedbacks.add(event.feedback_id);
      discarded++;
    }
  }
  assert(selectedPaints.every(paint => feedbacks.has(paint.feedback_id)), `${role} paint lacks native presentation feedback`);
  assert(presentedInInterval > 0, `${role} has no native presentation during capture`);
  return { output: name, wl_surface_id: surface, display_id: displayId, display_uuid: displayUuid,
    resolved_sync_output_ids: [...resolvedIds].sort((left, right) => left - right), fullscreen: true, painted_frames: selectedPaints.length,
    discarded_frames: discarded, missed_frame_budget: "NOT_EVALUATED", presented_frames: presented, presentations_in_interval: presentedInInterval };
}

export function inspectFullscreenEvidence({ events, condition, origin_ns, duration_ns, applied, completed }) {
  const result = { observation: "native_fullscreen_output_binding", status: "REJECTED", errors: [], roles: {},
    interval: { origin_ns, duration_ns }, occlusion: "NOT_EVALUATED", photons: "NOT_EVALUATED" };
  if (condition?.fullscreen === false) return { ...result, status: "NOT_REQUESTED" };
  try {
    assert(condition?.fullscreen === true && Array.isArray(events) && integer(origin_ns) && positive(duration_ns)
      && integer(origin_ns + duration_ns), "fullscreen condition or capture interval is invalid");
    assert([60, 120].includes(condition.refresh_hz) && Number.isFinite(condition.refresh_actual_hz)
      && condition.refresh_actual_hz > 0 && Math.abs(condition.refresh_actual_hz - condition.refresh_hz) < 1, "fullscreen nominal or actual refresh is invalid");
    assert(typeof condition.terminal_output === "string" && condition.terminal_output.length > 0
      && (condition.browser_output == null || (typeof condition.browser_output === "string" && condition.browser_output.length > 0
        && condition.browser_output !== condition.terminal_output)), "fullscreen roles require distinct named outputs");
    const end = origin_ns + duration_ns;
    const plan = inspectReceipt(applied, condition, "applied");
    assert(applied.monotonic_ns <= origin_ns, "display condition was applied after capture began");
    const names = new Set(plan.outputs.map(output => output.connector));
    const roleNames = { terminal: condition.terminal_output, ...(condition.browser_output ? { browser: condition.browser_output } : {}) };
    const nativeTypes = new Set(["viewport", "browser_viewport", "paint", "browser_paint", "presented", "browser_presented", "output", "output_removed", "presentation_ready"]);
    assert(events.filter(event => nativeTypes.has(event.event)).every(event => event.role === "terminal" || event.role === "browser"), "native fullscreen event lacks a recognized role");
    assert(!events.some(event => ["fatal", "discarded", "paint_failed", "browser_paint_unobserved"].includes(event.event)
      && (!integer(event.at_ns) || (event.at_ns >= origin_ns && event.at_ns <= end))), "native fullscreen evidence contains failed or discarded rendering");
    for (const [role, name] of Object.entries(roleNames)) result.roles[role] = inspectRole(events, role, name, condition, names, origin_ns, end, completed?.monotonic_ns);
    assert(new Set(Object.values(result.roles).map(role => role.wl_surface_id)).size === Object.keys(roleNames).length, "fullscreen roles identify the same wl_surface");
    if (completed == null) return { ...result, status: "PENDING_DISPLAY_COMPLETION" };
    const finalPlan = inspectReceipt(completed, condition, "completed");
    assert(completed.monotonic_ns >= end, "display completion does not cover the end of capture");
    assert(completed.serial_guard === applied.serial_guard && equal(finalPlan, plan), "display plan or serial changed before completion");
    return { ...result, status: "VERIFIED" };
  } catch (error) {
    result.errors.push(error.message);
    return result;
  }
}
