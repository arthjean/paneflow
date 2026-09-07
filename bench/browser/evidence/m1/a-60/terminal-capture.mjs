import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { replayBundle } from "./replay.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const worker = fileURLToPath(new URL("./replay-worker.py", import.meta.url));
const pause = milliseconds => new Promise(done => setTimeout(done, milliseconds));
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const json = async path => JSON.parse(await readFile(path, "utf8"));
const jsonl = async path => (await readFile(path, "utf8")).trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
const save = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + "\n", { flag: "wx", mode: 0o600 });
const quote = text => "'" + text.replaceAll("'", "'\\''") + "'";

function rpc(path, method, params = {}) {
  return new Promise((done, reject) => {
    const connection = createConnection(path);
    let bytes = Buffer.alloc(0);
    connection.setTimeout(2000, () => connection.destroy(new Error("private IPC timeout")));
    connection.once("error", reject);
    connection.once("connect", () => connection.write(JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) + "\n"));
    connection.on("data", chunk => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > 262144) { connection.destroy(new Error("IPC response exceeds 256 KiB")); return; }
      const end = bytes.indexOf(10);
      if (end === -1) return;
      try {
        const response = JSON.parse(bytes.subarray(0, end));
        if (response.error || response.result?.error || response.id !== 1) throw new Error(JSON.stringify(response));
        done(response.result);
      } catch (error) { reject(error); }
      connection.destroy();
    });
    connection.once("end", () => reject(new Error("private IPC ended before its response")));
  });
}

async function identity(pid) {
  try {
    const line = await readFile(`/proc/${pid}/stat`, "utf8");
    const fields = line.slice(line.lastIndexOf(")") + 2).split(" ");
    return { pid, parent: Number(fields[1]), start_ticks: fields[19] };
  } catch (error) {
    if (["ENOENT", "ESRCH"].includes(error.code)) return null;
    throw error;
  }
}

async function descendants(pid) {
  const all = (await Promise.all((await readdir("/proc")).filter(name => /^\d+$/.test(name)).map(name => identity(Number(name))))).filter(Boolean);
  const owned = new Set([pid]);
  let previous = 0;
  while (previous !== owned.size) {
    previous = owned.size;
    for (const item of all) if (owned.has(item.parent)) owned.add(item.pid);
  }
  return all.filter(item => owned.has(item.pid));
}

async function stopProcesses(owned) {
  const alive = async item => (await identity(item.pid))?.start_ticks === item.start_ticks;
  const signal = async (item, name) => {
    if (!await alive(item)) return;
    try { process.kill(item.pid, name); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  };
  for (const item of [...owned].reverse()) await signal(item, "SIGTERM");
  await pause(500);
  for (const item of [...owned].reverse()) await signal(item, "SIGKILL");
}

async function waitFor(predicate, timeout, assertRunning) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    assertRunning();
    if (await predicate()) return;
    await pause(25);
  }
  throw new Error("qualification readiness/completion timeout");
}

async function exists(path) {
  try { await stat(path); return true; }
  catch (error) { if (error.code === "ENOENT") return false; throw error; }
}

