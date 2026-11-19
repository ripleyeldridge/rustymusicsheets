use super::{tests::test_app, *};
use crate::annotations::store;
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, Rect};

fn generated_pdf(path: &std::path::Path, pages: u32, name: &str) {
    let kids = (0..pages)
        .map(|n| format!("{} 0 R ", 3 + 2 * n))
        .collect::<String>();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
    ];
    for page in 0..pages {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /Contents {} 0 R >>",
            4 + 2 * page
        ));
        let stream = format!("% {name} generated page {page}\n0 0 0 RG 1 w 30 300 m 370 300 l S\n");
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
    }
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn performance_save_status_does_not_cut_strokes_or_turn_pages() {
    let (_root, ctx, mut app, _, hash) = fixture();
    app.performance = true;
    let size = [900.0, 650.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    for count in 1..=2 {
        let rect = ready(&mut app, &ctx, size)[0];
        draw_stroke(&mut app, &ctx, rect, size);
        flush(&mut app, &ctx, size);
        let ink = store::load(&app.paths.data, &hash, 0, 3).unwrap().ink;
        assert_eq!(ink.strokes.len(), count);
        assert!(ink.strokes.iter().all(|s| s.points.len() >= 3));
    }
    assert_eq!(app.reader.page, 0);
    assert!(app.performance);
}

#[test]
fn cancelled_standalone_switch_preserves_the_current_setlist() {
    let (root, ctx, mut app, _, _) = fixture();
    let original = app.reader.score_id.unwrap();
    let (next, _, _) = add_score(&mut app, root.path(), "Standalone generated score", 2);
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rect = ready(&mut app, &ctx, size)[0];
    draw_stroke(&mut app, &ctx, rect, size);
    app.reader.sequence = vec![original, next, original];
    app.reader.sequence_index = 2;
    app.open_standalone_score(next);
    assert_eq!(app.reader.score_id, Some(original));
    assert_eq!(app.reader.sequence, [original, next, original]);
    assert_eq!(app.reader.sequence_index, 2);
    assert!(app
        .pending_annotated_score
        .as_ref()
        .unwrap()
        .sequence
        .as_ref()
        .unwrap()
        .0
        .is_empty());
    app.pending_annotated_score = None; // Explicit "Stay on this score" outcome.
    assert_eq!(app.reader.sequence_index, 2);
    app.annotations.discard_unsaved();
}

fn add_score(
    app: &mut MusicApp,
    _root: &std::path::Path,
    name: &str,
    pages: u32,
) -> (i64, PathBuf, String) {
    let relative = PathBuf::from(format!("{name}.pdf"));
    let path = app.paths.score_dir().join(&relative);
    generated_pdf(&path, pages, name);
    let hash = crate::folder::hash_file(&path, &AtomicBool::new(false)).unwrap();
    let id = app
        .db
        .insert_file(&hash, name, pages, "", &relative)
        .unwrap();
    app.reload();
    (id, path, hash)
}

fn fixture() -> (tempfile::TempDir, egui::Context, MusicApp, PathBuf, String) {
    let (root, ctx, mut app) = test_app();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let (id, source, hash) = add_score(&mut app, root.path(), "Generated ink", 3);
    app.load_score(id, Some(0));
    app.show_options = false;
    (root, ctx, app, source, hash)
}

fn frame(
    app: &mut MusicApp,
    ctx: &egui::Context,
    size: [f32; 2],
    events: Vec<Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size.into())),
            events,
            ..Default::default()
        },
        |ctx| {
            app.closing_this_frame = false;
            app.cancel_close_this_frame = false;
            app.poll(ctx);
            app.annotation_lifecycle(ctx);
            app.shortcuts(ctx);
            app.text_input_active = false;
            app.status_bar(ctx);
            app.workspace_view(ctx);
            app.annotation_recovery_ui(ctx);
            app.final_annotation_close_checkpoint(ctx);
        },
    )
}

