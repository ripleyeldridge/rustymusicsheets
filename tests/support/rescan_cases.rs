use super::*;
use rustymusicsheets::{folder, reconcile};

fn library() -> (tempfile::TempDir, Paths, std::path::PathBuf) {
    let (temp, paths) = setup();
    let root = temp.path().join("Music");
    fs::create_dir(&root).unwrap();
    let selected = folder::select(&paths, &root, &AtomicBool::new(false), |_| {}).unwrap();
    (temp, selected, root)
}

fn add_pdf(paths: &Paths, root: &Path, name: &str, pages: u32) -> Score {
    let path = root.join(name);
    make_pdf(&path, pages, name);
    let db = Library::open(paths).unwrap();
    import_file(&path, paths, &db, &AtomicBool::new(false)).unwrap();
    db.scores()
        .unwrap()
        .into_iter()
        .find(|score| score.file_path == Path::new(name))
        .unwrap()
}

#[test]
fn rescan_prunes_missing_scores_and_references_after_backing_up_metadata() {
    let (_temp, paths, root) = library();
    let a = add_pdf(&paths, &root, "First.pdf", 2);
    let b = add_pdf(&paths, &root, "Second.pdf", 3);
    let db = Library::open(&paths).unwrap();
    let list = db.create_group("Concert", "setlist").unwrap();
    for id in [a.id, b.id, a.id] {
        db.add_entry(list, id).unwrap();
    }
    let collection = db.create_group("Practice", "collection").unwrap();
    db.add_entry(collection, a.id).unwrap();
    db.toggle_bookmark(a.id, 1).unwrap();
    let kept = fs::read(root.join("Second.pdf")).unwrap();
    fs::remove_file(root.join("First.pdf")).unwrap();
    let report = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(db.scores().unwrap().len(), 1);
    assert_eq!(
        db.entries(list)
            .unwrap()
            .iter()
            .map(|e| e.score_id)
            .collect::<Vec<_>>(),
        [b.id]
    );
    assert!(db.entries(collection).unwrap().is_empty());
    assert!(db.bookmarks(a.id).unwrap().is_empty());
    assert_eq!(db.groups().unwrap().len(), 2);
    assert_eq!(fs::read(root.join("Second.pdf")).unwrap(), kept);
    let backup = report.backup.unwrap();
    assert!(backup.starts_with(paths.data.join("backups")));
    let snapshot = rusqlite::Connection::open(&backup).unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT COUNT(*) FROM scores", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        snapshot
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        snapshot
            .query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let again = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(again.removed, 0);
    assert!(again.backup.is_none());
}

#[test]
fn rescan_reconnects_renamed_pdfs_without_losing_metadata_or_memberships() {
    let (_temp, paths, root) = library();
    let mut original = add_pdf(&paths, &root, "Original.pdf", 3);
    let db = Library::open(&paths).unwrap();
    original.title = "Custom musical title".into();
    original.notes = "Keep my notes".into();
    db.save_score(&original).unwrap();
    db.remember_page(original.id, 2).unwrap();
    let list = db.create_group("Concert", "setlist").unwrap();
    db.add_entry(list, original.id).unwrap();
    db.toggle_bookmark(original.id, 1).unwrap();
    fs::rename(root.join("Original.pdf"), root.join("Renamed.PDF")).unwrap();
    let report = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.removed, 0);
    let current = db.scores().unwrap().remove(0);
    assert_eq!(current.id, original.id);
    assert_eq!(current.title, original.title);
    assert_eq!(current.notes, original.notes);
    assert_eq!(current.last_page, 2);
    assert_eq!(current.file_path, Path::new("Renamed.PDF"));
    assert_eq!(db.entries(list).unwrap()[0].score_id, original.id);
    assert_eq!(db.bookmarks(original.id).unwrap().len(), 1);
}

#[test]
fn missing_or_unreadable_library_roots_do_not_prune_any_records() {
    let (temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    let db = Library::open(&paths).unwrap();
    fs::rename(&root, temp.path().join("Disconnected")).unwrap();
    assert!(reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(db.scores().unwrap().len(), 1);
    fs::rename(temp.path().join("Disconnected"), &root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let permissions = fs::metadata(&root).unwrap().permissions();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o0)).unwrap();
    let inaccessible = fs::read_dir(&root).is_err();
    let result = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {});
    fs::set_permissions(&root, permissions).unwrap();
    if inaccessible {
        assert!(result.is_err());
    }
    assert_eq!(db.scores().unwrap().len(), 1);
}

