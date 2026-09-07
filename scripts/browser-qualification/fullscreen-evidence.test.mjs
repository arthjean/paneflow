import { expect, test } from "bun:test";
import { inspectFullscreenEvidence, waylandDisplayUuid } from "./fullscreen-evidence.mjs";

function evidence() {
  const origin = 10_000_000_000;
  const duration = 1_000_000_000;
  const actual = 119.8787841796875;
  const condition = { fullscreen: true, terminal_output: "DP-3", browser_output: "DP-4", refresh_hz: 120, refresh_actual_hz: actual };
  const outputs = ["DP-3", "DP-4"].map((connector, index) => ({
    connector, spec: [connector, "vendor", "model", String(index)], mode: "1920x1080@119.879",
    refresh_actual_hz: actual, x: index * 1920, y: 0, scale: 1, transform: 0,
    primary: index === 1, properties: { "color-mode": 0, "rgb-range": 1 },
  }));
  const state = {
    serial: 2, properties: { "layout-mode": 1 },
    monitors: outputs.map(output => ({ spec: output.spec, properties: output.properties, modes: [{
      id: output.mode, width: 1920, height: 1080, refresh_hz: actual, scales: [1],
      properties: { "is-current": true, "refresh-rate-mode": "fixed" },
    }] })),
    logical_monitors: outputs.map(output => ({
      x: output.x, y: output.y, scale: 1, transform: 0, primary: output.primary, monitors: [output.spec], properties: {},
    })),
  };
  const receipt = monotonic_ns => ({ schema_version: 1, monotonic_ns, state: structuredClone(state),
    verified_plan: { layout_mode: 1, outputs: structuredClone(outputs) }, serial_guard: 2, monitor_change_events: [] });
  const events = [];
  for (const [role, index] of [["terminal", 0], ["browser", 1]]) {
    const surface = 20 + index;
    const prefix = role === "browser" ? "browser_" : "";
    const identity = { role, wl_surface_id: surface, fullscreen: true, display_id: 3 + index, display_uuid: waylandDisplayUuid(outputs[index].connector) };
    const metadata = outputs.map((output, number) => ({
      name: output.connector, width_px: 1920, height_px: 1080, refresh_millihz: Math.round(actual * 1000),
      scale: 1, done_ns: origin - 20_000_000, wl_output_id: 100 + index * 10 + number, registry_global_name: 50 + number,
    }));
    events.push({ event: "presentation_ready", role, wl_surface_id: surface, clock_id: 1, at_ns: origin - 30_000_000 });
    for (const output of metadata) events.push({ event: "output", role, wl_surface_id: surface, output, at_ns: origin - 20_000_000 });
    events.push({ event: `${prefix}viewport`, ...identity, width_px: 1920, height_px: 1080, scale: 1, at_ns: origin - 10_000_000 });
    events.push({ event: `${prefix}paint`, ...identity, feedback_id: 1, at_ns: origin + 1_000_000 });
    events.push({ event: `${prefix}presented`, ...identity, feedback_id: 1, present_ns: origin + 2_000_000,
      native_present_ns: origin + 2_000_000, [role === "browser" ? "presentation_callback_ns" : "callback_ns"]: origin + 3_000_000,
      calibration: { source_ns: origin + 3_000_100, mapped_ns: origin + 3_000_100, max_error_ns: 100 },
      clock_id: 1, sequence: 1, refresh_ns: Math.round(1e9 / actual), flags: "Value(Kind(Vsync | HwClock | HwCompletion))",
      outputs: [metadata[index]], sync_output_ids: [metadata[index].wl_output_id], unknown_output_ids: [],
    });
  }
  return { events, condition, origin_ns: origin, duration_ns: duration,
    applied: receipt(origin - 50_000_000), completed: receipt(origin + duration + 1000) };
}

test("verifies native fullscreen output binding while leaving occlusion and photons unevaluated", () => {
  const result = inspectFullscreenEvidence(evidence());
  expect(result.status).toBe("VERIFIED");
  expect(result.observation).toBe("native_fullscreen_output_binding");
  expect(result.errors).toEqual([]);
  expect(result.roles.terminal.output).toBe("DP-3");
  expect(result.roles.browser.output).toBe("DP-4");
  expect(result.occlusion).toBe("NOT_EVALUATED");
  expect(result.photons).toBe("NOT_EVALUATED");
});

test("allows a terminal-only A condition on the two configured outputs", () => {
  const input = evidence();
  input.condition.browser_output = null;
  input.events = input.events.filter(event => event.role === "terminal");
  expect(inspectFullscreenEvidence(input).status).toBe("VERIFIED");
});

test("requires display completion before a pending capture can become verified", () => {
  const input = evidence();
  delete input.completed;
  expect(inspectFullscreenEvidence(input).status).toBe("PENDING_DISPLAY_COMPLETION");
  delete input.applied;
  expect(inspectFullscreenEvidence(input).status).toBe("REJECTED");
});

