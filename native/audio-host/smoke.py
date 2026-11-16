#!/usr/bin/env python3
"""Process/wire tests by default; real audio only with --allow-audio --soundfont."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time


def send(connection, message):
    data = json.dumps(message, separators=(",", ":")).encode()
    connection.sendall(struct.pack(">I", len(data)) + data)


def receive(connection):
    def exact(count):
        result = b""
        while len(result) < count:
            chunk = connection.recv(count - len(result))
            if not chunk:
                raise RuntimeError("unexpected helper EOF")
            result += chunk
        return result
    size, = struct.unpack(">I", exact(4))
    assert 0 < size <= 1024 * 1024
    return json.loads(exact(size))


def run(binary, scenario, soundfont=None, pw_jack=False):
    with tempfile.TemporaryDirectory(prefix="rms-host-") as directory:
        path = str(Path(directory) / "control.sock")
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(path)
            listener.listen(1)
            listener.settimeout(8)
            command = (["/usr/bin/pw-jack"] if pw_jack else []) + [
                str(binary), "serve", "--socket", path, "--parent-pid",
                str(os.getpid() + (1 if scenario == "peer" else 0))
            ]
            process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                connection, _ = listener.accept()
                with connection:
                    connection.settimeout(8)
                    pid, uid, _ = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    assert pid == process.pid and uid == os.getuid()
                    if scenario == "peer":
                        assert connection.recv(1) == b"", "host accepted incorrect parent peer identity"
                        assert process.wait(timeout=5) == 1
                        return
                    session = "c" * 64
                    message = {"version": 1, "session": session, "request_id": 0, "epoch": 0, "type": "hello"}
                    # Exercise a header and UTF-8 payload fragmented across writes.
                    payload = json.dumps(message).encode()
                    for byte in struct.pack(">I", len(payload)) + payload:
                        connection.sendall(bytes([byte]))
                    hello = receive(connection)
                    assert hello == {**message, "capabilities": ["soundfont", "jack"]}
                    if scenario == "oversize":
                        connection.sendall(struct.pack(">I", 1024 * 1024 + 1))
                        error = receive(connection)
                        assert error["type"] == "error"
                    elif scenario == "stale":
                        send(connection, {**message, "type": "pause"})
                        assert receive(connection)["type"] == "error"
                    elif scenario == "bad-load":
                        send(connection, {**message, "type": "load", "request_id": 1,
                                          "plan_path": str(Path(directory) / "absent.json"),
                                          "plan_bytes": 2, "plan_sha256": "0" * 64,
                                          "soundfont": "/absent.sf2", "autoplay": False,
                                          "master_volume": 100, "tempo_scale": 1})
                        error = receive(connection)
                        assert error["type"] == "error" and error["request_id"] == 0
                        assert "plan" in error["message"].lower()
                    elif scenario == "disconnect":
                        pass  # leaving context closes socket; helper must exit
                    elif scenario == "audio":
                        notes = [{"start_tick": 0, "duration_ticks": 1920, "pitch": 60, "velocity": 80}]
                        plan = {"version": 1, "ticks_per_quarter": 960, "duration_ticks": 3840,
                                "tempos": [{"tick": 0, "bpm": 120}], "parts": [
                                    {"id": f"p{i}", "enabled": True, "volume": 80, "program": i,
                                     "notes": notes} for i in range(2)]}
                        data = json.dumps(plan, separators=(",", ":")).encode()
                        plan_path = Path(directory) / "plan.json"
                        fd = os.open(plan_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                        with os.fdopen(fd, "wb") as file:
                            file.write(data)
                        send(connection, {**message, "request_id": 1, "type": "load",
                                          "plan_path": str(plan_path), "plan_bytes": len(data),
                                          "plan_sha256": hashlib.sha256(data).hexdigest(),
                                          "soundfont": str(soundfont), "autoplay": True,
                                          "master_volume": 25, "tempo_scale": 1})
                        deadline = time.monotonic() + 1
                        seen = 0
                        while time.monotonic() < deadline:
                            status = receive(connection)
                            if status["type"] == "error":
                                raise RuntimeError(status["message"])
                            assert status["type"] == "status" and status["request_id"] == 1
                            assert status["state"] in ("playing", "finished") and status["sample_rate"] > 0
                            seen += 1
                        assert seen >= 2
                        send(connection, {**message, "request_id": 2, "type": "shutdown"})
                    else:
                        send(connection, {**message, "request_id": 1, "type": "shutdown"})
                code = process.wait(timeout=5)
                assert code == (0 if scenario in ("shutdown", "audio") else 1), (scenario, code)
            finally:
                if process.poll() is None:
                    process.kill()
                _, stderr = process.communicate(timeout=5)
                if process.returncode not in (0, 1):
                    raise RuntimeError(stderr.decode(errors="replace"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--allow-audio", action="store_true")
    parser.add_argument("--soundfont", type=Path)
    parser.add_argument("--pw-jack", action="store_true")
    args = parser.parse_args()
    if args.allow_audio != (args.soundfont is not None):
        parser.error("live audio requires both --allow-audio and --soundfont")
    binary = args.binary.resolve()
    for scenario in ("shutdown", "oversize", "stale", "bad-load", "disconnect", "peer"):
        run(binary, scenario)
    if args.allow_audio:
        run(binary, "audio", args.soundfont.resolve(), args.pw_jack)
    print("helper wire/process tests passed" + ("; live audio smoke passed" if args.allow_audio else "; no audio opened"))


if __name__ == "__main__":
    main()
