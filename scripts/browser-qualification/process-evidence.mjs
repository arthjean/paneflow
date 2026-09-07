import * as filesystem from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

const DEFAULT_LIMITS = Object.freeze({
  scanned_processes: 8192, processes: 256, threads_per_process: 1024,
  threads_total: 8192, fds_per_process: 4096, fds_total: 16384,
  bytes_total: 16 * 1024 * 1024, duration_ms: 15000, errors: 256,
});
const ROLE_TYPES = new Set(["gpu-process", "renderer", "utility", "zygote", "broker", "crashpad-handler"]);
const SANDBOX_FLAGS = ["--no-sandbox", "--disable-gpu-sandbox", "--disable-seccomp-filter-sandbox", "--disable-setuid-sandbox", "--single-process", "--in-process-gpu"];
const monotonic = () => Number(process.hrtime.bigint());
function nativeMonotonic() {
  const result = spawnSync("python3", ["-c", "import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC))"], { encoding: "utf8", timeout: 5000, maxBuffer: 128 });
  const text = result.stdout?.trim();
  const value = Number(text);
  if (result.status !== 0 || !/^[0-9]+$/.test(text ?? "") || !Number.isSafeInteger(value)) throw new Error("native CLOCK_MONOTONIC observation failed");
  return value;
}
const roleHint = name => ({ CrGpuMain: "gpu-process", VizCompositorTh: "gpu-process", CrRendererMain: "renderer", Compositor: "renderer" })[name] ?? null;

function parseStat(text, pid) {
  const close = text.lastIndexOf(")");
  const open = text.indexOf("(");
  const fields = text.slice(close + 2).trim().split(/\s+/);
  if (open < 1 || close < open || Number(text.slice(0, open).trim()) !== pid
    || fields.length < 20 || !/^\d+$/.test(fields[19]) || !/^\d+$/.test(fields[1])) {
    throw Object.assign(new Error(), { code: "INVALID_STAT" });
  }
  return { pid, parent: Number(fields[1]), start_ticks: fields[19], role_hint: roleHint(text.slice(open + 1, close)) };
}

function security(text) {
  const field = name => {
    const value = text.match(new RegExp(`^${name}:\\s+(\\d+)\\s*$`, "m"))?.[1];
    return value === undefined ? null : Number(value);
  };
  return { seccomp: field("Seccomp"), no_new_privs: field("NoNewPrivs"), seccomp_filters: field("Seccomp_filters") };
}

function errorCode(error) {
  const code = error?.code;
  return typeof code === "string" && /^[A-Z_]{1,40}$/.test(code) ? code : "READ_FAILED";
}

