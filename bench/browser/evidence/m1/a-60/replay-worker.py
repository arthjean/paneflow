#!/usr/bin/env python3
import argparse
import base64
import ctypes
import errno
import fcntl
import gc
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
import struct
import sys
import termios
import threading
import time
import tty


class Timespec(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long), ("tv_nsec", ctypes.c_long)]


libc = ctypes.CDLL(None, use_errno=True)
libc.clock_nanosleep.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.POINTER(Timespec), ctypes.c_void_p]
libc.clock_nanosleep.restype = ctypes.c_int


def sleep_until(timestamp):
    deadline = Timespec(timestamp // 1_000_000_000, timestamp % 1_000_000_000)
    while True:
        result = libc.clock_nanosleep(time.CLOCK_MONOTONIC, 1, ctypes.byref(deadline), None)
        if result == 0:
            return
        if result != errno.EINTR:
            raise OSError(result, os.strerror(result))


def write_json(path, value):
    temporary = path.with_name(f".{path.name}.{os.getpid()}")
    with temporary.open("x", encoding="utf8") as stream:
        json.dump(value, stream, separators=(",", ":"))
        stream.write("\n")
    try:
        os.link(temporary, path)
    finally:
        temporary.unlink()


def write_records(path, records):
    with path.open("x", encoding="utf8") as stream:
        for record in records:
            stream.write(json.dumps(record, separators=(",", ":")) + "\n")


def process_identity():
    stat = Path("/proc/self/stat").read_text()
    return {"pid": os.getpid(), "start_ticks": stat[stat.rfind(")") + 2:].split()[19]}


def winsize():
    rows, columns, _, _ = struct.unpack("HHHH", fcntl.ioctl(1, termios.TIOCGWINSZ, b"\0" * 8))
    return {"rows": rows, "columns": columns}


def rpc(socket_path, request_id, method, params):
    request = json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}).encode() + b"\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(2)
        connection.connect(socket_path)
        before = time.monotonic_ns()
        connection.sendall(request)
        sent = time.monotonic_ns()
        data = bytearray()
        while b"\n" not in data:
            block = connection.recv(min(8192, 262144 - len(data)))
            if not block:
                raise RuntimeError("IPC closed before its response")
            data.extend(block)
            if len(data) >= 262144:
                raise RuntimeError("IPC response exceeds 256 KiB")
        received = time.monotonic_ns()
    response = json.loads(data.split(b"\n", 1)[0])
    if response.get("id") != request_id or response.get("error") is not None:
        raise RuntimeError(f"IPC rejected qualification input: {response}")
    result = response.get("result")
    if not isinstance(result, dict) or result.get("queued") is not True:
        raise RuntimeError(f"qualification input was not queued: {result}")
    return before, sent, received, result


def coordinate(directory, bundle, seconds, socket_path, surfaces):
    origin = time.monotonic_ns() + 2_000_000_000
    records = [{"event": "started", "clock": "CLOCK_MONOTONIC", **process_identity()}]
    failed = None
    try:
        inputs = [event for event in bundle["events"] if "input" in event and event["at_ns"] < seconds * 1_000_000_000]
        for terminal in range(4):
            selected = [event for event in inputs if event["terminal"] == terminal]
            if not selected:
                continue
            schedule = [{"input": event["input"], "planned_ns": origin + event["at_ns"]} for event in selected]
            before, sent, received, response = rpc(socket_path, terminal + 1, "qualification.input", {"surface_id": surfaces[terminal], "events": schedule})
            if response.get("scheduled_events") != len(selected):
                raise RuntimeError("application did not accept the complete qualification schedule")
            for event in selected:
                records.append({"event": "input", "sequence": event["sequence"], "terminal": terminal, "surface_id": surfaces[terminal], "input": event["input"], "planned_ns": origin + event["at_ns"], "registration_start_ns": before, "registration_sent_ns": sent, "registration_response_ns": received, "response": response})
        if time.monotonic_ns() + 500_000_000 >= origin:
            raise RuntimeError("input schedule registration missed the replay startup margin")
        write_json(directory / "start.json", {"clock": "CLOCK_MONOTONIC", "origin_ns": origin, "duration_ns": seconds * 1_000_000_000})
        sleep_until(origin + seconds * 1_000_000_000 + 500_000_000)
    except Exception as error:
        failed = str(error)
        records.append({"event": "fatal", "message": failed, "actual_ns": time.monotonic_ns()})
    finally:
        write_records(directory / "inputs.jsonl", records)
        write_json(directory / "coordinator-complete.json", {"failed": failed, "finished_ns": time.monotonic_ns()})
    if failed:
        raise RuntimeError(failed)


