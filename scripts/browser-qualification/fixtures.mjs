import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createServer as createSocketServer } from "node:net";

const directory = new URL("../../bench/browser/fixtures/", import.meta.url);
export const FIXTURE_FILES = ["empty.html", "page.html", "fixture.js", "fixture.css", "tile.svg", "sw.js", "frame.html"];
const files = FIXTURE_FILES;
const scenarios = ["empty", "scroll", "animation", "ime", "popup", "download", "webgl", "network", "combined", "serviceworker", "websocket", "iframe", "auth"];
const payload = "0123456789abcdef".repeat(64);
const credential = { user: "paneflow", password: "fixture-only-secret" };
const framed = new Set(["iframe"]);

export function bundleSuffix(bundledScenarios) {
  return JSON.stringify({ scenarios: bundledScenarios, payload, user: credential.user });
}

export async function fixtureBundle() {
  const assets = new Map(await Promise.all(files.map(async (name) => [name, await readFile(new URL(name, directory))])));
  const hash = createHash("sha256");
  for (const [name, bytes] of assets) hash.update(name).update("\0").update(bytes).update("\0");
  hash.update(bundleSuffix(scenarios));
  return { assets, manifest: { schema_version: 2, sha256: hash.digest("hex"), scenarios } };
}

function authorized(request) {
  const header = request.headers.authorization ?? "";
  if (!header.startsWith("Basic ")) return false;
  const decoded = Buffer.from(header.slice(6), "base64").toString("utf8");
  return decoded === `${credential.user}:${credential.password}`;
}

function handshake(socket, head, expectedHost) {
  const text = head.toString("latin1");
  const [line, ...headers] = text.split("\r\n");
  const fields = new Map(headers.filter(Boolean).map((entry) => {
    const index = entry.indexOf(":");
    return [entry.slice(0, index).trim().toLowerCase(), entry.slice(index + 1).trim()];
  }));
  const key = fields.get("sec-websocket-key");
  if (line !== "GET /ws HTTP/1.1" || fields.get("host") !== expectedHost || fields.get("upgrade")?.toLowerCase() !== "websocket" || !key) {
    socket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
    return null;
  }
  return key;
}

function echoFrames(socket, key) {
  const accept = createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  let buffered = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 2) {
      const opcode = buffered[0] & 0x0f;
      const masked = (buffered[1] & 0x80) !== 0;
      let length = buffered[1] & 0x7f;
      let offset = 2;
      if (length === 126) { if (buffered.length < 4) return; length = buffered.readUInt16BE(2); offset = 4; }
      else if (length === 127) { socket.destroy(); return; }
      if (!masked || length > 4096 || buffered.length < offset + 4 + length) { if (!masked || length > 4096) socket.destroy(); return; }
      const mask = buffered.subarray(offset, offset + 4);
      const body = Buffer.from(buffered.subarray(offset + 4, offset + 4 + length));
      for (let index = 0; index < body.length; index += 1) body[index] ^= mask[index % 4];
      buffered = buffered.subarray(offset + 4 + length);
      if (opcode === 0x8) { socket.end(Buffer.from([0x88, 0x00])); return; }
      if (opcode !== 0x1) continue;
      const header = length < 126 ? Buffer.from([0x81, length]) : Buffer.concat([Buffer.from([0x81, 126]), Buffer.from([length >> 8, length & 0xff])]);
      socket.write(Buffer.concat([header, body]));
    }
  });
  socket.on("error", () => socket.destroy());
}

export async function serveWebSockets() {
  const server = createSocketServer({ noDelay: true }, (socket) => {
    socket.setTimeout(15_000, () => socket.destroy());
    socket.once("data", (head) => {
      if (head.length > 4096) { socket.destroy(); return; }
      const key = handshake(socket, head, `127.0.0.1:${server.address().port}`);
      if (key) echoFrames(socket, key);
    });
    socket.on("error", () => socket.destroy());
  });
  server.maxConnections = 8;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return server;
}

export async function serveFixtures(port = 0) {
  const { assets, manifest } = await fixtureBundle();
  const sockets = await serveWebSockets();
  const websocket = `ws://127.0.0.1:${sockets.address().port}/ws`;
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
    let bytes;
    let type = "text/html; charset=utf-8";
    let ancestors = "'none'";
    if (pathname === "/manifest.json") {
      bytes = JSON.stringify(manifest);
      type = "application/json";
    } else if (pathname === "/websocket.json") {
      bytes = JSON.stringify({ websocket });
      type = "application/json";
    } else if (pathname === "/payload" || pathname === "/download.bin") {
      bytes = payload;
      type = "application/octet-stream";
    } else if (pathname === "/private") {
      if (!authorized(request)) {
        response.setHeader("WWW-Authenticate", 'Basic realm="Paneflow fixture", charset="UTF-8"');
        response.writeHead(401).end();
        return;
      }
      bytes = JSON.stringify({ authenticated: true, user: credential.user });
      type = "application/json";
    } else if (pathname === "/frame") {
      bytes = assets.get("frame.html");
      ancestors = "'self'";
    } else if (pathname === "/embedded") {
      bytes = assets.get("frame.html");
      response.setHeader("X-Frame-Options", "DENY");
    } else if (pathname === "/sw.js") {
      bytes = assets.get("sw.js");
      type = "text/javascript";
      response.setHeader("Service-Worker-Allowed", "/");
    } else if (scenarios.includes(pathname.slice(1))) {
      bytes = assets.get(pathname === "/empty" ? "empty.html" : "page.html");
      if (framed.has(pathname.slice(1))) ancestors = "'self'";
    } else if (["/fixture.js", "/fixture.css", "/tile.svg"].includes(pathname)) {
      bytes = assets.get(pathname.slice(1));
      type = { "/fixture.js": "text/javascript", "/fixture.css": "text/css", "/tile.svg": "image/svg+xml" }[pathname];
    }
    if (bytes === undefined) { response.writeHead(404).end(); return; }
    response.setHeader("Cache-Control", "no-store");
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Content-Security-Policy", `default-src 'self'; connect-src 'self' ${websocket}; object-src 'none'; base-uri 'none'; frame-ancestors ${ancestors}`);
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
  const close = () => {
    server.close();
    server.closeAllConnections();
    sockets.close();
  };
  return { server, sockets, close, url: `http://127.0.0.1:${server.address().port}`, manifest, websocket, credential };
}
