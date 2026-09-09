#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import struct
import tarfile
import tempfile
import tomllib
import urllib.request


def digest(path, algorithm):
    hasher = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def load_manifest(path):
    with path.open("rb") as stream:
        manifest = tomllib.load(stream)
    required = {
        "contract_version",
        "target",
        "cef_version",
        "chromium_version",
        "archive",
        "url",
        "archive_sha256",
        "archive_sha1",
        "archive_size",
        "source",
        "availability",
        "native_qualification",
        "evidence_kind",
        "ci_runner",
        "max_members",
        "max_unpacked_size",
        "max_file_size",
    }
    missing = sorted(required - manifest.keys())
    if missing:
        raise ValueError(f"runtime manifest is missing: {', '.join(missing)}")
    if manifest["contract_version"] != 1:
        raise ValueError("unsupported ARM64 runtime manifest contract")
    if manifest["target"] != "aarch64-unknown-linux-gnu":
        raise ValueError("ARM64 runtime target is not aarch64-unknown-linux-gnu")
    if manifest["availability"] != "development":
        raise ValueError("CI runtime must remain development-only")
    if manifest["native_qualification"] != "upstream_standard_not_qualified":
        raise ValueError("CI runtime must remain outside native qualification")
    if manifest["evidence_kind"] != "github_actions_arm64_code_test_build":
        raise ValueError("unexpected ARM64 evidence kind")
    if not manifest["url"].startswith("https://cef-builds.spotifycdn.com/"):
        raise ValueError("ARM64 runtime provenance must use the pinned CEF distribution host")
    if not manifest["archive"].endswith(".tar.bz2"):
        raise ValueError("ARM64 runtime archive must be a bzip2 tarball")
    if len(manifest["archive_sha256"]) != 64 or len(manifest["archive_sha1"]) != 40:
        raise ValueError("ARM64 runtime archive digests have invalid lengths")
    if manifest["archive_size"] <= 0:
        raise ValueError("ARM64 runtime archive size must be positive")
    return manifest


def download(url, destination, expected_size):
    temporary = destination.with_name(f".{destination.name}.part")
    try:
        with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as output:
            written = 0
            while chunk := response.read(1024 * 1024):
                written += len(chunk)
                if written > expected_size:
                    raise ValueError("download exceeds the pinned archive size")
                output.write(chunk)
        if written != expected_size:
            raise ValueError(f"download size {written} differs from pinned size {expected_size}")
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def verify_archive(path, manifest):
    if path.stat().st_size != manifest["archive_size"]:
        raise ValueError("ARM64 runtime archive size mismatch")
    sha256 = digest(path, "sha256")
    sha1 = digest(path, "sha1")
    if sha256 != manifest["archive_sha256"]:
        raise ValueError("ARM64 runtime archive SHA-256 mismatch")
    if sha1 != manifest["archive_sha1"]:
        raise ValueError("ARM64 runtime archive SHA-1 mismatch")
    return {"size": path.stat().st_size, "sha256": sha256, "sha1": sha1}


def extract_secure(archive, destination, manifest):
    top = manifest["archive"].removesuffix(".tar.bz2")
    members = []
    seen = set()
    total = 0
    with tarfile.open(archive, "r:bz2") as bundle:
        for entry in bundle:
            path = PurePosixPath(entry.name)
            if path.is_absolute() or ".." in path.parts or "\\" in entry.name or not path.parts or path.parts[0] != top:
                raise ValueError(f"unsafe archive path: {entry.name}")
            if entry.name in seen or not (entry.isdir() or entry.isfile()):
                raise ValueError(f"unsupported or duplicate archive entry: {entry.name}")
            seen.add(entry.name)
            total += entry.size
            if len(seen) > manifest["max_members"] or total > manifest["max_unpacked_size"] or entry.size > manifest["max_file_size"]:
                raise ValueError("ARM64 runtime archive exceeds its extraction budget")
            members.append(entry)
        root = destination.parent
        with tempfile.TemporaryDirectory(prefix=".arm64-runtime-", dir=root) as scratch_name:
            scratch = Path(scratch_name)
            extracted = scratch / top
            extracted.mkdir(parents=True)
            for entry in members:
                path = PurePosixPath(entry.name)
                relative = Path(*path.parts[1:])
                target = extracted / relative
                if entry.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                source = bundle.extractfile(entry)
                if source is None:
                    raise ValueError(f"unable to read archive entry: {entry.name}")
                with source, target.open("wb") as output:
                    for chunk in iter(lambda: source.read(1024 * 1024), b""):
                        output.write(chunk)
                os.chmod(target, entry.mode & 0o755)
            os.replace(extracted, destination)


def elf_machine(path):
    header = path.read_bytes()[:64]
    if len(header) < 20 or header[:4] != b"\x7fELF" or header[4] != 2 or header[5] != 1:
        raise ValueError("Release/libcef.so is not a little-endian ELF64 object")
    machine = struct.unpack_from("<H", header, 18)[0]
    if machine != 183:
        raise ValueError(f"Release/libcef.so has unexpected ELF machine {machine}")
    return "AArch64"


