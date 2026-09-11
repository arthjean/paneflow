import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, mkdirSync, existsSync, openSync, closeSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { fixture, analyze, distribution } from "./dock-benchmark.mjs";
import { correlate } from "./windows-analysis.mjs";
import { rpc } from "./windows-rpc.mjs";

const root = resolve(import.meta.dirname, "../..");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const save = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2));
const lines = (path) => {
  if (!existsSync(path)) return [];
  const text = readFileSync(path, "utf8");
  return text.slice(0, text.lastIndexOf("\n")).split(/\r?\n/).filter(Boolean).map(JSON.parse);
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const exited = (child) => new Promise((resolve, reject) => {
  child.once("error", reject);
  child.once("exit", (code) => code === 0 ? resolve() : reject(new Error(`Process ${child.pid} exited ${code}`)));
});

export function summarize(events, phases) {
  if (!phases.length || phases.some((phase, i) => phase.clock !== "QPC"
    || !Number.isSafeInteger(phase.at_ns) || (i > 0 && phase.at_ns <= phases[i - 1].at_ns))) {
    throw new Error("Invalid QPC phases");
  }
  if (events.some((event) => event.dropped > 0)) throw new Error("Benchmark events were dropped");
  const measured = analyze(events, phases).filter((phase) => ["steady", "scroll", "resize"].includes(phase.phase));
  for (const name of ["steady", "scroll", "resize"]) {
    const phase = measured.find((phase) => phase.phase === name);
    if (!phase?.frames || phase.host_prepare_ms.p95 < 0 || phase.ready_to_receive_ms.p95 < 0) {
      throw new Error(`Invalid ${name} frame evidence`);
    }
    if (name === "scroll" && phase.wheels < 10) throw new Error("Wheel input did not reach the dock");
    if (name === "resize" && (phase.resizes < 10 || phase.resize_ready_ms.count < 10)) {
      throw new Error("Resize input did not reach the dock");
    }
  }
  return measured;
}

async function capture(bundle, directory, seconds) {
  mkdirSync(directory);
  const binary = join(root, "target/release/paneflow.exe");
  const runtime = join(bundle, "browser");
  const host = join(bundle, "paneflow-browser-host.exe");
  const client = join(bundle, "paneflow-browser-host.dll");
  const presentmon = join(root, "target/PresentMon-2.5.1-x64.exe");
  for (const path of [binary, host, client, presentmon, join(runtime, "Release/libcef.dll")]) {
    if (!existsSync(path)) throw new Error(`Required input missing: ${path}`);
  }
  const runtimeHash = execFileSync("python", ["-c",
    "import importlib.util,pathlib,sys;s=importlib.util.spec_from_file_location('bp',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(m.runtime_digest(pathlib.Path(sys.argv[2])))",
    join(root, "scripts/browser-package.py"), runtime], { encoding: "utf8" }).trim();
  const frequency = Number(execFileSync("python", ["-c",
    "import ctypes;x=ctypes.c_longlong();ctypes.windll.kernel32.QueryPerformanceFrequency(ctypes.byref(x));print(x.value)"], { encoding: "utf8" }).trim());
  const metadata = { schema: 1, clock: "QPC", platform: process.platform, arch: process.arch,
    date: new Date().toISOString(), seconds, qpc_frequency: frequency,
    fixture_sha256: hash(fixture), app_sha256: hash(readFileSync(binary)),
    host_sha256: hash(readFileSync(host)), client_sha256: hash(readFileSync(client)),
    runtime_sha256: runtimeHash, frame_rate: process.env.PANEFLOW_BROWSER_FRAME_RATE ?? "60",
    git_head: execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim(),
    tracked_diff_sha256: hash(execFileSync("git", ["diff", "HEAD"], { cwd: root, maxBuffer: 16e6 })),
    input: "Native wheel 15Hz, alternating direction every 5s; native dock drag 60Hz, 180 logical pixels, 2s period",
    limitations: ["Same Linux fixture and phase durations; automated gestures differ from Linux manual input.",
      "No direct cross-platform regression verdict; screen and driver conditions differ.",
      "PresentMon is OS display timing, not input-to-photon measurement."],
  };
  const server = createServer((req, res) => {
    if (req.url !== "/") { res.writeHead(404).end(); return; }
    res.setHeader("Cache-Control", "no-store");
    res.setHeader("Content-Type", "text/html; charset=utf-8");
    res.end(fixture);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const pipe = `\\\\.\\pipe\\paneflow-dock-${process.pid}-${Date.now()}`;
  const descriptors = [];
  const log = (name) => { const fd = openSync(join(directory, name), "w"); descriptors.push(fd); return fd; };
  let app, trace, input;
  const traceSession = `Paneflow-Dock-${process.pid}-${Date.now()}`;
  try {
    app = spawn(binary, [], { cwd: root, windowsHide: false, stdio: ["ignore", log("stdout.log"), log("stderr.log")], env: {
      ...process.env, PANEFLOW_BROWSER_HOST: host, PANEFLOW_CEF_ROOT: runtime,
      PANEFLOW_BROWSER_QUALIFICATION_SHA256: runtimeHash,
      PANEFLOW_BROWSER_STAGE_ROOT: join(directory, "staging"),
      PANEFLOW_M1_LOG: join(directory, "application.jsonl"), PANEFLOW_M1_STATE_ROOT: join(directory, "state"),
      PANEFLOW_BROWSER_BENCH: join(directory, "events.jsonl"), PANEFLOW_SOCKET_PATH: pipe,
      PANEFLOW_IPC_SCRIPTING: "1",
    } });
    metadata.pid = app.pid;
    save(join(directory, "metadata.json"), metadata);
    let ready = false;
    for (let i = 0; i < 60; i++) {
      if (app.exitCode !== null) throw new Error(`App startup failed: ${app.exitCode}`);
      try { await rpc(pipe, "workspace.list"); ready = true; break; } catch { await sleep(500); }
    }
    if (!ready) throw new Error("Isolated application IPC unavailable");
    await rpc(pipe, "workspace.up", { name: "Browser dock measurement", layout: "tiled",
      panes: [{ name: "Idle terminal", cwd: root, command: 'python -c "import time; time.sleep(600)"' }] });
    await rpc(pipe, "qualification.browser.open", { url: `http://127.0.0.1:${server.address().port}/` });
    for (let i = 0; i < 120; i++) {
      if (lines(join(directory, "events.jsonl")).some((row) => row.event === "frame_received")) { ready = true; break; }
      ready = false;
      if (app.exitCode !== null) throw new Error(`App exited before browser frame: ${app.exitCode}`);
      await sleep(500);
    }
    if (!ready) throw new Error("No browser frame received");
    trace = spawn(presentmon, ["--process_id", String(app.pid), "--output_file", join(directory, "presentmon.csv"),
      "--qpc_time", "--timed", String(Math.ceil(seconds * 3 + 23)), "--terminate_after_timed",
      "--no_console_stats", "--no_track_gpu", "--no_track_input", "--session_name", traceSession],
      { windowsHide: true, stdio: ["ignore", log("presentmon.stdout.log"), log("presentmon.stderr.log")] });
    const traceDone = exited(trace);
    traceDone.catch(() => {});
    input = spawn("python", [join(import.meta.dirname, "windows-dock-input.py"), String(app.pid), directory, String(seconds)],
      { windowsHide: true, stdio: ["ignore", "inherit", "inherit"] });
    await exited(input);
    await traceDone;
    await sleep(500);
    const phases = JSON.parse(readFileSync(join(directory, "phases.json"), "utf8"));
    const events = lines(join(directory, "events.jsonl"));
    const measured = summarize(events, phases);
    const application = lines(join(directory, "application.jsonl"));
    const csv = readFileSync(join(directory, "presentmon.csv"), "utf8");
    const presentation = measured.map((phase) => {
      const start = phases.find((entry) => entry.name === phase.phase).at_ns;
      const result = correlate(application, csv, { pid: app.pid, qpc_frequency: frequency,
        origin_ns: start, warmup_seconds: 0, duration_seconds: seconds });
      const browser = result.samples.filter((sample) => sample.kind === "browser");
      const display = result.samples.filter((sample) => sample.kind === "presentation");
      return { phase: phase.phase, matched: display.length, unmatched: result.unmatched, dropped: result.dropped,
        callback_to_display_ms: distribution(browser.map((sample) => sample.latency_ns / 1e6)),
        scene_to_display_ms: distribution(browser.map((sample) => sample.draw_to_present_ns / 1e6)),
        display_gap_ms: distribution(display.slice(1).map((sample, i) => (sample.displayed_ns - display[i].displayed_ns) / 1e6)) };
    });
    const report = { status: presentation.some((phase) => phase.unmatched || !phase.matched)
      ? "INVALID_PRESENTATION_CORRELATION" : "OBSERVED_NOT_BUDGET_CERTIFIED", phases: measured, presentation };
    save(join(directory, "summary.json"), report);
    console.log(JSON.stringify({ directory, ...report }));
    return report;
  } catch (error) {
    save(join(directory, "failure.json"), { error: error.message });
    throw error;
  } finally {
    if (input && input.exitCode === null) input.kill();
    if (trace && trace.exitCode === null) trace.kill();
    if (app && app.exitCode === null) app.kill();
    try { execFileSync("logman", ["stop", traceSession, "-ets"], { stdio: "ignore" }); } catch {}
    if (input) {
      execFileSync("python", ["-c", "import ctypes;ctypes.windll.user32.mouse_event(4,0,0,0,0)"], { windowsHide: true });
    }
    server.close();
    for (const fd of descriptors) closeSync(fd);
  }
}

if (import.meta.main) {
  const [bundle, output, repetitions = "3", seconds = "20"] = process.argv.slice(2);
  if (!bundle || !output || process.platform !== "win32") throw new Error("Usage: bun windows-dock-benchmark.mjs <bundle> <new-output> [repetitions=3] [seconds=20]");
  const destination = resolve(output);
  mkdirSync(destination);
  for (let i = 1; i <= Number(repetitions); i++) {
    await capture(resolve(bundle), join(destination, `r${i}`), Number(seconds));
    await sleep(2000);
  }
}
