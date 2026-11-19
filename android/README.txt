RustyMusicSheets Android foundation
=================================

This is the experimental Android foundation introduced with the 0.2.2
preview, NOT a production Android release or desktop feature parity.
The v0.5.0 prerelease includes RustyMusicSheets-0.5.0-android-arm64-debug.apk:
    https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.5.0
Direct APK:
    https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.5.0/RustyMusicSheets-0.5.0-android-arm64-debug.apk
It is built from immutable v0.5.0 tag commit
88316bbb951e181f4ad61b89e0823b644fe0e7e6 and reports versionName 0.5.0,
arm64-v8a, minimum API 26/target API 36. Exact source commit, versionCode and
signer certificate fingerprint are recorded in build-record.json.
The annotation implementation is now also in main through PR #8, merge commit
8e786738d86677b10f9eb2569ffe0a7c2b750d56. Use main for development or the
v0.5.0 tag for the published source snapshot; no annotation feature branch is
required. Its removal did not rebuild or replace the published APK/assets.
It is debuggable and locally debug-signed, NOT production signed. Signature,
manifest, ABI and ZIP/16 KiB ELF alignment checks passed; this APK was NOT
executed on a device. Static verification does not establish device coverage
or Linux OMR/audio parity. The v0.2.3 APK is a historical foundation build.
Do not assume Linux OMR, synth, page-follow or desktop Browse features work here.

Download SHA256SUMS, build-record.json, LICENSE and
rustymusicsheets-0.5.0-third-party-notices.tar.gz from the same release.
All GitHub attachment names are flat. From their download directory:
    sha256sum --ignore-missing -c SHA256SUMS
This verifies only downloaded files; omit --ignore-missing after downloading
the complete set to verify everything. The build record is unsigned provenance,
not an attestation; checksums are not a release signature.
Check signer_certificate_sha256 in build-record.json against the installed
app's signing identity before updating. Local/CI debug signers can differ.
Preserve data and never uninstall a valuable library to bypass a mismatch.

M0 includes generated fixtures, isolated provider/import/rendering tests and
integrated CI. A historical hosted API 35 x86_64 baseline passed all ten
cases; see TESTING.txt and CI.txt for exact source/run and matrix limits.
Current changes still need their own checks before milestone review.

Implemented
-----------
The v0.5.0 APK shares the page-vector/storage annotation engine with desktop
for supported PNG/JPEG scores. This does not add Android PDF rendering/OMR/audio. Actual stylus/touch behavior
needs device testing; constant-width ink does not guarantee palm rejection,
pressure or pen buttons. Back up app-private annotations/ with complete library
data; original documents are not rewritten. See the root annotation guide.
- NativeActivity android_main entry point, Android logging, and eframe's
  Android event loop. Desktop main/CLI startup remains unchanged.
- Dynamic safe-area bounds for status/navigation bars, camera cutouts and
  the on-screen keyboard. Android UI-thread measurements are projected into
  egui coordinates without shifting touch events or double-applying insets
  to an already-resized surface.
- ARM64 cdylib configuration for cargo-apk, minimum API 26 and target API 36.
- App-private files/cache directories supplied by Android Context APIs,
  without HOME/XDG assumptions or broad storage permissions.
- Typed local-path versus content:// document references. URIs preserve their
  original encoding and are never converted into filesystem paths.
- Read-only ContentResolver access for metadata and file descriptors.
  The Activity remains alive through the resolver; no global raw Activity
  pointer is cached across Activity recreation.
- Initial ACTION_VIEW launch documents enter the existing background importer.
  The manifest advertises PNG/JPEG only, because those can use the current
  Rust image decoder. PDF/OCR helper execution is explicitly unsupported.
- A private managed copy is imported, hashed and deduplicated. Staging checks
  names/MIME, rejects traversal names, limits streamed content to 512 MiB,
  checks cancellation between reads, and removes temporary files on exit/error.

Build prerequisites
-------------------
The local build was verified with Java 21, Rust 1.86, cargo-apk 0.10.0,
NDK r28c (28.2.13676358), build-tools 36.0.0, platform-tools, and the API 35
and API 36 SDK platforms under ~/Android/Sdk. API 35 is needed for cargo-apk
0.10's NDK-range discovery; the application's target SDK remains API 36.
No SDK/NDK is bundled with the application or automatically installed by the
build wrapper. Install matching tools and accept their licenses separately.

For example, after configuring those tools:

    rustup target add aarch64-linux-android
    cargo install cargo-apk --version 0.10.0 --locked --root "$HOME/.local"
    ./scripts/build-android.sh

The wrapper defaults to ~/Android/Sdk and the pinned NDK above; override
ANDROID_HOME, ANDROID_NDK_ROOT, or JAVA_HOME when needed. It preflights the
Cargo lockfile, limits build jobs, requests 16 KiB ELF alignment, and omits
bulky native DWARF debug data while leaving the APK debuggable.
Set CARGO_PROFILE_DEV_DEBUG=2 when native source-level debugging is needed.
For direct cargo/adb use, source the environment helper:

    source scripts/android-env.sh

