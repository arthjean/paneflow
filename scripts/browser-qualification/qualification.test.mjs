import { afterAll, beforeAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { Script } from "node:vm";

const cli = fileURLToPath(new URL("../browser-qualification.mjs", import.meta.url));
let directory;
let child;
let origin;
let manifest;
const raw = "Synthetic unit-test trace. This is not a hardware measurement.\n";

function capture(configuration = "A", kind = "terminal_input_to_present", latency = 2_000_000) {
  const screen = kind.endsWith("_to_present");
  const start = kind === "terminal_input_to_present" ? "input_ns" : kind === "browser_callback_to_present" ? "callback_ns" : kind === "browser_draw_to_present" ? "draw_ns" : "start_ns";
  const end = screen ? "present_ns" : "end_ns";
  return {
    schema_version: 1, configuration, kind, scenario: "combined",
    fixture_sha256: "a".repeat(64), workload_sha256: "b".repeat(64),
    ...(configuration === "C" && kind.startsWith("browser_") ? { terminal_workload_sha256: "b".repeat(64) } : {}),
    paneflow_commit: "c".repeat(40), gpui_commit: "d".repeat(40),
    binary_sha256: "e".repeat(64), instrumentation: "SYNTHETIC_TEST_ONLY",
    engine: ["B", "C"].includes(configuration) ? { cef: "999.0.0-test", chromium: "999.0.0.0", cef_rs_commit: "f".repeat(40), manifest_sha256: "0".repeat(64) } : null,
    build_profile: "release", terminal_count: configuration === "B" ? 0 : 4,
    environment: { machine: "synthetic", os: "synthetic", kernel: "synthetic", compositor: "synthetic", cpu: "synthetic", gpu: "synthetic", driver: "synthetic", ram_mib: 32768, power: "AC", scale: 1, fullscreen: false, refresh_hz: 60, refresh_actual_hz: 60, expected_output: kind.startsWith("browser_") ? "DP-4" : "DP-3", physical_width: 1920, physical_height: 1080, display_backend: "synthetic" },
    warmup_seconds: 10, duration_seconds: 60, load_controlled: true, fixture_failed: false,
    clock: "synthetic_monotonic_ns", uncertainty_ns: 100_000,
    calibration: { clock: "synthetic_monotonic_ns", max_error_ns: 100_000, points: [{ source_ns: 0, mapped_ns: 0 }, { source_ns: 70_000_000_000, mapped_ns: 70_000_000_000 }] },
    replay: configuration === "B" ? null : { sha256: "b".repeat(64), expected_events: 5950, observed_events: 5950, divergent_events: 0, max_delivery_error_ns: 1000 },
    presentation_observation: screen ? "compositor_feedback" : null,
    artifacts: [{ path: "raw.txt", sha256: createHash("sha256").update(raw).digest("hex") }],
    repetitions: Array.from({ length: 5 }, (_, repetition) => ({
      index: repetition + 1,
      samples: Array.from({ length: 200 }, (_, sequence) => ({ sequence, [start]: 10_100_000_000 + sequence * 299_000_000, [end]: 10_100_000_000 + sequence * 299_000_000 + latency })),
    })),
  };
}

function invoke(args) {
  return spawnSync(process.execPath, [cli, ...args], { encoding: "utf8", timeout: 10_000 });
}

async function store(name, value) {
  const path = join(directory, name);
  await writeFile(path, JSON.stringify(value));
  return path;
}

function http(path, options = {}) {
  return new Promise((resolve, reject) => {
    const call = request(`${origin}${path}`, options, (response) => {
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => resolve({ status: response.statusCode, headers: response.headers, body: Buffer.concat(chunks) }));
    });
    call.on("error", reject);
    call.end();
  });
}

beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), "paneflow-browser-test-"));
  await writeFile(join(directory, "raw.txt"), raw);
  child = spawn(process.execPath, [cli, "serve", "0"], { stdio: ["ignore", "pipe", "pipe"] });
  const startup = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("fixture server did not start")), 5000);
    const lines = createInterface({ input: child.stdout });
    lines.once("line", (line) => { clearTimeout(timer); lines.close(); resolve(JSON.parse(line)); });
    child.once("error", (error) => { clearTimeout(timer); reject(error); });
    child.once("exit", () => { clearTimeout(timer); reject(new Error("fixture server exited before startup")); });
  });
  origin = startup.url;
  manifest = startup;
});