#[test]
fn cancelled_rescan_and_cancelled_transaction_leave_missing_records_intact() {
    let (_temp, paths, root) = library();
    let score = add_pdf(&paths, &root, "First.pdf", 2);
    fs::remove_file(root.join("First.pdf")).unwrap();
    let cancelled = reconcile::rescan(&paths, &AtomicBool::new(true), |_| {}).unwrap();
    assert!(cancelled.cancelled);
    let mut db = Library::open(&paths).unwrap();
    assert_eq!(db.scores().unwrap().len(), 1);
    let cancel = AtomicBool::new(false);
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let result = db.prune_missing(&paths, &[score], &cancel, || {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        db.scores().unwrap().len(),
        1,
        "A partially executed cleanup must roll back"
    );
}

#[test]
fn restored_or_relocated_files_are_rechecked_before_deleting_records() {
    let (_temp, paths, root) = library();
    let score = add_pdf(&paths, &root, "First.pdf", 2);
    let mut db = Library::open(&paths).unwrap();
    let bytes = fs::read(root.join("First.pdf")).unwrap();
    fs::remove_file(root.join("First.pdf")).unwrap();
    fs::write(root.join("First.pdf"), &bytes).unwrap();
    let (removed, _) = db
        .prune_missing(
            &paths,
            std::slice::from_ref(&score),
            &AtomicBool::new(false),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(removed, 0);
    fs::rename(root.join("First.pdf"), root.join("Relocated.pdf")).unwrap();
    db.relocate(&score.hash, Path::new("Relocated.pdf"))
        .unwrap();
    let (removed, _) = db
        .prune_missing(&paths, &[score], &AtomicBool::new(false), || Ok(()))
        .unwrap();
    assert_eq!(removed, 0);
    assert_eq!(db.scores().unwrap().len(), 1);
}

#[test]
fn root_replaced_during_rescan_prevents_cleanup() {
    let (temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    fs::remove_file(root.join("First.pdf")).unwrap();
    let mut replaced = false;
    let result = reconcile::rescan(&paths, &AtomicBool::new(false), |report| {
        if report.current.starts_with("Reconciling") && !replaced {
            fs::rename(&root, temp.path().join("Old root")).unwrap();
            fs::create_dir(&root).unwrap();
            replaced = true;
        }
    });
    assert!(result.is_err());
    assert_eq!(Library::open(&paths).unwrap().scores().unwrap().len(), 1);
}

#[test]
fn stale_folder_configuration_aborts_before_importing_old_folder_files() {
    let (temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    make_pdf(
        &root.join("Unregistered.pdf"),
        2,
        "Do not import through stale paths",
    );
    let updated = folder::select(
        &paths,
        &temp.path().join("New music"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!(reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(Library::open(&updated).unwrap().scores().unwrap().len(), 1);
}

#[test]
fn directory_replacing_a_pdf_is_not_kept_as_a_score_and_is_never_deleted() {
    let (_temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    let path = root.join("First.pdf");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let report = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.removed, 1);
    assert!(path.is_dir());
}

#[test]
fn invalid_new_pdf_does_not_block_cleanup_of_confirmed_deleted_files() {
    let (_temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    fs::remove_file(root.join("First.pdf")).unwrap();
    fs::write(root.join("Broken.pdf"), b"not a PDF").unwrap();
    let report = reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(fs::read(root.join("Broken.pdf")).unwrap(), b"not a PDF");
}

#[test]
fn backup_failure_prevents_missing_score_cleanup() {
    let (_temp, paths, root) = library();
    add_pdf(&paths, &root, "First.pdf", 2);
    fs::remove_file(root.join("First.pdf")).unwrap();
    fs::write(paths.data.join("backups"), b"Not a directory").unwrap();
    assert!(reconcile::rescan(&paths, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(Library::open(&paths).unwrap().scores().unwrap().len(), 1);
}
