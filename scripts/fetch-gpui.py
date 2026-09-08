#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "native" / "gpui" / "manifest.toml"
STAMP_NAME = ".paneflow-gpui-stamp.json"
STAMP_SCHEMA = 1


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest():
    with open(MANIFEST, "rb") as handle:
        manifest = tomllib.load(handle)
    if manifest.get("schema_version") != 1:
        raise SystemExit("native/gpui/manifest.toml: unsupported schema_version")
    if not re.fullmatch(r"[0-9a-f]{40}", manifest["upstream_sha"]):
        raise SystemExit("native/gpui/manifest.toml: upstream_sha must be a full commit hash")
    return manifest


def patch_series(manifest, require_pins, allow_missing=False):
    directory = ROOT / manifest["patch_directory"]
    series = []
    for entry in manifest.get("patches", []):
        path = directory / entry["file"]
        if not re.fullmatch(r"[0-9]{4}-[a-z0-9-]+\.patch", entry["file"]):
            raise SystemExit(f"patch name is not in NNNN-slug.patch form: {entry['file']}")
        if not path.is_file():
            if allow_missing:
                continue
            raise SystemExit(f"patch is missing: {path}")
        actual = sha256_file(path)
        if entry["sha256"] != actual:
            if require_pins:
                raise SystemExit(f"patch digest mismatch for {entry['file']}: manifest {entry['sha256'] or '(unpinned)'} actual {actual}; run scripts/fetch-gpui.py --pin-patches after review")
        series.append({"file": entry["file"], "sha256": actual, "path": path})
    return series


def expected_stamp(manifest, series, pristine):
    return {
        "schema": STAMP_SCHEMA,
        "upstream_repository": manifest["upstream_repository"],
        "upstream_sha": manifest["upstream_sha"],
        "checkout_paths": list(manifest["checkout_paths"]),
        "patches": [] if pristine else [{"file": item["file"], "sha256": item["sha256"]} for item in series],
    }


def read_stamp(checkout):
    try:
        with open(checkout / STAMP_NAME, "rb") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return None


def git(cwd, *args, capture=False):
    result = subprocess.run(["git", *args], cwd=cwd, check=False, text=True, capture_output=capture)
    if result.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed in {cwd}" + (f": {result.stderr.strip()}" if capture else ""))
    return result.stdout if capture else ""


def package_directories(checkout, paths):
    packages = []
    for relative in paths:
        base = checkout / relative
        if base.is_file():
            continue
        for manifest_path in sorted(base.rglob("Cargo.toml")):
            if {"examples", "tests", "benches"} & set(manifest_path.relative_to(checkout).parts):
                continue
            with open(manifest_path, "rb") as handle:
                data = tomllib.load(handle)
            if "package" in data:
                packages.append(manifest_path.parent.relative_to(checkout).as_posix())
    return packages


def rewrite_workspace_members(checkout, packages):
    manifest_path = checkout / "Cargo.toml"
    lines = manifest_path.read_text(encoding="utf-8").splitlines(keepends=True)
    output = []
    in_workspace = False
    skipping_array = False
    inserted = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped != "[workspace]":
            if in_workspace and not inserted:
                output.append(members_block(packages))
                inserted = True
            in_workspace = False
        if stripped == "[workspace]":
            in_workspace = True
            output.append(line)
            continue
        if in_workspace:
            if skipping_array:
                if stripped.startswith("]"):
                    skipping_array = False
                continue
            key = stripped.split("=", 1)[0].strip()
            if key in {"members", "default-members", "exclude"}:
                if "[" in stripped and not stripped.rstrip().endswith("]"):
                    skipping_array = True
                continue
        output.append(line)
    if in_workspace and not inserted:
        output.append(members_block(packages))
    manifest_path.write_text("".join(output), encoding="utf-8")


def members_block(packages):
    entries = "".join(f'    "{package}",\n' for package in packages)
    return f"members = [\n{entries}]\n"


def create_checkout(manifest, series, pristine):
    target = ROOT / manifest["checkout_directory"]
    target.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".checkout-", dir=target.parent))
    try:
        git(staging, "init", "--quiet")
        git(staging, "remote", "add", "origin", manifest["upstream_repository"])
        git(staging, "config", "advice.detachedHead", "false")
        git(staging, "sparse-checkout", "init", "--cone")
        git(staging, "sparse-checkout", "set", *manifest["checkout_paths"])
        git(staging, "fetch", "--quiet", "--depth", "1", "--filter=blob:none", "origin", manifest["upstream_sha"])
        git(staging, "checkout", "--quiet", "--detach", "FETCH_HEAD")
        head = git(staging, "rev-parse", "HEAD", capture=True).strip()
        if head != manifest["upstream_sha"]:
            raise SystemExit(f"checked out {head}, expected {manifest['upstream_sha']}")
        for relative in manifest["checkout_paths"]:
            if not (staging / relative).exists():
                raise SystemExit(f"upstream tree lacks {relative}")
        if not pristine:
            for item in series:
                git(staging, "apply", "--check", "--unidiff-zero", "--whitespace=nowarn", str(item["path"]))
                git(staging, "apply", "--unidiff-zero", "--whitespace=nowarn", str(item["path"]))
        rewrite_workspace_members(staging, package_directories(staging, manifest["checkout_paths"]))
        with open(staging / STAMP_NAME, "w", encoding="utf-8") as handle:
            json.dump(expected_stamp(manifest, series, pristine), handle, indent=2, sort_keys=True)
            handle.write("\n")
        if target.exists():
            shutil.rmtree(target)
        os.replace(staging, target)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    return target


def pin_patches(manifest):
    series = patch_series(manifest, require_pins=False)
    text = MANIFEST.read_text(encoding="utf-8")
    for item in series:
        pattern = re.compile(r'(file = "' + re.escape(item["file"]) + r'"\nsha256 = ")[0-9a-f]*(")')
        text, count = pattern.subn(lambda match: match.group(1) + item["sha256"] + match.group(2), text)
        if count != 1:
            raise SystemExit(f"cannot pin {item['file']} in the manifest")
    MANIFEST.write_text(text, encoding="utf-8")
    for item in series:
        print(f"{item['sha256']}  {item['file']}")


def main():
    parser = argparse.ArgumentParser(description="Place the patched GPUI checkout that Cargo's [patch] table points at.")
    parser.add_argument("--verify-only", action="store_true", help="Fail unless the checkout already matches the manifest and patch series")
    parser.add_argument("--pristine", action="store_true", help="Create the upstream checkout without applying the patch series, for authoring patches")
    parser.add_argument("--pin-patches", action="store_true", help="Rewrite the manifest sha256 of every patch from the current files")
    parser.add_argument("--force", action="store_true", help="Recreate the checkout even when its stamp matches")
    args = parser.parse_args()
    manifest = load_manifest()
    if args.pin_patches:
        pin_patches(manifest)
        return
    series = patch_series(manifest, require_pins=not args.pristine, allow_missing=args.pristine)
    target = ROOT / manifest["checkout_directory"]
    wanted = expected_stamp(manifest, series, args.pristine)
    if read_stamp(target) == wanted and not args.force:
        print(target)
        return
    if args.verify_only:
        raise SystemExit(f"{target} does not match native/gpui/manifest.toml; run scripts/fetch-gpui.py")
    print(create_checkout(manifest, series, args.pristine))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
