#!/usr/bin/env python3
"""Explicit opt-in native helper build. Never invoked by Cargo/Android builds."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="run deterministic CTest tests (no live audio)")
    parser.add_argument("--core-only", action="store_true", help="no JACK/FluidSynth headers or libraries required")
    parser.add_argument("--build-dir", type=Path, help="default: repository target/audio-host")
    parser.add_argument("--vst3-sdk", type=Path, help="absolute pinned SDK directory; enables VST3")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    build = (args.build_dir or root / "target" / "audio-host").resolve()
    for tool in ("cmake", "c++"):
        if not shutil.which(tool):
            parser.error(f"missing build tool: {tool}; no packages will be installed")
    command = [
        "cmake", "-S", str(root / "native" / "audio-host"), "-B", str(build),
        "-DCMAKE_BUILD_TYPE=RelWithDebInfo", "-DBUILD_TESTING=ON",
        f"-DRMS_CORE_ONLY={'ON' if args.core_only else 'OFF'}",
    ]
    if args.vst3_sdk:
        if not args.vst3_sdk.is_absolute() or not args.vst3_sdk.is_dir():
            parser.error("--vst3-sdk must name an existing absolute SDK directory")
        command.append(f"-DRMS_VST3_SDK={args.vst3_sdk}")
    else:
        command.append("-DRMS_VST3_SDK=")
    # Optional validation-only overrides; normal builds use pkg-config.
    for key in ("FLUIDSYNTH_INCLUDE_DIR", "FLUIDSYNTH_LIBRARY"):
        if os.environ.get(key):
            command.append(f"-D{key}={Path(os.environ[key]).resolve()}")
    try:
        subprocess.run(command, check=True)
        subprocess.run(["cmake", "--build", str(build), "--parallel", "2"], check=True)
        if args.test:
            subprocess.run(["ctest", "--test-dir", str(build), "--output-on-failure"], check=True)
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
