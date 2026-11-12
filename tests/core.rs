use rustymusicsheets::{
    import::{batch, import_file, Outcome},
    model::{Paths, Preferences, Score},
    pdf,
    reader::{Reader, Turn},
    render::{cache_over_limit, PageKey, PageRenderer, CACHE_BYTES, CACHE_PAGES},
    storage::Library,
};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[path = "support/folder_cases.rs"]
mod folder_cases;

fn setup() -> (tempfile::TempDir, Paths) {
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: temp.path().join("data"),
        cache: temp.path().join("cache"),
        config: temp.path().join("config"),
        music: None,
    };
    paths.create().unwrap();
    (temp, paths)
}

/// Build a real, self-contained PDF fixture with valid xref offsets.
fn make_pdf(path: &Path, pages: u32, title: &str) {
    let page_ids: Vec<_> = (0..pages).map(|n| 3 + n * 2).collect();
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R "))
        .collect::<String>();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
    ];
    for (index, id) in page_ids.iter().enumerate() {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> \
             /Contents {} 0 R >>",
            id + 1
        ));
        let stream = format!(
            "BT /F1 22 Tf 60 700 Td ({title} - page {}) Tj ET",
            index + 1
        );
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }
    let mut data = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(data.len());
        data.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = data.len();
    data.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        data.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    data.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    fs::write(path, data).unwrap();
}

#[test]
fn metadata_parser_rejects_encryption_and_bad_page_counts() {
    let info = pdf::parse_info("Pages: 12\nAuthor: Bach\nEncrypted: no\n").unwrap();
    assert_eq!(info.pages, 12);
    assert_eq!(info.author, "Bach");
    for output in [
        "Pages: 0",
        "Pages: -1",
        "Pages: garbage",
        "No fields",
        "Pages: 1\nEncrypted: yes (print:yes)",
    ] {
        assert!(pdf::parse_info(output).is_err());
    }
}

#[test]
fn real_pdf_metadata_and_rendering() {
    let (temp, _) = setup();
    let path = temp.path().join("score with spaces.pdf");
    make_pdf(&path, 3, "RustyMusicSheets");
    let cancel = AtomicBool::new(false);
    assert_eq!(pdf::info(&path, &cancel).unwrap().pages, 3);
    let image = pdf::render(&path, 1, 600, &cancel).unwrap();
    assert_eq!(image.height(), 600);
    assert!(image.width() > 400);
    assert!(image.pixels().any(|p| p[0] < 100));
    assert!(pdf::render(&path, 99, 600, &cancel).is_err());
}

#[test]
fn import_deduplicates_content_not_names_and_preserves_originals() {
    let (temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    let first = a.join("Étude score.PDF");
    let second = b.join("Étude score.PDF");
    make_pdf(&first, 2, "One");
    make_pdf(&second, 3, "Two");
    let original = fs::read(&first).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        import_file(&first, &paths, &db, &cancel).unwrap(),
        Outcome::Imported
    );
    assert_eq!(
        import_file(&first, &paths, &db, &cancel).unwrap(),
        Outcome::Duplicate
    );
    assert_eq!(
        import_file(&second, &paths, &db, &cancel).unwrap(),
        Outcome::Imported
    );
    assert_eq!(db.scores().unwrap().len(), 2);
    assert_eq!(fs::read(&first).unwrap(), original);
    for score in db.scores().unwrap() {
        assert!(paths.pdf(&score.hash).exists());
        assert_eq!(score.title, "Étude score");
    }
}

#[test]
fn folder_import_handles_recursion_duplicates_bad_files_and_symlinks() {
    let (temp, paths) = setup();
    let source = temp.path().join("input");
    fs::create_dir_all(source.join("sub")).unwrap();
    make_pdf(&source.join("top.PdF"), 1, "Top");
    make_pdf(&source.join("sub/nested.pdf"), 2, "Nested");
    fs::write(source.join("bad.pdf"), b"not a pdf").unwrap();
    fs::write(source.join("readme.txt"), b"ignore").unwrap();
    std::os::unix::fs::symlink(&source, source.join("sub/loop")).unwrap();
    std::os::unix::fs::symlink(source.join("top.PdF"), source.join("alias.pdf")).unwrap();
    let cancel = AtomicBool::new(false);
    let first = batch(
        std::slice::from_ref(&source),
        false,
        &paths,
        &cancel,
        |_| {},
    )
    .unwrap();
    assert_eq!((first.imported, first.skipped, first.failed), (1, 0, 1));
    let second = batch(&[source], true, &paths, &cancel, |_| {}).unwrap();
    assert_eq!((second.imported, second.skipped, second.failed), (1, 1, 1));
    assert_eq!(Library::open(&paths).unwrap().scores().unwrap().len(), 2);
}

