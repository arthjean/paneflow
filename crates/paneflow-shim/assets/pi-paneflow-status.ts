// Paneflow status bridge for Pi - INSTALLED AND REMOVED AUTOMATICALLY by
// paneflow-shim around each `pi` session started inside a Paneflow terminal.
// Safe to delete; do not edit (changes are overwritten).

import net from "node:net";

export default function (pi) {
  const send = (method, params) => {
    const sock = process.env["PANEFLOW_SOCKET_PATH"];
    const wsId = process.env["PANEFLOW_WORKSPACE_ID"];
    if (!sock || !wsId) return;
    try {
      const p = {
        workspace_id: Number(wsId),
        tool: "pi",
        pid: Number(process.env["PANEFLOW_AI_PID"] || process.pid),
        ...(params ?? {}),
      };
      const sid = process.env["PANEFLOW_SURFACE_ID"];
      if (sid) p.surface_id = Number(sid);
      const frame =
        JSON.stringify({ jsonrpc: "2.0", method, params: p, id: 1 }) + "\n";
      const conn = net.connect(sock);
      conn.on("error", () => {});
      conn.on("connect", () => {
        conn.end(frame);
      });
    } catch {
    }
  };
  pi.on("agent_start", () => send("ai.prompt_submit", { hook_payload: {} }));
  pi.on("agent_end", () => send("ai.stop", { hook_payload: {} }));
  pi.on("tool_execution_start", () => send("ai.tool_use", { hook_payload: {} }));
  pi.on("tool_execution_end", () => send("ai.tool_use", { hook_payload: {} }));
}
