RustyMusicSheets Android foundation
=================================

This is the experimental Android foundation distributed with the 0.2.2
prerelease, NOT a production Android release or desktop feature parity.
Work toward M0 now includes generated fixtures, isolated provider/import/
rendering smoke tests and a draft CI workflow; see TESTING.txt and CI.txt.
Hosted CI must pass before the next roadmap milestone is considered ready.

Implemented
-----------
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

A verified development copy and SHA256SUMS were saved under dist/android:
    rustymusicsheets_0.2.2-foundation_arm64_debug.apk

That filename is not a production/release-signed Android release.

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
The debug APK and a safe-area update were installed on an authorized Pixel 8
Pro (API 37, ARM64, 4 KiB pages). Activity startup and continued process
execution were checked. After the system-inset correction was installed,
the user confirmed that the layout works on their Pixel. Broader rendering,
document-provider, rotation and keyboard tests are still needed. The 0.2.2
APK is rebuilt from the same layout implementation with updated version
metadata; this does not establish Android release parity.

Manual device checks once an APK can be built
--------------------------------------------
1. Install a debug APK on a test device or emulator.
   With one authorized ARM64 device connected, from the repository root:

       source scripts/android-env.sh
       adb devices -l
       adb install -r dist/android/rustymusicsheets_0.2.2-foundation_arm64_debug.apk
       adb shell am start -W -n org.ripleyeldridge.rustymusicsheets/android.app.NativeActivity

   For multiple devices select an explicit serial using adb -s SERIAL.
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
