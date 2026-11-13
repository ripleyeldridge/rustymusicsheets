RustyMusicSheets 0.2.1
===================

A native Rust desktop sheet-music library and PDF/image performance reader for Linux.
This is milestone 1 of a larger functional replacement for MobileSheets, NOT
full feature parity yet.

Build and run
-------------
Rust 1.86 or newer and Cargo are required. Dependencies are locked for the
currently installed compiler. No compiler upgrade is needed for this version.

    cargo build --locked --release
    ./target/release/rustymusicsheets

From any directory:

    ~/Projects/LinuxSheets/run.sh

run.sh builds a release binary if none exists. Rebuild explicitly after source
changes; the wrapper does not check whether an existing binary is stale.

PDF viewing requires pdfinfo and pdftoppm (Ubuntu package: poppler-utils).
Instrument detection also uses pdftotext from poppler-utils and Tesseract
(Ubuntu packages: tesseract-ocr and tesseract-ocr-eng for English language data).
Additional OCR languages require their corresponding Tesseract language packages.
All text extraction, OCR, and classification run locally; scores are not uploaded.
The optional file/folder picker uses zenity. Paths can also be pasted into the
application, or files/folders can be dropped on its window.
Building requires a C compiler, pkg-config, and the X11/Wayland/xkbcommon
development libraries used by eframe. These were present on the development
machine. The UI uses OpenGL and supports X11 and Wayland.
Help and About
--------------
Open Help > Help contents, or press F1, for the built-in offline user guide.
The clickable topic tree contains 27 subjects grouped into getting started,
library/files, browsing/metadata, instruments/OCR, setlists, reading/performance,
settings/data, troubleshooting/reference, and About.
Search help matches every entered word across titles, keywords, and article
text, case-insensitively. Click a result to read it; Clear restores all topics.
Topics and articles scroll independently in wide windows. Compact windows
offer Topics/Article buttons so both remain usable at 640x360.
Close help or Escape returns to the previous workspace without changing
reading position, list playback, filters, or metadata drafts. Page-turn keys
are suspended while help is open; background library jobs can continue.
Help > Keyboard shortcuts opens the key/pedal reference directly.
Help > About RustyMusicSheets shows the compiled application version,
author credit RustyMusicSheets contributors, release year 2026, and MIT license.
The guide is embedded in the executable and also shipped as help.txt in the
package documentation directory; no network connection is required.

Library folder management
-------------------------
Choosing a music folder:
1. Open the Library tab, enable Options, and find "Music library folder".
2. Click "Choose…" to select a folder, or enter a path (including ~/ paths)
   and click "Set folder". A missing folder is created.
3. Wait for preparation to finish. The application scans existing PDF/PNG/JPEG scores and
   starts watching the selected folder and its subfolders automatically.

Before a folder is selected, manual imports use the default application-data
scores directory; automatic watching starts after you set a music folder.
The selected folder becomes the storage destination for future score imports.
Existing supported scores, including files in subfolders, are registered
in place without renaming them or creating duplicate permanent copies.
Scores imported from outside that folder are copied into it, preserving
readable names where possible and avoiding overwrites on filename collisions.

Automatic importing:

The selected folder is saved with the library metadata and watched while
the application is open. Background scans run every two seconds, wait for
files to remain unchanged for at least two seconds before importing, and
retry incomplete/invalid scores after 30 seconds. Content hashes prevent
duplicate scores when identical files are added under different names.
The watcher always includes subfolders; the "Include subfolders" checkbox
controls manual folder imports, not automatic watching. Symbolic links are
not followed.

Watching resumes automatically on startup, including discovery of files
added while the application was closed. There is no background service when
the application is not running. Automatic watching never removes score records.
A scan may take longer for large trees or slow storage.
Use explicit Rescan to reconcile files deleted outside the application.
An unavailable folder is reported in the Library section and checked again.
If the saved library path or picker starting directory is missing, the
library path field and file/folder chooser start at your home directory.
Stored score locations and the configured library are not silently changed:
choose a replacement folder explicitly, or reconnect the existing library.
Import summaries and the "Import errors" menu report individual failures.

