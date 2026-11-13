import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


class SafetyTests(unittest.TestCase):
    def test_verifier_rejects_optimized_python_before_sdk_access(self):
        result = subprocess.run(
            [sys.executable, "-O", str(SCRIPTS / "verify-android-apk.py"), "missing.apk"],
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("requires assertions", result.stderr)

    def test_smoke_rejects_optimized_python_without_touching_a_device(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            result = subprocess.run(
                [sys.executable, "-O", str(SCRIPTS / "android-smoke.py"),
                 "--serial", "not-a-device", "--apks", directory,
                 "--fixtures", directory, "--output", str(output)],
                text=True, capture_output=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("requires assertions", result.stderr)
            self.assertFalse(output.exists())

    def test_smoke_uses_only_reserved_test_ids(self):
        smoke = module("smoke", "android-smoke.py")
        production = "org.ripleyeldridge.rustymusicsheets"
        self.assertNotIn(production, smoke.PACKAGES)
        self.assertEqual(smoke.APP, production + ".smoketest")
        self.assertEqual(smoke.HELPER, production + ".fixture")

    def test_provider_is_private_and_has_no_storage_permission(self):
        manifest = ET.parse(ROOT / "android/test-harness/AndroidManifest.xml")
        android = "{http://schemas.android.com/apk/res/android}"
        self.assertEqual(manifest.findall("uses-permission"), [])
        provider = manifest.find("application/provider")
        self.assertEqual(provider.attrib[android + "exported"], "false")
        self.assertEqual(provider.attrib[android + "grantUriPermissions"], "true")

    def test_fixture_generator_refuses_unowned_nonempty_output(self):
        with tempfile.TemporaryDirectory() as directory:
            keep = Path(directory) / "keep.txt"
            keep.write_text("Not a generated fixture")
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / "generate-android-fixtures.py"), "--output", directory],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertEqual(keep.read_text(), "Not a generated fixture")


@unittest.skipUnless(all(shutil.which(tool) for tool in ("pdftoppm", "pdfinfo", "pdftotext")), "Poppler required")
class FixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        subprocess.run(
            [sys.executable, str(SCRIPTS / "generate-android-fixtures.py"), "--output", str(cls.root)],
            check=True, stdout=subprocess.DEVNULL,
        )
        cls.manifest = json.loads((cls.root / "fixtures.json").read_text())

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_manifest_covers_all_original_fixtures_and_hashes(self):
        self.assertEqual(self.manifest["generator"], "rustymusicsheets-original-fixtures-v1")
        self.assertEqual(len(self.manifest["files"]), 8)
        for entry in self.manifest["files"]:
            data = (self.root / entry["name"]).read_bytes()
            self.assertEqual(len(data), entry["bytes"])
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry["sha256"])

    def test_text_and_multipage_pdf_are_readable(self):
        text = subprocess.check_output(["pdftotext", str(self.root / "text-score.pdf"), "-"], text=True)
        self.assertIn("Cello", text)
        self.assertIn("Piano", text)
        info = subprocess.check_output(["pdfinfo", str(self.root / "multipage.pdf")], text=True)
        self.assertRegex(info, r"Pages:\s+3")

    def test_scan_has_no_embedded_text(self):
        text = subprocess.check_output(["pdftotext", str(self.root / "scanned.pdf"), "-"], text=True)
        self.assertFalse(text.strip())

    def test_unicode_fixture_is_distinct_valid_png_encoding(self):
        data = (self.root / "Étude Unicode.png").read_bytes()
        self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertNotEqual(data, (self.root / "score.png").read_bytes())
        self.assertIn("Étude".encode("latin-1"), data)


if __name__ == "__main__":
    unittest.main()