afterAll(async () => {
  if (child && child.exitCode === null) {
    const stopped = new Promise((resolve) => child.once("exit", resolve));
    child.kill("SIGTERM");
    const timeout = setTimeout(() => child.kill("SIGKILL"), 3000);
    await stopped;
    clearTimeout(timeout);
  }
  if (directory) await rm(directory, { recursive: true, force: true });
});

test("CLI serves every deterministic fixture and its assets from loopback", async () => {
  expect(origin).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
  expect(manifest.scenarios).toEqual(["empty", "scroll", "animation", "ime", "popup", "download", "webgl", "network", "combined", "serviceworker", "websocket", "iframe", "auth"]);
  for (const scenario of manifest.scenarios) {
    const first = await http(`/${scenario}`);
    const second = await http(`/${scenario}`);
    expect(first.status).toBe(200);
    expect(first.body.equals(second.body)).toBe(true);
    expect(first.headers["cache-control"]).toBe("no-store");
    expect(first.body.toString()).not.toMatch(/https?:\/\//);
  }
  for (const path of ["/fixture.js", "/fixture.css", "/tile.svg"]) expect((await http(path)).status).toBe(200);
  const download = await http("/download.bin");
  expect(download.body.length).toBe(1024);
  expect(download.headers["content-disposition"]).toContain("attachment");
  expect(download.body.equals((await http("/payload?sequence=0")).body)).toBe(true);
  expect((await http("/empty")).body.toString()).not.toContain("script");
  const script = await http("/fixture.js");
  expect(() => new Script(script.body.toString())).not.toThrow();
});

test("web-platform fixtures serve their own entry points from loopback only", async () => {
  const worker = await http("/sw.js");
  expect(worker.status).toBe(200);
  expect(worker.headers["content-type"]).toBe("text/javascript");
  expect(worker.headers["service-worker-allowed"]).toBe("/");
  expect(worker.body.toString()).toContain("served-by-service-worker");

  const frame = await http("/frame");
  expect(frame.status).toBe(200);
  expect(frame.headers["content-security-policy"]).toContain("frame-ancestors 'self'");
  const refused = await http("/embedded");
  expect(refused.status).toBe(200);
  expect(refused.headers["x-frame-options"]).toBe("DENY");
  expect(refused.headers["content-security-policy"]).toContain("frame-ancestors 'none'");
  expect((await http("/iframe")).headers["content-security-policy"]).toContain("frame-ancestors 'self'");

  const anonymous = await http("/private");
  expect(anonymous.status).toBe(401);
  expect(anonymous.headers["www-authenticate"]).toContain("Basic");
  const wrong = await http("/private", { headers: { Authorization: `Basic ${Buffer.from("paneflow:wrong").toString("base64")}` } });
  expect(wrong.status).toBe(401);
  const authenticated = await http("/private", { headers: { Authorization: `Basic ${Buffer.from("paneflow:fixture-only-secret").toString("base64")}` } });
  expect(authenticated.status).toBe(200);
  expect(JSON.parse(authenticated.body.toString())).toEqual({ authenticated: true, user: "paneflow" });

  const endpoint = JSON.parse((await http("/websocket.json")).body.toString()).websocket;
  expect(endpoint).toMatch(/^ws:\/\/127\.0\.0\.1:\d+\/ws$/);
  expect((await http("/websocket")).headers["content-security-policy"]).toContain(`connect-src 'self' ${endpoint}`);
});

test("WebSocket entry echoes masked text frames and refuses foreign hosts or paths", async () => {
  const endpoint = JSON.parse((await http("/websocket.json")).body.toString()).websocket;
  const echoed = await new Promise((resolve, reject) => {
    const socket = new WebSocket(endpoint);
    const timer = setTimeout(() => { socket.close(); reject(new Error("no echo")); }, 5000);
    socket.addEventListener("open", () => socket.send("paneflow-fixture-0"));
    socket.addEventListener("message", (event) => { clearTimeout(timer); socket.close(); resolve(event.data); });
    socket.addEventListener("error", (event) => { clearTimeout(timer); reject(event); });
  });
  expect(echoed).toBe("paneflow-fixture-0");

  await expect(new Promise((resolve, reject) => {
    const socket = new WebSocket(endpoint.replace("/ws", "/not-ws"));
    socket.addEventListener("open", () => { socket.close(); resolve("connected"); });
    socket.addEventListener("error", () => reject(new Error("refused")));
    socket.addEventListener("close", () => reject(new Error("refused")));
  })).rejects.toThrow("refused");
});

test("manifest CLI and server identify identical fixture bytes", async () => {
  const result = invoke(["manifest"]);
  expect(result.status).toBe(0);
  const served = JSON.parse((await http("/manifest.json")).body.toString());
  expect(JSON.parse(result.stdout)).toEqual(served);
  expect(served.sha256).toBe(manifest.sha256);
});

test("HTTP entry rejects foreign hosts, mutations and paths outside fixture allowlist", async () => {
  expect((await http("/empty", { headers: { Host: "example.org" } })).status).toBe(403);
  expect((await http("/empty", { method: "POST" })).status).toBe(403);
  expect((await http("/../../Cargo.toml")).status).toBe(404);
  expect((await http("/missing")).status).toBe(404);
  expect((await http("/empty", { method: "HEAD" })).body.length).toBe(0);
});

test("CLI computes per-repetition nearest-rank percentiles without certifying synthetic data", async () => {
  const run = capture();
  for (const sample of run.repetitions[4].samples.slice(-3)) sample.present_ns += 1_000_000;
  const result = invoke(["inspect", await store("valid.json", run)]);
  expect(result.status).toBe(0);
  const report = JSON.parse(result.stdout);
  expect(report.status).toBe("ACCEPTED_SAMPLES");
  expect(report.qualification).toBe("NOT_EVALUATED");
  expect(report.samples).toBe(1000);
  expect(report.aggregate.p95_ns).toEqual({ median: 2_000_000, worst: 2_000_000 });
  expect(report.aggregate.p99_ns).toEqual({ median: 2_000_000, worst: 3_000_000 });
  expect(result.stdout).not.toContain("PASS");
});

const invalid = [
  ["missing calibration", (run) => { delete run.calibration; }],
  ["calibration uncertainty understated", (run) => { run.calibration.max_error_ns = 100_001; }],
  ["replay divergence", (run) => { run.replay.divergent_events = 1; }],
  ["replay schedule drift", (run) => { run.replay.max_delivery_error_ns = 2_000_001; }],
  ["replay event loss", (run) => { run.replay.observed_events--; }],
  ["missing timestamp", (run) => { delete run.repetitions[0].samples[0].present_ns; }],
  ["reverse timestamp", (run) => { run.repetitions[0].samples[0].present_ns = 0; }],
  ["unsafe timestamp", (run) => { run.repetitions[0].samples[0].input_ns = Number.MAX_SAFE_INTEGER + 1; }],
  ["missing sequence", (run) => { run.repetitions[0].samples[1].sequence = 4; }],
  ["duplicate timestamp", (run) => { run.repetitions[0].samples[1].input_ns = run.repetitions[0].samples[0].input_ns; }],
  ["warmup sample", (run) => { run.repetitions[0].samples[0].input_ns = 1; }],
  ["insufficient events", (run) => { run.repetitions[0].samples.splice(50, 1); }],
  ["short observation", (run) => { run.repetitions[0].samples = run.repetitions[0].samples.slice(0, 10); }],
  ["uncontrolled load", (run) => { run.load_controlled = false; }],
  ["fixture failure", (run) => { run.fixture_failed = true; }],
  ["insufficient repetitions", (run) => { run.repetitions.pop(); }],
  ["bad uncertainty", (run) => { run.uncertainty_ns = 500_001; }],
  ["null uncertainty", (run) => { run.uncertainty_ns = null; }],
  ["callback masquerading as present", (run) => { run.presentation_observation = "OnPaint"; }],
  ["uncontrolled driver", (run) => { delete run.environment.driver; }],
  ["debug build", (run) => { run.build_profile = "debug"; }],
  ["missing artifact", (run) => { run.artifacts[0].path = "absent.txt"; }],
  ["tampered artifact", (run) => { run.artifacts[0].sha256 = "1".repeat(64); }],
  ["artifact traversal", (run) => { run.artifacts[0].path = "../raw.txt"; }],
];

for (const [name, mutate] of invalid) {
  test(`inspect CLI rejects ${name} and produces no successful result`, async () => {
    const run = capture();
    mutate(run);
    const result = invoke(["inspect", await store("invalid.json", run)]);
    expect(result.status).toBe(1);
    expect(result.stdout).toBe("");
    expect(JSON.parse(result.stderr).status).toBe("REJECTED");
  });
}

test("CPU timing is accepted only with an explicit CPU metric and no presentation claim", async () => {
  const run = capture("A", "terminal_cpu");
  expect(invoke(["inspect", await store("cpu.json", run)]).status).toBe(0);
  run.presentation_observation = "compositor_feedback";
  expect(invoke(["inspect", await store("cpu-invalid.json", run)]).status).toBe(1);
});

test("compare CLI pairs A/C terminal captures and B/C browser captures", async () => {
  for (const [configuration, kind] of [["A", "terminal_input_to_present"], ["B", "browser_callback_to_present"]]) {
    const left = await store("reference.json", capture(configuration, kind));
    const right = await store("candidate.json", capture("C", kind, 2_500_000));
    const result = invoke(["compare", left, right]);
    expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout).deltas.p95_ns).toEqual({ median: 500_000, worst: 500_000 });
    expect(JSON.parse(result.stdout).qualification).toBe("NOT_EVALUATED");
  }
});