Explicit Rescan and missing files:
Click Rescan in Library options to scan/import recursively, reconnect renamed
identical scores, and remove records whose backing file no longer exists.
Missing-score cleanup also removes those scores' bookmarks and occurrences
in setlists/collections. Groups themselves remain, including empty ones.
Rescan never deletes existing score files or directories.

Before cleanup, a complete SQLite metadata backup is saved under:
    ~/.local/share/rustymusicsheets/backups/before-rescan-*.sqlite3
The actual directory follows XDG_DATA_HOME. The status bar reports the
removed count and provides a Metadata backup menu with the exact path.
These are metadata-only backups, not copies of the scores or a portable restore
feature. Keep external backups of your music folder as described below.

A missing/unreadable root, filesystem access errors, cancellation, a folder
changing during the operation, or a backup failure prevents record cleanup.
An invalid new score is reported but does not cause other valid files to be
deleted. If all backing scores were deleted from a readable folder, its missing
records can all be removed. New imports completed before cancellation are kept.
An open removed score returns to Library; playback snapshots discard missing
entries. Save or cancel dirty metadata drafts before starting a rescan.
Metadata editing is temporarily disabled while reconciliation is running.

Changing folders safely:

Changing the library folder copies existing registered scores into the new
folder and preserves metadata, bookmarks, and setlists. Matching files already
there are reused. Existing destination files are never overwritten, and
old copies are retained for safety. The active folder changes only after
copying succeeds. Cancelled changes may leave complete copies in the
destination but do not switch the active folder.
If a registered score is missing and cannot be recovered from matching files
already in the destination, preparation fails rather than switching to an
incomplete library.

Storage and format support:
The chosen folder contains score files. The metadata database and preferences
remain in their XDG application directories (see "Data and backups").
The folder choice and score-relative file locations are saved together in
the database, so setlists and metadata survive folder changes and restarts.
PDF, PNG, JPG, and JPEG extensions are supported, case-insensitively.
Each standalone image is a separate single-page score; combining a sequence of
images into one score is not implemented. Transparent images display on white.
Malformed images and images exceeding 40 megapixels or the decoder's dimension/
allocation limits are rejected. Audio, text, and ChordPro imports are not implemented.

Import and organize
-------------------
The Library tab shows the score list in the large main workspace on the
right, not in the lower-left sidebar. Search, list/collection filters,
sorting, and genre filters are above the larger score rows.
The default left panel contains scrollable folder/import/display/list options.
Settings and metadata editors reuse that same panel instead of opening windows.
Click Options in the toolbar to hide that panel and give the browser the
full width. The main window can be resized down to 640 by 360 logical pixels.

Select a score to open the separate Reader tab at its remembered page.
Reader uses the full workspace unless a left-side editor is open. Return to
Library to continue browsing; search, filters, and scroll state are retained.
The Reader button returns to the current score without reloading its position.
Page-turn shortcuts apply only in Reader, not while browsing the Library.
When a list is selected, "+ Add" appends a score without leaving Library.
Use "Import folder…" to import an entire score folder. Subfolders are included
unless "Include subfolders" is unchecked.
Original score files are never changed by the importer.
Content hashes detect identical files, including copies with different names.
Different files sharing the same name remain separate scores.
Corrupt/encrypted PDFs are reported individually. Batch results show imported,
duplicate, and failed counts. Cancellation keeps completed imports and discards
the current unfinished import. Symbolic links are not followed.
The first 100 import errors are retained in the UI.

Search matches words across metadata. Sort by title, composer, or newest and
filter by genre, collection, or instrument. Select a score, then "Edit metadata"
to edit its title, composer, artist, genre, key, tags, notes, and instruments.
PDF Author metadata initializes Composer; filenames initialize titles.

Search supports physical keyboards, paste, and Linux on-screen keyboards
that send committed text without intermediate composition events.
Search fields keep a stable identity as results change, and page-turn
shortcuts stay inactive while a text field is being edited.
Instrument detection and review
-------------------------------
In Library options, expand "Instrument detection" and detect the current score,
filtered scores, or the entire library. These manual runs create proposals;
they do not silently assign instruments. Open "Review instrument results", or
use a score row's Review button, to inspect evidence, page, extraction method,
and OCR confidence. Select proposals and explicitly apply them.
Instrument detection can take a while for large libraries, especially scanned
PDFs and images that need OCR. A warning appears beside detection controls and
before CLI detection. Try a filtered batch first; keep reading or cancel from
the status bar while the task runs.

