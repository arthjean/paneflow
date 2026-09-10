#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
import tomllib
import uuid

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_SUBDIR = Path("lib/paneflow/browser")
HOST_SUBDIR = Path("lib/paneflow/paneflow-browser-host")
DOC_SUBDIR = Path("share/doc/paneflow")
STAMP = "verified-manifest.sha256"
REQUIRED = ("Release/libcef.so", "Release/chrome-sandbox", "Resources/icudtl.dat", "Resources/locales/en-US.pak")
COMPRESSED_BUDGET = 250 * 1024 * 1024
INSTALLED_BUDGET = 600 * 1024 * 1024
SETUID_FORMATS = ("deb", "rpm")
WINDOWS_PE_SUFFIXES = (".dll", ".exe")
WINDOWS_SYSTEM_DLLS = frozenset((
    "advapi32.dll", "bcrypt.dll", "cfgmgr32.dll", "combase.dll", "comctl32.dll", "comdlg32.dll",
    "crypt32.dll", "d3d11.dll", "d3d12.dll", "dbghelp.dll", "dcomp.dll", "dhcpcsvc.dll",
    "dnsapi.dll", "dwmapi.dll", "dwrite.dll", "dxgi.dll", "dxva2.dll", "gdi32.dll", "imm32.dll",
    "iphlpapi.dll", "kernel32.dll", "mf.dll", "mfplat.dll", "mfreadwrite.dll", "mscms.dll",
    "msimg32.dll", "ncrypt.dll", "netapi32.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll",
    "powrprof.dll", "propsys.dll", "psapi.dll", "rpcrt4.dll", "secur32.dll", "sechost.dll",
    "setupapi.dll", "shcore.dll", "shell32.dll", "shlwapi.dll", "urlmon.dll", "user32.dll",
    "userenv.dll", "usp10.dll", "uxtheme.dll", "version.dll", "wer.dll", "windowscodecs.dll",
    "wininet.dll", "winmm.dll", "winspool.drv", "wintrust.dll", "ws2_32.dll", "wtsapi32.dll",
))
WINDOWS_REDISTRIBUTABLE_DLLS = frozenset((
    "concrt140.dll", "msvcp140.dll", "msvcp140_1.dll", "msvcp140_2.dll",
    "vcruntime140.dll", "vcruntime140_1.dll",
))
WIX_NAMESPACE = "http://schemas.microsoft.com/wix/2006/wi"
WIX_COMPONENT_GROUP = "BrowserRuntime"
WIX_DIRECTORY_ROOT = "APPLICATIONFOLDER"
WIX_GUID_NAMESPACE = uuid.UUID("6f0d9a52-3b04-5f2d-9a3f-2a2a0a4c7f11")
# MSI Identifier columns hold 72 characters. The stem plus the "_" and the eight
# hexadecimal digits that disambiguate two paths sharing a slug must fit inside
# that ceiling, and File ids reuse the component stem, so the budget is shared.
WIX_IDENTIFIER_LIMIT = 72
WIX_IDENTIFIER_STEM = WIX_IDENTIFIER_LIMIT - 9
STAGE_RECEIPTS = ("stage.json", "browser-msi-plan.json", "browser-components.wxs")
CODEC_SYMBOLS = {
    "h264": "ff_h264_decoder",
    "hevc": "ff_hevc_decoder",
    "aac": "ff_aac_decoder",
    "mp3": "ff_mp3_decoder",
    "flac": "ff_flac_decoder",
    "vorbis": "ff_vorbis_decoder",
}
RESTRICTED_CODECS = ("h264", "hevc", "aac")
# The signed Windows CEF archive is stripped, so the `ff_*_decoder` symbol names
# the Linux probe reads are absent from libcef.dll. FFmpeg keeps each decoder's
# AVCodec.long_name string inside the decoder struct itself, which is only
# compiled in when that decoder is enabled, so the long name is the equivalent
# measurable signal on this artifact.
CODEC_LONG_NAMES = {
    "h264": b"H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10",
    "hevc": b"H.265 / HEVC (High Efficiency Video Coding)",
    "aac": b"AAC (Advanced Audio Coding)",
    "mp3": b"MP3 (MPEG audio layer 3)",
    "flac": b"FLAC (Free Lossless Audio Codec)",
    "vorbis": b"Vorbis",
}
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


