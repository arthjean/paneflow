#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=${1:-$HOME/pfq-cli-fake/inpane-principal.out}
{
echo "== env"; env | grep -E '^PANEFLOW_(SESSION_ID|SURFACE_ID|SOCKET_PATH|HOME)='
for on in window worker host; do echo "== whoami --on $on"; $PF api call system.whoami --on $on 2>&1; echo "[exit=$?]"; done
for m in layout.sweep approval.decide approval.follow session.runtime.bind session.workspace peer.classify layout.attach layout.launch; do echo "== $m"; $PF api call $m --args '{}' 2>&1 | head -5; echo "[exit=${PIPESTATUS[0]}]"; done
echo "== setsid double-fork whoami"
python3 - <<'PY'
import os, subprocess, sys
r, w = os.pipe()
pid = os.fork()
if pid == 0:
    os.setsid()
    if os.fork() == 0:
        os.close(r)
        out = subprocess.run(["/Users/m1/dev/paneflow/target/debug/paneflow","api","call","system.whoami"],capture_output=True,text=True)
        os.write(w, (out.stdout+out.stderr).encode()); os._exit(0)
    os._exit(0)
os.close(w); os.waitpid(pid,0)
data=b""
while True:
    c=os.read(r,4096)
    if not c: break
    data+=c
print(data.decode())
PY
echo "== DONE"
} > "$out" 2>&1
