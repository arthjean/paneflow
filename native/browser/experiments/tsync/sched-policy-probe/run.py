#!/usr/bin/env python3

import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def cases():
    result = []

    def add(name, target="worker", caller="main", method="raw", policy=3,
            sandbox="gpu", invalid=False, expected="eperm"):
        result.append({"name": name, "target": target, "caller": caller,
                       "method": method, "scheduling_policy": policy,
                       "sandbox": sandbox, "invalid_pointer": invalid,
                       "expected": expected})

    add("pid_zero_allowed", target="zero", expected="success")
    add("own_pid_allowed", target="pid", expected="success")
    add("self_tid_before_tsync_allowed", target="self", caller="before", expected="success")
    add("self_tid_after_tsync_allowed", target="self", caller="after", expected="success")
    add("other_worker_denied")
    add("preexisting_caller_other_worker_denied", caller="before")
    add("new_caller_other_worker_denied", caller="after")
    add("external_process_denied", target="external")
    add("preexisting_caller_external_process_denied", target="external", caller="before")
    add("new_caller_external_process_denied", target="external", caller="after")
    add("pthread_other_worker_denied", method="pthread", expected="pthread_eperm")
    add("invalid_pointer_other_worker_denied", invalid=True)
    add("invalid_pointer_external_process_denied", target="external", invalid=True)
    add("invalid_pointer_self_reaches_kernel", target="self", invalid=True, expected="efault")
    add("other_policy_still_fatal", policy=0, expected="fatal")
    add("fifo_policy_still_fatal", policy=1, expected="fatal")
    add("batch_reset_on_fork_still_fatal", policy=3 | 0x40000000, expected="fatal")
    add("batch_high_scalar_bits_still_fatal", policy=3 | (1 << 32), expected="fatal")
    for method in ("get_scheduler", "get_param", "set_param", "get_affinity", "set_affinity"):
        add(f"other_{method}_still_fatal", method=method, expected="fatal")
    add("renderer_batch_still_fatal", sandbox="renderer", expected="fatal")
    add("model_batch_still_fatal", sandbox="model", expected="fatal")
    add("default_gpu_constructor_still_fatal", sandbox="gpu-default", expected="fatal")
    add("video_encoder_batch_still_fatal", sandbox="video-encoder", expected="fatal")
    return result


def inspect(case, returncode, stdout, stderr, external_before, external_after):
    errors = []
    events = []
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
            if isinstance(event, dict):
                events.append(event)
        except json.JSONDecodeError:
            pass
    started = [event for event in events if event.get("event") == "sandbox_started"]
    armed = [event for event in events if event.get("event") == "armed"]
    results = [event for event in events if event.get("event") == "result"]
    completed = [event for event in events if event.get("event") == "completed"]
    if (case["sandbox"] == "video-encoder" and returncode == 77
            and external_before == external_after
            and events == [{"event": "factory_unavailable", "factory": "kHardwareVideoEncoding",
                            "USE_LINUX_VIDEO_ACCELERATION": False}]):
        return {"status": "NOT_BUILT", "errors": [], "events": events}
    if len(started) != 1 or started[0].get("tsync") is not True or len(armed) != 1:
        errors.append("the requested syscall was not armed after Chromium TSYNC")
    if external_before != external_after:
        errors.append("the external process scheduler changed")
    if case["expected"] == "fatal":
        if returncode != -signal.SIGSEGV or results or completed:
            errors.append("the original fatal denial was not preserved")
        if armed and f"seccomp-bpf failure in syscall nr=0x{armed[0]['syscall_nr']:x} " not in stderr:
            errors.append("the fatal signal lacks the expected syscall's Chromium SIGSYS diagnostic")
    else:
        if returncode != 0 or len(results) != 1 or len(completed) != 1:
            errors.append("the nonfatal operation did not complete")
        if results:
            value = results[0]
            expected = case["expected"]
            valid = ((expected == "success" and value.get("result") == 0 and value.get("errno") == 0)
                     or (expected == "eperm" and value.get("result") == -1 and value.get("errno") == errno.EPERM)
                     or (expected == "efault" and value.get("result") == -1 and value.get("errno") == errno.EFAULT)
                     or (expected == "pthread_eperm" and value.get("result") == errno.EPERM))
            if not valid:
                errors.append("the operation returned the wrong result or errno")
        if completed:
            value = completed[0]
            if (value.get("target_scheduler_before") != os.SCHED_OTHER
                    or value.get("target_scheduler_after") != os.SCHED_OTHER
                    or value.get("target_priority_before") != 0
                    or value.get("target_priority_after") != 0
                    or value.get("target_no_new_privs") != 1):
                errors.append("the target worker changed or did not receive TSYNC")
    return {"status": "PASS" if not errors else "FAIL", "errors": errors, "events": events}