def runtime_digest(root):
    result = hashlib.sha256()
    excluded = {"verified-manifest.sha256", "elf-audit.json", "windows-audit.json"}
    paths = (
        item for item in root.rglob("*")
        if item.is_file() and not item.is_symlink()
        and not (len(item.relative_to(root).parts) == 1 and item.name in excluded)
    )
    for path in sorted(paths, key=lambda item: item.relative_to(root).as_posix()):
        result.update(path.relative_to(root).as_posix().encode() + b"\0")
        result.update(str(path.stat().st_size).encode() + b"\0")
        result.update(digest(path).encode() + b"\n")
    return result.hexdigest()


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
    for name, expected in entry.get("files", {}).items():
        path = root / name
        if not path.is_file() or digest(path) != expected:
            raise ValueError(f"runtime file missing or checksum mismatch: {name}")
    expected_runtime = entry.get("runtime_sha256")
    if expected_runtime and runtime_digest(root) != expected_runtime:
        raise ValueError("runtime tree checksum mismatch")
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


def pe_sections(data):
    if data[:2] != b"MZ" or len(data) < 0x40:
        return None
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        return None
    coff = pe + 4
    machine, section_count = struct.unpack_from("<HH", data, coff)
    optional_size = struct.unpack_from("<H", data, coff + 16)[0]
    optional = coff + 20
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic != 0x20B:
        return machine, [], 0, 0
    directory_count = struct.unpack_from("<I", data, optional + 108)[0]
    table = optional + optional_size
    layout = [struct.unpack_from("<IIII", data, table + index * 40 + 8)
              for index in range(section_count)]
    return machine, layout, optional + 112, directory_count


def pe_offset(layout, rva):
    for virtual_size, virtual_address, raw_size, raw_offset in layout:
        span = max(virtual_size, raw_size)
        if virtual_address <= rva < virtual_address + span:
            return raw_offset + (rva - virtual_address)
    return None


def pe_report(path):
    data = path.read_bytes()
    parsed = pe_sections(data)
    if parsed is None:
        return None
    machine, layout, directories, directory_count = parsed
    names = []
    if directory_count >= 2:
        import_rva = struct.unpack_from("<I", data, directories + 8)[0]
        cursor = pe_offset(layout, import_rva) if import_rva else None
        while cursor is not None and cursor + 20 <= len(data):
            name_rva = struct.unpack_from("<I", data, cursor + 12)[0]
            if name_rva == 0:
                break
            offset = pe_offset(layout, name_rva)
            if offset is None:
                break
            end = data.find(b"\0", offset)
            if end < 0:
                break
            names.append(data[offset:end].decode("ascii", "replace").lower())
            cursor += 20
    return {"machine": machine, "imports": sorted(set(names))}


def windows_import_report(prefix):
    bundled = {path.name.lower() for path in tree_files(prefix)
               if path.suffix.lower() in WINDOWS_PE_SUFFIXES}
    foreign = {}
    redistributable = set()
    wrong_architecture = []
    for path in tree_files(prefix):
        if path.suffix.lower() not in WINDOWS_PE_SUFFIXES:
            continue
        report = pe_report(path)
        if report is None:
            wrong_architecture.append(str(path.relative_to(prefix)))
            continue
        if report["machine"] != 0x8664:
            wrong_architecture.append(str(path.relative_to(prefix)))
        for name in report["imports"]:
            if name in bundled or name in WINDOWS_SYSTEM_DLLS:
                continue
            if name.startswith(("api-ms-win-", "ext-ms-win-")):
                continue
            if name in WINDOWS_REDISTRIBUTABLE_DLLS:
                redistributable.add(name)
                continue
            foreign.setdefault(name, []).append(str(path.relative_to(prefix)))
    return {
        "foreign": {name: sorted(users) for name, users in sorted(foreign.items())},
        "redistributable": sorted(redistributable),
        "wrong_architecture": sorted(wrong_architecture),
    }