Detection first reads embedded PDF text. If a page has no clear instrument
labels, it renders that page and runs Tesseract on the top heading area and
left staff-label area. PNG/JPEG scores use the same OCR regions directly.
Settings controls the number of pages (default 2, maximum 6) and language
(default eng; combinations such as eng+deu require both language packages).
Only the selected opening pages and regions are inspected; this is not full
score transcription, musical-notation recognition, or a general OCR editor.
Uniform, text-free regions are skipped. Background progress, cancellation,
and detection errors appear in the status bar; the reader stays available.

Aliases such as Sax/Saxophone and Violoncello/Cello are normalized to a shared
instrument name. Subtype and part number are separate fields, so Alto Saxophone
1 remains Saxophone / Alto / 1. Several instruments can belong to one score.
Bare Alto, Tenor, or Bass are ambiguous and are NOT guessed as saxophone.
Title/prose mentions and weak OCR require review. Detection is a conservative
rule-based aid, not a guarantee; faint scans, handwritten labels, unfamiliar
abbreviations, and labels outside scanned regions may be missed.

The Instrument filter offers all instruments, unassigned scores, and individual
assigned instrument names. Search also matches assigned names, subtypes, and
parts. Proposals do not affect these filters until applied.
Edit metadata can add/remove instruments manually, including custom names.
Manual edits and confirmed proposals are protected from automatic replacement;
an intentionally empty manual value is protected too. Replacing protected
values in review requires explicit confirmation. The manual-protection option
in the metadata editor can be cleared if automatic filling is wanted again.

"Detect new imports; fill only clear, unassigned instruments" is optional and
off by default. When enabled, new GUI imports and watched arrivals are queued;
clear results fill only empty, unprotected instrument fields. Existing library
entries are not scanned merely by enabling it. Detection waits until metadata
editing and other library jobs finish, and does not switch the workspace.
CLI import does not schedule automatic detection; use --detect-instruments.
Save or cancel dirty metadata drafts before detection or applying proposals.
Changed files invalidate old proposals; rescan and detect them again.
Manual metadata is retained if file contents are replaced, while automatic
instrument assignments are cleared. Detection never modifies the score file.

Manage setlists and collections
-------------------------------
Click "Setlists" in the top toolbar to open the manager, or choose a group
in Library and click "Manage list…".

- Use "Existing list" to select a saved setlist or collection.
- Click "New list…", enter a name, and click "Create list". Check Collection
  if you want an unordered unique-score collection instead of a setlist.
- Edit the Name field and click "Rename" to save an existing list's new name.
- "Duplicate" makes an independent copy with a unique copy name. It copies
  membership and order, including repeated entries, but not the PDF files.
- The left column shows the current order. "Up"/"Down" move an occurrence;
  "Remove" removes only that occurrence, never its score or PDF.
- Search the full library in the right column and use "+ Add" to append a
  score. Setlists allow repeats; collections keep one copy of each score.
- "Delete list…" requires confirmation. Cancelling keeps the list intact;
  confirming deletes only the group and its membership records.
- "Play setlist" starts at the beginning; "Play here" starts at that entry.

Membership/order changes save immediately. Naming changes use Rename.
Edits do not alter an already-playing snapshot: start playback again to use
the new order. Clicking a score in the ordinary library list still opens it
individually. List management and Play actions remain available in Library options.
In narrow windows the manager stacks its panels vertically. Scroll the
manager to reach panels and actions that do not fit at once.

Readable display names
----------------------
Expand "Display names" in Library options to customize how score titles
appear. Both options are off by default:
- "Replace characters with spaces": each character in the Characters field
  becomes a space. The default character is underscore (_). Enter _- to also
  replace hyphens, or supply other characters as needed.
- "Remove IMSLP/PMLP download prefixes": recognize leading IMSLP/PMLP numeric
  identifiers, including common hyphen/underscore separators and bracketed
  prefixes. Arbitrary numbers elsewhere in a title are not removed.

