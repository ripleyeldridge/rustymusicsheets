#!/usr/bin/env python3
"""Explicitly acquire/verify the pinned official VST3 SDK in ignored build staging."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def git(directory, *arguments, capture=False):
    command = ["git", "-c", "protocol.file.allow=never", "-C", str(directory), *arguments]
    if capture:
        return subprocess.check_output(command, text=True).strip()
    subprocess.run(command, check=True)
    return ""


def verify(destination, lock):
    if git(destination, "rev-parse", "HEAD", capture=True) != lock["commit"]:
        raise ValueError("SDK revision differs from the pinned commit; existing checkout was not modified")
    if git(destination, "status", "--porcelain", "--untracked-files=normal", capture=True):
        raise ValueError("SDK checkout has local changes; preserve/review them or choose a new destination")
    for name, dependency in lock["submodules"].items():
        tree = git(destination, "ls-tree", "HEAD", "--", name, capture=True)
        fields = tree.split()
        if len(fields) != 4 or fields[:2] != ["160000", "commit"] or fields[2] != dependency["commit"]:
            raise ValueError(f"SDK gitlink differs from lock: {name}")
        path = destination / name
        if not (path / ".git").is_file():
            raise ValueError(f"Required pinned SDK submodule is missing: {name}")
        if git(path, "rev-parse", "HEAD", capture=True) != dependency["commit"]:
            raise ValueError(f"SDK submodule revision differs from lock: {name}")
        if git(path, "status", "--porcelain", "--untracked-files=normal", capture=True):
            raise ValueError(f"SDK submodule has local changes: {name}")
    if not (destination / "LICENSE.txt").is_file():
        raise ValueError("Pinned SDK license is missing")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=ROOT / "target/vst3-sdk")
    parser.add_argument("--verify-only", action="store_true",
                        help="check existing source without any clone/fetch/checkout")
    args = parser.parse_args()
    if not shutil.which("git"):
        parser.error("git is required; no packages will be installed")
    lock = json.loads((ROOT / "native/vst3-sdk.lock.json").read_text())
    destination = args.destination.expanduser().absolute()
    try:
        if not destination.exists():
            if args.verify_only:
                raise ValueError("SDK source is absent; verification never downloads it")
            destination.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "-c", "protocol.file.allow=never", "clone",
                            "--filter=blob:none", "--no-checkout",
                            lock["repository"], str(destination)], check=True)
            git(destination, "checkout", "--detach", lock["commit"])
            # Only the pinned SDK's required gitlinks, not optional GUI/tutorial trees.
            git(destination, "submodule", "update", "--init", "--depth", "1",
                *lock["submodules"])
        # Existing directories are only verified, never overwritten, reset or cleaned.
        if not (destination / ".git").is_dir():
            raise ValueError("Destination is not a standalone SDK Git checkout; choose another directory")
        verify(destination, lock)
        print(f"Verified VST3 SDK {lock['version']} at {lock['commit']}")
        print(f"SDK path: {destination}")
        print("Required submodule revisions match. No system packages or Wine settings changed.")
        return 0
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"VST3 SDK setup failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