test("compare CLI rejects changed load, hardware, engine pin and combined clock uncertainty", async () => {
  const left = await store("reference.json", capture("B", "browser_callback_to_present"));
  for (const mutate of [
    (run) => { run.fixture_sha256 = "9".repeat(64); },
    (run) => { run.workload_sha256 = "9".repeat(64); },
    (run) => { run.environment.driver = "changed"; },
    (run) => { run.engine.cef_rs_commit = "9".repeat(40); },
    (run) => { run.uncertainty_ns = 950_000; },
  ]) {
    const right = capture("C", "browser_callback_to_present");
    mutate(right);
    const result = invoke(["compare", left, await store("candidate.json", right)]);
    expect(result.status).toBe(1);
    expect(result.stdout).toBe("");
  }
});

test("CLI rejects malformed JSON and unknown commands", async () => {
  const path = join(directory, "broken.json");
  await writeFile(path, "{");
  expect(invoke(["inspect", path]).status).toBe(1);
  expect(invoke(["unknown"]).status).toBe(1);
  expect(invoke(["serve", "65536"]).status).toBe(1);
  expect(invoke(["inspect"]).status).toBe(1);
});

test("replay CLI freezes four deterministic terminal streams and input schedule", () => {
  const first = invoke(["replay"]);
  expect(first.status).toBe(0);
  expect(invoke(["replay"]).stdout).toBe(first.stdout);
  const bundle = JSON.parse(first.stdout);
  expect(bundle.sha256).toBe(createHash("sha256").update(JSON.stringify({ protocol: bundle.protocol, events: bundle.events })).digest("hex"));
  expect(new Set(bundle.events.map(event => event.terminal)).size).toBe(4);
  expect(bundle.events.filter(event => event.input && event.at_ns >= 10_000_000_000).length * 5).toBeGreaterThanOrEqual(1000);
});

