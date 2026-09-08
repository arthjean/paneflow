import { createHash } from "node:crypto";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";
import { inspectCapture } from "./measurements.mjs";
import { inspectBrowserEvents, inspectBrowserLifecycle, inspectVisibility } from "./terminal-capture.mjs";
import { inspectFullscreenEvidence } from "./fullscreen-evidence.mjs";
import { FIXTURE_FILES, bundleSuffix } from "./fixtures.mjs";

const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const digest = /^[a-f0-9]{64}$/;
const commit = /^[a-f0-9]{40}$/;
const requiredFlags = ["Vsync", "HwClock", "HwCompletion"];
const integer = value => Number.isSafeInteger(value) && value >= 0;
const assert = (condition, message) => { if (!condition) throw new Error(message); };
const encode = value => Buffer.from(JSON.stringify(value, null, 2) + "\n");
const lines = values => Buffer.from(values.map(value => JSON.stringify(value)).join("\n") + "\n");

async function bytes(path) {
  const info = await stat(path);
  assert(info.isFile() && info.size <= 128 * 1024 * 1024, `invalid or oversized evidence file: ${path}`);
  return readFile(path);
}

async function json(path) { return JSON.parse((await bytes(path)).toString()); }
async function jsonl(path) { return (await bytes(path)).toString().split("\n").filter(Boolean).map(line => JSON.parse(line)); }

function uniqueMap(records, key, label) {
  const result = new Map();
  for (const record of records) {
    const identity = key(record);
    assert(!result.has(identity), `duplicate ${label}: ${identity}`);
    result.set(identity, record);
  }
  return result;
}

function summarize(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const percentile = value => sorted[Math.ceil(sorted.length * value) - 1];
  return { samples: sorted.length, p50_ns: percentile(0.5), p95_ns: percentile(0.95), p99_ns: percentile(0.99), total_ns: sorted.reduce((total, value) => total + value, 0) };
}

function validatePresented(event, input, paint, echo, environment) {
  assert(typeof event.flags === "string" && requiredFlags.every(flag => new RegExp(`\\b${flag}\\b`).test(event.flags)), "presentation lacks Vsync, HwClock or HwCompletion evidence");
  assert(!event.flags.includes("Unknown"), "unknown compositor presentation flags");
  for (const key of ["surface_id", "terminal", "tick", "feedback_id", "input_ns", "completed_ns", "present_ns", "native_present_ns", "callback_ns", "clock_id", "refresh_ns", "sequence"]) assert(integer(event[key]), `invalid native presentation ${key}`);
  assert(event.refresh_ns > 0, "presentation refresh interval is missing");
  const refresh = 1e9 / event.refresh_ns;
  assert(Math.abs(refresh - (environment.refresh_actual_hz ?? environment.refresh_hz)) <= 0.1, "native presentation refresh differs from the recorded display condition");
  assert(event.input_ns === input.input_ns && event.completed_ns === input.completed_ns, "presentation refers to a different input timestamp");
  assert(event.surface_id === paint.surface_id && event.tick === paint.tick && event.terminal === paint.terminal, "presentation feedback does not identify the painted echo");
  assert(event.input_ns <= event.completed_ns && event.completed_ns <= echo.read_ns && echo.read_ns <= echo.actual_ns && echo.actual_ns <= echo.completed_ns && echo.completed_ns <= paint.at_ns && paint.at_ns <= event.present_ns, "input, PTY echo, paint and presentation are causally reversed");
  const calibration = event.calibration;
  assert(calibration && ["source_ns", "mapped_ns", "max_error_ns"].every(key => integer(calibration[key])), "presentation lacks a measured clock calibration");
  assert(calibration.max_error_ns <= 500_000, "clock calibration exceeds the terminal uncertainty budget");
  assert(event.present_ns === event.native_present_ns + calibration.mapped_ns - calibration.source_ns, "presentation clock mapping differs from its raw calibration");
  assert(event.present_ns <= event.callback_ns + calibration.max_error_ns, "presentation timestamp is later than its callback");
}