For example, with both options enabled:
    IMSLP237087-PMLP384177-Bach_Prelude_No.1
becomes:
    Bach Prelude No.1

Repeated whitespace is collapsed when cleanup is enabled. If cleanup would
produce an empty name, the original title is shown instead.
The preview and library list update immediately. Click "Save display options"
to retain the settings after restart. Options apply to existing and future
scores, setlist/collection entries, and the reader heading. Title sorting
uses displayed names; search matches both the original and cleaned title.
Hover over a library score to see its original title and stored filename.

This is reversible presentation only: PDFs are not renamed, saved metadata
is not rewritten, and distinct scores remain separate even if their cleaned
names happen to match. Metadata editing and CLI --list retain original titles.

Settings and metadata in the left panel
---------------------------------------
Settings replaces Library options in the left panel and also works alongside
Reader. Close settings to restore the previous layout. Save preferences stays
at the top while the settings body scrolls.

Edit metadata opens the current score's editor in that same left panel.
Save metadata and Cancel remain at the top while fields scroll. Save writes
the edited metadata; Cancel discards the draft. Back or switching to Settings
or Options keeps the unfinished draft, which Edit metadata can reopen.
Finish or cancel that draft before editing a different score or rescanning.
Closing an editor restores Library options if they were enabled, or restores
the full-width Reader when reading. Library files are never renamed by edits.

Keep awake while running
------------------------
Open Settings and enable "Prevent screen dimming/timeout and suspend while
running" under Power management. This option is off by default, takes
effect immediately, and is saved automatically for the next launch.

The application requests temporary screen-idle/dimming and suspend inhibitors
through GNOME or KDE session services, with freedesktop screen/power and
systemd-logind fallbacks. Requests run in a background worker, not the UI.
Inhibitors remain active while the application is running, including when
minimized, and are released when you disable the option or close the app.
No permanent desktop power settings are changed. A crash or process exit
also releases the underlying session connection and file-descriptor locks.

The status bar shows "Keep awake" when both requests were accepted. A yellow
status means protection is pending or incomplete; Settings shows details and
a retry button. Desktop policy, deliberate/forced suspend, lid-close handling,
and critical-battery safety actions may override application inhibitors.
This is not a way to bypass system security or mandatory power policies.

Read and perform
----------------
Reader controls have a bounded scrolling area so they leave room for the
score in short windows. Settings/metadata panels scroll within the sidebar;
error dialogs are constrained to the current window and scroll when needed.
Single/two-page display, fit page/fit width, zoom, page jumping, bookmarks,
and remembered reading positions are available. Rendering happens in a
background worker with neighboring-page prefetch. GPU page textures use
an LRU cache limited to 12 pages / 128 MiB; worker queues are also bounded.
Render resolution is limited to 2560 pixels along the longest edge in the UI.

PageDown / ArrowRight: next page
PageUp / ArrowLeft: previous page
Home / End: first / last page of this score
F1: open help
F11 (outside help): toggle fullscreen
Esc: close help, otherwise leave performance/fullscreen

Click/tap the left or right half of a displayed page to turn backward/forward.
Performance hides the library and editing controls. The bottom bar retains an
exit button. Setlist navigation continues across score boundaries; reaching
the end displays an explicit message.

Settings allow alternative next/previous keys for keyboard-emulating USB or
Bluetooth pedals. Pair Bluetooth devices using Linux's device settings.
Physical pedals, touchscreens, and MIDI hardware need device-specific testing.

Command-line options
--------------------
    rustymusicsheets --import "/path/to/score folder"
    rustymusicsheets --list
    rustymusicsheets --detect-instruments
    rustymusicsheets --detect-instruments 12
    rustymusicsheets --check
    rustymusicsheets --smoke-test

CLI import is recursive, uses the same managed library, and returns a nonzero
exit status if any files fail. --smoke-test opens the UI and closes after
three seconds. Use XDG_DATA_HOME/XDG_CONFIG_HOME/XDG_CACHE_HOME to isolate
a test library.
--detect-instruments creates proposals for all scores or the specified ID;
review/apply them in the GUI. --check verifies PDF and OCR helper executables.