test("compare CLI admits pre-feature control with a different commit and rejects uncontrolled replay", async () => {
  const left = capture("PRE_FEATURE");
  const right = capture("A");
  right.paneflow_commit = "8".repeat(40);
  const path = await store("pre-feature.json", left);
  expect(invoke(["compare", path, await store("a.json", right)]).status).toBe(0);
  right.replay.divergent_events = 1;
  expect(invoke(["compare", path, await store("a.json", right)]).status).toBe(1);
});

test("draw-to-presentation evidence keeps its own measured boundary", async () => {
  const result = invoke(["inspect", await store("draw.json", capture("B", "browser_draw_to_present"))]);
  expect(result.status).toBe(0);
  expect(JSON.parse(result.stdout).kind).toBe("browser_draw_to_present");
});

test("idle CEF observation requires the complete window and never invents latency", async () => {
  const run = capture("B", "browser_idle");
  run.scenario = "empty";
  run.presentation_observation = "compositor_feedback";
  for (const repetition of run.repetitions) {
    repetition.samples = [];
    repetition.idle_evidence = { measured_frames: 0, monitoring_start_ns: 0, monitoring_end_ns: 71e9, fixture_reports: 71 };
  }
  const result = invoke(["inspect", await store("idle.json", run)]);
  expect(result.status).toBe(0);
  expect(JSON.parse(result.stdout)).toMatchObject({ samples: 0, qualification: "NOT_EVALUATED", latency: "NOT_APPLICABLE_NO_FRAMES_AFTER_WARMUP" });
  run.repetitions[0].idle_evidence.monitoring_end_ns = 69e9;
  expect(invoke(["inspect", await store("idle-short.json", run)]).status).toBe(1);
  run.repetitions[0].idle_evidence.monitoring_end_ns = 71e9;
  run.repetitions[0].samples = [{ sequence: 0, draw_ns: 10e9, present_ns: 11e9 }];
  expect(invoke(["inspect", await store("idle-invented.json", run)]).status).toBe(1);
});


