import { afterAll, beforeAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { replayBundle } from "./replay.mjs";
import { fixtureBundle } from "./fixtures.mjs";
import { analyzeTerminalCapture } from "./terminal-analysis.mjs";

const hash = value => createHash("sha256").update(value).digest("hex");
const save = (path, value) => writeFile(path, JSON.stringify(value));
const saveLines = (path, values) => writeFile(path, values.map(value => JSON.stringify(value)).join("\n") + "\n");
let directory;
let source;
let metadata;
let outputIndex = 0;
const analyze = () => analyzeTerminalCapture(source, metadata, join(directory, `analysis-${outputIndex++}`));

async function repetition(index, plan) {
  const path = join(source, `r${index}`);
  await mkdir(path);
  const origin = 1_000_000_000_000 + index * 80e9;
  const summary = {
    status: "CAPTURED_REQUIRES_ANALYSIS", errors: [], duration_seconds: 70, origin_ns: origin,
    replay: { sha256: plan.sha256, expected_events: 5950, observed_events: 5950, divergent_events: 0, max_delivery_error_ns: 200 },
  };
  await save(join(path, "capture.json"), summary);
  await save(join(path, "start.json"), { clock: "CLOCK_MONOTONIC", origin_ns: origin, duration_ns: 70e9 });
  await save(join(path, "replay-plan.json"), plan);
  await save(join(path, "coordinator-complete.json"), { failed: null, finished_ns: origin + 70.5e9 });
  const workers = [[], [], [], []];
  const registrations = [];
  const native = [
    { event: "presentation_ready", at_ns: origin - 1e9, clock_id: 1 },
    { event: "viewport", at_ns: origin - 1e9, width_px: 1920, height_px: 1080, scale: 1 },
  ];
  let feedback = 0;
  for (const event of plan.events) {
    const planned = origin + event.at_ns;
    const geometry = plan.protocol.terminal_geometry[event.terminal];
    if (event.output_base64) {
      const payload = Buffer.from(event.output_base64, "base64");
      workers[event.terminal].push({
        event: "output", sequence: event.sequence, terminal: event.terminal, planned_ns: planned,
        actual_ns: planned + 100, completed_ns: planned + 200, bytes: payload.length, sha256: hash(payload), ...geometry,
      });
      continue;
    }
    const tick = Number(event.input.slice(1).trim());
    const surface = event.terminal + 1;
    const id = ++feedback;
    registrations.push({
      event: "input", sequence: event.sequence, terminal: event.terminal, surface_id: surface,
      input: event.input, planned_ns: planned, registration_response_ns: origin - 1e9,
    });
    native.push({ event: "input", surface_id: surface, tick, input_ns: planned + 100, completed_ns: planned + 200 });
    workers[event.terminal].push({
      event: "echo", terminal: event.terminal, tick, read_ns: planned + 300, actual_ns: planned + 400,
      completed_ns: planned + 500, input_base64: Buffer.from(event.input.replace("\n", "\r")).toString("base64"),
      marker: `pf-input:${event.terminal}:${tick}`, ...geometry,
    });
    native.push({ event: "paint", surface_id: surface, terminal: event.terminal, tick, feedback_id: id, at_ns: planned + 600 });
    native.push({
      event: "presented", surface_id: surface, terminal: event.terminal, tick, feedback_id: id,
      input_ns: planned + 100, completed_ns: planned + 200, present_ns: planned + 1e6,
      native_present_ns: planned + 1e6, callback_ns: planned + 1.1e6, clock_id: 1,
      refresh_ns: 16666667, sequence: id, flags: "Value(Kind(Vsync | HwClock | HwCompletion))",
      calibration: { source_ns: planned + 1.1e6 + 100, mapped_ns: planned + 1.1e6 + 100, max_error_ns: 100 },
    });
  }
  for (const phase of ["prepaint", "paint"]) native.push({
    event: "cpu", surface_id: 1, phase, start_ns: origin + 11e9, end_ns: origin + 11e9 + 10000,
    thread_cpu_start_ns: 1e9, thread_cpu_end_ns: 1e9 + 9000,
  });
  for (const terminal of [0, 1, 2, 3]) {
    await saveLines(join(path, `worker-${terminal}.jsonl`), workers[terminal]);
    await save(join(path, `worker-${terminal}-complete.json`), { failed: null, finished_ns: origin + 70.5e9 });
  }
  await saveLines(join(path, "inputs.jsonl"), registrations);
  await saveLines(join(path, "native.jsonl"), native);
  return summary;
}

beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), "pf-terminal-analysis-test-"));
  source = join(directory, "source");
  await mkdir(source);
  const plan = await replayBundle();
  const sourceFiles = [];
  const sourcePatch = "SYNTHETIC TEST ONLY: no hardware qualification.\n";
  await save(join(source, "source-files.json"), sourceFiles);
  await writeFile(join(source, "source.patch"), sourcePatch);
  const capture = {
    status: "CAPTURED_REQUIRES_ANALYSIS", purpose: "qualification_capture", configuration: "A",
    clock: "CLOCK_MONOTONIC", commit: "a".repeat(40), binary_sha256: "b".repeat(64), workload_sha256: plan.sha256,
    source_files_sha256: hash(JSON.stringify(sourceFiles)), tracked_source_diff_sha256: hash(sourcePatch),
    input_source: "SYNTHETIC_TEST_ONLY", repetitions: [],
  };
  for (const index of [1, 2, 3, 4, 5]) capture.repetitions.push(await repetition(index, plan));
  await save(join(source, "capture.json"), capture);
  const buildPath = join(directory, "build.json");
  await save(buildPath, {
    build_profile: "release", paneflow_commit: capture.commit, binary_sha256: capture.binary_sha256,
    source_files_sha256: capture.source_files_sha256, source_patch_sha256: capture.tracked_source_diff_sha256,
  });
  metadata = {
    build_profile: "release", load_controlled: true, fixture_failed: false, scenario: "combined",
    gpui_commit: "c".repeat(40), fixture_sha256: "d".repeat(64), instrumentation: "SYNTHETIC_TEST_ONLY",
    build_evidence: { path: buildPath, sha256: hash(await readFile(buildPath)) },
    environment: {
      machine: "synthetic", os: "synthetic", kernel: "synthetic", compositor: "synthetic", cpu: "synthetic",
      gpu: "synthetic", driver: "synthetic", ram_mib: 32768, power: "AC", scale: 1, refresh_hz: 60,
      physical_width: 1920, physical_height: 1080, display_backend: "wayland",
    },
  };
});