def write_evidence(path, manifest_path, manifest, archive_info, runtime, binary=None):
    evidence = {
        "schema_version": 1,
        "status": "CI_CODE_TEST_BUILD",
        "evidence_kind": manifest["evidence_kind"],
        "native_qualification": manifest["native_qualification"],
        "target": manifest["target"],
        "runner": manifest["ci_runner"],
        "engine": {
            "cef": manifest["cef_version"],
            "chromium": manifest["chromium_version"],
        },
        "provenance": {
            "source": manifest["source"],
            "url": manifest["url"],
            "manifest_sha256": digest(manifest_path, "sha256"),
            "archive": manifest["archive"],
            "archive_size": archive_info["size"],
            "archive_sha256": archive_info["sha256"],
            "archive_sha1": archive_info["sha1"],
        },
        "runtime": runtime,
        "checks": [
            "manifest_contract",
            "archive_size",
            "archive_sha256",
            "archive_sha1",
            "safe_extraction",
            "aarch64_elf",
        ],
    }
    if binary is not None:
        evidence["build"] = {
            "status": "PASS",
            "binary": str(binary),
            "sha256": digest(binary, "sha256"),
            "size": binary.stat().st_size,
            "commands": [
                "cargo test -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --locked",
                "cargo clippy -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --all-targets --locked -- -D warnings",
                "cargo build --release -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --locked",
                "cargo fmt --check",
                "cargo clippy --workspace --all-targets --locked --target aarch64-unknown-linux-gnu -- -D warnings",
                "cargo test --workspace --locked --target aarch64-unknown-linux-gnu",
                "cargo build --workspace --release --locked --target aarch64-unknown-linux-gnu",
            ],
        }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence


def main():
    parser = argparse.ArgumentParser(description="Verify the pinned ARM64 CEF development runtime used by GitHub Actions")
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--download", action="store_true")
    parser.add_argument("--check-manifest-only", action="store_true")
    parser.add_argument("--record-build", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    manifest = load_manifest(args.manifest)
    manifest_digest = digest(args.manifest, "sha256")
    if args.check_manifest_only:
        print(json.dumps({"status": "MANIFEST_VALID", "manifest_sha256": manifest_digest, "target": manifest["target"]}))
        return
    if args.destination is None:
        raise ValueError("--destination is required")
    evidence_path = args.output or args.destination / "arm64-runtime-verification.json"
    if args.record_build is not None:
        if not args.record_build.is_file():
            raise ValueError(f"ARM64 build binary is absent: {args.record_build}")
        existing = json.loads(evidence_path.read_text())
        if existing["provenance"]["manifest_sha256"] != manifest_digest:
            raise ValueError("runtime evidence was produced from a different manifest")
        existing["build"] = {
            "status": "PASS",
            "binary": str(args.record_build),
            "sha256": digest(args.record_build, "sha256"),
            "size": args.record_build.stat().st_size,
            "commands": [
                "cargo test -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --locked",
                "cargo clippy -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --all-targets --locked -- -D warnings",
                "cargo build --release -p paneflow-browser-host --target aarch64-unknown-linux-gnu --features cef-runtime --locked",
                "cargo fmt --check",
                "cargo clippy --workspace --all-targets --locked --target aarch64-unknown-linux-gnu -- -D warnings",
                "cargo test --workspace --locked --target aarch64-unknown-linux-gnu",
                "cargo build --workspace --release --locked --target aarch64-unknown-linux-gnu",
            ],
        }
        evidence_path.write_text(json.dumps(existing, indent=2) + "\n")
        print(json.dumps({"status": "BUILD_EVIDENCE_RECORDED", "path": str(evidence_path)}))
        return
    if args.archive is None:
        raise ValueError("--archive is required")
    args.archive.parent.mkdir(parents=True, exist_ok=True)
    if args.download and not args.archive.exists():
        download(manifest["url"], args.archive, manifest["archive_size"])
    if not args.archive.is_file():
        raise ValueError(f"ARM64 runtime archive is absent: {args.archive}")
    archive_info = verify_archive(args.archive, manifest)
    if args.destination.exists():
        raise ValueError(f"runtime destination already exists: {args.destination}")
    args.destination.parent.mkdir(parents=True, exist_ok=True)
    extract_secure(args.archive, args.destination, manifest)
    libcef = args.destination / "Release/libcef.so"
    if not libcef.is_file():
        raise ValueError("runtime has no Release/libcef.so")
    runtime = {
        "elf_machine": elf_machine(libcef),
        "libcef_sha256": digest(libcef, "sha256"),
        "libcef_size": libcef.stat().st_size,
    }
    (args.destination / "verified-manifest.sha256").write_text(f"{manifest_digest}\n")
    write_evidence(evidence_path, args.manifest, manifest, archive_info, runtime)
    print(json.dumps({"status": "VERIFIED", "runtime": str(args.destination), "evidence": str(evidence_path)}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, json.JSONDecodeError, tarfile.TarError, urllib.error.URLError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}))
        raise SystemExit(1) from None
