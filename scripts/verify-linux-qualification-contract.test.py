from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("verify-linux-qualification-contract.py")
CONTRACT = Path(__file__).parents[1] / "native/browser/linux-qualification-contract.toml"


class LinuxQualificationContractTests(unittest.TestCase):
    def run_cli(self, contract):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--contract", str(contract)],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_current_contract_is_valid(self):
        result = self.run_cli(CONTRACT)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('"status": "CONTRACT_VALID"', result.stdout)

    def test_public_release_promotion_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / "contract.toml"
            path.write_text(CONTRACT.read_text().replace('public_release = "NOT_QUALIFIED"', 'public_release = "human_qualified"'))
            result = self.run_cli(path)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("public release must remain NOT_QUALIFIED", result.stdout)

    def test_missing_distribution_reference_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / "contract.toml"
            path.write_text(CONTRACT.read_text().replace('distributions = ["Ubuntu", "Debian", "Fedora", "Arch", "openSUSE"]', 'distributions = ["Ubuntu"]'))
            result = self.run_cli(path)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("distribution matrix is incomplete", result.stdout)


if __name__ == "__main__":
    unittest.main()
