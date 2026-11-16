# RustyMusicSheets
A native Linux sheet-music library and performance reader, written in Rust with egui.
**A headline feature: offline instrument detection that helps turn a folder of scores into an instrument-searchable library—even when the parts are scanned PDFs or standalone images.**

**Version:** 0.3.1 preview (source checkout) · **Author:** [Ripley Eldridge](https://github.com/ripleyeldridge) · **Release year:** 2026 · **License:** [MIT](LICENSE)

## 0.2.2 preview downloads
The [0.2.2 prerelease](https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.2.2) includes:
- [Linux amd64 `.deb`](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.2.2/rustymusicsheets_0.2.2-1_amd64.deb), requiring glibc 2.43 or newer.
- [Experimental Android ARM64 debug APK](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.2.2/rustymusicsheets_0.2.2-foundation_arm64_debug.apk), minimum API 26 and target API 36.
- [SHA256SUMS](https://github.com/ripleyeldridge/rustymusicsheets/releases/download/v0.2.2/SHA256SUMS) for both downloads.

**Android is a foundation preview, not a full PDF sheet-music release.** It supports the initial Open with PNG/JPEG import path, local metadata and offline Help. Native PDF/OCR, the in-app picker, document-tree watching, and keep-awake integration are not implemented. Startup was checked on a Pixel 8 Pro; the user confirmed the inset-corrected layout works. Broader import/device testing is still needed. This APK uses a development/debug signature and is not a production-signed Play Store build. The existing [stable Linux 0.2.1 release](https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v0.2.1) remains available.

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
Download the existing stable native package (or use the preview downloads above):

**[RustyMusicSheets 0.2.1-2 — Ubuntu 26.10 amd64 (.deb)](https://github.com/ripleyeldridge/rustymusicsheets/raw/refs/heads/main/packages/rustymusicsheets_0.2.1-2_amd64.deb)**

Then install it using APT so dependencies are resolved:

```bash
sudo apt install ./rustymusicsheets_0.2.1-2_amd64.deb
```

For an optional integrity check, download [SHA256SUMS](https://github.com/ripleyeldridge/rustymusicsheets/raw/refs/heads/main/packages/SHA256SUMS) into the same directory as the `.deb` and run:

```bash
sha256sum -c SHA256SUMS
```

Development builds are generated in the ignored `dist/` directory. The selected downloadable build and checksum are tracked under [`packages/`](packages/). Build from source below if this package does not match your system.

**Compatibility matters:** the current native amd64 build targets the confirmed **Ubuntu 26.10 x86_64** reader and requires **glibc 2.43 or newer**. It is **not compatible with Ubuntu 22.04/24.04**. A `.deb` is not a universal Linux binary: use a matching package or build on your target distribution. Do not force-install incompatible packages or replace system glibc.

The package declares Poppler, Tesseract with English language data, and the graphics/input libraries it needs. Zenity is recommended for graphical file/folder pickers.

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
cargo build --locked --release
./target/release/rustymusicsheets
```

If you already have the repository, skip cloning and build from your existing checkout. If compilation reports missing native development libraries, install the appropriate eframe/X11/Wayland/xkbcommon development packages for your distribution.

To build a native Debian package without installing it or requiring root:

```bash
python3 scripts/build-deb.py
```

Packaging also requires Python 3.9+, dpkg-dev, dpkg-deb, and binutils. The resulting package is written to `dist/`.

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

- **28 clickable topics** cover setup, imports, folder watching, metadata, instruments/OCR, MusicXML/MIDI playback, setlists, reading, pedals, settings, backups, CLI use, and troubleshooting.
- **Search help** searches topic titles, keywords, and article text. Click a result to read it; **Clear** restores the full tree.
- **Help → Keyboard shortcuts** opens the keyboard and pedal reference.
- **Help → About RustyMusicSheets** shows the version, author, release year, license, and GitHub profile link.
- **Close help** or **Escape** returns to your previous view without losing reading position or metadata drafts.

Help is embedded in the executable and works without an internet connection. The [detailed text guide](README.txt) is also available in this repository. Installed packages include documentation under `/usr/share/doc/rustymusicsheets/`.

### Instrument detection: scan, review, apply
1. In **Library → Options → Instrument detection**, choose the current score, filtered scores, or the entire library.
2. Wait for detection to finish.
3. Open **Review instrument results** or a score's **Review** button.
4. Choose the score, tick the correct candidates, and click **Apply selected instruments**.

**Detection may take a while for large libraries**, especially scanned PDFs and images that require OCR. Try a filtered batch first. You can keep reading and cancel detection from the status bar.

Manual scans create proposals; they do not silently write assigned metadata. Existing protected choices require explicit replacement confirmation. For manual entry, use **Edit metadata → Instruments → + Add instrument**, then **Save metadata**.

Automatic detection for new GUI imports is optional and off by default. It fills only clear, empty, unprotected instrument values. Ambiguous labels such as bare “Alto 1” are not blindly treated as saxophone.

## Music recognition, MusicXML export, and MIDI playback (Linux)
This is an **experimental, review-first OMR workflow**, separate from instrument-label detection. Audiveris recognizes printed notation into MusicXML; RustyMusicSheets interprets supported notation and drives a separately installed FluidSynth process. No scores are uploaded, and source documents and library metadata remain unchanged.

### Setup
- Use **Browse…** beside the Audiveris, FluidSynth and SoundFont fields instead of typing filenames. MusicXML input and both export destination fields also have format-appropriate open/save dialogs. Library/import path controls provide file/folder selection. Text entry and paste remain available; Linux dialogs use optional Zenity.
- Install [Audiveris](https://github.com/Audiveris/audiveris/releases) separately using its official Linux package. Recent releases include Java. Configure its executable in **Settings → Music recognition and MIDI playback**, or **Reader → Music / OMR → Engine settings**. An absolute path such as `/opt/audiveris/bin/Audiveris` works; a Flatpak command with arguments is not an executable path—use a launcher script if necessary.
- Install FluidSynth and a SoundFont, for example `sudo apt install fluidsynth fluid-soundfont-gm`. Choose a `.sf2`/`.sf3` bank in settings; common system banks are detected automatically.
- Audiveris needs its own OCR language setup, even if the application's existing Tesseract label detector works. Install English through Audiveris's language management, or point `TESSDATA_PREFIX` at compatible full [Tesseract tessdata](https://github.com/tesseract-ocr/tessdata) models before launching the app. LSTM-only distro models may not provide the legacy data required by Audiveris.
- Save preferences. Missing helpers, unsupported targets, malformed notation, and audio-device failures are reported; optional helpers are not bundled or automatically installed.

### Reader workflow
1. Open a library score, then **Music / OMR → Recognize with Audiveris**. Recognition runs in the background and can be cancelled.
2. If several movements are exported, choose one and **Load movement**. Previously successful results are retained if another recognition attempt fails.
3. Review warnings and the exported notation. Part names and instrument guesses can be wrong: a cello line may be labelled “Voice.” Choose **Cello (GM 43)** explicitly when needed; Piano is GM 1. Numbers in the UI/CLI are one-based.
4. Tick the parts to hear and set each **Volume %** independently. A recognized piano grand staff stays one selectable part, with both staves and voices. Part choices, volume, instruments and tempo apply live and are remembered for that recognized movement.
5. Use **Play selected parts / Pause**, **Rewind 5s**, **Fast-forward 5s**, and **Stop audio**. Pause freezes position and silences active voices; resuming or seeking reconstructs notes held at the target position. Stop resets to the beginning. Seek while paused to stay paused, or seek before the first Play to prepare a starting position. Instrument envelopes restart on reconstructed notes; this is musical-position seeking, not sample-exact audio seeking.
6. Enter destination filenames and **Export MusicXML** or **Export selected MIDI**. Existing files are never overwritten. `.mxl` remains compressed MusicXML; use the matching extension. MusicXML preserves all recognized parts; MIDI reflects your selected parts, instruments, per-part channel volumes, and tempo.

### Performance accompaniment
The **Master volume %** slider below the transport controls adjusts all instruments together, from mute (0%) to normal output (100%), without changing their individual balances. It applies live and is remembered per recognized movement. The same slider appears in Music / OMR. This is a playback-output control; exported MIDI retains the individual part levels.
After saving engine settings, enter **Performance**. The bottom **Accompaniment** strip automatically loads saved recognition for the current score without opening settings. If no usable result exists, click **Recognize music** and use the visible progress/cancel controls. Multiple movements require an explicit **Load movement** choice.

The strip keeps **Rewind 5s**, **Play/Pause**, **Fast-forward 5s**, **Stop**, and position/duration available while the score remains visible. Open **Parts / volume** to choose individual parts, set independent 0–100% levels, or change their GM instruments and tempo. Unchecking a part mutes it without restarting the timeline. Live mixing supports up to 15 musical parts; MIDI percussion channel 10 is reserved. **Music settings / results** opens the full settings/review/export window without leaving performance mode.

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

Supported playback includes partwise MusicXML, chords/voices, multi-staff parts, rests, transposition, ties, tempo changes, basic repeats and first/second endings. Recognition and performance are **not guaranteed faithful**: expression, pedal, grace notes, advanced navigation/nested repeats and other unsupported constructs are warned about or approximated. GUI playback uses an application sequencer driving a persistent FluidSynth command process; the CLI retains one-shot MIDI playback. Note velocity is fixed, with independent part volume. A full notation editor, microphone/live-performance score tracking, looping, metronome, VST hosting, and Android/Windows OMR/audio are not implemented.

Audiveris is **AGPLv3**, FluidSynth is **LGPL**, and SoundFonts have their own licenses. They remain separately installed programs/data, not components embedded into or relicensed with this MIT application.

### Standalone music commands
These commands do not open or change the library database. Output directories/files are explicit:

```bash
rustymusicsheets --omr "/path/to/score.pdf" "/path/to/results" --audiveris /opt/audiveris/bin/Audiveris
rustymusicsheets --inspect-musicxml "/path/to/results/score.mxl"
rustymusicsheets --midi "/path/to/results/score.mxl" "/path/to/piano.mid" --parts P2
rustymusicsheets --midi "/path/to/results/score.mxl" "/path/to/cello.mid" --parts P1 --program P1=43 --play --play-seconds 5
```

Recognition prints the exact generated movement paths; use those in subsequent commands. Part IDs are read from MusicXML, not guessed from this example. Optional MIDI arguments include `--tempo-scale 0.8`, `--soundfont /absolute/bank.sf2`, and `--fluidsynth /absolute/fluidsynth`. CLI engines use their automatic defaults or explicit options, not GUI preferences. `--play-seconds` requires `--play`; without a duration playback continues until the MIDI finishes.

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

The app honors `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `XDG_CACHE_HOME`. For a complete backup, **close the app**, then copy the selected music folder **and** the entire app data and config directories. Automatic pre-rescan/schema-migration snapshots contain metadata only, not copies of your scores.

## Current limitations
Not implemented yet: annotations, crop/rotation/page reordering, in-score navigation links, vertical/half-page display, metronome, VST hosting, ChordPro/text, assembling image sequences, document printing/export, portable backup/restore, cloud integration, and multi-device sync. MusicXML/MIDI export and experimental Linux MIDI playback are available as described above.

Instrument OCR remains conservative label detection; musical transcription uses the separate optional Audiveris workflow. Faint scans, handwriting, uncommon abbreviations, and labels outside inspected regions may need manual entry. Audiveris targets printed notation, not handwriting. Physical pedals and touchscreens require device-specific testing. No compatibility is claimed for MobileSheets proprietary formats.

## Development checks
### Platform backends
File selection and keep-awake behavior use shared interfaces with separate platform backends:
- **Linux:** Zenity file/folder dialogs and the existing GNOME/KDE/freedesktop/logind power inhibitors.
- **Windows:** native `rfd` file/folder dialogs and worker-thread-scoped `SetThreadExecutionState` requests for display timeout and idle sleep. Windows screen savers and user-forced sleep are not blocked.
- **Other platforms:** explicit unsupported-status/error messages, with pasted paths and drag-and-drop remaining available for file selection.

Picker results use native paths, with cancellation separated from reported errors. Power guards release their requests when disabled, replaced/retried, cancelled, or shut down; Windows acquire/release operations stay on the same worker thread. Linux-only D-Bus dependencies are excluded from Windows builds.

**This refactor is not a Windows release:** the published download above is still Linux-only. A complete Windows package still needs its PDF/OCR helper tools, DLLs/language data, a native build toolchain, and Windows runtime testing. Module cross-compilation does not establish runtime compatibility.

### Android foundation
The source now includes a NativeActivity entry point and a document-access layer that distinguishes desktop paths from Android `content://` URIs. An initial Android **Open with** PNG/JPEG can be read through `ContentResolver`, staged privately, and passed to the existing importer without treating the URI as a filename.

This is an **experimental Android prerelease**, not desktop feature parity. Native PDF/OCR, the in-app Android picker, document-tree watching, and Android keep-awake integration remain incomplete. The ARM64 debug APK builds using `scripts/build-android.sh`; signature/manifest/16 KiB alignment checks pass with `scripts/verify-android-apk.py`. Startup was checked on a Pixel 8 Pro (API 37), and the user confirmed the safe-area layout works. Broader rendering, import/provider, and device testing remains pending. See [Android setup and implementation boundaries](android/README.txt) before attempting a build or installation.

### Validation commands
```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Tests generate their own score fixtures. PDF/OCR integration tests require Poppler, Tesseract, and English language data.

## Author and license
Created by **[Ripley Eldridge](https://github.com/ripleyeldridge)**, 2026.

Released under the [MIT License](LICENSE). Third-party dependencies retain their respective licenses.
