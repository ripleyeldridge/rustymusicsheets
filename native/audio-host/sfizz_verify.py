#!/usr/bin/env python3
"""Explicit opt-in sfizz 1.2.3 verification; no downloads, JACK or user data.

Usage: python3 native/audio-host/sfizz_verify.py --binary target/HOST/rms-sfizz-verify
       --bundle /ABS/sfizz.vst3 --output /ABS/FRESH_DIRECTORY [--editor-seconds 30]

The verification-only preset follows official sfizz-ui tag 1.2.3:
plugins/vst/SfizzVstState.{h,cpp}, and bundled SDK FStreamer::writeStr8/writeBool.
No vendor-specific state knowledge is used by production hosting code.
"""
import argparse
import math
import os
from pathlib import Path
import struct
import subprocess
import wave


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--editor-seconds", type=int)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(mode=0o700)  # Explicit fresh directory; never overwrite user data.
    env = os.environ.copy()  # Keep display/JACK runtime context unchanged.
    for key, directory in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                           ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")):
        private = output / directory
        private.mkdir(mode=0o700)
        env[key] = str(private)
    states = output / "states"
    states.mkdir(mode=0o700)
    with wave.open(str(output / "generated.wav"), "wb") as sample:
        sample.setnchannels(1)
        sample.setsampwidth(2)
        sample.setframerate(48000)
        sample.writeframes(b"".join(
            struct.pack("<h", int(8000 * math.sin(2 * math.pi * 220 * n / 48000)))
            for n in range(48000)))
    sfz = output / "generated.sfz"
    sfz.write_text("<region> sample=generated.wav lokey=0 hikey=127 pitch_keycenter=60 "
                   "loop_mode=loop_continuous loop_start=0 loop_end=47999 ampeg_release=0.05\n")

    def string(value):
        data = value.encode() + b"\0"
        return struct.pack("<i", len(data)) + data

    state = struct.pack("<Q", 5) + string(str(sfz))
    # The offline probe renders faster than wall-clock time: keep this tiny
    # generated sample resident instead of depending on a streaming worker.
    state += struct.pack("<fiii", 0, 64, 0, 65536) + string("")
    state += struct.pack("<iffiiiiHiI", 60, 440, 0, 2, 1, 10, 3, 0, -1, 0)
    preset = output / "generated.vstpreset"
    preset.write_bytes(b"VST3" + struct.pack("<i", 1) +
                       b"E8FAB71815ED46E38B5983101E12993F" +
                       struct.pack("<q", 48 + len(state)) + state +
                       b"List" + struct.pack("<i", 1) + b"Comp" +
                       struct.pack("<qq", 48, len(state)))
    command = [str(args.binary.resolve()), str(args.bundle.resolve()), str(preset), str(states)]
    if args.editor_seconds is not None:
        command.append(str(args.editor_seconds))
    with (output / "result.log").open("w") as stdout, (output / "stderr.log").open("w") as stderr:
        result = subprocess.run(command, env=env, stdout=stdout, stderr=stderr, timeout=180)
    print((output / "result.log").read_text(), end="")
    if result.returncode:
        print((output / "stderr.log").read_text(), end="")
    else:
        assert str(sfz).encode() + b"\0" in (states / "component.bin").read_bytes(), \
            "Component state did not retain the exact SFZ file path"
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