afterAll(async () => { if (directory) await rm(directory, { recursive: true, force: true }); });

async function mutateLines(name, mutation, expected) {
  const path = join(source, "r1", name);
  const original = await readFile(path, "utf8");
  try {
    const records = original.trim().split("\n").map(line => JSON.parse(line));
    mutation(records);
    await saveLines(path, records);
    await expect(analyze()).rejects.toThrow(expected);
  } finally { await writeFile(path, original); }
}

test("analysis preserves synthetic raw evidence and keeps CPU separate from presentation", async () => {
  const result = await analyze();
  expect(result.inspection.samples).toBe(1500);
  expect(result.inspection.qualification).toBe("NOT_EVALUATED");
  expect(result.capture.instrumentation).toBe("SYNTHETIC_TEST_ONLY");
  expect(result.cpu_summary.clock).toBe("CLOCK_THREAD_CPUTIME_ID");
  expect(result.cpu_summary.presentation_observation).toBeNull();
  expect(result.cpu_summary.repetitions[0].paint.p50_ns).toBe(9000);
  expect(result.capture.artifacts.some(artifact => artifact.path === "cpu.jsonl.gz")).toBe(true);
});

const malformed = [
  ["missing hardware presentation clock", rows => { rows.find(row => row.event === "presented").flags = "Value(Kind(Vsync | HwCompletion))"; }, "lacks Vsync, HwClock or HwCompletion"],
  ["excess calibration uncertainty", rows => { rows.find(row => row.event === "presented").calibration.max_error_ns = 500001; }, "uncertainty budget"],
  ["missing presentation", rows => { rows.splice(rows.findIndex(row => row.event === "presented"), 1); }, "missing or extra"],
  ["duplicate feedback", rows => { const events = rows.filter(row => row.event === "presented"); events[1].feedback_id = events[0].feedback_id; }, "duplicate presentation feedback"],
  ["tampered clock mapping", rows => { rows.find(row => row.event === "presented").calibration.mapped_ns++; }, "clock mapping differs"],
  ["paint preceding its echo", rows => { rows.find(row => row.event === "paint").at_ns -= 1000; }, "causally reversed"],
];
for (const [name, mutation, expected] of malformed) test(`analysis rejects ${name}`, () => mutateLines("native.jsonl", mutation, expected));

