import { afterAll, beforeAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { assembleIntegratedBrowserMeasurement, bindIntegratedRelease, integratedLifecycle, requireIntegratedCapture } from "./integrated-browser-analysis.mjs";
import { compareCaptures, inspectCapture } from "./measurements.mjs";

const hash = value => createHash("sha256").update(value).digest("hex");
let directory;
const rawBytes = "Synthetic integration test evidence, not a hardware measurement.\n";
beforeAll(async () => { directory = await mkdtemp(join(tmpdir(), "pf-integrated-analysis-")); await writeFile(join(directory, "raw.txt"), rawBytes); });
afterAll(async () => { await rm(directory, { recursive: true, force: true }); });

function fixture() {
  const repetitions = Array.from({ length: 5 }, (_, index) => ({ index: index + 1, origin_ns: 1e12 + index * 100e9, duration_seconds: 70,
    status: "CAPTURED_REQUIRES_ANALYSIS", errors: [], browser: { lifecycle: { fixture_reports: 71 } } }));
  const raw = { configuration: "C", purpose: "qualification_capture", status: "CAPTURED_REQUIRES_ANALYSIS", repetitions,
    binary_sha256: "a".repeat(64), workload_sha256: "b".repeat(64), fixture_sha256: "c".repeat(64), scenario: "combined" };
  const proofs = repetitions.map(repetition => ({ kind: "browser_draw_to_present", clock: "CLOCK_MONOTONIC", measurement_status: "COMPLETE", errors: [], uncertainty_ns: 1050,
    calibration: { points: [{ source_ns: repetition.origin_ns, mapped_ns: repetition.origin_ns }, { source_ns: repetition.origin_ns + 70e9, mapped_ns: repetition.origin_ns + 70e9 }] },
    diagnostics: { observation_start_ns: repetition.origin_ns + 10e9, observation_end_ns: repetition.origin_ns + 70e9 },
    capture_records: [{ kind: "CopyRequested", first_ns: repetition.origin_ns + 11e9 }],
    samples: Array.from({ length: 200 }, (_, sequence) => ({ sequence, draw_ns: 10.1e9 + sequence * 299e6, present_ns: 10.1e9 + sequence * 299e6 + 2e6 })) }));
  const terminal = { schema_version: 1, configuration: "C", kind: "terminal_input_to_present", terminal_count: 4,
    binary_sha256: raw.binary_sha256, workload_sha256: raw.workload_sha256, fixture_sha256: raw.fixture_sha256, scenario: raw.scenario,
    paneflow_commit: "d".repeat(40), gpui_commit: "e".repeat(40), engine: { cef: "synthetic", chromium: "synthetic", cef_rs_commit: "f".repeat(40), manifest_sha256: "0".repeat(64) },
    build_profile: "release", instrumentation: "synthetic-terminal", clock: "CLOCK_MONOTONIC",
    warmup_seconds: 10, duration_seconds: 60, load_controlled: true, fixture_failed: false,
    replay: { sha256: raw.workload_sha256, expected_events: 29750, observed_events: 29750, divergent_events: 0, max_delivery_error_ns: 100 },
    environment: { machine: "synthetic", os: "synthetic", kernel: "synthetic", compositor: "synthetic", cpu: "synthetic", gpu: "synthetic", driver: "synthetic", ram_mib: 32000, power: "AC", scale: 1, refresh_hz: 60, refresh_actual_hz: 60, fullscreen: true, terminal_output: "DP-3", browser_output: "DP-4", physical_width: 1920, physical_height: 1080, display_backend: "wayland" } };
  const artifacts = [{ path: "raw.txt", sha256: hash(rawBytes) }];
  return { raw, proofs, terminal, artifacts };
}
const assemble = data => assembleIntegratedBrowserMeasurement(data.terminal, data.raw, data.proofs, data.artifacts, "synthetic-browser-common-boundary");

test("integrated envelope preserves terminal replay while pairing exact web workload with B", async () => {
  const data = fixture();
  const run = assemble(data);
  expect(run.workload_sha256).toBe(hash(`${data.raw.fixture_sha256}:combined`));
  expect(run.terminal_workload_sha256).toBe(data.raw.workload_sha256);
  expect(run.replay.sha256).toBe(run.terminal_workload_sha256);
  const location = pathToFileURL(join(directory, "capture.json"));
  const inspection = await inspectCapture(run, location);
  expect(inspection.samples).toBe(1000);
  expect(inspection.qualification).toBe("NOT_EVALUATED");
  const reference = { ...run, configuration: "B", terminal_count: 0, replay: null };
  delete reference.terminal_workload_sha256;
  const comparison = await compareCaptures(reference, run, location, location);
  expect(comparison.deltas.p95_ns).toEqual({ median: 0, worst: 0 });
});

test("diagnostic and short inputs cannot enter M1 assembly", () => {
  for (const mutate of [raw => { raw.purpose = "diagnostic_only"; }, raw => { raw.status = "DIAGNOSTIC_ONLY"; }, raw => { raw.repetitions[0].duration_seconds = 3; }, raw => { raw.repetitions.pop(); }]) {
    const { raw } = fixture(); mutate(raw); expect(() => requireIntegratedCapture(raw)).toThrow();
  }
});

test("missing joins, shifted windows and changed fixture identities reject assembly", () => {
  for (const mutate of [data => { data.proofs[0].measurement_status = "INCOMPLETE"; }, data => { data.proofs[0].errors.push({ code: "missing_feedback" }); }, data => { data.proofs[0].diagnostics.observation_end_ns--; }, data => { data.raw.fixture_sha256 = "8".repeat(64); }]) {
    const data = fixture(); mutate(data); expect(() => assemble(data)).toThrow();
  }
});

test("C empty archives idle evidence without synthesizing browser latency", async () => {
  const data = fixture();
  data.raw.scenario = data.terminal.scenario = "empty";
  for (const proof of data.proofs) { proof.samples = []; proof.capture_records = []; }
  const run = assemble(data);
  expect(run.kind).toBe("browser_idle");
  expect((await inspectCapture(run, pathToFileURL(join(directory, "idle.json")))).samples).toBe(0);
  data.proofs[0].capture_records = [{ kind: "CopyRequested", first_ns: data.raw.repetitions[0].origin_ns + 11e9 }];
  expect(() => assemble(data)).toThrow("fresh Chromium captures");
});

function lifecycleFixture() {
  const origin = 1e12;
  const native = (name, at) => ({ native: { native: name, trace_us: at / 1000 } });
  const evidence = { schema_version: 1, host_pid: 10, host_start_ticks: "40", clock: "CLOCK_MONOTONIC", started_ns: origin - 0.4e9, ended_ns: origin - 0.2e9, complete: true, errors: [], omitted_errors: 0,
    processes: [{ pid: 11, parent: 10, start_ticks: "42", role: "gpu-process", seccomp: 2, no_new_privs: 1, seccomp_filters: 1, sandbox_flags: [],
      threads_complete: true, threads: [{ tid: 11, start_ticks: "42", seccomp: 2, no_new_privs: 1, seccomp_filters: 1 }] },
      { pid: 10, parent: 1, start_ticks: "40", role: "host" }] };
  return { browser: [native("trace_started", origin - 2e9), native("browser_create_requested", origin - 1e9), native("created", origin - 0.1e9), native("trace_stop_requested", origin + 70.5e9), native("trace_completed", origin + 72e9), { event: "host_ready", pid: 10 }],
    repetition: { origin_ns: origin }, evidence, endEvidence: { ...structuredClone(evidence), started_ns: origin + 70.1e9, ended_ns: origin + 70.3e9 } };
}

test("creation request bounds capturer start and async trace completion bounds late events", () => {
  const data = lifecycleFixture();
  const result = integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence);
  expect(result.gpu_pid).toBe(11);
  expect(result.capturer_lifecycle.browser_create_ns).toBe(999e9);
  expect(result.capturer_lifecycle.trace_stop_requested_ns).toBe(1070.5e9);
  expect(result.capturer_lifecycle.trace_end_ns).toBe(1072e9);
});

