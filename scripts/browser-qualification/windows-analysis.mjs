import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const percentile = (values, fraction) => {
  if (!values.length) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil(sorted.length * fraction) - 1];
};
const stats = (values) => ({ count: values.length, p50: percentile(values, 0.5), p95: percentile(values, 0.95), p99: percentile(values, 0.99), max: values.length ? Math.max(...values) : null });
const jsonLines = async (path) => (await readFile(path, "utf8")).trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);

export function foregroundOwnership(resources) {
  const known = resources.filter((sample) => typeof sample.foreground_owned === "boolean");
  if (!resources.length || known.length !== resources.length) return { samples: resources.length, owned: null, ratio: null };
  const owned = known.filter((sample) => sample.foreground_owned).length;
  return { samples: known.length, owned, ratio: owned / known.length };
}

export function correlate(events, csv, metadata) {
  const [header, ...lines] = csv.trim().split(/\r?\n/);
  const names = header.split(",");
  for (const required of ["TimeInQPC", "MsUntilDisplayed", "ProcessID"]) {
    if (!names.includes(required)) throw new Error(`PresentMon column missing: ${required}`);
  }
  const presents = lines.map((line) => Object.fromEntries(line.split(",").map((value, index) => [names[index], value])));
  const start = metadata.origin_ns + metadata.warmup_seconds * 1e9;
  const end = start + metadata.duration_seconds * 1e9;
  const stages = [];
  let browser = null;
  let echoes = [];
  let previousEnd = 0;
  for (const event of events) {
    if (event.dropped > 0 || event.event === "fatal") throw new Error("Application evidence reports a failure or dropped events");
    if (event.event === "browser_scene") browser = { ...event.fields, scene_ns: event.at_ns };
    if (event.event === "echo_scene") echoes.push({ ...event.fields, scene_ns: event.at_ns });
    if (event.event !== "renderer_stage" || event.fields.stage !== "dxgi_present_call") continue;
    const stage = event.fields;
    stages.push({ ...stage, browser: browser && browser.scene_ns >= previousEnd ? browser : null, echoes });
    echoes = [];
    previousEnd = stage.end_ns;
  }
  let cursor = 0;
  let unmatched = 0;
  const samples = [];
  const browserSeen = new Set();
  let dropped = 0;
  const calibration = [];
  for (const present of presents) {
    if (Number(present.ProcessID) !== metadata.pid) throw new Error("PresentMon process identity mismatch");
    const at = Number(BigInt(present.TimeInQPC) * 1000000000n / BigInt(metadata.qpc_frequency));
    if (at < start || at >= end) continue;
    while (cursor < stages.length && stages[cursor].end_ns < at) cursor++;
    const stage = stages[cursor];
    if (!stage || stage.start_ns > at || stage.end_ns < at) { unmatched++; continue; }
    if (stage.outcome !== "ok") throw new Error("DXGI Present failed");
    if (stage.width !== 1920 || stage.height !== 1080) throw new Error("M1 requires a 1920x1080 physical presentation viewport");
    calibration.push({ source_ns: at, mapped_ns: at, bracket_start_ns: stage.start_ns, bracket_end_ns: stage.end_ns });
    const delay = Number(present.MsUntilDisplayed);
    if (present.MsUntilDisplayed === "NA" || !Number.isFinite(delay)) { dropped++; continue; }
    const displayed = at + Math.round(delay * 1e6);
    samples.push({ kind: "presentation", present_call_ns: at, displayed_ns: displayed, dxgi_delay_ns: displayed - at, gpu_busy_ms: Number(present.MsGPUBusy), swap_chain: present.SwapChainAddress });
    if (stage.browser) {
      const frame = stage.browser;
      const identity = `${frame.page}:${frame.pool_generation}:${frame.sequence}`;
      if (!browserSeen.has(identity) && frame.callback_ns >= start) {
        browserSeen.add(identity);
        samples.push({ kind: "browser", ...frame, displayed_ns: displayed, latency_ns: displayed - frame.callback_ns, draw_to_present_ns: displayed - frame.scene_ns });
      }
    }
    for (const echo of stage.echoes) {
      if (echo.input_ns >= start) samples.push({ kind: "input", ...echo, displayed_ns: displayed, latency_ns: displayed - echo.input_ns });
    }
  }
  return { samples, unmatched, dropped, calibration };
}

export async function analyze(directory) {
  const metadata = JSON.parse(await readFile(join(directory, "capture.json"), "utf8"));
  const events = await jsonLines(join(directory, "application.jsonl"));
  const result = correlate(events, await readFile(join(directory, "presentmon.csv"), "utf8"), metadata);
  const start = metadata.origin_ns + metadata.warmup_seconds * 1e9;
  const end = start + metadata.duration_seconds * 1e9;
  const resources = (await jsonLines(join(directory, "resources.jsonl"))).filter((sample) => sample.at_ns >= start && sample.at_ns < end);
  const browser = result.samples.filter((sample) => sample.kind === "browser");
  const input = result.samples.filter((sample) => sample.kind === "input");
  const presentation = result.samples.filter((sample) => sample.kind === "presentation");
  const replay = [];
  if (metadata.configuration !== "B") {
    for (let terminal = 0; terminal < 4; terminal++) {
      const records = await jsonLines(join(directory, `terminal-${terminal}.jsonl`));
      const output = records.filter((record) => record.event === "output" && record.planned_ns >= metadata.origin_ns + metadata.warmup_seconds * 1e9);
      replay.push({ terminal, output_events: output.length, max_delivery_error_ns: Math.max(0, ...output.map((event) => event.delivery_error_ns)), echoes: records.filter((record) => record.event === "echo").length });
    }
  }
  const total = (field) => resources.map((sample) => sample.memory.reduce((sum, process) => sum + Number(process[field]), 0));
  const foreground = foregroundOwnership(resources);
  const status = result.unmatched
    ? "INVALID_UNMATCHED_PRESENTS"
    : foreground.ratio !== null && foreground.ratio < 1
      ? "INVALID_WINDOW_NOT_FOREGROUND"
      : "OBSERVED_NOT_BUDGET_CERTIFIED";
  const report = {
    configuration: metadata.configuration, scenario: metadata.scenario,
    status,
    window_foreground: foreground,
    protocol_duration_matches: metadata.warmup_seconds === 10 && metadata.duration_seconds === 60,
    presentation: "PresentMon ETW TimeInQPC plus MsUntilDisplayed, matched inside GPUI DXGI call; not a photonic measurement",
    clock: "Shared Windows QPC", qpc_resolution_ns: 1e9 / metadata.qpc_frequency,
    matched_presents: presentation.length, unmatched_presents: result.unmatched, dropped_presents: result.dropped,
    browser_callback_to_present_ns: stats(browser.map((sample) => sample.latency_ns)),
    browser_draw_to_present_ns: stats(browser.map((sample) => sample.draw_to_present_ns)),
    terminal_input_to_present_ns: stats(input.map((sample) => sample.latency_ns)),
    private_working_set_bytes: stats(total("WorkingSetPrivate")), handles: stats(total("HandleCount")),
    process_cpu_percent_one_core: stats(total("PercentProcessorTime")), replay,
    calibration: result.calibration,
  };
  await writeFile(join(directory, "correlated-samples.jsonl"), result.samples.map((sample) => JSON.stringify(sample)).join("\n") + "\n", { flag: "wx" });
  await writeFile(join(directory, "analysis.json"), JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  const { calibration, ...compact } = report;
  return compact;
}

if (import.meta.main) console.log(JSON.stringify(await analyze(process.argv[2]), null, 2));