fn text_rect(output: &egui::FullOutput, label: &str) -> Option<Rect> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.job.text == label => {
            let rect = shape
                .clip_rect
                .intersect(Rect::from_min_size(text.pos, text.galley.size()));
            (rect.width() > 8.0 && rect.height() > 8.0).then_some(rect)
        }
        _ => None,
    })
}

fn page_rects(output: &egui::FullOutput) -> Vec<Rect> {
    let mut rects: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rectangle) if rectangle.brush.is_some() => {
                let rect = rectangle.rect;
                (rect.width() > 50.0
                    && rect.height() > 60.0
                    && rect.intersect(shape.clip_rect).height() > 60.0)
                    .then_some(rect)
            }
            egui::Shape::Mesh(mesh)
                if mesh.texture_id != egui::TextureId::Managed(0) && mesh.vertices.len() == 4 =>
            {
                let rect =
                    Rect::from_points(&mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>());
                (rect.width() > 50.0
                    && rect.height() > 60.0
                    && rect.intersect(shape.clip_rect).height() > 60.0)
                    .then_some(rect)
            }
            _ => None,
        })
        .collect();
    rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
    rects
}

fn ready(app: &mut MusicApp, ctx: &egui::Context, size: [f32; 2]) -> Vec<Rect> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = frame(app, ctx, size, vec![]);
        let rects = page_rects(&output);
        let expected_pages = if app.reader.two_pages && app.reader.page + 1 < app.reader.pages {
            2
        } else {
            1
        };
        if rects.len() == expected_pages
            && app.annotations.writable(app.reader.page)
            && (!app.reader.two_pages || app.annotations.writable(app.reader.page + 1))
        {
            for _ in 0..2 {
                frame(app, ctx, size, vec![]);
            }
            return page_rects(&frame(app, ctx, size, vec![]));
        }
        assert!(
            Instant::now() < deadline,
            "Reader/ink loading timed out: {:?}",
            app.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pointer(pos: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        },
    ]
}

