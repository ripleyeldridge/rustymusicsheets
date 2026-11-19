# RustyMusicSheets
A native Linux sheet-music library and performance reader, written in Rust with egui.
**A headline feature: offline instrument detection that helps turn a folder of scores into an instrument-searchable library—even when the parts are scanned PDFs or standalone images.**

**Version:** 0.6.0 preview · **Author:** [Ripley Eldridge](https://github.com/ripleyeldridge) · **Release year:** 2026 · **License:** [MIT](LICENSE)

## Release 0.6.0 — Non-blocking yabridge Editor Warnings
- Allow explicit scans, instrument loads and playback with Wine 9.22+ and yabridge 5.1.1 or earlier, retaining the known editor-input warning.
- Show the advisory in diagnostics, scan results and playback controls, including the far-left / leftmost-screen workaround.
- Keep missing tools, invalid settings, failed version probes and actual plugin/helper failures as errors. Native Linux VST3 instruments remain independent of Wine/yabridge.
- Retain v0.5.0's non-destructive annotations and native VST3/SoundFont hosting with reversible OMR part combination.

Published on **19 November 2026**, this minor preview was merged into `main` through [PR #9](https://github.com/ripleyeldridge/rustymusicsheets/pull/9). Annotated tag **`v0.6.0`** points to release/build commit **`bab87330fbcb47e44739f43b4e650d28519e60a8`**. Later documentation or development commits on `main` do not change that snapshot. Asset hashes, signing identity and verification results are recorded in `build-record.json`. Previous tags and assets, including [v0.5.0](https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.5.0), remain unchanged. This is not a stable/latest promotion. The Linux package includes the SDK-enabled helper, not plugins, samples, SDK source or personal data.

## Latest published preview: v0.6.0
- [Release notes](https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.6.0)
- [Linux amd64 `.deb` — 0.6.0-1](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/rustymusicsheets_0.6.0-1_amd64.deb)
- [Experimental Android ARM64 debug APK](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/RustyMusicSheets-0.6.0-android-arm64-debug.apk)
- [SHA256SUMS](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/SHA256SUMS), [build record](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/build-record.json), [license](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/LICENSE) and [third-party notices](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/rustymusicsheets-0.6.0-third-party-notices.tar.gz)

The **Linux amd64 preview requires glibc 2.43 or newer**; it is not compatible with Ubuntu 22.04/24.04. The [stable/latest v0.2.1 release](https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.2.1) remains available.

The Android APK is **experimental, debuggable and locally debug-signed**, not a production-signed mobile release. This build passed static checks but **was not executed on a device**. Initial Open with PNG/JPEG, private imports, metadata and Help are supported; native PDF/OCR, the in-app picker, synth playback and Android keep-awake remain incomplete. Debug builds may use different signing keys: preserve existing data and do not uninstall merely to bypass a signature mismatch. See [Android setup and boundaries](android/README.txt).

## Why this exists
> I vibe-coded the fuck out of this because I couldn't find a suitable Linux sheet-music reader for what I needed and I have a full-time coding job so I don't have time to write the whole thing myself.
>
> — Ripley Eldridge

RustyMusicSheets is an attempt to build the practical, Linux-native score library and reader I wanted: organize my music, find the right part, put together a setlist, and read it without fighting the software.

This project was built with substantial AI assistance. It is a working, evolving application—not a claim of polished, fully audited software or complete MobileSheets feature parity. Back up your library, try it with copies first, and test your actual reader and pedals before trusting them in a performance.

## What it does
- **Read PDF, PNG, and JPEG scores** in separate Library and Reader views. Each standalone image is a single-page score.
- **Manage a music folder** with recursive imports, automatic discovery while the app is open, content-based deduplication, and readable filenames.
- **Organize and find scores** using editable metadata, search, genre/instrument filters, collections, and ordered setlists with repeated entries.
- **Detect instrument labels offline** using embedded PDF text first, then Tesseract OCR when needed. Review proposals before assigning instruments.
- **Store several instruments per score**, with optional subtype and part number, and protect manually entered choices.
- **Read during performance** with single/two-page display, fit-width and zoom controls, bookmarks, remembered positions, fullscreen, and page turning by keyboard, tap/click, or keyboard-emulating pedal.
- **Keep the reader awake**, optionally requesting temporary screen-idle and suspend protection without changing permanent desktop settings.
- **Find instructions inside the app** using a searchable, clickable offline help tree.
- **Recognize printed music and audition selected parts on Linux** using optional Audiveris and FluidSynth helpers, with MusicXML/MIDI export, instrument choices, and tempo scaling.
- **Balance accompaniment live** with per-part selection/volume, a master volume slider, play/pause, seek and stop; settings are remembered per recognized movement.
- **Use native VST3 instruments** with shared discovery, independent per-part state and X11 editors through the optional host backend. Play SFZ through a separately acquired sfizz VST3 sampler.
- **Reunite split OMR parts** with explicit selection, retained instrument/settings, overlap previews and reversible combinations, without rewriting original notation.
- **Annotate pages** using constant-width pen/highlighter ink, whole-stroke erasing, colors, thickness, undo/redo and separate autosaved page overlays.
- **Read ahead during playback** using optional original-score page following with an integer measure lead. Manual Previous/Next remains available in Performance without the synth.
- **Choose paths instead of typing them** using Browse dialogs for music engines, SoundFonts, MusicXML input and export destinations.

Original score documents are not rewritten by metadata editing or detection. Instrument metadata is stored in the local library database, **not embedded into the PDFs/images**. Scores are not uploaded for OCR.
## Spotlight: offline instrument detection
Finding the right part should not depend on a perfectly named file. RustyMusicSheets can inspect instrument labels printed inside scores and propose structured metadata you can review, save, and filter on.

- **Text PDFs and scans:** it extracts embedded PDF text first, then uses Tesseract OCR when needed for scanned pages and PNG/JPEG scores.
- **Consistent instrument names:** labels such as “Sax 1” can be categorized as **Saxophone, part 1**, while “Violoncello” is normalized to **Cello**.
- **Subtype and part information:** an explicit “Alto Saxophone 1” label can retain **Saxophone / Alto / 1**, rather than losing everything except the instrument family.
- **Ensemble scores:** more than one instrument can be assigned to a score, so a duet can appear under either instrument's filter.
- **Evidence you can check:** review the recognized text, source page, extraction method, and OCR confidence before applying proposals.
- **Optional automation:** detection on new GUI imports can fill clear, empty, unprotected instrument fields without replacing your manual choices.
- **Local processing:** no cloud OCR service, account, or score upload is required.

Once applied, instrument metadata becomes searchable and filterable in Library—for example, to find cello parts or alto saxophone scores even when the filenames do not mention those instruments.

This is conservative label recognition, not musical-notation transcription or a guarantee of correct categorization. A bare “Alto 1” could mean several things and is deliberately left for review. Large libraries, especially scans, may take a while; start with a filtered batch and cancel from the status bar if needed.

See [Scan, review, apply](#instrument-detection-scan-review-apply) for the workflow.

## Install
### Option 1: Ubuntu/Debian package
First check the reader's package architecture and glibc version:

```bash
dpkg --print-architecture
getconf GNU_LIBC_VERSION
```

The published package needs **amd64** and **glibc 2.43 or newer**. If the architecture differs or glibc is older, build on your target distribution instead. Download the [v0.6.0 preview package](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/rustymusicsheets_0.6.0-1_amd64.deb) and [SHA256SUMS](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.6.0/SHA256SUMS) into the **same directory**. Verify the files you downloaded, then install with APT so dependencies are resolved:

```bash
sha256sum --ignore-missing -c SHA256SUMS
sudo apt install ./rustymusicsheets_0.6.0-1_amd64.deb
rustymusicsheets --help
rustymusicsheets --check
```

These commands are for a compatible reader; check the architecture and runtime requirement below first. New downloads are published as GitHub Release assets. Development builds live in ignored `dist/`; [`packages/`](packages/) contains older selected packages, not the current preview. Build from source if the package does not match your system.

All six release attachments have flat filenames. `--ignore-missing` permits verifying just the downloaded subset; to verify the entire set, download every attachment and use `sha256sum -c SHA256SUMS` without that option. The build record is unsigned provenance, not a cryptographic attestation; checksums are not a release signature.

**Compatibility matters:** the current native amd64 build targets the confirmed **Ubuntu 26.10 x86_64** reader and requires **glibc 2.43 or newer**. It is **not compatible with Ubuntu 22.04/24.04**. A `.deb` is not a universal Linux binary: use a matching package or build on your target distribution. Do not force-install incompatible packages or replace system glibc.

The published package includes `/usr/libexec/rustymusicsheets/rms-audio-host` with protocol-v2 SoundFont/VST3 and X11 editor support. APT resolves its system runtime libraries, including FluidSynth, JACK and OpenSSL, plus Poppler/Tesseract/graphics dependencies. **You do not need a compiler or the VST3 SDK to run this package.** Audiveris, the external FluidSynth executable for legacy playback, SoundFont banks, VST3 plugins and sample libraries are separate optional dependencies. Zenity is recommended for file/folder pickers. An existing JACK or PipeWire-JACK output is required for the host backend; installing the package does not configure an audio server.

#### Upgrade from v0.5.0 or an older preview
1. Save plugin assignments/state while paused or stopped, wait for annotation saves, and close the app. Back up the music folder and complete app data/config directories, including `annotations/` and OMR mixes; see [Data and backups](#data-and-backups).
2. Install the verified v0.6.0 `.deb` with the same APT command above. You do not need to uninstall first or delete your catalogue, saved mixes or annotations.
3. Relaunch the installed app and check **Help → About** for **0.6.0**. If you normally use a source-preview launcher, it may still point to a different binary; rebuild that checkout explicitly or launch the installed application.
4. In **Settings → Music recognition and MIDI playback**, choose the intended backend. To use the packaged host, leave **Audio-host executable** empty and save preferences; an old absolute source-helper path otherwise remains selected.
5. For wrappers quarantined by the old compatibility rejection, use **Retry this bundle** or **Retry quarantined / rescan** in shared VST3 discovery. The non-blocking notification does not automatically clear quarantine or change an assignment. The far-left workaround remains relevant if an editor ignores mouse input.

`--help` reports the executable version; `--check` checks PDF/OCR helpers, **not** JACK, plugin audio, licensing or editor compatibility. No Wine downgrade, prefix migration or automatic yabridge `sync` is required by this policy change.

#### Packaged VST3 quick start
No source build is needed for the published `.deb`. Select **Audio host preview — PipeWire/JACK** for an existing PipeWire-JACK desktop, or **Audio host preview — native JACK** for an existing JACK server. Leave the helper path empty, save preferences, then open **VST3 bundle discovery (shared by all parts)** and **Probe helper capabilities**; expect `vst3` and `editor_keepalive`.

Inspect an outer Linux `.vst3` bundle once, or add existing yabridge wrapper roots explicitly, then choose its exact class for each part. Use **Load instruments paused → Open editor**, start at a low master level, and **Save plugin assignments and state** while paused/stopped. Browsing or scanning does not assign an instrument. See [the complete instrument workflow](#5-browse-assign-and-configure-an-instrument) for SFZ/sfizz, presets and state limitations.

### Option 2: Build from source
You need **Rust 1.86 or newer**, Cargo, Git, a C compiler, pkg-config, and the graphics/input development libraries required by eframe on your distribution. The app uses OpenGL and supports X11 and Wayland.

On Ubuntu/Debian, install the runtime helper programs:

```bash
sudo apt install poppler-utils tesseract-ocr tesseract-ocr-eng zenity
```

Zenity is optional; you can paste paths or drag and drop files/folders instead. Other OCR languages require their corresponding Tesseract language-data packages.

For a fresh checkout:

```bash
mkdir -p ~/Projects
git clone https://github.com/ripleyeldridge/rustymusicsheets.git ~/Projects/LinuxSheets
cd ~/Projects/LinuxSheets
git switch --detach v0.6.0
cargo build --locked --release
./target/release/rustymusicsheets
```

The example checks out the immutable release tag to reproduce the published source version. For ongoing development, use `git switch main` and `git pull --ff-only`; annotations are already merged into `main`, so no feature-branch checkout is required.

If you already have the repository, preserve local edits before switching revisions, fetch the tag with `git fetch origin tag v0.6.0`, and build from your chosen checkout. A Cargo app build alone does **not** build the optional audio host; follow [the separate helper instructions](#2-build-both-binaries) for source VST3 playback. If compilation reports missing native development libraries, install the appropriate eframe/X11/Wayland/xkbcommon development packages for your distribution.

To build a native Debian package without installing it or requiring root:

```bash
python3 scripts/build-deb.py
```

Packaging also requires Python 3.9+, dpkg-dev, dpkg-deb, and binutils. The builder performs a locked release build and writes `dist/rustymusicsheets_0.6.0-1_<architecture>.deb`. `--revision 2` selects Debian revision 2; `--skip-build` is only for a separately verified, current release executable. See [Publishing a release](#publishing-a-release) for commit/tag/checksum requirements.

## Start the app
**After installing the package:** launch **RustyMusicSheets** from your desktop application menu, or run:

```bash
rustymusicsheets
```

**From the source checkout:** run:

```bash
~/Projects/LinuxSheets/run.sh
```

The wrapper builds a release binary if none exists. It does **not** check whether an existing binary is stale; after changing or updating source, rebuild with `cargo build --locked --release`.

### First use
1. Open **Library**, enable **Options**, and find **Music library folder**.
2. Click **Choose…**, or enter a path and click **Set folder**.
3. Wait for the initial scan. Supported files in that folder and its subfolders are registered; new arrivals are discovered while the app is running.
4. Select a score to open **Reader**.
5. Use **Previous/Next**, **PageUp/PageDown**, or tap/click the left or right half of the page to turn pages.
6. Return to **Library** to search, edit metadata, or build a setlist.

Files imported from outside the selected folder are copied into it. Existing files inside that folder are registered in place. Automatic watching does not delete missing-score records; explicit **Rescan** safely reconciles missing files after making a metadata backup. Rescan never deletes score documents.

## Where to find help
Press **F1**, or choose **Help → Help contents**, for the built-in offline user guide.

- **30 clickable topics** cover setup, imports, folder watching, metadata, instruments/OCR, annotations, MusicXML/MIDI playback, the audio-host preview, setlists, reading, pedals, settings, backups, CLI use, and troubleshooting.
- **Search help** searches topic titles, keywords, and article text. Click a result to read it; **Clear** restores the full tree.
- **Help → Keyboard shortcuts** opens the keyboard and pedal reference.
- **Help → About RustyMusicSheets** shows the version, author, release year, license, and GitHub profile link.
- **Close help** or **Escape** returns without losing metadata drafts; enabled synth page following catches up to playback when Reader returns.

Help is embedded in the executable and works without an internet connection. The [detailed text guide](README.txt) is also available in this repository. Installed packages include documentation under `/usr/share/doc/rustymusicsheets/`.

### Instrument detection: scan, review, apply
1. In **Library → Options → Instrument detection**, choose the current score, filtered scores, or the entire library.
2. Wait for detection to finish.
3. Open **Review instrument results** or a score's **Review** button.
4. Choose the score, tick the correct candidates, and click **Apply selected instruments**.

**Detection may take a while for large libraries**, especially scanned PDFs and images that require OCR. Try a filtered batch first. You can keep reading and cancel detection from the status bar.

Manual scans create proposals; they do not silently write assigned metadata. Existing protected choices require explicit replacement confirmation. For manual entry, use **Edit metadata → Instruments → + Add instrument**, then **Save metadata**.

Automatic detection for new GUI imports is optional and off by default. It fills only clear, empty, unprotected instrument values. Ambiguous labels such as bare “Alto 1” are not blindly treated as saxophone.

## Handwritten annotations (introduced in v0.5.0)
Open a PDF/PNG/JPEG score in Reader or Performance and choose **Annotate**. Original documents remain unchanged. Annotation mode provides **Pen**, translucent **Highlighter**, **Stroke eraser**, **Hand / Pan**, color/thickness, **Undo**, **Redo**, **Save now**, **Show ink**, and **Done**. A tap makes a dot. The eraser removes whole intersected strokes, not individual pixels; **Clear page…** requires confirmation and is undoable within the session history.

Marks use page-relative vector coordinates, so they stay aligned through resizing, zoom, scrolling, fit-width and two-page view. Draw on either visible page; a gesture cannot spill onto the adjacent page or gutter. Choose Hand / Pan to drag-scroll, or Done to restore click-to-turn. Explicit page buttons and keyboard/pedal navigation work between gestures; finish a gesture before navigating. Escape leaves annotation mode before exiting Performance. Ctrl+Z, Ctrl+Shift+Z/Ctrl+Y and Ctrl+S apply to annotation undo/redo/save while the tools own input.

Entering annotation mode disables synth page following to avoid turning the page under your pen; audio may continue. Re-enable following explicitly when finished. In Performance, expand **Accompaniment (audio controls)** for audio controls while annotating.

### Saving and recovery
The app verifies the original score identity before editing. Completed gestures are autosaved in the background; watch the saved/pending/error status and use Save now to flush. Ink is stored in app-data `annotations/<score-sha256>/<zero-based-page>.ink-v1.json`, not embedded in the PDF/image or library SQLite schema. Renaming/moving identical score content preserves annotations; changed content does not inherit old ink. Repeated setlist occurrences share the score's annotations.

Atomic writes and content-token conflict checks protect the last saved page. Save errors retain unsaved ink in memory, with retry/reload controls; rejected sidecars are read-only until an explicit backed-up reset. Reload/discard actions require confirmation. Score switches and normal closing wait for pending saves; recovery dialogs offer retry, cancellation or explicit discard. A sudden crash/forced close can lose an in-progress or not-yet-acknowledged gesture: a visible stroke is not proof it reached disk.

Limits are 8 MiB, 200,000 points and 10,000 strokes per page. The cache holds at most eight pages, with bounded session undo/redo history; unsaved pages are not evicted. At a limit, resolve/save/erase rather than expecting silent truncation. A write-lock error can mean another reader instance or a stale `.json.lock` after a crash. Close all writers and preserve the page/lock before manually resolving a stale lock; the app does not automatically delete an unknown owner's lock. Include the entire `annotations/` directory in app-data backups. Rescan/removal does not delete these sidecars.

### Pen and platform boundaries
A stylus/Surface Pen works as basic ink when the OS exposes it as pointer/touch input; a mouse works too. This version uses constant-width ink and one active contact. It does **not** guarantee pressure sensitivity, palm rejection, pen barrel/inverted-eraser buttons or particular hardware. Configure/test your device through its normal OS tools; the app does not install drivers or change global input settings.

Shared drawing/storage is kept compatible with the Android foundation, but actual device behavior needs testing and Android PDF/OMR/audio limits remain. Typed text, musical stamps, moving/selecting objects, partial-stroke erasing and flattened annotated PDF/image export are not implemented. Annotations are initially visible inside the reader only; original score/MusicXML and MIDI exports do not contain the drawn overlay.

## Music recognition, MusicXML export, and MIDI playback (Linux)
This is an **experimental, review-first OMR workflow**, separate from instrument-label detection. Audiveris recognizes printed notation into MusicXML; RustyMusicSheets interprets it using legacy external FluidSynth or the optional native SoundFont/VST3 host. No scores are uploaded, and source documents and library metadata remain unchanged.

### Setup
- Use **Browse…** beside the Audiveris, FluidSynth and SoundFont fields instead of typing filenames. MusicXML input and both export destination fields also have format-appropriate open/save dialogs. Library/import path controls provide file/folder selection. Text entry and paste remain available; Linux dialogs use optional Zenity.
- Install [Audiveris](https://github.com/Audiveris/audiveris/releases) separately using its official Linux package. Recent releases include Java. Configure its executable in **Settings → Music recognition and MIDI playback**, or **Reader → Music / OMR → Engine settings**. An absolute path such as `/opt/audiveris/bin/Audiveris` works; a Flatpak command with arguments is not an executable path—use a launcher script if necessary.
- For legacy playback, install FluidSynth and a SoundFont, for example `sudo apt install fluidsynth fluid-soundfont-gm`. Choose a `.sf2`/`.sf3` bank in settings; common system banks are detected automatically. Pure-VST3 host playback needs neither that executable nor a bank, but still uses the package's system runtime libraries.
- Audiveris needs its own OCR language setup, even if the application's existing Tesseract label detector works. Install English through Audiveris's language management, or point `TESSDATA_PREFIX` at compatible full [Tesseract tessdata](https://github.com/tesseract-ocr/tessdata) models before launching the app. LSTM-only distro models may not provide the legacy data required by Audiveris.
- Save preferences. Missing helpers, unsupported targets, malformed notation, and audio-device failures are reported. The published v0.6.0 package includes the audio-host helper; recognition engines, plugins and instrument data are not automatically installed.

### Reader workflow
1. Open a library score, then **Music / OMR → Recognize with Audiveris**. Recognition runs in the background and can be cancelled.
2. If several movements are exported, choose one and **Load movement**. Previously successful results are retained if another recognition attempt fails.
3. Review warnings and the exported notation. Part names and instrument guesses can be wrong: a cello line may be labelled “Voice.” Choose **Cello (GM 43)** explicitly when needed; Piano is GM 1. Numbers in the UI/CLI are one-based.
4. Tick the parts to hear and set each **Volume %** independently. A recognized piano grand staff stays one selectable part, with both staves and voices. Part choices, volume, instruments and tempo apply live and are remembered for that recognized movement.
5. Use **Play selected parts / Pause**, **Rewind 5s**, **Fast-forward 5s**, and **Stop audio**. Pause freezes position and silences active voices; resuming or seeking reconstructs notes held at the target position. Stop resets to the beginning. Seek while paused to stay paused, or seek before the first Play to prepare a starting position. Instrument envelopes restart on reconstructed notes; this is musical-position seeking, not sample-exact audio seeking.
6. Enter destination filenames and **Export original MusicXML** or **Export selected MIDI**. Existing files are never overwritten. `.mxl` remains compressed MusicXML; use the matching extension. MusicXML preserves all original recognized parts; MIDI reflects your combined layout, selected parts, instruments, per-part channel volumes, and tempo.

### Combine split OMR parts
If Audiveris creates separate parts for a full instrument name and its later abbreviation, use **Combine parts…** beside the recognized parts or in **Parts / volume**. Nothing is combined automatically from names such as Vln., Vc. or V0.
- If instruments are loaded, finish any editor/file dialog, then **Save and unload instruments**. Saving runs asynchronously; cancellation retains the last valid checkpoint and requires a movement reload before structural changes.
- Use the separate combination checkboxes to select at least two ungrouped original IDs. Choose the retained target ID and a display name, then **Preview combination**. Review note ranges, simultaneous/unison/duplicate counts and each member's settings before **Apply combination**.
- Notes overlay at their original absolute times; they are not concatenated, silent gaps remain, and no cross-part ties are inferred. Playback/MIDI's existing overlapping-unison coalescing may affect doubled-note articulation or loudness; raw notes are never deleted.
- The target supplies the instrument/state, gain, volume and mute setting. Absorbed parts' settings remain stored but their plugins are not loaded. **Uncombine** or **Restore original part layout** restores the original notes/names; subsequent target instrument edits stay with that target.
- Close Combine parts and press Play explicitly to restart from the beginning. Combinations affect playback and MIDI only; **Export original MusicXML** preserves the original bytes and parts.

Groups and original-ID settings are stored in exact-score/MusicXML-bound `.audio-v3.json` sidecars. Older v1/v2 mixes are read as fallbacks and left untouched. Rejected/stale mixes are protected from autosave; the explicit recovery action preserves a backup before saving the current reviewed layout/settings.

After a cancelled/failed instrument save, use **Reload movement (discard unsaved edits)** before regrouping. For a rejected saved mix, **Recover using current layout (back up rejected mix)** preserves a separate copy before replacing it. Changed preview settings require a fresh preview. Escape closes the combination editor before leaving Performance, and page-turn shortcuts are suspended while it is open.

### Performance accompaniment
The **Master volume %** slider below the transport controls adjusts all instruments together, from mute (0%) to normal output (100%), without changing their individual balances. It applies live and is remembered per recognized movement. The same slider appears in Music / OMR. This is a playback-output control; exported MIDI retains the individual part levels.
After saving engine settings, enter **Performance**. The bottom **Accompaniment** strip automatically loads saved recognition for the current score without opening settings. If no usable result exists, click **Recognize music** and use the visible progress/cancel controls. Multiple movements require an explicit **Load movement** choice.

The strip keeps **Rewind 5s**, **Play/Pause**, **Fast-forward 5s**, **Stop**, and position/duration available while the score remains visible. Open **Parts / volume** to choose individual parts, set independent levels/instruments and tempo, or combine fragments. Unchecking a part mutes it without restarting the timeline. Legacy playback and MIDI support up to 15 pitched parts; MIDI percussion channel 10 is reserved. The native host supports up to 32 parts. **Music settings / results** opens the full settings/review/export window without leaving performance mode.

Closing the music window does not stop accompaniment; the performance strip remains available. Changing/removing the score, detecting a changed backing file, or exiting the application stops the synth. Escape closes an open music window/mixer before exiting performance.

### Automatic page turning (0.3.0)
Enable **Follow score pages** below the transport controls in Performance or Music / OMR. It is opt-in and remembered per recognized movement. The reader then follows the synth's actual musical timeline, including tempo changes, rests, repeats, alternative endings and forward/backward seeks. The entire score controls pagination, regardless of which parts are muted.

This uses **Audiveris original source-sheet references and preserved page/system breaks**, not equal-duration page estimates or printed page-number labels. Covers and skipped sheets retain their physical PDF page indexes. If the metadata is missing, conflicting, reflowed inconsistently or outside the current document, the checkbox is disabled with an explanation; musical playback still works. Re-recognize the original PDF when necessary. Review matching page layout before enabling following, especially after editing MusicXML externally.

Manual taps/pedals, Previous/Next, page-number jumps, Home/End and bookmark navigation turn following off until you explicitly re-enable it. Automatic changes preserve normal page rendering and remembered positions. In two-page mode, no turn occurs while the playing page is already visible. Pause freezes the clock; seeking while paused updates the target page. Stop does not force page 1, and finishing never starts a different setlist score. Visual following is suspended while Help or Library is visible and catches up on return to Reader.

Set **Turn pages early by … measures** to a nonnegative integer for read-ahead. Zero retains boundary-based turning; one turns at the start of the last played measure before the next page, two turns two measures early, and so on. Counts use the repeat-expanded musical timeline, including variable measure lengths and tempo changes—not fixed seconds. Oversized values are capped so the current page is not skipped entirely. Preview targets remain stable across the actual page boundary and work for backward repeat turns and paused seeks. The setting is saved per recognized movement. For the two-page cello/piano test piece, one-measure lead moves the page-2 target from measure 19 (36 seconds at 120 BPM) to measure 18 (34 seconds).

**Previous page / Next page buttons remain visible in Performance even without recognition or synth playback.** They respect document/setlist boundaries and two-page mode; using them disables auto-follow just like tapping or a pedal.

Browse for MusicXML input or an export destination only fills the field: use **Load MusicXML**, **Export**, or **Save preferences** explicitly afterward. Export dialogs suggest the matching extension and add it when omitted; existing destination files are still never overwritten. Cancellation leaves fields unchanged. Settings fields require Unicode-representable filenames rather than silently replacing path bytes. A per-score chooser result is discarded if the score/movement changed while it was open.

Use **Load corrected or existing MusicXML** to load a private copy of a `.mxl`, `.musicxml`, or `.xml` file edited externally, for example in Audiveris or MuseScore. This does not import a new library score or alter assigned instrument metadata.

Successful recognition artifacts and logs live under the app-data `omr/<source-hash>/run-*/` tree, with a small `omr-index/` for reuse across restarts. Per-movement mixer settings live under `omr-mix/`, keyed by source and MusicXML content hashes. Include app data in backups. Failed/cancelled runs and temporary helper files are cleaned up. The backing score is hash-checked when loading results; detected later changes invalidate playback until the score is rescanned/reopened.

Supported playback includes partwise MusicXML, chords/voices, multi-staff parts, rests, transposition, ties, tempo changes, basic repeats and first/second endings. Recognition and performance are **not guaranteed faithful**: expression, pedal, grace notes, advanced navigation/nested repeats and other unsupported constructs are warned about or approximated. Default GUI playback drives a persistent FluidSynth command process; the optional host below renders SoundFonts and VST3 instruments. The CLI retains one-shot MIDI playback. Note velocity is fixed, with independent part volume. A full notation editor, microphone/live-performance score tracking, looping, metronome, and Android/Windows OMR/audio are not implemented.

Some OMR exports leave a layout-only `implicit="yes"` measure in one part. If another part supplies its timing, that placeholder becomes a rest on the shared measure clock, with a warning identifying the part and measure position. Its attributes, tempo/repeats and page boundary are retained; later notes are not shifted by a guessed full bar. If every part lacks timing at that position, loading still fails for review rather than inventing a duration. The source MusicXML and score are not rewritten.

Audiveris is **AGPLv3**, FluidSynth is **LGPL**, and SoundFonts have their own licenses. Legacy playback uses separately installed programs/data; the optional helper below links the system FluidSynth library. Their licenses are not replaced by this application's MIT license.

### Optional audio host and VST3 instruments (development preview)
The separately supervised C++ host provides bounded/versioned Unix-socket control, shared-clock SoundFont/VST3 mixing, isolated plugin scanning, per-part instrument assignments, generic parameters/presets and saved plugin state. Custom editors use separate X11 windows when supported. **VST3 requires an SDK-enabled helper**; the ordinary helper build below remains SoundFont-only.

Legacy FluidSynth playback remains the default. The preview targets native Linux x86_64. Ordinary Cargo/Android builds do not build/download the helper or SDK. **Wine/yabridge are not needed for native Linux VST3**; they are only relevant to existing Windows-plugin wrappers. A SoundFont-only helper does not need the SDK.
#### Using the published v0.6.0 package
The published `.deb` already contains the SDK-enabled helper. Sections 1–4 below describe **source builds**, not prerequisites for installed-package users; section 5 covers instrument setup for either route.
1. With an existing JACK/PipeWire-JACK output available, open **Settings → Music recognition and MIDI playback** or **Music / OMR → Engine settings**, and choose the matching **Audio host preview** backend.
2. Leave **Audio-host executable** empty to use `/usr/libexec/rustymusicsheets/rms-audio-host`. Clear an old source-preview path if you intend to use the packaged helper. Save preferences, then use **Probe helper capabilities** in shared discovery; look for `vst3` and, for modal X11 editors, `editor_keepalive`.
3. Inspect/scan a separately installed native `.vst3` bundle once in **VST3 bundle discovery (shared by all parts)**, then choose the desired class for each part.
4. **Load instruments paused**, configure the editor/parameters/preset, and **Save plugin assignments and state** while paused/stopped. A pure-VST3 graph needs no SoundFont bank; SoundFont parts and legacy playback do. Start at a low master level.

The host uses existing audio services; it does not start JACK, set up PipeWire, install plugins or repair Wine prefixes. Custom editors require supported X11/display integration; generic controls do not provide native Wayland editor embedding.

#### 1. Prepare the optional dependencies
Complete the normal source-build setup above, including Rust >=1.86. The helper additionally needs CMake >=3.16, a C++17 compiler, pkg-config and development packages for JACK, FluidSynth >=2.2 and OpenSSL. Ubuntu package example:

```bash
sudo apt update
sudo apt install build-essential cmake pkg-config libjack-jackd2-dev libfluidsynth-dev libssl-dev
```

For an **existing PipeWire desktop**, its JACK compatibility launcher and an example SoundFont:

```bash
sudo apt install pipewire-jack fluid-soundfont-gm
command -v pw-jack
pkg-config --modversion jack fluidsynth openssl
```

These are manual setup commands, not actions performed by the application or build scripts. Review the package manager's proposed changes. Another compatible SF2/SF3 bank is fine. A FluidSynth runtime/executable alone does **not** provide the development headers or `fluidsynth.pc`. Do not replace your existing audio server: use PipeWire/JACK for a running PipeWire desktop, or native JACK only if you already run a JACK server.

#### 2. Build both binaries
From the repository root:

```bash
cargo build --locked --release
python3 scripts/build-audio-host.py --test
./target/release/rustymusicsheets
```

The outputs are `target/release/rustymusicsheets` and `target/audio-host/rms-audio-host`. Launch the newly built app, not an older installed version lacking the backend selector. Cargo does not build the helper. `--test` runs native scheduling/protocol tests without opening an audio device.

For scheduler tests without JACK/FluidSynth development dependencies, use a **separate directory**:

```bash
python3 scripts/build-audio-host.py --core-only --test --build-dir target/audio-host-core
```

Core-only mode does not create a live helper; it also tests the protocol when OpenSSL development files are available.

#### 3. Enable and test the preview
1. In **Settings → Music recognition and MIDI playback**, or **Music / OMR → Engine settings**, select **Audio host preview — PipeWire/JACK** or **Audio host preview — native JACK** under **Playback backend (Linux)**.
2. Use **Browse…** for **Audio-host executable** and select the absolute path of `target/audio-host/rms-audio-host`. An empty field instead selects `/usr/libexec/rustymusicsheets/rms-audio-host`, which requires a helper-enabled package; it does not search PATH.
3. For SoundFont parts, choose an existing `.sf2`/`.sf3` **SoundFont** and save preferences/music settings. A pure-VST3 session needs no bank file. The external FluidSynth executable field is for legacy playback, not this backend.
4. Load existing MusicXML or recognize a score using the workflow above. Audiveris is only needed for recognition, not existing MusicXML. Start with a low master level, then **Play selected parts**. Sample rate, estimated output latency and xruns appear with the transport.
5. To return to the previous backend, choose **FluidSynth (legacy, default)** and save preferences. Backend changes stop playback; explicitly press Play to start the selected engine. There is no silent fallback or restart loop.

PipeWire mode launches only the helper through `/usr/bin/pw-jack`; native JACK never starts a server. Both require an existing stereo physical output. The preview connects to the first two physical playback ports, without a device-routing picker.

The preview supports up to 32 independent musical parts, preserving piano staff grouping, legacy CC7 part levels and a smoothed master gain. MIDI export and legacy live playback still have their 15-pitched-part limit. A host failure stops audio and displays an error without closing the reader or silently switching engines; press Play explicitly to retry. Only owned processes/private temporary files are cleaned up.

Position and page following use estimated audible device-clock timing, including output latency and tempo changes. **Real loopback calibration is still required.** FluidSynth may delay note onset within its internal 64-frame synthesis quantum. Preview chorus/reverb are disabled to flush voices on pause/seek, so its sound is not acoustically identical to the legacy engine. Each part currently loads its own SoundFont instance, which can use substantial RAM. Test levels, timing and memory before relying on it in performance.

#### 4. Build VST3 support explicitly
For plugin loading, additionally supply **CMake >=3.25** and X11 development headers (`libx11-dev` on Ubuntu). Acquire the pinned official MIT-licensed SDK explicitly, then rebuild the helper:

```bash
python3 scripts/fetch-vst3-sdk.py
python3 scripts/fetch-vst3-sdk.py --verify-only
python3 scripts/build-audio-host.py --test --vst3-sdk "$PWD/target/vst3-sdk"
target/audio-host/rms-audio-host --build-info
cargo build --locked --release
./target/release/rustymusicsheets
```

SDK setup downloads only the pinned official source/submodules to ignored `target/vst3-sdk/`. It never overwrites an existing modified checkout, installs packages or touches Wine. Normal Cargo/CMake/Android builds do not download it. SDK-enabled and non-SDK helpers use protocol v2; rebuild the app and helper together rather than mixing them with the older M0 protocol-v1 binary.

#### 5. Browse, assign and configure an instrument
1. Choose an audio-host backend and its freshly rebuilt helper as above. In application **Settings**, or **Music / OMR → Engine settings**, expand **VST3 bundle discovery (shared by all parts)** and use **Probe helper capabilities**. The reply must include `vst3`. No loaded score is required to discover plugins in Settings.
2. Browse to the **Direct .vst3 bundle folder** in that shared section, then **Inspect this bundle**. Alternatively, use **Add search folder → Browse…**, then **Scan selected roots**; **Use standard Linux roots** fills existing conventional folders. Scan each bundle once: the shared catalogue populates every part's dropdown. Browsing/scanning never assigns a part automatically; browsing alone never executes a plugin. Use **Retry this bundle** or the root rescan to retry quarantine explicitly.
3. Choose the exact instrument class from each part's dropdown. Vendor, module path and class ID disambiguate duplicate names; two parts may choose the same plugin independently, with separate instances, gain and state. The selected bundle is shown read-only in the part; bundle browsing/scanning is no longer repeated per part. The previous GM program is retained for explicit SoundFont selection and MIDI export, not sent to the VST3 instrument.
4. Use **Load instruments paused** to prepare the graph without playing notes. Use **Open editor** for a supported custom X11 window, **Generic parameters** for paginated host controls, or Browse to a class-matching `.vstpreset` and **Load selected .vstpreset** while paused/stopped. Use a low master level initially. Plugin level is **Post-plugin audio gain**, not CC7.
5. While paused/stopped, use **Save plugin assignments and state**. Assignments/checkpoints are tied to the original score and MusicXML hashes; component/controller chunks remain separate. A plugin's external samples/licenses are not bundled. Dirty changes are indicated; missing plugins never silently become SoundFonts.

Linux `.vst3` plugins are **bundle directories**, usually containing `Contents/x86_64-linux/<bundle-name>.so`; choose the outer `.vst3` folder in the browser. A Windows `.vst3` file, VST2 `.dll`/loose `.so`, effect or wrong-architecture plugin cannot be loaded directly. VST2, CLAP/LV2, effects chains and plugin audio export are not implemented. MIDI export contains retained GM choices, not plugin audio/state. Failed scans are quarantined until an explicit **Retry quarantined / rescan** or a changed module/runtime profile.

Initial DSP routing requires zero audio inputs and one mono/stereo main output bus; multi-output instruments are rejected rather than silently misrouted. Supported latency/bus changes pause and reprepare processing/alignment; press Play after reviewing the change. A requested full component reload or unsupported bus layout is reported explicitly. During a paused preset/checkpoint operation, **Stop audio** cancels the owned helper and keeps the last valid checkpoint; unsaved edits may be lost. Reload the movement before restarting after such cancellation. Save before changing instruments. New mix metadata uses `.audio-v3.json` sidecars with original-ID settings/groups, leaving v1/v2 files intact.

#### SFZ instruments through sfizz
SFZ is not a SoundFont or plugin bundle. Use the native Linux x86_64 **[sfizz VST3](https://github.com/sfztools/sfizz-ui/releases/tag/1.2.3)** sampler to play an `.sfz` definition and its sample files. Discover its outer `sfizz.vst3` bundle once in the shared section, then choose **sfizz**, not **sfizz-multi**, for each desired part. The multi-output class is outside this host's routing profile.

Use **Load instruments paused → Open editor**, then choose the `.sfz` inside sfizz. A class-matching `.vstpreset` can also supply the instrument. Use a low master level, play, then pause/stop and **Save plugin assignments and state**. SFZ/sample paths are retained in the plugin's opaque state; the application does not copy, convert or bundle the SFZ, WAV files, includes or licenses. Keep their folder layout available when reopening the movement. Different parts may use different sfizz instances and SFZ instruments.

Rebuild/reopen **both app and helper** for modal-file-dialog support. Some plugin editors synchronously wait for their file picker; the negotiated `editor_keepalive` extension prevents that legitimate wait from being mistaken for a dead host. The transport is held while the editor callback is busy; finish/cancel its dialog to resume, or use **Stop audio** to cancel the owned helper and retain the last saved checkpoint. Finish the dialog before issuing new control/state operations; they are refused while it is busy. The normal five-second missing-heartbeat watchdog remains enabled, with a separate thirty-minute bound on continuous editor activity.

The optional plugin is acquired/built separately; normal application/helper builds never download or install sfizz. Prefer official native builds or official source, preserve the plugin's licenses, and retain a verified build. The official 1.2.3 release provides source rather than a Linux binary. This preview uses bounded plugin process blocks independently of its larger scratch buffers, avoiding sfizz's oversized-block assertion. SFZ remains unsupported in the `.sf2`/`.sf3` SoundFont picker, and no second SFZ engine or vendor-specific production state format is added.

Official sfizz 1.2.3 was verified with a GCC 13 native build; its bundled GUI library fails to build unchanged with the local GCC 15. Development dependencies were staged privately for verification, not installed into the system. The installed plugin remains separate from this repository's binaries, SDK and license.

#### Existing yabridge wrappers
The host loads yabridge's generated **Linux VST3 bundles**, normally under `~/.vst3/yabridge`, through the same VST3 adapter. It does not load the original Windows file or install/activate the vendor plugin.

Use **Read-only bridge diagnostics** before **Use existing yabridge wrappers → Scan selected roots**. The yabridgectl executable, generated wrapper root and existing Wine loader each have Browse controls. The Wine loader is scoped to the helper; there is no global `WINEPREFIX` override, installation, automatic `add`/`sync`/`prune`, prefix modification or Wine downgrade. Vendor installation and user-reviewed yabridge registration/synchronization must already be complete; do not register generated output as a source or recursively nest wrapper roots.

**Known editor-input warning (non-blocking):** Wine 9.22 and newer (including 10.x and 11.x) have known embedded-editor mouse/coordinate failures with yabridge 5.1.1 and earlier, even when audio plays. Diagnostics, scan results and playback controls show a notification, but allow scans and playback; this includes Wine 11.22 Staging with yabridge 5.1.1. Workaround: move the editor to the far left of your desktop / leftmost screen; this may restore mouse interaction but does not establish compatibility. Updating only to 5.1.1 is not a fix. A separately verified existing runtime/bridge profile is another option; consult [official yabridge guidance](https://github.com/robbert-vdh/yabridge#troubleshooting-common-issues). No Wine/prefix/registration changes happen automatically. Version preflight permits an explicit attempt, not a guarantee of plugin audio/editor/state compatibility.

##### Bridge diagnostic behavior
- Run **Read-only bridge diagnostics** in application Settings or Music / OMR engine settings. It probes the selected Wine loader and `yabridgectl` versions and may read source registrations; it does not load a plugin, synchronize wrappers, install software or modify Wine prefixes. The CLI `--check` command checks PDF/OCR helpers, not this bridge profile.
- With **yabridge ≤ 5.1.1**, **Wine ≥ 9.22** triggers an editor warning without an upper-major-version cutoff, including Wine 11.x and later majors. For example, Wine 11.22 Staging with yabridge 5.1.1 produces `yabridge editor warning: Wine 11.22.0 with yabridge 5.1.1.`
- This non-blocking notification permits **scans, instrument loads and playback**. Missing tools, failed/unparseable version probes, invalid settings and actual plugin/helper failures remain errors. **Native Linux VST3 instruments are exempt** and do not require Wine/yabridge. No version pair certifies editor, audio or saved-state compatibility.
- This changes policy, **not the editor embedding itself**. The notification includes the far-left workaround. Runtime/bridge selection, upgrades and wrapper synchronization remain separate, user-directed actions; existing plugin assignments/checkpoints are not rewritten. Previously quarantined bundles require **Retry this bundle** or **Retry quarantined / rescan**.
- The published v0.5.0 includes the earlier blocking Wine 11 diagnostic correction. v0.6.0 makes it advisory; older packages are unchanged.

#### Optional native package
For source packaging, `python3 scripts/build-deb.py --with-audio-host` builds/packages a SoundFont-only helper. Add `--vst3-sdk "$PWD/target/vst3-sdk"` for the VST3-enabled variant shipped in v0.6.0, with pinned SDK provenance/license notices. Invoking the builder without `--with-audio-host` omits the helper; that is not the published v0.6.0 package configuration. Install the explicitly chosen output `.deb` using your normal package manager; leave **Audio-host executable** empty for its installed location. Building does not install/publish a package or change Wine/yabridge. `--skip-build` checks helper protocol/features and SDK provenance; it does not rebuild stale binaries. Native packages retain build-host architecture/glibc requirements and are not universal Ubuntu binaries.

#### Verification and troubleshooting

An explicit real-helper transport smoke check keeps the master muted throughout (it is ignored by ordinary tests):

```bash
RMS_TEST_AUDIO_HOST="$PWD/target/audio-host/rms-audio-host" \
  cargo test --locked --lib real_audio_host_muted_smoke -- --ignored --nocapture
```

It uses the detected SoundFont and PipeWire by default; optionally set `RMS_TEST_SOUNDFONT` and `RMS_TEST_AUDIO_BACKEND=jack`. This verifies live lifecycle/clock/controls, not acoustic onset or loopback page timing.

`tests/vst3_host.rs` also has explicit muted Rust-to-host checks for independent generated-plugin states and mixed SoundFont/VST3 playback. Set `RMS_TEST_AUDIO_HOST` to the SDK-enabled helper, `RMS_TEST_VST3_BUNDLE` to its generated `VST3/RelWithDebInfo/rms-test-instrument.vst3` bundle, and optionally `RMS_TEST_AUDIO_BACKEND=jack` and `RMS_TEST_SOUNDFONT`. Run the named ignored fixture tests with one test thread so live audio clients do not overlap.

To test a trusted real native instrument instead of the fixture, explicitly set `RMS_TEST_REAL_VST3_BUNDLE` and `RMS_TEST_REAL_VST3_PARAM_ID`, then run `cargo test --locked --test vst3_host actual_native_instrument_plays_and_restores_state_without_soundfont -- --ignored --nocapture`. Also set `RMS_TEST_REAL_VST3_CLASS_ID` for a multi-class bundle; no class is guessed. Choose a writable, persistent continuous parameter from its first parameter page. This executes third-party code, edits that parameter in a temporary instance, checkpoints it, reloads it, and advances playback with master muted. Dexed 1.1.0 has been verified with Output parameter ID `1141971201`; sfizz's stereo class is `E8FAB71815ED46E38B5983101E12993F` and Volume parameter ID is `0`. Plugin defaults/presets and existing movement state are not rewritten.

The sfizz-specific opt-in verifier generates its own SFZ, WAV and standard VST3 preset in a **fresh** directory. It checks all three chord frequencies, opaque checkpoint/reload, parameters, and optionally the X11 editor. It uses a private HOME/config/data/cache and opens **no JACK client or audio device**. Build its target explicitly in your SDK-enabled helper build directory (adjust `HOST_BUILD` if you used `--build-dir`):

```bash
HOST_BUILD="$PWD/target/audio-host"
cmake --build "$HOST_BUILD" --target rms-sfizz-verify
python3 native/audio-host/sfizz_verify.py \
  --binary "$HOST_BUILD/rms-sfizz-verify" \
  --bundle "$HOME/.vst3/sfizz.vst3" \
  --output "$PWD/target/sfizz-verification"
```

The output directory must not already exist. Add `--editor-seconds 3` for the optional X11 editor lifecycle check. This explicitly executes a trusted user-selected sfizz 1.2.3 bundle; it is never run by ordinary CTest/CI and does not download plugins. The fixture's sfizz state-schema knowledge is verification-only, not part of the production loader. To exercise the generated instrument through the Rust supervisor too, set `RMS_TEST_REAL_VST3_PRESET` to the resulting `generated.vstpreset` when running the real-instrument Rust test above, together with sfizz's class/parameter IDs. That test submits a simultaneous three-note chord with master muted and verifies state restoration.

- **Missing `fluidsynth.pc`/headers:** install `libfluidsynth-dev`, not only the runtime/executable; check `pkg-config --modversion fluidsynth`.
- **Missing helper/protocol mismatch:** build both binaries from the same checkout and Browse to the actual output. A source build does not install the helper under `/usr/libexec/`.
- **File picker closes with a heartbeat error:** reload instruments using the rebuilt app/helper pair and probe for `editor_keepalive` on X11. An older pair may confuse a synchronous editor chooser with a stalled host. A busy editor can be cancelled with Stop audio; unsaved edits may be lost, but the last saved checkpoint is retained.
- **JACK unavailable / no physical stereo ports:** match the running server/backend and enable a physical stereo output in the desktop/audio controls. PipeWire mode requires `/usr/bin/pw-jack`; native JACK does not start a daemon.
- **Device/rate/output changed or host stalled:** fix the device/routing and explicitly press Play again. More than ten seconds of reported latency or exhausted retained timing history is rejected, not guessed.
- **High RAM/repeated xruns:** reduce instrument instances and other heavy audio workloads, and review the existing server's buffer settings. Muting does not unload a SoundFont or free its sample RAM. Test on your setup before performance use.
- **Old extracted-header overrides:** `FLUIDSYNTH_INCLUDE_DIR`/`FLUIDSYNTH_LIBRARY` and CMake's cached values are validation overrides, not portable prerequisites. Use a fresh `--build-dir` when changing them and Browse to its helper output. Debian packaging expects the default `target/audio-host/` output.
- **Desktop launch stalls during graphics initialization:** try the command-scoped X11 route below, which passes startup/close validation. This does not change global desktop/environment settings, and is not evidence of native Wayland plugin-editor support:

```bash
env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 ./target/release/rustymusicsheets
```

### Standalone music commands
These commands do not open or change the library database. Output directories/files are explicit:

```bash
rustymusicsheets --omr "/path/to/score.pdf" "/path/to/results" --audiveris /opt/audiveris/bin/Audiveris
rustymusicsheets --inspect-musicxml "/path/to/results/score.mxl"
rustymusicsheets --midi "/path/to/results/score.mxl" "/path/to/piano.mid" --parts P2
rustymusicsheets --midi "/path/to/results/score.mxl" "/path/to/cello.mid" --parts P1 --program P1=43 --play --play-seconds 5
```

Recognition prints the exact generated movement paths; use those in subsequent commands. Part IDs are read from MusicXML, not guessed from this example. Optional MIDI arguments include `--tempo-scale 0.8`, `--soundfont /absolute/bank.sf2`, and `--fluidsynth /absolute/fluidsynth`. CLI engines use their automatic defaults or explicit options, not GUI preferences. `--play-seconds` requires `--play`; without a duration playback continues until the MIDI finishes.

CLI `--midi` reads the original supplied notation and **does not apply GUI part combinations or saved mixes**. Use **Export selected MIDI** in the GUI for a combined layout.

## Useful command-line options
```bash
rustymusicsheets --help
rustymusicsheets --check
rustymusicsheets --import "/path/to/score-or-folder"
rustymusicsheets --list
rustymusicsheets --detect-instruments
rustymusicsheets --detect-instruments 12
```

CLI detection creates proposals only; review and apply them in the GUI. `--check` checks helper executables, not every OCR language package.

## Data and backups
By default:
- `~/.local/share/rustymusicsheets/` stores the SQLite library and default/legacy score copies.
- `~/.config/rustymusicsheets/preferences.json` stores preferences.
- Your selected music folder stores your score documents.

The app honors `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `XDG_CACHE_HOME`. For a complete backup, **close the app after ink saving completes**, then copy the selected music folder **and** the entire app data and config directories, including `annotations/` sidecars. Automatic pre-rescan/schema-migration snapshots contain metadata only, not copies of scores or drawn ink.

## Current limitations
Not implemented yet: typed/stamp annotations or annotated PDF/image export, crop/rotation/page reordering, in-score navigation links, vertical/half-page display, metronome, VST2/CLAP/LV2 hosting, ChordPro/text, assembling image sequences, document printing/export, portable backup/restore, cloud integration, and multi-device sync. Freehand annotations, MusicXML/MIDI export, experimental Linux SoundFont/VST3 playback and compatibility limits are described above.

Instrument OCR remains conservative label detection; musical transcription uses the separate optional Audiveris workflow. Faint scans, handwriting, uncommon abbreviations, and labels outside inspected regions may need manual entry. Audiveris targets printed notation, not handwriting. Physical pedals and touchscreens require device-specific testing. No compatibility is claimed for MobileSheets proprietary formats.

## Development checks
### Platform backends
File selection and keep-awake behavior use shared interfaces with separate platform backends:
- **Linux:** Zenity file/folder dialogs and the existing GNOME/KDE/freedesktop/logind power inhibitors.
- **Windows:** native `rfd` file/folder dialogs and worker-thread-scoped `SetThreadExecutionState` requests for display timeout and idle sleep. Windows screen savers and user-forced sleep are not blocked.
- **Other platforms:** explicit unsupported-status/error messages, with pasted paths and drag-and-drop remaining available for file selection.

Picker results use native paths, with cancellation separated from reported errors. Power guards release their requests when disabled, replaced/retried, cancelled, or shut down; Windows acquire/release operations stay on the same worker thread. Linux-only D-Bus dependencies are excluded from Windows builds.

**This refactor is not a Windows release:** the `.deb` above is Linux-only, and the Android APK is a separate experimental foundation. A complete Windows package still needs its PDF/OCR helper tools, DLLs/language data, a native build toolchain, and Windows runtime testing. Module cross-compilation does not establish runtime compatibility.

### Android foundation
The source now includes a NativeActivity entry point and a document-access layer that distinguishes desktop paths from Android `content://` URIs. An initial Android **Open with** PNG/JPEG can be read through `ContentResolver`, staged privately, and passed to the existing importer without treating the URI as a filename.

This is an **experimental Android prerelease**, not desktop feature parity. Native PDF/OCR, the in-app Android picker, document-tree watching, and Android keep-awake integration remain incomplete. The ARM64 debug APK builds using `scripts/build-android.sh`; signature/manifest/16 KiB alignment checks pass with `scripts/verify-android-apk.py`. The 0.6.0 APK reports versionName `0.6.0`, minimum API 26 and target API 36; its numeric versionCode and signing certificate fingerprint are recorded in `build-record.json`. Earlier foundation builds had Pixel 8 Pro (API 37) startup/safe-area checks; **the 0.6.0 APK has static verification only, not current device execution**. Broader rendering, import/provider, and device testing remains pending. See [Android setup and implementation boundaries](android/README.txt) before attempting a build or installation.

### Validation commands
The published v0.6.0 release passed **336 standard Rust tests, 46 Python tooling tests and 16 native CTests**, including sanitizers, plus formatting/strict Clippy, SDK integrity, package identity/content, extracted-app startup/close and Android static verification. All six uploaded assets were downloaded and byte-verified before publication. [Hosted CI on the matching PR source tree](https://github.com/ripleyeldridge/rustymusicsheets/actions/runs/44564733225) also passed Linux regression, ARM64 static and isolated API 35 **x86_64 emulator** jobs. That emulator result is separate from the locally built ARM64 APK, which was not run on a device. The release build record lists the exact source and checks performed. Nine opt-in Rust tests were not run; historical private dummy JACK runs do not certify this build on physical output. Device-clock latency still needs loopback calibration, and no universal plugin, pedal or stylus compatibility is claimed. The commands below validate the current checkout, which may include later documentation changes.
```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Tests generate their own score fixtures. PDF/OCR integration tests require Poppler, Tesseract, and English language data.

Also run the Python tooling checks:

```bash
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

The ordinary Rust suite uses generated scores and fake synth/dialog helpers. Real-audio smoke tests are intentionally ignored by default and do not run merely because CI is green. With FluidSynth, a SoundFont and a private/public-domain Audiveris export available, an explicit check is:

```bash
RMS_TEST_MUSICXML="/absolute/path/to/score.mxl" cargo test --locked --lib playback::tests::real_synth_page_follow_smoke -- --ignored --nocapture
```

This check plays a short passage and requires consistent original-page metadata with a multi-measure page before the next page. Do not commit personal scores or generated notation/audio as test fixtures.

## Publishing a release
These are **maintainer instructions**, not actions performed automatically by the application or CI. Run them from the repository root after review/merge. The v0.6.0 release already exists: choose an unused next version before following this procedure. Publishing a tag alone neither builds assets nor creates a GitHub Release.

1. Update the root application version in `Cargo.toml` and its own `Cargo.lock` entry, current README headings and the man-page header. Do not replace dependency versions or regenerate the whole lockfile. CLI/About and cargo-apk derive application version metadata from Cargo; numeric Android updates need an increasing major/minor/patch, not just an alpha/beta suffix.
2. Run the validation commands above, review changes, and commit them **before building/tagging**. Start from clean `main`; record the exact source commit. `--skip-build` cannot establish that an old binary matches a new version.
3. Build and inspect a native package, without cross-target/output-directory overrides; the builder expects `target/release/rustymusicsheets`. `SOURCE_DATE_EPOCH` fixes package metadata timestamps, not all compiler/dependency inputs; this is not a bit-for-bit reproducibility claim.

```bash
VERSION=0.6.1  # example next version; must match the committed Cargo version
REVISION=1
TAG="v$VERSION"
REPO=ripleyeldridge/rustymusicsheets
COMMIT="$(git rev-parse HEAD)"
ARCH="$(dpkg --print-architecture)"
PACKAGE="rustymusicsheets_${VERSION}-${REVISION}_${ARCH}.deb"
ASSET_DIR="dist/releases/$TAG"

git status --short
git tag --list "$TAG"
git ls-remote --tags origin "refs/tags/$TAG"
```

The working tree must be clean and both tag checks must show that the proposed tag is unused. Check the committed application version before building:

```bash
cargo metadata --locked --no-deps --format-version 1 |
  python3 -c 'import json,sys; p=next(p for p in json.load(sys.stdin)["packages"] if p["name"]=="rustymusicsheets"); assert p["version"]==sys.argv[1], p["version"]' "$VERSION" &&
SOURCE_DATE_EPOCH="$(git show -s --format=%ct "$COMMIT")" python3 scripts/build-deb.py --revision "$REVISION" &&
dpkg-deb --field "dist/$PACKAGE" Package Version Architecture Depends
```

Stop if the tree is dirty, a tag already exists, or package version/architecture/dependencies are wrong. Extract with `dpkg-deb -x` into a temporary directory and check the **packaged** executable's `--help` version and `--check` output. Inspect the included Help/README/licenses. Never claim older-glibc compatibility just because a build succeeded. Ensure no scores, audio, SDKs, keys or personal configuration entered the archive.

4. Stage only distributable files and checksums. Leave `dist/` and `target/` out of source commits.

```bash
mkdir -p "$ASSET_DIR"
cp "dist/$PACKAGE" "$ASSET_DIR/"
(cd "$ASSET_DIR" && sha256sum "$PACKAGE" > SHA256SUMS && sha256sum -c SHA256SUMS)
```

5. Confirm the source has not changed, create an annotated tag on the built commit, and push branch/tag without force:

```bash
test "$(git branch --show-current)" = main &&
test "$(git rev-parse HEAD)" = "$COMMIT" &&
test -z "$(git status --porcelain)" &&
git tag -a "$TAG" "$COMMIT" -m "RustyMusicSheets $VERSION" &&
git push --atomic origin main "refs/tags/$TAG"
test "$(git rev-parse "$TAG^{}")" = "$COMMIT"
```

Do not move an existing published tag to fix a version mismatch. Prefer a new version; if binaries must be corrected separately, identify their actual build commit and matching source in the release notes. Do not silently rename old-version binaries as a new version.

6. Create a draft preview with verified-tag protection. Add accurate changes, compatibility, optional engine dependencies and verification/signing notes in the draft before publishing.

```bash
gh release create "$TAG" "$ASSET_DIR/$PACKAGE" "$ASSET_DIR/SHA256SUMS" \
  --repo "$REPO" --verify-tag --draft --prerelease --latest=false \
  --title "RustyMusicSheets $TAG Preview" \
  --notes "Built from $COMMIT. Complete the feature, compatibility and verification notes before publishing."
VERIFY_DIR="$(mktemp -d)"
gh release download "$TAG" --repo "$REPO" --dir "$VERIFY_DIR"
(cd "$VERIFY_DIR" && sha256sum -c SHA256SUMS)
cmp "$ASSET_DIR/$PACKAGE" "$VERIFY_DIR/$PACKAGE"
cmp "$ASSET_DIR/SHA256SUMS" "$VERIFY_DIR/SHA256SUMS"
```

Only after download verification and notes review, publish with `gh release edit "$TAG" --repo "$REPO" --draft=false --prerelease --latest=false`. Promoting a preview to stable/latest is a separate deliberate decision. For an existing release, verify replacement uploads before deleting superseded assets and refresh both notes and checksums.

Android assets are optional and **not part of the Linux `.deb` build**. Follow `android/README.txt`, `android/TESTING.txt` and `android/CI.txt`; verify APK version, exact source commit, signature, ABI, manifest and ZIP/ELF alignment, and include a clearly labelled build record. Static checks are not device execution or a production signer. A CI/local debug key may not update a previous APK in place; never uninstall a valuable library just to bypass signing. List every distributable asset in `SHA256SUMS` except that checksum file itself, and never upload signing material, SDKs or private smoke evidence.

GitHub release assets use flat filenames. If local artifacts use subdirectories such as `experimental/`, prepare a separate flat upload directory, update its build-record asset paths and generate its checksum manifest there; do not alter the original build artifacts. Download that complete upload set to a fresh directory, verify checksums and compare bytes before publishing. For v0.6.0 this flat set contains the `.deb`, experimental APK, license, third-party notices archive, build record and `SHA256SUMS`.

## Author and license
Created by **[Ripley Eldridge](https://github.com/ripleyeldridge)**, 2026.

Released under the [MIT License](LICENSE). Third-party dependencies retain their respective licenses.