test("deduplicates identical SyncOutput names and object IDs", () => {
  const input = evidence();
  const presented = input.events.find(event => event.event === "presented");
  presented.outputs.push(structuredClone(presented.outputs[0]));
  presented.sync_output_ids.push(presented.sync_output_ids[0]);
  expect(inspectFullscreenEvidence(input).status).toBe("VERIFIED");
});

const malformed = [
  ["shared requested output", input => { input.condition.browser_output = "DP-3"; }, "distinct"],
  ["overlapping display layout", input => {
    input.applied.verified_plan.outputs[1].x = 100;
    input.applied.state.logical_monitors[1].x = 100;
  }, "overlap"],
  ["cloned logical monitors", input => { input.applied.state.logical_monitors[0].monitors.push(input.applied.state.monitors[1].spec); }, "mirrored"],
  ["wrong raw physical mode", input => { input.applied.state.monitors[0].modes[0].width = 2560; }, "physical mode"],
  ["late applied receipt", input => { input.applied.monotonic_ns = input.origin_ns + 1; }, "after capture began"],
  ["premature completion", input => { input.completed.monotonic_ns = input.origin_ns + input.duration_ns - 1; }, "end of capture"],
  ["serial changed and returned layout", input => { input.completed.serial_guard = 4; input.completed.state.serial = 4; }, "serial changed"],
  ["MonitorsChanged despite identical serial", input => { input.completed.monitor_change_events.push({ signal: "MonitorsChanged" }); }, "display changes"],
  ["completion plan changed", input => {
    for (const output of input.completed.verified_plan.outputs) output.primary = !output.primary;
    for (const logical of input.completed.state.logical_monitors) logical.primary = !logical.primary;
  }, "plan or serial changed"],
  ["nominal substituted for actual refresh", input => { input.condition.refresh_actual_hz = 120; }, "actual refresh"],
  ["fullscreen only requested", input => { input.events.find(event => event.event === "viewport").fullscreen = false; }, "fullscreen"],
  ["surface changed on paint", input => { input.events.find(event => event.event === "paint").wl_surface_id++; }, "wl_surface"],
  ["shared native surface", input => {
    for (const event of input.events.filter(event => event.role === "browser")) event.wl_surface_id = 20;
  }, "same wl_surface"],
  ["observer clock changed during capture", input => {
    input.events.push({ event: "presentation_ready", role: "terminal", wl_surface_id: 20, clock_id: 0, at_ns: input.origin_ns + 1000 });
  }, "observer clock or surface changed"],
  ["missing role", input => { delete input.events.find(event => event.event === "paint").role; }, "recognized role"],
  ["viewport resize during capture", input => {
    const initial = input.events.find(event => event.event === "viewport");
    input.events.push({ ...initial, at_ns: input.origin_ns + 5000, width_px: 1900 });
  }, "viewport differs"],
  ["output removed then returned", input => {
    const output = input.events.find(event => event.event === "output");
    input.events.push({ ...output, event: "output_removed", at_ns: input.origin_ns + 1000 });
  }, "removed"],
  ["output mode changed during capture", input => {
    const output = structuredClone(input.events.find(event => event.event === "output"));
    output.at_ns = input.origin_ns + 1000;
    output.output.width_px = 2560;
    input.events.push(output);
  }, "native output mode"],
  ["unknown SyncOutput object", input => {
    const event = input.events.find(event => event.event === "presented");
    event.unknown_output_ids = [999];
    event.sync_output_ids.push(999);
  }, "unresolved SyncOutput alias"],
  ["different output in feedback", input => {
    const presented = input.events.find(event => event.event === "presented");
    const other = input.events.find(event => event.event === "output" && event.role === "terminal" && event.output.name === "DP-4").output;
    presented.outputs = [other];
    presented.sync_output_ids = [other.wl_output_id];
  }, "requested output"],
  ["mismatched SyncOutput ID", input => { input.events.find(event => event.event === "presented").sync_output_ids = [999]; }, "object IDs"],
  ["missing hardware flags", input => { input.events.find(event => event.event === "presented").flags = "Vsync"; }, "HwClock"],
  ["bad clock mapping", input => { input.events.find(event => event.event === "presented").calibration.mapped_ns++; }, "clock or causal"],
  ["presentation before paint", input => {
    const presented = input.events.find(event => event.event === "presented");
    presented.present_ns = input.origin_ns;
    presented.native_present_ns = input.origin_ns;
  }, "clock or causal"],
  ["wrong native refresh", input => { input.events.find(event => event.event === "presented").refresh_ns = 16666667; }, "actual refresh"],
  ["callback after display completion", input => { input.events.find(event => event.event === "presented").callback_ns = input.completed.monotonic_ns + 1; }, "exceeds display completion"],
  ["presentations entirely after replay", input => {
    const presented = input.events.find(event => event.event === "presented");
    presented.present_ns = input.origin_ns + input.duration_ns + 10;
    presented.native_present_ns = presented.present_ns;
    presented.callback_ns = presented.present_ns + 100;
  }, "no native presentation during capture"],
  ["missing presentation feedback", input => { input.events = input.events.filter(event => event.event !== "presented"); }, "lacks native presentation"],
  ["unsafe integer timestamp", input => { input.events.find(event => event.event === "paint").at_ns = Number.MAX_SAFE_INTEGER + 1; }, "timestamp"],
];

