function webglFixture() {
  const token = "__PANEFLOW_GATE_TOKEN__";
  document.documentElement.dataset.fixtureState = "missing";
  const canvas = node("canvas", "Shader compilation qualification");
  canvas.width = 64;
  canvas.height = 64;
  const output = node("output", "Preparing shader baseline");
  function shaderFailure(message) {
    document.title = "PANEFLOW_SHADER_CHURN_FAILED";
    fail(message);
  }
  async function run() {
    document.title = "PANEFLOW_SHADER_CHURN_ARMED";
    output.textContent = "Waiting for native thread census before creating WebGL resources";
    const deadline = performance.now() + 60000;
    while (true) {
      if (performance.now() > deadline) throw new Error("Native baseline gate timed out");
      const response = await fetch(`/shader-gate/${token}`, { cache: "no-store" });
      if (response.status === 204) break;
      if (response.status !== 202) throw new Error(`Native gate failed: ${response.status}`);
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    document.title = "PANEFLOW_SHADER_CHURN_STARTED";
    const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: false });
    if (!gl) return shaderFailure("WebGL2 unavailable");
    canvas.addEventListener("webglcontextlost", () => shaderFailure("WebGL context lost"));
    const vertex = gl.createShader(gl.VERTEX_SHADER);
    if (!vertex) return shaderFailure("Vertex shader creation failed");
    gl.shaderSource(vertex, "#version 300 es\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.0-1.0,0.0,1.0);}");
    gl.compileShader(vertex);
    if (!gl.getShaderParameter(vertex, gl.COMPILE_STATUS)) return shaderFailure("Vertex shader compilation failed");
    gl.viewport(0, 0, 64, 64);
    const pixel = new Uint8Array(4);
    function compile(index) {
      const fragment = gl.createShader(gl.FRAGMENT_SHADER);
      const program = gl.createProgram();
      if (!fragment || !program) throw new Error("Shader resource creation failed");
      const red = (index + 1) / 123;
      gl.shaderSource(fragment, `#version 300 es\nprecision highp float;out vec4 color;void main(){color=vec4(${red.toFixed(8)},0.25,0.75,1.0);}`);
      gl.compileShader(fragment);
      gl.attachShader(program, vertex);
      gl.attachShader(program, fragment);
      gl.linkProgram(program);
      return { fragment, program, red, index };
    }
    function verify(item) {
      if (gl.isContextLost() || !gl.getShaderParameter(item.fragment, gl.COMPILE_STATUS)
        || !gl.getProgramParameter(item.program, gl.LINK_STATUS)) throw new Error(`Compilation failed: ${item.index}`);
      gl.useProgram(item.program);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.readPixels(16, 16, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
      if (gl.getError() !== gl.NO_ERROR || Math.abs(pixel[0] - Math.round(item.red * 255)) > 3
        || Math.abs(pixel[1] - 64) > 3 || Math.abs(pixel[2] - 191) > 3 || pixel[3] !== 255) {
        throw new Error(`Shader pixel mismatch at ${item.index}: ${Array.from(pixel).join(",")}`);
      }
      gl.useProgram(null);
      gl.deleteProgram(item.program);
      gl.deleteShader(item.fragment);
    }
    verify(compile(0));
    for (let first = 1; first <= 120; first += 60) {
      const batch = [];
      for (let index = first; index < first + 60; index += 1) batch.push(compile(index));
      for (const item of batch) verify(item);
      output.textContent = `${first + 59}/120 post-baseline shader programs verified`;
      await new Promise(requestAnimationFrame);
    }
    if (gl.isContextLost()) throw new Error("WebGL context lost before completion");
    gl.deleteShader(vertex);
    document.documentElement.dataset.fixtureState = "ready";
    document.title = "PANEFLOW_SHADER_CHURN_PASSED:120";
  }
  run().catch(error => shaderFailure(String(error)));
}