export async function collectBrowserProcessEvidence(hostPid, options = {}) {
  if (!Number.isSafeInteger(hostPid) || hostPid <= 0) throw new TypeError("hostPid must be a positive integer");
  const procRoot = options.procRoot ?? "/proc";
  const fs = options.filesystem ?? filesystem;
  const limits = { ...DEFAULT_LIMITS, ...options.limits };
  for (const [key, value] of Object.entries(limits)) {
    if (!(key in DEFAULT_LIMITS) || !Number.isSafeInteger(value) || value <= 0 || value > DEFAULT_LIMITS[key]) {
      throw new TypeError(`invalid process evidence limit: ${key}`);
    }
  }
  const started = monotonic();
  const result = { schema_version: 1, host_pid: hostPid, host_start_ticks: null,
    clock: "CLOCK_MONOTONIC", clock_observation: "native_syscall_bounds_around_collection", started_ns: nativeMonotonic(), ended_ns: null, complete: true,
    limits, scanned_processes: 0, bytes_read: 0, processes: [], errors: [], omitted_errors: 0 };
  let threadsTotal = 0;
  let fdsTotal = 0;
  const fail = (operation, error, pid = null, tid = null) => {
    result.complete = false;
    const item = { operation, code: errorCode(error), pid, tid, at_elapsed_ns: monotonic() - started };
    if (result.errors.length < limits.errors) result.errors.push(item);
    else result.omitted_errors++;
  };
  const checkTime = () => {
    if ((monotonic() - started) / 1e6 > limits.duration_ms) throw Object.assign(new Error(), { code: "TIME_LIMIT" });
  };
  async function read(path, maximum) {
    checkTime();
    const remaining = limits.bytes_total - result.bytes_read;
    if (remaining <= 0) throw Object.assign(new Error(), { code: "BYTE_LIMIT" });
    const cap = Math.min(maximum + 1, remaining);
    const handle = await fs.open(path, "r");
    try {
      const buffer = Buffer.alloc(cap);
      let length = 0;
      while (length < cap) {
        checkTime();
        const { bytesRead } = await handle.read(buffer, length, cap - length, null);
        result.bytes_read += bytesRead;
        if (!bytesRead) return buffer.subarray(0, length).toString("utf8");
        length += bytesRead;
      }
      throw Object.assign(new Error(), { code: cap <= maximum ? "BYTE_LIMIT" : "FILE_SIZE_LIMIT" });
    } finally { await handle.close(); }
  }
  async function numericEntries(path, maximum) {
    checkTime();
    const values = [];
    const directory = await fs.opendir(path);
    try {
      for await (const entry of directory) {
        checkTime();
        if (!/^(?:0|[1-9]\d*)$/.test(entry.name)) continue;
        if (values.length >= maximum) throw Object.assign(new Error(), { code: "ENTRY_LIMIT" });
        values.push(Number(entry.name));
      }
    } catch (error) {
      error.entries = values;
      throw error;
    }
    return values.sort((a, b) => a - b);
  }
  const identity = async (pid, tid = null) => parseStat(await read(join(procRoot, String(pid), ...(tid === null ? [] : ["task", String(tid)]), "stat"), 4096), tid ?? pid);
  const same = (left, right) => left.pid === right.pid && left.start_ticks === right.start_ticks && left.parent === right.parent;
  async function entries(path, maximum, operation, pid) {
    try { return await numericEntries(path, maximum); }
    catch (error) { fail(operation, error, pid); return error.entries ?? []; }
  }
  async function collect(member) {
    const { role_hint, ...base } = member;
    const item = { ...base, role: member.pid === hostPid ? "host" : "unknown", role_sources: [],
      seccomp: null, no_new_privs: null, seccomp_filters: null, sandbox_flags: null,
      threads: [], threads_complete: true, fds: { total: 0, inspected: 0, complete: true,
        types: { dma_buf: 0, socket: 0, anon_inode: 0, pipe: 0, file: 0, unknown: 0 } },
      memory: null, started_elapsed_ns: monotonic() - started, ended_elapsed_ns: null, complete: true };
    const errorsBefore = result.errors.length + result.omitted_errors;
    const hints = new Map();
    const hint = (role, source) => { if (role) hints.set(source, role); };
    hint(role_hint, "process_comm");
    try {
      const current = await identity(member.pid);
      if (!same(member, current)) throw Object.assign(new Error(), { code: "PID_REUSED_OR_REPARENTED" });
      const text = await read(join(procRoot, String(member.pid), "cmdline"), 65536);
      const args = text.split("\0");
      const type = args.find(arg => arg.startsWith("--type="))?.slice(7);
      hint(ROLE_TYPES.has(type) ? type : null, "cmdline_type");
      item.sandbox_flags = SANDBOX_FLAGS.filter(flag => args.some(arg => arg === flag || arg.startsWith(`${flag}=`)));
    } catch (error) { fail("process_cmdline_identity", error, member.pid); }
    try {
      Object.assign(item, security(await read(join(procRoot, String(member.pid), "status"), 32768)));
      if (item.seccomp === null || item.no_new_privs === null) throw Object.assign(new Error(), { code: "MISSING_SECURITY_FIELDS" });
    } catch (error) { fail("process_status", error, member.pid); }
    const taskErrors = result.errors.length + result.omitted_errors;
    const tids = await entries(join(procRoot, String(member.pid), "task"), limits.threads_per_process, "thread_inventory", member.pid);
    for (const tid of tids) {
      if (++threadsTotal > limits.threads_total) { fail("thread_inventory", { code: "THREAD_LIMIT" }, member.pid); break; }
      try {
        const before = await identity(member.pid, tid);
        const thread = { tid, start_ticks: before.start_ticks,
          role_hint: before.role_hint, ...security(await read(join(procRoot, String(member.pid), "task", String(tid), "status"), 32768)) };
        const after = await identity(member.pid, tid);
        if (!same(before, after)) throw Object.assign(new Error(), { code: "TID_REUSED_OR_REPARENTED" });
        if (thread.seccomp === null || thread.no_new_privs === null) throw Object.assign(new Error(), { code: "MISSING_SECURITY_FIELDS" });
        hint(thread.role_hint, `thread:${tid}`);
        item.threads.push(thread);
      } catch (error) { fail("thread_status_identity", error, member.pid, tid); }
    }
    item.threads_complete = tids.length > 0 && item.threads.length === tids.length
      && taskErrors === result.errors.length + result.omitted_errors;
    if (!tids.length) fail("thread_inventory", { code: "NO_THREADS_OBSERVED" }, member.pid);
    try {
      const afterTids = await numericEntries(join(procRoot, String(member.pid), "task"), limits.threads_per_process);
      if (tids.join() !== afterTids.join()) { item.threads_complete = false; fail("thread_inventory", { code: "THREAD_SET_CHANGED" }, member.pid); }
    } catch (error) { item.threads_complete = false; fail("thread_inventory", error, member.pid); }
    if (member.pid !== hostPid) {
      const observedRoles = [...new Set(hints.values())];
      const specializedRoles = observedRoles.filter(role => role !== "zygote");
      const roles = specializedRoles.length ? specializedRoles : observedRoles;
      if (roles.length === 1) item.role = roles[0];
      else if (roles.length > 1) fail("process_role", { code: "CONFLICTING_ROLE_EVIDENCE" }, member.pid);
    }
    item.role_sources = [...hints].map(([source, role]) => ({ source, role }));
    const fdErrors = result.errors.length + result.omitted_errors;
    const fds = await entries(join(procRoot, String(member.pid), "fd"), limits.fds_per_process, "fd_inventory", member.pid);
    item.fds.total = fds.length;
    for (const fd of fds) {
      if (++fdsTotal > limits.fds_total) { fail("fd_inventory", { code: "FD_LIMIT" }, member.pid); break; }
      try {
        checkTime();
        const target = await fs.readlink(join(procRoot, String(member.pid), "fd", String(fd)));
        if (target.length > 4096) throw Object.assign(new Error(), { code: "FD_TARGET_SIZE_LIMIT" });
        const type = /^(?:\/?dma[-_]?buf(?::|$)|anon_inode:\[?dma[-_]?buf(?:\]|$))/i.test(target) ? "dma_buf" : target.startsWith("socket:") ? "socket"
          : target.startsWith("anon_inode:") ? "anon_inode" : target.startsWith("pipe:") ? "pipe" : "file";
        item.fds.types[type]++;
        item.fds.inspected++;
      } catch (error) { item.fds.types.unknown++; fail("fd_type", error, member.pid); }
    }
    item.fds.complete = fdErrors === result.errors.length + result.omitted_errors && item.fds.inspected === fds.length;
    try {
      const text = await read(join(procRoot, String(member.pid), "smaps_rollup"), 65536);
      const kib = name => { const raw = text.match(new RegExp(`^${name}:\\s+(\\d+) kB$`, "m"))?.[1]; return raw === undefined ? null : Number(raw); };
      item.memory = { pss_kb: kib("Pss"), private_clean_kb: kib("Private_Clean"), private_dirty_kb: kib("Private_Dirty"), private_hugetlb_kb: kib("Private_Hugetlb") };
      if (Object.values(item.memory).some(value => value === null)) throw Object.assign(new Error(), { code: "MISSING_MEMORY_FIELDS" });
      item.memory.private_kb = item.memory.private_clean_kb + item.memory.private_dirty_kb + item.memory.private_hugetlb_kb;
    } catch (error) { item.memory = null; fail("smaps_rollup", error, member.pid); }
    item.ended_elapsed_ns = monotonic() - started;
    item.complete = errorsBefore === result.errors.length + result.omitted_errors;
    return item;
  }
  try {
    const host = await identity(hostPid);
    result.host_start_ticks = host.start_ticks;
    const all = new Map([[hostPid, host]]);
    const pids = await entries(procRoot, limits.scanned_processes, "process_inventory", null);
    for (const pid of pids) {
      result.scanned_processes++;
      if (pid === hostPid) continue;
      try { all.set(pid, await identity(pid)); }
      catch (error) { if (errorCode(error) !== "ENOENT" && errorCode(error) !== "ESRCH") fail("process_stat_inventory", error, pid); }
    }
    const selected = new Map([[hostPid, host]]);
    for (const parent of selected.values()) {
      for (const child of all.values()) {
        if (selected.has(child.pid) || child.parent !== parent.pid) continue;
        if (BigInt(child.start_ticks) < BigInt(parent.start_ticks)) { fail("descendant_identity", { code: "CHILD_OLDER_THAN_PARENT" }, child.pid); continue; }
        if (selected.size >= limits.processes) { fail("descendant_inventory", { code: "PROCESS_LIMIT" }, child.pid); continue; }
        selected.set(child.pid, child);
      }
    }
    const collected = [];
    for (const member of selected.values()) collected.push(await collect(member));
    const valid = new Set();
    for (const item of collected) {
      try {
        const after = await identity(item.pid);
        if (!same(item, after)) throw Object.assign(new Error(), { code: "PID_REUSED_OR_REPARENTED" });
        if (item.pid !== hostPid && !valid.has(item.parent)) throw Object.assign(new Error(), { code: "ANCESTOR_IDENTITY_REJECTED" });
        valid.add(item.pid);
        result.processes.push(item);
      } catch (error) { fail("process_final_identity", error, item.pid); }
    }
  } catch (error) { fail("host_collection", error, hostPid); }
  result.ended_ns = nativeMonotonic();
  return result;
}