Data and backups
----------------
Default directories:
    ~/.local/share/rustymusicsheets/   SQLite library and legacy/default score copies
    ~/.config/rustymusicsheets/       preferences.json
    ~/.cache/rustymusicsheets/        reserved for disposable render data
Pages are currently cached in memory, not on disk.
The program honors the corresponding XDG environment variables.

Your selected music folder stores the scores; the database and preferences
remain in the XDG locations above. The folder choice is saved atomically
with score locations in the SQLite database.

Until portable backup/restore is implemented, close the application and copy
the selected music folder AND the entire data and config directories.
Never copy only the SQLite main file while the app is running (WAL files
may be needed).
Before adding instrument tables, existing databases are backed up under
data/backups/before-instruments-*.sqlite3. Migration preserves existing metadata,
bookmarks, and list membership. This is a metadata backup, not a copy of scores.
Do not manually rename managed score files.
Interrupted imports can leave unreferenced complete score files; importing
the same original again recovers them. A repeated import also repairs a
missing managed copy if its database record still exists.

Validation
----------
    cargo fmt --check
    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings
    cargo build --locked --release

Tests generate their own small PDFs/images and use actual Poppler and Tesseract
with English language data, including OCR on scanned PDFs, PNG, and JPEG.
They do not import or modify your personal documents.

Desktop launcher
----------------
assets/ contains the freedesktop launcher and an original copper musical-note
icon. PNG sizes from 16 to 512 pixels and the scalable SVG are included.
The 256-pixel icon is also embedded in the executable, so X11 window icons
work when launching directly through run.sh.

The Wayland application ID, desktop filename, icon name, and X11 startup
window class use rustymusicsheets. Installing the Debian package registers
the icon for the application-menu entry, shortcuts, dock/taskbar, pager,
and task switcher. Title-bar icon visibility depends on your desktop/window
manager; some themes deliberately do not display title-bar icons.
The package does not force a desktop shortcut or change your pinned apps:
pin RustyMusicSheets from your application menu/taskbar if desired.

Regenerate PNG artwork from the SVG using scripts/render-icons.sh (requires
Inkscape). Rebuild the executable after changing the embedded icon.

Debian / Ubuntu package
-----------------------
Build a native package without installing it or requiring root:
    python3 scripts/build-deb.py

This requires Python 3.9+, Cargo/Rust, dpkg-deb, dpkg-shlibdeps (dpkg-dev),
and binutils. The result is written to:
    dist/rustymusicsheets_0.2.1-1_<architecture>.deb

To package an already-built release executable:
    python3 scripts/build-deb.py --skip-build

The builder installs the binary under /usr/bin, launcher under
/usr/share/applications, and icon sizes under /usr/share/icons/hicolor.
It includes documentation and dependency license notices, and declares
Poppler, Tesseract with English data, graphics/input libraries, and the actual
glibc/libgcc requirements.
Zenity is recommended for file/folder dialogs.

IMPORTANT: A native .deb supports the build machine's CPU architecture and
minimum runtime libraries, not every Ubuntu version. Building on a newer
Ubuntu release can produce a binary that will not run on older releases.
Build on the oldest release you intend to support, or use a matching isolated
build environment. Check the reader's Ubuntu version and architecture before
installing. The current development machine is amd64 and its binary requires
glibc 2.43; that particular build is not compatible with Ubuntu 22.04/24.04.
Do not force-install it or replace the reader's glibc to bypass dependencies.

After confirming compatibility, install the package with APT so dependencies
are resolved:
    sudo apt install ./dist/rustymusicsheets_0.2.1-1_amd64.deb

Replace the filename with the package matching your reader's architecture.
Personal music-library data and preferences are not shipped in the package.
Uninstalling the application does not delete your library.

Not implemented in milestone 1
------------------------------
Annotations, crop/rotation/page reordering, links/repeats within scores,
vertical/half-page display, audio/metronome, MIDI, ChordPro/text, image-sequence assembly,
printing/export, portable backups, cloud integration, and multi-device sync.
Those are later milestones, not hidden or simulated features in this release.
There is no compatibility claim for MobileSheets proprietary formats or
device protocols.