fn click(app: &mut MusicApp, ctx: &egui::Context, size: [f32; 2], label: &str) {
    for attempt in 0..35 {
        let output = frame(app, ctx, size, vec![]);
        if let Some(rect) = text_rect(&output, label) {
            for pressed in [true, false] {
                frame(app, ctx, size, pointer(rect.center(), pressed));
            }
            return;
        }
        frame(
            app,
            ctx,
            size,
            vec![
                Event::PointerMoved(egui::pos2(300.0, 70.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, if attempt < 5 { 80.0 } else { -80.0 }),
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    panic!("Missing reachable control {label}");
}

fn draw_stroke(app: &mut MusicApp, ctx: &egui::Context, rect: Rect, size: [f32; 2]) {
    let a = rect.lerp_inside(egui::vec2(0.2, 0.3));
    let b = rect.lerp_inside(egui::vec2(0.5, 0.4));
    let c = rect.lerp_inside(egui::vec2(0.7, 0.6));
    frame(app, ctx, size, pointer(a, true));
    frame(
        app,
        ctx,
        size,
        vec![Event::PointerMoved(b), Event::PointerMoved(c)],
    );
    frame(app, ctx, size, pointer(c, false));
}

fn flush(app: &mut MusicApp, ctx: &egui::Context, size: [f32; 2]) {
    app.annotations.finish_gesture();
    app.annotations.save_now();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.annotations.has_unsaved() {
        frame(app, ctx, size, vec![]);
        assert!(!app.annotations.has_save_error(), "Annotation save failed");
        assert!(Instant::now() < deadline, "Annotation save hung");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn reader_draw_save_reload_and_undo_preserve_source_bytes() {
    let (_root, ctx, mut app, source, hash) = fixture();
    let original = std::fs::read(&source).unwrap();
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    assert!(app.annotations.editing());
    let rect = ready(&mut app, &ctx, size)[0];
    draw_stroke(&mut app, &ctx, rect, size);
    assert_eq!(app.reader.page, 0, "Drawing must not turn pages");
    flush(&mut app, &ctx, size);
    let loaded = store::load(&app.paths.data, &hash, 0, 3).unwrap();
    assert_eq!(loaded.ink.strokes.len(), 1);
    assert!(loaded.ink.strokes[0].points.len() >= 3);
    assert!((loaded.ink.strokes[0].points[0].x - 0.2).abs() < 0.01);
    for (key, expected) in [(Key::Z, 0), (Key::Y, 1)] {
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                size,
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::CTRL | Modifiers::COMMAND,
                }],
            );
        }
        flush(&mut app, &ctx, size);
        assert_eq!(
            store::load(&app.paths.data, &hash, 0, 3)
                .unwrap()
                .ink
                .strokes
                .len(),
            expected
        );
    }
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn spread_input_belongs_to_right_page_and_resize_keeps_coordinates() {
    let (_root, ctx, mut app, _, hash) = fixture();
    app.reader.two_pages = true;
    let size = [1200.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rects = ready(&mut app, &ctx, size);
    assert_eq!(rects.len(), 2);
    draw_stroke(&mut app, &ctx, rects[1], size);
    flush(&mut app, &ctx, size);
    let before = store::load(&app.paths.data, &hash, 1, 3).unwrap().ink;
    assert_eq!(before.strokes.len(), 1);
    assert!(store::load(&app.paths.data, &hash, 0, 3)
        .unwrap()
        .ink
        .strokes
        .is_empty());
    for (key, count) in [(Key::Z, 0), (Key::Y, 1)] {
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                size,
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::CTRL | Modifiers::COMMAND,
                }],
            );
        }
        flush(&mut app, &ctx, size);
        assert_eq!(
            store::load(&app.paths.data, &hash, 1, 3)
                .unwrap()
                .ink
                .strokes
                .len(),
            count
        );
        assert!(store::load(&app.paths.data, &hash, 0, 3)
            .unwrap()
            .ink
            .strokes
            .is_empty());
    }
    ready(&mut app, &ctx, [900.0, 650.0]);
    assert_eq!(
        store::load(&app.paths.data, &hash, 1, 3).unwrap().ink,
        before
    );
    assert_eq!(app.reader.page, 0);
}

