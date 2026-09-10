import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../fetch-browser.py", import.meta.url));

for (const attack of ["traversal", "absolute", "symlink", "hardlink", "fifo", "duplicate", "oversize", "hash", "sha1", "glibc"]) {
  test(`fetch CLI rejects ${attack} without touching an existing runtime`, async () => {
    const directory = await mkdtemp(join(tmpdir(), "paneflow-fetch-test-"));
    try {
      const archive = join(directory, "candidate.tar.bz2");
      const fixture = `import tarfile,io,sys
attack=sys.argv[2]
with tarfile.open(sys.argv[1], 'w:bz2') as t:
 e=tarfile.TarInfo({'traversal':'candidate/../../outside','absolute':'/outside'}.get(attack,'candidate/Release/libcef.so'))
 data=b'\\x7fELFmalformed' if attack=='glibc' else b'x'
 e.size=len(data)
 if attack in ('symlink','hardlink','fifo'):
  e.type={'symlink':tarfile.SYMTYPE,'hardlink':tarfile.LNKTYPE,'fifo':tarfile.FIFOTYPE}[attack];e.linkname='/outside';e.size=0
 t.addfile(e,io.BytesIO(data))
 if attack=='duplicate': t.addfile(e,io.BytesIO(data))
`;
      const setup = spawnSync("python3", ["-c", fixture, archive, attack], { encoding: "utf8" });
      expect(setup.status).toBe(0);
      const bytes = await readFile(archive);
      const manifest = join(directory, "manifest.toml");
      const sha = createHash("sha256").update(bytes).digest("hex");
      const archiveSha1 = attack === "sha1" ? `archive_sha1 = "${"0".repeat(40)}"\n` : "";
      const contentSha = createHash("sha256").update(attack === "glibc" ? Buffer.from("\x7fELFmalformed") : "x").digest("hex");
      await writeFile(manifest, `contract_version = 3
maximum_glibc = "2.35"
[targets.test]
availability = "development"
  archive = "candidate.tar.bz2"
${archiveSha1}sha256 = "${attack === "hash" ? "0".repeat(64) : sha}"
size = ${bytes.length}
unpacked_size = ${attack === "oversize" ? 0 : 100}
max_file_size = 100
[targets.test.files]
"Release/libcef.so" = "${contentSha}"
`);
      const sentinel = join(directory, "operational-runtime");
      await writeFile(sentinel, "preserve");
      const result = spawnSync("python3", [script, "--target", "test", "--archive", archive, "--manifest", manifest, "--destination", join(directory, "installed")], { encoding: "utf8" });
      expect(result.status).toBe(1);
      expect(JSON.parse(result.stderr).status).toBe("REJECTED");
      expect(result.stdout).toBe("");
      expect(await readFile(sentinel, "utf8")).toBe("preserve");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
}

test("fetch CLI refuses unqualified platforms and absent runtime without downloading", () => {
  for (const target of ["missing", "aarch64-apple-darwin"]) {
    const result = spawnSync("python3", [script, "--target", target, "--verify-only"], { encoding: "utf8" });
    expect(result.status).toBe(1);
    expect(JSON.parse(result.stderr).error).toContain("unavailable");
  }
});

test("fetch CLI recognizes the pinned Windows candidate without treating an absent runtime as unavailable", async () => {
  const directory = await mkdtemp(join(tmpdir(), "paneflow-fetch-windows-test-"));
  try {
    const result = spawnSync("python3", [
      script,
      "--target",
      "x86_64-pc-windows-msvc",
      "--verify-only",
      "--destination",
      join(directory, "installed"),
    ], { encoding: "utf8" });
    expect(result.status).toBe(1);
    expect(JSON.parse(result.stderr).error).toContain("runtime absent");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
