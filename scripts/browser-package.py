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
import tomllib

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_SUBDIR = Path("lib/paneflow/browser")
HOST_SUBDIR = Path("lib/paneflow/paneflow-browser-host")
DOC_SUBDIR = Path("share/doc/paneflow")
STAMP = "verified-manifest.sha256"
REQUIRED = ("Release/libcef.so", "Release/chrome-sandbox", "Resources/icudtl.dat", "Resources/locales/en-US.pak")
COMPRESSED_BUDGET = 250 * 1024 * 1024
INSTALLED_BUDGET = 600 * 1024 * 1024
SETUID_FORMATS = ("deb", "rpm")
CODEC_SYMBOLS = {
    "h264": "ff_h264_decoder",
    "hevc": "ff_hevc_decoder",
    "aac": "ff_aac_decoder",
    "mp3": "ff_mp3_decoder",
    "flac": "ff_flac_decoder",
    "vorbis": "ff_vorbis_decoder",
}
RESTRICTED_CODECS = ("h264", "hevc", "aac")
REQUIRED_HARDENING = {
    "is_debug": False,
    "is_official_build": True,
    "is_cfi": True,
    "use_cfi_icall": True,
    "use_cfi_diag": False,
    "use_cfi_recover": False,
    "use_thin_lto": True,
    "thin_lto_enable_optimizations": True,
    "chrome_pgo_phase": 2,
    "symbol_level": 0,
    "blink_symbol_level": 0,
    "v8_symbol_level": 0,
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load_manifest(path):
    text = path.read_text()
    manifest = tomllib.loads(text)
    if manifest["contract_version"] != 3:
        raise ValueError("unsupported browser contract version")
    return manifest, hashlib.sha256(text.encode()).hexdigest()


def target_entry(manifest, target):
    entry = manifest["targets"].get(target)
    if not entry or entry["availability"] == "absent":
        raise ValueError(f"browser unavailable: no pinned candidate for {target}")
    return entry


def verified_runtime(target, entry, prebuilt):
    root = prebuilt / target / entry["sha256"]
    if not root.is_dir():
        raise ValueError(f"runtime absent: run scripts/fetch-browser.py --target {target} first")
    for name, expected in entry["files"].items():
        path = root / name
        if not path.is_file() or digest(path) != expected:
            raise ValueError(f"runtime file missing or checksum mismatch: {name}")
    return root


def readelf(path, *flags):
    result = subprocess.run(["readelf", "-W", *flags, str(path)], check=True, capture_output=True, text=True)
    return result.stdout


def elf_report(path):
    dynamic = readelf(path, "-d")
    return {
        "needed": re.findall(r"\(NEEDED\).*?\[(.*?)\]", dynamic),
        "runpath": re.findall(r"\((?:RUNPATH|RPATH)\).*?\[(.*?)\]", dynamic),
        "soname": re.findall(r"\(SONAME\).*?\[(.*?)\]", dynamic),
    }


def is_elf(path):
    with path.open("rb") as stream:
        return stream.read(4) == b"\x7fELF"


def tree_files(root):
    return sorted(path for path in root.rglob("*") if path.is_file() and not path.is_symlink())


def installed_bytes(root):
    return sum(path.stat().st_size for path in tree_files(root))


def browser_bytes(prefix):
    payload = [prefix / "lib/paneflow"]
    payload += [prefix / DOC_SUBDIR / name for name in ("BROWSER_THIRD_PARTY_NOTICES.md", "browser-sbom.json")]
    return sum(installed_bytes(entry) if entry.is_dir() else entry.stat().st_size
               for entry in payload if entry.exists())


def contains(path, needle):
    window = len(needle)
    previous = b""
    with path.open("rb") as stream:
        while chunk := stream.read(1 << 20):
            if needle in previous + chunk:
                return True
            previous = chunk[-window:]
    return False


def stage(args):
    manifest, manifest_digest = load_manifest(args.manifest)
    entry = target_entry(manifest, args.target)
    source = verified_runtime(args.target, entry, args.prebuilt)
    host = args.host or default_host(args.target)
    if not host.is_file() or not os.access(host, os.X_OK):
        raise ValueError(
            f"host binary absent at {host}: build it with "
            f"cargo build --release --locked -p paneflow-browser-host --features cef-runtime"
        )
    destination = args.destination.resolve()
    runtime = destination / RUNTIME_SUBDIR
    if destination.exists():
        shutil.rmtree(destination)
    for name in entry["files"]:
        target_path = runtime / name
        target_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / name, target_path)
        target_path.chmod(0o755 if is_elf(target_path) else 0o644)
    (runtime / STAMP).write_text(manifest_digest + "\n")
    (runtime / STAMP).chmod(0o644)
    sandbox = runtime / "Release/chrome-sandbox"
    sandbox.chmod(0o4755)
    host_target = destination / HOST_SUBDIR
    host_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(host, host_target)
    host_target.chmod(0o755)
    docs = destination / DOC_SUBDIR
    docs.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "native/browser/BROWSER_THIRD_PARTY_NOTICES.md", docs / "BROWSER_THIRD_PARTY_NOTICES.md")
    (docs / "BROWSER_THIRD_PARTY_NOTICES.md").chmod(0o644)
    write_sbom(destination, args.target, "stage", manifest, entry, manifest_digest,
               docs / "browser-sbom.json")
    receipt = {
        "schema_version": 1,
        "target": args.target,
        "cef_version": manifest["cef_version"],
        "chromium_version": manifest["chromium_version"],
        "manifest_sha256": manifest_digest,
        "runtime_sha256": entry["sha256"],
        "availability": entry["availability"],
        "native_qualification": entry["native_qualification"],
        "host_sha256": digest(host_target),
        "runtime_files": len(entry["files"]),
        "installed_bytes": installed_bytes(destination),
        "prefix": str(destination),
    }
    (destination / "stage.json").write_text(json.dumps(receipt, indent=2) + "\n")
    verdict = inspect(destination, "stage", args.target, manifest, entry, manifest_digest)
    receipt["verification"] = verdict["status"]
    (destination / "stage.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))
    if verdict["status"] != "PASS":
        failed = [item for item in verdict["checks"] if item["status"] == "FAIL"]
        print(json.dumps(failed, indent=2), file=sys.stderr)
        if not args.allow_open_gates:
            raise ValueError(
                "the staged payload does not pass its release gates; pass --allow-open-gates to stage it anyway"
            )


def default_host(target):
    triple = ROOT / "target" / target / "release/paneflow-browser-host"
    return triple if triple.is_file() else ROOT / "target/release/paneflow-browser-host"


def check(report, name, ok, detail):
    report.append({"check": name, "status": "PASS" if ok else "FAIL", "detail": detail})
    return ok


def packaged_sandbox(archive, package_format):
    wanted = "/usr/lib/paneflow/browser/Release/chrome-sandbox"
    if package_format == "deb":
        members = subprocess.run(["ar", "t", str(archive)], check=True, capture_output=True,
                                 text=True).stdout.splitlines()
        data = next((name for name in members if name.startswith("data.tar")), None)
        if not data:
            return False, "deb has no data archive"
        producer = subprocess.Popen(["ar", "p", str(archive), data], stdout=subprocess.PIPE)
        compression = (["-J"] if data.endswith(".xz") else ["-z"] if data.endswith(".gz")
                       else ["-j"] if data.endswith(".bz2") else ["--zstd"] if data.endswith(".zst")
                       else [])
        listing = subprocess.run(["tar", *compression, "-tvf", "-"], stdin=producer.stdout, check=True,
                                 capture_output=True, text=True).stdout.splitlines()
        producer.stdout.close()
        if producer.wait() != 0:
            raise subprocess.SubprocessError("ar failed while reading deb data archive")
    elif package_format == "rpm":
        listing = subprocess.run(["rpm", "-qplv", str(archive)], check=True,
                                 capture_output=True, text=True).stdout.splitlines()
    else:
        return False, f"unsupported package metadata format {package_format}"
    line = next((item for item in listing if item.rstrip().endswith(wanted)), "")
    fields = line.split()
    mode = fields[0] if fields else ""
    root_owned = (
        " 0/0 " in f" {line} "
        or len(fields) > 3 and fields[2:4] == ["root", "root"]
    )
    return len(mode) > 3 and mode[3] == "s" and root_owned, line or f"{wanted} absent"


def verify(args):
    manifest, manifest_digest = load_manifest(args.manifest)
    entry = target_entry(manifest, args.target)
    verdict = inspect(args.prefix.resolve(), args.format, args.target, manifest, entry,
                      manifest_digest, args.archive, args.baseline)
    print(json.dumps(verdict, indent=2))
    if verdict["status"] != "PASS":
        raise SystemExit(1)


def inspect(prefix, package_format, target, manifest, entry, manifest_digest, archive=None, baseline=None):
    runtime = prefix / RUNTIME_SUBDIR
    host = prefix / HOST_SUBDIR
    report = []

    hardening = entry.get("hardening", {})
    hardening_mismatch = {
        key: {"expected": expected, "actual": hardening.get(key)}
        for key, expected in REQUIRED_HARDENING.items()
        if hardening.get(key) != expected
    }
    check(report, "runtime_hardening", not hardening_mismatch,
          "CFI/icall/ThinLTO/PGO official build" if not hardening_mismatch
          else json.dumps(hardening_mismatch, sort_keys=True))
    provenance = entry.get("provenance", {})
    provenance_fields = (
        "chromium_source_sha", "cef_source_sha", "gn_args_sha256",
        "candidate_metadata_sha256", "build_log_sha256", "unstripped_libcef_sha256",
        "stripped_libcef_sha256", "credits_sha256", "patches_sha256",
    )
    invalid_provenance = [
        name for name in provenance_fields
        if not re.fullmatch(r"[0-9a-f]{40}" if name.endswith("source_sha") else r"[0-9a-f]{64}",
                            str(provenance.get(name, "")))
    ]
    check(report, "runtime_provenance", not invalid_provenance,
          f"invalid or absent fields: {invalid_provenance or 'none'}")
    codecs = entry.get("codecs", {})
    invalid_codecs = [name for name in CODEC_SYMBOLS if not isinstance(codecs.get(name), bool)]
    check(report, "runtime_codecs", not invalid_codecs,
          f"invalid or absent codec fields: {invalid_codecs or 'none'}")

    check(report, "layout", all((runtime / name).is_file() for name in REQUIRED) and host.is_file(),
          f"runtime {runtime}, host {host}")
    stamp = (runtime / STAMP).read_text().strip() if (runtime / STAMP).is_file() else ""
    check(report, "manifest_stamp", stamp == manifest_digest,
          f"stamp {stamp or 'absent'} against embedded manifest {manifest_digest}")

    mismatched = [name for name, expected in entry["files"].items()
                  if not (runtime / name).is_file() or digest(runtime / name) != expected]
    check(report, "runtime_digests", not mismatched,
          f"{len(entry['files']) - len(mismatched)}/{len(entry['files'])} pinned files match"
          + (f"; first mismatch {mismatched[0]}" if mismatched else ""))

    if host.is_file():
        mode = stat.S_IMODE(host.stat().st_mode)
        check(report, "host_executable", is_elf(host) and mode & 0o111, f"mode {mode:04o}")
        elf = elf_report(host)
        check(report, "host_runpath", all(not entry_path.startswith("/") for entry_path in elf["runpath"]),
              f"RUNPATH {elf['runpath'] or 'none'}")

    libcef = runtime / "Release/libcef.so"
    if libcef.is_file():
        elf = elf_report(libcef)
        check(report, "runtime_runpath", all(not entry_path.startswith("/") for entry_path in elf["runpath"]),
              f"RUNPATH {elf['runpath'] or 'none'}")
        check(report, "runtime_needed", bool(elf["needed"]),
              f"{len(elf['needed'])} SONAME dependencies: {', '.join(sorted(elf['needed']))}")
        sections = readelf(libcef, "-S")
        check(report, "runtime_stripped", ".symtab" not in sections,
              "libcef.so has no static symbol table" if ".symtab" not in sections
              else "libcef.so still contains .symtab")

    sandbox = runtime / "Release/chrome-sandbox"
    if sandbox.is_file():
        mode = stat.S_IMODE(sandbox.stat().st_mode)
        required_setuid = package_format in SETUID_FORMATS
        if required_setuid and archive:
            sandbox_ok, sandbox_detail = packaged_sandbox(archive, package_format)
        elif package_format == "targz":
            sandbox_ok = not bool(mode & stat.S_ISUID)
            sandbox_detail = f"mode {mode:04o}; targz requires the unprivileged user-namespace sandbox"
        else:
            sandbox_ok = bool(mode & stat.S_ISUID) or not required_setuid
            sandbox_detail = f"mode {mode:04o}; {package_format} " + (
                "requires a root-owned setuid helper" if required_setuid
                else "relies on the unprivileged user-namespace sandbox"
            )
        check(report, "sandbox_rights", sandbox_ok, sandbox_detail)

    checkout = str(ROOT).encode()
    leaking = [str(path.relative_to(prefix)) for path in tree_files(prefix)
               if path.suffix not in {".pak", ".dat", ".bin", ".json", ".md"} and contains(path, checkout)]
    check(report, "no_checkout_dependency", not leaking,
          f"binaries referencing {ROOT}: {leaking or 'none'}")

    total = browser_bytes(prefix)
    check(report, "installed_budget", total <= INSTALLED_BUDGET,
          f"{total} bytes of browser payload against the NFR-13 {INSTALLED_BUDGET} byte ceiling")
    if archive and baseline:
        overhead = archive.stat().st_size - baseline.stat().st_size
        check(report, "compressed_budget", overhead <= COMPRESSED_BUDGET,
              f"{overhead} bytes over {baseline.name} against the NFR-13 {COMPRESSED_BUDGET} byte ceiling")

    credits = [name for name in ("Resources/CREDITS.html", "Release/CREDITS.html", "CREDITS.html")
               if (runtime / name).is_file()]
    check(report, "chromium_credits", bool(credits),
          f"Chromium credits document {credits[0] if credits else 'absent from the pinned runtime'}")

    notices = prefix / DOC_SUBDIR / "BROWSER_THIRD_PARTY_NOTICES.md"
    check(report, "notices", notices.is_file(), f"{notices}")

    check(report, "capability_manifest", entry["availability"] != "absent",
          f"availability {entry['availability']}, native qualification {entry['native_qualification']}")

    return {
        "schema_version": 1,
        "target": target,
        "format": package_format,
        "prefix": str(prefix),
        "status": "PASS" if all(item["status"] == "PASS" for item in report) else "FAIL",
        "checks": report,
    }


def sbom_document(prefix, target, package_format, manifest, entry, manifest_digest):
    runtime = prefix / RUNTIME_SUBDIR
    libcef = runtime / "Release/libcef.so"
    if not libcef.is_file():
        raise ValueError(f"no staged runtime under {runtime}")
    codecs = entry["codecs"]
    restricted = sorted(name for name in RESTRICTED_CODECS if codecs.get(name))
    files = [
        {
            "path": str(path.relative_to(prefix)),
            "bytes": path.stat().st_size,
            "sha256": digest(path),
        }
        for path in tree_files(prefix)
    ]
    return {
        "schema_version": 1,
        "target": target,
        "format": package_format,
        "manifest_sha256": manifest_digest,
        "runtime_sha256": entry["sha256"],
        "availability": entry["availability"],
        "native_qualification": entry["native_qualification"],
        "hardening": entry["hardening"],
        "provenance": entry["provenance"],
        "components": [
            {"name": "Chromium Embedded Framework", "version": manifest["cef_version"], "license": manifest["cef_license"], "commit": manifest["cef_commit"]},
            {"name": "Chromium", "version": manifest["chromium_version"], "license": "BSD-3-Clause AND LicenseRef-Chromium-Credits"},
            {"name": "cef-rs bindings", "version": manifest["cef_rs_version"], "license": manifest["cef_rs_license"], "commit": manifest["cef_rs_commit"]},
            {"name": "SwiftShader", "version": manifest["chromium_version"], "license": "Apache-2.0"},
            {"name": "Vulkan-Loader", "version": manifest["chromium_version"], "license": "Apache-2.0"},
            {"name": "ICU data", "version": manifest["chromium_version"], "license": "Unicode-3.0"},
        ],
        "codecs": codecs,
        "restricted_codecs": restricted,
        "redistribution": "unrestricted" if not restricted else "review_required",
        "installed_bytes": browser_bytes(prefix),
        "files": files,
    }


def write_sbom(prefix, target, package_format, manifest, entry, manifest_digest, output):
    document = sbom_document(prefix, target, package_format, manifest, entry, manifest_digest)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(document, indent=2) + "\n")
    output.chmod(0o644)
    return document


