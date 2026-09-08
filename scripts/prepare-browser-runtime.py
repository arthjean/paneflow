#!/usr/bin/env python3
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
REQUIRED_ARGS = {
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
BINARIES = {
    "chrome_sandbox": "chrome-sandbox",
    "libcef.so": "libcef.so",
    "libEGL.so": "libEGL.so",
    "libGLESv2.so": "libGLESv2.so",
    "libvk_swiftshader.so": "libvk_swiftshader.so",
    "libvulkan.so.1": "libvulkan.so.1",
    "v8_context_snapshot.bin": "v8_context_snapshot.bin",
    "vk_swiftshader_icd.json": "vk_swiftshader_icd.json",
}
RESOURCES = ("chrome_100_percent.pak", "chrome_200_percent.pak", "resources.pak", "icudtl.dat")
CODEC_SYMBOLS = {
    "h264": "ff_h264_decoder",
    "hevc": "ff_hevc_decoder",
    "aac": "ff_aac_decoder",
    "mp3": "ff_mp3_decoder",
    "flac": "ff_flac_decoder",
    "vorbis": "ff_vorbis_decoder",
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def command(*args, cwd=None):
    return subprocess.run(args, cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def parse_arg(value):
    value = value.strip()
    if value == "true":
        return True
    if value == "false":
        return False
    if value.startswith('"'):
        return json.loads(value)
    try:
        return int(value)
    except ValueError:
        return value


def load_args(path):
    result = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        result[key.strip()] = parse_arg(value)
    return result


def verify_build(build, metadata_path, log_path, manifest):
    args_path = build / "args.gn"
    actual_args = load_args(args_path)
    mismatched = {
        key: {"expected": expected, "actual": actual_args.get(key)}
        for key, expected in REQUIRED_ARGS.items()
        if actual_args.get(key) != expected
    }
    if mismatched:
        raise ValueError(f"hardening arguments do not match: {json.dumps(mismatched, sort_keys=True)}")
    metadata = json.loads(metadata_path.read_text())
    if metadata.get("gn_args_sha256") != digest(args_path):
        raise ValueError("candidate metadata does not bind the actual args.gn")
    for key, expected in REQUIRED_ARGS.items():
        if metadata.get("gn_args", {}).get(key) != expected:
            raise ValueError(f"candidate metadata has invalid {key}")
    log_lines = [line for line in log_path.read_text(errors="replace").splitlines() if line.strip()]
    if not log_lines or not log_lines[-1].startswith("[21140/21140] LINK ./cefsimple"):
        raise ValueError("official build log has no successful terminal action")
    source = build.parent.parent
    chromium_sha = command("git", "rev-parse", "HEAD", cwd=source)
    cef_sha = command("git", "rev-parse", "HEAD", cwd=source / "cef")
    if cef_sha != manifest["cef_commit"]:
        raise ValueError(f"CEF checkout {cef_sha} differs from manifest {manifest['cef_commit']}")
    patches = metadata.get("patches", {})
    if not patches:
        raise ValueError("candidate metadata has no patch inventory")
    for name, expected in patches.items():
        patch = ROOT / "native/browser/experiments/tsync" / name
        if not patch.is_file() or digest(patch) != expected:
            raise ValueError(f"patch provenance mismatch: {name}")
        subprocess.run(["git", "apply", "--reverse", "--check", str(patch)], cwd=source, check=True,
                       capture_output=True, text=True)
    return {
        "args": actual_args,
        "args_sha256": digest(args_path),
        "metadata_sha256": digest(metadata_path),
        "build_log_sha256": digest(log_path),
        "chromium_source_sha": chromium_sha,
        "cef_source_sha": cef_sha,
        "patches": patches,
    }


def copy_runtime(build, root, credits):
    release = root / "Release"
    resources = root / "Resources"
    locales = resources / "locales"
    release.mkdir(parents=True)
    locales.mkdir(parents=True)
    for source, destination in BINARIES.items():
        path = build / source
        if not path.is_file():
            raise ValueError(f"official build output is missing {source}")
        shutil.copy2(path, release / destination)
    for name in RESOURCES:
        path = build / name
        if not path.is_file():
            raise ValueError(f"official build output is missing {name}")
        shutil.copy2(path, resources / name)
    locale_files = sorted((build / "locales").glob("*.pak"))
    if not locale_files:
        raise ValueError("official build output has no locales")
    for locale in locale_files:
        shutil.copy2(locale, locales / locale.name)
    shutil.copy2(credits, root / "CREDITS.html")
    sandbox = release / "chrome-sandbox"
    sandbox.chmod(0o755)
    for path in root.rglob("*"):
        if path.is_file() and path != sandbox:
            path.chmod(0o755 if path.suffix == ".so" or ".so." in path.name else 0o644)


def strip_runtime(root, strip_binary):
    libcef = root / "Release/libcef.so"
    unstripped = digest(libcef)
    subprocess.run([strip_binary, "--strip-unneeded", str(libcef)], check=True)
    sections = command("readelf", "-W", "-S", str(libcef))
    if ".symtab" in sections:
        raise ValueError("stripped libcef.so still contains .symtab")
    return unstripped, digest(libcef)


def codec_inventory(libcef):
    symbols = set(command("strings", "-a", str(libcef)).splitlines())
    return {name: symbol in symbols for name, symbol in CODEC_SYMBOLS.items()}


def toml_value(value):
    return json.dumps(value, ensure_ascii=True)


def write_manifest(path, base, target, files, hardening, provenance, codecs):
    lines = [f"{key} = {toml_value(value)}" for key, value in base.items() if key != "targets"]
    lines += ["", '[targets."x86_64-unknown-linux-gnu"]']
    lines += [f"{key} = {toml_value(value)}" for key, value in target.items()]
    lines += ["", '[targets."x86_64-unknown-linux-gnu".hardening]']
    lines += [f"{key} = {toml_value(value)}" for key, value in hardening.items()]
    lines += ["", '[targets."x86_64-unknown-linux-gnu".provenance]']
    lines += [f"{key} = {toml_value(value)}" for key, value in provenance.items()]
    lines += ["", '[targets."x86_64-unknown-linux-gnu".codecs]']
    lines += [f"{key} = {toml_value(value)}" for key, value in codecs.items()]
    lines += ["", '[targets."x86_64-unknown-linux-gnu".files]']
    lines += [f"{toml_value(name)} = {toml_value(value)}" for name, value in files.items()]
    path.write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser(description="Create a verified hardened CEF candidate without modifying its build output")
    parser.add_argument("--build-output", type=Path, required=True)
    parser.add_argument("--candidate-metadata", type=Path, required=True)
    parser.add_argument("--build-log", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-manifest", type=Path, default=ROOT / "native/browser/manifest.toml")
    parser.add_argument("--strip-binary", default=shutil.which("llvm-strip") or shutil.which("strip"))
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError(f"output already exists: {args.output}")
    if not args.strip_binary:
        raise ValueError("no ELF strip tool available")
    build = args.build_output.resolve()
    manifest = tomllib.loads(args.base_manifest.read_text())
    provenance = verify_build(build, args.candidate_metadata.resolve(), args.build_log.resolve(), manifest)
    args.output.mkdir(parents=True)
    name = f"cef_binary_{manifest['cef_version']}_linux64_hardened_candidate"
    runtime = args.output / name
    credits = build / "gen/components/resources/about_credits.html"
    if not credits.is_file():
        raise ValueError("official build output has no generated Chromium credits")
    copy_runtime(build, runtime, credits)
    codecs = codec_inventory(runtime / "Release/libcef.so")
    unstripped_sha, stripped_sha = strip_runtime(runtime, args.strip_binary)
    files = {
        path.relative_to(runtime).as_posix(): digest(path)
        for path in sorted(runtime.rglob("*")) if path.is_file()
    }
    total = sum(path.stat().st_size for path in runtime.rglob("*") if path.is_file())
    maximum = max(path.stat().st_size for path in runtime.rglob("*") if path.is_file())
    archive = args.output / f"{name}.tar.bz2"
    with tarfile.open(archive, "w:bz2", compresslevel=9) as bundle:
        bundle.add(runtime, arcname=name)
    target = {
        "unpacked_size": total,
        "max_file_size": maximum,
        "availability": "development",
        "native_qualification": "hardened_candidate_not_qualified",
        "archive": archive.name,
        "url": archive.as_uri(),
        "sha256": digest(archive),
        "size": archive.stat().st_size,
    }
    hardening = {key: REQUIRED_ARGS[key] for key in REQUIRED_ARGS}
    recorded_provenance = {
        "chromium_source_sha": provenance["chromium_source_sha"],
        "cef_source_sha": provenance["cef_source_sha"],
        "gn_args_sha256": provenance["args_sha256"],
        "candidate_metadata_sha256": provenance["metadata_sha256"],
        "build_log_sha256": provenance["build_log_sha256"],
        "unstripped_libcef_sha256": unstripped_sha,
        "stripped_libcef_sha256": stripped_sha,
        "credits_sha256": digest(runtime / "CREDITS.html"),
        "strip_tool": command(args.strip_binary, "--version").splitlines()[0],
        "patches_sha256": hashlib.sha256(json.dumps(provenance["patches"], sort_keys=True).encode()).hexdigest(),
    }
    candidate = args.output / "manifest.toml"
    write_manifest(candidate, manifest, target, files, hardening, recorded_provenance, codecs)
    receipt = {
        "schema_version": 1,
        "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "build_output": str(build),
        "runtime": str(runtime),
        "archive": str(archive),
        "manifest": str(candidate),
        "manifest_sha256": digest(candidate),
        "archive_sha256": target["sha256"],
        "runtime_files": len(files),
        "unpacked_size": total,
        "max_file_size": maximum,
        "hardening": hardening,
        "provenance": recorded_provenance,
        "codecs": codecs,
        "qualification": "NOT_EXECUTED",
    }
    (args.output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}), file=__import__("sys").stderr)
        raise SystemExit(1) from None
