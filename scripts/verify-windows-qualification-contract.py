#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import tomllib


TARGET = "x86_64-pc-windows-msvc"
EXPECTED_TARGETS = {
    "first-smoke",
    "minimum-product",
    "current-reference",
    "gpu-displays",
    "performance",
    "distribution",
    "agent",
}
EXPECTED_SECURITY = {f"SEC-{index:02d}" for index in range(1, 13)}
HUMAN_SECURITY = {f"SEC-{index:02d}" for index in range(1, 9)}
EXPECTED_BUDGETS = {f"NFR-{index:02d}" for index in (1, 2, 3, 4, 5, 6, 7, 8, 11, 13)}
EXPECTED_DOCUMENTS = {"release-report", "usage-guide", "agent-tools"}
TARGET_STATUS = {"NOT_EXECUTED", "WAIVED_BY_OWNER", "WORKS_NOT_MEASURED", "WORKS_MEASURED", "FAILED"}
WAIVER_FIELDS = ("waived_by", "waived_at", "waiver_reason", "code_evidence", "residual_risk")
SECURITY_STATUS = {"AUTOMATED_TEST", "NATIVE_DEFERRED", "NATIVE_PROVEN"}
BUDGET_STATUS = {"NOT_MEASURED", "MEASURED"}
RELEASES = {"human", "agent"}
AVAILABILITY_CEILING = {
    ("NOT_QUALIFIED", "NOT_QUALIFIED"): {"absent", "development"},
    ("QUALIFIED", "NOT_QUALIFIED"): {"absent", "development", "human_qualified"},
    ("QUALIFIED", "QUALIFIED"): {"absent", "development", "human_qualified", "agent_qualified"},
}


def load(path):
    with path.open("rb") as stream:
        return tomllib.load(stream)


def verify_header(contract):
    if contract.get("schema_version") != 1:
        raise ValueError("unsupported Windows qualification contract")
    if contract.get("mode") != "internal_code_and_tests":
        raise ValueError("qualification contract must use the internal code-and-tests mode")
    if contract.get("target") != TARGET:
        raise ValueError(f"qualification contract must describe {TARGET}")
    for verdict in ("human_release", "agent_release"):
        if contract.get(verdict) not in {"NOT_QUALIFIED", "QUALIFIED"}:
            raise ValueError(f"{verdict} must be NOT_QUALIFIED or QUALIFIED")
    if contract.get("native_release_evidence_required") is not True:
        raise ValueError("native release evidence must remain required")
    if not contract.get("non_inference"):
        raise ValueError("qualification contract must require non-inference")
    if contract.get("foreign_platform_evidence_accepted") is not False:
        raise ValueError("no Linux or macOS result may condition this Windows verdict")


def verify_targets(contract):
    targets = {entry["id"]: entry for entry in contract.get("targets", [])}
    if set(targets) != EXPECTED_TARGETS:
        raise ValueError("S1-W matrix is incomplete or contains an unexpected row")
    for identifier, entry in sorted(targets.items()):
        if entry.get("release") not in RELEASES:
            raise ValueError(f"matrix row {identifier} has no valid release")
        if entry.get("status") not in TARGET_STATUS:
            raise ValueError(f"matrix row {identifier} has an invalid status")
        if not entry.get("owner") or not entry.get("evidence_kind") or not entry.get("requires"):
            raise ValueError(f"matrix row {identifier} has incomplete evidence metadata")
        if entry["status"] == "NOT_EXECUTED":
            if entry.get("evidence"):
                raise ValueError(f"matrix row {identifier} carries evidence while declared NOT_EXECUTED")
            if not entry.get("missing"):
                raise ValueError(f"matrix row {identifier} must name what is missing")
            if entry.get("covered") and not entry.get("observed_on"):
                raise ValueError(f"matrix row {identifier} claims partial coverage without a machine identity")
        elif entry["status"] == "WAIVED_BY_OWNER":
            if entry.get("evidence") or entry.get("observed_on"):
                raise ValueError(f"matrix row {identifier} presents native evidence while declared waived")
            missing_fields = [field for field in WAIVER_FIELDS if not entry.get(field)]
            if missing_fields:
                raise ValueError(
                    f"matrix row {identifier} is waived without {', '.join(missing_fields)}"
                )
        elif not entry.get("evidence") or not entry.get("observed_on"):
            raise ValueError(f"matrix row {identifier} claims execution without evidence and a machine identity")
        if entry.get("native_release_qualified") is not False and entry["status"] != "WORKS_MEASURED":
            raise ValueError(f"matrix row {identifier} was promoted without a measured native result")
    return targets


