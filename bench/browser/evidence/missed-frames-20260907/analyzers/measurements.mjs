import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validateCalibration } from "./replay.mjs";
import { countMissedFrames } from "./missed-frames.mjs";

const commits = /^[a-f0-9]{40}$/;
const digest = /^[a-f0-9]{64}$/;
const identity = ["machine", "os", "kernel", "compositor", "cpu", "gpu", "driver", "ram_mib", "power", "scale", "refresh_hz", "physical_width", "physical_height", "display_backend"];
const kinds = {
  terminal_input_to_present: ["input_ns", "present_ns"],
  browser_callback_to_present: ["callback_ns", "present_ns"],
  browser_draw_to_present: ["draw_ns", "present_ns"],
  browser_idle: [],
  terminal_cpu: ["start_ns", "end_ns"],
  editor_cpu: ["start_ns", "end_ns"],
};

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function text(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function finite(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function percentile(values, fraction) {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil(sorted.length * fraction) - 1];
}

function summary(values) {
  return { p50_ns: percentile(values, 0.5), p95_ns: percentile(values, 0.95), p99_ns: percentile(values, 0.99) };
}

async function verifyArtifacts(run, location) {
  requireValue(Array.isArray(run.artifacts) && run.artifacts.length > 0, "raw artifacts are required");
  for (const artifact of run.artifacts) {
    requireValue(text(artifact.path) && digest.test(artifact.sha256), "invalid artifact reference");
    requireValue(!artifact.path.includes("\\") && !artifact.path.startsWith("/") && !artifact.path.includes(":") && !artifact.path.split("/").includes(".."), "artifacts must be relative to the capture directory");
    const bytes = await readFile(resolve(dirname(fileURLToPath(location)), artifact.path));
    requireValue(createHash("sha256").update(bytes).digest("hex") === artifact.sha256, `artifact digest mismatch: ${artifact.path}`);
  }
}

export async function inspectCapture(run, location) {
  requireValue(run !== null && typeof run === "object", "capture must be an object");
  requireValue(run.schema_version === 1, "unsupported capture schema");
  requireValue(["A", "B", "C", "PRE_FEATURE"].includes(run.configuration), "invalid configuration");
  requireValue(Object.hasOwn(kinds, run.kind), "invalid metric kind");
  requireValue(text(run.scenario) && digest.test(run.fixture_sha256) && digest.test(run.workload_sha256), "scenario and fixture/workload digests are required");
  requireValue(["empty", "scroll", "animation", "webgl", "network", "combined"].includes(run.scenario), "invalid measurement scenario");
  requireValue(commits.test(run.paneflow_commit) && commits.test(run.gpui_commit), "exact source commits are required");
  requireValue(digest.test(run.binary_sha256) && text(run.instrumentation), "binary digest and instrumentation identity are required");
  requireValue(run.build_profile === "release", "M1 requires a release build");
  requireValue(!run.kind.startsWith("browser_") || ["B", "C"].includes(run.configuration), "browser presentation requires B or C");
  requireValue(!["terminal_input_to_present", "terminal_cpu", "editor_cpu"].includes(run.kind) || run.configuration !== "B", "CEF-only B cannot measure the Paneflow terminal or editor");
  requireValue(run.configuration === "B" ? run.terminal_count === 0 : run.terminal_count === 4, "incorrect terminal workload");
  if (["B", "C"].includes(run.configuration)) {
    requireValue(run.engine && text(run.engine.cef) && text(run.engine.chromium) && commits.test(run.engine.cef_rs_commit) && digest.test(run.engine.manifest_sha256), "exact engine provenance is required for B and C");
  } else {
    requireValue(run.engine === null, "inactive browser must have a null engine");
  }
  requireValue(run.environment && identity.every((key) => text(run.environment[key]) || finite(run.environment[key])), "complete machine/display/driver identity is required");
  for (const key of ["ram_mib", "scale", "refresh_hz", "physical_width", "physical_height"]) {
    requireValue(finite(run.environment[key]) && run.environment[key] > 0, `invalid environment ${key}`);
  }
  requireValue(run.environment.physical_width === 1920 && run.environment.physical_height === 1080, "M1 presentation viewport must be 1920x1080 physical pixels");
  requireValue([60, 120].includes(run.environment.refresh_hz), "M1 requires an explicitly qualified 60 or 120 Hz condition");
  requireValue(run.warmup_seconds === 10 && run.duration_seconds === 60, "M1 requires 10 s warm-up and 60 s capture");
  requireValue(run.load_controlled === true && run.fixture_failed === false, "uncontrolled load or failed fixture invalidates capture");
  requireValue(text(run.clock) && finite(run.uncertainty_ns) && run.uncertainty_ns >= 0, "clock and finite uncertainty are required");
  const integratedBrowser = run.configuration === "C" && run.kind.startsWith("browser_");
  if (integratedBrowser) requireValue(digest.test(run.terminal_workload_sha256), "integrated browser requires a separate terminal replay digest");
  validateCalibration(integratedBrowser ? { ...run, workload_sha256: run.terminal_workload_sha256 } : run);
  const idle = run.kind === "browser_idle";
  const screen = run.kind.endsWith("_to_present") || idle;
  if (screen) {
    requireValue(["compositor_feedback", "gpu_present_trace"].includes(run.presentation_observation), "presentation must be observed after submission, not OnPaint, requestAnimationFrame or a CPU callback");
    requireValue(run.uncertainty_ns <= (run.kind === "terminal_input_to_present" ? 500_000 : 1_000_000), "instrument uncertainty exceeds half the delta budget");
  } else {
    requireValue(run.presentation_observation === null, "CPU metrics cannot claim screen latency");
  }
  requireValue(Array.isArray(run.repetitions) && run.repetitions.length === 5, "M1 requires five repetitions");
  if (idle) {
    requireValue(["B", "C"].includes(run.configuration) && run.scenario === "empty", "idle observation requires the empty B or C browser");
    for (const [index, repetition] of run.repetitions.entries()) {
      requireValue(repetition.index === index + 1 && repetition.samples.length === 0, "idle repetitions cannot invent latency samples");
      if (integratedBrowser) requireValue(repetition.idle_evidence?.fresh_captures === 0, "integrated idle capture requires no fresh Chromium captures in the observation window");
      requireValue(repetition.idle_evidence?.measured_frames === 0 && repetition.idle_evidence.monitoring_start_ns <= 10_000_000_000 && repetition.idle_evidence.monitoring_end_ns >= 70_000_000_000 && repetition.idle_evidence.fixture_reports >= 70, "idle capture requires continuous lifecycle and fixture evidence for the full window");
    }
    await verifyArtifacts(run, location);
    return { status: "ACCEPTED_IDLE_OBSERVATION", qualification: "NOT_EVALUATED", kind: run.kind, samples: 0, repetitions: 5, latency: "NOT_APPLICABLE_NO_FRAMES_AFTER_WARMUP" };
  }
  const repetitionSummaries = [];
  const [startKey, endKey] = kinds[run.kind];
  let totalSamples = 0;
  for (const [index, repetition] of run.repetitions.entries()) {
    requireValue(repetition.index === index + 1, "repetitions must be uniquely ordered from 1 to 5");
    requireValue(Array.isArray(repetition.samples) && repetition.samples.length >= 1, "empty repetition");
    let previous = -1;
    const durations = [];
    for (const [sampleIndex, sample] of repetition.samples.entries()) {
      requireValue(sample.sequence === sampleIndex, "sample sequence gap or duplicate");
      const start = sample[startKey];
      const end = sample[endKey];
      requireValue(Number.isSafeInteger(start) && Number.isSafeInteger(end), "timestamps must be safe integer nanoseconds relative to repetition start");
      requireValue(start >= 10_000_000_000 && end <= 70_000_000_000 && end >= start && start > previous, "timestamps are unordered, reversed, or outside the post-warmup measurement window");
      previous = start;
      durations.push(end - start);
    }
    requireValue(repetition.samples[0][startKey] <= 11_000_000_000 && previous >= 69_000_000_000, "samples must cover the entire 60 s observation window");
    totalSamples += durations.length;
    repetitionSummaries.push(summary(durations));
  }
  requireValue(totalSamples >= 1000, "at least 1000 raw samples are required");
  await verifyArtifacts(run, location);
  const aggregate = {};
  for (const key of ["p50_ns", "p95_ns", "p99_ns"]) {
    const values = repetitionSummaries.map((value) => value[key]);
    aggregate[key] = { median: percentile(values, 0.5), worst: Math.max(...values) };
  }
  const missedFrameBudget = run.kind === "browser_draw_to_present" && ["scroll", "animation"].includes(run.scenario)
    ? run.repetitions.map(repetition => {
      const evidence = repetition.missed_frames?.evidence;
      if (!evidence) return { status: "NOT_EVALUATED", errors: ["missing native refresh slot evidence"] };
      requireValue(Number.isSafeInteger(repetition.origin_ns) && evidence.start_ns === repetition.origin_ns + 10e9
        && evidence.end_ns === repetition.origin_ns + 70e9, "missed-frame evidence differs from the exact M1 window");
      requireValue(evidence.refresh_hz === run.environment.refresh_actual_hz, "missed-frame actual refresh differs from condition");
      requireValue(evidence.uncertainty_ns === run.uncertainty_ns || (evidence.uncertainty_ns <= run.uncertainty_ns
        && Number.isSafeInteger(evidence.uncertainty_ns)), "missed-frame uncertainty exceeds capture calibration");
      return Object.fromEntries(Object.entries(countMissedFrames(evidence)).filter(([key]) => key !== "evidence"));
    }) : [];
  return { status: "ACCEPTED_SAMPLES", missed_frames: missedFrameBudget, qualification: "NOT_EVALUATED", kind: run.kind, samples: totalSamples, repetitions: repetitionSummaries, aggregate };
}

function pairedDisplayCondition(run) {
  const environment = run.environment;
  const recorded = run.display_condition ?? {};
  requireValue(recorded && typeof recorded === "object" && !Array.isArray(recorded), "invalid recorded display condition");
  const resolveField = (key, valid) => {
    const values = [environment[key], recorded[key]].filter(value => value !== undefined && value !== null);
    requireValue(values.length > 0 && values.every(valid), `explicit display condition required: ${key}`);
    requireValue(values.every(value => value === values[0]), `conflicting display condition: ${key}`);
    return values[0];
  };
  const fullscreen = resolveField("fullscreen", value => typeof value === "boolean");
  const refresh = resolveField("refresh_actual_hz", value => finite(value) && value > 0);
  if (recorded.refresh_hz !== undefined && recorded.refresh_hz !== null) requireValue(recorded.refresh_hz === environment.refresh_hz, "conflicting display condition: refresh_hz");
  const role = run.kind.startsWith("browser_") ? "browser" : "terminal";
  const outputs = [environment.expected_output, environment[`${role}_output`], recorded[`${role}_output`]].filter(value => value !== undefined && value !== null);
  requireValue(outputs.length > 0 && outputs.every(text), `explicit display condition required: ${role} output`);
  requireValue(outputs.every(value => value === outputs[0]), `conflicting display condition: ${role} output`);
  return { fullscreen, refresh_actual_hz: refresh, role, output: outputs[0] };
}

export async function compareCaptures(left, right, leftLocation, rightLocation) {
  const reference = await inspectCapture(left, leftLocation);
  const candidate = await inspectCapture(right, rightLocation);
  const browser = left.kind.startsWith("browser_");
  requireValue(left.kind !== "browser_idle" && right.kind !== "browser_idle", "idle references have no latency delta");
  const preFeature = left.configuration === "PRE_FEATURE" && right.configuration === "A" && !browser;
  requireValue(preFeature || (left.configuration === (browser ? "B" : "A") && right.configuration === "C"), "comparison requires PRE_FEATURE/A, A/C for terminal or CPU, B/C for browser");
  for (const key of ["kind", "scenario", "fixture_sha256", "workload_sha256", "paneflow_commit", "gpui_commit", "instrumentation", "clock", "build_profile"]) {
    if (preFeature && key === "paneflow_commit") continue;
    requireValue(left[key] === right[key], `unpaired capture: ${key}`);
  }
  for (const key of identity) requireValue(left.environment[key] === right.environment[key], `unpaired environment: ${key}`);
  const displayCondition = pairedDisplayCondition(left);
  const candidateDisplay = pairedDisplayCondition(right);
  for (const key of ["fullscreen", "refresh_actual_hz", "role", "output"]) requireValue(displayCondition[key] === candidateDisplay[key], `unpaired display condition: ${key}`);
  if (browser) {
    for (const key of ["cef", "chromium", "cef_rs_commit", "manifest_sha256"]) {
      requireValue(left.engine[key] === right.engine[key], `unpaired engine: ${key}`);
    }
  }
  if (left.kind.endsWith("_to_present")) {
    const budget = browser ? 1_000_000 : 500_000;
    requireValue(left.uncertainty_ns + right.uncertainty_ns <= budget, "combined instrument uncertainty exceeds half the delta budget");
    requireValue(left.presentation_observation === right.presentation_observation, "unpaired presentation observation");
  }
  const deltas = {};
  for (const key of ["p50_ns", "p95_ns", "p99_ns"]) {
    const values = candidate.repetitions.map((value, index) => value[key] - reference.repetitions[index][key]);
    deltas[key] = { median: percentile(values, 0.5), worst: Math.max(...values) };
  }
  const limits = !preFeature && left.kind === "terminal_input_to_present"
    ? { p95_ns: 1_000_000, p99_ns: 2_000_000 }
    : !preFeature && left.kind === "browser_draw_to_present" ? { p95_ns: 2_000_000 } : null;
  const latency_budget = limits ? {
    status: Object.entries(limits).every(([metric, limit]) => deltas[metric].worst <= limit) ? "SATISFIED" : "EXCEEDED",
    aggregation: "worst_paired_repetition",
    checks: Object.entries(limits).map(([metric, limit_ns]) => ({ metric, limit_ns, measured_delta_ns: deltas[metric].worst, satisfied: deltas[metric].worst <= limit_ns })),
  } : { status: "NOT_APPLICABLE", reason: "this pair does not measure an M1 presentation latency budget" };
  const missedFrames = browser && ["scroll", "animation"].includes(left.scenario)
    ? { status: [reference, candidate].every(result => result.missed_frames.length === 5
        && result.missed_frames.every(repetition => repetition.status === "SATISFIED")) ? "SATISFIED"
        : [reference, candidate].some(result => result.missed_frames.some(repetition => repetition.status === "EXCEEDED")) ? "EXCEEDED" : "NOT_EVALUATED",
      aggregation: "every_repetition_B_and_C", reference: reference.missed_frames, candidate: candidate.missed_frames }
    : { status: "NOT_APPLICABLE" };
  return { status: "PAIRED_SAMPLES", missed_frame_budget: missedFrames, qualification: "NOT_EVALUATED", kind: left.kind, display_condition: displayCondition, deltas, latency_budget };
}
