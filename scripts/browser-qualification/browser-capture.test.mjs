import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { finalizeBrowserCapture, inspectBrowserDisplayEvidence, inspectWitnessProcessEvidence } from "./browser-capture.mjs";

function evidence() {
  const origin_ns = 100e9;
  const outputs = ["DP-3", "DP-4"].map((connector, index) => ({ connector, spec: [connector, "vendor", "model", String(index)], mode: "1920x1080@60", refresh_actual_hz: 60, x: index * 1920, y: 0, scale: 1, transform: 0, primary: index === 1 }));
  const state = {
    serial: 2, properties: { "layout-mode": 1 },
    monitors: outputs.map(output => ({ spec: output.spec, modes: [{ id: output.mode, width: 1920, height: 1080, refresh_hz: 60, properties: { "is-current": true } }] })),
    logical_monitors: outputs.map(output => ({ x: output.x, y: 0, scale: 1, transform: 0, primary: output.primary, monitors: [output.spec] })),
  };
  const receipt = monotonic_ns => ({ schema_version: 1, monotonic_ns, state: structuredClone(state), serial_guard: 2, monitor_change_events: [], verified_plan: { layout_mode: 1, outputs: structuredClone(outputs) } });
  const events = [
    { native: "created", trace_us: (origin_ns - 2e9) / 1000 },
    { native: "window_fullscreen", trace_us: (origin_ns - 1e9) / 1000, completed: true, fullscreen: true, width: 1920, height: 1080 },
    ...Array.from({ length: 71 }, (_, index) => ({ native: "fixture_state", trace_us: (origin_ns + index * 1e9) / 1000, state: { width: 1920, height: 1080, scale: 1, visibility: "visible" } })),
  ];
  return { origin_ns, events, environment: { fullscreen: true, expected_output: "DP-4", refresh_actual_hz: 60 },
    proof: { diagnostics: { wl_surface: 46 }, display_evidence: { wl_surface: 46, output: { name: "DP-4" }, observation_start_ns: origin_ns, observation_end_ns: origin_ns + 70e9 } },
    applied: receipt(origin_ns - 3e9), completed: receipt(origin_ns + 71e9) };
}

test("B fullscreen receives verified evidence only after supervisor completion", () => {
  const input = evidence();
  expect(inspectBrowserDisplayEvidence(input)).toMatchObject({ status: "VERIFIED", output: { name: "DP-4" }, occlusion: "NOT_EVALUATED", photons: "NOT_EVALUATED" });
  delete input.completed;
  expect(inspectBrowserDisplayEvidence(input).status).toBe("PENDING_DISPLAY_COMPLETION");
});

test("B process interval binds the same sandboxed GPU and all its threads across measurement", () => {
  const origin = 100e9;
  const processEvidence = {
    schema_version: 1, host_pid: 10, host_start_ticks: "40", clock: "CLOCK_MONOTONIC",
    started_ns: origin - 2e9, ended_ns: origin - 1e9, complete: true, errors: [], omitted_errors: 0,
    processes: [
      { pid: 10, parent: 1, role: "host", start_ticks: "40" },
      { pid: 11, parent: 10, role: "gpu-process", start_ticks: "42", seccomp: 2, no_new_privs: 1,
        seccomp_filters: 1, sandbox_flags: [], threads_complete: true,
        threads: [{ tid: 11, start_ticks: "42", seccomp: 2, no_new_privs: 1, seccomp_filters: 1 }] },
    ],
  };
  const native = {
    host_pid: 10, processes: processEvidence.processes, observation_origin_ns: origin,
    events: [{ native: "trace_stop_requested", trace_us: (origin + 72e9) / 1000 }],
    process_evidence_start: processEvidence,
    process_evidence_end: { ...structuredClone(processEvidence), started_ns: origin + 70e9, ended_ns: origin + 71e9 },
  };
  expect(inspectWitnessProcessEvidence(native)).toMatchObject({ gpu_pid: 11, gpu_start_ticks: "42", resource_observation_complete: true });
  for (const mutate of [
    value => { delete value.process_evidence_start; },
    value => { value.process_evidence_start.ended_ns = origin + 1; },
    value => { value.process_evidence_end.started_ns = origin + 69e9; },
    value => { value.process_evidence_end.processes[1].start_ticks = "43"; },
    value => { value.process_evidence_end.processes[1].threads[0].no_new_privs = 0; },
    value => { value.process_evidence_start.processes[1].threads_complete = false; },
    value => { value.events.push(value.events[0]); },
  ]) {
    const corrupted = structuredClone(native);
    mutate(corrupted);
    expect(() => inspectWitnessProcessEvidence(corrupted)).toThrow();
  }
});

