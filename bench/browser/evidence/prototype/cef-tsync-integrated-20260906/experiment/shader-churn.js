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
