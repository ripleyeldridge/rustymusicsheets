use super::*;
use crate::{model::Paths, text_input::ImeCompatibility};

fn frame(
    ctx: &egui::Context,
    manager: &mut SetlistManager,
    db: &mut Library,
    scores: &[Score],
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Outcome) {
    let mut outcome = None;
    let mut ime = ImeCompatibility::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            ime.prepare(ctx);
            outcome = Some(manager.show(ctx, db, scores, &DisplayNames::default()));
        },
    );
    (output, outcome.unwrap())
}

fn text_position(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
    match shape {
        egui::Shape::Text(text) if text.galley.job.text == label => {
            Some(text.pos + text.galley.size() / 2.0)
        }
        egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_position(s, label)),
        _ => None,
    }
}

fn click(
    ctx: &egui::Context,
    manager: &mut SetlistManager,
    db: &mut Library,
    scores: &[Score],
    label: &str,
) -> Outcome {
    let (output, _) = frame(ctx, manager, db, scores, vec![]);
    let position = output
        .shapes
        .iter()
        .find_map(|s| text_position(&s.shape, label))
        .unwrap_or_else(|| panic!("Button not rendered: {label}"));
    frame(
        ctx,
        manager,
        db,
        scores,
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        manager,
        db,
        scores,
        vec![egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    )
    .1
}

#[test]
fn manager_buttons_rename_copy_reorder_add_remove_play_and_confirm_delete() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: root.path().join("data"),
        config: root.path().join("config"),
        cache: root.path().join("cache"),
        music: None,
    };
    paths.create().unwrap();
    let mut db = Library::open(&paths).unwrap();
    let a = db.insert("a", "Alpha", 2, "").unwrap();
    let b = db.insert("b", "Beta", 2, "").unwrap();
    let group = db.create_group("Concert", "setlist").unwrap();
    for id in [a, b, a] {
        db.add_entry(group, id).unwrap();
    }
    let scores = db.scores().unwrap();
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut manager = SetlistManager::new(&db.groups().unwrap(), Some(group));
    for _ in 0..3 {
        frame(&ctx, &mut manager, &mut db, &scores, vec![]);
    }
    manager.name = "Evening concert".into();
    assert!(click(&ctx, &mut manager, &mut db, &scores, "Rename").changed);
    assert_eq!(db.groups().unwrap()[0].name, "Evening concert");
    let outcome = click(&ctx, &mut manager, &mut db, &scores, "Duplicate");
    assert!(outcome.changed);
    let copy = outcome.selected.unwrap();
    assert_ne!(copy, group);
    assert_eq!(db.entries(copy).unwrap().len(), 3);
    assert!(click(&ctx, &mut manager, &mut db, &scores, "Down").changed);
    assert_eq!(db.entries(copy).unwrap()[0].score_id, b);
    assert!(click(&ctx, &mut manager, &mut db, &scores, "Remove").changed);
    assert_eq!(db.entries(copy).unwrap().len(), 2);
    assert!(click(&ctx, &mut manager, &mut db, &scores, "+ Add").changed);
    assert_eq!(db.entries(copy).unwrap().len(), 3);
    click(&ctx, &mut manager, &mut db, &scores, "Delete list…");
    click(&ctx, &mut manager, &mut db, &scores, "Cancel deletion");
    assert_eq!(db.groups().unwrap().len(), 2);
    let playing = click(&ctx, &mut manager, &mut db, &scores, "Play setlist");
    assert_eq!(playing.play, Some((copy, 0)));
    assert!(!playing.open);
    click(&ctx, &mut manager, &mut db, &scores, "Delete list…");
    assert!(click(&ctx, &mut manager, &mut db, &scores, "Confirm delete list").changed);
    assert_eq!(db.groups().unwrap().len(), 1);
    assert_eq!(db.entries(group).unwrap().len(), 3);
    assert_eq!(db.scores().unwrap().len(), 2);
}

#[test]
fn shrinking_manager_keeps_rename_controls_visible() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: root.path().join("data"),
        config: root.path().join("config"),
        cache: root.path().join("cache"),
        music: None,
    };
    paths.create().unwrap();
    let mut db = Library::open(&paths).unwrap();
    let score = db.insert("a", "Resize score", 2, "").unwrap();
    let group = db.create_group("Resize concert", "setlist").unwrap();
    for _ in 0..10 {
        db.add_entry(group, score).unwrap();
    }
    let scores = db.scores().unwrap();
    let mut manager = SetlistManager::new(&db.groups().unwrap(), Some(group));
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    for size in [
        [1280.0, 900.0],
        [800.0, 500.0],
        [640.0, 360.0],
        [1280.0, 900.0],
    ] {
        let mut visible = false;
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                    ..Default::default()
                },
                |ctx| {
                    let outcome = manager.show(ctx, &mut db, &scores, &DisplayNames::default());
                    assert!(outcome.error.is_none());
                },
            );
            visible = output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Rename" => {
                    let clipped = shape
                        .clip_rect
                        .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    clipped.height() > 8.0 && clipped.width() > 20.0
                }
                _ => false,
            });
        }
        assert!(visible, "Manager header clipped at {size:?}");
    }
}

#[test]
fn duplicate_names_are_unique_within_the_same_group_type() {
    let group = Group {
        id: 1,
        name: "Concert".into(),
        kind: "setlist".into(),
    };
    let groups = vec![
        group.clone(),
        Group {
            id: 2,
            name: "Concert (copy)".into(),
            kind: "setlist".into(),
        },
        Group {
            id: 3,
            name: "Concert (copy 2)".into(),
            kind: "setlist".into(),
        },
    ];
    assert_eq!(copy_name(&groups, &group), "Concert (copy 3)");
}