test("created callback cannot replace pre-create marker, and trace startup is never inferred", () => {
  const data = lifecycleFixture();
  data.browser = data.browser.filter(event => event.native?.native !== "browser_create_requested");
  expect(() => integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence)).toThrow("browser_create_requested");
});

test("lifecycle rejects late trace start, early stop and unrelated observed GPU", () => {
  for (const mutate of [data => { data.browser[0].native.trace_us += 2e6; }, data => { data.browser[3].native.trace_us -= 1e6; }, data => { data.evidence.processes[0].parent = 99; }, data => { data.evidence.processes.push({ ...data.evidence.processes[0], pid: 12 }); }, data => { data.evidence.processes[0].sandbox_flags = ["--no-sandbox"]; }]) {
    const data = lifecycleFixture(); mutate(data); expect(() => integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence)).toThrow();
  }
});

function bindingFixture() {
  const gpuiManifest = Buffer.from("synthetic pinned GPUI manifest");
  const gpuiInventory = [{ path: "crates/gpui/src/window.rs", sha256: "a".repeat(64) }];
  const provenance = { commit: "b".repeat(40), sourceHashes: [{ path: "src-app/src/main.rs", sha256: "c".repeat(64) }], snapshot: Buffer.from("synthetic snapshot") };
  const raw = { commit: provenance.commit, source_files_sha256: hash(JSON.stringify(provenance.sourceHashes)), binary_sha256: "d".repeat(64), snapshot_provenance_sha256: hash(provenance.snapshot), engine: { host_sha256: "e".repeat(64), manifest_sha256: "f".repeat(64) } };
  const build = { build_profile: "release", paneflow_commit: raw.commit, source_files_sha256: raw.source_files_sha256, binary_sha256: raw.binary_sha256, snapshot_provenance_sha256: raw.snapshot_provenance_sha256,
    browser_host_sha256: raw.engine.host_sha256, browser_manifest_sha256: raw.engine.manifest_sha256, gpui_manifest_sha256: hash(gpuiManifest), gpui_source_files: structuredClone(gpuiInventory), gpui_source_files_sha256: hash(JSON.stringify(gpuiInventory)) };
  return { raw, build, provenance, gpuiManifest, gpuiInventory };
}
const bind = data => bindIntegratedRelease(data.raw, data.build, data.provenance, data.gpuiManifest, data.gpuiInventory);