test("analysis rejects replay delivery beyond 2 ms despite a successful capture summary", () => mutateLines("worker-0.jsonl", rows => {
  const event = rows.find(row => row.event === "output");
  event.actual_ns = event.planned_ns + 2_000_001;
  event.completed_ns = event.actual_ns + 100;
}, "delivery error"));

test("analysis refuses a rejected source capture", async () => {
  const path = join(source, "capture.json");
  const original = await readFile(path, "utf8");
  try {
    await save(path, { ...JSON.parse(original), status: "REJECTED" });
    await expect(analyze()).rejects.toThrow("rejected or incomplete");
  } finally { await writeFile(path, original); }
});

test("C preserves paired terminal measurements and engine provenance without qualifying Browser latency", async () => {
  const restored = [];
  const replace = async (path, content) => {
    const original = await readFile(path).catch(error => { if (error.code === "ENOENT") return null; throw error; });
    restored.push({ path, original });
    await writeFile(path, typeof content === "string" ? content : JSON.stringify(content));
  };
  const capturePath = join(source, "capture.json");
  const capture = JSON.parse(await readFile(capturePath));
  const manifest = `cef_version = "synthetic-cef"\nchromium_version = "synthetic-chromium"\ncef_rs_commit = "${"e".repeat(40)}"\n[targets."x86_64-unknown-linux-gnu"]\nsha256 = "${"f".repeat(64)}"\n`;
  const engine = { cef: "synthetic-cef", chromium: "synthetic-chromium", cef_rs_commit: "e".repeat(40), manifest_sha256: hash(manifest), archive_sha256: "f".repeat(64), host_sha256: "1".repeat(64), target: "x86_64-unknown-linux-gnu" };
  const previousBuildEvidence = metadata.build_evidence;
  const previousFixture = metadata.fixture_sha256;
  try {
    const fixture = await fixtureBundle();
    await mkdir(join(source, "fixtures"));
    for (const [name, bytes] of fixture.assets) await replace(join(source, "fixtures", name), bytes.toString());
    await replace(join(source, "fixture-manifest.json"), fixture.manifest);
    capture.fixture_sha256 = fixture.manifest.sha256;
    capture.scenario = metadata.scenario;
    metadata.fixture_sha256 = fixture.manifest.sha256;
    capture.configuration = "C";
    capture.engine = engine;
    await replace(capturePath, capture);
    await replace(join(source, "browser-manifest.toml"), manifest);
    await replace(join(source, "runtime-verification.json"), { status: "VERIFIED", manifest_sha256: engine.manifest_sha256 });
    const buildPath = join(directory, "combined-build.json");
    const build = JSON.parse(await readFile(metadata.build_evidence.path));
    await replace(buildPath, { ...build, browser_host_sha256: engine.host_sha256, browser_manifest_sha256: engine.manifest_sha256 });
    metadata.build_evidence = { path: buildPath, sha256: hash(await readFile(buildPath)) };
    for (const index of [1, 2, 3, 4, 5]) {
      const path = join(source, `r${index}`);
      const repetitionCapture = JSON.parse(await readFile(join(path, "capture.json")));
      const origin = repetitionCapture.origin_ns;
      await replace(join(path, "capture.json"), { ...repetitionCapture, engine, application_pid: 123 });
      const frame = {
        document: { owner: { workspace: "synthetic", session: "synthetic" }, browser: "synthetic", generation: 1 },
        pool_generation: 1, buffer: 0, frame_sequence: 1, callback_ns: origin - 3e6,
        ready_ns: origin - 2e6, intake_ns: origin - 1e6, capture_timestamp_us: (origin - 4e6) / 1000,
      };
      const nativePath = join(path, "native.jsonl");
      const existing = await readFile(nativePath, "utf8");
      const events = [
        { event: "presentation_ready", at_ns: origin - 1e9, clock_id: 1 },
        { event: "browser_viewport", at_ns: origin - 1e9, width_px: 1920, height_px: 1080, scale: 1 },
        { event: "browser_intake", ...frame, at_ns: frame.intake_ns },
        { event: "browser_paint", ...frame, at_ns: origin - 0.9e6, feedback_id: 1 },
        { event: "browser_presented", ...frame, feedback_id: 1, present_ns: origin - 0.8e6,
          native_present_ns: origin - 0.8e6, presentation_callback_ns: origin - 0.7e6,
          refresh_ns: 16666667, sequence: 900, clock_id: 1, flags: "Vsync | HwClock | HwCompletion",
          calibration: { source_ns: origin - 0.7e6, mapped_ns: origin - 0.7e6, max_error_ns: 100 } },
      ];
      await replace(nativePath, existing + events.map(event => JSON.stringify(event)).join("\n") + "\n");
      const lifecycle = [{ event: "native", native: { native: "loaded" }, at_ns: origin - 2e9 },
        ...Array.from({ length: 72 }, (_, i) => ({ event: "native", at_ns: origin + (i - 1) * 1e9, native: { native: "fixture_state", state: { state: "ready", visibility: "visible", width: 1920, height: 1080, scale: 1 } } }))];
      await replace(join(path, "browser.jsonl"), lifecycle.map(event => JSON.stringify(event)).join("\n") + "\n");
      const evidence = "SYNTHETIC TEST ONLY, not native compositor evidence";
      await mkdir(join(path, "visibility-artifacts"));
      await replace(join(path, "visibility-artifacts/compositor.txt"), evidence);
      await replace(join(path, "visibility.json"), {
        schema_version: 1, observation: "native_compositor_surface_visibility", clock: "CLOCK_MONOTONIC", application_pid: 123,
        artifacts: [{ path: "compositor.txt", sha256: hash(evidence) }],
        intervals: [{ start_ns: origin, end_ns: origin + 70e9,
          terminal: { visible: true, occluded: false, x_px: 0, y_px: 0, width_px: 1920, height_px: 1080 },
          browser: { visible: true, occluded: false, x_px: 1920, y_px: 0, width_px: 1920, height_px: 1080 } }],
      });
    }
    const result = await analyze();
    expect(result.capture.configuration).toBe("C");
    expect(result.capture.engine).toEqual(engine);
    expect(result.capture.kind).toBe("terminal_input_to_present");
    expect(result.inspection.samples).toBe(1500);
    expect(result.capture.browser_presentation_qualification).toBe("NOT_EVALUATED");
    expect(result.inspection.qualification).toBe("NOT_EVALUATED");
    const visibilityPath = join(source, "r1/visibility.json");
    const visibility = JSON.parse(await readFile(visibilityPath));
    visibility.intervals[0].browser.x_px = 0;
    await writeFile(visibilityPath, JSON.stringify(visibility));
    await expect(analyze()).rejects.toThrow("overlap");
  } finally {
    metadata.build_evidence = previousBuildEvidence;
    metadata.fixture_sha256 = previousFixture;
    for (const { path, original } of restored.reverse()) {
      if (original === null) await rm(path, { force: true });
      else await writeFile(path, original);
    }
  }
});
