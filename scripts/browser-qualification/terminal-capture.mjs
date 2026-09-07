import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { constants, createReadStream, createWriteStream } from "node:fs";
import { copyFile, mkdir, readFile, readdir, realpath, stat, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { replayBundle } from "./replay.mjs";
import { fixtureBundle, serveFixtures } from "./fixtures.mjs";
import { collectBrowserProcessEvidence } from "./process-evidence.mjs";
import { inspectFullscreenEvidence } from "./fullscreen-evidence.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const worker = fileURLToPath(new URL("./replay-worker.py", import.meta.url));
const pause = milliseconds => new Promise(done => setTimeout(done, milliseconds));
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const json = async path => JSON.parse(await readFile(path, "utf8"));
const jsonl = async path => (await readFile(path, "utf8")).trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
const save = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + "\n", { flag: "wx", mode: 0o600 });
const quote = text => "'" + text.replaceAll("'", "'\\''") + "'";

const digest = /^[a-f0-9]{64}$/;
const commitPattern = /^[a-f0-9]{40}$/;
const sourcePath = path => typeof path === "string" && path.length > 0 && !path.startsWith("/") && !path.includes("\\") && !path.split("/").includes("..");
const compiledSource = path => /(?:\.rs|\.toml|Cargo\.lock)$/.test(path);
const integer = value => Number.isSafeInteger(value) && value >= 0;

export async function sourceProvenance(source) {
  if (await exists(join(source, ".git"))) {
    const commit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: source, encoding: "utf8" });
    const diff = spawnSync("git", ["diff", "--binary", "HEAD"], { cwd: source, encoding: "buffer", maxBuffer: 32 * 1024 * 1024 });
    const files = spawnSync("git", ["ls-files", "-co", "--exclude-standard", "-z"], { cwd: source, encoding: "utf8", maxBuffer: 8 * 1024 * 1024 });
    if (commit.status !== 0 || diff.status !== 0 || files.status !== 0) throw new Error("cannot archive source provenance");
    const sourceHashes = [];
    for (const path of [...new Set(files.stdout.split("\0").filter(compiledSource))].sort()) sourceHashes.push({ path, sha256: hash(await readFile(join(source, path))) });
    return { commit: commit.stdout.trim(), commit_provenance: "git_head_plus_archived_worktree", patch: diff.stdout, sourceHashes, snapshot: null };
  }
  const snapshot = await readFile(join(source, "snapshot-provenance.json"));
  const receipt = JSON.parse(snapshot);
  if (!commitPattern.test(receipt.head) || !receipt.files_sha256 || typeof receipt.files_sha256 !== "object" || Array.isArray(receipt.files_sha256)) throw new Error("snapshot requires a declared base head and files_sha256 inventory");
  const inventory = { ...receipt.files_sha256 };
  for (const [path, override] of Object.entries(receipt.snapshot_overrides ?? {})) inventory[path] = override.sha256;
  for (const [path, sha256] of Object.entries(receipt.embedded_helper_artifacts ?? {})) inventory[path] = sha256;
  if (!Object.keys(inventory).some(path => path.startsWith("src-app/") && path.endsWith(".rs")) || !inventory["Cargo.toml"] || !inventory["Cargo.lock"]) throw new Error("snapshot inventory omits compiled application or workspace sources");
  const sourceHashes = [];
  for (const [path, expected] of Object.entries(inventory).sort(([left], [right]) => left.localeCompare(right))) {
    if (!sourcePath(path) || !digest.test(expected)) throw new Error("invalid snapshot source inventory entry");
    const actual = hash(await readFile(join(source, path)));
    if (actual !== expected) throw new Error(`snapshot source differs from provenance: ${path}`);
    if (compiledSource(path) || Object.hasOwn(receipt.embedded_helper_artifacts ?? {}, path)) sourceHashes.push({ path, sha256: actual });
  }
  const scan = async (directory, relative = "") => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (["target", ".git", "prebuilt", "checkout"].includes(entry.name)) continue;
      const path = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await scan(join(directory, entry.name), path);
      else if (compiledSource(path) && !Object.hasOwn(inventory, path)) throw new Error(`compiled source missing from snapshot provenance: ${path}`);
    }
  };
  await scan(source);
  return { commit: receipt.head, commit_provenance: "snapshot_declared_base_plus_verified_inventory", patch: null, sourceHashes, snapshot };
}

