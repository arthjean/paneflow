import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";
import { extractIntegratedBrowserPresentation } from "./integrated-browser-trace.mjs";
import { inspectCapture } from "./measurements.mjs";
import { verifyBrowserProcessInterval } from "./process-proof.mjs";

const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const digest = /^[a-f0-9]{64}$/;
const encode = value => Buffer.from(JSON.stringify(value, null, 2) + "\n");
const assert = (condition, message) => { if (!condition) throw new Error(`integrated browser analysis: ${message}`); };
const safePath = path => typeof path === "string" && path.length > 0 && !path.startsWith("/") && !path.includes("\\") && !path.includes(":") && !path.split("/").some(part => ["..", ".", ""].includes(part));
const integer = value => Number.isSafeInteger(value) && value >= 0;

async function bytes(path, limit = 128 * 1024 * 1024) {
  const info = await stat(path);
  assert(info.isFile() && info.size <= limit, `invalid or oversized artifact ${path}`);
  return readFile(path);
}
const json = async path => JSON.parse((await bytes(path)).toString());
function jsonl(content) {
  const text = content.toString();
  assert(text.endsWith("\n"), "truncated JSONL artifact");
  return text.trim().split("\n").map(line => JSON.parse(line));
}

export function requireIntegratedCapture(raw) {
  assert(raw?.configuration === "C" && raw.purpose === "qualification_capture" && raw.status === "CAPTURED_REQUIRES_ANALYSIS", "requires a completed C measurement capture, never diagnostic input");
  assert(Array.isArray(raw.repetitions) && raw.repetitions.length === 5, "requires five captured repetitions");
  assert(raw.repetitions.every(repetition => repetition.status === "CAPTURED_REQUIRES_ANALYSIS" && repetition.duration_seconds === 70 && Array.isArray(repetition.errors) && repetition.errors.length === 0), "incomplete, failed or short repetition");
}

export function integratedLifecycle(browser, repetition, processEvidence, processEvidenceEnd) {
  const unique = name => {
    const values = browser.filter(event => event.native?.native === name);
    assert(values.length === 1 && integer(values[0].native.trace_us) && integer(values[0].native.trace_us * 1000), `missing or ambiguous native ${name}`);
    return values[0].native.trace_us * 1000;
  };
  const traceStart = unique("trace_started");
  const create = unique("browser_create_requested");
  const traceStop = unique("trace_stop_requested");
  const traceCompleted = unique("trace_completed");
  assert(traceStart <= create && create <= repetition.origin_ns && traceStop >= repetition.origin_ns + 70e9 && traceCompleted >= traceStop, "trace does not cover capturer creation and the full observation");
  const hosts = browser.filter(event => event.event === "host_ready");
  assert(hosts.length === 1 && integer(hosts[0].pid), "missing or ambiguous observed host PID");
  const processes = verifyBrowserProcessInterval(hosts[0].pid, repetition.origin_ns, repetition.origin_ns + 70e9, traceStop, processEvidence, processEvidenceEnd);
  return { ...processes,
    capturer_lifecycle: { trace_start_ns: traceStart, browser_create_ns: create, observation_end_ns: repetition.origin_ns + 70e9, trace_stop_requested_ns: traceStop, trace_end_ns: traceCompleted } };

}

