#!/usr/bin/env python3
"""Run a build with only the pinned SDK tools visible; never edits the shared SDK."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def run_with_sdk(sdk, command, run=subprocess.run):
    sdk = sdk.resolve()
    required = ("build-tools/36.0.0", "platforms/android-35", "platforms/android-36", "platform-tools")
    for package in required:
        if not (sdk / package).is_dir():
            raise RuntimeError(f"Missing required SDK package: {package}")
    with tempfile.TemporaryDirectory(prefix="rms-android-sdk-") as directory:
        view = Path(directory)
        for package in required:
            link = view / package
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(sdk / package, target_is_directory=True)
        env = dict(os.environ, ANDROID_HOME=str(view))
        # ndk-build prefers the deprecated alias when both variables exist.
        env.pop("ANDROID_SDK_ROOT", None)
        print("Android build SDK view: build-tools 36.0.0; platforms 35/36; shared SDK unchanged.", flush=True)
        return run(command, env=env, check=False).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("A build command is required after --")
    raise SystemExit(run_with_sdk(args.sdk, command))


if __name__ == "__main__":
    main()
