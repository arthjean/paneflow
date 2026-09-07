import argparse
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


BUS_NAME = "org.gnome.Mutter.DisplayConfig"
OBJECT_PATH = "/org/gnome/Mutter/DisplayConfig"
ACTUAL_HZ = {60: 60.0, 120: 119.8787841796875}
MONITOR_PROPERTIES = {"color-mode": "u", "rgb-range": "u"}


def snapshot(raw):
    serial, monitors, logical, properties = raw
    return {
        "serial": serial,
        "monitors": [
            {
                "spec": list(spec),
                "modes": [
                    {"id": mode[0], "width": mode[1], "height": mode[2],
                     "refresh_hz": mode[3], "preferred_scale": mode[4],
                     "scales": list(mode[5]), "properties": mode[6]}
                    for mode in modes
                ],
                "properties": props,
            }
            for spec, modes, props in monitors
        ],
        "logical_monitors": [
            {"x": item[0], "y": item[1], "scale": item[2], "transform": item[3],
             "primary": item[4], "monitors": [list(spec) for spec in item[5]],
             "properties": item[6]}
            for item in logical
        ],
        "properties": properties,
    }


def physical_inventory(state, connectors):
    monitors = {item["spec"][0]: item for item in state["monitors"]}
    if len(state["monitors"]) != 2 or len(monitors) != 2 or set(monitors) != set(connectors):
        raise ValueError("the two requested connected outputs must be the complete display inventory")
    return monitors


def inventory(state, connectors):
    monitors = physical_inventory(state, connectors)
    logical = state["logical_monitors"]
    if len(logical) != 2 or any(len(item["monitors"]) != 1 for item in logical):
        raise ValueError("exactly two active, unmirrored logical monitors are required")
    if {item["monitors"][0][0] for item in logical} != set(connectors):
        raise ValueError("active outputs do not match the requested connectors")
    if sum(item["primary"] for item in logical) != 1:
        raise ValueError("exactly one primary output is required")
    if state["properties"].get("layout-mode") not in (1, 2):
        raise ValueError("the compositor did not provide a restorable layout mode")
    return monitors


def monitor_properties(monitor):
    properties = monitor["properties"]
    result = {name: properties[name] for name in MONITOR_PROPERTIES if name in properties}
    if "is-underscanning" in properties:
        result["underscanning"] = properties["is-underscanning"]
    return result


def current_plan(state, connectors):
    monitors = inventory(state, connectors)
    plan = []
    rectangles = []
    for logical in state["logical_monitors"]:
        spec = logical["monitors"][0]
        monitor = monitors[spec[0]]
        if spec != monitor["spec"]:
            raise ValueError("logical and physical monitor identities differ")
        modes = [mode for mode in monitor["modes"] if mode["properties"].get("is-current")]
        if len(modes) != 1:
            raise ValueError("each active output must have exactly one current mode")
        scale, transform = logical["scale"], logical["transform"]
        if not math.isfinite(scale) or scale <= 0 or transform not in range(8):
            raise ValueError("invalid logical output scale or transform")
        width, height = modes[0]["width"], modes[0]["height"]
        if transform % 2:
            width, height = height, width
        if state["properties"]["layout-mode"] == 1:
            width, height = width / scale, height / scale
        rectangles.append((logical["x"], logical["y"], width, height))
        plan.append({
            "connector": spec[0], "spec": spec, "mode": modes[0]["id"],
            "refresh_actual_hz": modes[0]["refresh_hz"],
            "x": logical["x"], "y": logical["y"], "scale": logical["scale"],
            "transform": logical["transform"], "primary": logical["primary"],
            "properties": monitor_properties(monitor),
        })
    a, b = rectangles
    if a[0] < b[0] + b[2] and b[0] < a[0] + a[2] and a[1] < b[1] + b[3] and b[1] < a[1] + a[3]:
        raise ValueError("logical output rectangles overlap")
    return {"layout_mode": state["properties"]["layout-mode"], "outputs": plan}


def measurement_plan(state, connectors, refresh_hz, primary):
    monitors = inventory(state, connectors)
    if primary not in connectors:
        raise ValueError("the primary output must be one of the requested connectors")
    outputs = []
    for index, connector in enumerate(connectors):
        monitor = monitors[connector]
        candidates = [mode for mode in monitor["modes"]
                      if mode["width"] == 1920 and mode["height"] == 1080
                      and math.isclose(mode["refresh_hz"], ACTUAL_HZ[refresh_hz], abs_tol=0.0001)
                      and mode["properties"].get("refresh-rate-mode", "fixed") == "fixed"
                      and "+vrr" not in mode["id"] and 1.0 in mode["scales"]]
        if len(candidates) != 1:
            raise ValueError(f"{connector} must advertise one fixed 1920x1080 mode at {ACTUAL_HZ[refresh_hz]:.9f} Hz and scale 1")
        mode = candidates[0]
        outputs.append({
            "connector": connector, "spec": monitor["spec"], "mode": mode["id"],
            "refresh_actual_hz": mode["refresh_hz"], "x": index * 1920, "y": 0,
            "scale": 1.0, "transform": 0, "primary": connector == primary,
            "properties": monitor_properties(monitor),
        })
    return {"layout_mode": state["properties"]["layout-mode"], "outputs": outputs}