test("release binding checks actual Rust and full GPUI inventory against build evidence", () => { expect(() => bind(bindingFixture())).not.toThrow(); });
test("release binding rejects omitted GPUI source, changed Rust, host/runtime or binary", () => {
  for (const mutate of [data => { data.gpuiInventory.push({ path: "crates/gpui/src/new.rs", sha256: "1".repeat(64) }); }, data => { data.provenance.sourceHashes[0].sha256 = "2".repeat(64); }, data => { data.build.browser_host_sha256 = "3".repeat(64); }, data => { data.build.browser_manifest_sha256 = "3".repeat(64); }, data => { data.build.binary_sha256 = "3".repeat(64); }, data => { data.build.gpui_manifest_sha256 = "3".repeat(64); }, data => { data.build.build_profile = "debug"; }, data => { data.build.gpui_source_files[0].path = "../escape"; }]) {
    const data = bindingFixture(); mutate(data); expect(() => bind(data)).toThrow();
  }
});


test("resource read denial stays explicit without erasing proven GPU identity", () => {
  const data = lifecycleFixture();
  data.evidence.complete = false;
  data.evidence.errors.push({ operation: "smaps_rollup", code: "EACCES", pid: 11 });
  const result = integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence);
  expect(result.resource_observation_complete).toBe(false);
  expect(result.gpu_pid).toBe(11);
});

test("GPU replacement, incomplete thread census and hidden errors invalidate process binding", () => {
  for (const mutate of [data => { data.endEvidence.processes[0].start_ticks = "43"; }, data => { data.evidence.processes[0].threads_complete = false; }, data => { data.evidence.processes[0].threads[0].seccomp = 0; }, data => { data.evidence.omitted_errors = 1; }, data => { data.evidence.errors.push({ operation: "thread_inventory", code: "THREAD_SET_CHANGED", pid: 11 }); }, data => { data.endEvidence.started_ns = data.repetition.origin_ns + 69e9; }]) {
    const data = lifecycleFixture(); mutate(data); expect(() => integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence)).toThrow();
  }
});


test("C idle rejects a fresh draw crossing the warmup boundary even if its request began earlier", () => {
  const data = fixture();
  data.raw.scenario = data.terminal.scenario = "empty";
  for (const proof of data.proofs) { proof.samples = []; proof.capture_records = []; }
  data.proofs[0].capture_records = [{ kind: "CopyRequested", first_ns: data.raw.repetitions[0].origin_ns + 9.99e9, draw_ns: data.raw.repetitions[0].origin_ns + 10.01e9 }];
  expect(() => assemble(data)).toThrow("fresh Chromium captures");
});


function zygoteFixture() {
  const data = lifecycleFixture();
  data.browser.find(event => event.event === "host_ready").pid = 785779;
  for (const evidence of [data.evidence, data.endEvidence]) {
    evidence.host_pid = 785779;
    evidence.host_start_ticks = "3736930";
    Object.assign(evidence.processes[0], { pid: 785833, parent: 785789, start_ticks: "3736946" });
    Object.assign(evidence.processes[0].threads[0], { tid: 785833, start_ticks: "3736946" });
    evidence.processes[1] = { pid: 785779, parent: 785688, start_ticks: "3736930", role: "host" };
    evidence.processes.push({ pid: 785789, parent: 785779, start_ticks: "3736933", role: "zygote" });
  }
  return data;
}

test("GPU behind a Chromium zygote joins the verified host ancestry", () => {
  const data = zygoteFixture();
  const result = integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence);
  expect(result.gpu_pid).toBe(785833);
  expect(result.gpu_start_ticks).toBe("3736946");
});

test("ancestry rejects cycles, unknown parents, reused identities and younger ancestors", () => {
  for (const mutate of [
    data => { data.evidence.processes[2].parent = 785833; data.evidence.processes[2].start_ticks = "3736946"; },
    data => { data.evidence.processes = data.evidence.processes.filter(process => process.role !== "zygote"); },
    data => { data.evidence.processes.push({ ...data.evidence.processes[2] }); },
    data => { data.evidence.processes[2].start_ticks = "3736947"; },
    data => { data.evidence.processes[1].start_ticks = "3736931"; },
    data => { data.endEvidence.processes[0].start_ticks = "3736999"; },
  ]) {
    const data = zygoteFixture();
    mutate(data);
    expect(() => integratedLifecycle(data.browser, data.repetition, data.evidence, data.endEvidence)).toThrow();
  }
});
