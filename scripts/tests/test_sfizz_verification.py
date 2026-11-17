import contextlib
import importlib.util
import io
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import wave


SCRIPT = Path(__file__).resolve().parents[2] / "native/audio-host/sfizz_verify.py"


def load_script():
    spec = importlib.util.spec_from_file_location("sfizz_verification", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SfizzVerificationTests(unittest.TestCase):
    def test_generates_private_sample_preset_and_isolated_verifier_environment(self):
        module = load_script()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "fresh"
            original = root / "original.sfz"
            original.write_bytes(b"do not modify")

            def verify(command, *, env, stdout, stderr, timeout):
                self.assertEqual(timeout, 180)
                self.assertEqual(Path(command[0]), root / "verifier")
                self.assertEqual(Path(command[1]), root / "sfizz.vst3")
                preset = Path(command[2]).read_bytes()
                self.assertEqual(preset[:4], b"VST3")
                self.assertEqual(preset[8:40], b"E8FAB71815ED46E38B5983101E12993F")
                offset = struct.unpack_from("<q", preset, 40)[0]
                self.assertEqual(preset[offset:offset + 4], b"List")
                state = preset[48:offset]
                self.assertEqual(struct.unpack_from("<Q", state)[0], 5)
                size = struct.unpack_from("<i", state, 8)[0]
                sfz = Path(state[12:12 + size - 1].decode())
                self.assertEqual(sfz, output / "generated.sfz")
                self.assertIn("sample=generated.wav", sfz.read_text())
                with wave.open(str(output / "generated.wav")) as sample:
                    self.assertEqual(sample.getparams()[:4], (1, 2, 48000, 48000))
                for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
                    private = Path(env[key])
                    self.assertEqual(private.parent, output)
                    self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o700)
                self.assertEqual(env["XDG_RUNTIME_DIR"], "/runtime-kept-for-display")
                self.assertEqual(env["DISPLAY"], ":test-display")
                states = Path(command[3])
                self.assertEqual(states, output / "states")
                (states / "component.bin").write_bytes(state)
                stdout.write("verified generated fixture\n")
                return subprocess.CompletedProcess(command, 0)

            args = ["sfizz_verify.py", "--binary", str(root / "verifier"),
                    "--bundle", str(root / "sfizz.vst3"), "--output", str(output)]
            with mock.patch.object(sys, "argv", args), \
                    mock.patch.dict(os.environ, DISPLAY=":test-display",
                                    XDG_RUNTIME_DIR="/runtime-kept-for-display"), \
                    mock.patch.object(module.subprocess, "run", side_effect=verify) as run, \
                    contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(SystemExit) as completed:
                    module.main()
            self.assertEqual(completed.exception.code, 0)
            run.assert_called_once()
            self.assertEqual(original.read_bytes(), b"do not modify")

    def test_existing_output_is_not_overwritten_or_executed(self):
        module = load_script()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sentinel = root / "keep.sfz"
            sentinel.write_bytes(b"existing user instrument")
            args = ["sfizz_verify.py", "--binary", str(root / "verifier"),
                    "--bundle", str(root / "sfizz.vst3"), "--output", str(root)]
            with mock.patch.object(sys, "argv", args), \
                    mock.patch.object(module.subprocess, "run") as run:
                with self.assertRaises(FileExistsError):
                    module.main()
            run.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b"existing user instrument")


if __name__ == "__main__":
    unittest.main()
