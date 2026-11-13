use super::*;

#[test]
fn rename_duplicate_reorder_and_delete_preserve_scores_and_original_lists() {
    let (temp, paths) = setup();
    let first = temp.path().join("First.pdf");
    let second = temp.path().join("Second.pdf");
    make_pdf(&first, 2, "First");
    make_pdf(&second, 2, "Second");
    let mut db = Library::open(&paths).unwrap();
    let cancel = AtomicBool::new(false);
    import_file(&first, &paths, &db, &cancel).unwrap();
    import_file(&second, &paths, &db, &cancel).unwrap();
    let scores = db.scores().unwrap();
    let a = scores[0].id;
    let b = scores[1].id;
    let original = db.create_group("Concert", "setlist").unwrap();
    for id in [a, b, a] {
        db.add_entry(original, id).unwrap();
    }
    db.rename_group(original, "  Evening recital  ").unwrap();
    assert_eq!(db.groups().unwrap()[0].name, "Evening recital");
    let copy = db.duplicate_group(original, "Rehearsal").unwrap();
    let original_entries = db.entries(original).unwrap();
    let copied = db.entries(copy).unwrap();
    assert_eq!(
        copied.iter().map(|e| e.score_id).collect::<Vec<_>>(),
        [a, b, a]
    );
    assert!(copied
        .iter()
        .all(|entry| original_entries.iter().all(|old| old.id != entry.id)));
    db.move_entry(copy, 0, 1).unwrap();
    db.remove_entry(db.entries(copy).unwrap()[1].id).unwrap();
    assert_eq!(
        db.entries(copy)
            .unwrap()
            .iter()
            .map(|e| e.score_id)
            .collect::<Vec<_>>(),
        [b, a]
    );
    assert_eq!(
        db.entries(original)
            .unwrap()
            .iter()
            .map(|e| e.score_id)
            .collect::<Vec<_>>(),
        [a, b, a]
    );
    assert!(db.rename_group(copy, "Evening recital").is_err());
    assert!(db.rename_group(copy, " ").is_err());
    assert!(db.rename_group(i64::MAX, "Missing").is_err());
    assert!(db.duplicate_group(original, "Rehearsal").is_err());
    assert!(db.duplicate_group(original, " ").is_err());
    assert_eq!(db.groups().unwrap().len(), 2);
    drop(db);
    let db = Library::open(&paths).unwrap();
    assert_eq!(db.entries(copy).unwrap().len(), 2);
    db.delete_group(copy).unwrap();
    assert_eq!(db.entries(original).unwrap().len(), 3);
    assert_eq!(db.scores().unwrap().len(), 2);
    for score in db.scores().unwrap() {
        assert!(db.pdf_path(&paths, &score.hash).unwrap().exists());
    }
    assert!(first.exists() && second.exists());
}

#[test]
fn collection_copies_keep_collection_semantics() {
    let (_temp, paths) = setup();
    let mut db = Library::open(&paths).unwrap();
    let score = db.insert("score", "Practice score", 1, "").unwrap();
    let collection = db.create_group("Practice", "collection").unwrap();
    db.add_entry(collection, score).unwrap();
    let copy = db.duplicate_group(collection, "Next week").unwrap();
    db.add_entry(copy, score).unwrap();
    assert_eq!(db.entries(copy).unwrap().len(), 1);
    assert_eq!(
        db.groups()
            .unwrap()
            .iter()
            .find(|g| g.id == copy)
            .unwrap()
            .kind,
        "collection"
    );
}