def windows_codec_report(runtime):
    libcef = runtime / "Release/libcef.dll"
    if not libcef.is_file():
        return {}
    return {name: contains(libcef, needle) for name, needle in CODEC_LONG_NAMES.items()}


def escaped_paths(prefix):
    escaped = []
    for path in tree_files(prefix):
        relative = path.relative_to(prefix)
        if relative.is_absolute() or ".." in relative.parts or relative.parts[0] == "":
            escaped.append(str(relative))
    return escaped


def wix_identifier(kind, relative):
    slug = re.sub(r"[^A-Za-z0-9_]", "_", relative.as_posix())
    return f"{kind}_{slug}"[:WIX_IDENTIFIER_STEM] + "_" + hashlib.sha256(
        relative.as_posix().encode()).hexdigest()[:8]


def packaged_files(prefix):
    return [path for path in tree_files(prefix)
            if not (len(path.relative_to(prefix).parts) == 1 and path.name in STAGE_RECEIPTS)]


def msi_components(prefix):
    directories = {}
    for path in packaged_files(prefix):
        relative = path.relative_to(prefix)
        directories.setdefault(relative.parent, []).append(relative)
    return {parent: sorted(files) for parent, files in sorted(directories.items())}


def msi_plan_document(prefix, target, manifest, entry, manifest_digest):
    components = msi_components(prefix)
    files = [
        {
            "path": relative.as_posix(),
            "directory": parent.as_posix(),
            "component": wix_identifier("cmp", relative),
            "sha256": digest(prefix / relative),
            "bytes": (prefix / relative).stat().st_size,
        }
        for parent, entries in components.items()
        for relative in entries
    ]
    signed = [item["path"] for item in files
              if Path(item["path"]).name.lower().startswith("paneflow-browser-host.")]
    return {
        "schema_version": 1,
        "target": target,
        "format": "msi",
        "component_group": WIX_COMPONENT_GROUP,
        "directory_root": WIX_DIRECTORY_ROOT,
        "manifest_sha256": manifest_digest,
        "runtime_sha256": entry.get("runtime_sha256", entry["sha256"]),
        "cef_version": manifest["cef_version"],
        "install_scope": "perMachine",
        "signed_components": signed,
        "directories": [parent.as_posix() for parent in components],
        "files": files,
        "installed_bytes": browser_bytes(prefix),
    }


def wix_fragment(plan):
    directories = {}
    for item in plan["files"]:
        directories.setdefault(item["directory"], []).append(item)
    lines = [
        "<?xml version='1.0' encoding='utf-8'?>",
        f"<Wix xmlns='{WIX_NAMESPACE}'>",
        "    <Fragment>",
        f"        <DirectoryRef Id='{plan['directory_root']}'>",
    ]
    tree = {}
    for directory in sorted(directories):
        parts = [] if directory == "." else directory.split("/")
        node = tree
        for part in parts:
            node = node.setdefault(part, {})
        node.setdefault("", []).extend(directories[directory])

    def emit(node, path, indent):
        for item in node.get("", []):
            lines.append(
                f"{indent}<Component Id='{item['component']}' "
                f"Guid='{uuid.uuid5(WIX_GUID_NAMESPACE, item['path'])}'>"
            )
            lines.append(
                f"{indent}    <File Id='fil_{item['component'][4:]}' "
                f"Name='{Path(item['path']).name}' DiskId='1' "
                f"Source='$(var.BrowserStage)\\{item['path'].replace('/', chr(92))}' "
                "KeyPath='yes'/>"
            )
            lines.append(f"{indent}</Component>")
        for name in sorted(key for key in node if key != ""):
            identifier = wix_identifier("dir", Path(path) / name if path else Path(name))
            lines.append(f"{indent}<Directory Id='{identifier}' Name='{name}'>")
            emit(node[name], f"{path}/{name}" if path else name, indent + "    ")
            lines.append(f"{indent}</Directory>")

    emit(tree, "", "            ")
    lines.append("        </DirectoryRef>")
    lines.append(
        f"        <ComponentGroup Id='{plan['component_group']}'>"
    )
    for item in plan["files"]:
        lines.append(f"            <ComponentRef Id='{item['component']}'/>")
    lines.append("        </ComponentGroup>")
    lines.append("    </Fragment>")
    lines.append("</Wix>")
    return "\n".join(lines) + "\n"


