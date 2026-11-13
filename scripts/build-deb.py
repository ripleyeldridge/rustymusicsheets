#!/usr/bin/env python3
"""Build a native .deb without root. Run on the oldest Ubuntu release to support."""
import argparse
import email.utils
from datetime import datetime, timezone
import gzip
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = "rustymusicsheets"


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def output(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def install(source, destination, mode=0o644):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true",
                        help="package the existing native release executable")
    parser.add_argument("--revision", default="1", help="Debian package revision")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9][A-Za-z0-9.+~]*", args.revision):
        parser.error("revision must start with a digit and contain Debian version characters")
    for tool in ("cargo", "dpkg", "dpkg-deb", "dpkg-shlibdeps", "readelf", "strip"):
        if not shutil.which(tool):
            parser.error(f"required tool not found: {tool}")
    version_match = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)
    if version_match is None:
        parser.error("package version not found in Cargo.toml")
    version = version_match.group(1)
    version = f"{version}-{args.revision}"
    arch = output("dpkg", "--print-architecture")
    if not args.skip_build:
        run("cargo", "build", "--locked", "--release", cwd=ROOT)
    binary = ROOT / "target/release" / PACKAGE
    if not binary.is_file():
        parser.error("release binary missing; build it first or omit --skip-build")
    machine = output("readelf", "-h", str(binary))
    expected = {"amd64": "Advanced Micro Devices X86-64", "arm64": "AArch64",
                "armhf": "ARM"}.get(arch)
    if expected is None or not re.search(rf"Machine:\s+{re.escape(expected)}\s*$", machine, re.M):
        parser.error(f"release binary does not match supported native architecture {arch}")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", int(time.time())))
    env = dict(os.environ, SOURCE_DATE_EPOCH=str(epoch), LC_ALL="C")
    destination = ROOT / "dist" / f"{PACKAGE}_{version}_{arch}.deb"
    destination.parent.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="rustymusicsheets-deb-") as temp:
        work = Path(temp)
        stage = work / "debian" / PACKAGE
        install(binary, stage / "usr/bin" / PACKAGE, 0o755)
        run("strip", "--strip-unneeded", str(stage / "usr/bin" / PACKAGE))
        install(ROOT / "assets/rustymusicsheets.desktop",
                stage / "usr/share/applications/rustymusicsheets.desktop")
        install(ROOT / "assets/rustymusicsheets.svg",
                stage / "usr/share/icons/hicolor/scalable/apps/rustymusicsheets.svg")
        for size in (16, 24, 32, 48, 64, 128, 256, 512):
            name = f"{size}x{size}"
            install(ROOT / f"assets/icons/{name}/rustymusicsheets.png",
                    stage / f"usr/share/icons/hicolor/{name}/apps/rustymusicsheets.png")
        install(ROOT / "README.txt", stage / f"usr/share/doc/{PACKAGE}/README")
        install(ROOT / "assets/help.txt", stage / f"usr/share/doc/{PACKAGE}/help.txt")
        install(ROOT / "LICENSE", stage / f"usr/share/doc/{PACKAGE}/copyright")
        # dpkg-shlibdeps derives actual symbol-version requirements, not the
        # build host's libc version guessed from its release name.
        debian = work / "debian"
        debian.mkdir(exist_ok=True)
        (debian / "control").write_text(
            f"Source: {PACKAGE}\nSection: sound\nPriority: optional\n"
            "Maintainer: Ripley Eldridge <noreply@rustymusicsheets.invalid>\n"
            "Homepage: https://github.com/ripleyeldridge\n"
            "Standards-Version: 4.7.0\n\n"
            f"Package: {PACKAGE}\nArchitecture: any\n"
            "Description: Native sheet music library and performance reader\n"
        )
        dependencies = output("dpkg-shlibdeps", "-O", str(stage / "usr/bin" / PACKAGE),
                              cwd=work, env=env)
        linked = next(line.removeprefix("shlibs:Depends=")
                      for line in dependencies.splitlines() if line.startswith("shlibs:Depends="))
        # eframe loads graphics/input libraries dynamically; ldd cannot discover them.
        runtime = [
            linked, "poppler-utils", "tesseract-ocr", "tesseract-ocr-eng",
            "libx11-6", "libxcursor1", "libxi6", "libxrandr2",
            "libxkbcommon0", "libxkbcommon-x11-0", "libwayland-client0",
            "libwayland-cursor0", "libwayland-egl1", "libegl1", "libgl1", "hicolor-icon-theme",
        ]
        docs = stage / f"usr/share/doc/{PACKAGE}"
        date = email.utils.format_datetime(datetime.fromtimestamp(epoch, timezone.utc))
        changelog = (
            f"{PACKAGE} ({version}) unstable; urgency=medium\n\n"
            "  * Native Linux sheet-music reader with watched library folders,\n"
            "    readable names, desktop icons, scoped keep-awake controls,\n"
            "    home-directory fallback, virtual-keyboard input fixes,\n"
            "    setlist management, resize-safe layouts, and separate main\n"
            "    Library/Reader tabs, safe explicit Rescan cleanup with backups,\n"
            "    shared editors, offline instrument detection/review/filtering,\n"
            "    standalone PNG/JPEG score support, searchable offline Help/About,\n"
            "    large-library instrument detection warnings, platform-specific\n"
            "    picker/power backends, and an experimental Android activity,\n"
            "    content-URI import, and safe-area layout foundation.\n\n"
            f" -- Ripley Eldridge <noreply@rustymusicsheets.invalid>  {date}\n"
        )
        (docs / "changelog.Debian.gz").write_bytes(gzip.compress(changelog.encode(), mtime=0))
        man = stage / "usr/share/man/man1/rustymusicsheets.1.gz"
        man.parent.mkdir(parents=True)
        man.write_bytes(gzip.compress((ROOT / "assets/rustymusicsheets.1").read_bytes(), mtime=0))
        metadata = output("cargo", "metadata", "--locked", "--format-version=1", cwd=ROOT)
        import json
        packages = json.loads(metadata)["packages"]
        notices = docs / "third-party-licenses"
        notices.mkdir()
        entries = ["Resolved Cargo dependencies (includes target-specific packages):", ""]
        for package in sorted(packages, key=lambda item: (item["name"], item["version"])):
            name = f'{package["name"]}-{package["version"]}'
            entries.append(f'{name}: {package.get("license") or "See package license files"}')
            directory = Path(package["manifest_path"]).parent
            for source in directory.iterdir():
                if source.is_file() and source.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "OFL")):
                    install(source, notices / name / source.name)
            for subdir in ("licenses", "LICENSES"):
                if (directory / subdir).is_dir():
                    shutil.copytree(directory / subdir, notices / name / subdir, dirs_exist_ok=True)
        (docs / "third-party-notices.txt").write_text("\n".join(entries) + "\n")
        installed_kib = (sum(p.stat().st_size for p in stage.rglob("*") if p.is_file()) + 1023) // 1024
        control = stage / "DEBIAN"
        control.mkdir()
        (control / "control").write_text(
            f"Package: {PACKAGE}\nVersion: {version}\nArchitecture: {arch}\n"
            "Section: sound\nPriority: optional\n"
            "Maintainer: Ripley Eldridge <noreply@rustymusicsheets.invalid>\n"
            "Homepage: https://github.com/ripleyeldridge\n"
            f"Installed-Size: {installed_kib}\n"
            f"Depends: {', '.join(runtime)}\n"
            "Recommends: zenity, libgl1-mesa-dri\n"
            "Description: Sheet music library and performance reader for Linux\n"
            " Organize PDF/PNG/JPEG scores, detect and review instruments offline,\n"
            " watch folders, build setlists, and read with pedal page turning.\n"
        )
        for directory in [stage, *(p for p in stage.rglob("*") if p.is_dir())]:
            directory.chmod(0o755)
        if shutil.which("desktop-file-validate"):
            run("desktop-file-validate", str(stage / "usr/share/applications/rustymusicsheets.desktop"))
        run("dpkg-deb", "--root-owner-group", "--uniform-compression", "-Zxz", "-z6",
            "--build", str(stage), str(destination), env=env)
    print(f"\nPackage: {destination}\nArchitecture: {arch}\nRuntime dependencies: {', '.join(runtime)}")
    print("This is a native build, not a universal Ubuntu package.")
    print("For older Ubuntu releases, rebuild inside that release or a matching build environment.")


if __name__ == "__main__":
    main()
