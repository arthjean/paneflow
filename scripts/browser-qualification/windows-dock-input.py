import ctypes
import os
from ctypes import wintypes
import json
import math
from pathlib import Path
import sys
import time

pid = int(sys.argv[1])
directory = Path(sys.argv[2])
seconds = float(sys.argv[3])
user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.GetForegroundWindow.restype = wintypes.HWND
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.BringWindowToTop.argtypes = [wintypes.HWND]
user32.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
windows = []


@callback_type
def collect(hwnd, _):
    owner = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
    if owner.value == pid and user32.IsWindowVisible(hwnd):
        windows.append(hwnd)
    return True


user32.EnumWindows(collect, 0)
if len(windows) != 1:
    raise SystemExit(f"Expected one visible benchmark window, found {len(windows)}")
window = windows[0]
current_thread = ctypes.windll.kernel32.GetCurrentThreadId()
foreground_thread = user32.GetWindowThreadProcessId(user32.GetForegroundWindow(), None)
attached = user32.AttachThreadInput(current_thread, foreground_thread, True)
try:
    user32.ShowWindow(window, 9)
    user32.BringWindowToTop(window)
    user32.SetForegroundWindow(window)
finally:
    if attached:
        user32.AttachThreadInput(current_thread, foreground_thread, False)
time.sleep(0.5)
if user32.GetForegroundWindow() != window:
    raise SystemExit("Benchmark window could not become foreground")


def viewport():
    rows = (directory / "events.jsonl").read_text(encoding="utf-8").splitlines()
    for line in reversed(rows):
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event["event"] == "viewport":
            return event["fields"]
    raise RuntimeError("No measured browser viewport")


def screen_point(x, y):
    point = wintypes.POINT(round(x), round(y))
    if not user32.ClientToScreen(window, ctypes.byref(point)):
        raise ctypes.WinError(ctypes.get_last_error())
    return point.x, point.y


def move(x, y):
    if not user32.SetCursorPos(round(x), round(y)):
        raise ctypes.WinError(ctypes.get_last_error())


phases = []
inputs = (directory / "inputs.jsonl").open("x", encoding="utf-8")


def mark(name):
    event = {"name": name, "clock": "QPC", "at_ns": time.perf_counter_ns()}
    phases.append(event)
    (directory / "phases.json").write_text(json.dumps(phases, indent=2), encoding="utf-8")
    print(json.dumps(event), flush=True)


def record(kind, planned, **fields):
    inputs.write(json.dumps({"kind": kind, "planned_ns": planned,
        "at_ns": time.perf_counter_ns(), **fields}) + "\n")


try:
    for phase in ("steady", "scroll", "resize"):
        mark("prepare_" + phase)
        time.sleep(5)
        bounds = viewport()
        scale = bounds["scale"]
        center = screen_point((bounds["x"] + bounds["width"] / 2) * scale,
                              (bounds["y"] + bounds["height"] / 2) * scale)
        move(*center)
        anchor = screen_point((bounds["x"] - 2) * scale,
                              (bounds["y"] + bounds["height"] / 2) * scale)
        if phase == "resize":
            move(*anchor)
            user32.mouse_event(0x0002, 0, 0, 0, 0)
        mark(phase)
        start = phases[-1]["at_ns"]
        tick = 0
        period = 1 / (15 if phase == "scroll" else float(os.environ.get("PANEFLOW_DOCK_DRAG_HZ", "60")))
        while time.perf_counter_ns() < start + seconds * 1e9:
            planned = start + round(tick * period * 1e9)
            remaining = (planned - time.perf_counter_ns()) / 1e9
            if remaining > 0:
                time.sleep(remaining)
            if user32.GetForegroundWindow() != window:
                raise RuntimeError("Foreground changed during benchmark: capture rejected")
            elapsed = (time.perf_counter_ns() - start) / 1e9
            if phase == "scroll":
                delta = -120 if int(elapsed / 5) % 2 == 0 else 120
                user32.mouse_event(0x0800, 0, 0, ctypes.c_uint32(delta).value, 0)
                record("wheel", planned, delta=delta)
            elif phase == "resize":
                offset = 180 * scale * (1 - math.cos(elapsed * math.pi)) / 2
                move(anchor[0] + offset, anchor[1])
                record("drag", planned, x=anchor[0] + offset, y=anchor[1])
            tick += 1
        if phase == "resize":
            user32.mouse_event(0x0004, 0, 0, 0, 0)
        inputs.flush()
        mark("end_" + phase)
finally:
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    inputs.close()
