use super::*;
use rustymusicsheets::{
    folder,
    watch::{FolderWatcher, Scanner},
};

#[test]
fn command_line_import_uses_the_persisted_music_folder_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: root.path().join("xdg-data/rustymusicsheets"),
        config: root.path().join("xdg-config/rustymusicsheets"),
        cache: root.path().join("xdg-cache/rustymusicsheets"),
        music: None,
    };
    paths.create().unwrap();
    let selected = root.path().join("Selected music");
    folder::select(&paths, &selected, &AtomicBool::new(false), |_| {}).unwrap();
    let source = root.path().join("Outside.pdf");
    make_pdf(&source, 2, "CLI persistence");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_rustymusicsheets"))
        .arg("--import")
        .arg(&source)
        .env("XDG_DATA_HOME", root.path().join("xdg-data"))
        .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
        .env("XDG_CACHE_HOME", root.path().join("xdg-cache"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(selected.join("Outside.pdf").exists());
    assert_eq!(Library::open(&paths).unwrap().scores().unwrap().len(), 1);
}

#[test]
fn chosen_folder_registers_existing_pdfs_in_place_and_stores_outside_imports() {
    let (temp, paths) = setup();
    let music = temp.path().join("My music");
    fs::create_dir_all(music.join("Bach")).unwrap();
    let existing = music.join("Bach/Étude.PDF");
    make_pdf(&existing, 2, "Existing");
    let cancel = AtomicBool::new(false);
    let selected = folder::select(&paths, &music, &cancel, |_| {}).unwrap();
    let report = batch(
        std::slice::from_ref(&music),
        true,
        &selected,
        &cancel,
        |_| {},
    )
    .unwrap();
    assert_eq!(report.imported, 1);
    let db = Library::open(&selected).unwrap();
    let first = db.scores().unwrap().remove(0);
    assert_eq!(db.pdf_path(&selected, &first.hash).unwrap(), existing);
    assert_eq!(first.file_path, Path::new("Bach/Étude.PDF"));
    assert_eq!(fs::read_dir(&music).unwrap().count(), 1);
    let outside = temp.path().join("Outside.pdf");
    make_pdf(&outside, 3, "Outside");
    import_file(&outside, &selected, &db, &cancel).unwrap();
    assert!(music.join("Outside.pdf").exists());
    assert!(outside.exists());
    assert_eq!(db.music_folder().unwrap(), Some(music));
    assert_eq!(db.scores().unwrap().len(), 2);
}

#[test]
fn changing_library_folder_preserves_metadata_and_reuses_matching_files() {
    let (temp, paths) = setup();
    let cancel = AtomicBool::new(false);
    let original = temp.path().join("Source.pdf");
    make_pdf(&original, 3, "Move this library");
    let db = Library::open(&paths).unwrap();
    import_file(&original, &paths, &db, &cancel).unwrap();
    let mut score = db.scores().unwrap().remove(0);
    score.title = "Custom title".into();
    db.save_score(&score).unwrap();
    db.remember_page(score.id, 2).unwrap();
    let setlist = db.create_group("Concert", "setlist").unwrap();
    db.add_entry(setlist, score.id).unwrap();
    let old_file = db.pdf_path(&paths, &score.hash).unwrap();
    let destination = temp.path().join("New music");
    fs::create_dir(&destination).unwrap();
    fs::copy(&original, destination.join("Already here.pdf")).unwrap();
    let updated = folder::select(&paths, &destination, &cancel, |_| {}).unwrap();
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        1,
        "No duplicate copy"
    );
    assert!(old_file.exists(), "Keep the old library copy");
    assert_eq!(
        db.pdf_path(&updated, &score.hash).unwrap(),
        destination.join("Already here.pdf")
    );
    assert_eq!(db.scores().unwrap()[0].title, "Custom title");
    assert_eq!(db.scores().unwrap()[0].last_page, 2);
    assert_eq!(db.entries(setlist).unwrap()[0].score_id, score.id);
    drop(db);
    assert_eq!(
        Library::open(&updated).unwrap().music_folder().unwrap(),
        Some(destination)
    );
}