async function analyzeRepetition(directory, summary, index, workload, environment, configuration, engine, displayCompletion) {
  const capture = await json(join(directory, "capture.json"));
  assert(summary.status === "CAPTURED_REQUIRES_ANALYSIS" && capture.status === "CAPTURED_REQUIRES_ANALYSIS", `repetition ${index} was rejected or incomplete`);
  assert(Array.isArray(capture.errors) && capture.errors.length === 0 && capture.duration_seconds === 70 && summary.duration_seconds === 70, `repetition ${index} is not an error-free 70 second capture`);
  const start = await json(join(directory, "start.json"));
  assert(start.clock === "CLOCK_MONOTONIC" && integer(start.origin_ns) && start.duration_ns === 70e9, "repetition origin or duration is invalid");
  assert(start.origin_ns === capture.origin_ns && start.origin_ns === summary.origin_ns, "repetition origin differs between raw and summary evidence");
  const plan = await json(join(directory, "replay-plan.json"));
  assert(plan.sha256 === workload && hash(JSON.stringify({ protocol: plan.protocol, events: plan.events })) === workload, "archived replay bytes do not match the workload digest");
  assert(plan.protocol.terminal_count === 4 && plan.protocol.warmup_seconds === 10 && plan.protocol.duration_seconds === 60 && plan.protocol.tick_ns === 50_000_000 && plan.protocol.input_every_ticks === 4 && plan.events.length === 5950, "archived replay differs from the fixed M1 terminal workload");
  const geometry = plan.protocol.terminal_geometry ?? Array.from({ length: 4 }, () => ({ columns: plan.protocol.columns, rows: plan.protocol.rows }));
  assert(geometry.length === 4 && geometry.every(item => integer(item.columns) && item.columns > 0 && integer(item.rows) && item.rows > 0), "replay lacks four explicit terminal geometries");
  const native = await jsonl(join(directory, "native.jsonl"));
  const registrations = await jsonl(join(directory, "inputs.jsonl"));
  const workers = (await Promise.all([0, 1, 2, 3].map(terminal => jsonl(join(directory, `worker-${terminal}.jsonl`))))).flat();
  for (const record of [...native, ...registrations, ...workers]) assert(!["fatal", "discarded", "paint_failed"].includes(record.event), `raw evidence contains ${record.event}: ${JSON.stringify(record)}`);
  const coordinator = await json(join(directory, "coordinator-complete.json"));
  assert(coordinator.failed === null && coordinator.finished_ns >= start.origin_ns + 70e9, "input coordinator failed or stopped early");
  for (const terminal of [0, 1, 2, 3]) {
    const complete = await json(join(directory, `worker-${terminal}-complete.json`));
    assert(complete.failed === null && complete.finished_ns >= start.origin_ns + 70e9, `terminal ${terminal} failed or stopped early`);
  }
  const readiness = native.filter(event => event.event === "presentation_ready");
  assert(readiness.length === (configuration === "C" ? 2 : 1) && readiness.every(event => event.at_ns < start.origin_ns && event.clock_id === readiness[0].clock_id), "compositor observer was not ready before replay");
  const viewports = native.filter(event => event.event === "viewport");
  const initial = viewports.filter(event => event.at_ns <= start.origin_ns).at(-1);
  assert(initial && [initial, ...viewports.filter(event => event.at_ns > start.origin_ns && event.at_ns <= start.origin_ns + 70e9)].every(event => event.width_px === 1920 && event.height_px === 1080 && event.scale === environment.scale), "raw viewport differs from the physical M1 condition");
  const outputs = uniqueMap(workers.filter(event => event.event === "output"), event => event.sequence, "replay output");
  const scheduled = uniqueMap(registrations.filter(event => event.event === "input"), event => event.sequence, "scheduled input");
  const inputs = uniqueMap(native.filter(event => event.event === "input"), event => `${event.surface_id}:${event.tick}`, "native input");
  const echoes = uniqueMap(workers.filter(event => event.event === "echo"), event => `${event.terminal}:${event.tick}`, "PTY echo");
  const paints = uniqueMap(native.filter(event => event.event === "paint"), event => event.feedback_id, "paint feedback id");
  const presented = uniqueMap(native.filter(event => event.event === "presented"), event => `${event.surface_id}:${event.tick}`, "presented input");
  uniqueMap([...presented.values()], event => event.feedback_id, "presentation feedback id");
  assert(outputs.size === 5600 && scheduled.size === 350 && inputs.size === 350 && echoes.size === 350 && paints.size === 350 && presented.size === 350, "repetition has missing or extra replay/input/echo/presentation evidence");
  const samples = [];
  const presentationRecords = [];
  const calibration = [];
  let deliveryError = 0;
  for (const [sequence, event] of plan.events.entries()) {
    assert(event.sequence === sequence && integer(event.at_ns) && event.at_ns < 70e9 && integer(event.terminal) && event.terminal < 4, "replay sequence or scheduling metadata is invalid");
    const planned = start.origin_ns + event.at_ns;
    if (event.output_base64 !== undefined) {
      const output = outputs.get(sequence);
      const payload = Buffer.from(event.output_base64, "base64");
      assert(payload.toString("base64") === event.output_base64 && output && output.terminal === event.terminal && output.sha256 === hash(payload) && output.bytes === payload.length && output.planned_ns === planned, "raw terminal bytes diverge from replay");
      assert(output.columns === geometry[event.terminal].columns && output.rows === geometry[event.terminal].rows, "observed terminal geometry diverges from replay");
      assert(integer(output.actual_ns) && integer(output.completed_ns) && output.completed_ns >= output.actual_ns, "invalid PTY output delivery timestamps");
      deliveryError = Math.max(deliveryError, Math.abs(output.actual_ns - planned), Math.abs(output.completed_ns - planned));
      continue;
    }
    assert(typeof event.input === "string" && /^i[0-9]+\n$/.test(event.input), "invalid replay input");
    const tick = Number(event.input.slice(1).trim());
    const registration = scheduled.get(sequence);
    assert(registration && registration.input === event.input && registration.terminal === event.terminal && registration.planned_ns === planned && registration.registration_response_ns < start.origin_ns, "input calendar was incomplete or not preloaded before replay");
    const key = `${registration.surface_id}:${tick}`;
    const input = inputs.get(key);
    const presentation = presented.get(key);
    const echo = echoes.get(`${event.terminal}:${tick}`);
    const paint = paints.get(presentation?.feedback_id);
    assert(input && presentation && echo && paint && presentation.terminal === event.terminal, "input has no uniquely correlated painted PTY echo");
    assert(echo.marker === `pf-input:${event.terminal}:${tick}` && [Buffer.from(event.input).toString("base64"), Buffer.from(event.input.replace("\n", "\r")).toString("base64")].includes(echo.input_base64), "PTY input echo differs from the injected event");
    assert(echo.columns === geometry[event.terminal].columns && echo.rows === geometry[event.terminal].rows, "PTY echo geometry differs from replay");
    validatePresented(presentation, input, paint, echo, environment);
    assert(presentation.clock_id === readiness[0].clock_id, "presentation clock changed after readiness");
    deliveryError = Math.max(deliveryError, Math.abs(input.input_ns - planned), Math.abs(input.completed_ns - planned));
    calibration.push(presentation.calibration);
    presentationRecords.push({ ...presentation, repetition: index, origin_ns: start.origin_ns, paint, echo });
    if (event.at_ns >= 10e9) {
      assert(input.input_ns >= start.origin_ns + 10e9 && presentation.present_ns <= start.origin_ns + 70e9, "post-warmup input finishes outside the measurement window");
      samples.push({ sequence: samples.length, input_ns: input.input_ns - start.origin_ns, present_ns: presentation.present_ns - start.origin_ns, surface_id: input.surface_id, terminal: event.terminal, tick, feedback_id: presentation.feedback_id, compositor_sequence: presentation.sequence });
    }
  }
  assert(deliveryError <= 2_000_000, `actual terminal replay delivery error ${deliveryError} ns exceeds 2 ms`);
  assert(capture.replay?.sha256 === workload && capture.replay.expected_events === 5950 && capture.replay.observed_events === 5950 && capture.replay.divergent_events === 0 && capture.replay.max_delivery_error_ns <= 2_000_000, "runner replay summary is missing or invalid");
  const cpu = native.filter(event => event.event === "cpu").map(event => {
    assert(["prepaint", "paint"].includes(event.phase), "unexpected terminal CPU phase");
    for (const key of ["start_ns", "end_ns", "thread_cpu_start_ns", "thread_cpu_end_ns"]) assert(integer(event[key]), `CPU event lacks ${key}`);
    assert(event.end_ns >= event.start_ns && event.thread_cpu_end_ns >= event.thread_cpu_start_ns, "reversed terminal CPU clocks");
    return { repetition: index, ...event, duration_thread_cpu_ns: event.thread_cpu_end_ns - event.thread_cpu_start_ns, measurement_window: event.start_ns >= start.origin_ns + 10e9 && event.end_ns <= start.origin_ns + 70e9 };
  });
  const cpuSummary = {};
  for (const phase of ["prepaint", "paint"]) {
    const durations = cpu.filter(event => event.measurement_window && event.phase === phase).map(event => event.duration_thread_cpu_ns);
    assert(durations.length > 0, `missing ${phase} thread CPU samples`);
    cpuSummary[phase] = summarize(durations);
  }
  let browser = null;
  let visibility = null;
  if (configuration === "C") {
    assert(JSON.stringify(capture.engine) === JSON.stringify(engine), "repetition engine differs from the combined capture");
    browser = inspectBrowserEvents(native, start.origin_ns, 70, environment.refresh_actual_hz ?? environment.refresh_hz, environment.scale);
    assert(browser.errors.length === 0, `invalid combined Browser observation: ${browser.errors.join("; ")}`);
    for (const event of native.filter(event => event.event === "browser_presented")) {
      assert(typeof event.flags === "string" && requiredFlags.every(flag => new RegExp(`\\b${flag}\\b`).test(event.flags)) && !event.flags.includes("Unknown"), "Browser presentation lacks native hardware feedback flags");
    }
    const lifecycle = await jsonl(join(directory, "browser.jsonl"));
    browser.lifecycle = inspectBrowserLifecycle(lifecycle, start.origin_ns, 70, environment.scale);
    assert(browser.lifecycle.errors.length === 0, `Browser lifecycle is invalid: ${browser.lifecycle.errors.join("; ")}`);
  }
  if (configuration === "C" || capture.display_condition?.fullscreen) {
    visibility = await json(join(directory, "visibility.json"));
    if (visibility.observation === "native_fullscreen_output_binding") {
      assert(visibility.application_pid === capture.application_pid && JSON.stringify(visibility.condition) === JSON.stringify(capture.display_condition), "fullscreen evidence differs from the captured process or condition");
      assert(visibility.artifacts?.length === 1 && visibility.artifacts[0].path === "display-applied.json", "fullscreen evidence lacks the applied display receipt");
      const appliedBytes = await bytes(join(directory, "visibility-artifacts/display-applied.json"));
      assert(hash(appliedBytes) === visibility.artifacts[0].sha256, "applied display receipt digest differs");
      const proof = inspectFullscreenEvidence({ events: native, condition: visibility.condition, origin_ns: start.origin_ns, duration_ns: 70e9, applied: JSON.parse(appliedBytes), completed: displayCompletion });
      assert(proof.status === "VERIFIED", `fullscreen output binding is incomplete: ${proof.errors.join("; ")}`);
      visibility = { ...visibility, proof };
    } else inspectVisibility(visibility, capture.application_pid, start.origin_ns, 70);
  }
  return { samples, presentationRecords, calibration, cpu, cpuSummary, deliveryError, browser, visibility };
}