The generated APK is target/debug/apk/RustyMusicSheets.apk. Verify it with:

    python3 scripts/verify-android-apk.py target/debug/apk/RustyMusicSheets.apk

The manifest version is derived from Cargo.toml, not the output basename.
Inspect the versionName/versionCode and native ABI of every APK before
distribution; do not rename an old APK to imply a new application version.
Older copies under dist/android/ are historical artifacts and need not match
the current source. The canonical fresh-build output above is debug, not a
production/release-signed artifact.

cargo-apk selects this package's cdylib. Debug packaging can create a local
debug keystore. Do not commit signing keys or passwords; provide release
signing configuration securely outside the repository. A release-grade APK
still requires signing, native-library/page-size checks, and device testing.

The full application, including bundled SQLite, now builds and links into an
ARM64 debug APK. APK Signature Scheme v2/v3 verification, ZIP alignment,
16 KiB ELF load segments, NativeActivity/android_main exports, manifest
identity and URI filters, and absence of broad storage permissions passed.
Desktop tests also exercise URI validation, private import, deduplication,
cancellation/cleanup and shared-UI integration using generated documents.
Earlier foundation APKs and a safe-area update were installed on an authorized Pixel 8
Pro (API 37, ARM64, 4 KiB pages). Activity startup and continued process
execution were checked. After the system-inset correction was installed,
the user confirmed that the layout works on their Pixel. Broader rendering,
document-provider, rotation and keyboard tests are still needed. Historical
0.2.2 builds reused that layout implementation with updated version metadata;
neither that startup check nor a later version-only rebuild establishes
current Android device coverage or release parity.

Manual device checks for a new build
------------------------------------
1. Prefer the isolated test harness in TESTING.txt. For an ordinary debug APK,
   use a dedicated authorized test device/emulator with no valuable library,
   or an explicitly verified compatible signer and independent data backup.
   Select its serial explicitly, from the repository root:

       source scripts/android-env.sh
       adb devices -l
       adb -s YOUR_TEST_SERIAL install -r target/debug/apk/RustyMusicSheets.apk
       adb -s YOUR_TEST_SERIAL shell am start -W -n org.ripleyeldridge.rustymusicsheets/android.app.NativeActivity

   Do not uninstall an existing app to bypass a signing mismatch; that can
   delete its private library. Use the original signing key or a test device.
2. Launch normally and check Library, Help, rotation, background/resume,
   on-screen keyboard behavior and durable SQLite metadata. Verify that
   controls stay below the status bar/camera and above navigation/keyboard
   areas, and that touch targets still match the rendered controls.
3. In a document/file app, select an original PNG/JPEG and use Open with,
   choosing RustyMusicSheets. The provider must grant read access to its URI.
4. Confirm the score imports and renders. Repeat to confirm deduplication.
5. Test unknown-size documents, permission denial/revocation, cancellation,
   a provider returning a pipe descriptor, and Activity recreation.
6. Confirm metadata survives cache clearing, while temporary staging does not.
   Uninstalling Android apps normally deletes app-private copies and metadata;
   keep independent backups. It does not delete the provider's original file.

Permission and lifecycle boundaries
-----------------------------------
Only the initial ACTION_VIEW data URI is handled. No new-intent callback,
SEND/share attachments, in-app SAF result bridge, persisted URI grants, or
document-tree browsing/watching is implemented. One-time launch permissions
are sufficient to make the private copy while the launching Activity remains
alive; later launches may require the user to grant access again.

ContentResolver query/open calls and provider reads can block. Cancellation
is checked between reads; this foundation does not promise interruption of a
blocked provider call. Virtual documents requiring typed asset conversion are
not supported by the current read-only file-descriptor backend.

Remaining mobile work
---------------------
- In-app Storage Access Framework picker and asynchronous result delivery.
- Android-native PDF rendering/text extraction and offline OCR libraries.
- Android keep-screen-on integration (currently reports unavailable).
- Document-tree providers, rescanning semantics and lifecycle-safe tasks.
- Tablet/phone layout refinement, Back navigation, process-death restoration,
  signing/distribution and real-device/16 KiB native-library verification.
- Android-native musical notation recognition, synth playback/mixing and
  synth-driven page following; current music helper processes are Linux-only.

Distributing development artifacts
----------------------------------
The Linux .deb builder does not build an APK, and a tag push does not publish
one. Before attaching an APK to a GitHub Release, verify its application version,
exact build commit, signing identity, API/ABI/manifest and ZIP/ELF alignment.
Include checksums and a clearly labelled local or CI build record; unsigned
provenance is not a cryptographic attestation. Retain the distinction between
static verification, isolated x86_64 emulator tests and actual ARM64 hardware.
Complete notice/signing/device gates in CI.txt still apply.
Do not publish harness APKs, SDK/NDK files, keys, AVDs or personal documents.
Local debug and ephemeral CI keys can differ and block in-place upgrades;
preserve data and signing continuity rather than telling users to uninstall.
