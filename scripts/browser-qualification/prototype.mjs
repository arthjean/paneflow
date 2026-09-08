import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, readlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { serveFixtures } from "./fixtures.mjs";
import { sandboxDisabled } from "./witness.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const pause = ms => new Promise(done => setTimeout(done, ms));

function argument(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index === -1 ? fallback : process.argv[index + 1];
}

async function readProcess(pid) {
  try {
    const text = await readFile(`/proc/${pid}/stat`, "utf8");
    const fields = text.slice(text.lastIndexOf(")") + 2).split(" ");
    const status = await readFile(`/proc/${pid}/status`, "utf8");
    const args = (await readFile(`/proc/${pid}/cmdline`, "utf8")).split("\0").filter(Boolean);
    const comm = (await readFile(`/proc/${pid}/comm`, "utf8")).trim();
    const environ = (await readFile(`/proc/${pid}/environ`, "utf8")).split("\0");
    const sockets = [];
    for (const fd of await readdir(`/proc/${pid}/fd`)) {
      try {
        const target = await readlink(`/proc/${pid}/fd/${fd}`);
        const match = target.match(/^socket:\[(\d+)\]$/);
        if (match) sockets.push(Number(match[1]));
      } catch {}
    }
    const type = args.find(arg => arg.startsWith("--type="))?.slice(7);
    const threads = [];
    const threadSecurity = [];
    let threadSecurityComplete = true;
    const tasks = await readdir(`/proc/${pid}/task`).catch(() => { threadSecurityComplete = false; return []; });
    for (const task of tasks) {
      try {
        const [name, security] = await Promise.all([
          readFile(`/proc/${pid}/task/${task}/comm`, "utf8"),
          readFile(`/proc/${pid}/task/${task}/status`, "utf8"),
        ]);
        threads.push(name.trim());
        threadSecurity.push({ tid: Number(task), name: name.trim(), seccomp: Number(security.match(/^Seccomp:\s+(\d+)/m)?.[1]), no_new_privs: Number(security.match(/^NoNewPrivs:\s+(\d+)/m)?.[1]), filter_count: Number(security.match(/^Seccomp_filters:\s+(\d+)/m)?.[1]) });
      } catch { threadSecurityComplete = false; }
    }
    const role = type === undefined ? "host" : threads.includes("VizCompositorTh") ? "gpu-process" : threads.includes("Compositor") ? "renderer" : type;
    return { pid, parent: Number(fields[1]), start_ticks: fields[19], comm, role, thread_security: threadSecurity, thread_security_complete: threadSecurityComplete && threadSecurity.length > 0, args_count: args.length, sandbox_disabled: args.some(sandboxDisabled), seccomp: Number(status.match(/^Seccomp:\s+(\d+)/m)?.[1]), no_new_privs: Number(status.match(/^NoNewPrivs:\s+(\d+)/m)?.[1]), user_namespace: await readlink(`/proc/${pid}/ns/user`), pid_namespace: await readlink(`/proc/${pid}/ns/pid`), display_env: environ.some(entry => entry.startsWith("DISPLAY=")), wayland_env: environ.some(entry => entry.startsWith("WAYLAND_DISPLAY=")), ozone: args.find(arg => arg.startsWith("--ozone-platform="))?.slice(17) ?? null, sockets };
  } catch (error) {
    if (["ENOENT", "ESRCH", "EACCES"].includes(error.code)) return null;
    throw error;
  }
}

async function processTree(rootPid) {
  const all = new Map();
  for (const name of await readdir("/proc")) {
    if (!/^\d+$/.test(name)) continue;
    try {
      const text = await readFile(`/proc/${name}/stat`, "utf8");
      const parent = Number(text.slice(text.lastIndexOf(")") + 2).split(" ")[1]);
      all.set(Number(name), parent);
    } catch {}
  }
  const members = [];
  const queue = [rootPid];
  const seen = new Set();
  while (queue.length) {
    const pid = queue.shift();
    if (seen.has(pid)) continue;
    seen.add(pid);
    const info = await readProcess(pid);
    if (info) members.push(info);
    for (const [child, parent] of all) if (parent === pid) queue.push(child);
  }
  return members;
}

