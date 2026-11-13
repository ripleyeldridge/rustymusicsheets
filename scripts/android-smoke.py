#!/usr/bin/env python3
"""Exercise only isolated test package IDs on an explicitly selected Android target."""
import argparse
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import struct
import subprocess
import time

APP = "org.ripleyeldridge.rustymusicsheets.smoketest"
HELPER = "org.ripleyeldridge.rustymusicsheets.fixture"
PACKAGES = (APP, HELPER)


def wait_for(action, description, seconds=25):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = action()
        if result:
            return result
        time.sleep(0.25)
    raise RuntimeError(f"Timed out: {description}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--apks", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--keep-installed", action="store_true", help="Keep only the isolated test packages for diagnosis")
    args = parser.parse_args()
    if not __debug__:
        parser.error("Smoke verification requires assertions; disable Python -O/PYTHONOPTIMIZE")
    if not shutil.which("tesseract") or not shutil.which("convert"):
        parser.error("Host Tesseract English and ImageMagick convert are required for render evidence")
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Android/Sdk"))
    adb = sdk / "platform-tools/adb"
    base = [str(adb), "-s", args.serial]
    args.output.mkdir(parents=True, exist_ok=True)
    if any(args.output.iterdir()):
        parser.error("Choose an empty evidence output directory")

    def call(*arguments, binary=False, check=True):
        result = subprocess.run(base + list(arguments), check=check, capture_output=True, text=not binary, timeout=45)
        return result.stdout

    def shell(*arguments, check=True):
        return call("shell", *arguments, check=check).strip()

    assert shell("getprop", "sys.boot_completed") == "1", "Device is not booted"
    assert int(shell("getprop", "ro.build.version.sdk")) >= 26
    for package in PACKAGES:
        assert not shell("pm", "path", package, check=False), f"Refusing to replace existing test app {package}; uninstall it explicitly first"
    installed = []
    report = {
        "serial": args.serial,
        "api": shell("getprop", "ro.build.version.sdk"),
        "abis": shell("getprop", "ro.product.cpu.abilist"),
        "page_size": shell("getconf", "PAGE_SIZE"),
        "package": APP,
        "cases": [],
        "complete": False,
        "inputs": {
            name: hashlib.sha256((args.apks / name).read_bytes()).hexdigest()
            for name in ("smoke-app.apk", "fixture-provider.apk")
        },
        "fixtures_sha256": hashlib.sha256((args.fixtures / "fixtures.json").read_bytes()).hexdigest(),
    }

    def stop():
        shell("am", "force-stop", APP)

    def pid():
        return shell("pidof", APP, check=False)

    def screenshot(name):
        policy = shell("dumpsys", "window", "policy")
        assert not re.search(r"^\s*showing=true\s*$", policy, re.M), "Unlock the test target before UI tests"
        windows = shell("dumpsys", "window", "displays")
        assert any(APP in line for line in windows.splitlines() if "mCurrentFocus=" in line), "Isolated app is not foreground"
        path = args.output / name
        path.write_bytes(call("exec-out", "screencap", "-p", binary=True))
        return path

    def snapshot(name):
        # Stop only our fresh test app, then copy its consistent DB/WAL pair.
        stop()
        directory = args.output / name
        directory.mkdir()
        for suffix in ("", "-wal"):
            filename = "files/library/library.sqlite3" + suffix
            exists = subprocess.run(base + ["shell", "run-as", APP, "test", "-f", filename], capture_output=True, timeout=10)
            if exists.returncode == 0:
                (directory / ("library.sqlite3" + suffix)).write_bytes(call("exec-out", "run-as", APP, "cat", filename, binary=True))
        with sqlite3.connect(directory / "library.sqlite3") as database:
            assert database.execute("PRAGMA quick_check").fetchone() == ("ok",), "Test library integrity check failed"
            return database.execute("SELECT title, hash, pages FROM scores ORDER BY title").fetchall()

    def launch_fixture(fixture, count):
        stop()
        shell("am", "start", "-W", "-n", HELPER + "/.FixtureActivity", "--es", "fixture", fixture)
        wait_for(pid, f"app starts for {fixture}")
        if count:
            wait_for(lambda: len(shell("run-as", APP, "ls", "files/library/scores", check=False).splitlines()) >= count, f"{fixture} publishes a private copy")
        time.sleep(1.5)

    def render(name, token):
        library = screenshot(name + "-library.png")
        locator = args.output / (name + "-locator.png")
        subprocess.run(["convert", str(library), "-colorspace", "Gray", "-threshold", "45%", "-negate", "-resize", "300%", str(locator)], check=True)
        result = subprocess.check_output(
            ["tesseract", str(locator), "stdout", "--psm", "11", "tsv"],
            text=True, env=dict(os.environ, OMP_THREAD_LIMIT="1"), stderr=subprocess.DEVNULL,
        )
        words = list(csv.DictReader(io.StringIO(result), delimiter="\t"))
        match = next((word for word in words if re.search(r"\b" + token + r"\b", word.get("text", ""), re.I) and "/" not in word.get("text", "")), None)
        assert match, f"No visible generated {token} score row"
        x = (int(match["left"]) + int(match["width"]) // 2) // 3
        y = (int(match["top"]) + int(match["height"]) // 2) // 3
        shell("input", "tap", str(x), str(y))
        time.sleep(2)
        reader = screenshot(name + "-reader.png")
        width, height = struct.unpack(">II", reader.read_bytes()[16:24])
        pixels = subprocess.check_output(["convert", str(reader), "-alpha", "off", "-depth", "8", "rgb:-"])
        assert len(pixels) == width * height * 3
        counts, centers = [0, 0, 0], [0, 0, 0]
        for index in range(0, len(pixels), 3):
            red, green, blue = pixels[index:index + 3]
            for channel, present in enumerate((red > 200 and green < 50 and blue < 50, green > 200 and red < 50 and blue < 50, blue > 200 and red < 50 and green < 50)):
                if present:
                    counts[channel] += 1
                    centers[channel] += (index // 3) % width
        assert all(count > 300 for count in counts), f"Generated RGB score fiducials not rendered: {counts}"
        centers = [total / count for total, count in zip(centers, counts)]
        assert centers[0] < centers[1] < centers[2], "Rendered fiducial ordering is incorrect"
        return {"colored_pixels": counts, "centers_x": centers}

    def visible_result(name):
        image = screenshot(name + "-result.png")
        locator = args.output / (name + "-result-ocr.png")
        subprocess.run(["convert", str(image), "-channel", "R", "-separate", "-threshold", "30%", "-negate", "-resize", "400%", str(locator)], check=True)
        return subprocess.check_output(["tesseract", str(locator), "stdout", "--psm", "11"], text=True, env=dict(os.environ, OMP_THREAD_LIMIT="1"), stderr=subprocess.DEVNULL).lower()

    try:
        for name, package in (("smoke-app.apk", APP), ("fixture-provider.apk", HELPER)):
            call("install", "--no-streaming", str(args.apks / name))
            installed.append(package)
        shell("am", "start", "-W", "-n", APP + "/android.app.NativeActivity")
        wait_for(pid, "fresh startup")
        time.sleep(2)
        screenshot("startup.png")
        expected = json.loads((args.fixtures / "fixtures.json").read_text())
        hashes = {item["name"]: item["sha256"] for item in expected["files"]}
        for fixture, token, count, title, filename in (
            ("png", "PNG", 1, "Smoke PNG", "score.png"),
            ("jpeg", "JPEG", 2, "Smoke JPEG", "score.jpg"),
        ):
            launch_fixture(fixture, count)
            evidence = render(fixture, token)
            rows = snapshot(fixture + "-database")
            assert len(rows) == count and (title, hashes[filename], 1) in rows, rows
            report["cases"].append({"fixture": fixture, "result": "imported and rendered", "evidence": evidence})
            print("PASS:", fixture, "real URI import, private copy, DB metadata and rendering", flush=True)
        launch_fixture("unicode", 3)
        rows = snapshot("unicode-database")
        assert len(rows) == 3 and ("Étude Unicode", hashes["Étude Unicode.png"], 1) in rows, rows
        report["cases"].append({"fixture": "unicode", "result": "Unicode metadata imported"})
        launch_fixture("png", 3)
        assert re.search(r"\b1\s+dupl", visible_result("duplicate")), "Duplicate import has not visibly completed"
        assert len(snapshot("duplicate-database")) == 3
        report["cases"].append({"fixture": "duplicate", "result": "deduplicated"})
        launch_fixture("pipe", 3)
        assert re.search(r"\b1\s+dupl", visible_result("pipe")), "Pipe import has not visibly completed"
        assert len(snapshot("pipe-database")) == 3
        report["cases"].append({"fixture": "pipe", "result": "non-seekable descriptor imported and deduplicated"})
        failures = {
            "corrupt": "contents do not match",
            "oversized": "megapixel",
            "oversized-metadata": "size limit",
            "unsafe-name": "unsafe display name",
            "denied": "read grant",
        }
        for fixture, reason in failures.items():
            launch_fixture(fixture, 0)
            assert pid(), f"App crashed for {fixture}"
            error = screenshot(fixture + "-error.png")
            locator = args.output / (fixture + "-error-ocr.png")
            subprocess.run(["convert", str(error), "-channel", "R", "-separate", "-threshold", "45%", "-negate", "-resize", "300%", str(locator)], check=True)
            text = subprocess.check_output(["tesseract", str(locator), "stdout", "--psm", "11"], text=True, env=dict(os.environ, OMP_THREAD_LIMIT="1"), stderr=subprocess.DEVNULL).lower()
            assert "action needed" in text and reason in text, f"Expected handled {fixture} error is not visible"
            assert len(snapshot(fixture + "-database")) == 3, f"Invalid fixture inserted a record: {fixture}"
            report["cases"].append({"fixture": fixture, "result": "rejected without new record/crash"})
            print("PASS:", fixture, "rejected without new record or crash", flush=True)
        assert not shell("run-as", APP, "ls", "cache/rustymusicsheets"), "Leaked staging files"
        report["complete"] = True
    except BaseException:
        current = pid()
        if current:
            (args.output / "app.log").write_text(call("logcat", "-d", f"--pid={current}", "-t", "200"))
        raise
    finally:
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        if not args.keep_installed:
            for package in reversed(installed):
                call("uninstall", package, check=False)
    print("Isolated Android smoke passed. Evidence:", args.output)


if __name__ == "__main__":
    main()
