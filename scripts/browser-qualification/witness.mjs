import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { chmod, copyFile, link, mkdir, readFile, readdir, readlink, stat, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { serveFixtures } from "./fixtures.mjs";
import { collectBrowserProcessEvidence } from "./process-evidence.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const pause = ms => new Promise(done => setTimeout(done, ms));

export function sandboxDisabled(argument) {
  return /^--?(?:no-sandbox|disable-gpu-sandbox|disable-setuid-sandbox)(?:=|$)/i.test(argument);
}

function packet(command, operation) {
  const body = Buffer.from(JSON.stringify({ version: 3, operation, command }));
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length);
  return Buffer.concat([header, body]);
}

async function topology(hostPid) {
  const processes = [];
  for (const name of await readdir("/proc")) {
    if (!/^\d+$/.test(name)) continue;
    try {
      const text = await readFile(`/proc/${name}/stat`, "utf8");
      const fields = text.slice(text.lastIndexOf(")") + 2).split(" ");
      if (Number(fields[2]) !== hostPid) continue;
      const status = await readFile(`/proc/${name}/status`, "utf8");
      const args = (await readFile(`/proc/${name}/cmdline`, "utf8")).split("\0");
      const comm = (await readFile(`/proc/${name}/comm`, "utf8")).trim();
      const role = ({ CrRendererMain: "renderer", CrGpuMain: "gpu-process" })[comm] ?? args.find(arg => arg.startsWith("--type="))?.slice(7) ?? "host";
      processes.push({ pid: Number(name), parent: Number(fields[1]), start_ticks: Number(fields[19]), role, comm, seccomp: Number(status.match(/^Seccomp:\s+(\d+)/m)?.[1]), no_new_privs: Number(status.match(/^NoNewPrivs:\s+(\d+)/m)?.[1]), user_namespace: await readlink(`/proc/${name}/ns/user`), pid_namespace: await readlink(`/proc/${name}/ns/pid`), sandbox_disabled: args.some(sandboxDisabled) });
    } catch (error) {
      if (!["ENOENT", "ESRCH", "EACCES"].includes(error.code)) throw error;
    }
  }
  return processes;
}

async function stage(directory, runtime, binary) {
  const linkOrCopy = async (from, to) => {
    try {
      await link(from, to);
    } catch (error) {
      if (error.code !== "EXDEV") throw error;
      await copyFile(from, to);
      const mode = (await stat(from)).mode & 0o7777;
      await chmod(to, mode);
    }
  };
  await mkdir(directory, { mode: 0o700 });
  for (const source of [join(runtime, "Release"), join(runtime, "Resources")]) {
    for (const name of await readdir(source)) {
      const from = join(source, name);
      const to = join(directory, name);
      if ((await stat(from)).isDirectory()) {
        await mkdir(to);
        for (const child of await readdir(from)) await linkOrCopy(join(from, child), join(to, child));
      } else {
        await linkOrCopy(from, to);
      }
    }
  }
  await copyFile(binary, join(directory, "paneflow-browser-host"));
}