export async function verifyBrowserRuntime(source, host, runtime) {
  if (!host || !runtime) throw new Error("configuration C requires explicit --host and --runtime");
  const executable = resolve(host);
  const runtimeRoot = resolve(runtime);
  const hostStat = await stat(executable);
  if (!hostStat.isFile() || !(hostStat.mode & 0o111)) throw new Error("Browser host is not an executable file");
  const manifestPath = join(source, "native/browser/manifest.toml");
  const manifestBytes = await readFile(manifestPath);
  const manifest = Bun.TOML.parse(manifestBytes.toString());
  const target = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-unknown-linux-gnu`;
  const candidate = manifest.targets?.[target];
  if (!candidate || !digest.test(candidate.sha256) || candidate.availability === "absent") throw new Error(`no runtime candidate for ${target}`);
  const destination = dirname(dirname(runtimeRoot));
  if (resolve(destination, target, candidate.sha256) !== runtimeRoot) throw new Error("runtime path must identify the manifest target and archive digest");
  const command = [join(root, "scripts/fetch-browser.py"), "--target", target, "--manifest", manifestPath, "--destination", destination, "--verify-only"];
  const verification = spawnSync("python3", command, { encoding: "utf8", timeout: 60_000 });
  if (verification.status !== 0) throw new Error(`runtime verification failed: ${verification.stderr || verification.error?.message}`);
  const receipt = JSON.parse(verification.stdout);
  if (receipt.status !== "VERIFIED" || resolve(receipt.path) !== runtimeRoot) throw new Error("runtime verification did not identify the requested runtime");
  return { host: executable, runtime: runtimeRoot, manifestBytes,
    engine: { cef: manifest.cef_version, chromium: manifest.chromium_version, cef_rs_commit: manifest.cef_rs_commit, manifest_sha256: hash(manifestBytes), archive_sha256: candidate.sha256, host_sha256: hash(await readFile(executable)), target },
    verification: { command: ["python3", ...command], ...receipt, manifest_sha256: hash(manifestBytes) } };
}

const browserKey = frame => JSON.stringify([frame.document, frame.pool_generation, frame.buffer, frame.frame_sequence]);

export function inspectBrowserEvents(events, origin, seconds, refreshHz, scale) {
  const errors = [];
  const end = origin + seconds * 1e9;
  const inWindow = event => event.at_ns >= origin && event.at_ns <= end;
  const viewports = events.filter(event => event.event === "browser_viewport");
  const initial = viewports.filter(event => event.at_ns <= origin).at(-1);
  if (!initial || [initial, ...viewports.filter(inWindow)].some(event => event.width_px !== 1920 || event.height_px !== 1080 || (scale !== undefined && event.scale !== scale))) errors.push("Browser physical viewport or scale differs from the M1 condition");
  const frames = events.filter(event => event.event === "browser_intake");
  const paints = events.filter(event => event.event === "browser_paint");
  const presentations = events.filter(event => event.event === "browser_presented");
  const discards = events.filter(event => event.event === "browser_discarded");
  const intakes = new Map();
  const feedbacks = new Map();
  const completions = new Map();
  for (const frame of frames) {
    const key = browserKey(frame);
    if (intakes.has(key)) errors.push("duplicate Browser intake identity");
    if (!frame.document?.owner || typeof frame.document.browser !== "string" || !["pool_generation", "buffer", "frame_sequence", "callback_ns", "ready_ns", "intake_ns", "capture_timestamp_us"].every(field => integer(frame[field])) || frame.callback_ns > frame.ready_ns || frame.ready_ns > frame.intake_ns) errors.push("Browser intake has invalid identity or timestamps");
    intakes.set(key, frame);
  }
  for (const paint of paints) {
    const intake = intakes.get(browserKey(paint));
    if (!integer(paint.feedback_id) || feedbacks.has(paint.feedback_id) || !intake || paint.at_ns < intake.intake_ns) errors.push("Browser paint does not uniquely identify an accepted frame");
    if (intake && ["callback_ns", "ready_ns", "intake_ns", "capture_timestamp_us", "capture_counter"].some(field => intake[field] !== paint[field])) errors.push("Browser paint timing differs from its accepted frame");
    feedbacks.set(paint.feedback_id, paint);
  }
  for (const event of [...presentations, ...discards]) {
    const paint = feedbacks.get(event.feedback_id);
    if (completions.has(event.feedback_id) || !paint || browserKey(paint) !== browserKey(event)) errors.push("Browser feedback does not uniquely identify the painted frame");
    const intake = intakes.get(browserKey(event));
    if (intake && ["callback_ns", "ready_ns", "intake_ns", "capture_timestamp_us", "capture_counter"].some(field => intake[field] !== event[field])) errors.push("Browser feedback timing differs from its accepted frame");
    completions.set(event.feedback_id, event);
    if (event.event !== "browser_presented") continue;
    const calibration = event.calibration;
    if (!paint || !["present_ns", "native_present_ns", "presentation_callback_ns", "refresh_ns", "sequence", "clock_id"].every(field => integer(event[field])) || !calibration || !["source_ns", "mapped_ns", "max_error_ns"].every(field => integer(calibration[field])) || calibration.max_error_ns > 1_000_000 || event.present_ns !== event.native_present_ns + calibration.mapped_ns - calibration.source_ns || event.present_ns < paint.at_ns || event.present_ns > event.presentation_callback_ns + calibration.max_error_ns) errors.push("Browser presentation clock or causal boundary is invalid");
    if (refreshHz !== undefined && (!event.refresh_ns || Math.abs(1e9 / event.refresh_ns - refreshHz) > 0.1)) errors.push("Browser native refresh differs from the requested condition");
  }
  if (!presentations.some(event => event.present_ns <= origin)) errors.push("Browser has no native presentation before terminal replay");
  if (paints.filter(inWindow).some(event => !completions.has(event.feedback_id))) errors.push("Browser paint lacks terminal compositor feedback within the captured evidence");
  if (events.some(event => event.event === "browser_paint_unobserved" && inWindow(event))) errors.push("Browser paint has no presentation observer during replay");
  return { status: errors.length ? "REJECTED" : "OBSERVED_REQUIRES_ANALYSIS", errors: [...new Set(errors)], intake_frames: frames.length, painted_frames: paints.length, presented_frames: presentations.length, discarded_frames: discards.length, chromium_correlation: "NOT_EVALUATED", presentation_qualification: "NOT_EVALUATED" };
}

export function inspectBrowserLifecycle(events, origin, seconds, scale) {
  const end = origin + seconds * 1e9;
  const errors = events.filter(event => event.event === "fatal" || ["load_failed", "fixture_state_invalid", "trace_failed"].includes(event.native?.native)).map(event => JSON.stringify(event));
  const reports = events.filter(event => event.native?.native === "fixture_state").sort((left, right) => left.at_ns - right.at_ns);
  const initial = reports.filter(event => event.at_ns <= origin).at(-1);
  const observed = [initial, ...reports.filter(event => event.at_ns > origin && event.at_ns <= end)].filter(Boolean);
  if (!initial || initial.at_ns < origin - 1.5e9 || observed.at(-1)?.at_ns < end - 1.5e9 || observed.some((event, index) => index > 0 && event.at_ns - observed[index - 1].at_ns > 1.5e9)) errors.push("Browser fixture reports do not cover the complete replay");
  if (observed.some(event => {
    const state = event.native.state;
    return state?.state !== "ready" || state.visibility !== "visible" || state.scale !== scale || Math.round(state.width * state.scale) !== 1920 || Math.round(state.height * state.scale) !== 1080;
  })) errors.push("Browser fixture failed, became hidden or changed its physical geometry");
  if (!events.some(event => event.native?.native === "loaded" && event.at_ns <= origin)) errors.push("Browser page was not loaded before replay");
  return { errors: [...new Set(errors)], fixture_reports: observed.length, fixture_poll_max_gap_ns: 1.5e9 };
}

export function inspectVisibility(evidence, pid, origin, seconds) {
  if (evidence?.schema_version !== 1 || evidence.observation !== "native_compositor_surface_visibility" || evidence.clock !== "CLOCK_MONOTONIC" || evidence.application_pid !== pid || !Array.isArray(evidence.intervals) || !Array.isArray(evidence.artifacts) || evidence.artifacts.length === 0) throw new Error("combined capture requires native compositor visibility evidence for this application PID");
  let covered = origin;
  for (const interval of [...evidence.intervals].sort((left, right) => left.start_ns - right.start_ns)) {
    if (![interval.start_ns, interval.end_ns].every(integer) || interval.end_ns <= interval.start_ns) throw new Error("invalid compositor visibility interval");
    if (interval.end_ns <= covered) continue;
    if (interval.start_ns > covered) throw new Error("compositor visibility evidence has a gap");
    const a = interval.terminal;
    const b = interval.browser;
    if (![a, b].every(rect => rect?.visible === true && rect.occluded === false && Number.isSafeInteger(rect.x_px) && Number.isSafeInteger(rect.y_px) && rect.width_px === 1920 && rect.height_px === 1080)) throw new Error("both M1 viewports must remain visible at 1920x1080 physical pixels");
    if (!(a.x_px + a.width_px <= b.x_px || b.x_px + b.width_px <= a.x_px || a.y_px + a.height_px <= b.y_px || b.y_px + b.height_px <= a.y_px)) throw new Error("combined M1 viewports overlap");
    covered = interval.end_ns;
    if (covered >= origin + seconds * 1e9) return;
  }
  throw new Error("compositor visibility evidence does not cover the complete replay");
}

function shutdownObserved(events) {
  return events.some(event => event.native?.native === "trace_completed")
    && events.some(event => event.event === "closed" || event.native?.native === "closed")
    && events.some(event => event.event === "host_stopped");
}

async function hashFile(path) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest("hex");
}

export async function archiveBrowserShutdown(directory, dataRoot, events) {
  if (!shutdownObserved(events)) throw new Error("Browser shutdown lacks trace_completed, closed or host_stopped");
  if (events.some(event => event.native?.native === "trace_failed")) throw new Error("Browser trace failed during shutdown");
  const traces = events.filter(event => event.native?.native === "trace_completed");
  const configs = events.filter(event => event.event === "host_config");
  if (traces.length !== 1 || configs.length !== 1) throw new Error("combined shutdown does not identify one owned Browser profile");
  const trace = traces[0].native.path;
  const stderr = configs[0].host_stderr;
  const ownRoot = await realpath(dataRoot);
  const owned = async (path, name, limit) => {
    if (typeof path !== "string" || basename(path) !== name) throw new Error(`unexpected Browser evidence path for ${name}`);
    const canonical = await realpath(path);
    const local = relative(ownRoot, canonical);
    if (!sourcePath(local)) throw new Error(`Browser ${name} is outside the repetition's private data directory`);
    const info = await stat(canonical);
    if (!info.isFile() || info.size > limit || (name === "chromium-trace.json" && info.size === 0)) throw new Error(`invalid Browser evidence file: ${name}`);
    return { canonical, info };
  };
  const traceFile = await owned(trace, "chromium-trace.json", 512 * 1024 * 1024);
  const stderrFile = await owned(stderr, "host.stderr", 32 * 1024 * 1024);
  if (dirname(traceFile.canonical) !== dirname(stderrFile.canonical)) throw new Error("Browser trace and stderr identify different profiles");
  const artifacts = [];
  for (const [name, file] of [["chromium-trace.json", traceFile], ["host.stderr", stderrFile]]) {
    const destination = join(directory, name);
    await copyFile(file.canonical, destination, constants.COPYFILE_EXCL);
    artifacts.push({ path: name, sha256: await hashFile(destination), bytes: file.info.size, source: file.canonical });
  }
  return { status: "CLOSED_WITH_TRACE_ARCHIVED", qualification: "NOT_EVALUATED", artifacts,
    observed: events.filter(event => event.native?.native === "trace_completed" || event.native?.native === "closed" || event.event === "closed" || event.event === "host_stopped") };
}