export function bindIntegratedRelease(raw, build, provenance, gpuiManifestBytes, gpuiInventory) {
  assert(build?.build_profile === "release" && build.binary_sha256 === raw.binary_sha256 && build.paneflow_commit === raw.commit && build.source_files_sha256 === raw.source_files_sha256, "release build does not bind application binary and source inventory");
  assert(provenance.commit === raw.commit && hash(JSON.stringify(provenance.sourceHashes)) === raw.source_files_sha256, "current compiled sources differ from captured release inventory");
  assert(build.browser_host_sha256 === raw.engine?.host_sha256 && build.browser_manifest_sha256 === raw.engine?.manifest_sha256, "release build does not bind Browser host/runtime");
  assert(build.gpui_manifest_sha256 === hash(gpuiManifestBytes), "release build does not bind GPUI manifest");
  assert(Array.isArray(build.gpui_source_files) && build.gpui_source_files.length > 0 && build.gpui_source_files.every(entry => safePath(entry.path) && digest.test(entry.sha256)), "missing exact GPUI compiled dependency inventory");
  assert(hash(JSON.stringify(build.gpui_source_files)) === build.gpui_source_files_sha256 && JSON.stringify(gpuiInventory) === JSON.stringify(build.gpui_source_files), "GPUI checkout differs from release source inventory");
  if (raw.snapshot_provenance_sha256) assert(build.snapshot_provenance_sha256 === raw.snapshot_provenance_sha256 && hash(provenance.snapshot) === raw.snapshot_provenance_sha256, "release snapshot binding differs");
  else assert(build.source_patch_sha256 === raw.tracked_source_diff_sha256 && hash(provenance.patch) === raw.tracked_source_diff_sha256, "release source patch binding differs");
}

async function gpuiSources(root, paths) {
  const files = new Set();
  async function visit(path) {
    assert(safePath(path), "invalid GPUI checkout path");
    const info = await stat(join(root, path));
    if (info.isFile()) { files.add(path); return; }
    assert(info.isDirectory(), "unsupported GPUI source entry");
    for (const entry of await readdir(join(root, path), { withFileTypes: true })) {
      assert(!entry.isSymbolicLink(), "GPUI dependency inventory contains an unresolved symlink");
      await visit(`${path}/${entry.name}`);
    }
  }
  for (const path of paths) await visit(path);
  const result = [];
  for (const path of [...files].sort()) result.push({ path, sha256: hash(await bytes(join(root, path))) });
  return result;
}

export function assembleIntegratedBrowserMeasurement(terminal, raw, proofs, artifacts, instrumentation) {
  requireIntegratedCapture(raw);
  assert(terminal.configuration === "C" && terminal.kind === "terminal_input_to_present" && terminal.terminal_count === 4, "requires the validated combined terminal measurement");
  assert(typeof instrumentation === "string" && instrumentation.length > 0, "explicit paired browser instrumentation identity required");
  assert(terminal.binary_sha256 === raw.binary_sha256 && terminal.workload_sha256 === raw.workload_sha256 && terminal.fixture_sha256 === raw.fixture_sha256 && terminal.scenario === raw.scenario, "terminal and browser capture identity differ");
  assert(Array.isArray(proofs) && proofs.length === 5 && proofs.every(proof => proof.kind === "browser_draw_to_present" && proof.clock === "CLOCK_MONOTONIC" && proof.measurement_status === "COMPLETE" && proof.errors.length === 0), "exact browser correlation is incomplete");
  const points = proofs.flatMap(proof => proof.calibration.points).sort((a, b) => a.source_ns - b.source_ns);
  const repetitions = proofs.map((proof, offset) => {
    const origin = raw.repetitions[offset].origin_ns;
    assert(proof.diagnostics.observation_start_ns === origin + 10e9 && proof.diagnostics.observation_end_ns === origin + 70e9, "browser proof does not cover the exact M1 window");
    const base = { index: offset + 1, origin_ns: origin, samples: proof.samples, diagnostics: proof.diagnostics, missed_frames: proof.missed_frames, correlation_errors: proof.errors };
    if (raw.scenario !== "empty") { assert(proof.samples.length > 0, "active fixture has no exact fresh presentations"); return base; }
    const fresh = proof.capture_records.filter(record => record.kind === "CopyRequested" && (record.draw_ns ?? record.first_ns) >= origin + 10e9 && (record.draw_ns ?? record.first_ns) <= origin + 70e9).length;
    assert(proof.samples.length === 0 && fresh === 0, "empty fixture contains fresh Chromium captures after warmup");
    return { ...base, idle_evidence: { measured_frames: 0, fresh_captures: fresh, monitoring_start_ns: 0, monitoring_end_ns: 70e9, fixture_reports: raw.repetitions[offset].browser.lifecycle.fixture_reports } };
  });
  const uncertainty = Math.max(...proofs.map(proof => proof.uncertainty_ns));
  return { ...terminal, kind: raw.scenario === "empty" ? "browser_idle" : "browser_draw_to_present", instrumentation,
    workload_sha256: hash(Buffer.from(`${raw.fixture_sha256}:${raw.scenario}`)), terminal_workload_sha256: terminal.workload_sha256,
    uncertainty_ns: uncertainty, calibration: { clock: "CLOCK_MONOTONIC", max_error_ns: uncertainty, points },
    presentation_observation: "compositor_feedback", browser_collector: "cef-capture-copy-draw-to-gpui-wayland-v1", repetitions, artifacts };
}

