import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import { gunzipSync, gzipSync } from "node:zlib";
import { extractBrowserPresentation, readBrowserPresentation } from "./browser-trace.mjs";
import { inspectDisplayReceipt } from "./fullscreen-evidence.mjs";
import { inspectCapture } from "./measurements.mjs";
import { runWitness } from "./witness.mjs";
import { sourceProvenance } from "./terminal-capture.mjs";
import { verifyBrowserProcessInterval } from "./process-proof.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const save = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + "\n", { flag: "wx", mode: 0o600 });

const requireValue = (condition, message) => { if (!condition) throw new Error(`browser display proof: ${message}`); };
const canonical = value => JSON.stringify(value, (_key, item) => item && typeof item === "object" && !Array.isArray(item) ? Object.fromEntries(Object.entries(item).sort(([left], [right]) => left.localeCompare(right))) : item);

export function inspectBrowserDisplayEvidence({ events, proof, environment, origin_ns, applied, completed }) {
  requireValue(environment.fullscreen === true && typeof environment.expected_output === "string" && environment.expected_output.length > 0, "explicit fullscreen condition and expected output are required");
  requireValue(Number.isSafeInteger(origin_ns) && Number.isSafeInteger(origin_ns + 70e9), "invalid observation origin");
  const end = origin_ns + 70e9;
  const condition = { refresh_actual_hz: environment.refresh_actual_hz, browser_output: environment.expected_output };
  const plan = inspectDisplayReceipt(applied, condition, "applied");
  requireValue(applied.monotonic_ns <= origin_ns, "display applied after capture began");
  requireValue(Array.isArray(events), "native lifecycle events are absent");
  const relevant = events.filter(event => ["created", "window_fullscreen", "fixture_state"].includes(event.native));
  requireValue(relevant.every(event => Number.isSafeInteger(event.trace_us * 1000)), "native timestamp is invalid");
  const created = relevant.filter(event => event.native === "created");
  requireValue(created.length === 1 && created[0].trace_us * 1000 <= origin_ns, "one browser lifecycle before observation is required");
  const transitions = relevant.filter(event => event.native === "window_fullscreen").sort((left, right) => left.trace_us - right.trace_us);
  const initial = transitions.filter(event => event.trace_us * 1000 <= origin_ns).at(-1);
  requireValue(initial, "fullscreen completion is absent before observation");
  for (const event of [initial, ...transitions.filter(event => event.trace_us * 1000 > origin_ns && event.trace_us * 1000 <= end)]) {
    requireValue(event.completed === true && event.fullscreen === true && event.width === 1920 && event.height === 1080, "fullscreen transition or viewport differs during observation");
  }
  const states = relevant.filter(event => event.native === "fixture_state" && event.trace_us * 1000 >= origin_ns).sort((left, right) => left.trace_us - right.trace_us);
  requireValue(states[0]?.trace_us * 1000 === origin_ns && states.at(-1)?.trace_us * 1000 >= end && states.length >= 70, "fixture monitoring does not cover observation");
  for (const event of states.filter(event => event.trace_us * 1000 <= end)) {
    requireValue(event.state?.width === 1920 && event.state.height === 1080 && event.state.scale === 1 && event.state.visibility === "visible", "fixture viewport, scale or visibility differs during observation");
  }
  const display = proof.display_evidence;
  requireValue(display?.observation_start_ns === origin_ns && display.observation_end_ns === end && display.wl_surface === proof.diagnostics?.wl_surface && display.output?.name === environment.expected_output, "exact native output association is absent");
  const result = { observation: "native_fullscreen_output_binding", wl_surface: display.wl_surface, output: display.output, origin_ns, end_ns: end, occlusion: "NOT_EVALUATED", photons: "NOT_EVALUATED" };
  if (!completed) return { ...result, status: "PENDING_DISPLAY_COMPLETION" };
  const finalPlan = inspectDisplayReceipt(completed, condition, "completed");
  requireValue(completed.monotonic_ns >= end, "display completion precedes end of capture");
  requireValue(completed.serial_guard === applied.serial_guard && canonical(finalPlan) === canonical(plan), "display plan or serial changed during capture");
  return { ...result, status: "VERIFIED" };
}

function observedHostPid(native) {
  const hosts = native.processes?.filter(process => process.role === "host") ?? [];
  requireValue(hosts.length === 1 && hosts[0].pid === native.host_pid && Number.isSafeInteger(native.host_pid) && native.host_pid > 0, "native host PID is not uniquely observed");
  return native.host_pid;
}