#[test]
fn schema_one_migrates_and_folder_change_never_overwrites_files() {
    let (temp, paths) = setup();
    let cancel = AtomicBool::new(false);
    let source = temp.path().join("Legacy.pdf");
    make_pdf(&source, 1, "Legacy");
    let db = Library::open(&paths).unwrap();
    import_file(&source, &paths, &db, &cancel).unwrap();
    let hash = db.scores().unwrap()[0].hash.clone();
    drop(db);
    let conn = rusqlite::Connection::open(paths.data.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "DROP TABLE score_instruments; DROP TABLE instrument_detections;
         ALTER TABLE scores DROP COLUMN instruments_manual;
         ALTER TABLE scores DROP COLUMN file_path;
                        DROP TABLE settings; PRAGMA user_version=1;",
    )
    .unwrap();
    drop(conn);
    let db = Library::open(&paths).unwrap();
    assert!(db.scores().unwrap()[0].file_path.as_os_str().is_empty());
    let destination = temp.path().join("Collision");
    fs::create_dir(&destination).unwrap();
    let occupied = destination.join(format!("{hash}.pdf"));
    make_pdf(&occupied, 2, "Unrelated user file");
    let before = fs::read(&occupied).unwrap();
    let selected = folder::select(&paths, &destination, &cancel, |_| {}).unwrap();
    assert_eq!(fs::read(&occupied).unwrap(), before);
    let location = db.pdf_path(&selected, &hash).unwrap();
    assert_ne!(location, occupied);
    assert_eq!(fs::read(location).unwrap(), fs::read(source).unwrap());
}

#[test]
fn failed_or_cancelled_folder_changes_keep_the_active_library() {
    let (temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let id = db.insert("missing", "Missing score", 1, "").unwrap();
    let destination = temp.path().join("Cannot complete");
    assert!(folder::select(&paths, &destination, &AtomicBool::new(false), |_| {}).is_err());
    assert!(db.music_folder().unwrap().is_none());
    assert_eq!(db.scores().unwrap()[0].id, id);
    assert!(folder::select(&paths, &destination, &AtomicBool::new(true), |_| {}).is_err());
    assert!(db.music_folder().unwrap().is_none());
}

#[test]
fn scanner_waits_for_stable_files_imports_new_arrivals_and_retries_failures() {
    let (temp, paths) = setup();
    let root = temp.path().join("Music");
    let cancel = AtomicBool::new(false);
    let selected = folder::select(&paths, &root, &cancel, |_| {}).unwrap();
    make_pdf(&root.join("First.pdf"), 1, "First");
    let mut scanner = Scanner::default();
    let now = Instant::now();
    let settle = Duration::from_secs(2);
    let scan = |scanner: &mut Scanner, seconds| {
        scanner
            .scan(
                &selected,
                &cancel,
                now + Duration::from_secs(seconds),
                settle,
            )
            .unwrap()
    };
    assert_eq!(scan(&mut scanner, 0).imported, 0);
    assert_eq!(scan(&mut scanner, 3).imported, 1);
    assert_eq!(
        scan(&mut scanner, 4).skipped,
        0,
        "Unchanged files are not rehashed"
    );
    fs::create_dir(root.join("Nested")).unwrap();
    let next = root.join("Nested/Second.PDF");
    fs::write(&next, b"partial").unwrap();
    assert_eq!(scan(&mut scanner, 5).failed, 0);
    make_pdf(&next, 2, "Finished copying");
    assert_eq!(scan(&mut scanner, 6).imported, 0);
    assert_eq!(scan(&mut scanner, 9).imported, 1);
    let broken = root.join("Broken.pdf");
    fs::write(&broken, b"broken").unwrap();
    scan(&mut scanner, 10);
    assert_eq!(scan(&mut scanner, 13).failed, 1);
    assert_eq!(scan(&mut scanner, 14).failed, 0, "Back off invalid files");
    make_pdf(&broken, 3, "Fixed");
    scan(&mut scanner, 15);
    assert_eq!(scan(&mut scanner, 18).imported, 1);
    let db = Library::open(&selected).unwrap();
    assert_eq!(db.scores().unwrap().len(), 3);
    let mut restarted = Scanner::default();
    scan(&mut restarted, 20);
    assert_eq!(scan(&mut restarted, 23).skipped, 3);
    let cancelled = scanner
        .scan(
            &selected,
            &AtomicBool::new(true),
            now + Duration::from_secs(24),
            settle,
        )
        .unwrap();
    assert!(cancelled.cancelled);
    fs::remove_file(root.join("First.pdf")).unwrap();
    scan(&mut scanner, 25);
    assert_eq!(
        db.scores().unwrap().len(),
        3,
        "External deletion never deletes metadata"
    );
}

#[test]
fn live_folder_watcher_imports_initial_and_later_files_without_user_actions() {
    let (temp, paths) = setup();
    let root = temp.path().join("Watched");
    let selected = folder::select(&paths, &root, &AtomicBool::new(false), |_| {}).unwrap();
    make_pdf(&root.join("Initial.pdf"), 1, "Initial");
    let watcher = FolderWatcher::start(selected.clone(), || {});
    let db = Library::open(&selected).unwrap();
    let wait_for = |count| {
        let deadline = Instant::now() + Duration::from_secs(12);
        while db.scores().unwrap().len() != count {
            assert!(Instant::now() < deadline, "Watcher did not import file");
            watcher
                .updates
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
        }
    };
    wait_for(1);
    make_pdf(&root.join("Later.PDF"), 2, "Later");
    wait_for(2);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
    drop(watcher);
}
