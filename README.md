# RustyMusicSheets
A native Linux sheet-music library and performance reader, written in Rust with egui.
**A headline feature: offline instrument detection that helps turn a folder of scores into an instrument-searchable library—even when the parts are scanned PDFs or standalone images.**

**Version:** 0.2.1 · **Author:** [Ripley Eldridge](https://github.com/ripleyeldridge) · **Release year:** 2026 · **License:** [MIT](LICENSE)

## Why this exists
> I vibe-coded the fuck out of this because I couldn't find a suitable Linux sheet-music reader for what I needed.
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
Download the current native package:

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
python3 scripts/build-deb.py --revision 2
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

- **27 clickable topics** cover setup, imports, folder watching, metadata, instruments/OCR, setlists, reading, pedals, settings, backups, CLI use, and troubleshooting.
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
Not implemented yet: annotations, crop/rotation/page reordering, in-score links/repeats, vertical/half-page display, audio/metronome, MIDI, ChordPro/text, assembling image sequences, printing/export, portable backup/restore, cloud integration, and multi-device sync.

OCR is conservative label detection, not musical-notation recognition. Faint scans, handwriting, uncommon abbreviations, and labels outside the inspected regions may need manual entry. Physical pedals and touchscreens require device-specific testing. No compatibility is claimed for MobileSheets proprietary formats.

## Development checks
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