export function inspectWitnessProcessEvidence(native) {
  const stops = native.events?.filter(event => event.native === "trace_stop_requested") ?? [];
  requireValue(stops.length === 1, "trace stop is missing or ambiguous");
  return verifyBrowserProcessInterval(observedHostPid(native), native.observation_origin_ns,
    native.observation_origin_ns + 70e9, stops[0].trace_us * 1000,
    native.process_evidence_start, native.process_evidence_end);
}

export async function finalizeBrowserCapture(output, completionPath) {
  const directory = resolve(output);
  const run = JSON.parse(await readFile(join(directory, "capture-pending.json"), "utf8"));
  requireValue(run.environment?.fullscreen === true && run.repetitions?.length === 5, "pending fullscreen capture is incomplete");
  const archived = new Map();
  for (const artifact of run.artifacts) {
    requireValue(typeof artifact.path === "string" && /^[a-zA-Z0-9_.-]+$/.test(artifact.path) && !archived.has(artifact.path), "invalid or duplicate artifact path");
    const bytes = await readFile(join(directory, artifact.path));
    requireValue(hash(bytes) === artifact.sha256, `artifact digest mismatch: ${artifact.path}`);
    archived.set(artifact.path, bytes);
  }
  requireValue(canonical(JSON.parse(archived.get("environment.json"))) === canonical(run.environment), "pending environment differs from its archive");
  const applied = JSON.parse(archived.get("display-applied.json"));
  const completionBytes = await readFile(completionPath ?? join(run.environment.display_evidence_directory, "condition-complete.json"));
  const completed = JSON.parse(completionBytes);
  const calibration = [];
  for (const repetition of run.repetitions) {
    const index = repetition.index;
    const native = JSON.parse(gunzipSync(archived.get(`native-r${index}.json.gz`)));
    const processes = inspectWitnessProcessEvidence(native);
    requireValue(native.observation_origin_ns === repetition.origin_ns && processes.gpu_pid === repetition.diagnostics.gpu_pid, "pending process interval differs from its native archive");
    const proof = extractBrowserPresentation(JSON.parse(gunzipSync(archived.get(`chromium-r${index}.json.gz`))), gunzipSync(archived.get(`wayland-r${index}.txt.gz`)).toString(), { origin_ns: repetition.origin_ns, gpu_pid: repetition.diagnostics.gpu_pid, host_pid: observedHostPid(native), fullscreen: true, expected_output: run.environment.expected_output, refresh_actual_hz: run.environment.refresh_actual_hz });
    calibration.push(...proof.calibration.points);
    requireValue(native.processes?.filter(process => process.role === "gpu-process").length === 1 && native.processes.find(process => process.role === "gpu-process").pid === repetition.diagnostics.gpu_pid, "pending GPU identity differs from its native archive");
    requireValue(canonical(proof.samples) === canonical(repetition.samples), "pending samples differ from archived native traces");
    repetition.missed_frames = proof.missed_frames;
    repetition.diagnostics = proof.diagnostics;
    repetition.display_evidence = inspectBrowserDisplayEvidence({ events: native.events, proof, environment: run.environment, origin_ns: repetition.origin_ns, applied, completed });
  }
  calibration.sort((left, right) => left.source_ns - right.source_ns);
  requireValue(canonical(calibration) === canonical(run.calibration.points), "pending calibration differs from native correspondence");
  await writeFile(join(directory, "display-completed.json"), completionBytes, { flag: "wx", mode: 0o600 });
  run.artifacts.push({ path: "display-completed.json", sha256: hash(completionBytes) });
  const result = await inspectCapture(run, pathToFileURL(join(directory, "capture.json")));
  await save(join(directory, "capture.json"), run);
  await save(join(directory, "inspection.json"), result);
  return result;
}

