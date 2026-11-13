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
from unittest.mock import patch
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


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        self.smoke = module("readiness_smoke", "android-smoke.py")

    def state(self, component=None, policy="showing=false\ninputRestricted=false", pid="123"):
        component = component or self.smoke.APP + "/android.app.NativeActivity"
        return self.smoke.window_state(policy, f"mCurrentFocus=Window{{abc u0 {component} Private title}}", pid)

    def test_only_live_unlocked_exact_test_component_is_ready(self):
        self.assertTrue(self.state()["ready"])
        for state in (
            self.state(policy="showing=true"),
            self.state(policy="inputRestricted=true"),
            self.state(component=self.smoke.APP + ".other/.Activity"),
            self.state(component="com.android.launcher3/.Launcher"),
            self.state(pid=""),
            self.state(pid="device offline"),
        ):
            self.assertFalse(state["ready"], state)
        self.assertNotIn("Private title", json.dumps(self.state()))

    def wait(self, states, seconds=2):
        now = [0.0]
        observed = []
        queue = iter(states)
        last = [states[-1]]
        def query():
            last[0] = next(queue, last[0])
            return last[0]
        def sleep(value):
            now[0] += value
        result = self.smoke.wait_for_readiness(
            query, "unit startup", seconds=seconds, sample=observed.append,
            clock=lambda: now[0], sleep=sleep,
        )
        return result, observed

    def test_waits_through_launcher_lock_and_transient_focus(self):
        ready = self.state()
        launcher = self.state(component="com.android.launcher3/.Launcher")
        locked = self.state(policy="showing=true")
        result, observed = self.wait([launcher, locked, ready, launcher, ready, ready])
        self.assertEqual(result, ready)
        self.assertEqual(len(observed), 6)

    def test_persistent_focus_failure_times_out_instead_of_bypassing_guard(self):
        with self.assertRaisesRegex(RuntimeError, "foreground readiness.*unit startup"):
            self.wait([self.state(component="com.android.systemui/.Dialog")], seconds=1)

    def test_adb_error_does_not_count_as_ready(self):
        ready = self.state()
        _, observed = self.wait([{"ready": False, "adb_error": "TimeoutExpired"}, ready, ready])
        self.assertEqual(len(observed), 3)


class SdkViewTests(unittest.TestCase):
    def test_extra_build_tools_cannot_override_the_pin_and_shared_sdk_is_unchanged(self):
        helper = module("sdk_view", "android-sdk-view.py")
        with tempfile.TemporaryDirectory() as directory:
            sdk = Path(directory)
            for name in ("build-tools/36.0.0", "build-tools/37.0.0", "platforms/android-35", "platforms/android-36", "platform-tools"):
                (sdk / name).mkdir(parents=True)
            marker = sdk / "build-tools/37.0.0/keep"
            marker.write_text("Unrelated preinstalled SDK")
            captured = []
            def fake_run(command, env, check):
                view = Path(env["ANDROID_HOME"])
                captured.append(view)
                self.assertEqual(command, ["cargo", "apk", "build"])
                self.assertEqual([p.name for p in (view / "build-tools").iterdir()], ["36.0.0"])
                self.assertEqual((view / "build-tools/36.0.0").resolve(), sdk / "build-tools/36.0.0")
                self.assertTrue("ANDROID_SDK_ROOT" not in env)
                self.assertEqual(env["ANDROID_NDK_ROOT"], "/pinned/ndk")
                return subprocess.CompletedProcess(command, 7)
            with patch.dict(os.environ, {"ANDROID_SDK_ROOT": "/unrelated/sdk", "ANDROID_NDK_ROOT": "/pinned/ndk"}):
                self.assertEqual(helper.run_with_sdk(sdk, ["cargo", "apk", "build"], run=fake_run), 7)
            self.assertFalse(captured[0].exists())
            self.assertEqual(marker.read_text(), "Unrelated preinstalled SDK")

    def test_missing_pinned_package_fails_before_starting_a_build(self):
        helper = module("sdk_view_missing", "android-sdk-view.py")
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "Missing required SDK package"):
                helper.run_with_sdk(Path(directory), ["unused"], run=lambda *a, **k: self.fail("Build was started"))


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
