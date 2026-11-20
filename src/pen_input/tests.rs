use super::*;
use egui::{pos2, Modifiers};

#[test]
fn toolkit_delivered_press_is_detected_without_consuming_mouse_or_touch() {
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        events: vec![button(pos2(40.0, 50.0), true, Modifiers::NONE)],
        ..Default::default()
    };
    assert!(toolkit_has_contact(&ctx, &raw, pos2(40.0, 50.0)));
    assert!(!toolkit_has_contact(&ctx, &raw, pos2(100.0, 200.0)));
    assert_eq!(
        raw.events.len(),
        1,
        "Detection must not intercept backend input"
    );
}

#[test]
fn pending_fallback_yields_to_toolkit_and_drops_released_or_lost_contacts() {
    let pending = packet(Kind::Button {
        button: 1,
        pressed: true,
    });
    let held = Some((12, pos2(125.0, 245.0), true));
    assert!(!settle_contact(pending, held, true).unwrap().supplement);
    assert!(settle_contact(pending, held, false).unwrap().supplement);
    assert!(settle_contact(pending, None, false).is_none());
    assert!(settle_contact(pending, Some((12, pending.pos, false)), false).is_none());
    assert!(settle_contact(pending, Some((13, pending.pos, true)), false).is_none());
}

fn packet(kind: Kind) -> Packet {
    Packet {
        source: 12,
        pos: pos2(120.0, 240.0),
        supplement: true,
        kind,
    }
}

#[test]
fn duplicate_matching_keeps_mouse_events_and_distinct_pen_taps() {
    let pos = pos2(40.0, 50.0);
    let mouse = pos2(100.0, 200.0);
    let mut events = vec![
        button(mouse, true, Modifiers::NONE),
        button(pos, true, Modifiers::NONE),
        button(pos, false, Modifiers::NONE),
        button(mouse, false, Modifiers::NONE),
        Event::Text("untouched generated input".into()),
    ];
    append_supplement(
        &mut events,
        vec![
            button(pos, true, Modifiers::NONE),
            button(pos, false, Modifiers::NONE),
            button(pos, true, Modifiers::NONE),
            button(pos, false, Modifiers::NONE),
        ],
    );
    assert_eq!(events.len(), 7);
    assert_eq!(
        events
            .iter()
            .filter(
                |e| matches!(e, Event::PointerButton { pos: p, pressed: true, .. } if *p == pos)
            )
            .count(),
        2
    );
    assert!(matches!(events[0], Event::PointerButton { pos: p, pressed: true, .. } if p == mouse));
    assert!(matches!(&events[4], Event::Text(text) if text == "untouched generated input"));
}

#[test]
fn hover_side_buttons_and_normally_delivered_buttons_do_not_inject_contacts() {
    let mut translator = Translator::default();
    let mut events = Vec::new();
    let mut evidence = Provenance::default();
    for kind in [
        Kind::Move,
        Kind::Button {
            button: 2,
            pressed: true,
        },
        Kind::Button {
            button: 1,
            pressed: false,
        },
    ] {
        translator.translate(
            packet(kind),
            2.0,
            Modifiers::NONE,
            &mut events,
            &mut evidence,
        );
    }
    assert!(events.is_empty() && translator.active.is_none());
    let mut normal = packet(Kind::Button {
        button: 1,
        pressed: true,
    });
    normal.supplement = false;
    translator.translate(normal, 2.0, Modifiers::NONE, &mut events, &mut evidence);
    assert!(events.is_empty() && translator.active.is_none());
    assert_eq!(evidence.pointer_presses, [pos2(60.0, 120.0)]);
}

#[test]
fn emulated_tip_has_one_ordered_scaled_contact_and_release() {
    let mut translator = Translator::default();
    let mut events = Vec::new();
    let mut evidence = Provenance::default();
    for kind in [
        Kind::Button {
            button: 1,
            pressed: true,
        },
        Kind::Move,
        Kind::Button {
            button: 1,
            pressed: false,
        },
    ] {
        translator.translate(
            packet(kind),
            2.0,
            Modifiers::CTRL,
            &mut events,
            &mut evidence,
        );
    }
    let touches: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Touch { id, phase, pos, .. } => Some((*id, *phase, *pos)),
            _ => None,
        })
        .collect();
    assert_eq!(touches.len(), 3);
    assert_eq!(
        touches.iter().map(|t| t.1).collect::<Vec<_>>(),
        [TouchPhase::Start, TouchPhase::Move, TouchPhase::End]
    );
    assert!(touches
        .iter()
        .all(|t| t.0 == touches[0].0 && t.2 == pos2(60.0, 120.0)));
    assert!(translator.active.is_none());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::PointerButton { .. }))
            .count(),
        2
    );
}

