"""Native audio builds are explicit, offline with respect to source dependencies, and optional."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


build = load("audio_build", "build-audio-host.py")
package = load("audio_package", "build-deb.py")


class AudioBuildTests(unittest.TestCase):
    def test_core_only_build_and_tests_are_explicit_and_never_install(self):
        with mock.patch("sys.argv", ["build-audio-host.py", "--core-only", "--test",
                                     "--build-dir", str(ROOT / "target" / "unit-core")]), \
                mock.patch.object(build.shutil, "which", return_value="/usr/bin/tool"), \
                mock.patch.object(build.subprocess, "run") as run, \
                mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(build.main(), 0)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(len(commands), 3)
        self.assertIn("-DRMS_CORE_ONLY=ON", commands[0])
        self.assertEqual(commands[1][0:2], ["cmake", "--build"])
        self.assertEqual(commands[2][0], "ctest")
        self.assertFalse(any(arg in ("sudo", "apt", "apt-get", "curl", "wget")
                             for command in commands for arg in command))

    def test_live_build_can_use_explicit_validation_headers_without_installing(self):
        with mock.patch("sys.argv", ["build-audio-host.py"]), \
                mock.patch.object(build.shutil, "which", return_value="/usr/bin/tool"), \
                mock.patch.object(build.subprocess, "run") as run, \
                mock.patch.dict(os.environ, {"FLUIDSYNTH_INCLUDE_DIR": "/tmp/headers",
                                             "FLUIDSYNTH_LIBRARY": "/usr/lib/libfluidsynth.so.3"}, clear=True):
            self.assertEqual(build.main(), 0)
        configure = run.call_args_list[0].args[0]
        self.assertIn("-DRMS_CORE_ONLY=OFF", configure)
        self.assertIn("-DFLUIDSYNTH_INCLUDE_DIR=/tmp/headers", configure)
        self.assertEqual(run.call_count, 2)

    def test_missing_tools_and_failed_configure_do_not_continue(self):
        with mock.patch("sys.argv", ["build-audio-host.py"]), \
                mock.patch.object(build.shutil, "which", return_value=None), \
                mock.patch.object(build.subprocess, "run") as run, \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                build.main()
            run.assert_not_called()
        with mock.patch("sys.argv", ["build-audio-host.py", "--test"]), \
                mock.patch.object(build.shutil, "which", return_value="/usr/bin/tool"), \
                mock.patch.object(build.subprocess, "run",
                                  side_effect=subprocess.CalledProcessError(17, ["cmake"])) as run:
            self.assertEqual(build.main(), 17)
            self.assertEqual(run.call_count, 1)

    def test_vendored_json_matches_pinned_provenance(self):
        vendor = ROOT / "native/vendor/nlohmann"
        provenance = json.loads((vendor / "provenance.json").read_text())
        self.assertIn("3.11.3", json.dumps(provenance))
        digest = hashlib.sha256((vendor / "json.hpp").read_bytes()).hexdigest()
        self.assertIn(digest, json.dumps(provenance))
        self.assertIn(digest, (ROOT / "native/audio-host/CMakeLists.txt").read_text())
        self.assertTrue((vendor / "LICENSE.MIT").is_file())
        for entry in provenance["files"]:
            self.assertEqual(
                hashlib.sha256((vendor / entry["path"]).read_bytes()).hexdigest(),
                entry["sha256"],
            )


class AudioPackageTests(unittest.TestCase):
    def test_both_helper_and_app_need_matching_native_elf_architecture(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "rms-audio-host"
            with self.assertRaisesRegex(ValueError, "missing"):
                package.validate_native_binary(binary, "amd64")
            binary.write_bytes(b"test ELF stand-in")
            with mock.patch.object(package, "output", return_value="  Machine: Advanced Micro Devices X86-64\n"):
                package.validate_native_binary(binary, "amd64")
                with self.assertRaisesRegex(ValueError, "architecture"):
                    package.validate_native_binary(binary, "arm64")
            with mock.patch.object(package, "output", return_value="  Machine: AArch64\n"):
                package.validate_native_binary(binary, "arm64")
                with self.assertRaisesRegex(ValueError, "architecture"):
                    package.validate_native_binary(binary, "unknown")


if __name__ == "__main__":
    unittest.main()
