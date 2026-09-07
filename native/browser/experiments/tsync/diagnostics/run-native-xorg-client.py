#!/usr/bin/env python3

import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time


def helpers():
    spec = importlib.util.spec_from_file_location("native_xorg_runner", Path(__file__).with_name("run-native-xorg.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def process(pid):
    root = Path(f"/proc/{pid}")
    fields = root.joinpath("stat").read_text().rsplit(")", 1)[1].split()
    status = dict(line.split(":", 1) for line in root.joinpath("status").read_text().splitlines() if ":" in line)
    return {"pid": pid, "parent_pid": int(fields[1]), "start_ticks": int(fields[19]),
            "exe": str(root.joinpath("exe").resolve(strict=True)), "uids": [int(value) for value in status["Uid"].split()],
            "cgroup": root.joinpath("cgroup").read_text()}


def server_identity(preflight, utility):
    display = os.environ.get("DISPLAY")
    if display != preflight["display"] or not re.fullmatch(r":[0-9]+", display or ""):
        raise RuntimeError("the diagnostic client did not inherit the requested X display")
    number = display[1:]
    pid = int(Path(f"/tmp/.X{number}-lock").read_text().strip())
    server = process(pid)
    parent = process(os.getppid())
    argv = Path(f"/proc/{pid}/cmdline").read_bytes().decode().split("\0")
    expected = preflight["server"]
    session_scope = preflight["session"]["Scope"]
    same_scope = any(session_scope in line.split(":", 2)[-1].split("/") for line in server["cgroup"].splitlines())
    if (server["exe"] != expected["path"] or utility.digest(Path(server["exe"])) != expected["sha256"]
            or server["parent_pid"] != parent["pid"] or parent["exe"] != str(Path(shutil.which("xinit")).resolve())
            or server["uids"] != [os.getuid()] * 4 or display not in argv
            or f"vt{preflight['session']['VTNr']}" not in argv or "-keeptty" not in argv
            or not same_scope):
        raise RuntimeError("the display is not the private, unprivileged Xorg child of this xinit and PAM session")
    current = utility.session()
    if current != preflight["session"]:
        raise RuntimeError("the active login session changed before native diagnostics")
    return {"server": server, "xinit": parent, "client_pid": os.getpid(), "session": current,
            "server_sha256": expected["sha256"], "server_topology": "native_Xorg_on_active_TTY"}


def gpu_nodes():
    result = []
    for node in sorted(Path("/sys/class/drm").glob("card[0-9]*")):
        if not re.fullmatch(r"card[0-9]+", node.name):
            continue
        device = node / "device"
        fields = {"card": node.name, "pci_device_path": str(device.resolve()), "driver": str((device / "driver").resolve())}
        for name in ("vendor", "device", "boot_vga"):
            try:
                fields[name] = (device / name).read_text().strip()
            except OSError:
                fields[name] = None
        result.append(fields)
    return result


def main():
    utility = helpers()
    receipt = Path(os.environ["PANEFLOW_NATIVE_XORG_DIAGNOSTIC"])
    directory = receipt.parent
    preflight = json.loads(receipt.read_text())
    commands = []
    errors = []
    interrupted = []
    current_child = None

    def stop(number, _frame):
        interrupted.append(number)
        if current_child is not None:
            current_child.terminate()

    def query(name, command, timeout=30, environment=None):
        nonlocal current_child
        if interrupted:
            raise InterruptedError("diagnostic interrupted")
        started = time.monotonic_ns()
        with (directory / f"{name}.stdout").open("xb") as stdout, (directory / f"{name}.stderr").open("xb") as stderr:
            current_child = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment)
            try:
                code = current_child.wait(timeout=timeout)
                timed_out = False
            except subprocess.TimeoutExpired:
                current_child.kill()
                code = current_child.wait()
                timed_out = True
            finally:
                current_child = None
        commands.append({"name": name, "argv": command, "started_ns": started,
                         "completed_ns": time.monotonic_ns(), "exit_code": code, "timed_out": timed_out})
        if code or timed_out:
            errors.append(f"{name} failed or timed out; inspect its raw output")

    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, stop)
    try:
        identity = server_identity(preflight, utility)
        utility.record(directory / "native-session.json", {**identity, "gpu_nodes": gpu_nodes(), "qualification": "NOT_EVALUATED"})
        utility.record(directory / "query-environment.json", {"display": os.environ["DISPLAY"],
                       "session_type_environment_only": os.environ.get("XDG_SESSION_TYPE"),
                       "vulkan_wayland_connection": "disabled with a nonexistent private endpoint"})
        query("xorg-version", [preflight["server"]["path"], "-version"])
        query("xrandr-before", ["xrandr", "--query", "--verbose"])
        query("xrandr-providers", ["xrandr", "--listproviders"])
        query("xrandr-monitors", ["xrandr", "--listmonitors"])
        query("xdpyinfo", ["xdpyinfo", "-queryExtensions"])
        query("glxinfo", ["glxinfo", "-B"])
        query("lspci", ["lspci", "-nnk"])
        if shutil.which("nvidia-smi"):
            query("nvidia-smi", ["nvidia-smi", "-q"])
        source = Path(__file__).parent
        query("x11-present-capabilities", [sys.executable, str(source / "x11-present-capabilities.py"), "--display", os.environ["DISPLAY"]])
        environment = dict(os.environ)
        environment["WAYLAND_DISPLAY"] = str(directory / "no-wayland-endpoint")
        query("vulkaninfo", ["vulkaninfo"], timeout=90, environment=environment)
        query("vulkan-present-timing", [sys.executable, str(source / "vulkan-present-timing.py"), "--input", str(directory / "vulkaninfo.stdout")])
        query("xrandr-after", ["xrandr", "--query", "--verbose"])
        final_identity = server_identity(preflight, utility)
        if final_identity != identity:
            raise RuntimeError("the native Xorg process or session changed during diagnostics")
        utility.record(directory / "native-session-complete.json", {**final_identity, "qualification": "NOT_EVALUATED"})
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        errors.append(str(error))
    finally:
        if current_child is not None:
            current_child.kill()
            current_child.wait()
        artifacts = []
        for path in sorted(directory.iterdir()):
            if path.is_file() and path.name not in ("Xorg.log", "startx.log", "diagnostic.json"):
                artifacts.append({"path": path.name, "sha256": utility.digest(path), "bytes": path.stat().st_size})
        utility.record(directory / "diagnostic.json", {"status": "INTERRUPTED" if interrupted else "FAILED" if errors else "CAPABILITIES_COLLECTED",
                       "qualification": "NOT_EVALUATED", "presentations_observed": 0, "errors": errors,
                       "signals": interrupted, "commands": commands, "artifacts": artifacts,
                       "server_shutdown": "delegated to xinit/startx after this client exits",
                       "logs_finalized_by_startx": ["Xorg.log", "startx.log"]})
    return 128 + interrupted[0] if interrupted else int(bool(errors))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"native Xorg diagnostic client failed: {error}", file=sys.stderr)
        sys.exit(1)