async function stopBrowser(directory, paths) {
  const log = join(directory, "browser.jsonl");
  const response = await rpc(paths.socket, "qualification.browser.stop");
  if (response.queued !== true) throw new Error("M1 Browser close was not queued");
  await waitFor(async () => {
    try { return shutdownObserved(await jsonl(log)); }
    catch (error) { if (error instanceof SyntaxError || error.code === "ENOENT") return false; throw error; }
  }, 75_000, () => {});
  return archiveBrowserShutdown(directory, paths.data, await jsonl(log));
}

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

async function captureRepetition(binary, directory, bundle, seconds, interrupted, expectedRefreshHz, browser, displayCondition) {
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
  if (displayCondition.fullscreen) Object.assign(env, {
    PANEFLOW_M1_FULLSCREEN: "1", PANEFLOW_M1_TERMINAL_OUTPUT: displayCondition.terminal_output,
    ...(browser ? { PANEFLOW_M1_BROWSER_OUTPUT: displayCondition.browser_output } : {}),
  });
  if (browser) Object.assign(env, {
    PANEFLOW_M1_BROWSER_URL: browser.url, PANEFLOW_M1_BROWSER_LOG: join(directory, "browser.jsonl"),
    PANEFLOW_BROWSER_HOST: browser.host, PANEFLOW_CEF_ROOT: browser.runtime,
    PANEFLOW_BROWSER_FRAME_RATE: String(displayCondition.refresh_hz ?? 60),
    PANEFLOW_M1_BROWSER_X_PX: String(browser.x), PANEFLOW_M1_BROWSER_Y_PX: String(browser.y),
    ...(process.env.PANEFLOW_BROWSER_RENDER_NODE ? { PANEFLOW_BROWSER_RENDER_NODE: process.env.PANEFLOW_BROWSER_RENDER_NODE } : {}),
  });
  const app = spawn(binary, [], { cwd: paths.work, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
  let appFailure;
  let coordinator;
  const knownProcesses = [];
  let browserHostPid;
  const output = createWriteStream(join(directory, "app.stdout.log"), { flags: "wx", mode: 0o600 });
  const stderr = createWriteStream(join(directory, "app.stderr.log"), { flags: "wx", mode: 0o600 });
  app.stdout.pipe(output);
  app.stderr.pipe(stderr);
  app.once("error", error => { appFailure = error.message; });
  app.once("exit", (code, signal) => { appFailure = `application exited ${code ?? signal}`; });
  const assertRunning = () => { if (interrupted()) throw new Error("capture interrupted"); if (appFailure) throw new Error(appFailure); };
  const result = { status: "REJECTED", errors: [], native_log: "native.jsonl", clock: "CLOCK_MONOTONIC", application_pid: app.pid, duration_seconds: seconds, display_condition: displayCondition };
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
        if (!native.some(event => event.event === "presentation_ready")) return false;
        if (!browser) return true;
        const lifecycle = await jsonl(join(directory, "browser.jsonl"));
        if (lifecycle.some(event => event.event === "fatal")) throw new Error("Browser failed before terminal replay");
        return native.some(event => event.event === "browser_presented") && lifecycle.some(event => event.native?.native === "fixture_state" && event.native.state?.state === "ready");
      } catch (error) {
        if (error.code === "ENOENT" || error instanceof SyntaxError) return false;
        throw error;
      }
    }, 10_000, assertRunning);
    assertRunning();
    if (browser) {
      const lifecycle = await jsonl(join(directory, "browser.jsonl"));
      const hosts = lifecycle.filter(event => event.event === "host_ready");
      if (hosts.length !== 1 || !Number.isSafeInteger(hosts[0].pid)) throw new Error("combined capture has no unique observed Browser host PID");
      browserHostPid = hosts[0].pid;
      const evidence = await collectBrowserProcessEvidence(browserHostPid);
      await save(join(directory, "gpu-processes-start.json"), evidence);
      result.gpu_process_evidence = { path: "gpu-processes-start.json", sha256: hash(await readFile(join(directory, "gpu-processes-start.json"))) };
      const identityErrors = evidence.errors.filter(error => !["fd_inventory", "fd_type", "smaps_rollup"].includes(error.operation));
      const gpus = evidence.processes.filter(item => item.role === "gpu-process");
      if (identityErrors.length || evidence.omitted_errors || gpus.length !== 1 || !gpus[0].threads_complete) throw new Error("Browser GPU process inventory is incomplete or ambiguous");
    }
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
    if (browser) {
      await pause(250);
      await save(join(directory, "gpu-processes-end.json"), await collectBrowserProcessEvidence(browserHostPid));
      result.gpu_process_evidence_end = { path: "gpu-processes-end.json", sha256: hash(await readFile(join(directory, "gpu-processes-end.json"))) };
    }
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
    if (browser) {
      result.browser = inspectBrowserEvents(native, result.origin_ns, seconds, expectedRefreshHz, initialViewport?.scale);
      result.errors.push(...result.browser.errors);
      const lifecycle = await jsonl(join(directory, "browser.jsonl"));
      result.browser.lifecycle = inspectBrowserLifecycle(lifecycle, result.origin_ns, seconds, initialViewport?.scale);
      result.errors.push(...result.browser.lifecycle.errors);
      result.engine = browser.engine;
      result.fixture_sha256 = browser.fixture;
      result.scenario = browser.scenario;
      result.visibility = "NOT_VERIFIED";
      if (browser.visibilityEvidence) {
        const evidence = await json(browser.visibilityEvidence);
        inspectVisibility(evidence, app.pid, result.origin_ns, seconds);
        await save(join(directory, "visibility.json"), evidence);
        for (const artifact of evidence.artifacts) {
          if (!sourcePath(artifact.path) || !digest.test(artifact.sha256)) throw new Error("invalid compositor visibility artifact");
          const data = await readFile(resolve(dirname(browser.visibilityEvidence), artifact.path));
          if (hash(data) !== artifact.sha256) throw new Error("compositor visibility artifact digest mismatch");
          const destination = join(directory, "visibility-artifacts", artifact.path);
          await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
          await writeFile(destination, data, { flag: "wx", mode: 0o600 });
        }
        result.visibility = "NATIVE_EVIDENCE_ARCHIVED_REQUIRES_ANALYSIS";
      } else if (!browser.diagnostic && !displayCondition.fullscreen) result.errors.push("combined qualification capture lacks native visibility evidence");
    }
    if (displayCondition.fullscreen) {
      const appliedBytes = await readFile(join(displayCondition.evidence_directory, "applied.json"));
      const applied = JSON.parse(appliedBytes);
      const proof = inspectFullscreenEvidence({ events: native, condition: displayCondition, origin_ns: result.origin_ns, duration_ns: seconds * 1e9, applied });
      result.errors.push(...proof.errors);
      const artifact = { path: "display-applied.json", sha256: hash(appliedBytes) };
      await mkdir(join(directory, "visibility-artifacts"), { mode: 0o700 });
      await writeFile(join(directory, "visibility-artifacts", artifact.path), appliedBytes, { flag: "wx", mode: 0o600 });
      await save(join(directory, "visibility.json"), { observation: "native_fullscreen_output_binding", application_pid: app.pid, condition: displayCondition, proof, artifacts: [artifact] });
      result.visibility = proof.status;
    }
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
    if (browser) {
      try {
        result.browser_shutdown = await stopBrowser(directory, paths);
      } catch (error) {
        result.browser_shutdown = { status: "REJECTED", reason: error.message };
        result.errors.push(`Browser graceful shutdown failed: ${error.message}`);
        result.status = "REJECTED";
      }
      await save(join(directory, "browser-shutdown.json"), result.browser_shutdown);
    }
    await stopProcesses(owned);
    await save(join(directory, "capture.json"), result);
  }
  return result;
}