#[test]
fn initial_contact_and_pedal_in_same_frame_cannot_turn_under_ink() {
    let (_root, ctx, mut app, _, hash) = fixture();
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rect = ready(&mut app, &ctx, size)[0];
    let pos = rect.center();
    let mut events = pointer(pos, true);
    events.push(Event::Key {
        key: Key::PageDown,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    frame(&mut app, &ctx, size, events);
    assert_eq!(app.reader.page, 0);
    let mut events = pointer(pos, false);
    events.push(Event::Key {
        key: Key::PageDown,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    frame(&mut app, &ctx, size, events);
    flush(&mut app, &ctx, size);
    assert_eq!(
        store::load(&app.paths.data, &hash, 0, 3)
            .unwrap()
            .ink
            .strokes
            .len(),
        1
    );
    for pressed in [true, false] {
        frame(
            &mut app,
            &ctx,
            size,
            vec![Event::Key {
                key: Key::PageDown,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
    }
    assert_eq!(app.reader.page, 1, "Pedal should work between gestures");
}

#[test]
fn escape_leaves_annotations_without_exiting_performance() {
    let (_root, ctx, mut app, _, _) = fixture();
    app.performance = true;
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    app.music.controls_active = false;
    for pressed in [true, false] {
        frame(
            &mut app,
            &ctx,
            size,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
    }
    assert!(!app.annotations.editing());
    assert!(app.performance);
}

#[test]
fn failed_save_defers_score_switch_and_setlist_position_until_retry() {
    let (root, ctx, mut app, _, hash) = fixture();
    let original = app.reader.score_id;
    let (next, _, _) = add_score(&mut app, root.path(), "Another generated score", 2);
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rect = ready(&mut app, &ctx, size)[0];
    let directory = app.paths.data.join("annotations").join(&hash);
    std::fs::create_dir_all(&directory).unwrap();
    let lock = directory.join("0.ink-v1.json.lock");
    std::fs::write(&lock, b"generated lock fixture").unwrap();
    draw_stroke(&mut app, &ctx, rect, size);
    app.reader.sequence = vec![original.unwrap(), next];
    app.reader.sequence_index = 0;
    app.reader.page = 2;
    app.turn(true);
    assert!(app.pending_annotated_score.is_some());
    assert_eq!(app.reader.score_id, original);
    assert_eq!(app.reader.sequence_index, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.annotations.has_save_error() {
        frame(&mut app, &ctx, size, vec![]);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.reader.score_id, original);
    std::fs::remove_file(lock).unwrap();
    app.annotations.save_now();
    while app.pending_annotated_score.is_some() {
        frame(&mut app, &ctx, size, vec![]);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.reader.score_id, Some(next));
    assert_eq!(app.reader.sequence_index, 1);
    assert_eq!(
        store::load(&app.paths.data, &hash, 0, 3)
            .unwrap()
            .ink
            .strokes
            .len(),
        1
    );
}

#[test]
fn close_request_waits_for_dirty_ink_and_emits_cancel_close() {
    let (_root, ctx, mut app, _, hash) = fixture();
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rect = ready(&mut app, &ctx, size)[0];
    draw_stroke(&mut app, &ctx, rect, size);
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = ctx.run(input, |ctx| app.annotation_lifecycle(ctx));
    assert!(app.closing_annotations);
    assert!(output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::CancelClose));
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.closing_annotations {
        frame(&mut app, &ctx, size, vec![]);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.annotations.has_unsaved());
    assert_eq!(
        store::load(&app.paths.data, &hash, 0, 3)
            .unwrap()
            .ink
            .strokes
            .len(),
        1
    );
}

#[test]
fn programmatic_close_cancel_does_not_queue_a_later_close_event() {
    let (_root, ctx, mut app) = test_app();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.request_app_close(ctx);
        app.cancel_app_close(ctx);
        app.final_annotation_close_checkpoint(ctx);
    });
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(!commands.contains(&egui::ViewportCommand::Close));
    assert!(commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(!app.closing_this_frame);
    assert!(!app.closing_annotations);
}

#[test]
fn close_intent_in_same_frame_as_new_ink_is_checked_after_canvas() {
    for native_request in [false, true] {
        let (_root, ctx, mut app, _, hash) = fixture();
        let size = [1100.0, 800.0];
        ready(&mut app, &ctx, size);
        click(&mut app, &ctx, size, "Annotate");
        let rect = ready(&mut app, &ctx, size)[0];
        assert!(!app.annotations.has_unsaved());
        let pos = rect.center();
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size.into())),
            events: [
                pointer(pos, true),
                pointer(pos + egui::vec2(15.0, 10.0), false),
            ]
            .concat(),
            ..Default::default()
        };
        if native_request {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        let output = ctx.run(input, |ctx| {
            app.closing_this_frame = false;
            app.cancel_close_this_frame = false;
            app.poll(ctx);
            app.annotation_lifecycle(ctx);
            assert!(
                !app.closing_annotations,
                "No ink existed at the early checkpoint"
            );
            if !native_request {
                app.request_app_close(ctx);
            }
            app.shortcuts(ctx);
            app.text_input_active = false;
            app.status_bar(ctx);
            app.workspace_view(ctx);
            app.final_annotation_close_checkpoint(ctx);
        });
        assert!(app.closing_annotations);
        assert!(app.annotations.has_unsaved());
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose));
        flush(&mut app, &ctx, size);
        assert_eq!(
            store::load(&app.paths.data, &hash, 0, 3)
                .unwrap()
                .ink
                .strokes
                .len(),
            1
        );
    }
}

#[test]
fn same_frame_undo_and_explicit_close_cancel_keep_unsaved_ink_safe() {
    let (_root, ctx, mut app, _, hash) = fixture();
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    click(&mut app, &ctx, size, "Annotate");
    let rect = ready(&mut app, &ctx, size)[0];
    draw_stroke(&mut app, &ctx, rect, size);
    flush(&mut app, &ctx, size);
    let mut input = egui::RawInput {
        events: vec![Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL | Modifiers::COMMAND,
        }],
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = ctx.run(input, |ctx| {
        app.closing_this_frame = false;
        app.cancel_close_this_frame = false;
        app.annotation_lifecycle(ctx);
        assert!(!app.closing_annotations);
        app.shortcuts(ctx);
        app.final_annotation_close_checkpoint(ctx);
    });
    assert!(app.closing_annotations);
    assert!(output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::CancelClose));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        app.cancel_app_close(ctx);
        app.final_annotation_close_checkpoint(ctx);
    });
    assert!(!app.closing_annotations);
    assert!(app.annotations.has_unsaved());
    flush(&mut app, &ctx, size);
    assert!(store::load(&app.paths.data, &hash, 0, 3)
        .unwrap()
        .ink
        .strokes
        .is_empty());
}