export async function analyzeTerminalCapture(captureDir, metadata, outputDir) {
  assert(metadata && typeof metadata === "object" && !Array.isArray(metadata), "explicit measurement metadata is required");
  const source = resolve(captureDir);
  const output = resolve(outputDir);
  await mkdir(output, { mode: 0o700 });
  const artifacts = [];
  const archive = async (path, content, compress = false) => {
    const payload = compress ? gzipSync(content) : content;
    const destination = join(output, path);
    await writeFile(destination, payload, { flag: "wx", mode: 0o600 });
    const sha256 = hash(payload);
    assert(hash(await bytes(destination)) === sha256, `written artifact digest mismatch: ${path}`);
    artifacts.push({ path, sha256, ...(compress ? { encoding: "gzip", uncompressed_sha256: hash(content) } : {}) });
  };
  try {
    const captureBytes = await bytes(join(source, "capture.json"));
    const capture = JSON.parse(captureBytes.toString());
    assert(capture.status === "CAPTURED_REQUIRES_ANALYSIS" && capture.purpose === "qualification_capture", "diagnostic, rejected or incomplete captures cannot be analyzed as qualification evidence");
    assert(["A", "C", "PREFEATURE"].includes(capture.configuration) && capture.clock === "CLOCK_MONOTONIC" && capture.repetitions?.length === 5, "five paired terminal repetitions on CLOCK_MONOTONIC are required");
    assert(commit.test(capture.commit) && digest.test(capture.binary_sha256) && digest.test(capture.workload_sha256), "capture source, binary or workload provenance is incomplete");
    assert(metadata.build_profile === "release" && metadata.load_controlled === true && metadata.fixture_failed === false, "metadata must attest a controlled release measurement with no fixture failure");
    assert(metadata.environment && metadata.environment.display_backend === "wayland", "terminal analysis requires the actual Wayland display identity");
    assert(commit.test(metadata.gpui_commit) && digest.test(metadata.fixture_sha256) && typeof metadata.instrumentation === "string" && metadata.instrumentation.length > 0, "GPUI, fixture and instrumentation provenance are required");
    if (capture.configuration === "C") assert(digest.test(capture.fixture_sha256) && capture.scenario === metadata.scenario, "combined capture requires fixture identity and scenario");
    if (capture.fixture_sha256) {
      assert(capture.fixture_sha256 === metadata.fixture_sha256 && capture.scenario === metadata.scenario, "measurement fixture/scenario differs from capture");
      const manifest = await json(join(source, "fixture-manifest.json"));
      assert(manifest.sha256 === capture.fixture_sha256 && Array.isArray(manifest.scenarios) && manifest.scenarios.includes(capture.scenario), "fixture manifest differs from capture");
      const fixtureHash = createHash("sha256");
      for (const name of FIXTURE_FILES) {
        const content = await bytes(join(source, "fixtures", name));
        fixtureHash.update(name).update("\0").update(content).update("\0");
        await archive(`fixture-${name}.gz`, content, true);
      }
      fixtureHash.update(bundleSuffix(manifest.scenarios));
      assert(fixtureHash.digest("hex") === capture.fixture_sha256, "archived fixture bytes differ from their bundle digest");
      await archive("fixture-manifest.json", encode(manifest));
    }
    const engine = capture.configuration === "C" ? capture.engine : null;
    if (engine) {
      assert(typeof engine.cef === "string" && typeof engine.chromium === "string" && commit.test(engine.cef_rs_commit) && digest.test(engine.manifest_sha256) && digest.test(engine.host_sha256), "combined capture lacks exact engine/host provenance");
      const manifest = await bytes(join(source, "browser-manifest.toml"));
      assert(hash(manifest) === engine.manifest_sha256, "Browser manifest differs from the captured engine");
      const parsed = Bun.TOML.parse(manifest.toString());
      assert(parsed.cef_version === engine.cef && parsed.chromium_version === engine.chromium && parsed.cef_rs_commit === engine.cef_rs_commit && parsed.targets?.[engine.target]?.sha256 === engine.archive_sha256, "Browser manifest fields differ from the captured engine");
      const verification = await json(join(source, "runtime-verification.json"));
      assert(verification.status === "VERIFIED" && verification.manifest_sha256 === engine.manifest_sha256, "combined runtime has no matching verification receipt");
      await archive("browser-manifest.toml", manifest);
      await archive("runtime-verification.json", encode(verification));
    } else assert(capture.configuration !== "C", "combined capture has no engine provenance");
    const sourceFiles = await bytes(join(source, "source-files.json"));
    const sourceInventory = JSON.parse(sourceFiles.toString());
    assert(hash(JSON.stringify(sourceInventory)) === capture.source_files_sha256, "source inventory digest differs from capture provenance");
    let sourcePatch = null;
    let snapshot = null;
    if (capture.snapshot_provenance_sha256) {
      snapshot = await bytes(join(source, "snapshot-provenance.json"));
      assert(hash(snapshot) === capture.snapshot_provenance_sha256 && capture.tracked_source_diff_sha256 === null && capture.commit_provenance === "snapshot_declared_base_plus_verified_inventory", "snapshot source provenance is inconsistent");
      const receipt = JSON.parse(snapshot);
      const inventory = { ...receipt.files_sha256 };
      for (const [path, override] of Object.entries(receipt.snapshot_overrides ?? {})) inventory[path] = override.sha256;
      Object.assign(inventory, receipt.embedded_helper_artifacts ?? {});
      assert(receipt.head === capture.commit && sourceInventory.length > 0 && sourceInventory.every(item => inventory[item.path] === item.sha256), "snapshot inventory does not bind the archived compiled sources and declared base");
    } else {
      sourcePatch = await bytes(join(source, "source.patch"));
      assert(hash(sourcePatch) === capture.tracked_source_diff_sha256, "source patch digest differs from capture provenance");
    }
    assert(metadata.build_evidence?.path && digest.test(metadata.build_evidence.sha256), "a digest-identified build evidence file is required");
    const buildBytes = await bytes(resolve(metadata.build_evidence.path));
    assert(hash(buildBytes) === metadata.build_evidence.sha256, "build evidence digest mismatch");
    const build = JSON.parse(buildBytes.toString());
    assert(build.build_profile === "release" && build.binary_sha256 === capture.binary_sha256 && build.paneflow_commit === capture.commit && build.source_files_sha256 === capture.source_files_sha256, "release build evidence does not bind the measured binary to its archived source");
    assert(snapshot ? build.snapshot_provenance_sha256 === capture.snapshot_provenance_sha256 : build.source_patch_sha256 === capture.tracked_source_diff_sha256, "release build evidence does not bind the source patch or snapshot");
    if (engine) assert(build.browser_host_sha256 === engine.host_sha256 && build.browser_manifest_sha256 === engine.manifest_sha256, "release build evidence does not bind the combined host/runtime");
    await archive("metadata.json", encode(metadata));
    await archive("build-evidence.json", buildBytes);
    if (metadata.build_log) {
      const log = await bytes(resolve(metadata.build_log.path));
      assert(digest.test(metadata.build_log.sha256) && hash(log) === metadata.build_log.sha256 && (!build.build_log_sha256 || build.build_log_sha256 === metadata.build_log.sha256), "build log digest mismatch");
      await archive("build.log.gz", log, true);
    }
    await archive("raw-capture.json.gz", captureBytes, true);
    await archive("source-files.json.gz", sourceFiles, true);
    if (sourcePatch) await archive("source.patch.gz", sourcePatch, true);
    if (snapshot) await archive("snapshot-provenance.json.gz", snapshot, true);
    const runnerManifest = join(source, "runner-sources.json");
    const runnerBytes = await bytes(runnerManifest).catch(error => {
      if (error.code === "ENOENT") return null;
      throw error;
    });
    if (runnerBytes) {
      const runners = JSON.parse(runnerBytes);
      await archive("runner-sources.json", runnerBytes);
      for (const runner of runners) {
        assert(["terminal-capture.mjs", "replay-worker.py", "replay.mjs", "fixtures.mjs", "process-evidence.mjs", "fullscreen-evidence.mjs"].includes(runner.path), "unexpected replay runner source");
        const content = await bytes(join(source, runner.path));
        assert(hash(content) === runner.sha256, "replay runner source digest differs");
        await archive(runner.path, content);
      }
    }
    let displayCompletion;
    if (capture.display_condition?.fullscreen) {
      assert(metadata.display_completion?.path && digest.test(metadata.display_completion.sha256), "fullscreen capture needs digest-identified display completion evidence");
      const completionBytes = await bytes(resolve(metadata.display_completion.path));
      assert(hash(completionBytes) === metadata.display_completion.sha256, "display completion receipt digest differs");
      displayCompletion = JSON.parse(completionBytes);
      await archive("display-completion.json", completionBytes);
    }
    const repetitions = [];
    const calibration = [];
    const cpu = [];
    const cpuSummaries = [];
    const browserObservations = [];
    let deliveryError = 0;
    for (const [offset, summary] of capture.repetitions.entries()) {
      const index = offset + 1;
      const directory = join(source, `r${index}`);
      const result = await analyzeRepetition(directory, summary, index, capture.workload_sha256, metadata.environment, capture.configuration, engine, displayCompletion);
      if (result.browser) {
        browserObservations.push({ index, ...result.browser });
        await archive(`browser-r${index}.jsonl.gz`, await bytes(join(directory, "browser.jsonl")), true);
      }
      if (result.visibility) {
        await archive(`visibility-r${index}.json`, encode(result.visibility));
        for (const [artifactIndex, artifact] of result.visibility.artifacts.entries()) {
          assert(typeof artifact.path === "string" && !artifact.path.startsWith("/") && !artifact.path.includes("\\") && !artifact.path.split("/").includes("..") && digest.test(artifact.sha256), "invalid visibility artifact path");
          const content = await bytes(join(directory, "visibility-artifacts", artifact.path));
          assert(hash(content) === artifact.sha256, "visibility artifact digest differs");
          await archive(`visibility-r${index}-artifact-${artifactIndex}.gz`, content, true);
        }
      }
      repetitions.push({ index, samples: result.samples });
      calibration.push(...result.calibration);
      cpu.push(...result.cpu);
      cpuSummaries.push({ index, ...result.cpuSummary });
      deliveryError = Math.max(deliveryError, result.deliveryError);
      await archive(`presentation-r${index}.jsonl.gz`, lines(result.presentationRecords), true);
      for (const name of ["capture.json", "start.json", "replay-plan.json", "inputs.jsonl", "native.jsonl", "coordinator-complete.json", ...[0, 1, 2, 3].flatMap(terminal => [`worker-${terminal}.jsonl`, `worker-${terminal}-complete.json`])]) await archive(`raw-r${index}-${name}.gz`, await bytes(join(directory, name)), true);
    }
    await archive("cpu.jsonl.gz", lines(cpu), true);
    const cpuSummary = { status: "SUPPLEMENTAL_CPU_MEASUREMENTS", clock: "CLOCK_THREAD_CPUTIME_ID", presentation_observation: null, duration: "thread_cpu_end_ns - thread_cpu_start_ns", window_clock: "CLOCK_MONOTONIC", warmup_seconds: 10, duration_seconds: 60, repetitions: cpuSummaries };
    await archive("cpu-summary.json", encode(cpuSummary));
    calibration.sort((left, right) => left.source_ns - right.source_ns);
    assert(calibration.length >= 2 && calibration.every((point, index) => index === 0 || point.source_ns > calibration[index - 1].source_ns), "clock calibration samples are duplicated or unordered");
    const maxError = Math.max(...calibration.map(point => point.max_error_ns));
    const uncertainty = metadata.uncertainty_ns ?? maxError;
    assert(integer(uncertainty) && uncertainty >= maxError && uncertainty <= 500_000, "reported uncertainty does not cover the measured calibration within 500 us");
    const run = {
      schema_version: 1, configuration: capture.configuration === "PREFEATURE" ? "PRE_FEATURE" : capture.configuration, kind: "terminal_input_to_present",
      scenario: metadata.scenario, fixture_sha256: metadata.fixture_sha256, workload_sha256: capture.workload_sha256,
      paneflow_commit: capture.commit, gpui_commit: metadata.gpui_commit, binary_sha256: capture.binary_sha256, instrumentation: metadata.instrumentation,
      build_profile: "release", terminal_count: 4, engine, environment: metadata.environment,
      display_condition: capture.display_condition,
      warmup_seconds: 10, duration_seconds: 60, load_controlled: true, fixture_failed: false,
      clock: "CLOCK_MONOTONIC", uncertainty_ns: uncertainty,
      calibration: { clock: "CLOCK_MONOTONIC", max_error_ns: maxError, points: calibration.map(({ source_ns, mapped_ns }) => ({ source_ns, mapped_ns })) },
      replay: { sha256: capture.workload_sha256, expected_events: 5950 * 5, observed_events: 5950 * 5, divergent_events: 0, max_delivery_error_ns: deliveryError },
      commit_provenance: capture.commit_provenance ?? "git_head_plus_archived_worktree", source_files_sha256: capture.source_files_sha256,
      snapshot_provenance_sha256: capture.snapshot_provenance_sha256 ?? null,
      browser_observations: browserObservations, browser_presentation_qualification: "NOT_EVALUATED",
      input_source: capture.input_source, presentation_observation: "compositor_feedback", artifacts, repetitions,
    };
    const inspection = await inspectCapture(run, pathToFileURL(join(output, "capture.json")));
    await writeFile(join(output, "capture.json"), encode(run), { flag: "wx", mode: 0o600 });
    await writeFile(join(output, "inspection.json"), encode(inspection), { flag: "wx", mode: 0o600 });
    return { capture: run, inspection, cpu_summary: cpuSummary, capture_path: join(output, "capture.json") };
  } catch (error) {
    await writeFile(join(output, "analysis-rejected.json"), encode({ status: "REJECTED", reason: error.message }), { flag: "wx", mode: 0o600 });
    throw error;
  }
}

if (import.meta.main) {
  try {
    const [directory, metadataPath, output] = process.argv.slice(2);
    if (!directory || !metadataPath || !output || process.argv.length !== 5) throw new Error("requires <raw-capture-directory> <metadata.json> <new-archive-directory>");
    const result = await analyzeTerminalCapture(directory, await json(metadataPath), output);
    process.stdout.write(JSON.stringify({ inspection: result.inspection, capture_path: result.capture_path }, null, 2) + "\n");
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
