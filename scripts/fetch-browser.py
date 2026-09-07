#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def extract(archive, destination, top, total_limit, file_limit):
    total = 0
    seen = set()
    with tarfile.open(archive, "r:bz2") as bundle:
        members = []
        for entry in bundle:
            path = PurePosixPath(entry.name)
            if path.is_absolute() or ".." in path.parts or "\\" in entry.name or not path.parts or path.parts[0] != top:
                raise ValueError(f"unsafe archive path: {entry.name}")
            if not (entry.isfile() or entry.isdir()) or entry.name in seen:
                raise ValueError(f"unsupported or duplicate archive entry: {entry.name}")
            seen.add(entry.name)
            total += entry.size
            if len(seen) > 20000 or total > total_limit or entry.size > file_limit:
                raise ValueError("archive exceeds extraction budget")
            entry.mode &= 0o755
            members.append(entry)
        bundle.extractall(destination, members=members, filter="data")


def inspect_elf(root, maximum):
    report = []
    for path in sorted((root / "Release").iterdir()):
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        result = subprocess.run(["readelf", "-W", "-V", "-d", "-h", str(path)], check=True, capture_output=True, text=True)
        versions = sorted(set(re.findall(r"\bGLIBC_([0-9.]+)", result.stdout)), key=lambda item: tuple(map(int, item.split("."))))
        if versions and tuple(map(int, versions[-1].split("."))) > tuple(map(int, maximum.split("."))):
            raise ValueError(f"{path.name}: GLIBC_{versions[-1]} exceeds Ubuntu 22.04 GLIBC_{maximum}")
        report.append({"file": path.name, "glibc": versions, "needed": re.findall(r"\(NEEDED\).*?\[(.*?)\]", result.stdout)})
    if not any(item["file"] == "libcef.so" for item in report):
        raise ValueError("runtime has no ELF libcef.so")
    return report


def verify(root, target):
    for name, expected in target["files"].items():
        path = root / name
        if not path.is_file() or digest(path) != expected:
            raise ValueError(f"runtime file missing or checksum mismatch: {name}")


def main():
    parser = argparse.ArgumentParser(description="Explicit verified CEF fetch; never runs a browser or modifies installed profiles")
    parser.add_argument("--target", required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--manifest", type=Path, default=ROOT / "native/browser/manifest.toml")
    parser.add_argument("--destination", type=Path, default=ROOT / "native/browser/prebuilt")
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    manifest = tomllib.loads(args.manifest.read_text())
    if manifest["contract_version"] != 3:
        raise ValueError("unsupported browser contract version")
    target = manifest["targets"].get(args.target)
    if not target or target["availability"] == "absent":
        raise ValueError(f"browser unavailable: no candidate for {args.target}")
    destination = args.destination.resolve() / args.target / target["sha256"]
    if destination.exists():
        verify(destination, target)
        (destination / "verified-manifest.sha256").write_text(digest(args.manifest) + "\n")
        print(json.dumps({"status": "VERIFIED", "path": str(destination), "native_qualification": "not_executed"}))
        return
    if args.verify_only:
        raise ValueError(f"runtime absent: explicitly fetch {args.target} first")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".fetch-", dir=destination.parent) as scratch:
        scratch = Path(scratch)
        archive = args.archive
        if archive is None:
            archive = scratch / target["archive"]
            if not target["url"].startswith("https://cef-builds.spotifycdn.com/"):
                raise ValueError("archive provenance must use the pinned HTTPS distribution host")
            with urllib.request.urlopen(target["url"], timeout=60) as response, archive.open("wb") as output:
                remaining = target["size"]
                while chunk := response.read(min(1024 * 1024, remaining + 1)):
                    remaining -= len(chunk)
                    if remaining < 0:
                        raise ValueError("download exceeds pinned archive size")
                    output.write(chunk)
        if archive.stat().st_size != target["size"] or digest(archive) != target["sha256"]:
            raise ValueError("archive size or SHA-256 mismatch; existing runtimes preserved")
        top = target["archive"].removesuffix(".tar.bz2")
        extract(archive, scratch, top, target["unpacked_size"], target["max_file_size"])
        extracted = scratch / top
        verify(extracted, target)
        report = inspect_elf(extracted, manifest["maximum_glibc"])
        (extracted / "elf-audit.json").write_text(json.dumps(report, indent=2) + "\n")
        (extracted / "verified-manifest.sha256").write_text(digest(args.manifest) + "\n")
        os.rename(extracted, destination)
    print(json.dumps({"status": "VERIFIED", "path": str(destination), "native_qualification": "not_executed"}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}), file=__import__("sys").stderr)
        raise SystemExit(1) from None
