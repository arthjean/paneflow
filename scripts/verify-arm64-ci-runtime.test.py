import hashlib
import io
import json
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("verify-arm64-ci-runtime.py")


def fake_elf():
    header = bytearray(64)
    header[:4] = b"\x7fELF"
    header[4] = 2
    header[5] = 1
    struct.pack_into("<H", header, 18, 183)
    return bytes(header)


class Arm64RuntimeVerifierTests(unittest.TestCase):
    def make_archive(self, directory, unsafe=False):
        top = "cef_binary_test_linuxarm64_minimal"
        archive = directory / f"{top}.tar.bz2"
        with tarfile.open(archive, "w:bz2") as bundle:
            root = tarfile.TarInfo(top)
            root.type = tarfile.DIRTYPE
            bundle.addfile(root)
            release = tarfile.TarInfo(f"{top}/Release")
            release.type = tarfile.DIRTYPE
            bundle.addfile(release)
            libcef = tarfile.TarInfo(f"{top}/Release/libcef.so")
            payload = fake_elf()
            libcef.size = len(payload)
            bundle.addfile(libcef, io.BytesIO(payload))
            if unsafe:
                evil = tarfile.TarInfo(f"{top}/../evil")
                evil.size = 1
                bundle.addfile(evil, io.BytesIO(b"x"))
        return archive, top

    def write_manifest(self, directory, archive, top):
        text = "\n".join(
            [
                "contract_version = 1",
                'target = "aarch64-unknown-linux-gnu"',
                'cef_version = "test"',
                'chromium_version = "test"',
                f'archive = "{archive.name}"',
                'url = "https://cef-builds.spotifycdn.com/test.tar.bz2"',
                f'archive_sha256 = "{hashlib.sha256(archive.read_bytes()).hexdigest()}"',
                f'archive_sha1 = "{hashlib.sha1(archive.read_bytes()).hexdigest()}"',
                f"archive_size = {archive.stat().st_size}",
                'source = "test"',
                'availability = "development"',
                'native_qualification = "upstream_standard_not_qualified"',
                'evidence_kind = "github_actions_arm64_code_test_build"',
                'ci_runner = "ubuntu-22.04-arm"',
                "max_members = 20",
                "max_unpacked_size = 1000000",
                "max_file_size = 1000000",
            ]
        )
        manifest = directory / "manifest.toml"
        manifest.write_text(text + "\n")
        return manifest

    def run_cli(self, manifest, archive, destination):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--manifest", str(manifest), "--archive", str(archive), "--destination", str(destination)],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_verifies_extracts_and_records_provenance(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            archive, top = self.make_archive(directory)
            manifest = self.write_manifest(directory, archive, top)
            destination = directory / "runtime"
            result = self.run_cli(manifest, archive, destination)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads(result.stdout)["status"], "VERIFIED")
            evidence = json.loads((destination / "arm64-runtime-verification.json").read_text())
            self.assertEqual(evidence["status"], "CI_CODE_TEST_BUILD")
            self.assertEqual(evidence["runtime"]["elf_machine"], "AArch64")
            self.assertEqual((destination / "verified-manifest.sha256").read_text().strip(), hashlib.sha256(manifest.read_bytes()).hexdigest())

    def test_rejects_path_traversal_before_extraction(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            archive, top = self.make_archive(directory, unsafe=True)
            manifest = self.write_manifest(directory, archive, top)
            result = self.run_cli(manifest, archive, directory / "runtime")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe archive path", result.stdout)


if __name__ == "__main__":
    unittest.main()
