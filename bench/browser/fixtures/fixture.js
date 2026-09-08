const root = document.querySelector("#fixture");
const scenario = location.pathname.slice(1);
root.querySelector("h1").textContent = `Fixture: ${scenario}`;
document.documentElement.dataset.fixtureState = "ready";

function node(tag, text, parent = root) {
  const element = document.createElement(tag);
  element.textContent = text;
  parent.append(element);
  return element;
}

function fail(message) {
  document.documentElement.dataset.fixtureState = "failed";
  node("output", message);
}

function scrollFixture() {
  const area = node("div", "");
  area.className = "scroll";
  for (let index = 0; index < 1000; index += 1) {
    const row = node("div", `Row ${String(index).padStart(4, "0")} abcdefghijklmnopqrstuvwxyz 0123456789`, area);
    row.className = "row";
    const image = node("img", "", row);
    image.src = "/tile.svg";
    image.alt = "Blue and white pattern";
  }
  let origin;
  function tick(timestamp) {
    origin ??= timestamp;
    const position = ((timestamp - origin) * 0.24) % (2 * (area.scrollHeight - area.clientHeight));
    area.scrollTop = Math.min(position, 2 * (area.scrollHeight - area.clientHeight) - position);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}

function animationFixture() {
  const track = node("div", "");
  track.className = "track";
  node("div", "", track).className = "moving";
}

function webglFixture() {
  const canvas = node("canvas", "WebGL fixture");
  canvas.width = 640;
  canvas.height = 360;
  const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: false });
  if (!gl) return fail("WebGL2 unavailable");
  const program = gl.createProgram();
  const vertex = gl.createShader(gl.VERTEX_SHADER);
  const fragment = gl.createShader(gl.FRAGMENT_SHADER);
  if (!program || !vertex || !fragment) return fail("WebGL resource creation failed");
  gl.shaderSource(vertex, "#version 300 es\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.0-1.0,0.0,1.0);}");
  gl.shaderSource(fragment, "#version 300 es\nprecision highp float;uniform float phase;out vec4 color;void main(){vec2 p=gl_FragCoord.xy/vec2(640.0,360.0);color=vec4(p,0.5+0.5*sin(phase),1.0);}");
  for (const shader of [vertex, fragment]) {
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) return fail("WebGL shader compilation failed");
    gl.attachShader(program, shader);
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return fail("WebGL program link failed");
  gl.useProgram(program);
  const phase = gl.getUniformLocation(program, "phase");
  let origin;
  function tick(timestamp) {
    origin ??= timestamp;
    gl.uniform1f(phase, (timestamp - origin) / 1000);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    requestAnimationFrame(tick);
  }
  canvas.addEventListener("webglcontextlost", () => fail("WebGL context lost"));
  requestAnimationFrame(tick);
}

function networkFixture() {
  let sequence = 0;
  let pending = false;
  const status = node("output", "Network: 0 completed requests");
  setInterval(async () => {
    if (pending) return fail("Network request exceeded the fixed 250 ms interval");
    pending = true;
    try {
      const response = await fetch(`/payload?sequence=${sequence}`, { cache: "no-store", signal: AbortSignal.timeout(200) });
      if (!response.ok || (await response.text()) !== "0123456789abcdef".repeat(64)) throw new Error("Invalid payload");
      sequence += 1;
      status.textContent = `Network: ${sequence} completed requests`;
    } catch {
      fail("Network fixture failed");
    } finally {
      pending = false;
    }
  }, 250);
}

function imeFixture() {
  const label = node("label", "Compose text, including Japanese and accented characters");
  label.htmlFor = "ime";
  const input = node("input", "");
  input.id = "ime";
  input.autocomplete = "off";
  const output = node("output", "No events");
  for (const event of ["compositionstart", "compositionupdate", "compositionend", "input"]) {
    input.addEventListener(event, () => { output.textContent = `${event}: ${input.value}`; });
  }
}

