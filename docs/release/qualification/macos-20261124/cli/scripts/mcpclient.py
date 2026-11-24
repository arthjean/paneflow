import json, os, subprocess, sys, time
binp = os.environ.get("MCP_BIN", os.path.expanduser("~/.paneflow-dev-qual-cli/bin/paneflow-mcp"))
calls = json.loads(sys.argv[1]) if len(sys.argv) > 1 else []
p = subprocess.Popen([binp], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
nid = [0]
def rpc(method, params=None, notify=False):
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None: msg["params"] = params
    if not notify:
        nid[0] += 1; msg["id"] = nid[0]
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
    if notify: return None
    return json.loads(p.stdout.readline())
print("initialize:", json.dumps(rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "qual-probe", "version": "0"}}))[:400])
rpc("notifications/initialized", {}, notify=True)
if "--tools" in sys.argv:
    print("tools/list:", json.dumps(rpc("tools/list", {}), indent=1))
for c in calls:
    if c.get("sleep"): time.sleep(c["sleep"]); continue
    r = rpc("tools/call", {"name": c["name"], "arguments": c.get("arguments", {})})
    print("==", c["name"], json.dumps(c.get("arguments", {})))
    print(json.dumps(r, indent=1)[:2500])
p.stdin.close()
try: p.wait(3)
except Exception: p.kill()
err = p.stderr.read()
if err.strip(): print("stderr:", err[:1000])