def write_msi_plan(prefix, target, manifest, entry, manifest_digest):
    plan = msi_plan_document(prefix, target, manifest, entry, manifest_digest)
    (prefix / "browser-msi-plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    (prefix / "browser-components.wxs").write_text(wix_fragment(plan))
    return plan


def msi_plan(args):
    manifest, manifest_digest = load_manifest(args.manifest)
    entry = target_entry(manifest, args.target)
    if entry.get("platform") != "windows":
        raise ValueError(f"the MSI plan applies to Windows targets only, not {args.target}")
    plan = write_msi_plan(args.prefix.resolve(), args.target, manifest, entry, manifest_digest)
    print(json.dumps({key: value for key, value in plan.items() if key != "files"}, indent=2))


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
    windows = entry.get("platform") == "windows"
    client = host.with_suffix(".dll") if windows else None
    if not host.is_file() or (not windows and not os.access(host, os.X_OK)):
        raise ValueError(
            f"host binary absent at {host}: build it with "
            f"cargo build --release --locked -p paneflow-browser-host --features cef-runtime"
        )
    if windows and (client is None or not client.is_file()):
        raise ValueError(f"Windows client DLL absent next to host binary: {host}")
    destination = args.destination.resolve()
    runtime = destination / RUNTIME_SUBDIR
    if destination.exists():
        shutil.rmtree(destination)
    runtime_names = tree_files(source) if windows else [source / name for name in entry.get("files", {})]
    for source_path in runtime_names:
        name = source_path.relative_to(source) if windows else source_path.relative_to(source)
        target_path = runtime / name
        target_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_path, target_path)
        if not windows:
            target_path.chmod(0o755 if is_elf(target_path) else 0o644)
    (runtime / STAMP).write_text(manifest_digest + "\n")
    if not windows:
        (runtime / STAMP).chmod(0o644)
        sandbox = runtime / "Release/chrome-sandbox"
        sandbox.chmod(0o4755)
    host_target = destination / (HOST_SUBDIR.with_suffix(".exe") if windows else HOST_SUBDIR)
    host_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(host, host_target)
    if not windows:
        host_target.chmod(0o755)
    client_target = None
    if windows:
        client_target = destination / HOST_SUBDIR.with_suffix(".dll")
        shutil.copyfile(client, client_target)
    docs = destination / DOC_SUBDIR
    docs.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "native/browser/BROWSER_THIRD_PARTY_NOTICES.md", docs / "BROWSER_THIRD_PARTY_NOTICES.md")
    (docs / "BROWSER_THIRD_PARTY_NOTICES.md").chmod(0o644)
    write_sbom(destination, args.target, "stage", manifest, entry, manifest_digest,
               docs / "browser-sbom.json")
    if windows:
        write_msi_plan(destination, args.target, manifest, entry, manifest_digest)
    receipt = {
        "schema_version": 1,
        "target": args.target,
        "cef_version": manifest["cef_version"],
        "chromium_version": manifest["chromium_version"],
        "manifest_sha256": manifest_digest,
        "runtime_sha256": entry.get("runtime_sha256", entry["sha256"]),
        "availability": entry["availability"],
        "native_qualification": entry["native_qualification"],
        "host_sha256": digest(host_target),
        "client_sha256": digest(client_target) if client_target else None,
        "runtime_files": len(tree_files(runtime)),
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
    if target.endswith("-pc-windows-msvc"):
        candidates = (
            ROOT / "target" / target / "release/paneflow-browser-host.exe",
            ROOT / "target/release/paneflow-browser-host.exe",
        )
        return next((candidate for candidate in candidates if candidate.is_file()), candidates[0])
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


def inspect_windows(prefix, package_format, target, manifest, entry, manifest_digest):
    runtime = prefix / RUNTIME_SUBDIR
    host = prefix / HOST_SUBDIR.with_suffix(".exe")
    client = prefix / HOST_SUBDIR.with_suffix(".dll")
    report = []
    required = entry.get("files", {})
    mismatched = [name for name, expected in required.items()
                  if not (runtime / name).is_file() or digest(runtime / name) != expected]
    check(report, "layout", runtime.is_dir() and host.is_file() and client.is_file(),
          f"runtime {runtime}, bootstrap {host}, client {client}")
    check(report, "runtime_digests", not mismatched,
          f"{len(required) - len(mismatched)}/{len(required)} pinned files match"
          + (f"; first mismatch {mismatched[0]}" if mismatched else ""))
    expected_runtime = entry.get("runtime_sha256")
    actual_runtime = runtime_digest(runtime) if runtime.is_dir() else ""
    check(report, "runtime_tree_digest", not expected_runtime or actual_runtime == expected_runtime,
          f"runtime tree {actual_runtime or 'absent'} against {expected_runtime or 'not pinned'}")
    stamp = (runtime / STAMP).read_text().strip() if (runtime / STAMP).is_file() else ""
    check(report, "manifest_stamp", stamp == manifest_digest,
          f"stamp {stamp or 'absent'} against embedded manifest {manifest_digest}")
    if host.is_file():
        with host.open("rb") as stream:
            pe = stream.read(2) == b"MZ"
        check(report, "bootstrap_executable", host.suffix.lower() == ".exe" and pe,
              f"host {host} uses the Windows bootstrap executable contract")
    if client.is_file():
        with client.open("rb") as stream:
            pe = stream.read(2) == b"MZ"
        check(report, "client_library", client.suffix.lower() == ".dll" and pe,
              f"client {client} uses the Windows DLL contract")
    metadata_path = runtime / "cef_version.json"
    metadata = json.loads(metadata_path.read_text()) if metadata_path.is_file() else {}
    check(report, "runtime_metadata",
          metadata.get("platform") == "windows64" and metadata.get("version_full") == manifest["cef_version"],
          f"cef_version.json {metadata.get('version_full', 'absent')}")
    expected_abi = entry.get("abi_hash")
    check(report, "runtime_abi", not expected_abi or metadata.get("abi_hash") == expected_abi,
          f"ABI {metadata.get('abi_hash', 'absent')} against {expected_abi or 'not pinned'}")
    credits = runtime / "CREDITS.html"
    license_path = runtime / "LICENSE.txt"
    check(report, "chromium_credits", credits.is_file(), f"Chromium credits document {credits}")
    check(report, "runtime_license", license_path.is_file(), f"CEF license document {license_path}")
    checkout = str(ROOT).encode()
    leaking = [str(path.relative_to(prefix)) for path in tree_files(prefix)
               if path.suffix not in {".pak", ".dat", ".bin", ".json", ".md", ".html", ".txt"}
               and contains(path, checkout)]
    check(report, "no_checkout_dependency", not leaking,
          f"binaries referencing {ROOT}: {leaking or 'none'}")
    total = browser_bytes(prefix)
    check(report, "installed_budget", total <= INSTALLED_BUDGET,
          f"{total} bytes of browser payload against the NFR-13 {INSTALLED_BUDGET} byte ceiling")
    notices = prefix / DOC_SUBDIR / "BROWSER_THIRD_PARTY_NOTICES.md"
    check(report, "notices", notices.is_file(), f"{notices}")
    imports = windows_import_report(prefix)
    check(report, "runtime_architecture", not imports["wrong_architecture"],
          f"non-x86_64 or unreadable images: {imports['wrong_architecture'] or 'none'}")
    check(report, "runtime_imports", not imports["foreign"],
          "every imported module is bundled, a Windows system DLL or the Microsoft C runtime; "
          f"redistributables {imports['redistributable'] or 'none'}, "
          f"unresolved {sorted(imports['foreign']) or 'none'}")
    escaped = escaped_paths(prefix)
    check(report, "bundle_paths", not escaped,
          f"staged files outside the bundle prefix: {escaped or 'none'}")
    declared_codecs = entry.get("codecs", {})
    measured_codecs = windows_codec_report(runtime)
    codec_drift = sorted(name for name in CODEC_LONG_NAMES
                         if declared_codecs.get(name) != measured_codecs.get(name))
    check(report, "runtime_codecs", bool(measured_codecs) and not codec_drift,
          f"declared {declared_codecs or 'absent'} against measured {measured_codecs or 'absent'}"
          + (f"; drift {codec_drift}" if codec_drift else ""))
    if package_format == "msi":
        plan_path = prefix / "browser-msi-plan.json"
        plan = json.loads(plan_path.read_text()) if plan_path.is_file() else {}
        planned = {item["path"]: item["sha256"] for item in plan.get("files", [])}
        staged = {path.relative_to(prefix).as_posix(): digest(path)
                  for path in packaged_files(prefix)}
        drifted = sorted(name for name in set(planned) | set(staged)
                         if planned.get(name) != staged.get(name))
        check(report, "msi_plan", bool(planned) and not drifted,
              f"{len(planned)} planned components against {len(staged)} staged files"
              + (f"; first drift {drifted[0]}" if drifted else ""))
        fragment = prefix / "browser-components.wxs"
        check(report, "msi_fragment",
              fragment.is_file() and WIX_COMPONENT_GROUP in fragment.read_text(),
              f"WiX fragment {fragment} declares the {WIX_COMPONENT_GROUP} component group")
        check(report, "msi_signed_components",
              sorted(plan.get("signed_components", [])) == sorted(
                  path.relative_to(prefix).as_posix() for path in packaged_files(prefix)
                  if path.name.lower().startswith("paneflow-browser-host.")),
              f"components requiring a Paneflow signature: {plan.get('signed_components', [])}")
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


def inspect(prefix, package_format, target, manifest, entry, manifest_digest, archive=None, baseline=None):
    if entry.get("platform") == "windows":
        return inspect_windows(prefix, package_format, target, manifest, entry, manifest_digest)
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
    libcef = runtime / ("Release/libcef.dll" if entry.get("platform") == "windows" else "Release/libcef.so")
    if not libcef.is_file():
        raise ValueError(f"no staged runtime under {runtime}")
    codecs = entry.get("codecs", {})
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
        "runtime_sha256": entry.get("runtime_sha256", entry["sha256"]),
        "availability": entry["availability"],
        "native_qualification": entry["native_qualification"],
        "hardening": entry.get("hardening", {}),
        "provenance": entry.get("provenance", {}),
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
    parser = argparse.ArgumentParser(description="Stage, verify and inventory the distributed browser runtime")
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
    verification.add_argument("--format", choices=["deb", "rpm", "appimage", "targz", "msi", "stage"], required=True)
    verification.add_argument("--archive", type=Path)
    verification.add_argument("--baseline", type=Path)
    verification.set_defaults(handler=verify)

    inventory = commands.add_parser("sbom")
    inventory.add_argument("--prefix", type=Path, default=ROOT / "target/browser-stage")
    inventory.add_argument("--format", choices=["deb", "rpm", "appimage", "targz", "msi"], required=True)
    inventory.add_argument("--output", type=Path, default=ROOT / "target/browser-stage/browser-sbom.json")
    inventory.set_defaults(handler=sbom)

    planning = commands.add_parser("msi-plan")
    planning.add_argument("--prefix", type=Path, default=ROOT / "target/browser-stage")
    planning.set_defaults(handler=msi_plan)

    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "REJECTED", "error": str(error)}), file=sys.stderr)
        raise SystemExit(1) from None
