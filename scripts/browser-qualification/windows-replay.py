import json
import msvcrt
from pathlib import Path
import sys
import time

terminal = int(sys.argv[1])
directory = Path(sys.argv[2])
if terminal not in range(4) or not sys.stdout.isatty():
    raise SystemExit("replay requires terminal 0..3 inside a PTY")
bundle = json.loads((directory / "replay.json").read_text(encoding="utf-8"))
events = [event for event in bundle["events"] if event["terminal"] == terminal and "output_base64" in event]
log = (directory / f"terminal-{terminal}.jsonl").open("x", encoding="utf-8")

def record(event, **fields):
    log.write(json.dumps({"event": event, "at_ns": time.perf_counter_ns(), "terminal": terminal, **fields}) + "\n")
    log.flush()

line = ""

def read_input():
    global line
    while msvcrt.kbhit():
        character = msvcrt.getwch()
        if character in ("\r", "\n"):
            if line.startswith("i") and line[1:].isdigit():
                tick = int(line[1:])
                sys.stdout.write(f"\x1b7\x1b[1;1H\x1b[2Kpf-input:{terminal}:{tick}\x1b8")
                sys.stdout.flush()
                record("echo", tick=tick)
            line = ""
        elif len(line) < 32:
            line += character

record("ready", clock="QPC", workload_sha256=bundle["sha256"])
deadline = time.perf_counter_ns() + 90000000000
barrier = directory / "origin.json"
while not barrier.exists():
    if time.perf_counter_ns() > deadline:
        raise SystemExit("replay start barrier timed out")
    time.sleep(0.05)
origin = json.loads(barrier.read_text(encoding="utf-8"))["origin_ns"]
sys.stdout.write("\x1b[2J\x1b[H\x1b[2;r\x1b[2;1H")
sys.stdout.flush()
import base64
for event in events:
    planned = origin + event["at_ns"]
    while (remaining := planned - time.perf_counter_ns()) > 0:
        read_input()
        if remaining > 2000000:
            time.sleep(min(0.001, (remaining - 1000000) / 1000000000))
    started = time.perf_counter_ns()
    data = base64.b64decode(event["output_base64"])
    sys.stdout.buffer.write(data)
    sys.stdout.flush()
    record("output", sequence=event["sequence"], planned_ns=planned, write_start_ns=started, bytes=len(data), delivery_error_ns=abs(started-planned))
    read_input()
record("finished")
sys.stdout.write("\x1b[r")
sys.stdout.flush()
log.close()
