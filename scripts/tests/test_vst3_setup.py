"""SDK setup is explicit, revision-pinned and never resets an existing checkout."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("vst3_setup", ROOT / "scripts/fetch-vst3-sdk.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)
LOCK = json.loads((ROOT / "native/vst3-sdk.lock.json").read_text())


class SdkSetupTests(unittest.TestCase):
    def test_lock_has_exact_official_sdk_and_required_gitlinks(self):
        self.assertEqual(LOCK["commit"], "3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96")
        self.assertEqual(LOCK["repository"], "https://github.com/steinbergmedia/vst3sdk.git")
        self.assertEqual(set(LOCK["submodules"]), {"base", "cmake", "pluginterfaces", "public.sdk"})
        for value in LOCK["submodules"].values():
            self.assertRegex(value["commit"], r"^[0-9a-f]{40}$")
            self.assertTrue(value["repository"].startswith("https://github.com/steinbergmedia/"))

    def test_verify_only_missing_destination_never_downloads(self):
        with tempfile.TemporaryDirectory() as temp, \
                mock.patch("sys.argv", ["fetch-vst3-sdk.py", "--verify-only",
                                       "--destination", str(Path(temp) / "missing")]), \
                mock.patch.object(setup.shutil, "which", return_value="/usr/bin/git"), \
                mock.patch.object(setup.subprocess, "run") as run, \
                mock.patch.object(setup.subprocess, "check_output") as output, \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(setup.main(), 1)
            run.assert_not_called()
            output.assert_not_called()

    def test_existing_user_directory_is_not_modified(self):
        with tempfile.TemporaryDirectory() as temp:
            destination = Path(temp)
            keep = destination / "keep.txt"
            keep.write_text("user content")
            with mock.patch("sys.argv", ["fetch-vst3-sdk.py", "--destination", temp]), \
                    mock.patch.object(setup.shutil, "which", return_value="/usr/bin/git"), \
                    mock.patch.object(setup.subprocess, "run") as run, \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(setup.main(), 1)
                run.assert_not_called()
            self.assertEqual(keep.read_text(), "user content")

    def test_wrong_revision_is_rejected_without_checkout_or_reset(self):
        with tempfile.TemporaryDirectory() as temp, \
                mock.patch.object(setup, "git", return_value="0" * 40) as git:
            with self.assertRaisesRegex(ValueError, "revision differs"):
                setup.verify(Path(temp), LOCK)
            git.assert_called_once_with(Path(temp), "rev-parse", "HEAD", capture=True)

    def test_required_submodules_are_checked_against_gitlinks_and_worktrees(self):
        with tempfile.TemporaryDirectory() as temp:
            destination = Path(temp)
            (destination / "LICENSE.txt").write_text("fixture MIT license")
            for name in LOCK["submodules"]:
                (destination / name).mkdir()
                (destination / name / ".git").write_text("gitdir: fixture")

            def fake_git(path, *arguments, capture=False):
                self.assertTrue(capture, "Verification must not mutate Git state")
                if arguments == ("rev-parse", "HEAD"):
                    return LOCK["commit"] if path == destination else LOCK["submodules"][path.name]["commit"]
                if arguments[0] == "status":
                    return ""
                if arguments[0] == "ls-tree":
                    name = arguments[-1]
                    return f"160000 commit {LOCK['submodules'][name]['commit']}\t{name}"
                raise AssertionError(arguments)

            with mock.patch.object(setup, "git", side_effect=fake_git):
                setup.verify(destination, LOCK)
            with mock.patch.object(setup, "git", side_effect=lambda path, *args, capture=False:
                                   "160000 commit" if args[0] == "ls-tree" else fake_git(path, *args, capture=capture)):
                with self.assertRaisesRegex(ValueError, "gitlink differs"):
                    setup.verify(destination, LOCK)


if __name__ == "__main__":
    unittest.main()