test("browser B/C pair shares web load while C retains distinct terminal replay", async () => {
  const reference = capture("B", "browser_draw_to_present");
  const candidate = capture("C", "browser_draw_to_present");
  candidate.terminal_workload_sha256 = "7".repeat(64);
  candidate.replay.sha256 = candidate.terminal_workload_sha256;
  const left = await store("web-reference.json", reference);
  expect(invoke(["compare", left, await store("web-candidate.json", candidate)]).status).toBe(0);
  candidate.replay.sha256 = "8".repeat(64);
  expect(invoke(["compare", left, await store("web-candidate-corrupt.json", candidate)]).status).toBe(1);
  delete candidate.terminal_workload_sha256;
  candidate.replay.sha256 = candidate.workload_sha256;
  expect(invoke(["inspect", await store("web-candidate-conflated.json", candidate)]).status).toBe(1);
});

test("idle C requires both complete browser observation and valid terminal replay", async () => {
  const run = capture("C", "browser_idle");
  run.scenario = "empty";
  run.presentation_observation = "compositor_feedback";
  for (const repetition of run.repetitions) {
    repetition.samples = [];
    repetition.idle_evidence = { measured_frames: 0, fresh_captures: 0, monitoring_start_ns: 0, monitoring_end_ns: 71e9, fixture_reports: 71 };
  }
  expect(invoke(["inspect", await store("integrated-idle.json", run)]).status).toBe(0);
  run.repetitions[0].idle_evidence.fresh_captures = 1;
  expect(invoke(["inspect", await store("integrated-idle-fresh.json", run)]).status).toBe(1);
  run.repetitions[0].idle_evidence.fresh_captures = 0;
  run.replay.divergent_events = 1;
  expect(invoke(["inspect", await store("integrated-idle-replay.json", run)]).status).toBe(1);
});


test("comparison resolves terminal A/C and browser B/C to their distinct role outputs", async () => {
  for (const [configuration, kind, expectedOutput] of [["A", "terminal_input_to_present", "DP-3"], ["B", "browser_draw_to_present", "DP-4"]]) {
    const left = capture(configuration, kind);
    const right = capture("C", kind);
    for (const run of [left, right]) {
      run.environment.fullscreen = true;
      run.environment.refresh_actual_hz = 59.93939208984375;
      run.environment.display_evidence_directory = `/different/${run.configuration}`;
    }
    right.display_condition = { fullscreen: true, refresh_hz: 60, refresh_actual_hz: 59.93939208984375, terminal_output: "DP-3", browser_output: "DP-4" };
    delete right.environment.expected_output;
    if (configuration === "A") {
      left.display_condition = { ...right.display_condition, browser_output: null };
      delete left.environment.expected_output;
      delete left.environment.fullscreen;
    }
    const result = invoke(["compare", await store("display-reference.json", left), await store("display-candidate.json", right)]);
    expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout).display_condition).toEqual({ fullscreen: true, refresh_actual_hz: 59.93939208984375, role: kind.startsWith("browser_") ? "browser" : "terminal", output: expectedOutput });
  }
});