def same_plan(actual, expected):
    def ordered(plan):
        return {"layout_mode": plan["layout_mode"],
                "outputs": sorted(plan["outputs"], key=lambda item: item["connector"])}
    return ordered(actual) == ordered(expected)


class DisplayConfig:
    def __init__(self):
        from gi.repository import Gio, GLib
        self.Gio, self.GLib = Gio, GLib
        self.proxy = Gio.DBusProxy.new_for_bus_sync(
            Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, None,
            BUS_NAME, OBJECT_PATH, BUS_NAME, None)
        self.monitor_changes = []
        self.proxy.connect("g-signal", self.on_signal)

    def on_signal(self, _proxy, _sender, name, _parameters):
        if name == "MonitorsChanged":
            self.monitor_changes.append({"signal": name, "received_monotonic_ns": time.monotonic_ns()})

    def observed_changes(self):
        context = self.GLib.MainContext.default()
        dispatched = 0
        while context.pending():
            context.iteration(False)
            dispatched += 1
            if dispatched > 1024:
                raise RuntimeError("display event queue did not stabilize")
        return list(self.monitor_changes)

    def begin_observation(self):
        self.observed_changes()
        self.monitor_changes.clear()

    def read(self):
        return snapshot(self.proxy.call_sync(
            "GetCurrentState", None, self.Gio.DBusCallFlags.NONE, 5000, None).unpack())

    def apply(self, plan, connectors):
        for attempt in range(3):
            state = self.read()
            monitors = physical_inventory(state, connectors)
            for output in plan["outputs"]:
                monitor = monitors[output["connector"]]
                if monitor["spec"] != output["spec"]:
                    raise ValueError("physical output identity changed; refusing to configure a replacement")
                if output["mode"] not in {mode["id"] for mode in monitor["modes"]}:
                    raise ValueError("a requested mode is no longer available")
            logical = []
            for output in plan["outputs"]:
                props = {key: self.GLib.Variant(MONITOR_PROPERTIES.get(key, "b"), value)
                         for key, value in output["properties"].items()}
                logical.append((output["x"], output["y"], output["scale"],
                                output["transform"], output["primary"],
                                [(output["connector"], output["mode"], props)]))
            parameters = lambda method: self.GLib.Variant(
                "(uua(iiduba(ssa{sv}))a{sv})",
                (state["serial"], method, logical,
                 {"layout-mode": self.GLib.Variant("u", plan["layout_mode"])}))
            try:
                self.proxy.call_sync("ApplyMonitorsConfig", parameters(0),
                                     self.Gio.DBusCallFlags.NONE, 5000, None)
                self.proxy.call_sync("ApplyMonitorsConfig", parameters(1),
                                     self.Gio.DBusCallFlags.NONE, 5000, None)
                return
            except self.GLib.Error as error:
                if attempt == 2 or self.read()["serial"] == state["serial"]:
                    raise
        raise RuntimeError("display configuration serial did not stabilize")


def wait_for_plan(display, plan, connectors, timeout=10):
    deadline = time.monotonic() + timeout
    while True:
        state = display.read()
        if same_plan(current_plan(state, connectors), plan):
            return state
        if time.monotonic() >= deadline:
            raise RuntimeError("the compositor did not confirm the requested display condition")
        time.sleep(0.1)


def receipt(directory, name, value):
    payload = {"schema_version": 1, "recorded_at_unix_ns": time.time_ns(),
               "monotonic_ns": time.monotonic_ns(), **value}
    path = directory / name
    with path.open("x", encoding="utf-8") as stream:
        os.chmod(path, 0o600)
        json.dump(payload, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def verify_condition(display, directory, plan, connectors, serial):
    state = display.read()
    events = display.observed_changes()
    if events or state["serial"] != serial or not same_plan(current_plan(state, connectors), plan):
        receipt(directory, "condition-changed.json", {"state": state, "monitor_change_events": events,
                "expected_serial": serial})
        raise RuntimeError("display condition changed during measurement")
    return state


def terminate_child(child):
    if child is None:
        return
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=5)