for (const [name, mutation, message] of malformed) test(`rejects ${name}`, () => {
  const input = evidence();
  mutation(input);
  const result = inspectFullscreenEvidence(input);
  expect(result.status).toBe("REJECTED");
  expect(result.errors.join(" ")).toContain(message);
});

test("does not demand fullscreen evidence for an explicitly ordinary window condition", () => {
  expect(inspectFullscreenEvidence({ condition: { fullscreen: false } }).status).toBe("NOT_REQUESTED");
});


test("GPUI output UUID matches the pinned DNS v5 name derivation", () => {
  expect(waylandDisplayUuid("DP-3")).toBe("d4e537bd-c489-5538-aaea-0d1ebbc92180");
  expect(waylandDisplayUuid("DP-4")).toBe("79c45803-288e-5dd9-8ab9-e14a47fabe6d");
});

function withAliases() {
  const input = evidence();
  const event = input.events.find(event => event.event === "presented");
  event.sync_output_ids = [3, 100, 110];
  event.unknown_output_ids = [3, 110];
  return input;
}

test("resolves GPUI and another observer aliases to the same physical output", () => {
  const result = inspectFullscreenEvidence(withAliases());
  expect(result.status).toBe("VERIFIED");
  expect(result.roles.terminal.resolved_sync_output_ids).toEqual([3, 100, 110]);
  expect(result.roles.terminal.display_uuid).toBe(waylandDisplayUuid("DP-3"));
});

test("does not resolve an alias using metadata observed after its presentation callback", () => {
  const input = withAliases();
  const event = input.events.find(event => event.event === "presented");
  input.events.find(event => event.event === "output" && event.output.wl_output_id === 110).at_ns = event.callback_ns + 1;
  const result = inspectFullscreenEvidence(input);
  expect(result.status).toBe("REJECTED");
  expect(result.errors.join()).toContain("unresolved SyncOutput alias 110");
});

test("rejects an alias to the other physical output", () => {
  const input = withAliases();
  const event = input.events.find(event => event.event === "presented");
  event.sync_output_ids = [3, 100, 111];
  event.unknown_output_ids = [3, 111];
  const result = inspectFullscreenEvidence(input);
  expect(result.status).toBe("REJECTED");
  expect(result.errors.join()).toContain("different output");
});

test("requires native GPUI display UUID and stable display ID on paints", () => {
  const wrongName = withAliases();
  wrongName.events.find(event => event.event === "viewport").display_uuid = waylandDisplayUuid("DP-4");
  expect(inspectFullscreenEvidence(wrongName).status).toBe("REJECTED");
  const moved = withAliases();
  moved.events.find(event => event.event === "paint").display_id = 4;
  expect(inspectFullscreenEvidence(moved).errors.join()).toContain("GPUI display identity changed");
});

test("rejects contradictory metadata even for an ID matching the GPUI display", () => {
  const input = withAliases();
  const conflict = structuredClone(input.events.find(event => event.event === "output" && event.output.wl_output_id === 111));
  conflict.output.wl_output_id = 3;
  input.events.push(conflict);
  expect(inspectFullscreenEvidence(input).errors.join()).toContain("different output");
});

test("rejects a removed cross-observer alias", () => {
  const input = withAliases();
  const alias = input.events.find(event => event.event === "output" && event.output.wl_output_id === 110);
  input.events.push({ ...alias, event: "output_removed", at_ns: input.origin_ns + 100 });
  expect(inspectFullscreenEvidence(input).errors.join()).toContain("removed before presentation");
});


test("output binding preserves a resolved browser discard without claiming its frame budget", () => {
  const input = evidence();
  const original = input.events.find(event => event.event === "browser_presented");
  const paint = input.events.find(event => event.event === "browser_paint");
  input.events.push({ ...paint, feedback_id: 2, at_ns: paint.at_ns + 10_000_000 });
  input.events.push({ ...original, event: "browser_discarded", feedback_id: 2, at_ns: paint.at_ns + 20_000_000 });
  const result = inspectFullscreenEvidence(input);
  expect(result.status).toBe("VERIFIED");
  expect(result.roles.browser.discarded_frames).toBe(1);
  expect(result.roles.browser.missed_frame_budget).toBe("NOT_EVALUATED");
  input.events.push({ ...original, feedback_id: 2 });
  expect(inspectFullscreenEvidence(input).status).toBe("REJECTED");
});