export async function captureBrowser({ binary, output, rawOutput, environment, scenario, buildEvidence, sourceRoot = root, fullscreen = false }) {
  requireValue(typeof fullscreen === "boolean", "fullscreen option must be boolean");
  environment = { ...environment, fullscreen };
  let appliedBytes;
  let applied;
  if (fullscreen) {
    requireValue(typeof environment.display_evidence_directory === "string" && environment.display_evidence_directory.length > 0 && typeof environment.expected_output === "string" && environment.expected_output.length > 0, "fullscreen requires display_evidence_directory and expected_output");
    environment.display_evidence_directory = resolve(environment.display_evidence_directory);
    appliedBytes = await readFile(join(environment.display_evidence_directory, "applied.json"));
    applied = JSON.parse(appliedBytes);
    inspectDisplayReceipt(applied, { refresh_actual_hz: environment.refresh_actual_hz, browser_output: environment.expected_output }, "applied");
  }
  const source = resolve(sourceRoot);
  if (!["empty", "scroll", "animation", "webgl", "network", "combined"].includes(scenario)) throw new Error("invalid M1 browser scenario");
  const directory = resolve(output);
  const raw = resolve(rawOutput);
  await mkdir(directory, { mode: 0o700 });
  await mkdir(raw, { mode: 0o700 });
  const executable = resolve(binary);
  const buildBytes = await readFile(buildEvidence);
  const build = JSON.parse(buildBytes);
  if (build.build_profile !== "release" || build.binary_sha256 !== hash(await readFile(executable)) || !Array.isArray(build.source_files) || build.source_files.length < 10) throw new Error("release build evidence does not bind the witness binary and sources");
  for (const item of build.source_files) {
    if (item.sha256 !== hash(await readFile(join(source, item.path)))) throw new Error(`source changed since release build: ${item.path}`);
  }
  const buildLog = await readFile(build.build_log);
  if (hash(buildLog) !== build.build_log_sha256) throw new Error("release build log digest differs");
  const manifestBytes = await readFile(join(source, "native/browser/manifest.toml"));
  const manifest = Bun.TOML.parse(manifestBytes.toString());
  const provenance = await sourceProvenance(source);
  const gpuiManifestBytes = await readFile(join(source, "native/gpui/manifest.toml"));
  const gpuiManifest = Bun.TOML.parse(gpuiManifestBytes.toString());
  if (!/^[a-f0-9]{40}$/.test(gpuiManifest.upstream_sha)) throw new Error("GPUI source pin is invalid");
  const artifacts = [];
  const archive = async (name, bytes) => {
    await writeFile(join(directory, name), bytes, { flag: "wx", mode: 0o600 });
    artifacts.push({ path: name, sha256: hash(bytes) });
  };
  if (appliedBytes) await archive("display-applied.json", appliedBytes);
  await archive("environment.json", Buffer.from(JSON.stringify(environment, null, 2) + "\n"));
  await archive("engine-manifest.toml", manifestBytes);
  await archive("gpui-manifest.toml", gpuiManifestBytes);
  if (provenance.snapshot) await archive("snapshot-provenance.json", provenance.snapshot);
  if (provenance.patch) await archive("source.patch", provenance.patch);
  await archive("build-evidence.json", buildBytes);
  await archive("build.log", buildLog);
  for (const name of ["browser-capture.mjs", "browser-trace.mjs", "missed-frames.mjs", "measurements.mjs"]) {
    await archive(`analyzer-${name}`, await readFile(new URL(name, import.meta.url)));
  }
  await archive("source-files.json", Buffer.from(JSON.stringify(build.source_files, null, 2) + "\n"));
  const run = {
    schema_version: 1, configuration: "B", kind: "browser_draw_to_present", scenario,
    paneflow_commit: provenance.commit, commit_provenance: provenance.commit_provenance, gpui_commit: gpuiManifest.upstream_sha, gpui_manifest_sha256: hash(gpuiManifestBytes),
    binary_sha256: hash(await readFile(executable)), build_profile: "release", terminal_count: 0,
    instrumentation: "cef-viz-wp-presentation-v1", clock: "CLOCK_MONOTONIC", uncertainty_ns: 1000,
    engine: { cef: manifest.cef_version, chromium: manifest.chromium_version, cef_rs_commit: manifest.cef_rs_commit, manifest_sha256: hash(manifestBytes) },
    environment, warmup_seconds: 10, duration_seconds: 60, load_controlled: true, fixture_failed: false,
    presentation_observation: "compositor_feedback", replay: null, repetitions: [], artifacts,
    calibration: { clock: "CLOCK_MONOTONIC", max_error_ns: 1000, points: [] },
  };
  try {
    for (let index = 1; index <= 5; index++) {
      process.stderr.write(`M1 B ${scenario}: repetition ${index}/5, 10 s warmup + 60 s observation\n`);
      const repetitionDirectory = join(raw, `r${index}`);
      const native = await runWitness(executable, repetitionDirectory, scenario, 70, { sourceRoot: source, display: "wayland", fullscreen });
      const states = native.events.filter(event => event.native === "fixture_state");
      const origin = native.observation_origin_ns;
      const last = states.at(-1)?.trace_us * 1000;
      if (!Number.isSafeInteger(origin) || last - origin < 70e9 || states.length < 70) throw new Error("incomplete fixture monitoring window");
      if (native.binary_sha256 !== run.binary_sha256) throw new Error("host binary changed between repetitions");
      const processes = inspectWitnessProcessEvidence(native);
      const gpu = native.processes.find(process => process.role === "gpu-process");
      if (gpu?.pid !== processes.gpu_pid) throw new Error("native GPU topology differs from process interval proof");
      const proof = await readBrowserPresentation(join(repetitionDirectory, "profile/chromium-trace.json"), join(repetitionDirectory, "stderr.txt"), { origin_ns: origin, gpu_pid: gpu?.pid, ...(fullscreen ? { host_pid: observedHostPid(native) } : {}), fullscreen, expected_output: environment.expected_output, refresh_actual_hz: environment.refresh_actual_hz });
      const displayEvidence = fullscreen ? inspectBrowserDisplayEvidence({ events: native.events, proof, environment, origin_ns: origin, applied }) : null;
      const samples = proof.samples;
      if (samples.some(sample => Math.abs(1e9 / sample.refresh_ns - environment.refresh_actual_hz) > 0.1)) throw new Error("native display refresh differs from declared condition");
      if (samples.length === 0 && scenario !== "empty") throw new Error("active fixture has no measured presentations");
      if (samples.length > 0 && scenario === "empty") throw new Error("empty fixture was not idle after warmup");
      if (scenario === "empty") run.kind = "browser_idle";
      run.fixture_sha256 ??= native.fixture_sha256;
      run.workload_sha256 ??= hash(Buffer.from(`${native.fixture_sha256}:${scenario}`));
      if (run.fixture_sha256 !== native.fixture_sha256) throw new Error("fixture bytes changed between repetitions");
      run.calibration.points.push(...proof.calibration.points);
      run.repetitions.push({ index, origin_ns: origin, samples, ...(fullscreen ? { origin_ns: origin, display_evidence: displayEvidence } : {}), diagnostics: proof.diagnostics, missed_frames: proof.missed_frames, ...(scenario === "empty" ? { idle_evidence: { measured_frames: 0, monitoring_start_ns: 0, monitoring_end_ns: last - origin, fixture_reports: states.length } } : {}) });
      for (const [name, path] of [[`native-r${index}.json`, "native.json"], [`wayland-r${index}.txt`, "stderr.txt"], [`chromium-r${index}.json`, "profile/chromium-trace.json"]]) {
        await archive(`${name}.gz`, gzipSync(await readFile(join(repetitionDirectory, path)), { level: 6 }));
      }
      await archive(`presentation-r${index}.jsonl.gz`, gzipSync(Buffer.from(samples.map(sample => JSON.stringify(sample)).join("\n") + "\n"), { level: 6 }));
      await save(join(directory, `progress-r${index}.json`), { status: "NATIVE_SAMPLES_CAPTURED", samples: samples.length, diagnostics: proof.diagnostics });
    }
    run.calibration.points.sort((left, right) => left.source_ns - right.source_ns);
    if (fullscreen) {
      await save(join(directory, "capture-pending.json"), run);
      return { status: "PENDING_DISPLAY_COMPLETION", qualification: "NOT_EVALUATED", repetitions: run.repetitions.length, finalize: "browser-capture.mjs --finalize --output <directory>" };
    }
    await save(join(directory, "capture.json"), run);
    const result = await inspectCapture(run, pathToFileURL(join(directory, "capture.json")));
    await save(join(directory, "inspection.json"), result);
    return result;
  } catch (error) {
    await save(join(directory, "rejected.json"), { status: "REJECTED", error: error.message, completed_repetitions: run.repetitions.length });
    throw error;
  }
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ options: { fullscreen: { type: "boolean", default: false }, finalize: { type: "boolean", default: false }, "display-completion": { type: "string" }, binary: { type: "string" }, output: { type: "string" }, raw: { type: "string" }, environment: { type: "string" }, scenario: { type: "string" }, "build-evidence": { type: "string" }, "source-root": { type: "string" } } });
    if (values.finalize) {
      if (!values.output) throw new Error("--finalize requires --output");
      process.stdout.write(JSON.stringify(await finalizeBrowserCapture(values.output, values["display-completion"]), null, 2) + "\n");
    } else {
      if (![values.binary, values.output, values.raw, values.environment, values.scenario, values["build-evidence"]].every(Boolean)) throw new Error("requires --binary --output --raw --environment --scenario --build-evidence");
      const environment = JSON.parse(await readFile(values.environment, "utf8"));
      process.stdout.write(JSON.stringify(await captureBrowser({ binary: values.binary, output: values.output, rawOutput: values.raw, environment, scenario: values.scenario, buildEvidence: values["build-evidence"], sourceRoot: values["source-root"], fullscreen: values.fullscreen }), null, 2) + "\n");
    }
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