test("B observation starts at the first fixture report after its initial process census", () => {
  const input = evidence();
  input.events.unshift({ native: "fixture_state", trace_us: (input.origin_ns - 0.5e9) / 1000,
    state: { width: 1920, height: 1080, scale: 1, visibility: "visible" } });
  expect(inspectBrowserDisplayEvidence(input).status).toBe("VERIFIED");
});

test.each([
  ["fullscreen merely requested", input => { input.events[1].completed = false; }, "fullscreen transition"],
  ["fullscreen completion missing", input => { input.events.splice(1, 1); }, "completion is absent"],
  ["window resized after warmup", input => { input.events.push({ ...input.events[1], trace_us: (input.origin_ns + 20e9) / 1000, width: 1800 }); }, "viewport differs"],
  ["fixture hidden", input => { input.events[20].state.visibility = "hidden"; }, "visibility differs"],
  ["fixture scale changed", input => { input.events[20].state.scale = 2; }, "scale or visibility"],
  ["second browser", input => { input.events.push({ ...input.events[0] }); }, "one browser lifecycle"],
  ["fixture archive incomplete", input => { input.events.pop(); }, "monitoring does not cover"],
  ["wrong native output", input => { input.proof.display_evidence.output.name = "DP-3"; }, "native output association"],
  ["display applied too late", input => { input.applied.monotonic_ns = input.origin_ns + 1; }, "after capture began"],
  ["completion too early", input => { input.completed.monotonic_ns = input.origin_ns + 69e9; }, "precedes end"],
  ["display changed during child", input => { input.completed.monitor_change_events.push({ signal: "MonitorsChanged" }); }, "display changes"],
  ["display serial changed", input => { input.completed.serial_guard++; input.completed.state.serial++; }, "serial changed"],
])("B fullscreen rejects %s", (_name, mutate, error) => {
  const input = evidence();
  mutate(input);
  expect(() => inspectBrowserDisplayEvidence(input)).toThrow(error);
});


test("finalizer leaves capture pending while supervisor completion is absent", async () => {
  const directory = await mkdtemp(join(tmpdir(), "paneflow-browser-pending-"));
  try {
    const input = evidence();
    const environment = { ...input.environment, display_evidence_directory: directory };
    const artifacts = [];
    for (const [path, value] of [["environment.json", environment], ["display-applied.json", input.applied]]) {
      const bytes = JSON.stringify(value);
      await writeFile(join(directory, path), bytes);
      artifacts.push({ path, sha256: createHash("sha256").update(bytes).digest("hex") });
    }
    await writeFile(join(directory, "capture-pending.json"), JSON.stringify({ environment, repetitions: Array.from({ length: 5 }, (_, index) => ({ index: index + 1 })), artifacts }));
    await expect(finalizeBrowserCapture(directory)).rejects.toThrow("condition-complete.json");
    await expect(readFile(join(directory, "capture.json"))).rejects.toThrow("ENOENT");
    await writeFile(join(directory, "display-applied.json"), "{}");
    await expect(finalizeBrowserCapture(directory)).rejects.toThrow("artifact digest mismatch");
  } finally { await rm(directory, { recursive: true, force: true }); }
});