#[test]
fn foreign_pen_duplicate_down_invalid_coordinates_and_cancel_are_safe() {
    let mut translator = Translator::default();
    let mut events = Vec::new();
    let mut evidence = Provenance::default();
    let down = packet(Kind::Button {
        button: 1,
        pressed: true,
    });
    translator.translate(down, 1.0, Modifiers::NONE, &mut events, &mut evidence);
    translator.translate(down, 1.0, Modifiers::NONE, &mut events, &mut evidence);
    let before = events.len();
    let mut foreign = packet(Kind::Button {
        button: 1,
        pressed: false,
    });
    foreign.source = 13;
    translator.translate(foreign, 1.0, Modifiers::NONE, &mut events, &mut evidence);
    assert_eq!(events.len(), before);
    translator.translate(
        packet(Kind::Move),
        f32::NAN,
        Modifiers::NONE,
        &mut events,
        &mut evidence,
    );
    assert!(translator.active.is_none());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                Event::Touch {
                    phase: TouchPhase::Cancel,
                    ..
                }
            ))
            .count(),
        1
    );
    let after = events.len();
    translator.cancel(&mut events, &mut evidence);
    assert_eq!(events.len(), after);
}

#[test]
#[cfg(target_os = "linux")]
fn classification_uses_type_atoms_not_product_name_substrings() {
    for atom in [b"STYLUS".as_slice(), b"stylus", b"PEN"] {
        assert!(linux::stylus_type(atom));
    }
    for atom in [
        b"MOUSE".as_slice(),
        b"TOUCHSCREEN",
        b"ERASER",
        b"Open Pen Mouse",
        b"",
    ] {
        assert!(!linux::stylus_type(atom));
    }
}

#[test]
fn diagnostics_are_opt_in_and_never_count_text_or_key_contents() {
    let ctx = egui::Context::default();
    let mut input = PenInput::default();
    let mut raw = egui::RawInput {
        events: vec![
            Event::Text("private generated fixture".into()),
            button(pos2(20.0, 20.0), true, Modifiers::NONE),
        ],
        ..Default::default()
    };
    input.raw_input(&ctx, &mut raw);
    assert_eq!(input.counts.pointer_down, 0);
    input.diagnostic = true;
    input.raw_input(&ctx, &mut raw);
    assert_eq!(input.counts.pointer_down, 1);
    assert_eq!(raw.events.len(), 2);
}

#[test]
fn focus_loss_releases_contact_without_making_a_click_or_restarting_on_hover() {
    let ctx = egui::Context::default();
    let mut input = PenInput::default();
    let mut events = Vec::new();
    input.translator.translate(
        packet(Kind::Button {
            button: 1,
            pressed: true,
        }),
        1.0,
        Modifiers::NONE,
        &mut events,
        &mut Provenance::default(),
    );
    let mut raw = egui::RawInput {
        focused: false,
        ..Default::default()
    };
    input.raw_input(&ctx, &mut raw);
    assert!(input.translator.active.is_none());
    assert!(raw.events.iter().any(|e| matches!(
        e,
        Event::Touch {
            phase: TouchPhase::Cancel,
            ..
        }
    )));
    assert!(raw
        .events
        .iter()
        .any(|e| matches!(e, Event::PointerButton { pos, pressed: false, .. } if pos.x < 0.0)));
    events.clear();
    input.translator.translate(
        packet(Kind::Move),
        1.0,
        Modifiers::NONE,
        &mut events,
        &mut Provenance::default(),
    );
    assert!(
        events.is_empty(),
        "A new explicit tip-down is required after cancellation"
    );
}

#[test]
fn pen_diagnostic_controls_are_visible_and_counter_reset_is_session_local() {
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut input = PenInput::default();
    let render = |input: &mut PenInput, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| input.diagnostic_ui(ui));
            },
        )
    };
    let visible = |output: &egui::FullOutput, label: &str| {
        output.shapes.iter().find_map(|s| {
            if let egui::Shape::Text(text) = &s.shape {
                if text.galley.job.text.contains(label) {
                    return Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center());
                }
            }
            None
        })
    };
    for _ in 0..3 {
        render(&mut input, vec![]);
    }
    let output = render(&mut input, vec![]);
    let pos = visible(&output, "Pen / touch input diagnostic").unwrap();
    for pressed in [true, false] {
        render(
            &mut input,
            vec![
                Event::PointerMoved(pos),
                button(pos, pressed, Modifiers::NONE),
            ],
        );
    }
    let output = render(&mut input, vec![]);
    let pos = visible(&output, "Count input events (session only)").unwrap();
    assert!(!input.diagnostic);
    for pressed in [true, false] {
        render(
            &mut input,
            vec![
                Event::PointerMoved(pos),
                button(pos, pressed, Modifiers::NONE),
            ],
        );
    }
    assert!(input.diagnostic);
    input.counts.pen_down = 3;
    let output = render(&mut input, vec![]);
    assert!(visible(&output, "3 tip presses").is_some());
    let pos = visible(&output, "Reset input counters").unwrap();
    for pressed in [true, false] {
        render(
            &mut input,
            vec![
                Event::PointerMoved(pos),
                button(pos, pressed, Modifiers::NONE),
            ],
        );
    }
    assert_eq!(input.counts.pen_down, 0);
}
