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

if (scenario === "scroll" || scenario === "combined") scrollFixture();
if (scenario === "animation" || scenario === "combined") animationFixture();
if (scenario === "webgl" || scenario === "combined") webglFixture();
if (scenario === "network" || scenario === "combined") networkFixture();
if (scenario === "ime") imeFixture();
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
