import { createConnection } from "node:net";
import { readFile } from "node:fs/promises";

export function rpc(pipe, method, params = {}) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(pipe);
    let received = "";
    socket.setTimeout(15000, () => socket.destroy(new Error(`IPC timeout: ${method}`)));
    socket.on("error", reject);
    socket.on("connect", () => socket.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method, params })}\n`));
    socket.on("data", (chunk) => {
      received += chunk.toString("utf8");
      if (received.length > 1048576) { socket.destroy(new Error("IPC response limit")); return; }
      const end = received.indexOf("\n");
      if (end < 0) return;
      socket.end();
      try {
        const response = JSON.parse(received.slice(0, end));
        if (response.error || response.result?.error) throw new Error(JSON.stringify(response));
        resolve(response.result);
      } catch (error) { reject(error); }
    });
  });
}

if (import.meta.main) {
  const [pipe, method, parameterSource = "{}"] = process.argv.slice(2);
  try {
    const parameters = parameterSource.startsWith("@")
      ? await readFile(parameterSource.slice(1), "utf8")
      : parameterSource;
    console.log(JSON.stringify(await rpc(pipe, method, JSON.parse(parameters))));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
