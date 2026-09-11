from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("verify-windows-qualification-contract.py")
REPO = Path(__file__).parents[1]
CONTRACT = REPO / "native/browser/windows-qualification-contract.toml"
DOCUMENTS = (
    "docs/release/browser-windows.md",
    "docs/browser/windows-usage.md",
    "docs/browser/agent-tools.md",
)
MANIFEST = 'contract_version = 3\n[targets."x86_64-pc-windows-msvc"]\navailability = "{availability}"\n'


class WindowsQualificationContractTests(unittest.TestCase):
    def run_cli(self, contract, root=None):
        command = [sys.executable, str(SCRIPT), "--contract", str(contract)]
        if root is not None:
            command += ["--root", str(root)]
        return subprocess.run(command, capture_output=True, text=True, check=False)

    def stage(self, directory, contract_text, availability="development"):
        root = Path(directory)
        for relative in DOCUMENTS:
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(REPO / relative, destination)
        manifest = root / "native/browser/manifest.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(MANIFEST.format(availability=availability), encoding="utf-8")
        path = root / "native/browser/windows-qualification-contract.toml"
        path.write_text(contract_text, encoding="utf-8")
        return path, root

    def test_the_shipped_contract_matches_the_shipped_manifest(self):
        result = self.run_cli(CONTRACT)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('"status": "CONTRACT_VALID"', result.stdout)
        self.assertIn('"declared_availability": "development"', result.stdout)

    def test_a_promoted_distribution_manifest_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path, root = self.stage(name, CONTRACT.read_text(encoding="utf-8"), "human_qualified")
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("while the Windows verdict allows", result.stdout)

    def test_a_verdict_without_its_matrix_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            text = CONTRACT.read_text(encoding="utf-8").replace(
                'human_release = "NOT_QUALIFIED"', 'human_release = "QUALIFIED"', 1
            )
            path, root = self.stage(name, text)
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("declared QUALIFIED while its matrix is not fully proven", result.stdout)

    def test_an_executed_row_without_evidence_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            text = CONTRACT.read_text(encoding="utf-8").replace(
                'id = "distribution"\nrelease = "human"\nowner = "US-013"\nstatus = "NOT_EXECUTED"',
                'id = "distribution"\nrelease = "human"\nowner = "US-013"\nstatus = "WORKS_MEASURED"',
                1,
            )
            path, root = self.stage(name, text)
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("claims execution without evidence and a machine identity", result.stdout)

    def test_an_unattributed_waiver_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            text = CONTRACT.read_text(encoding="utf-8").replace(
                'waived_by = "Arthur Jean"\n', "", 1
            )
            path, root = self.stage(name, text)
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("is waived without waived_by", result.stdout)

    def test_a_waiver_presenting_native_evidence_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            text = CONTRACT.read_text(encoding="utf-8").replace(
                'waiver_reason = "The owner accepted',
                'observed_on = "a Windows 10 1809 host"\nwaiver_reason = "The owner accepted',
                1,
            )
            path, root = self.stage(name, text)
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("presents native evidence while declared waived", result.stdout)

    def test_partial_coverage_without_a_machine_identity_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            lines = CONTRACT.read_text(encoding="utf-8").splitlines(keepends=True)
            anchor = next(index for index, line in enumerate(lines) if line.startswith("findings = ["))
            del lines[anchor - 1]
            text = "".join(lines)
            path, root = self.stage(name, text)
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("claims partial coverage without a machine identity", result.stdout)

    def test_a_missing_document_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path, root = self.stage(name, CONTRACT.read_text(encoding="utf-8"))
            (root / "docs/browser/windows-usage.md").unlink()
            result = self.run_cli(path, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("does not resolve to a written file", result.stdout)


if __name__ == "__main__":
    unittest.main()
