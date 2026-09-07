#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time
import uuid


SESSION_FIELDS = ("Id", "User", "Type", "Class", "Active", "Remote", "Seat", "VTNr", "TTY", "Scope")
REMOVED_ENVIRONMENT = (
    "DISPLAY", "WAYLAND_DISPLAY", "SESSION_MANAGER", "DBUS_SESSION_BUS_ADDRESS",
    "LD_PRELOAD", "LD_LIBRARY_PATH", "VK_DRIVER_FILES", "VK_ICD_FILENAMES",
    "VK_ADD_DRIVER_FILES", "VK_LAYER_PATH", "VK_INSTANCE_LAYERS",
    "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE", "LIBGL_ALWAYS_SOFTWARE",
    "DRI_PRIME", "__NV_PRIME_RENDER_OFFLOAD", "__GLX_VENDOR_LIBRARY_NAME",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
)


def record(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump({"schema_version": 1, "monotonic_ns": time.monotonic_ns(), **value}, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def session(identifier="self"):
    command = ["loginctl", "show-session", identifier]
    for field in SESSION_FIELDS:
        command.extend(["-p", field])
    result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=True)
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def active_tty_session():
    if os.getuid() == 0 or os.geteuid() != os.getuid():
        raise RuntimeError("run as the ordinary logged-in user without elevated privileges")
    if not os.isatty(0):
        raise RuntimeError("stdin must be the controlling real TTY, not a pipe or desktop terminal")
    tty = os.ttyname(0)
    match = re.fullmatch(r"/dev/tty([1-9][0-9]*)", tty)
    if not match:
        raise RuntimeError("a /dev/ttyN login is required; pseudo-terminals are not accepted")
    current = session()
    expected = {"User": str(os.getuid()), "Type": "tty", "Class": "user", "Active": "yes",
                "Remote": "no", "Seat": "seat0", "VTNr": match[1], "TTY": tty.removeprefix("/dev/")}
    if any(current.get(key) != value for key, value in expected.items()):
        raise RuntimeError("logind must report this user's active local PAM TTY session on seat0")
    if not current.get("Id") or not current.get("Scope", "").startswith("session-"):
        raise RuntimeError("the TTY session lacks a logind identity or session scope")
    return current


def free_display(number):
    if (Path(f"/tmp/.X{number}-lock").exists()
            or Path(f"/tmp/.X{number}-lock").is_symlink()
            or Path(f"/tmp/.X11-unix/X{number}").exists()
            or Path(f"/tmp/.X11-unix/X{number}").is_symlink()):
        return False
    names = {line.split()[-1].removeprefix("@") for line in Path("/proc/net/unix").read_text().splitlines()[1:] if line.split()}
    return f"/tmp/.X11-unix/X{number}" not in names


def other_graphical_sessions():
    result = subprocess.run(["loginctl", "list-sessions", "--no-legend"], capture_output=True, text=True, timeout=10, check=True)
    sessions = []
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1] == str(os.getuid()):
            candidate = session(fields[0])
            if candidate.get("Type") in ("wayland", "x11"):
                sessions.append(candidate)
    return sessions


def runtime_files(runtime):
    relative = [
        "usr/libexec/Xorg", "usr/lib64/xorg/modules/drivers/modesetting_drv.so",
        "usr/lib64/xorg/modules/drivers/nvidia_drv.so", "usr/lib64/xorg/modules/input/libinput_drv.so",
        "usr/lib64/xorg/modules/extensions/libglx.so", "usr/lib64/xorg/modules/extensions/libglxserver_nvidia.so",
    ]
    files = [runtime / "root" / name for name in relative]
    config = runtime / "root/usr/share/X11/xorg.conf.d"
    if not config.is_dir():
        raise RuntimeError("the extracted Xorg configuration directory is missing")
    files.extend(sorted(config.glob("*.conf")))
    result = []
    for path in files:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode):
            raise RuntimeError(f"runtime file is not regular: {path}")
        result.append({"path": str(path), "sha256": digest(path), "bytes": info.st_size})
    server = files[0]
    if not os.access(server, os.X_OK) or server.stat().st_mode & (stat.S_ISUID | stat.S_ISGID):
        raise RuntimeError("use the unprivileged libexec/Xorg binary, not Xorg.wrap")
    with server.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise RuntimeError("the private Xorg server must be an ELF executable")
    return result


