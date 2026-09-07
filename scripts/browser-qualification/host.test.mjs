import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { sandboxDisabled } from "./witness.mjs";

const binary = process.env.PANEFLOW_CEF_TEST_BINARY;
const runtime = process.env.PANEFLOW_CEF_ROOT;
const native = binary && runtime ? test : test.skip;

for (const [name, input, args] of [
  ["oversized handshake before body allocation", Buffer.from([0, 4, 0, 1]), []],
  ["truncated handshake", Buffer.from([0, 0, 0, 10, 123]), []],
  ["missing handshake", Buffer.alloc(0), []],
  ...["--no-sandbox", "--no-sandbox=1", "-no-sandbox", "--NO-SANDBOX", "--disable-gpu-sandbox=true", "-disable-setuid-sandbox=0"].map(argument => [`sandbox disabling flag ${argument}`, Buffer.alloc(0), [argument]]),
]) {
  native(`native host rejects ${name} before initialization`, async () => {
    const profile = await mkdtemp(join(tmpdir(), "paneflow-host-contract-"));
    try {
      const result = spawnSync(binary, args, { input, encoding: "utf8", timeout: 10_000, env: { ...process.env, PANEFLOW_CEF_PROFILE: profile, PANEFLOW_CEF_ORIGIN: "http://127.0.0.1:1", LD_LIBRARY_PATH: join(runtime, "Release") } });
      expect(result.status).toBe(1);
      expect(result.stdout).toBe("");
      expect(result.stderr).toContain(name.startsWith("sandbox disabling flag") ? "forbidden" : "handshake");
    } finally {
      await rm(profile, { recursive: true, force: true });
    }
  });
}

test("topology rejects the sandbox switch spellings accepted by CEF", () => {
  for (const argument of ["--no-sandbox", "--no-sandbox=1", "-no-sandbox", "--NO-SANDBOX", "--disable-gpu-sandbox=true", "-disable-setuid-sandbox=0"]) expect(sandboxDisabled(argument)).toBe(true);
  for (const argument of ["no-sandbox", "--enable-sandbox", "--title=no-sandbox"]) expect(sandboxDisabled(argument)).toBe(false);
});
