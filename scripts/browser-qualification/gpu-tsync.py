import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import signal
import subprocess
import sys
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def assess(events, returncode, tsync, trap):
    before = {e["tid"]: e for e in events if e.get("phase") == "before"}
    sealed = {e["tid"]: e for e in events if e.get("phase") == "sealed"}
    after = [e for e in events if e.get("phase") == "after_render"]
    installs = [e for e in events if e["event"] == "filter_install"]
    markers = [e for e in events if e["event"] == "marker"]
    renderers = [e for e in events if e["event"] == "renderer"]
    checks = {
        "expected_exit": returncode == (-signal.SIGSYS if trap else 0),
        "hardware_renderer": len(renderers) == 1 and not any(
            token in renderers[0]["renderer"].lower()
            for token in ("llvmpipe", "softpipe", "swrast", "software")
        ),
        "multiple_threads_before_filter": len(before) >= 2,
        "unfiltered_initial_state": bool(before) and all(e["seccomp"] == 0 for e in before.values()),
        "same_thread_set_at_install": bool(before) and before.keys() == sealed.keys(),
        "filter_installed": len(installs) == 1 and installs[0]["result"] == 0,
        "marker_denied_on_main": len(markers) == 1 and markers[0]["main_result"] == -1 and markers[0]["main_errno"] == 1,
        "render_after_filter": bool(after),
        "completion": any(e["event"] == ("trap_control" if trap else "completed") for e in events),
    }
    if tsync:
        checks["all_threads_filtered"] = bool(sealed) and all(
            e["seccomp"] == 2 and e["filters"] == 1 and e["no_new_privs"] == 1
            for e in [*sealed.values(), *after]
        )
        checks["marker_denied_on_existing_worker"] = len(markers) == 1 and markers[0]["worker_result"] == -1 and markers[0]["worker_errno"] == 1
    else:
        checks["control_other_threads_unfiltered"] = bool(sealed) and sum(e["seccomp"] == 2 for e in sealed.values()) == 1
        checks["control_worker_marker_allowed"] = len(markers) == 1 and markers[0]["worker_result"] == markers[0]["worker_before"] and markers[0]["worker_result"] > 0
    return checks


def main():
    if sys.platform != "linux":
        raise SystemExit("NOT_EXECUTED: the TSYNC/EGL probe requires Linux")
    import resource

    parser = argparse.ArgumentParser(description="Test kernel TSYNC with native EGL, without claiming Chromium sandbox qualification")
    parser.add_argument("--render-node", required=True)
    parser.add_argument("--egl-vendor", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    render_node = Path(args.render_node).resolve(strict=True)
    vendor = args.egl_vendor.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    source = Path(__file__).with_name("gpu-tsync-linux.c")
    source_copy = output / source.name
    source_copy.write_bytes(source.read_bytes())
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    env = os.environ.copy()
    env["__EGL_VENDOR_LIBRARY_FILENAMES"] = str(vendor)
    env["MESA_SHADER_CACHE_DISABLE"] = "true"
    receipt = {
        "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "scope": "native EGL kernel TSYNC probe; synthetic policies, no Chromium, broker, ANGLE or GPUI",
        "qualification": "NOT_EVALUATED",
        "kernel": platform.release(),
        "machine": platform.machine(),
        "render_node": str(render_node),
        "egl_vendor": str(vendor),
        "egl_vendor_sha256": digest(vendor),
        "source_sha256": digest(source_copy),
        "runner_sha256": digest(Path(__file__)),
        "environment": {k: env[k] for k in (
            "__EGL_VENDOR_LIBRARY_FILENAMES", "MESA_SHADER_CACHE_DISABLE", "DRI_PRIME",
            "MESA_LOADER_DRIVER_OVERRIDE", "LIBGL_ALWAYS_SOFTWARE", "GALLIUM_DRIVER"
        ) if k in env},
        "cases": [],
    }
    with tempfile.TemporaryDirectory(prefix="paneflow-gpu-tsync-") as temporary:
        binary = Path(temporary) / "gpu-tsync"
        command = [*shlex.split(os.environ.get("CC", "cc")), "-O2", "-Wall", "-Wextra", "-Werror", str(source_copy), "-o", str(binary), "-lEGL", "-lGLESv2", "-lseccomp", "-pthread"]
        build = subprocess.run(command, capture_output=True, text=True, timeout=60)
        (output / "build.stdout").write_text(build.stdout)
        (output / "build.stderr").write_text(build.stderr)
        receipt["build"] = {"command": command, "returncode": build.returncode}
        if build.returncode:
            (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
            raise SystemExit("Probe compilation failed; see build.stderr")
        receipt["binary_sha256"] = digest(binary)
        for name, tsync, bounded, trap in [
            ("control-no-tsync", 0, 0, False),
            ("tsync-marker", 1, 0, False),
            ("tsync-bounded", 1, 1, False),
            ("trap-control", 1, 1, True),
        ]:
            command = [str(binary), str(render_node), str(tsync), str(bounded)]
            if trap:
                command.append("trap-control")
            try:
                run = subprocess.run(command, capture_output=True, text=True, timeout=45, env=env)
                stdout, stderr, returncode = run.stdout, run.stderr, run.returncode
            except subprocess.TimeoutExpired as error:
                stdout = (error.stdout or b"").decode(errors="replace")
                stderr = (error.stderr or b"").decode(errors="replace") + "\nTIMEOUT after 45 seconds\n"
                returncode = None
            (output / f"{name}.jsonl").write_text(stdout)
            (output / f"{name}.stderr").write_text(stderr)
            events = [json.loads(line) for line in stdout.splitlines()]
            checks = assess(events, returncode, tsync, trap)
            case = {"name": name, "command": command, "returncode": returncode, "checks": checks, "passed": all(checks.values())}
            receipt["cases"].append(case)
            (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
            print(f"{name}: {'PASS' if case['passed'] else 'FAIL'} (exit {returncode})", flush=True)
    receipt["probe_passed"] = all(case["passed"] for case in receipt["cases"])
    (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return 0 if receipt["probe_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