def supervise(display, directory, connectors, refresh_hz, primary, command, timeout):
    interrupted = []
    previous_handlers = {}
    child = None
    changed = False
    baseline = None
    failure = None
    exit_code = 1
    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        previous_handlers[signum] = signal.signal(signum, lambda number, _frame: interrupted.append(number))
    try:
        before = display.read()
        baseline = current_plan(before, connectors)
        plan = measurement_plan(before, connectors, refresh_hz, primary)
        receipt(directory, "before.json", {"state": before, "restore_plan": baseline,
                "requested_plan": plan, "refresh_nominal_hz": refresh_hz,
                "supervisor_pid": os.getpid(), "child_executable": Path(command[0]).name})
        if interrupted:
            raise InterruptedError("interrupted before applying the display condition")
        changed = True
        display.apply(plan, connectors)
        applied = wait_for_plan(display, plan, connectors)
        display.begin_observation()
        receipt(directory, "applied.json", {"state": applied, "verified_plan": plan,
                "monitor_change_events": [], "serial_guard": applied["serial"]})
        if interrupted:
            raise InterruptedError("interrupted before launching the measured command")
        child = subprocess.Popen(command, start_new_session=True)
        receipt(directory, "child.json", {"pid": child.pid, "process_group": child.pid})
        deadline = None if timeout is None else time.monotonic() + timeout
        while child.poll() is None:
            if interrupted:
                raise InterruptedError("measurement interrupted")
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError("measured command exceeded its deadline")
            verify_condition(display, directory, plan, connectors, applied["serial"])
            time.sleep(0.25)
        current = verify_condition(display, directory, plan, connectors, applied["serial"])
        receipt(directory, "condition-complete.json", {"state": current, "verified_plan": plan,
                "monitor_change_events": [], "serial_guard": applied["serial"]})
        exit_code = child.returncode if child.returncode >= 0 else 128 - child.returncode
    except Exception as error:
        failure = str(error)
        exit_code = 128 + interrupted[0] if interrupted else 1
    finally:
        try:
            terminate_child(child)
        except Exception as error:
            failure = f"{failure or ''}; child cleanup failed: {error}".strip("; ")
            exit_code = 1
        if changed and baseline is not None:
            try:
                display.apply(baseline, connectors)
                restored = wait_for_plan(display, baseline, connectors)
                receipt(directory, "restored.json", {"status": "RESTORED", "state": restored,
                        "verified_plan": baseline})
            except Exception as error:
                failure = f"{failure or ''}; display restoration failed: {error}".strip("; ")
                exit_code = 1
                try:
                    receipt(directory, "restore-failed.json", {"status": "RESTORE_FAILED", "error": str(error),
                            "restore_plan": baseline})
                except Exception as receipt_error:
                    print(f"restoration failure receipt could not be written: {receipt_error}", file=sys.stderr)
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
    if interrupted and exit_code == 0:
        exit_code = 128 + interrupted[0]
    receipt(directory, "result.json", {"status": "COMPLETED" if exit_code == 0 else "FAILED",
            "exit_code": exit_code, "error": failure, "signals": interrupted})
    return exit_code


def main():
    parser = argparse.ArgumentParser(description="Inspect or temporarily supervise a paired GNOME M1 display condition.")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--inspect", action="store_true", help="Read the current state and proposed condition (default).")
    mode.add_argument("--run", action="store_true", help="Apply a temporary condition, run the command, then restore.")
    parser.add_argument("--outputs", nargs=2, default=["DP-3", "DP-4"], metavar=("LEFT", "RIGHT"))
    parser.add_argument("--primary", default="DP-3")
    parser.add_argument("--refresh-hz", type=int, choices=(60, 120), default=60)
    parser.add_argument("--output", type=Path, help="New directory for private JSON receipts; required with --run.")
    parser.add_argument("--timeout-seconds", type=float)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if len(set(args.outputs)) != 2:
        parser.error("--outputs requires two different connector names")
    if args.timeout_seconds is not None and (not math.isfinite(args.timeout_seconds) or args.timeout_seconds <= 0):
        parser.error("--timeout-seconds must be finite and positive")
    if args.run and (not command or args.output is None):
        parser.error("--run requires --output and a command after --")
    if not args.run and (command or args.output is not None):
        parser.error("--inspect does not launch commands or create receipt directories")
    display = DisplayConfig()
    if not args.run:
        state = display.read()
        result = {"state": state, "restore_plan": current_plan(state, args.outputs),
                  "requested_plan": measurement_plan(state, args.outputs, args.refresh_hz, args.primary),
                  "refresh_nominal_hz": args.refresh_hz, "display_changed": False}
        print(json.dumps(result, indent=2))
        return 0
    args.output.mkdir(mode=0o700, parents=False, exist_ok=False)
    return supervise(display, args.output, args.outputs, args.refresh_hz, args.primary,
                     command, args.timeout_seconds)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"display session failed: {error}", file=sys.stderr)
        sys.exit(1)
