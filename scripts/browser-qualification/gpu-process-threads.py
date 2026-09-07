import json
import os
import re
import sys
import time

MARK = sys.argv[1]
OUT = sys.argv[2]
LIBS = re.compile(r"(mesa|_dri\.so|libEGL|libGL|vulkan|nvidia|wayland|angle|libgbm|libdrm|radeonsi|radv|glvnd|swrast|llvm|ffmpeg|va)", re.I)

seen = {}
start = time.monotonic()
out = open(OUT, "w")


def read(path):
    try:
        with open(path, "rb") as handle:
            return handle.read()
    except OSError:
        return b""


def libs(pid):
    found = set()
    for line in read(f"/proc/{pid}/maps").decode(errors="replace").splitlines():
        parts = line.split()
        if len(parts) >= 6 and parts[5].startswith("/") and LIBS.search(parts[5]):
            found.add(os.path.basename(parts[5]))
    return sorted(found)


def threads(pid):
    names = []
    try:
        tids = sorted(int(t) for t in os.listdir(f"/proc/{pid}/task"))
    except OSError:
        return None
    for tid in tids:
        comm = read(f"/proc/{pid}/task/{tid}/comm").decode(errors="replace").strip()
        stat = read(f"/proc/{pid}/task/{tid}/stat").decode(errors="replace")
        started = stat[stat.rfind(")") + 2:].split()
        names.append((tid, comm, int(started[19]) if len(started) > 19 else 0))
    return names


while True:
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        pid = int(name)
        comm = read(f"/proc/{pid}/comm").decode(errors="replace").strip()
        cmd = read(f"/proc/{pid}/cmdline")
        if not comm.startswith(MARK):
            continue
        current = threads(pid)
        if current is None:
            continue
        status = read(f"/proc/{pid}/status").decode(errors="replace")
        seccomp = re.search(r"^Seccomp:\s+(\d+)", status, re.M)
        seccomp = int(seccomp.group(1)) if seccomp else -1
        key = (seccomp,) + tuple((tid, comm) for tid, comm, _ in current)
        previous = seen.get(pid)
        if previous is None:
            seen[pid] = key
            out.write(json.dumps({"t": round(time.monotonic() - start, 4), "pid": pid, "event": "first_seen", "seccomp": seccomp, "threads": current, "libs": libs(pid), "cmd": cmd.decode(errors="replace").split("\0")[:40]}) + "\n")
            out.flush()
        elif previous != key:
            seen[pid] = key
            out.write(json.dumps({"t": round(time.monotonic() - start, 4), "pid": pid, "event": "threads_changed", "seccomp": seccomp, "threads": current, "libs": libs(pid)}) + "\n")
            out.flush()
    for pid in list(seen):
        if not os.path.exists(f"/proc/{pid}"):
            out.write(json.dumps({"t": round(time.monotonic() - start, 4), "pid": pid, "event": "exited"}) + "\n")
            out.flush()
            del seen[pid]
    time.sleep(0.005)
