import { expect, test } from "bun:test";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectBrowserProcessEvidence } from "./process-evidence.mjs";

const stat = (pid, parent, ticks, comm = "test") => `${pid} (${comm}) ${["S", parent, ...Array(17).fill(0), ticks].join(" ")}\n`;
const status = (seccomp = 2) => `Name:\tprivate-name\nSeccomp:\t${seccomp}\nNoNewPrivs:\t1\nSeccomp_filters:\t2\n`;
async function member(root, pid, parent, ticks, { args = [], threads = [[pid, "test"]], comm = "test" } = {}) {
  const base = join(root, String(pid));
  await fs.mkdir(join(base, "fd"), { recursive: true });
  await fs.writeFile(join(base, "stat"), stat(pid, parent, ticks, comm));
  await fs.writeFile(join(base, "cmdline"), ["private-binary", ...args].join("\0"));
  await fs.writeFile(join(base, "status"), status());
  await fs.writeFile(join(base, "smaps_rollup"), "Pss: 120 kB\nPrivate_Clean: 10 kB\nPrivate_Dirty: 20 kB\nPrivate_Hugetlb: 0 kB\n");
  for (const [tid, comm] of threads) {
    await fs.mkdir(join(base, "task", String(tid)), { recursive: true });
    await fs.writeFile(join(base, "task", String(tid), "stat"), stat(tid, parent, ticks, comm));
    await fs.writeFile(join(base, "task", String(tid), "status"), status());
  }
}
async function fixture(run) {
  const root = await fs.mkdtemp(join(tmpdir(), "pf-process-evidence-"));
  try { await run(root); } finally { await fs.rm(root, { recursive: true, force: true }); }
}

test("collects only descendants, infers sandbox-masked GPU role from threads, and excludes sensitive text", async () => fixture(async root => {
  await member(root, 100, 1, "50");
  await member(root, 200, 100, "60", { args: ["--user-data-dir=/private/token", "https://secret.example/key"], threads: [[200, "test"], [201, "CrGpuMain"]] });
  await member(root, 300, 200, "70", { args: ["--type=renderer", "--disable-gpu-sandbox"] });
  await member(root, 400, 1, "80", { args: ["--type=gpu-process"] });
  for (const [fd, target] of [[0, "socket:[123]"], [1, "/dmabuf:123"], [2, "anon_inode:[eventfd]"], [3, "pipe:[1]"], [4, "/private/dmabuf-secret"]]) {
    await fs.symlink(target, join(root, "200/fd", String(fd)));
  }
  const result = await collectBrowserProcessEvidence(100, { procRoot: root });
  expect(result.complete).toBe(true);
  expect(result.processes.map(item => item.role)).toEqual(["host", "gpu-process", "renderer"]);
  expect(result.processes[1].fds).toEqual({ total: 5, inspected: 5, complete: true,
    types: { dma_buf: 1, socket: 1, anon_inode: 1, pipe: 1, file: 1, unknown: 0 } });
  expect(result.processes[1].threads_complete).toBe(true);
  expect(result.processes[1].threads.map(item => item.seccomp)).toEqual([2, 2]);
  expect(result.processes[1].memory.private_kb).toBe(30);
  expect(result.processes[2].sandbox_flags).toEqual(["--disable-gpu-sandbox"]);
  expect(result.processes[1].start_ticks).toBe("60");
  expect(result.ended_ns).toBeGreaterThanOrEqual(result.started_ns);
  expect(JSON.stringify(result)).not.toMatch(/private-binary|private-name|secret\.example|private\/token|dmabuf-secret/);
}));

test("rejects a reused parent PID and all collected descendants", async () => fixture(async root => {
  await member(root, 100, 1, "50");
  await member(root, 200, 100, "60", { threads: [[200, "CrGpuMain"]] });
  let hostReads = 0;
  const wrapped = { ...fs, open: async (...args) => {
    if (args[0] === join(root, "100/stat") && ++hostReads === 3) await fs.writeFile(args[0], stat(100, 1, "999"));
    return fs.open(...args);
  } };
  const result = await collectBrowserProcessEvidence(100, { procRoot: root, filesystem: wrapped });
  expect(result.processes).toEqual([]);
  expect(result.complete).toBe(false);
  expect(result.errors.map(error => error.code)).toContain("PID_REUSED_OR_REPARENTED");
  expect(result.errors.map(error => error.code)).toContain("ANCESTOR_IDENTITY_REJECTED");
}));