#[test]
fn cancellation_and_invalid_imports_leave_no_partial_records_or_files() {
    let (temp, paths) = setup();
    let source = temp.path().join("input");
    fs::create_dir(&source).unwrap();
    make_pdf(&source.join("one.pdf"), 1, "One");
    make_pdf(&source.join("two.pdf"), 2, "Two");
    let cancel = AtomicBool::new(false);
    let report = batch(&[source], true, &paths, &cancel, |report| {
        if report.imported == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(report.imported, 1);
    assert!(report.cancelled);
    let db = Library::open(&paths).unwrap();
    assert_eq!(db.scores().unwrap().len(), 1);
    assert_eq!(fs::read_dir(paths.data.join("scores")).unwrap().count(), 1);
    let invalid = temp.path().join("broken.pdf");
    fs::write(&invalid, b"%PDF broken").unwrap();
    assert!(import_file(&invalid, &paths, &db, &AtomicBool::new(false)).is_err());
    assert!(import_file(&invalid, &paths, &db, &AtomicBool::new(true)).is_err());
    assert_eq!(fs::read_dir(paths.data.join("scores")).unwrap().count(), 1);
}

#[test]
fn interrupted_managed_copy_is_repaired_by_reimport() {
    let (temp, paths) = setup();
    let source = temp.path().join("score.pdf");
    make_pdf(&source, 1, "Repair");
    let db = Library::open(&paths).unwrap();
    let cancel = AtomicBool::new(false);
    import_file(&source, &paths, &db, &cancel).unwrap();
    let score = db.scores().unwrap().remove(0);
    fs::remove_file(paths.pdf(&score.hash)).unwrap();
    assert_eq!(
        import_file(&source, &paths, &db, &cancel).unwrap(),
        Outcome::Duplicate
    );
    assert!(paths.pdf(&score.hash).exists());
}

#[test]
fn library_persists_metadata_positions_bookmarks_and_migrations() {
    let (_temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let id = db.insert("hash", "Original", 4, "Composer").unwrap();
    let mut score = db.scores().unwrap().remove(0);
    score.title = "Renamed".into();
    score.genre = "Classical".into();
    score.tags = "practice".into();
    db.save_score(&score).unwrap();
    db.remember_page(id, 3).unwrap();
    assert!(db.remember_page(id, 4).is_err());
    db.toggle_bookmark(id, 2).unwrap();
    assert!(db.toggle_bookmark(id, 4).is_err());
    drop(db);
    let db = Library::open(&paths).unwrap();
    let score = db.scores().unwrap().remove(0);
    assert_eq!(score.title, "Renamed");
    assert_eq!(score.last_page, 3);
    assert!(score.matches("CLASSICAL practice"));
    assert!(!score.matches("missing"));
    assert_eq!(db.bookmarks(id).unwrap(), vec![(2, "Page 3".into())]);
    db.toggle_bookmark(id, 2).unwrap();
    assert!(db.bookmarks(id).unwrap().is_empty());
    assert!(db.create_group("", "setlist").is_err());
}

#[test]
fn setlists_preserve_repeats_and_collections_deduplicate() {
    let (_temp, paths) = setup();
    let mut db = Library::open(&paths).unwrap();
    let a = db.insert("a", "A", 2, "").unwrap();
    let b = db.insert("b", "B", 2, "").unwrap();
    let list = db.create_group("Concert", "setlist").unwrap();
    for id in [a, b, a] {
        db.add_entry(list, id).unwrap();
    }
    assert_eq!(
        db.entries(list)
            .unwrap()
            .iter()
            .map(|e| e.score_id)
            .collect::<Vec<_>>(),
        [a, b, a]
    );
    db.move_entry(list, 0, -1).unwrap();
    db.move_entry(list, 0, 1).unwrap();
    assert_eq!(
        db.entries(list)
            .unwrap()
            .iter()
            .map(|e| e.score_id)
            .collect::<Vec<_>>(),
        [b, a, a]
    );
    let entry = db.entries(list).unwrap()[1].id;
    db.remove_entry(entry).unwrap();
    assert_eq!(db.entries(list).unwrap().len(), 2);
    let collection = db.create_group("Practice", "collection").unwrap();
    db.add_entry(collection, a).unwrap();
    db.add_entry(collection, a).unwrap();
    assert_eq!(db.entries(collection).unwrap().len(), 1);
    db.delete_group(list).unwrap();
    assert!(db.entries(list).unwrap().is_empty());
    assert_eq!(db.scores().unwrap().len(), 2);
}

#[test]
fn reader_bounds_two_page_turns_and_repeated_setlist_entries() {
    let mut reader = Reader::default();
    assert_eq!(reader.turn(true), Turn::Boundary);
    reader.load(1, 3, 99);
    assert_eq!(reader.page, 2);
    reader.jump(0);
    reader.two_pages = true;
    assert_eq!(reader.turn(true), Turn::Page);
    assert_eq!(reader.page, 2);
    assert_eq!(reader.turn(true), Turn::Boundary);
    reader.sequence = vec![1, 2, 1];
    assert_eq!(reader.turn(true), Turn::Score(2));
    reader.load(2, 1, 0);
    assert_eq!(reader.turn(true), Turn::Score(1));
    reader.load(1, 3, 0);
    assert_eq!(reader.sequence_index, 2);
    assert_eq!(reader.turn(false), Turn::Score(2));
}

#[test]
fn preferences_roundtrip_and_search() {
    let (_temp, paths) = setup();
    let prefs = Preferences {
        next_key: "Space".into(),
        ..Default::default()
    };
    paths.save_preferences(&prefs).unwrap();
    assert_eq!(paths.preferences().unwrap().next_key, "Space");
    let score = Score {
        title: "Étude".into(),
        composer: "Chopin".into(),
        ..Default::default()
    };
    assert!(score.matches("étu CHOPIN"));
    assert!(!score.matches("Mozart"));
}

#[test]
fn subprocess_timeout_and_cancellation() {
    let start = Instant::now();
    assert!(pdf::run(
        std::process::Command::new("sleep").arg("10"),
        &AtomicBool::new(false),
        Duration::from_millis(100)
    )
    .is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(pdf::run(
        std::process::Command::new("sleep").arg("10"),
        &AtomicBool::new(true),
        Duration::from_secs(5)
    )
    .is_err());
}

#[test]
fn cache_limits_and_background_renderer() {
    assert!(!cache_over_limit(CACHE_PAGES, CACHE_BYTES));
    assert!(cache_over_limit(CACHE_PAGES + 1, 0));
    assert!(cache_over_limit(0, CACHE_BYTES + 1));
    let (temp, paths) = setup();
    let source = temp.path().join("score.pdf");
    make_pdf(&source, 2, "Background rendering");
    let db = Library::open(&paths).unwrap();
    import_file(&source, &paths, &db, &AtomicBool::new(false)).unwrap();
    let hash = db.scores().unwrap()[0].hash.clone();
    let ctx = eframe::egui::Context::default();
    let mut renderer = PageRenderer::new(paths, ctx.clone());
    let key = PageKey {
        hash,
        page: 0,
        pixels: 600,
    };
    renderer.request(key.clone());
    let start = Instant::now();
    loop {
        renderer.poll(&ctx);
        if let Some(texture) = renderer.texture(&key) {
            assert_eq!(texture.size()[1], 600);
            break;
        }
        assert!(
            renderer.failure(&key).is_none(),
            "{:?}",
            renderer.failure(&key)
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    renderer.invalidate_requests();
}

#[test]
fn renderer_evicts_old_pages_and_ignores_stale_generations() {
    let (temp, paths) = setup();
    let source = temp.path().join("cache-test.pdf");
    make_pdf(&source, 15, "Cache test");
    let db = Library::open(&paths).unwrap();
    import_file(&source, &paths, &db, &AtomicBool::new(false)).unwrap();
    let hash = db.scores().unwrap()[0].hash.clone();
    let ctx = eframe::egui::Context::default();
    let mut renderer = PageRenderer::new(paths, ctx.clone());
    let first = PageKey {
        hash: hash.clone(),
        page: 0,
        pixels: 256,
    };
    for page in 0..15 {
        let key = PageKey {
            hash: hash.clone(),
            page,
            pixels: 256,
        };
        renderer.request(key.clone());
        let start = Instant::now();
        loop {
            renderer.poll(&ctx);
            if renderer.texture(&key).is_some() {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        let (pages, bytes) = renderer.cache_usage();
        assert!(pages <= CACHE_PAGES);
        assert!(bytes <= CACHE_BYTES);
    }
    assert!(renderer.texture(&first).is_none());
    assert_eq!(renderer.cache_usage().0, CACHE_PAGES);
    let stale = PageKey {
        hash,
        page: 0,
        pixels: 512,
    };
    renderer.request(stale.clone());
    renderer.invalidate_requests();
    std::thread::sleep(Duration::from_millis(100));
    renderer.poll(&ctx);
    assert!(renderer.texture(&stale).is_none());
}
