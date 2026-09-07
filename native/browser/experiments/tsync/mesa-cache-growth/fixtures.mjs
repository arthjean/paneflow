import { createHash, randomUUID } from "node:crypto";
import { readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createServer } from "node:http";

const directory = new URL("../../bench/browser/fixtures/", import.meta.url);
const files = ["empty.html", "page.html", "fixture.js", "fixture.css", "tile.svg"];
const scenarios = ["empty", "scroll", "animation", "ime", "popup", "download", "webgl", "network", "combined"];
const payload = "0123456789abcdef".repeat(64);

export async function fixtureBundle() {
  const assets = new Map(await Promise.all(files.map(async (name) => [name, await readFile(new URL(name, directory))])));
  const hash = createHash("sha256");
  for (const [name, bytes] of assets) hash.update(name).update("\0").update(bytes).update("\0");
  hash.update(JSON.stringify({ scenarios, payload }));
  return { assets, manifest: { schema_version: 1, sha256: hash.digest("hex"), scenarios } };
}

async function writeGateReceipt(directory, name, value) {
  const temporary = join(directory, `${name}.${randomUUID()}.tmp`);
  await writeFile(temporary, JSON.stringify(value), { flag: "wx", mode: 0o600 });
  await rename(temporary, join(directory, name));
}

async function serveShaderGate(response) {
  const directory = process.env.PANEFLOW_SHADER_GATE_DIRECTORY;
  const token = process.env.PANEFLOW_SHADER_GATE_TOKEN;
  await writeGateReceipt(directory, "armed.json", { schema_version: 1, token, request_received: true });
  let release = null;
  try { release = JSON.parse(await readFile(join(directory, "release.json"), "utf8")); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  response.setHeader("Cache-Control", "no-store");
  if (!release) { response.writeHead(202).end(); return; }
  if (release.token !== token || release.baseline_complete !== true) throw new Error("Invalid baseline gate receipt");
  await writeGateReceipt(directory, "observed.json", { token, release });
  response.writeHead(204).end();
}

export async function serveFixtures(port = 0) {
  const { assets, manifest } = await fixtureBundle();
  const server = createServer((request, response) => {
    const address = server.address();
    const expectedHost = `127.0.0.1:${address.port}`;
    if (request.headers.host !== expectedHost || !["GET", "HEAD"].includes(request.method)) {
      response.writeHead(403).end();
      return;
    }
    let pathname;
    try { pathname = new URL(request.url, `http://${expectedHost}`).pathname; }
    catch { response.writeHead(400).end(); return; }
    if (process.env.PANEFLOW_SHADER_GATE_DIRECTORY && process.env.PANEFLOW_SHADER_GATE_TOKEN
      && pathname === `/shader-gate/${process.env.PANEFLOW_SHADER_GATE_TOKEN}` && request.method === "GET") {
      void serveShaderGate(response).catch(() => { if (!response.headersSent) response.writeHead(500); response.end(); });
      return;
    }
    let bytes;
    let type = "text/html; charset=utf-8";
    if (pathname === "/manifest.json") {
      bytes = JSON.stringify(manifest);
      type = "application/json";
    } else if (pathname === "/payload" || pathname === "/download.bin") {
      bytes = payload;
      type = "application/octet-stream";
    } else if (scenarios.includes(pathname.slice(1))) {
      bytes = assets.get(pathname === "/empty" ? "empty.html" : "page.html");
    } else if (["/fixture.js", "/fixture.css", "/tile.svg"].includes(pathname)) {
      bytes = assets.get(pathname.slice(1));
      type = { "/fixture.js": "text/javascript", "/fixture.css": "text/css", "/tile.svg": "image/svg+xml" }[pathname];
    }
    if (bytes === undefined) { response.writeHead(404).end(); return; }
    response.setHeader("Cache-Control", "no-store");
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Content-Security-Policy", "default-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'");
    if (pathname === "/download.bin") response.setHeader("Content-Disposition", 'attachment; filename="paneflow-fixture.bin"');
    response.setHeader("Content-Type", type);
    response.setHeader("Content-Length", Buffer.byteLength(bytes));
    response.writeHead(200).end(request.method === "HEAD" ? undefined : bytes);
  });
  server.requestTimeout = 5000;
  server.headersTimeout = 5000;
  server.maxConnections = 32;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  return { server, url: `http://127.0.0.1:${server.address().port}`, manifest };
}
