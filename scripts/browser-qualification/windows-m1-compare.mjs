import { readFile, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";

const METRICS = [
  "browser_callback_to_present_ns",
  "browser_draw_to_present_ns",
  "terminal_input_to_present_ns",
  "private_working_set_bytes",
  "handles",
  "process_cpu_percent_one_core",
];

const median = (values) => {
  const sorted = [...values].sort((left, right) => left - right);
  if (!sorted.length) return null;
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};

const defined = (values) => values.filter((value) => typeof value === "number" && Number.isFinite(value));

export function summarize(runs) {
  const repetitions = runs.length;
  const summary = { repetitions, matched_presents: null, unmatched_presents: null, dropped_presents: null };
  for (const field of ["matched_presents", "unmatched_presents", "dropped_presents"]) {
    const values = defined(runs.map((run) => run.analysis[field]));
    summary[field] = values.length ? { median: median(values), worst: Math.max(...values) } : null;
  }
  for (const metric of METRICS) {
    const entries = runs.map((run) => run.analysis[metric]).filter((entry) => entry && typeof entry === "object");
    const statistics = {};
    for (const field of ["p50", "p95", "p99", "max"]) {
      const values = defined(entries.map((entry) => entry[field]));
      statistics[field] = values.length ? { median: median(values), worst: Math.max(...values) } : null;
    }
    const counts = defined(entries.map((entry) => entry.count));
    statistics.samples = counts.reduce((total, value) => total + value, 0);
    summary[metric] = statistics;
  }
  return summary;
}

const delta = (candidate, reference, metric, field) => {
  const left = candidate?.[metric]?.[field]?.median;
  const right = reference?.[metric]?.[field]?.median;
  if (typeof left !== "number" || typeof right !== "number") return null;
  return left - right;
};

export function compare(runs) {
  if (!runs.length) throw new Error("the M1 comparison requires at least one capture directory");
  const identities = new Set(runs.map((run) => `${run.capture.binary_sha256}:${run.capture.host_sha256}:${run.capture.runtime_sha256}:${run.capture.manifest_sha256}`));
  if (identities.size !== 1) throw new Error("the M1 comparison refuses captures produced by different application or runtime artifacts");
  const invalid = runs.filter((run) => run.analysis.status !== "OBSERVED_NOT_BUDGET_CERTIFIED");
  if (invalid.length) {
    const named = invalid.map((run) => `${basename(run.directory)} ${run.analysis.status}`).join(", ");
    throw new Error(`the M1 comparison refuses captures their own analysis rejected: ${named}`);
  }
  const protocol = runs.every((run) => run.analysis.protocol_duration_matches);
  const configurations = {};
  for (const configuration of ["A", "B", "C"]) {
    const selected = runs.filter((run) => run.capture.configuration === configuration);
    if (selected.length) {
      const ratios = selected.map((run) => run.analysis.window_foreground?.ratio ?? null);
      configurations[configuration] = {
        ...summarize(selected),
        window_foreground_ratio: ratios.every((ratio) => typeof ratio === "number") ? Math.min(...ratios) : null,
        scenarios: [...new Set(selected.map((run) => run.capture.scenario))],
        directories: selected.map((run) => run.directory),
      };
    }
  }
  const identity = runs[0].capture;
  return {
    schema_version: 1,
    status: protocol && Object.keys(configurations).length === 3 ? "OBSERVED_NOT_BUDGET_CERTIFIED" : "INCOMPLETE_M1_CAMPAIGN",
    protocol: {
      warmup_seconds: 10,
      duration_seconds: 60,
      repetitions_per_configuration: 5,
      duration_matches: protocol,
      configurations_present: Object.keys(configurations),
      repetitions: Object.fromEntries(Object.entries(configurations).map(([key, value]) => [key, value.repetitions])),
      complete: protocol && ["A", "B", "C"].every((key) => configurations[key]?.repetitions >= 5),
    },
    artifacts: {
      paneflow_commit: identity.paneflow_commit,
      binary_sha256: identity.binary_sha256,
      host_sha256: identity.host_sha256,
      client_sha256: identity.client_sha256,
      runtime_sha256: identity.runtime_sha256,
      manifest_sha256: identity.manifest_sha256,
      gpui_manifest_sha256: identity.gpui_manifest_sha256,
    },
    machine: { os_identity: identity.os_identity ?? null, gpu: identity.gpu ?? null, displays: identity.displays ?? null, cpu: identity.cpu ?? null, ram_bytes: identity.ram_bytes ?? null, power_source: identity.power_source ?? null },
    configurations,
    comparison: {
      presentation: "PresentMon ETW display time; the deltas are observed medians of the per-repetition percentiles, never a budget verdict",
      integrated_browser_overhead_ns: {
        p95: delta(configurations.C, configurations.B, "browser_callback_to_present_ns", "p95"),
        p99: delta(configurations.C, configurations.B, "browser_callback_to_present_ns", "p99"),
      },
      terminal_input_overhead_ns: {
        p95: delta(configurations.C, configurations.A, "terminal_input_to_present_ns", "p95"),
        p99: delta(configurations.C, configurations.A, "terminal_input_to_present_ns", "p99"),
      },
      private_working_set_delta_bytes: {
        p50: delta(configurations.C, configurations.A, "private_working_set_bytes", "p50"),
        max: delta(configurations.C, configurations.A, "private_working_set_bytes", "max"),
      },
      handle_delta: { max: delta(configurations.C, configurations.A, "handles", "max") },
    },
  };
}

export async function loadRun(directory) {
  const [capture, analysis] = await Promise.all([
    readFile(join(directory, "capture.json"), "utf8").then(JSON.parse),
    readFile(join(directory, "analysis.json"), "utf8").then(JSON.parse),
  ]);
  return { directory: basename(directory), capture, analysis };
}

if (import.meta.main) {
  const [output, ...directories] = process.argv.slice(2);
  if (!output || !directories.length) throw new Error("usage: windows-m1-compare.mjs report.json directory...");
  const report = compare(await Promise.all(directories.map(loadRun)));
  await writeFile(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  console.log(JSON.stringify({ status: report.status, protocol: report.protocol, comparison: report.comparison }, null, 2));
}
