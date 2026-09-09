#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import tomllib


EXPECTED_TARGETS = {
    "linux-aarch64-ci",
    "linux-m1-analysis",
    "linux-distro-compositor-gpu",
    "linux-native-security",
}
EXPECTED_SECURITY = {f"SEC-{index:02d}" for index in range(1, 9)}
NATIVE_DEFERRED = {"SEC-02", "SEC-04", "SEC-05"}
EXPECTED_DISTRIBUTIONS = {"Ubuntu", "Debian", "Fedora", "Arch", "openSUSE"}
EXPECTED_COMPOSITORS = {"GNOME/Mutter", "KDE/KWin", "wlroots"}
EXPECTED_GPU_FAMILIES = {"NVIDIA proprietary", "Intel or AMD Mesa"}
EXPECTED_RESTRICTIONS = {"SELinux/AppArmor", "user namespaces", "FUSE", "loader and libraries"}


def load(path):
    with path.open("rb") as stream:
        return tomllib.load(stream)


def verify(contract):
    if contract.get("schema_version") != 1:
        raise ValueError("unsupported Linux qualification contract")
    if contract.get("mode") != "internal_code_and_tests":
        raise ValueError("qualification contract must use the internal code-and-tests mode")
    if contract.get("public_release") != "NOT_QUALIFIED":
        raise ValueError("public release must remain NOT_QUALIFIED")
    if contract.get("native_release_evidence_required") is not True:
        raise ValueError("native release evidence must remain required")
    targets = {entry["id"]: entry for entry in contract.get("targets", [])}
    if set(targets) != EXPECTED_TARGETS:
        raise ValueError("qualification target matrix is incomplete or contains an unexpected target")
    for target in targets.values():
        if target.get("native_release_qualified") is not False:
            raise ValueError(f"target {target['id']} must not be promoted to native release qualification")
        if not target.get("status") or not target.get("evidence_kind"):
            raise ValueError(f"target {target['id']} has incomplete evidence metadata")
    matrix = targets["linux-distro-compositor-gpu"]
    if set(matrix.get("distributions", [])) != EXPECTED_DISTRIBUTIONS:
        raise ValueError("distribution matrix is incomplete")
    if set(matrix.get("compositors", [])) != EXPECTED_COMPOSITORS:
        raise ValueError("compositor matrix is incomplete")
    if set(matrix.get("gpu_families", [])) != EXPECTED_GPU_FAMILIES:
        raise ValueError("GPU matrix is incomplete")
    if set(matrix.get("restrictions", [])) != EXPECTED_RESTRICTIONS:
        raise ValueError("restriction matrix is incomplete")
    security = {entry["id"]: entry for entry in contract.get("security", [])}
    if set(security) != EXPECTED_SECURITY:
        raise ValueError("security contract must enumerate SEC-01 through SEC-08 exactly once")
    for identifier, entry in security.items():
        if not entry.get("proof") or not entry.get("status"):
            raise ValueError(f"security case {identifier} has incomplete proof metadata")
        if identifier in NATIVE_DEFERRED:
            if entry["proof"] != "native" or entry["status"] != "NATIVE_DEFERRED":
                raise ValueError(f"native security case {identifier} was promoted without native evidence")
        elif entry["proof"] != "automated" or entry["status"] != "AUTOMATED_TEST":
            raise ValueError(f"automated security case {identifier} has an invalid proof status")
    if not contract.get("non_inference"):
        raise ValueError("qualification contract must require non-inference")
    return {
        "status": "CONTRACT_VALID",
        "public_release": contract["public_release"],
        "targets": len(targets),
        "security_cases": len(security),
        "native_security_deferred": sorted(NATIVE_DEFERRED),
    }


def main():
    parser = argparse.ArgumentParser(description="Validate the internal Linux qualification contract")
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = verify(load(args.contract))
    encoded = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded)
    print(encoded, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}))
        raise SystemExit(1) from None