#[test]
fn reader_controls_switch_cannot_render_previous_score_surfaces() {
    let (root, ctx, mut app, _, _) = fixture();
    let previous = app.reader.score_id.unwrap();
    let (next, next_source, next_hash) = add_score(&mut app, root.path(), "Next clean score", 2);
    let size = [1100.0, 800.0];
    ready(&mut app, &ctx, size);
    app.reader.sequence = vec![previous, next];
    app.reader.sequence_index = 0;
    app.reader.page = 2;
    app.changed_page();
    ready(&mut app, &ctx, size);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_rect(&output, "Next ▶").unwrap().center();
    frame(&mut app, &ctx, size, pointer(pos, true));
    let output = frame(&mut app, &ctx, size, pointer(pos, false));
    assert_eq!(app.reader.score_id, Some(next));
    assert!(app.annotations.matches_score(&next_hash, 2, &next_source));
    assert!(
        page_rects(&output).is_empty(),
        "Score changed inside controls; stale surfaces must not be emitted"
    );
    assert!(!ready(&mut app, &ctx, size).is_empty());
}

#[test]
fn annotation_tools_are_scroll_reachable_in_small_reader_and_performance() {
    for performance in [false, true] {
        let (_root, ctx, mut app, _, _) = fixture();
        app.performance = performance;
        let size = [640.0, 360.0];
        ready(&mut app, &ctx, size);
        click(&mut app, &ctx, size, "Annotate");
        let mut seen = std::collections::BTreeSet::new();
        for step in 0..80 {
            let pos = if performance {
                egui::pos2(300.0, 230.0)
            } else {
                egui::pos2(300.0, 70.0)
            };
            let output = frame(
                &mut app,
                &ctx,
                size,
                vec![
                    Event::PointerMoved(pos),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, if step < 20 { 100.0 } else { -100.0 }),
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let visible = shape
                        .clip_rect
                        .intersect(Rect::from_min_size(text.pos, text.galley.size()));
                    if visible.height() > 8.0 && visible.width() > 10.0 {
                        seen.insert(text.galley.job.text.clone());
                    }
                }
            }
        }
        for label in [
            "Pen",
            "Highlighter",
            "Stroke eraser",
            "Hand / Pan",
            "Save now",
            "Undo",
            "Redo",
            "Done",
            "Clear page…",
        ] {
            assert!(
                seen.contains(label),
                "Unreachable {label}, performance={performance}; {seen:?}"
            );
        }
    }
}
