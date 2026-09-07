#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def inspect(raw):
    lines = raw.decode("utf-8", errors="replace").splitlines()
    context = None
    surfaces = []
    for index, line in enumerate(lines):
        if line.startswith("GPU id :"):
            context = line.strip()
        if line.strip() != "VkPresentTimingSurfaceCapabilitiesEXT:":
            continue
        if context is None:
            raise RuntimeError("Present-timing capabilities have no GPU/surface heading")
        properties = {}
        stages = []
        block = []
        for following in lines[index + 1:]:
            stripped = following.strip()
            if not stripped:
                break
            block.append(following)
            if stripped.startswith("presentStageQueries: count = "):
                properties["presentStageCount"] = int(stripped.rsplit(" = ", 1)[1])
            elif " = " in stripped:
                key, value = stripped.split(" = ", 1)
                properties[key] = {"true": True, "false": False}.get(value, value)
            elif stripped.startswith("PRESENT_STAGE_"):
                stages.append(stripped)
        surfaces.append({
            "gpu_and_surface": context,
            "properties": properties,
            "present_stages": stages,
            "first_pixel_out_advertised": "PRESENT_STAGE_IMAGE_FIRST_PIXEL_OUT_BIT_EXT" in stages,
            "raw_capability_lines": block,
        })
    if not surfaces:
        raise RuntimeError("No per-surface present-timing capability blocks; use a current vulkaninfo without --summary.")
    return {
        "schema_version": 1,
        "qualification": "NOT_EVALUATED",
        "vulkaninfo_sha256": hashlib.sha256(raw).hexdigest(),
        "surfaces": surfaces,
        "presentations_observed": 0,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Report Vulkan present timing per available WSI surface.")
    parser.add_argument("--input", type=Path, help="Read an archived full vulkaninfo output instead of querying.")
    args = parser.parse_args()
    try:
        if args.input:
            raw = args.input.read_bytes()
        else:
            result = subprocess.run(["vulkaninfo"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if result.stderr:
                sys.stderr.buffer.write(result.stderr)
            if result.returncode:
                raise RuntimeError(f"vulkaninfo exited with status {result.returncode}")
            raw = result.stdout
        report = inspect(raw)
        report["source"] = str(args.input) if args.input else "vulkaninfo"
        report["query_environment"] = None if args.input else {
            "display": os.environ.get("DISPLAY"),
            "wayland_display": os.environ.get("WAYLAND_DISPLAY"),
            "session_type_environment_only": os.environ.get("XDG_SESSION_TYPE"),
        }
        print(json.dumps(report, indent=2))
    except (OSError, RuntimeError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