export async function captureTerminals({ binary, output, configuration = "A", diagnosticSeconds, sourceRoot = root, refreshHz, refreshActualHz, fullscreen = false, terminalOutput, browserOutput, displayEvidence, host, runtime, scenario = "combined", visibilityEvidence, browserX = 0, browserY = 0 }) {
  if (process.platform !== "linux" || !process.env.WAYLAND_DISPLAY) throw new Error("terminal native capture requires a Linux Wayland session");
  if (!["A", "C", "PREFEATURE"].includes(configuration)) throw new Error("terminal capture configuration must be A, C or PREFEATURE");
  if (!binary || !output) throw new Error("--binary and --output are required");
  if (diagnosticSeconds !== undefined && (!Number.isInteger(diagnosticSeconds) || diagnosticSeconds < 1 || diagnosticSeconds > 69)) throw new Error("diagnostic duration must be 1..69 seconds");
  if (![browserX, browserY].every(Number.isSafeInteger)) throw new Error("Browser placement requires integer physical pixel coordinates");
  if (configuration === "C" && diagnosticSeconds === undefined && !visibilityEvidence && !(fullscreen && displayEvidence)) throw new Error("qualification C requires --visibility-evidence; diagnostic capture can leave visibility unverified");
  const expectedRefreshHz = refreshHz ?? (diagnosticSeconds === undefined ? 60 : undefined);
  if (expectedRefreshHz !== undefined && ![60, 120].includes(expectedRefreshHz)) throw new Error("qualification refresh must be 60 or 120 Hz");
  if (refreshActualHz !== undefined && (!Number.isFinite(refreshActualHz) || expectedRefreshHz === undefined || Math.abs(refreshActualHz - expectedRefreshHz) > 0.5)) throw new Error("actual refresh must identify the selected nominal 60 or 120 Hz mode within 0.5 Hz");
  const outputName = value => typeof value === "string" && /^[A-Za-z0-9_.-]{1,64}$/.test(value);
  if (fullscreen && (!outputName(terminalOutput) || (configuration === "C" && (!outputName(browserOutput) || terminalOutput === browserOutput)))) throw new Error("fullscreen capture requires a terminal output and a distinct Browser output for C");
  if (!fullscreen && (terminalOutput !== undefined || browserOutput !== undefined)) throw new Error("explicit output selection requires fullscreen capture");
  if (fullscreen && !displayEvidence) throw new Error("fullscreen capture requires --display-evidence pointing to the active supervisor receipts");
  if (fullscreen && visibilityEvidence) throw new Error("select either native fullscreen evidence or external visibility evidence");
  const displayCondition = { fullscreen, evidence_directory: displayEvidence ? resolve(displayEvidence) : null, terminal_output: terminalOutput ?? null, browser_output: configuration === "C" ? browserOutput ?? null : null, refresh_hz: expectedRefreshHz ?? null, refresh_actual_hz: refreshActualHz ?? expectedRefreshHz ?? null };
  const directory = resolve(output);
  const executable = resolve(binary);
  const source = resolve(sourceRoot);
  const provenance = await sourceProvenance(source);
  const verified = configuration === "C" ? await verifyBrowserRuntime(source, host, runtime) : null;
  const fixture = await fixtureBundle();
  if (!fixture.manifest.scenarios.includes(scenario)) throw new Error("unknown local fixture scenario");
  await mkdir(directory, { mode: 0o700 });
  const bundle = await replayBundle(fullscreen ? "fullscreen" : "windowed");
  if (provenance.patch) await writeFile(join(directory, "source.patch"), provenance.patch, { flag: "wx", mode: 0o600 });
  if (provenance.snapshot) await writeFile(join(directory, "snapshot-provenance.json"), provenance.snapshot, { flag: "wx", mode: 0o600 });
  const sourceHashes = provenance.sourceHashes;
  await save(join(directory, "source-files.json"), sourceHashes);
  const runnerSources = [];
  for (const name of ["terminal-capture.mjs", "replay-worker.py", "replay.mjs", "fixtures.mjs", "process-evidence.mjs", "fullscreen-evidence.mjs"]) {
    const bytes = await readFile(new URL(name, import.meta.url));
    await writeFile(join(directory, name), bytes, { flag: "wx", mode: 0o600 });
    runnerSources.push({ path: name, sha256: hash(bytes) });
  }
  await save(join(directory, "runner-sources.json"), runnerSources);
  await mkdir(join(directory, "fixtures"), { mode: 0o700 });
  for (const [name, bytes] of fixture.assets) await writeFile(join(directory, "fixtures", name), bytes, { flag: "wx", mode: 0o600 });
  await save(join(directory, "fixture-manifest.json"), fixture.manifest);
  if (verified) {
    await writeFile(join(directory, "browser-manifest.toml"), verified.manifestBytes, { flag: "wx", mode: 0o600 });
    await save(join(directory, "runtime-verification.json"), verified.verification);
  }
  let interrupted = false;
  const interrupt = () => { interrupted = true; };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  const report = { schema_version: 1, configuration, purpose: diagnosticSeconds === undefined ? "qualification_capture" : "diagnostic_only", status: "REJECTED", qualification: "NOT_EVALUATED", build_profile: "release_required", binary: executable, binary_sha256: hash(await readFile(executable)), source_directory: source, source_binding: "build evidence must bind this binary to the archived source inventory", commit: provenance.commit, commit_provenance: provenance.commit_provenance, tracked_source_diff_sha256: provenance.patch ? hash(provenance.patch) : null, snapshot_provenance_sha256: provenance.snapshot ? hash(provenance.snapshot) : null, source_files_sha256: hash(JSON.stringify(sourceHashes)), workload_sha256: bundle.sha256, fixture_sha256: fixture.manifest.sha256, scenario, engine: verified?.engine ?? null, input_source: "synthetic_guarded_ipc_to_normal_gpui_key_handler", clock: "CLOCK_MONOTONIC", repetitions: [] };
  report.display_condition = displayCondition;
  let fixtures;
  try {
    if (verified) fixtures = await serveFixtures();
    const repetitions = diagnosticSeconds === undefined ? 5 : 1;
    const seconds = diagnosticSeconds ?? bundle.protocol.warmup_seconds + bundle.protocol.duration_seconds;
    for (let index = 0; index < repetitions && !interrupted; index++) {
      process.stderr.write(`M1 ${configuration}: repetition ${index + 1}/${repetitions}, ${seconds} seconds\n`);
      const browser = verified ? { ...verified, url: `${fixtures.url}/${scenario}`, scenario, fixture: fixtures.manifest.sha256, diagnostic: diagnosticSeconds !== undefined, visibilityEvidence, x: browserX, y: browserY } : null;
      const result = await captureRepetition(executable, join(directory, `r${index + 1}`), bundle, seconds, () => interrupted, refreshActualHz ?? expectedRefreshHz, browser, displayCondition);
      report.repetitions.push(result);
      if (result.status === "REJECTED") break;
    }
    if (report.repetitions.length === repetitions && report.repetitions.every(item => item.status === "CAPTURED_REQUIRES_ANALYSIS")) report.status = diagnosticSeconds === undefined ? "CAPTURED_REQUIRES_ANALYSIS" : "DIAGNOSTIC_ONLY";
  } finally {
    fixtures?.server.close();
    fixtures?.server.closeAllConnections();
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
    await save(join(directory, "capture.json"), report);
  }
  return report;
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ options: { binary: { type: "string" }, output: { type: "string" }, configuration: { type: "string", default: "A" }, "diagnostic-seconds": { type: "string" }, "source-root": { type: "string" }, "refresh-hz": { type: "string" }, "refresh-actual-hz": { type: "string" }, fullscreen: { type: "boolean", default: false }, "terminal-output": { type: "string" }, "browser-output": { type: "string" }, "display-evidence": { type: "string" }, host: { type: "string" }, runtime: { type: "string" }, scenario: { type: "string", default: "combined" }, "visibility-evidence": { type: "string" }, "browser-x": { type: "string" }, "browser-y": { type: "string" }, help: { type: "boolean" } } });
    if (values.help) process.stdout.write("bun scripts/browser-qualification/terminal-capture.mjs --binary <instrumented-release-paneflow> --output <new-directory> --configuration A|C|PREFEATURE [--host <cef-host> --runtime <verified-target-digest-directory> --scenario combined --visibility-evidence <native-compositor.json>] [--source-root <binary-source-checkout>] [--fullscreen --terminal-output DP-3 --browser-output DP-4 --display-evidence <supervisor-directory>] [--refresh-hz 120 --refresh-actual-hz 119.8787841796875] [--diagnostic-seconds 3]\n");
    else {
      const report = await captureTerminals({ binary: values.binary, output: values.output, configuration: values.configuration, host: values.host, runtime: values.runtime, scenario: values.scenario, visibilityEvidence: values["visibility-evidence"], browserX: values["browser-x"] === undefined ? 0 : Number(values["browser-x"]), browserY: values["browser-y"] === undefined ? 0 : Number(values["browser-y"]), sourceRoot: values["source-root"], fullscreen: values.fullscreen, terminalOutput: values["terminal-output"], browserOutput: values["browser-output"], displayEvidence: values["display-evidence"], refreshActualHz: values["refresh-actual-hz"] === undefined ? undefined : Number(values["refresh-actual-hz"]), refreshHz: values["refresh-hz"] === undefined ? undefined : Number(values["refresh-hz"]), diagnosticSeconds: values["diagnostic-seconds"] === undefined ? undefined : Number(values["diagnostic-seconds"]) });
      process.stdout.write(JSON.stringify(report, null, 2) + "\n");
      if (report.status === "REJECTED") process.exitCode = 1;
    }
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