def sbom(args):
    manifest, manifest_digest = load_manifest(args.manifest)
    entry = target_entry(manifest, args.target)
    document = write_sbom(args.prefix.resolve(), args.target, args.format, manifest, entry,
                          manifest_digest, args.output)
    print(json.dumps({k: v for k, v in document.items() if k != "files"}, indent=2))


def main():
    parser = argparse.ArgumentParser(description="Stage, verify and inventory the distributed Linux browser runtime")
    parser.add_argument("--manifest", type=Path, default=ROOT / "native/browser/manifest.toml")
    parser.add_argument("--target", default="x86_64-unknown-linux-gnu")
    commands = parser.add_subparsers(dest="command", required=True)

    staging = commands.add_parser("stage")
    staging.add_argument("--prebuilt", type=Path, default=ROOT / "native/browser/prebuilt")
    staging.add_argument("--destination", type=Path, default=ROOT / "target/browser-stage")
    staging.add_argument("--host", type=Path)
    staging.add_argument("--allow-open-gates", action="store_true")
    staging.set_defaults(handler=stage)

    verification = commands.add_parser("verify")
    verification.add_argument("--prefix", type=Path, default=ROOT / "target/browser-stage")
    verification.add_argument("--format", choices=["deb", "rpm", "appimage", "targz", "stage"], required=True)
    verification.add_argument("--archive", type=Path)
    verification.add_argument("--baseline", type=Path)
    verification.set_defaults(handler=verify)

    inventory = commands.add_parser("sbom")
    inventory.add_argument("--prefix", type=Path, default=ROOT / "target/browser-stage")
    inventory.add_argument("--format", choices=["deb", "rpm", "appimage", "targz"], required=True)
    inventory.add_argument("--output", type=Path, default=ROOT / "target/browser-stage/browser-sbom.json")
    inventory.set_defaults(handler=sbom)

    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}), file=sys.stderr)
        raise SystemExit(1) from None
