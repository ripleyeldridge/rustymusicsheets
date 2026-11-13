use super::*;
use crate::{
    folder,
    instruments::{classify, Candidate, Instrument},
};
use std::sync::atomic::AtomicBool;

fn setup() -> (tempfile::TempDir, Paths, Library, i64, String) {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: root.path().join("data"),
        config: root.path().join("config"),
        cache: root.path().join("cache"),
        music: None,
    };
    paths.create().unwrap();
    let file = paths.score_dir().join("Part.png");
    image::RgbImage::from_pixel(40, 50, image::Rgb([255, 255, 255]))
        .save(&file)
        .unwrap();
    let hash = folder::hash_file(&file, &AtomicBool::new(false)).unwrap();
    let db = Library::open(&paths).unwrap();
    let id = db
        .insert_file(&hash, "Part", 1, "", std::path::Path::new("Part.png"))
        .unwrap();
    let metadata = std::fs::metadata(&file).unwrap();
    let mut result = Detection {
        file_size: metadata.len(),
        modified_ns: metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_nanos()).ok()),
        pages: 1,
        ..Default::default()
    };
    classify::text("Cello", 1, "test", &mut result);
    db.store_detection(id, &hash, &result, false).unwrap();
    (root, paths, db, id, hash)
}

fn frame(
    ctx: &egui::Context,
    review: &mut Review,
    db: &Library,
    paths: &Paths,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Outcome) {
    let mut outcome = None;
    let scores = db.scores().unwrap();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::SidePanel::left("review_test")
                .default_width(340.0)
                .show(ctx, |ui| {
                    outcome =
                        Some(review.show(ui, db, paths, &scores, &DisplayNames::default(), false));
                });
        },
    );
    (output, outcome.unwrap())
}

fn click(
    ctx: &egui::Context,
    review: &mut Review,
    db: &Library,
    paths: &Paths,
    label: &str,
) -> Outcome {
    let (output, _) = frame(ctx, review, db, paths, [1280.0, 900.0], vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("Review action must be rendered");
    frame(
        ctx,
        review,
        db,
        paths,
        [1280.0, 900.0],
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        review,
        db,
        paths,
        [1280.0, 900.0],
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    )
    .1
}

#[test]
fn review_shows_large_library_warning_before_detection() {
    let (_root, paths, db, id, _) = setup();
    let ctx = egui::Context::default();
    let mut review = Review::default();
    review.open(Some(id));
    for _ in 0..3 {
        frame(&ctx, &mut review, &db, &paths, [1280.0, 900.0], vec![]);
    }
    let (output, _) = frame(&ctx, &mut review, &db, &paths, [1280.0, 900.0], vec![]);
    assert!(output.shapes.iter().any(|shape| matches!(
        &shape.shape,
        egui::Shape::Text(text) if text.galley.job.text == crate::instruments::DETECTION_TIME_WARNING
    )));
}

#[test]
fn review_apply_requires_explicit_manual_replacement() {
    let (_root, paths, db, id, hash) = setup();
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut review = Review::default();
    review.open(Some(id));
    for _ in 0..3 {
        frame(&ctx, &mut review, &db, &paths, [1280.0, 900.0], vec![]);
    }
    assert!(click(&ctx, &mut review, &db, &paths, "Apply selected instruments").applied);
    assert!(db.scores().unwrap()[0].instruments_manual);
    db.apply_instruments(
        id,
        &hash,
        &[Instrument {
            name: "Saxophone".into(),
            ..Default::default()
        }],
    )
    .unwrap();
    review.invalidate();
    assert!(!click(&ctx, &mut review, &db, &paths, "Apply selected instruments").applied);
    assert_eq!(db.instruments_for(id).unwrap()[0].name, "Saxophone");
    review.replace_manual = true;
    assert!(click(&ctx, &mut review, &db, &paths, "Apply selected instruments").applied);
    assert_eq!(db.instruments_for(id).unwrap()[0].name, "Cello");
}

#[test]
fn review_rejects_file_changes_before_the_watcher_updates_its_hash() {
    let (_root, paths, db, id, _) = setup();
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut review = Review::default();
    review.open(Some(id));
    for _ in 0..3 {
        frame(&ctx, &mut review, &db, &paths, [1280.0, 900.0], vec![]);
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(paths.score_dir().join("Part.png"))
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let result = click(&ctx, &mut review, &db, &paths, "Apply selected instruments");
    assert!(!result.applied);
    assert!(result.error.unwrap().contains("File changed"));
    assert!(db.instruments_for(id).unwrap().is_empty());
}

#[test]
fn small_review_panel_can_scroll_to_the_last_piece_of_evidence() {
    let (_root, paths, db, id, hash) = setup();
    let mut result = db.detection(id, &hash).unwrap().unwrap();
    result.candidates = (0..32)
        .map(|index| Candidate {
            instrument: Instrument {
                name: format!("Custom {index}"),
                ..Default::default()
            },
            evidence: if index == 31 {
                "End marker".into()
            } else {
                format!("Evidence {index}")
            },
            page: 1,
            method: "test".into(),
            clear: false,
            ocr_confidence: None,
        })
        .collect();
    db.store_detection(id, &hash, &result, false).unwrap();
    let mut review = Review::default();
    review.open(Some(id));
    let ctx = egui::Context::default();
    let mut reached = false;
    for index in 0..45 {
        let events = if index < 3 {
            vec![]
        } else {
            vec![
                egui::Event::PointerMoved(egui::pos2(150.0, 280.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -600.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let (output, _) = frame(&ctx, &mut review, &db, &paths, [640.0, 360.0], events);
        reached |= output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Read: End marker" => {
                shape
                    .clip_rect
                    .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    .height()
                    > 8.0
            }
            _ => false,
        });
    }
    assert!(
        reached,
        "All OCR evidence must remain reachable in a small panel"
    );
}