def scheduler():
    return {"pid": os.getpid(), "policy": os.sched_getscheduler(0),
            "priority": os.sched_getparam(0).sched_priority}


def interrupt(number, _frame):
    raise SystemExit(128 + number)


def main():
    parser = argparse.ArgumentParser(description="Run native scheduling cases against the actual patched Chromium policies.")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--run", action="store_true")
    args = parser.parse_args()
    for number in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, interrupt)
    matrix = cases()
    if not args.run:
        print(json.dumps({"status": "NOT_RUN", "cases": matrix}, indent=2))
        return 0
    if args.output is None:
        parser.error("--run requires a new --output directory")
    binary = args.binary.resolve(strict=True)
    initial = scheduler()
    if os.getuid() == 0 or initial["policy"] != os.SCHED_OTHER or initial["priority"] != 0:
        parser.error("run as an ordinary user with SCHED_OTHER and priority zero")
    os.umask(0o077)
    directory = args.output.absolute()
    directory.mkdir(mode=0o700, exist_ok=False)
    source = Path(__file__).resolve().parent
    paths = [binary, *sorted(source.glob("*.cc")), source / "probe.h",
             source / "BUILD.gn", source / "run.py",
             source.parent / "0008-deny-foreign-gpu-batch-scheduler.patch"]
    receipt = {"schema_version": 1, "status": "RUNNING", "started_ns": time.monotonic_ns(),
               "scope": "native Chromium scheduling policy with TSYNC; no namespace or GPU rendering qualification",
               "sources": [{"path": str(path), "sha256": digest(path)} for path in paths], "cases": []}
    active = None
    try:
        for case in matrix:
            command = [str(binary), f"--sandbox={case['sandbox']}", f"--target={case['target']}",
                       f"--caller={case['caller']}", f"--method={case['method']}",
                       f"--scheduling-policy={case['scheduling_policy']}"]
            if case["invalid_pointer"]:
                command.append("--invalid-pointer")
            stdout_path = directory / f"{case['name']}.stdout"
            stderr_path = directory / f"{case['name']}.stderr"
            before = scheduler()
            timed_out = False
            started = time.monotonic_ns()
            with stdout_path.open("xb") as stdout, stderr_path.open("xb") as stderr:
                active = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True)
                try:
                    code = active.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    timed_out = True
                    code = None
                finally:
                    try:
                        os.killpg(active.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    active.wait()
                    active = None
            after = scheduler()
            verdict = inspect(case, code, stdout_path.read_text(errors="replace"),
                              stderr_path.read_text(errors="replace"), before, after)
            if timed_out:
                verdict["status"] = "FAIL"
                verdict["errors"].append("case timed out; its private process group was terminated")
            item = {**case, **verdict, "argv": command, "returncode": code,
                    "started_ns": started, "completed_ns": time.monotonic_ns(),
                    "external_before": before, "external_after": after,
                    "artifacts": [{"path": path.name, "sha256": digest(path)} for path in (stdout_path, stderr_path)]}
            receipt["cases"].append(item)
            print(json.dumps({"case": case["name"], "status": verdict["status"], "errors": verdict["errors"]}), flush=True)
        statuses = {case["status"] for case in receipt["cases"]}
        receipt["status"] = ("PASS" if statuses == {"PASS"} else
                             "PASS_AVAILABLE_CASES" if statuses <= {"PASS", "NOT_BUILT"} else "FAIL")
    finally:
        if active is not None:
            try:
                os.killpg(active.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            active.wait()
        if receipt["status"] == "RUNNING":
            receipt["status"] = "INTERRUPTED"
        receipt["completed_ns"] = time.monotonic_ns()
        receipt["external_final"] = scheduler()
        if receipt["external_final"] != initial:
            receipt["status"] = "FAIL"
        (directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return int(receipt["status"] not in ("PASS", "PASS_AVAILABLE_CASES"))


if __name__ == "__main__":
    raise SystemExit(main())