def main():
    parser = argparse.ArgumentParser(description="Prepare or run a private native Xorg capability session from an active real TTY.")
    parser.add_argument("--runtime", type=Path, default=Path.home() / ".cache/paneflow-cef-tsync/xorg-runtime")
    parser.add_argument("--output", type=Path, help="New evidence directory, required with --run.")
    parser.add_argument("--display", type=int, help="Unused X display number; default selects the first free number from 2.")
    parser.add_argument("--run", action="store_true", help="Launch startx after all checks; default only prints the plan.")
    args = parser.parse_args()
    if args.run and args.output is None:
        parser.error("--run requires --output")
    if args.display is not None and not 0 <= args.display <= 999:
        parser.error("--display must be an integer between 0 and 999")
    current = active_tty_session()
    runtime = args.runtime.resolve(strict=True)
    client = Path(__file__).resolve().with_name("run-native-xorg-client.py")
    for name in ("startx", "xinit", "xauth", "mcookie", "python3", "xdpyinfo", "xrandr", "glxinfo", "vulkaninfo", "lspci"):
        if not shutil.which(name):
            raise RuntimeError(f"required diagnostic tool is missing: {name}")
    number = args.display
    if number is None:
        number = next((item for item in range(2, 1000) if free_display(item)), None)
    if number is None or not free_display(number):
        raise RuntimeError("the selected display has a socket or lock; no existing server will be reused or stopped")
    output = args.output.absolute() if args.output else None
    for path in (runtime, client, output, Path(sys.executable), Path.home()):
        if path and any(character.isspace() or character in "*?[]" for character in str(path)):
            raise RuntimeError("startx splits arguments: runtime, source, Python, home and output paths must not contain whitespace or glob characters")
    files = runtime_files(runtime)
    plan = {"qualification": "NOT_EVALUATED", "session": current,
            "other_graphical_sessions_untouched": other_graphical_sessions(), "display": f":{number}",
            "server": files[0], "runtime_files": files, "client": str(client),
            "source_files": [{"path": str(path), "sha256": digest(path)} for path in
                             (Path(__file__).resolve(), client, client.with_name("x11-present-capabilities.py"), client.with_name("vulkan-present-timing.py"))],
            "removed_environment_names": [name for name in REMOVED_ENVIRONMENT if name in os.environ],
            "display_modes_requested": None, "server_started": False}
    if not args.run:
        print(json.dumps(plan, indent=2))
        return 0
    os.umask(0o077)
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    auth_directory = runtime / "session-authority"
    auth_directory.mkdir(mode=0o700, exist_ok=True)
    if auth_directory.is_symlink() or auth_directory.stat().st_uid != os.getuid() or auth_directory.stat().st_mode & 0o077:
        raise RuntimeError("the runtime authority directory must be private and owned by this user")
    authority = auth_directory / uuid.uuid4().hex
    environment = {name: value for name, value in os.environ.items() if name not in REMOVED_ENVIRONMENT}
    environment.update({"XAUTHORITY": str(authority), "XDG_SESSION_TYPE": "x11",
                        "PANEFLOW_NATIVE_XORG_DIAGNOSTIC": str(output / "preflight.json")})
    command = [shutil.which("startx"), str(Path(sys.executable).resolve()), str(client), "--", files[0]["path"],
               f":{number}", f"vt{current['VTNr']}", "-keeptty", "-nolisten", "tcp",
               "-modulepath", str(runtime / "root/usr/lib64/xorg/modules"),
               "-configdir", str(runtime / "root/usr/share/X11/xorg.conf.d"),
               "-logfile", str(output / "Xorg.log")]
    record(output / "preflight.json", {**plan, "startx_argv": command})
    fresh = active_tty_session()
    if fresh != current or not free_display(number):
        raise RuntimeError("session or display availability changed before launch")
    with (output / "startx.log").open("xb") as stream:
        os.dup2(stream.fileno(), 1)
        os.dup2(stream.fileno(), 2)
        os.execvpe(command[0], command, environment)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"native Xorg diagnostic refused: {error}", file=sys.stderr)
        sys.exit(1)