async function inspectReplay(directory, bundle, seconds) {
  const expected = bundle.events.filter(event => event.at_ns < seconds * 1e9);
  const inputs = await jsonl(join(directory, "inputs.jsonl"));
  const native = await jsonl(join(directory, "native.jsonl"));
  const workers = (await Promise.all([0, 1, 2, 3].map(id => jsonl(join(directory, `worker-${id}.jsonl`))))).flat();
  const deliveredInputs = inputs.filter(event => event.event === "input").flatMap(event => {
    const tick = Number(event.input.slice(1).trim());
    return native.filter(item => item.event === "input" && item.surface_id === event.surface_id && item.tick === tick).map(item => ({ ...event, actual_ns: item.input_ns, completed_ns: item.completed_ns }));
  });
  const observed = [...deliveredInputs, ...workers.filter(event => event.event === "output")];
  const indexed = new Map();
  const errors = [...inputs, ...workers].filter(event => event.event === "fatal").map(event => event.message);
  let divergent = 0;
  let maxError = 0;
  for (const event of observed) {
    if (indexed.has(event.sequence)) { divergent++; continue; }
    indexed.set(event.sequence, event);
    maxError = Math.max(maxError, Math.abs(event.actual_ns - event.planned_ns), Math.max(0, (event.completed_ns ?? event.sent_ns) - event.planned_ns));
  }
  for (const event of expected) {
    const actual = indexed.get(event.sequence);
    if (!actual || actual.terminal !== event.terminal) { divergent++; continue; }
    if (event.output_base64) {
      const bytes = Buffer.from(event.output_base64, "base64");
      if (actual.sha256 !== hash(bytes) || actual.bytes !== bytes.length) divergent++;
      const geometry = bundle.protocol.terminal_geometry[event.terminal];
      if (actual.columns !== geometry.columns || actual.rows !== geometry.rows) errors.push(`terminal ${event.terminal} geometry ${actual.columns}x${actual.rows} differs from ${geometry.columns}x${geometry.rows}`);
    } else {
      if (actual.input !== event.input) divergent++;
      const tick = Number(event.input.slice(1).trim());
      const echoes = workers.filter(item => item.event === "echo" && item.terminal === event.terminal && item.tick === tick);
      if (echoes.length !== 1 || ![Buffer.from(event.input).toString("base64"), Buffer.from(event.input.replace("\n", "\r")).toString("base64")].includes(echoes[0]?.input_base64)) divergent++;
    }
  }
  if (observed.length !== expected.length || divergent !== 0) errors.push("replay has missing, duplicate or divergent output/input events");
  if (maxError > bundle.protocol.max_delivery_error_ns) errors.push(`replay delivery error ${maxError} ns exceeds ${bundle.protocol.max_delivery_error_ns} ns`);
  return { sha256: bundle.sha256, expected_events: expected.length, observed_events: observed.length, divergent_events: divergent, max_delivery_error_ns: maxError, errors: [...new Set(errors)] };
}

