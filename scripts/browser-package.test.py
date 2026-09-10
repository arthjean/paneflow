import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
import xml.etree.ElementTree as ElementTree


SCRIPT = Path(__file__).with_name("browser-package.py")
WIX = "{http://schemas.microsoft.com/wix/2006/wi}"


def load_module():
    spec = importlib.util.spec_from_file_location("browser_package", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


packaging = load_module()


def pe_image(imports, machine=0x8664):
    image = bytearray(0x800)
    image[0:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, 0x80)
    image[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", image, 0x84, machine, 1)
    struct.pack_into("<H", image, 0x94, 0xF0)
    struct.pack_into("<H", image, 0x98, 0x20B)
    struct.pack_into("<I", image, 0x104, 16)
    struct.pack_into("<I", image, 0x110, 0x1100 if imports else 0)
    section = 0x188
    image[section:section + 5] = b".text"
    struct.pack_into("<IIII", image, section + 8, 0x400, 0x1000, 0x400, 0x400)
    descriptors = 0x500
    names = 0x600
    cursor = names
    for index, name in enumerate(imports):
        struct.pack_into("<I", image, descriptors + index * 20 + 12, 0x1000 + (cursor - 0x400))
        encoded = name.encode() + b"\0"
        image[cursor:cursor + len(encoded)] = encoded
        cursor += len(encoded)
    return bytes(image)


class WindowsBundlePlanTests(unittest.TestCase):
    def prefix(self, directory, host_imports=("kernel32.dll", "libcef.dll"),
               codecs=("mp3", "flac", "vorbis")):
        prefix = Path(directory) / "browser-stage"
        runtime = prefix / packaging.RUNTIME_SUBDIR / "Release"
        locales = prefix / packaging.RUNTIME_SUBDIR / "Resources/locales"
        docs = prefix / packaging.DOC_SUBDIR
        for folder in (runtime, locales, docs):
            folder.mkdir(parents=True, exist_ok=True)
        (runtime / "libcef.dll").write_bytes(
            pe_image(["kernel32.dll"]) + b"".join(packaging.CODEC_LONG_NAMES[name]
                                                  for name in codecs)
        )
        (locales / "en-US.pak").write_bytes(b"pak")
        (prefix / packaging.RUNTIME_SUBDIR / "cef_version.json").write_text(
            json.dumps({"platform": "windows64", "version_full": "1.2.3", "abi_hash": "abi"})
        )
        (prefix / packaging.RUNTIME_SUBDIR / "CREDITS.html").write_text("<html/>")
        (prefix / packaging.RUNTIME_SUBDIR / "LICENSE.txt").write_text("license")
        (docs / "BROWSER_THIRD_PARTY_NOTICES.md").write_text("notices")
        host = prefix / packaging.HOST_SUBDIR
        host.with_suffix(".exe").write_bytes(pe_image(list(host_imports)))
        host.with_suffix(".dll").write_bytes(pe_image(list(host_imports)))
        stamp = prefix / packaging.RUNTIME_SUBDIR / packaging.STAMP
        stamp.write_text("manifest-digest\n")
        return prefix

    def manifest(self, prefix):
        libcef = prefix / packaging.RUNTIME_SUBDIR / "Release/libcef.dll"
        entry = {
            "platform": "windows",
            "availability": "development",
            "native_qualification": "control_only_windows11",
            "abi_hash": "abi",
            "sha256": "archive-digest",
            "codecs": packaging.windows_codec_report(prefix / packaging.RUNTIME_SUBDIR),
            "runtime_sha256": packaging.runtime_digest(prefix / packaging.RUNTIME_SUBDIR),
            "files": {"Release/libcef.dll": packaging.digest(libcef)},
        }
        manifest = {
            "cef_version": "1.2.3",
            "cef_license": "BSD-3-Clause",
            "cef_commit": "cef-commit",
            "chromium_version": "1.2.3.4",
            "cef_rs_version": "0.1.0",
            "cef_rs_license": "Apache-2.0",
            "cef_rs_commit": "cef-rs-commit",
        }
        return manifest, entry

    def inspect(self, prefix, package_format="msi"):
        manifest, entry = self.manifest(prefix)
        return packaging.inspect_windows(
            prefix, package_format, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
        )

    def statuses(self, verdict):
        return {item["check"]: item["status"] for item in verdict["checks"]}

    def test_plan_covers_every_packaged_file_and_excludes_stage_receipts(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            (prefix / "stage.json").write_text("{}")
            manifest, entry = self.manifest(prefix)
            plan = packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            paths = {item["path"] for item in plan["files"]}
            self.assertNotIn("stage.json", paths)
            self.assertNotIn("browser-msi-plan.json", paths)
            self.assertNotIn("browser-components.wxs", paths)
            self.assertEqual(
                paths,
                {path.relative_to(prefix).as_posix()
                 for path in packaging.packaged_files(prefix)},
            )
            self.assertEqual(
                sorted(plan["signed_components"]),
                [
                    "lib/paneflow/paneflow-browser-host.dll",
                    "lib/paneflow/paneflow-browser-host.exe",
                ],
            )
            identifiers = [item["component"] for item in plan["files"]]
            self.assertEqual(len(identifiers), len(set(identifiers)))

    def test_generated_fragment_declares_one_component_per_file(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            manifest, entry = self.manifest(prefix)
            plan = packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            tree = ElementTree.parse(prefix / "browser-components.wxs")
            components = tree.findall(f".//{WIX}Component")
            files = tree.findall(f".//{WIX}File")
            groups = tree.findall(f".//{WIX}ComponentGroup")
            self.assertEqual(len(components), len(plan["files"]))
            self.assertEqual(len(files), len(plan["files"]))
            self.assertEqual([group.get("Id") for group in groups], ["BrowserRuntime"])
            self.assertEqual(
                len({component.get("Guid") for component in components}), len(components)
            )
            for element in files:
                self.assertTrue(element.get("Source").startswith("$(var.BrowserStage)\\"))
            root = tree.find(f".//{WIX}DirectoryRef")
            self.assertEqual(root.get("Id"), "APPLICATIONFOLDER")

    def test_a_coherent_windows_bundle_passes_every_plan_check(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            manifest, entry = self.manifest(prefix)
            packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            verdict = self.inspect(prefix)
            self.assertEqual(verdict["status"], "PASS", verdict["checks"])

    def test_a_foreign_import_keeps_the_bundle_undistributable(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory, host_imports=("kernel32.dll", "helper.dll"))
            manifest, entry = self.manifest(prefix)
            packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            report = packaging.windows_import_report(prefix)
            self.assertIn("helper.dll", report["foreign"])
            verdict = self.inspect(prefix)
            self.assertEqual(verdict["status"], "FAIL")
            self.assertEqual(self.statuses(verdict)["runtime_imports"], "FAIL")

    def test_the_microsoft_c_runtime_is_reported_as_a_redistributable(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory, host_imports=("kernel32.dll", "vcruntime140.dll"))
            report = packaging.windows_import_report(prefix)
            self.assertEqual(report["foreign"], {})
            self.assertEqual(report["redistributable"], ["vcruntime140.dll"])

    def test_a_wrong_architecture_image_keeps_the_bundle_undistributable(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            host = prefix / packaging.HOST_SUBDIR
            host.with_suffix(".exe").write_bytes(pe_image(["kernel32.dll"], machine=0xAA64))
            manifest, entry = self.manifest(prefix)
            packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            verdict = self.inspect(prefix)
            self.assertEqual(verdict["status"], "FAIL")
            self.assertEqual(self.statuses(verdict)["runtime_architecture"], "FAIL")

    def test_a_payload_edited_after_planning_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            manifest, entry = self.manifest(prefix)
            packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            locale = prefix / packaging.RUNTIME_SUBDIR / "Resources/locales/en-US.pak"
            locale.write_bytes(b"tampered")
            verdict = self.inspect(prefix)
            self.assertEqual(verdict["status"], "FAIL")
            self.assertEqual(self.statuses(verdict)["msi_plan"], "FAIL")

    def test_a_missing_plan_blocks_the_msi_format(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            verdict = self.inspect(prefix)
            self.assertEqual(self.statuses(verdict)["msi_plan"], "FAIL")
            self.assertEqual(self.statuses(verdict)["msi_fragment"], "FAIL")

    def test_the_plan_is_stable_across_regenerations(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            manifest, entry = self.manifest(prefix)
            first = packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            fragment = (prefix / "browser-components.wxs").read_text()
            second = packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            self.assertEqual(first, second)
            self.assertEqual(fragment, (prefix / "browser-components.wxs").read_text())

    def test_restricted_codecs_are_measured_from_the_runtime_and_must_match(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory, codecs=("h264", "aac", "mp3"))
            measured = packaging.windows_codec_report(prefix / packaging.RUNTIME_SUBDIR)
            self.assertEqual(
                measured,
                {"h264": True, "hevc": False, "aac": True,
                 "mp3": True, "flac": False, "vorbis": False},
            )
            manifest, entry = self.manifest(prefix)
            packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            self.assertEqual(self.inspect(prefix)["status"], "PASS")
            document = packaging.sbom_document(
                prefix, "x86_64-pc-windows-msvc", "msi", manifest, entry, "manifest-digest"
            )
            self.assertEqual(document["restricted_codecs"], ["aac", "h264"])
            self.assertEqual(document["redistribution"], "review_required")
            entry["codecs"]["h264"] = False
            verdict = packaging.inspect_windows(
                prefix, "msi", "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            self.assertEqual(self.statuses(verdict)["runtime_codecs"], "FAIL")

    def test_component_identifiers_survive_long_locale_paths(self):
        relative = Path("lib/paneflow/browser/Resources/locales/pt-BR_MASCULINE.pak")
        identifier = packaging.wix_identifier("cmp", relative)
        self.assertTrue(identifier.startswith("cmp_"))
        self.assertLessEqual(len(identifier), packaging.WIX_IDENTIFIER_LIMIT)
        self.assertTrue(all(character.isalnum() or character == "_" for character in identifier))
        self.assertEqual(
            identifier.rsplit("_", 1)[1],
            hashlib.sha256(relative.as_posix().encode()).hexdigest()[:8],
        )

    def test_every_generated_identifier_fits_the_msi_identifier_column(self):
        deep = Path("lib/paneflow/browser/Resources/locales/extra-long-regional-variant") / (
            "a" * 120 + ".pak"
        )
        for kind in ("cmp", "dir"):
            self.assertLessEqual(
                len(packaging.wix_identifier(kind, deep)), packaging.WIX_IDENTIFIER_LIMIT
            )
        with tempfile.TemporaryDirectory() as directory:
            prefix = self.prefix(directory)
            manifest, entry = self.manifest(prefix)
            plan = packaging.write_msi_plan(
                prefix, "x86_64-pc-windows-msvc", manifest, entry, "manifest-digest"
            )
            tree = ElementTree.parse(prefix / "browser-components.wxs")
            identifiers = [element.get("Id")
                           for tag in ("Component", "File", "Directory")
                           for element in tree.findall(f".//{WIX}{tag}")]
            self.assertGreaterEqual(len(identifiers), 2 * len(plan["files"]))
            for identifier in identifiers:
                self.assertLessEqual(len(identifier), packaging.WIX_IDENTIFIER_LIMIT)


if __name__ == "__main__":
    unittest.main()
