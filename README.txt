RustyMusicSheets 0.1
===================

A native Rust desktop sheet-music library and PDF performance reader for Linux.
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
The optional file/folder picker uses zenity. Paths can also be pasted into the
application, or files/folders can be dropped on its window.
Building requires a C compiler, pkg-config, and the X11/Wayland/xkbcommon
development libraries used by eframe. These were present on the development
machine. The UI uses OpenGL and supports X11 and Wayland.

Library folder management
-------------------------
Choosing a music folder:
1. Open the Library section and find "Music library folder".
2. Click "Choose…" to select a folder, or enter a path (including ~/ paths)
   and click "Set folder". A missing folder is created.
3. Wait for preparation to finish. The application scans existing PDFs and
   starts watching the selected folder and its subfolders automatically.

Before a folder is selected, manual imports use the default application-data
scores directory; automatic watching starts after you set a music folder.
The selected folder becomes the storage destination for future PDF imports.
Existing PDFs, including PDFs in subfolders, are registered
in place without renaming them or creating duplicate permanent copies.
PDFs imported from outside that folder are copied into it, preserving
readable names where possible and avoiding overwrites on filename collisions.

Automatic importing:

The selected folder is saved with the library metadata and watched while
the application is open. Background scans run every two seconds, wait for
files to remain unchanged for at least two seconds before importing, and
retry incomplete/invalid PDFs after 30 seconds. Content hashes prevent
duplicate scores when identical PDFs are added under different names.
The watcher always includes subfolders; the "Include subfolders" checkbox
controls manual folder imports, not automatic watching. Symbolic links are
not followed.

Watching resumes automatically on startup, including discovery of files
added while the application was closed. There is no background service when
the application is not running. "Rescan" restarts discovery.
A scan may take longer for large trees or slow storage.
Removing files externally does not delete score metadata.
An unavailable folder is reported in the Library section and checked again.
Import summaries and the "Import errors" menu report individual failures.

Changing folders safely:

Changing the library folder copies existing registered scores into the new
folder and preserves metadata, bookmarks, and setlists. Matching files already
there are reused. Existing destination files are never overwritten, and
old copies are retained for safety. The active folder changes only after
copying succeeds. Cancelled changes may leave complete copies in the
destination but do not switch the active folder.
If a registered PDF is missing and cannot be recovered from matching files
already in the destination, preparation fails rather than switching to an
incomplete library.

Storage and format support:
The chosen folder contains score PDFs. The metadata database and preferences
remain in their XDG application directories (see "Data and backups").
The folder choice and score-relative file locations are saved together in
the database, so setlists and metadata survive folder changes and restarts.
Only PDFs are supported for import and automatic discovery in this milestone;
image, audio, text, and ChordPro imports are not implemented yet.

Import and organize
-------------------
Use "Import folder…" to import an entire PDF folder. Subfolders are included
unless "Include subfolders" is unchecked. PDF extensions are case-insensitive.
Original PDFs are never changed by the importer.
Content hashes detect identical files, including copies with different names.
Different files sharing the same name remain separate scores.
Corrupt/encrypted PDFs are reported individually. Batch results show imported,
duplicate, and failed counts. Cancellation keeps completed imports and discards
the current unfinished import. Symbolic links are not followed.
The first 100 import errors are retained in the UI.

Search matches words across metadata. Sort by title, composer, or newest and
filter by genre or collection. Select a score, then "Edit metadata" to edit its
title, composer, artist, genre, key, tags, and notes.
PDF Author metadata initializes Composer; filenames initialize titles.

Expand "Create setlist / collection", enter a name, and create a group.
Enable "Show all scores to add to this group", then press + beside scores.
Setlists can repeat the same score. Expand "Setlist order / play" and use the
up/down buttons to reorder entries or x to remove an occurrence. Click an
entry in that panel to start playing the setlist at that position.
Clicking a score in the ordinary library list opens it individually instead.
Changes to a setlist do not change a currently playing sequence until you
start it again. Deleting a group preserves all scores and PDFs.

Read and perform
----------------
Single/two-page display, fit page/fit width, zoom, page jumping, bookmarks,
and remembered reading positions are available. Rendering happens in a
background worker with neighboring-page prefetch. GPU page textures use
an LRU cache limited to 12 pages / 128 MiB; worker queues are also bounded.
Render resolution is limited to 2560 pixels along the longest edge in the UI.

PageDown / ArrowRight: next page
PageUp / ArrowLeft: previous page
Home / End: first / last page of this score
F11: toggle fullscreen
Esc: leave performance/fullscreen

Click/tap the left or right half of a displayed page to turn backward/forward.
Performance hides the library and editing controls. The bottom bar retains an
exit button. Setlist navigation continues across score boundaries; reaching
the end displays an explicit message.

Settings allow alternative next/previous keys for keyboard-emulating USB or
Bluetooth pedals. Pair Bluetooth devices using Linux's device settings.
Physical pedals, touchscreens, and MIDI hardware need device-specific testing.

Command-line options
--------------------
    rustymusicsheets --import "/path/to/PDF folder"
    rustymusicsheets --list
    rustymusicsheets --check
    rustymusicsheets --smoke-test

CLI import is recursive, uses the same managed library, and returns a nonzero
exit status if any files fail. --smoke-test opens the UI and closes after
three seconds. Use XDG_DATA_HOME/XDG_CONFIG_HOME/XDG_CACHE_HOME to isolate
a test library.

Data and backups
----------------
Default directories:
    ~/.local/share/rustymusicsheets/   SQLite library and legacy/default PDF copies
    ~/.config/rustymusicsheets/       preferences.json
    ~/.cache/rustymusicsheets/        reserved for disposable render data
Pages are currently cached in memory, not on disk.
The program honors the corresponding XDG environment variables.

Your selected music folder stores the PDFs; the database and preferences
remain in the XDG locations above. The folder choice is saved atomically
with score locations in the SQLite database.

Until portable backup/restore is implemented, close the application and copy
the selected music folder AND the entire data and config directories.
Never copy only the SQLite main file while the app is running (WAL files
may be needed).
Do not manually rename managed PDFs.
Interrupted imports can leave unreferenced complete PDF files; importing
the same original again recovers them. A repeated import also repairs a
missing managed copy if its database record still exists.

Validation
----------
    cargo fmt --check
    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings
    cargo build --locked --release

Tests generate their own small PDFs and use the actual Poppler tools.
They do not import or modify your personal documents.

Desktop launcher
----------------
assets/ contains a freedesktop launcher template and an original SVG icon.
The launcher expects rustymusicsheets on PATH and the icon in the icon theme.
No system-wide installation or desktop registration is performed automatically.
run.sh can be launched directly from the project directory.

Not implemented in milestone 1
------------------------------
Annotations, crop/rotation/page reordering, links/repeats within scores,
vertical/half-page display, audio/metronome, MIDI, ChordPro/text/images,
printing/export, portable backups, cloud integration, and multi-device sync.
Those are later milestones, not hidden or simulated features in this release.
There is no compatibility claim for MobileSheets proprietary formats or
device protocols.