async function captureRepetition(binary, directory, bundle, seconds, interrupted, expectedRefreshHz) {
  await mkdir(directory, { mode: 0o700 });
  const paths = { config: join(directory, "config"), data: join(directory, "data"), cache: join(directory, "cache"), work: join(directory, "work"), socket: join(directory, "ipc.sock"), native: join(directory, "native.jsonl") };
  if (Buffer.byteLength(paths.socket) >= 108) throw new Error("private IPC path exceeds Linux sockaddr_un; use a shorter output directory");
  for (const path of [paths.config, paths.data, paths.cache, paths.work]) await mkdir(path, { mode: 0o700 });
  await mkdir(join(paths.config, "paneflow"), { mode: 0o700 });
  await save(join(paths.config, "paneflow/paneflow.json"), { default_shell: "/bin/sh", font_family: bundle.protocol.font, font_size: bundle.protocol.font_size, line_height: bundle.protocol.line_height, shell_integration: false, agent_stall_detection: false, reduce_motion: true, window_backdrop: "off", telemetry: { enabled: false } });
  await save(join(paths.config, "paneflow/window-state.json"), { width: 1920, height: 1080 });
  await save(join(directory, "replay-plan.json"), bundle);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("PANEFLOW_")));
  Object.assign(env, { XDG_CONFIG_HOME: paths.config, XDG_DATA_HOME: paths.data, XDG_CACHE_HOME: paths.cache, PANEFLOW_SOCKET_PATH: paths.socket, PANEFLOW_ALLOW_MULTIPLE: "1", PANEFLOW_IPC_ORCHESTRATION: "1", PANEFLOW_IPC_SCRIPTING: "1", PANEFLOW_M1_LOG: paths.native });
  const app = spawn(binary, [], { cwd: paths.work, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
  let appFailure;
  let coordinator;
  const knownProcesses = [];
  const output = createWriteStream(join(directory, "app.stdout.log"), { flags: "wx", mode: 0o600 });
  const stderr = createWriteStream(join(directory, "app.stderr.log"), { flags: "wx", mode: 0o600 });
  app.stdout.pipe(output);
  app.stderr.pipe(stderr);
  app.once("error", error => { appFailure = error.message; });
  app.once("exit", (code, signal) => { appFailure = `application exited ${code ?? signal}`; });
  const assertRunning = () => { if (interrupted()) throw new Error("capture interrupted"); if (appFailure) throw new Error(appFailure); };
  const result = { status: "REJECTED", errors: [], native_log: "native.jsonl", clock: "CLOCK_MONOTONIC", application_pid: app.pid, duration_seconds: seconds };
  try {
    if (app.pid) { const item = await identity(app.pid); if (item) knownProcesses.push(item); }
    await waitFor(async () => { try { return Boolean(await rpc(paths.socket, "system.capabilities")); } catch { return false; } }, 60_000, assertRunning);
    const panes = Array.from({ length: 4 }, (_, terminal) => ({ name: `M1-${terminal}`, cwd: paths.work, command: `exec python3 ${quote(worker)} terminal --directory ${quote(directory)} --terminal ${terminal}`, focus: terminal === 0 }));
    const workspace = await rpc(paths.socket, "workspace.up", { name: "M1 qualification", layout: "tiled", panes });
    await save(join(directory, "workspace.json"), workspace);
    if (workspace.surface_ids?.length !== 4 || workspace.panes !== 4) throw new Error("workspace.up did not create four real terminal surfaces");
    const workspaces = await rpc(paths.socket, "workspace.list");
    for (const item of [...workspaces.workspaces].sort((left, right) => right.index - left.index)) {
      if (item.index !== workspace.index) await rpc(paths.socket, "workspace.close", { index: item.index });
    }
    await waitFor(async () => (await Promise.all([0, 1, 2, 3].map(id => exists(join(directory, `worker-${id}-ready.json`))))).every(Boolean), 45_000, assertRunning);
    knownProcesses.push(...await descendants(app.pid));
    await waitFor(async () => {
      try {
        const native = await jsonl(paths.native);
        const fatal = native.find(event => event.event === "fatal");
        if (fatal) throw new Error(`native observer initialization failed: ${JSON.stringify(fatal)}`);
        return native.some(event => event.event === "presentation_ready");
      } catch (error) {
        if (error.code === "ENOENT" || error instanceof SyntaxError) return false;
        throw error;
      }
    }, 10_000, assertRunning);
    assertRunning();
    coordinator = spawn("python3", [worker, "coordinator", "--directory", directory, "--seconds", String(seconds), "--socket", paths.socket, "--surfaces", JSON.stringify(workspace.surface_ids)], { stdio: ["ignore", "pipe", "pipe"] });
    const coordinatorLog = createWriteStream(join(directory, "coordinator.log"), { flags: "wx", mode: 0o600 });
    coordinator.stdout.pipe(coordinatorLog, { end: false });
    coordinator.stderr.pipe(coordinatorLog);
    let coordinatorError;
    coordinator.once("error", error => { coordinatorError = error.message; });
    if (coordinator.pid) { const item = await identity(coordinator.pid); if (item) knownProcesses.push(item); }
    if (expectedRefreshHz !== undefined) {
      await waitFor(async () => {
        let native;
        try { native = await jsonl(paths.native); }
        catch (error) { if (error instanceof SyntaxError) return false; throw error; }
        const first = native.find(event => event.event === "presented");
        if (!first) return false;
        if (!first.refresh_ns || Math.abs(1e9 / first.refresh_ns - expectedRefreshHz) > 0.1) throw new Error(`native refresh ${1e9 / first.refresh_ns} Hz differs from the requested ${expectedRefreshHz} Hz condition`);
        return true;
      }, 10_000, assertRunning);
    }
    await waitFor(async () => {
      if (coordinatorError) throw new Error(coordinatorError);
      return (await Promise.all(["coordinator-complete.json", ...[0, 1, 2, 3].map(id => `worker-${id}-complete.json`)].map(name => exists(join(directory, name))))).every(Boolean);
    }, (seconds + 10) * 1000, assertRunning);
    result.replay = await inspectReplay(directory, bundle, seconds);
    result.errors.push(...result.replay.errors);
    result.origin_ns = (await json(join(directory, "start.json"))).origin_ns;
    result.surface_ids = workspace.surface_ids;
    const native = await jsonl(paths.native);
    result.native_events = native.length;
    result.native_presented = native.filter(event => event.event === "presented" || event.type === "presented").length;
    result.errors.push(...native.filter(event => ["fatal", "discarded"].includes(event.event ?? event.type)).map(event => JSON.stringify(event)));
    if (result.native_presented === 0) result.errors.push("native capture contains no compositor presentation proof");
    let dispatchError = 0;
    for (const event of bundle.events.filter(event => event.input && event.at_ns < seconds * 1e9)) {
      const tick = Number(event.input.slice(1).trim());
      const surface = workspace.surface_ids[event.terminal];
      const observed = native.filter(item => item.event === "input" && item.surface_id === surface && item.tick === tick);
      const presented = native.filter(item => item.event === "presented" && item.surface_id === surface && item.terminal === event.terminal && item.tick === tick);
      if (observed.length !== 1 || presented.length !== 1) result.errors.push(`input ${event.terminal}:${tick} lacks unique handler and presentation evidence`);
      for (const item of observed) dispatchError = Math.max(dispatchError, Math.abs(item.input_ns - result.origin_ns - event.at_ns));
    }
    result.max_input_dispatch_error_ns = dispatchError;
    if (dispatchError > bundle.protocol.max_delivery_error_ns) result.errors.push(`input handler delivery error ${dispatchError} ns exceeds ${bundle.protocol.max_delivery_error_ns} ns`);
    const viewports = native.filter(event => event.event === "viewport");
    const initialViewport = viewports.filter(event => event.at_ns <= result.origin_ns).at(-1);
    if (!initialViewport || [initialViewport, ...viewports.filter(event => event.at_ns > result.origin_ns)].some(event => event.width_px !== 1920 || event.height_px !== 1080)) result.errors.push("observed physical viewport differs from 1920x1080");
    result.errors = [...new Set(result.errors)];
    result.status = result.errors.length ? "REJECTED" : "CAPTURED_REQUIRES_ANALYSIS";
  } catch (error) { result.errors.push(error.message); }
  finally {
    const appIdentity = knownProcesses.find(item => item.pid === app.pid);
    const liveApp = appIdentity && (await identity(app.pid))?.start_ticks === appIdentity.start_ticks;
    const owned = [...knownProcesses, ...(liveApp ? await descendants(app.pid) : [])];
    for (const terminal of [0, 1, 2, 3]) {
      try {
        const ready = await json(join(directory, `worker-${terminal}-ready.json`));
        const item = await identity(ready.pid);
        if (!item || item.start_ticks !== ready.start_ticks) continue;
        const args = (await readFile(`/proc/${ready.pid}/cmdline`, "utf8")).split("\0");
        if (args.includes(worker) && args.includes(directory)) owned.push(item);
      } catch (error) { if (!["ENOENT", "ESRCH"].includes(error.code)) throw error; }
    }
    await writeFile(join(directory, "stop"), "", { flag: "wx", mode: 0o600 });
    await stopProcesses(owned);
    await save(join(directory, "capture.json"), result);
  }
  return result;
}

export async function captureTerminals({ binary, output, configuration = "A", diagnosticSeconds, sourceRoot = root, refreshHz }) {
  if (process.platform !== "linux" || !process.env.WAYLAND_DISPLAY) throw new Error("terminal native capture requires a Linux Wayland session");
  if (!["A", "PREFEATURE"].includes(configuration)) throw new Error("terminal capture configuration must be A or PREFEATURE");
  if (!binary || !output) throw new Error("--binary and --output are required");
  if (diagnosticSeconds !== undefined && (!Number.isInteger(diagnosticSeconds) || diagnosticSeconds < 1 || diagnosticSeconds > 69)) throw new Error("diagnostic duration must be 1..69 seconds");
  const expectedRefreshHz = refreshHz ?? (diagnosticSeconds === undefined ? 60 : undefined);
  if (expectedRefreshHz !== undefined && ![60, 120].includes(expectedRefreshHz)) throw new Error("qualification refresh must be 60 or 120 Hz");
  const directory = resolve(output);
  const executable = resolve(binary);
  const source = resolve(sourceRoot);
  await mkdir(directory, { mode: 0o700 });
  const bundle = await replayBundle();
  const commit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: source, encoding: "utf8" });
  const diff = spawnSync("git", ["diff", "--binary", "HEAD"], { cwd: source, encoding: "buffer", maxBuffer: 32 * 1024 * 1024 });
  const files = spawnSync("git", ["ls-files", "-co", "--exclude-standard", "-z"], { cwd: source, encoding: "utf8", maxBuffer: 8 * 1024 * 1024 });
  if (commit.status !== 0 || diff.status !== 0 || files.status !== 0) throw new Error("cannot archive source provenance");
  await writeFile(join(directory, "source.patch"), diff.stdout, { flag: "wx", mode: 0o600 });
  const sourceHashes = [];
  for (const path of [...new Set(files.stdout.split("\0").filter(path => /(?:\.rs|\.toml|Cargo\.lock)$/.test(path)))].sort()) sourceHashes.push({ path, sha256: hash(await readFile(join(source, path))) });
  await save(join(directory, "source-files.json"), sourceHashes);
  const runnerSources = [];
  for (const name of ["terminal-capture.mjs", "replay-worker.py", "replay.mjs"]) {
    const bytes = await readFile(new URL(name, import.meta.url));
    await writeFile(join(directory, name), bytes, { flag: "wx", mode: 0o600 });
    runnerSources.push({ path: name, sha256: hash(bytes) });
  }
  await save(join(directory, "runner-sources.json"), runnerSources);
  let interrupted = false;
  const interrupt = () => { interrupted = true; };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  const report = { schema_version: 1, configuration, purpose: diagnosticSeconds === undefined ? "qualification_capture" : "diagnostic_only", status: "REJECTED", qualification: "NOT_EVALUATED", build_profile: "release_required", binary: executable, binary_sha256: hash(await readFile(executable)), source_directory: source, source_binding: "build evidence must bind this binary to the archived source inventory", commit: commit.stdout.trim(), tracked_source_diff_sha256: hash(diff.stdout), source_files_sha256: hash(JSON.stringify(sourceHashes)), workload_sha256: bundle.sha256, input_source: "synthetic_guarded_ipc_to_normal_gpui_key_handler", clock: "CLOCK_MONOTONIC", repetitions: [] };
  try {
    const repetitions = diagnosticSeconds === undefined ? 5 : 1;
    const seconds = diagnosticSeconds ?? bundle.protocol.warmup_seconds + bundle.protocol.duration_seconds;
    for (let index = 0; index < repetitions && !interrupted; index++) {
      process.stderr.write(`M1 ${configuration}: repetition ${index + 1}/${repetitions}, ${seconds} seconds\n`);
      const result = await captureRepetition(executable, join(directory, `r${index + 1}`), bundle, seconds, () => interrupted, expectedRefreshHz);
      report.repetitions.push(result);
      if (result.status === "REJECTED") break;
    }
    if (report.repetitions.length === repetitions && report.repetitions.every(item => item.status === "CAPTURED_REQUIRES_ANALYSIS")) report.status = diagnosticSeconds === undefined ? "CAPTURED_REQUIRES_ANALYSIS" : "DIAGNOSTIC_ONLY";
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
    await save(join(directory, "capture.json"), report);
  }
  return report;
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ options: { binary: { type: "string" }, output: { type: "string" }, configuration: { type: "string", default: "A" }, "diagnostic-seconds": { type: "string" }, "source-root": { type: "string" }, "refresh-hz": { type: "string" }, help: { type: "boolean" } } });
    if (values.help) process.stdout.write("bun scripts/browser-qualification/terminal-capture.mjs --binary <instrumented-release-paneflow> --output <new-directory> --configuration A|PREFEATURE [--source-root <binary-source-checkout>] [--diagnostic-seconds 3]\n");
    else {
      const report = await captureTerminals({ binary: values.binary, output: values.output, configuration: values.configuration, sourceRoot: values["source-root"], refreshHz: values["refresh-hz"] === undefined ? undefined : Number(values["refresh-hz"]), diagnosticSeconds: values["diagnostic-seconds"] === undefined ? undefined : Number(values["diagnostic-seconds"]) });
      process.stdout.write(JSON.stringify(report, null, 2) + "\n");
      if (report.status === "REJECTED") process.exitCode = 1;
    }
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