export async function analyzeIntegratedBrowserCapture(captureDir, metadata, outputDir) {
  const source = resolve(captureDir);
  const raw = await json(join(source, "capture.json"));
  requireIntegratedCapture(raw);
  assert(metadata?.build_evidence?.path && digest.test(metadata.build_evidence.sha256) && metadata.build_log?.path && digest.test(metadata.build_log.sha256), "digest-bound release build evidence and log required");
  const buildBytes = await bytes(resolve(metadata.build_evidence.path));
  const build = JSON.parse(buildBytes);
  assert(hash(buildBytes) === metadata.build_evidence.sha256, "build evidence digest differs");
  const buildLog = await bytes(resolve(metadata.build_log.path));
  assert(hash(buildLog) === metadata.build_log.sha256 && build.build_log_sha256 === metadata.build_log.sha256, "release build log is unbound or changed");
  const appSource = resolve(raw.source_directory);
  const { sourceProvenance } = await import("./terminal-capture.mjs");
  const provenance = await sourceProvenance(appSource);
  const gpuiManifestBytes = await bytes(join(appSource, "native/gpui/manifest.toml"));
  const gpuiManifest = Bun.TOML.parse(gpuiManifestBytes.toString());
  assert(gpuiManifest.upstream_sha === metadata.gpui_commit && safePath(gpuiManifest.checkout_directory) && Array.isArray(gpuiManifest.checkout_paths) && gpuiManifest.checkout_paths.length > 0 && safePath(gpuiManifest.patch_directory), "invalid GPUI source manifest");
  const gpuiInventory = await gpuiSources(join(appSource, gpuiManifest.checkout_directory), gpuiManifest.checkout_paths);
  bindIntegratedRelease(raw, build, provenance, gpuiManifestBytes, gpuiInventory);
  assert(hash(await bytes(resolve(raw.binary), 1024 * 1024 * 1024)) === raw.binary_sha256, "application binary differs from captured release");
  const output = resolve(outputDir);
  await mkdir(output, { mode: 0o700 });
  const artifacts = [];
  const archive = async (path, content, compress = false) => {
    const payload = compress ? gzipSync(content) : content;
    await writeFile(join(output, path), payload, { flag: "wx", mode: 0o600 });
    artifacts.push({ path, sha256: hash(payload) });
  };
  try {
    for (const name of ["integrated-browser-analysis.mjs", "integrated-browser-trace.mjs", "missed-frames.mjs", "measurements.mjs"]) {
      await archive(`analyzer-${name}`, await readFile(new URL(name, import.meta.url)));
    }
    await archive("gpui-manifest.toml", gpuiManifestBytes);
    await archive("gpui-source-files.json", encode(gpuiInventory));
    for (const patch of gpuiManifest.patches ?? []) {
      assert(safePath(patch.file) && digest.test(patch.sha256), "invalid GPUI patch identity");
      const content = await bytes(join(appSource, gpuiManifest.patch_directory, patch.file));
      assert(hash(content) === patch.sha256, "GPUI patch differs from source manifest");
      await archive(`gpui-${patch.file.replaceAll("/", "_")}.gz`, content, true);
    }
    const { analyzeTerminalCapture } = await import("./terminal-analysis.mjs");
    const terminal = await analyzeTerminalCapture(source, metadata, join(output, "terminal"));
    for (const artifact of terminal.capture.artifacts) artifacts.push({ ...artifact, path: `terminal/${artifact.path}` });
    for (const name of ["capture.json", "inspection.json"]) artifacts.push({ path: `terminal/${name}`, sha256: hash(await bytes(join(output, "terminal", name))) });
    const proofs = [];
    for (let index = 1; index <= 5; index++) {
      const directory = join(source, `r${index}`);
      const repetitionBytes = await bytes(join(directory, "capture.json"));
      const repetition = JSON.parse(repetitionBytes);
      assert(JSON.stringify(repetition) === JSON.stringify(raw.repetitions[index - 1]), "repetition summary differs from root capture");
      const nativeBytes = await bytes(join(directory, "native.jsonl"));
      const browserBytes = await bytes(join(directory, "browser.jsonl"));
      const traceBytes = await bytes(join(directory, "chromium-trace.json"), 512 * 1024 * 1024);
      const archivedTrace = repetition.browser_shutdown?.artifacts?.filter(artifact => artifact.path === "chromium-trace.json");
      assert(archivedTrace?.length === 1 && archivedTrace[0].sha256 === hash(traceBytes) && archivedTrace[0].bytes === traceBytes.length, "trace differs from host shutdown archive");
      const processBytes = [];
      for (const field of ["gpu_process_evidence", "gpu_process_evidence_end"]) {
        const reference = repetition[field];
        assert(reference && safePath(reference.path) && digest.test(reference.sha256), `missing digest-bound ${field}`);
        const content = await bytes(join(directory, reference.path));
        assert(hash(content) === reference.sha256, "GPU process observation digest differs");
        processBytes.push(content);
      }
      const lifecycle = integratedLifecycle(jsonl(browserBytes), repetition, ...processBytes.map(content => JSON.parse(content)));
      const proof = extractIntegratedBrowserPresentation(JSON.parse(traceBytes), jsonl(nativeBytes), { origin_ns: repetition.origin_ns,
        warmup_ns: 10e9, duration_ns: 60e9, refresh_hz: metadata.environment.refresh_actual_hz ?? metadata.environment.refresh_hz, ...lifecycle });
      await archive(`chromium-r${index}.json.gz`, traceBytes, true);
      await archive(`native-r${index}.jsonl.gz`, nativeBytes, true);
      await archive(`browser-r${index}.jsonl.gz`, browserBytes, true);
      await archive(`capture-r${index}.json`, repetitionBytes);
      await archive(`gpu-process-start-r${index}.json`, processBytes[0]);
      await archive(`gpu-process-end-r${index}.json`, processBytes[1]);
      await archive(`lifecycle-r${index}.json`, encode(lifecycle));
      await archive(`correlation-r${index}.json.gz`, encode(proof), true);
      assert(proof.measurement_status === "COMPLETE" && proof.errors.length === 0, `repetition ${index} has incomplete exact Browser correlation`);
      proofs.push(proof);
    }
    const run = assembleIntegratedBrowserMeasurement(terminal.capture, raw, proofs, artifacts, metadata.browser_instrumentation);
    const inspection = await inspectCapture(run, pathToFileURL(join(output, "capture.json")));
    await writeFile(join(output, "capture.json"), encode(run), { flag: "wx", mode: 0o600 });
    await writeFile(join(output, "inspection.json"), encode(inspection), { flag: "wx", mode: 0o600 });
    return { capture: run, inspection, capture_path: join(output, "capture.json"), terminal_capture_path: terminal.capture_path };
  } catch (error) {
    await writeFile(join(output, "rejected.json"), encode({ status: "REJECTED", reason: error.message, artifacts }), { flag: "wx", mode: 0o600 });
    throw error;
  }
}

if (import.meta.main) {
  try {
    const [directory, metadataPath, output] = process.argv.slice(2);
    assert(directory && metadataPath && output && process.argv.length === 5, "requires <raw-capture-directory> <metadata.json> <new-archive-directory>");
    const result = await analyzeIntegratedBrowserCapture(directory, await json(metadataPath), output);
    process.stdout.write(JSON.stringify({ inspection: result.inspection, capture_path: result.capture_path }, null, 2) + "\n");
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
