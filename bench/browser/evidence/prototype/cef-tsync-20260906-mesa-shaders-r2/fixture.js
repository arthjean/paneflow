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
  document.documentElement.dataset.fixtureState = "missing";
  function shaderFailure(message) {
    document.title = "PANEFLOW_SHADER_CHURN_FAILED";
    fail(message);
  }
  const canvas = node("canvas", "Shader compilation qualification");
  canvas.width = 64;
  canvas.height = 64;
  const output = node("output", "Compiling shaders");
  const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: false });
  if (!gl) return shaderFailure("WebGL2 unavailable");
  canvas.addEventListener("webglcontextlost", () => shaderFailure("WebGL context lost during shader compilation"));
  const vertex = gl.createShader(gl.VERTEX_SHADER);
  if (!vertex) return shaderFailure("Vertex shader creation failed");
  gl.shaderSource(vertex, "#version 300 es\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.0-1.0,0.0,1.0);}");
  gl.compileShader(vertex);
  if (!gl.getShaderParameter(vertex, gl.COMPILE_STATUS)) return shaderFailure("Vertex shader compilation failed");
  gl.viewport(0, 0, 64, 64);
  const pixel = new Uint8Array(4);
  async function compileAndRender() {
    for (let index = 0; index < 120; index += 1) {
      if (gl.isContextLost()) throw new Error("WebGL context lost");
      const fragment = gl.createShader(gl.FRAGMENT_SHADER);
      const program = gl.createProgram();
      if (!fragment || !program) throw new Error("Shader resource creation failed");
      const red = (index + 1) / 121;
      gl.shaderSource(fragment, `#version 300 es\nprecision highp float;out vec4 color;void main(){color=vec4(${red.toFixed(8)},0.25,0.75,1.0);}`);
      gl.compileShader(fragment);
      if (!gl.getShaderParameter(fragment, gl.COMPILE_STATUS)) throw new Error(`Fragment compilation failed at ${index}`);
      gl.attachShader(program, vertex);
      gl.attachShader(program, fragment);
      gl.linkProgram(program);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(`Program link failed at ${index}`);
      gl.useProgram(program);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.readPixels(16, 16, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
      if (gl.getError() !== gl.NO_ERROR || Math.abs(pixel[0] - Math.round(red * 255)) > 3 || Math.abs(pixel[1] - 64) > 3 || Math.abs(pixel[2] - 191) > 3 || pixel[3] !== 255) {
        throw new Error(`Shader pixel mismatch at ${index}: ${Array.from(pixel).join(",")}`);
      }
      gl.useProgram(null);
      gl.deleteProgram(program);
      gl.deleteShader(fragment);
      output.textContent = `${index + 1}/120 shader programs verified`;
      if (index % 4 === 3) await new Promise(requestAnimationFrame);
    }
    if (gl.isContextLost()) throw new Error("WebGL context lost before completion");
    gl.deleteShader(vertex);
    document.documentElement.dataset.fixtureState = "ready";
    output.textContent = "120 shader programs and pixel checks passed";
    document.title = "PANEFLOW_SHADER_CHURN_PASSED:120";
  }
  compileAndRender().catch(error => shaderFailure(String(error)));
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