test.each([
  ["fullscreen mode", run => { run.environment.fullscreen = true; }, "unpaired display condition: fullscreen"],
  ["actual frequency", run => { run.environment.refresh_actual_hz = 59.95; }, "unpaired display condition: refresh_actual_hz"],
  ["small actual frequency difference", run => { run.environment.refresh_actual_hz += 0.00001; }, "unpaired display condition: refresh_actual_hz"],
  ["output connector", run => { run.environment.expected_output = "DP-3"; }, "unpaired display condition: output"],
  ["missing fullscreen", run => { delete run.environment.fullscreen; }, "explicit display condition required: fullscreen"],
  ["missing actual frequency", run => { delete run.environment.refresh_actual_hz; }, "explicit display condition required: refresh_actual_hz"],
  ["missing output", run => { delete run.environment.expected_output; }, "explicit display condition required: browser output"],
  ["conflicting fullscreen sources", run => { run.display_condition = { fullscreen: true }; }, "conflicting display condition: fullscreen"],
  ["conflicting frequency sources", run => { run.display_condition = { refresh_actual_hz: 59.95 }; }, "conflicting display condition: refresh_actual_hz"],
  ["conflicting nominal frequency", run => { run.display_condition = { refresh_hz: 120 }; }, "conflicting display condition: refresh_hz"],
  ["conflicting role output", run => { run.display_condition = { browser_output: "DP-3", terminal_output: "DP-4" }; }, "conflicting display condition: browser output"],
  ["terminal output cannot replace browser output", run => { delete run.environment.expected_output; run.display_condition = { terminal_output: "DP-4" }; }, "explicit display condition required: browser output"],
])("comparison rejects %s", async (_label, mutate, error) => {
  const left = capture("B", "browser_draw_to_present");
  const right = capture("C", "browser_draw_to_present");
  mutate(right);
  const result = invoke(["compare", await store("display-reference.json", left), await store("display-candidate.json", right)]);
  expect(result.status).toBe(1);
  expect(result.stderr).toContain(error);
});


test("paired latency budgets check the worst repetition and exact M1 boundaries", async () => {
  for (const [configuration, kind, limit] of [["A", "terminal_input_to_present", 1_000_000], ["B", "browser_draw_to_present", 2_000_000]]) {
    const reference = await store("budget-reference.json", capture(configuration, kind));
    const candidate = capture("C", kind, 2_000_000 + limit);
    const accepted = invoke(["compare", reference, await store("budget-boundary.json", candidate)]);
    expect(accepted.status).toBe(0);
    expect(JSON.parse(accepted.stdout)).toMatchObject({ qualification: "NOT_EVALUATED", latency_budget: { status: "SATISFIED" } });
    for (const sample of candidate.repetitions[4].samples) sample.present_ns += 1;
    const rejected = invoke(["compare", reference, await store("budget-over.json", candidate)]);
    expect(rejected.status).toBe(0);
    expect(JSON.parse(rejected.stdout)).toMatchObject({ qualification: "NOT_EVALUATED", latency_budget: { status: "EXCEEDED", aggregation: "worst_paired_repetition" } });
  }
});

test("terminal p99 budget cannot be hidden by a compliant p95", async () => {
  const reference = await store("p99-reference.json", capture());
  const candidate = capture("C", "terminal_input_to_present", 3_000_000);
  for (const sample of candidate.repetitions[4].samples.slice(-3)) sample.present_ns += 1_000_001;
  const result = invoke(["compare", reference, await store("p99-candidate.json", candidate)]);
  expect(result.status).toBe(0);
  const budget = JSON.parse(result.stdout).latency_budget;
  expect(budget.status).toBe("EXCEEDED");
  expect(budget.checks).toEqual([
    { metric: "p95_ns", limit_ns: 1_000_000, measured_delta_ns: 1_000_000, satisfied: true },
    { metric: "p99_ns", limit_ns: 2_000_000, measured_delta_ns: 2_000_001, satisfied: false },
  ]);
});

test("B/C missed-frame budget recomputes fixed-window evidence and ignores claimed verdicts", async () => {
  const left = capture("B", "browser_draw_to_present");
  const right = capture("C", "browser_draw_to_present");
  for (const run of [left, right]) {
    run.scenario = "animation";
    for (const repetition of run.repetitions) {
      repetition.origin_ns = 0;
      repetition.missed_frames = { status: "SATISFIED", evidence: {
        start_ns: 10e9, end_ns: 70e9, refresh_hz: 60, uncertainty_ns: 100_000,
        presentations: Array.from({ length: 3721 }, (_, index) => ({ present_ns: Math.round(9e9 + index * 1e9 / 60),
          refresh_ns: 16_666_667, output_sequence: String(index), fresh: true, content_id: String(index) })),
      } };
    }
  }
  const compare = async () => {
    const result = invoke(["compare", await store("missed-B.json", left), await store("missed-C.json", right)]);
    expect(result.status).toBe(0);
    return JSON.parse(result.stdout).missed_frame_budget;
  };
  expect((await compare()).status).toBe("SATISFIED");
  right.repetitions[4].missed_frames.evidence.presentations.slice(100, 140).forEach(point => { point.fresh = false; });
  expect((await compare()).status).toBe("EXCEEDED");
  delete right.repetitions[4].missed_frames.evidence;
  expect((await compare()).status).toBe("NOT_EVALUATED");
});