function x11Connections(ssOutput, tree) {
  const rows = ssOutput.split("\n").slice(1).map(line => line.trim().split(/\s+/)).filter(row => row.length >= 7);
  const byInode = new Map();
  for (const row of rows) {
    const inode = Number(row[5]);
    const peer = Number(row[7]);
    const path = row[4];
    byInode.set(inode, { path, peer, pids: [...row.join(" ").matchAll(/pid=(\d+)/g)].map(match => Number(match[1])) });
  }
  const serverInodes = [...byInode.values()].filter(row => /\.X11-unix\//.test(row.path)).map(row => row.peer);
  const clientInodes = new Set();
  for (const [inode, row] of byInode) if (/\.X11-unix\//.test(row.path) && row.peer) clientInodes.add(row.peer);
  for (const inode of serverInodes) clientInodes.add(inode);
  const connected = [];
  for (const member of tree) {
    const hits = member.sockets.filter(inode => clientInodes.has(inode));
    if (hits.length) connected.push({ pid: member.pid, role: member.role, inodes: hits });
  }
  return { client_inodes: [...clientInodes], connected };
}

function summarize(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const pick = q => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return { count: sorted.length, min: sorted[0], p50: pick(0.5), p95: pick(0.95), max: sorted[sorted.length - 1] };
}

async function observeShutdown(snapshots) {
  const observed = [...new Map(snapshots.flatMap(snapshot => snapshot.tree).map(member => [`${member.pid}:${member.start_ticks}`, member])).values()];
  const started = Date.now();
  let remaining = [];
  do {
    remaining = (await Promise.all(observed.map(async member => {
      try {
        const text = await readFile(`/proc/${member.pid}/stat`, "utf8");
        const fields = text.slice(text.lastIndexOf(")") + 2).split(" ");
        return fields[19] === member.start_ticks ? { pid: member.pid, role: member.role, state: fields[0] } : null;
      } catch (error) {
        return ["ENOENT", "ESRCH"].includes(error.code) ? null : { pid: member.pid, role: member.role, error: error.code };
      }
    }))).filter(Boolean);
    if (!remaining.length || Date.now() - started >= 5000) break;
    await pause(100);
  } while (true);
  return { observed_count: observed.length, waited_ms: Date.now() - started, remaining, scope: "processes captured in runtime snapshots" };
}

export async function runPrototype(options) {
  if (process.platform !== "linux") throw new Error("the GPU presentation prototype has a Linux adapter only");
  const { binary, host, display, scenario, steps, output, hold, source } = options;
  const manifest = Bun.TOML.parse(await readFile(join(root, "native/browser/manifest.toml"), "utf8"));
  const target = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-unknown-linux-gnu`;
  const candidate = manifest.targets[target];
  const runtime = join(root, "native/browser/prebuilt", target, candidate.sha256);
  const verification = spawnSync("python3", [join(root, "scripts/fetch-browser.py"), "--target", target, "--verify-only"], { encoding: "utf8", timeout: 60_000 });
  if (verification.status !== 0) throw new Error(`runtime verification failed: ${verification.stderr}`);
  const fixtures = await serveFixtures();
  const directory = resolve(output);
  await mkdir(directory, { mode: 0o700 });
  const logPath = join(directory, "prototype.jsonl");
  const env = { ...process.env, PANEFLOW_CEF_ROOT: runtime, PANEFLOW_BROWSER_HOST: resolve(host), RUST_LOG: process.env.RUST_LOG ?? "warn" };
  if (display === "x11") delete env.WAYLAND_DISPLAY;
  const args = ["browser-prototype", "--source", source, "--log", logPath, "--hold", String(hold)];
  if (source === "cef") args.push("--url", `${fixtures.url}/${scenario}`);
  if (steps) args.push("--scenario", steps);
  if (display === "x11") args.push("--x11");
  const started = Date.now();
  const child = spawn(resolve(binary), args, { stdio: ["ignore", "pipe", "pipe"], env });
  let stderr = "";
  let stdout = "";
  child.stderr.on("data", bytes => { if (stderr.length < 8 * 1024 * 1024) stderr += bytes.toString(); });
  child.stdout.on("data", bytes => { if (stdout.length < 1024 * 1024) stdout += bytes.toString(); });
  let exit = null;
  child.on("exit", (code, signal) => { exit = { code, signal }; });
  const snapshots = [];
  const events = () => readFile(logPath, "utf8").then(text => text.split("\n").filter(Boolean).map(line => JSON.parse(line))).catch(() => []);
  let snapshotPids = new Set();
  const deadline = started + 300_000;
  while (exit === null && Date.now() < deadline) {
    await pause(500);
    const log = await events();
    for (const ready of log.filter(event => event.event === "host_ready")) {
      if (snapshotPids.has(ready.pid)) continue;
      const since = log.filter(event => event.event === "frame" && event.at_ns > ready.at_ns).length;
      if (since < 1) continue;
      snapshotPids.add(ready.pid);
      const tree = await processTree(ready.pid);
      const ss = spawnSync("ss", ["-xp"], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024, timeout: 10_000 });
      snapshots.push({ host_pid: ready.pid, taken_after_ms: Date.now() - started, tree, x11: x11Connections(ss.stdout ?? "", tree), ss_available: ss.status === 0, ss_status: ss.status, ss_error: ss.error?.code ?? null });
    }
  }
  if (exit === null) { child.kill("SIGKILL"); await pause(500); }
  fixtures.close();
  const shutdown = await observeShutdown(snapshots);
  const log = await events();
  const summary = log.find(event => event.event === "summary") ?? null;
  const window = log.find(event => event.event === "window") ?? null;
  const importer = log.find(event => event.event === "importer_ready") ?? null;
  const hostReady = log.filter(event => event.event === "host_ready");
  const completed = log.filter(event => event.event === "step_completed").map(event => event.step);
  const skipped = log.filter(event => event.event === "step_skipped").map(event => event.step);
  const requested = steps ? steps.split(",").filter(Boolean) : [];
  const fatal = log.find(event => event.event === "fatal") ?? null;
  const frames = log.filter(event => event.event === "frame");
  const copy = summarize(frames.map(frame => frame.copy_us));
  const transfer = summarize(frames.map(frame => frame.transfer_us));
  const sandboxed = (tree, role) => tree.some(process => process.role === role && process.seccomp === 2 && process.no_new_privs === 1 && (role === "gpu-process" || process.pid_namespace !== tree.find(member => member.role === "host")?.pid_namespace));
  const sandbox = snapshots.map(snapshot => ({ host_pid: snapshot.host_pid, renderer: sandboxed(snapshot.tree, "renderer"), gpu_process: sandboxed(snapshot.tree, "gpu-process"), switches_clean: !snapshot.tree.some(process => process.sandbox_disabled), roles: snapshot.tree.map(process => process.role) }));
  const x11 = snapshots.map(snapshot => ({ host_pid: snapshot.host_pid, connected: snapshot.x11.connected, ss_available: snapshot.ss_available, display_env: snapshot.tree.filter(process => process.display_env).map(process => process.pid), ozone: [...new Set(snapshot.tree.map(process => process.ozone).filter(Boolean))] }));
  const status = exit?.code === 0 && summary?.status === "COMPLETED" ? "COMPLETED" : "FAILED";
  const verdict = {
    page_presented: status === "COMPLETED" && (summary?.presented_frames ?? 0) > 0 ? "MEASURED" : "FAILED",
    x11_connection_absent: display === "wayland" ? (snapshots.length && x11.every(entry => entry.ss_available && entry.connected.length === 0 && entry.display_env.length === 0) ? "MEASURED" : snapshots.length ? "FAILED" : "NOT_EXECUTED") : "NOT_APPLICABLE",
    sandbox: snapshots.length ? (sandbox.every(entry => entry.renderer && entry.gpu_process && entry.switches_clean) ? "MEASURED" : "FAILED") : "NOT_EXECUTED",
    observed_processes_exited: shutdown.observed_count ? (shutdown.remaining.length === 0 ? "MEASURED" : "FAILED") : "NOT_EXECUTED",
    steps: Object.fromEntries(requested.map(step => [step, completed.includes(step) ? "MEASURED" : skipped.includes(step) ? "NOT_EXECUTED" : "FAILED"])),
    m1_presentation_feedback: "NOT_MEASURED",
    x11_session: display === "x11" ? (process.env.XDG_SESSION_TYPE === "x11" ? "NATIVE" : "XWAYLAND_CLIENT_ONLY") : "NOT_APPLICABLE",
  };
  const evidence = {
    schema_version: 1,
    status,
    recorded_at: new Date().toISOString(),
    paneflow_commit: spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).stdout.trim(),
    display,
    source,
    scenario,
    requested_steps: requested,
    session_type: process.env.XDG_SESSION_TYPE ?? "unknown",
    cef_version: manifest.cef_version,
    archive_sha256: candidate.sha256,
    fixture_sha256: fixtures.manifest.sha256,
    binary_sha256: createHash("sha256").update(await readFile(resolve(binary))).digest("hex"),
    host_sha256: createHash("sha256").update(await readFile(resolve(host))).digest("hex"),
    exit,
    fatal,
    window,
    importer,
    host_ready: hostReady.map(event => ({ pid: event.pid, presentation: event.presentation, initialized: event.initialized })),
    summary,
    frame_chain_us: { copy_in_host: copy, host_ready_to_app_intake: transfer, note: "callback_ns to ready_ns covers the host Vulkan blit and fence wait; ready_ns to intake_ns covers the SCM_RIGHTS transfer and app dispatch; presentation feedback is not measured here" },
    sandbox,
    x11,
    snapshots,
    shutdown,
    verdict,
  };
  await writeFile(join(directory, "native.json"), JSON.stringify(evidence, null, 2) + "\n");
  await writeFile(join(directory, "stderr.txt"), stderr);
  const hostStderr = (await events()).find(event => event.event === "host_config")?.host_stderr;
  if (hostStderr) await writeFile(join(directory, "host-stderr.txt"), await readFile(hostStderr, "utf8").catch(error => `unavailable: ${error.message}\n`));
  await writeFile(join(directory, "stdout.txt"), stdout);
  return evidence;
}

if (import.meta.main) {
  const options = {
    binary: argument("binary", join(root, "target/release/paneflow")),
    host: argument("host", join(root, "target/release/paneflow-browser-host")),
    display: argument("display", "wayland"),
    scenario: argument("scenario", "empty"),
    steps: argument("steps", "input,resize,scale,host-loss"),
    output: argument("output"),
    hold: Number(argument("hold", "5")),
    source: argument("source", "cef"),
  };
  if (!options.output) { console.error("usage: bun scripts/browser-qualification/prototype.mjs --output <new dir> [--display wayland|x11] [--scenario empty] [--steps input,resize,scale,host-loss] [--source cef|pattern]"); process.exit(2); }
  const evidence = await runPrototype(options);
  console.log(JSON.stringify({ status: evidence.status, verdict: evidence.verdict, output: resolve(options.output) }, null, 2));
  process.exit(evidence.status === "COMPLETED" ? 0 : 1);
}
