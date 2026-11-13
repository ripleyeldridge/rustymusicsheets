#!/usr/bin/env python3
"""Package the real native binary under an isolated ID plus a private fixture provider."""
import argparse
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
SMOKE = "org.ripleyeldridge.rustymusicsheets.smoketest"


def run(*args, env=None):
    subprocess.run([str(arg) for arg in args], check=True, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--abi", choices=("arm64-v8a", "x86_64"), required=True)
    args = parser.parse_args()
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Android/Sdk"))
    java = Path(os.environ.get("JAVA_HOME", "/usr/lib/jvm/java-21-openjdk-amd64"))
    if not (java / "bin/javac").is_file():
        parser.error("JAVA_HOME must contain a JDK with javac")
    tools = sdk / "build-tools/36.0.0"
    android = sdk / "platforms/android-36/android.jar"
    args.output.mkdir(parents=True, exist_ok=True)
    for name in ("smoke-app.apk", "fixture-provider.apk"):
        if (args.output / name).exists():
            parser.error(f"Refusing to overwrite {name}; select a fresh output directory")
    env = dict(os.environ, PATH=f"{java}/bin{os.pathsep}{os.environ.get('PATH', '')}")
    # Ephemeral test key only. Password exists only in this process's environment.
    env["RMS_TEST_KEYSTORE_PASSWORD"] = secrets.token_urlsafe(32)
    with tempfile.TemporaryDirectory(prefix="rms-smoke-build-") as temporary:
        temp = Path(temporary)
        key = temp / "test.keystore"
        run(java / "bin/keytool", "-genkeypair", "-keystore", key, "-storetype", "PKCS12",
            "-storepass:env", "RMS_TEST_KEYSTORE_PASSWORD", "-keypass:env", "RMS_TEST_KEYSTORE_PASSWORD",
            "-alias", "rms-smoke", "-keyalg", "RSA", "-keysize", "2048", "-validity", "3",
            "-dname", "CN=RMS Isolated Smoke Test", "-noprompt", env=env)
        manifest = temp / "AndroidManifest.xml"
        manifest.write_text(f'''<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="{SMOKE}" android:versionCode="1" android:versionName="smoke">
<uses-sdk android:minSdkVersion="26" android:targetSdkVersion="36"/>
<application android:label="RustyMusicSheets Smoke" android:hasCode="false" android:debuggable="true" android:allowBackup="false" android:extractNativeLibs="true" android:theme="@android:style/Theme.DeviceDefault.NoActionBar">
<activity android:name="android.app.NativeActivity" android:exported="true" android:configChanges="orientation|keyboardHidden|screenSize|screenLayout|density|uiMode">
<meta-data android:name="android.app.lib_name" android:value="rustymusicsheets"/>
<intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter>
<intent-filter><action android:name="android.intent.action.VIEW"/><category android:name="android.intent.category.DEFAULT"/><data android:scheme="content" android:mimeType="image/png"/><data android:scheme="content" android:mimeType="image/jpeg"/></intent-filter>
</activity></application></manifest>''')
        native = temp / "native-unsigned.apk"
        run(tools / "aapt", "package", "-f", "-M", manifest, "-I", android, "-F", native)
        library = f"lib/{args.abi}/librustymusicsheets.so"
        with zipfile.ZipFile(args.apk) as original, zipfile.ZipFile(native, "a") as target:
            target.writestr(library, original.read(library), compress_type=zipfile.ZIP_DEFLATED)
        classes, dex = temp / "classes", temp / "dex"
        classes.mkdir()
        dex.mkdir()
        sources = sorted((ROOT / "android/test-harness").glob("*.java"))
        run(java / "bin/javac", "-encoding", "UTF-8", "-source", "8", "-target", "8", "-Xlint:-options",
            "-bootclasspath", android, "-d", classes, *sources)
        run(tools / "d8", "--lib", android, "--min-api", "26", "--output", dex, *sorted(classes.rglob("*.class")), env=env)
        helper = temp / "helper-unsigned.apk"
        assets = temp / "assets"
        assets.mkdir()
        for fixture in args.fixtures.iterdir():
            if fixture.is_file():
                name = "unicode.png" if fixture.name == "Étude Unicode.png" else fixture.name
                shutil.copyfile(fixture, assets / name)
        run(tools / "aapt", "package", "-f", "-M", ROOT / "android/test-harness/AndroidManifest.xml",
            "-I", android, "-A", assets, "-F", helper)
        with zipfile.ZipFile(helper, "a") as target:
            target.write(dex / "classes.dex", "classes.dex")
        for source, name in ((native, "smoke-app.apk"), (helper, "fixture-provider.apk")):
            aligned = temp / name
            run(tools / "zipalign", "-f", "-P", "16", "4", source, aligned)
            run(tools / "apksigner", "sign", "--ks", key, "--ks-key-alias", "rms-smoke",
                "--ks-pass", "env:RMS_TEST_KEYSTORE_PASSWORD", "--key-pass", "env:RMS_TEST_KEYSTORE_PASSWORD",
                "--out", args.output / name, aligned, env=env)
            run(tools / "apksigner", "verify", args.output / name, env=env)
    print("Isolated test APKs:", args.output)
    print("Production app, production signing keys and personal documents were not used.")


if __name__ == "__main__":
    main()
