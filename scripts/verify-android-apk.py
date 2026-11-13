#!/usr/bin/env python3
"""Verify the initial ARM64 Android APK; this does not replace device testing."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import zipfile


def run(*args, env=None):
    return subprocess.check_output([str(arg) for arg in args], text=True, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--abi", choices=("arm64-v8a", "x86_64"), default="arm64-v8a")
    parser.add_argument("--package", default="org.ripleyeldridge.rustymusicsheets")
    parser.add_argument("--sdk", type=Path, default=Path(os.environ.get("ANDROID_HOME", Path.home() / "Android/Sdk")))
    parser.add_argument("--ndk", type=Path)
    args = parser.parse_args()
    if not __debug__:
        parser.error("APK verification requires assertions; disable Python -O/PYTHONOPTIMIZE")
    sdk = args.sdk
    ndk = args.ndk or Path(os.environ.get("ANDROID_NDK_ROOT", sdk / "ndk/28.2.13676358"))
    tools = sdk / "build-tools/36.0.0"
    env = os.environ.copy()
    java = env.get("JAVA_HOME", "/usr/lib/jvm/java-21-openjdk-amd64")
    if (Path(java) / "bin/java").is_file():
        env["PATH"] = f"{java}/bin{os.pathsep}{env.get('PATH', '')}"
    print(run(tools / "apksigner", "verify", "--verbose", args.apk, env=env), end="")
    run(tools / "zipalign", "-c", "-P", "16", "4", args.apk)
    badging = run(tools / "aapt", "dump", "badging", args.apk)
    manifest = run(tools / "aapt", "dump", "xmltree", args.apk, "AndroidManifest.xml")
    permissions = run(tools / "aapt", "dump", "permissions", args.apk)
    assert f"name='{args.package}'" in badging, "Unexpected package"
    assert "sdkVersion:'26'" in badging, "Unexpected minimum API"
    assert "targetSdkVersion:'36'" in badging, "Unexpected target API"
    assert f"native-code: '{args.abi}'" in badging, "Unexpected ABI"
    assert "application-debuggable" in badging, "Expected development APK"
    for value in (
        "RustyMusicSheets", "android.app.NativeActivity", "android.intent.action.MAIN",
        "android.intent.category.LAUNCHER", "android.intent.action.VIEW", "image/png", "image/jpeg",
    ):
        assert value in manifest, f"Missing Android manifest entry: {value}"
    assert not re.search(r"android\.permission\.(MANAGE_EXTERNAL_STORAGE|READ_EXTERNAL_STORAGE|WRITE_EXTERNAL_STORAGE)", permissions), "Unexpected broad storage permission"
    host = "darwin-x86_64" if os.uname().sysname == "Darwin" else "linux-x86_64"
    readelf = ndk / f"toolchains/llvm/prebuilt/{host}/bin/llvm-readelf"
    with tempfile.TemporaryDirectory(prefix="rustymusicsheets-apk-verify-") as directory:
        with zipfile.ZipFile(args.apk) as bundle:
            assert bundle.testzip() is None, "Corrupt APK ZIP member"
            libraries = [name for name in bundle.namelist() if name.startswith("lib/") and name.endswith(".so")]
            assert libraries == [f"lib/{args.abi}/librustymusicsheets.so"], f"Unexpected native libraries: {libraries}"
            data = bundle.read(libraries[0])
            assert data[:6] == b"\x7fELF\x02\x01", "Expected little-endian ELF64"
            assert struct.unpack_from("<H", data, 18)[0] == {"arm64-v8a": 183, "x86_64": 62}[args.abi], "Unexpected ELF architecture"
            offset = struct.unpack_from("<Q", data, 32)[0]
            entry_size, count = struct.unpack_from("<HH", data, 54)
            alignments = []
            for index in range(count):
                base = offset + index * entry_size
                if struct.unpack_from("<I", data, base)[0] == 1:
                    alignment = struct.unpack_from("<Q", data, base + 48)[0]
                    file_offset, virtual_address = struct.unpack_from("<QQ", data, base + 8)
                    assert alignment >= 16384, "ELF load alignment is less than 16KiB"
                    assert file_offset % 16384 == virtual_address % 16384, "ELF load offsets are incompatible with 16KiB pages"
                    alignments.append(alignment)
            assert alignments, "Missing ELF load segments"
            library = Path(directory) / "librustymusicsheets.so"
            library.write_bytes(data)
            symbols = run(readelf, "--dyn-syms", "--wide", library)
            for symbol in ("android_main", "ANativeActivity_onCreate"):
                assert any(line.split()[-1:] == [symbol] and " UND " not in line for line in symbols.splitlines()), f"Missing export {symbol}"
            print("Native entry exports and PT_LOAD alignments verified:", alignments)
    print("Verified APK:", args.apk)
    print("Bytes:", args.apk.stat().st_size)
    print("SHA256:", hashlib.sha256(args.apk.read_bytes()).hexdigest())
    print(f"API 26 minimum; target API 36; {args.abi}; debug-signed; no broad storage permission.")
    print("Device execution has NOT been verified by this script.")


if __name__ == "__main__":
    main()