export async function runWitness(binary, output, scenario = "empty", duration = 2, options = {}) {
  const sourceRoot = resolve(options.sourceRoot ?? root);
  const display = options.display ?? "wayland";
  if (!["wayland", "x11"].includes(display)) throw new Error("invalid witness display backend");
  if (process.platform !== "linux") throw new Error("native CEF witness unavailable on this platform");
  if (!binary || !output || !Number.isInteger(duration) || duration < 1 || duration > 70) throw new Error("witness requires binary, new output directory, fixture and duration (1..70 s)");
  const manifest = Bun.TOML.parse(await readFile(join(sourceRoot, "native/browser/manifest.toml"), "utf8"));
  const target = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-unknown-linux-gnu`;
  const candidate = manifest.targets[target];
  if (!candidate?.sha256) throw new Error(`no runtime candidate for ${target}`);
  const runtime = join(sourceRoot, "native/browser/prebuilt", target, candidate.sha256);
  const verification = spawnSync("python3", [join(root, "scripts/fetch-browser.py"), "--target", target, "--manifest", join(sourceRoot, "native/browser/manifest.toml"), "--destination", join(sourceRoot, "native/browser/prebuilt"), "--verify-only"], { encoding: "utf8", timeout: 30_000 });
  if (verification.status !== 0) throw new Error(`runtime verification failed: ${verification.stderr}`);
  const fixtures = await serveFixtures();
  if (!fixtures.manifest.scenarios.includes(scenario)) { fixtures.close(); throw new Error("unknown fixture"); }
  const directory = resolve(output);
  try { await mkdir(directory, { mode: 0o700 }); }
  catch (error) { fixtures.close(); throw error; }
  let child;
  const events = [];
  let pending = Buffer.alloc(0);
  let stderr = "";
  let failure;
  let closed = false;
  let processTree = [];
  let processEvidenceStart;
  let processEvidenceEnd;
  let observationOrigin;
  const started = process.hrtime.bigint();
  const owner = { workspace: "witness", session: "qualification" };
  let document;
  const interrupted = () => { failure = "witness interrupted"; };
  process.once("SIGINT", interrupted);
  process.once("SIGTERM", interrupted);
  try {
    const profile = join(directory, "profile");
    await mkdir(profile, { mode: 0o700 });
    await stage(join(directory, "bin"), runtime, resolve(binary));
    const env = { ...process.env, GDK_BACKEND: display, PANEFLOW_BROWSER_OZONE: display, PANEFLOW_CEF_ROOT: runtime, PANEFLOW_CEF_PROFILE: profile, PANEFLOW_CEF_ORIGIN: fixtures.url, LD_LIBRARY_PATH: join(directory, "bin") };
    if (options.fullscreen === true) env.PANEFLOW_M1_FULLSCREEN = "1";
    else delete env.PANEFLOW_M1_FULLSCREEN;
    if (display === "wayland") {
      env.WAYLAND_DEBUG = "client";
      delete env.DISPLAY;
    } else {
      delete env.WAYLAND_DISPLAY;
      delete env.WAYLAND_DEBUG;
    }
    child = spawn(join(directory, "bin/paneflow-browser-host"), [], { detached: true, stdio: ["pipe", "pipe", "pipe"], env });
    child.on("error", error => { failure = error.message; });
    child.on("exit", (code, signal) => { closed = true; if (code !== 0) failure = `host exited ${code ?? signal}`; });
    child.stdin.on("error", error => { if (!closed) failure = error.message; });
    child.stderr.on("data", bytes => {
      if (stderr.length + bytes.length > 32 * 1024 * 1024) { failure = "native Wayland evidence exceeded 32 MiB"; return; }
      stderr += bytes.toString();
    });
    child.stdout.on("data", bytes => {
      if (pending.length + bytes.length > 256 * 1024 + 4) { failure = "host receive buffer exceeded"; child.stdout.destroy(); return; }
      pending = Buffer.concat([pending, bytes]);
      while (pending.length >= 4) {
        const length = pending.readUInt32BE();
        if (length > 256 * 1024) { failure = "oversized host event"; return; }
        if (pending.length < length + 4) break;
        try {
          const event = JSON.parse(pending.subarray(4, length + 4));
          if (events.length >= 1024) { failure = "host event budget exhausted"; return; }
          events.push({ elapsed_ns: Number(process.hrtime.bigint() - started), ...event });
          if (["fixture_state_invalid", "load_failed", "trace_failed"].includes(event.native)) failure = `native: ${event.native}`;
          if (event.protocol?.result?.Err) failure = `protocol: ${event.protocol.result.Err}`;
          if (event.protocol?.result?.Ok?.session?.document) document = event.protocol.result.Ok.session.document;
        } catch { failure = "invalid host event"; }
        pending = pending.subarray(length + 4);
      }
    });
    const wait = async (predicate, timeout = 20_000) => {
      const deadline = Date.now() + timeout;
      while (!predicate()) {
        if (failure || closed || Date.now() > deadline) throw new Error(failure ?? "host handshake/lifecycle timeout");
        await pause(20);
      }
    };
    child.stdin.write(packet({ type: "capabilities" }, "hello"));
    await wait(() => events.some(event => event.native === "initialized"));
    await wait(() => events.some(event => event.native === "trace_started"));
    child.stdin.write(packet({ type: "create", owner, browser: "fixture", profile: "isolated", url: `${fixtures.url}/${scenario}`, title: "Qualification fixture" }, "create"));
    await wait(() => Boolean(document));
    child.stdin.write(packet({ type: "start", document }, "start"));
    await wait(() => events.some(event => event.native === "created"));
    await wait(() => events.some(event => event.native === "loaded"));
    await wait(() => events.some(event => event.native === "fixture_state"));
    processEvidenceStart = await collectBrowserProcessEvidence(child.pid);
    await wait(() => events.some(event => event.native === "fixture_state" && event.trace_us * 1000 >= processEvidenceStart.ended_ns));
    observationOrigin = events.find(event => event.native === "fixture_state" && event.trace_us * 1000 >= processEvidenceStart.ended_ns).trace_us * 1000;
    const observationEnd = Date.now() + (duration + 1) * 1000;
    while (Date.now() < observationEnd && !failure && !closed) await pause(20);
    if (failure || closed) throw new Error(failure ?? "host exited during observation");
    processEvidenceEnd = await collectBrowserProcessEvidence(child.pid);
    processTree = await topology(child.pid);
    child.stdin.write(packet({ type: "close", document }, "close"));
    await wait(() => events.some(event => event.native === "trace_completed"), 60_000);
    await wait(() => events.some(event => event.native === "shutdown"));
    await wait(() => closed);
    if (failure) throw new Error(failure);
    const states = events.filter(event => event.native === "fixture_state");
    if (!states.length || states.some(event => event.state.state !== "ready" || event.state.width !== 1920 || event.state.height !== 1080 || event.state.scale !== 1 || event.state.visibility !== "visible")) throw new Error("fixture state or physical viewport is invalid");
    if (!events.some(event => event.native === "closed")) throw new Error("CEF OnBeforeClose was not observed");
    const trace = JSON.parse(await readFile(join(profile, "chromium-trace.json"), "utf8"));
    for (const event of trace.traceEvents ?? []) {
      if (event.name !== "process_name" || event.ph !== "M") continue;
      const process = processTree.find(process => process.pid === event.pid);
      if (!process) continue;
      process.trace_name = event.args?.name;
      process.role = ({ Renderer: "renderer", "GPU Process": "gpu-process" })[event.args?.name] ?? process.role;
      process.role_evidence = "Chromium trace process_name metadata joined to live /proc PID";
    }
    const host = processTree.find(process => process.pid === child.pid);
    const sandboxed = role => processTree.some(process => process.role === role && process.seccomp === 2 && process.no_new_privs === 1 && (role === "gpu-process" || process.pid_namespace !== host?.pid_namespace));
    const sandboxFailure = !sandboxed("renderer") || !sandboxed("gpu-process") || processTree.some(process => process.sandbox_disabled);
    if (sandboxFailure) throw new Error("sandbox not proven for both renderer and GPU (seccomp, no_new_privs, renderer namespace)");
  } catch (error) {
    failure = error instanceof Error ? error.message : String(error);
  } finally {
    if (child?.pid) {
      if (!closed && document && !child.stdin.destroyed) {
        child.stdin.write(packet({ type: "close", document }, "cleanup"));
        const deadline = Date.now() + 5000;
        while (!closed && Date.now() < deadline) await pause(20);
      }
      try { process.kill(-child.pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") failure ??= error.message; }
      const deadline = Date.now() + 4000;
      while ((await topology(child.pid)).length && Date.now() < deadline) await pause(50);
      const remaining = await topology(child.pid);
      if (remaining.length) {
        try { process.kill(-child.pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") failure ??= error.message; }
        failure ??= "witness descendants exceeded graceful shutdown deadline";
      }
    }
    fixtures.close();
    process.removeListener("SIGINT", interrupted);
    process.removeListener("SIGTERM", interrupted);
  }
  const evidence = { schema_version: 1, host_pid: child?.pid, status: failure ? "REJECTED" : "NATIVE_LIFECYCLE_OBSERVED", error: failure ?? null, target, cef_version: manifest.cef_version, archive_sha256: candidate.sha256, fixture_sha256: fixtures.manifest.sha256, scenario, duration_seconds: duration, binary_sha256: createHash("sha256").update(await readFile(resolve(binary))).digest("hex"), display_backend: display, source_root: sourceRoot, session_type: process.env.XDG_SESSION_TYPE ?? "unknown", presentation_measurement: "NOT_EXECUTED", observation_origin_ns: observationOrigin, process_evidence_start: processEvidenceStart, process_evidence_end: processEvidenceEnd, processes: processTree, events };
  await writeFile(join(directory, "native.json"), JSON.stringify(evidence, null, 2) + "\n");
  await writeFile(join(directory, "stderr.txt"), stderr);
  if (failure) throw new Error(`${failure}; evidence: ${join(directory, "native.json")}`);
  return evidence;
}