def verify_security(contract):
    security = {entry["id"]: entry for entry in contract.get("security", [])}
    if set(security) != EXPECTED_SECURITY:
        raise ValueError("security contract must enumerate SEC-01 through SEC-12 exactly once")
    for identifier, entry in sorted(security.items()):
        expected_release = "human" if identifier in HUMAN_SECURITY else "agent"
        if entry.get("release") != expected_release:
            raise ValueError(f"security case {identifier} is attached to the wrong release")
        if entry.get("proof") not in {"automated", "native"}:
            raise ValueError(f"security case {identifier} has no valid proof kind")
        if entry.get("status") not in SECURITY_STATUS:
            raise ValueError(f"security case {identifier} has an invalid proof status")
        if entry["proof"] == "automated" and entry["status"] != "AUTOMATED_TEST":
            raise ValueError(f"automated security case {identifier} has an invalid proof status")
        if entry["status"] == "NATIVE_PROVEN" and not entry.get("evidence"):
            raise ValueError(f"native security case {identifier} was promoted without evidence")
    return security


def verify_budgets(contract):
    budgets = {entry["id"]: entry for entry in contract.get("budgets", [])}
    if set(budgets) != EXPECTED_BUDGETS:
        raise ValueError("budget contract must enumerate the required NFR rows exactly once")
    for identifier, entry in sorted(budgets.items()):
        if entry.get("status") not in BUDGET_STATUS:
            raise ValueError(f"budget {identifier} has an invalid status")
        if not entry.get("owner") or not entry.get("subject"):
            raise ValueError(f"budget {identifier} has incomplete metadata")
        if entry["status"] == "MEASURED" and not entry.get("evidence"):
            raise ValueError(f"budget {identifier} claims a measurement without evidence")
    return budgets


def verify_documents(contract, root):
    documents = {entry["id"]: entry for entry in contract.get("documents", [])}
    if set(documents) != EXPECTED_DOCUMENTS:
        raise ValueError("document contract is incomplete or contains an unexpected entry")
    for identifier, entry in sorted(documents.items()):
        if not entry.get("covers"):
            raise ValueError(f"document {identifier} does not say what it covers")
        path = root / entry.get("path", "")
        if not path.is_file() or not path.read_text(encoding="utf-8").strip():
            raise ValueError(f"document {identifier} does not resolve to a written file")
    return documents


def verify_verdicts(contract, targets, security, budgets):
    for release in sorted(RELEASES):
        rows = [entry for entry in targets.values() if entry["release"] == release]
        cases = [entry for entry in security.values() if entry["release"] == release]
        limits = [entry for entry in budgets.values() if entry["release"] == release]
        proven = (
            all(entry["status"] in {"WORKS_MEASURED", "WAIVED_BY_OWNER"} for entry in rows)
            and all(entry["status"] in {"AUTOMATED_TEST", "NATIVE_PROVEN"} for entry in cases)
            and all(entry["status"] == "MEASURED" for entry in limits)
        )
        declared = contract[f"{release}_release"]
        if declared == "QUALIFIED" and not proven:
            raise ValueError(f"{release} release was declared QUALIFIED while its matrix is not fully proven")


def verify_manifest(contract, root):
    manifest = load(root / contract["distribution_manifest"])
    entry = manifest.get("targets", {}).get(TARGET)
    if entry is None:
        raise ValueError(f"the distribution manifest does not declare {TARGET}")
    availability = entry.get("availability")
    verdicts = (contract["human_release"], contract["agent_release"])
    if verdicts not in AVAILABILITY_CEILING:
        raise ValueError("the agent release cannot be QUALIFIED while the human release is not")
    allowed = AVAILABILITY_CEILING[verdicts]
    if availability not in allowed:
        raise ValueError(
            f"the distribution manifest declares availability {availability} while the Windows verdict allows {sorted(allowed)}"
        )
    return availability


def verify(contract, root):
    verify_header(contract)
    targets = verify_targets(contract)
    security = verify_security(contract)
    budgets = verify_budgets(contract)
    documents = verify_documents(contract, root)
    verify_verdicts(contract, targets, security, budgets)
    availability = verify_manifest(contract, root)
    return {
        "status": "CONTRACT_VALID",
        "target": TARGET,
        "human_release": contract["human_release"],
        "agent_release": contract["agent_release"],
        "declared_availability": availability,
        "matrix_rows": len(targets),
        "matrix_not_executed": sorted(
            identifier for identifier, entry in targets.items() if entry["status"] == "NOT_EXECUTED"
        ),
        "matrix_waived_by_owner": sorted(
            identifier
            for identifier, entry in targets.items()
            if entry["status"] == "WAIVED_BY_OWNER"
        ),
        "security_cases": len(security),
        "security_native_deferred": sorted(
            identifier for identifier, entry in security.items() if entry["status"] == "NATIVE_DEFERRED"
        ),
        "budgets_not_measured": sorted(
            identifier for identifier, entry in budgets.items() if entry["status"] == "NOT_MEASURED"
        ),
        "documents": sorted(documents),
    }


def main():
    parser = argparse.ArgumentParser(description="Validate the Windows browser qualification contract")
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    contract_path = args.contract.resolve()
    root = args.root.resolve() if args.root else contract_path.parents[2]
    result = verify(load(contract_path), root)
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