test("permission-denied memory and thread reads stay explicit and never imply complete sandbox evidence", async () => fixture(async root => {
  await member(root, 100, 1, "50");
  const wrapped = { ...fs, open: async (...args) => {
    if (args[0].endsWith("smaps_rollup") || args[0].endsWith("task/100/status")) {
      throw Object.assign(new Error("secret path must not escape"), { code: "EACCES" });
    }
    return fs.open(...args);
  } };
  const result = await collectBrowserProcessEvidence(100, { procRoot: root, filesystem: wrapped });
  expect(result.complete).toBe(false);
  expect(result.processes[0].threads_complete).toBe(false);
  expect(result.processes[0].memory).toBeNull();
  expect(result.errors.map(error => error.operation)).toContain("smaps_rollup");
  expect(result.errors.map(error => error.operation)).toContain("thread_status_identity");
  expect(JSON.stringify(result)).not.toContain("secret path");
}));

test("thread and process bounds produce partial evidence instead of silently passing", async () => fixture(async root => {
  await member(root, 100, 1, "50", { threads: [[100, "test"], [101, "CrGpuMain"]] });
  await member(root, 200, 100, "60");
  const result = await collectBrowserProcessEvidence(100, { procRoot: root, limits: { processes: 1, threads_per_process: 1 } });
  expect(result.complete).toBe(false);
  expect(result.processes).toHaveLength(1);
  expect(result.processes[0].threads_complete).toBe(false);
  expect(result.errors.map(error => error.code)).toContain("ENTRY_LIMIT");
  expect(result.errors.map(error => error.code)).toContain("PROCESS_LIMIT");
}));

test("oversized cmdline is rejected without storing its content", async () => fixture(async root => {
  await member(root, 100, 1, "50", { args: ["secret".repeat(12000)] });
  const result = await collectBrowserProcessEvidence(100, { procRoot: root });
  expect(result.complete).toBe(false);
  expect(result.processes[0].sandbox_flags).toBeNull();
  expect(result.errors.map(error => error.code)).toContain("FILE_SIZE_LIMIT");
  expect(JSON.stringify(result)).not.toContain("secret");
}));

test("conflicting Chromium roles remain unknown", async () => fixture(async root => {
  await member(root, 100, 1, "50");
  await member(root, 200, 100, "60", { args: ["--type=renderer"], threads: [[200, "CrGpuMain"]] });
  const result = await collectBrowserProcessEvidence(100, { procRoot: root });
  expect(result.processes[1].role).toBe("unknown");
  expect(result.errors.map(error => error.code)).toContain("CONFLICTING_ROLE_EVIDENCE");
}));


test("specialized GPU and renderer threads override inherited zygote cmdline after sandbox fork", async () => fixture(async root => {
  await member(root, 100, 1, "50");
  await member(root, 200, 100, "60", { args: ["--type=zygote"], comm: "paneflow-browse" });
  await member(root, 300, 200, "70", { args: ["--type=zygote"], comm: "paneflow-browse",
    threads: [[300, "paneflow-browse"], [301, "VizCompositorTh"]] });
  await member(root, 400, 200, "80", { args: ["--type=zygote"], comm: "paneflow-browse",
    threads: [[400, "paneflow-browse"], [401, "Compositor"]] });
  const result = await collectBrowserProcessEvidence(100, { procRoot: root });
  expect(result.complete).toBe(true);
  expect(result.processes.map(item => [item.pid, item.role])).toEqual([[100, "host"], [200, "zygote"], [300, "gpu-process"], [400, "renderer"]]);
  expect(result.processes[2].role_sources).toContainEqual({ source: "cmdline_type", role: "zygote" });
  expect(result.processes[2].role_sources).toContainEqual({ source: "thread:301", role: "gpu-process" });
  expect(result.processes[3].role_sources).toContainEqual({ source: "thread:401", role: "renderer" });
}));