function serviceWorkerFixture() {
  const status = node("output", "Service worker: registering");
  if (!navigator.serviceWorker) return fail("Service workers unavailable");
  navigator.serviceWorker
    .register("/sw.js", { scope: "/" })
    .then((registration) => navigator.serviceWorker.ready.then(() => registration))
    .then(() => fetch("/service-worker-probe", { cache: "no-store" }))
    .then((response) => response.text())
    .then((text) => {
      if (text !== "served-by-service-worker") throw new Error("probe was not served by the worker");
      status.textContent = "Service worker: controlling and serving the probe";
    })
    .catch((error) => fail(`Service worker fixture failed: ${error.message}`));
}

async function webSocketFixture() {
  const status = node("output", "WebSocket: connecting");
  const endpoint = await fetch("/websocket.json", { cache: "no-store" })
    .then((response) => response.json())
    .then((body) => body.websocket)
    .catch(() => null);
  if (!endpoint) return fail("WebSocket endpoint unavailable");
  const socket = new WebSocket(endpoint);
  let echoes = 0;
  socket.addEventListener("open", () => socket.send("paneflow-fixture-0"));
  socket.addEventListener("message", (event) => {
    if (event.data !== `paneflow-fixture-${echoes}`) return fail("WebSocket echo mismatch");
    echoes += 1;
    status.textContent = `WebSocket: ${echoes} echoes`;
    if (echoes < 8) socket.send(`paneflow-fixture-${echoes}`);
  });
  socket.addEventListener("error", () => fail("WebSocket fixture failed"));
  socket.addEventListener("close", (event) => {
    if (echoes < 8 && !event.wasClean) fail("WebSocket closed before the fixture completed");
  });
}

function iframeFixture() {
  const allowed = node("iframe", "");
  allowed.src = "/frame";
  allowed.title = "Same-origin fixture frame";
  const refused = node("iframe", "");
  refused.src = "/embedded";
  refused.title = "Frame refused by X-Frame-Options";
  const status = node("output", "Frames: pending");
  allowed.addEventListener("load", () => {
    const reachable = (() => {
      try { return allowed.contentDocument?.documentElement.dataset.fixtureState === "ready"; }
      catch { return false; }
    })();
    status.textContent = reachable
      ? "Frames: same-origin frame reachable, /embedded refuses framing by X-Frame-Options"
      : "Frames: same-origin frame unreadable";
    if (!reachable) fail("Same-origin frame was not reachable");
  });
}

function authFixture() {
  const status = node("output", "Auth: requesting /private without credentials");
  fetch("/private", { cache: "no-store" })
    .then((response) => {
      if (response.status !== 401) throw new Error(`expected 401, received ${response.status}`);
      status.textContent = "Auth: /private refused without credentials; navigate to /private for the native prompt";
      const link = node("a", "Open /private and answer the authentication prompt");
      link.href = "/private";
    })
    .catch((error) => fail(`Auth fixture failed: ${error.message}`));
}

if (scenario === "scroll" || scenario === "combined") scrollFixture();
if (scenario === "animation" || scenario === "combined") animationFixture();
if (scenario === "webgl" || scenario === "combined") webglFixture();
if (scenario === "network" || scenario === "combined") networkFixture();
if (scenario === "ime") imeFixture();
if (scenario === "serviceworker") serviceWorkerFixture();
if (scenario === "websocket") webSocketFixture();
if (scenario === "iframe") iframeFixture();
if (scenario === "auth") authFixture();
if (scenario === "popup") {
  node("button", "Open local popup").addEventListener("click", () => {
    const popup = window.open("/ime", "qualification-popup", "width=640,height=480");
    node("output", popup ? "Popup created" : "Popup blocked or rerouted: inspect host policy");
  });
}
if (scenario === "download") {
  const link = node("a", "Download deterministic 1 KiB fixture");
  link.href = "/download.bin";
  link.download = "paneflow-fixture.bin";
}