def replay(directory, bundle, terminal):
    if not os.isatty(0) or not os.isatty(1):
        raise RuntimeError("replay worker requires the real terminal PTY")
    original = termios.tcgetattr(0)
    tty.setraw(0)
    records = []
    output_lock = threading.Lock()
    stop = threading.Event()
    fatal = []
    automatic_gc = gc.isenabled()
    gc_paused = False

    def output(payload):
        start = time.monotonic_ns()
        view = memoryview(payload)
        while view:
            written = os.write(1, view)
            view = view[written:]
        return start, time.monotonic_ns()

    def read_input():
        pending = bytearray()
        try:
            while not stop.is_set():
                if not select.select([0], [], [], 0.05)[0]:
                    continue
                block = os.read(0, 4096)
                if not block:
                    raise RuntimeError("PTY stdin closed")
                for byte in block:
                    pending.append(byte)
                    if byte not in (10, 13):
                        if len(pending) > 64:
                            raise RuntimeError("unexpected oversized qualification input")
                        continue
                    raw = bytes(pending)
                    pending.clear()
                    match = re.fullmatch(rb"i([0-9]+)[\r\n]", raw)
                    if not match:
                        raise RuntimeError(f"unexpected PTY input bytes: {raw!r}")
                    tick = int(match[1])
                    received = time.monotonic_ns()
                    marker = f"pf-input:{terminal}:{tick}"
                    with output_lock:
                        before, after = output(f"\x1b7\x1b[1;1H\x1b[2K\x1b[0m{marker}\x1b8".encode())
                        records.append({"event": "echo", "terminal": terminal, "tick": tick, "read_ns": received, "actual_ns": before, "completed_ns": after, "input_base64": base64.b64encode(raw).decode(), "marker": marker, **winsize()})
        except Exception as error:
            fatal.append(str(error))
            stop.set()

    reader = threading.Thread(target=read_input, daemon=True)
    try:
        identity = process_identity()
        write_json(directory / f"worker-{terminal}-ready.json", {"terminal": terminal, "clock": "CLOCK_MONOTONIC", **identity, **winsize()})
        timeout = time.monotonic() + 90
        while not (directory / "start.json").exists():
            if time.monotonic() >= timeout:
                raise RuntimeError("timed out waiting for common replay origin")
            time.sleep(0.01)
        start = json.loads((directory / "start.json").read_text())
        origin = start["origin_ns"]
        records.append({"event": "started", "clock": "CLOCK_MONOTONIC", "terminal": terminal, "origin_ns": origin, **identity})
        geometry = winsize()
        setup = f"\x1b[2;{geometry['rows']}r\x1b[2;1H".encode()
        before, after = output(setup)
        records.append({"event": "setup", "terminal": terminal, "actual_ns": before, "completed_ns": after, "output_base64": base64.b64encode(setup).decode(), **geometry})
        events = [(event, base64.b64decode(event["output_base64"], validate=True)) for event in bundle["events"] if event["terminal"] == terminal and "output_base64" in event and event["at_ns"] < start["duration_ns"]]
        gc.collect()
        gc.disable()
        gc_paused = True
        records.append({"event": "gc_policy", "automatic_collections_during_replay": False, "previously_enabled": automatic_gc, "at_ns": time.monotonic_ns()})
        reader.start()
        for event, payload in events:
            planned = origin + event["at_ns"]
            sleep_until(planned)
            if fatal:
                raise RuntimeError(fatal[0])
            with output_lock:
                before, after = output(payload)
                records.append({"event": "output", "sequence": event["sequence"], "terminal": terminal, "planned_ns": planned, "actual_ns": before, "completed_ns": after, "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest(), **winsize()})
        sleep_until(origin + start["duration_ns"] + 500_000_000)
        if fatal:
            raise RuntimeError(fatal[0])
    except Exception as error:
        fatal.append(str(error))
        records.append({"event": "fatal", "message": str(error), "actual_ns": time.monotonic_ns()})
    finally:
        stop.set()
        if reader.is_alive():
            reader.join(timeout=1)
        if gc_paused and automatic_gc:
            gc.enable()
        termios.tcsetattr(0, termios.TCSANOW, original)
        write_records(directory / f"worker-{terminal}.jsonl", records)
        write_json(directory / f"worker-{terminal}-complete.json", {"failed": fatal[0] if fatal else None, "finished_ns": time.monotonic_ns()})
    timeout = time.monotonic() + 15
    while not (directory / "stop").exists() and time.monotonic() < timeout:
        time.sleep(0.05)
    if fatal:
        raise RuntimeError(fatal[0])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["terminal", "coordinator"])
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--terminal", type=int, choices=range(4))
    parser.add_argument("--seconds", type=int, default=70)
    parser.add_argument("--socket")
    parser.add_argument("--surfaces")
    args = parser.parse_args()
    directory = args.directory.resolve(strict=True)
    bundle = json.loads((directory / "replay-plan.json").read_text())
    if args.mode == "terminal":
        if args.terminal is None:
            parser.error("terminal mode requires --terminal")
        replay(directory, bundle, args.terminal)
    else:
        surfaces = json.loads(args.surfaces or "null")
        if not args.socket or not isinstance(surfaces, list) or len(surfaces) != 4 or not all(isinstance(value, int) and value > 0 for value in surfaces):
            parser.error("coordinator requires --socket and four positive --surfaces ids")
        if args.seconds < 1 or args.seconds > 70:
            parser.error("--seconds must be in 1..70")
        coordinate(directory, bundle, args.seconds, args.socket, surfaces)


if __name__ == "__main__":
    main()
