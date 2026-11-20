use super::*;

#[test]
fn native_tap_retains_down_and_up_within_one_frame() {
    let mut contact = Contact::default();
    for event in [
        ToolEvent::Supported(true),
        ToolEvent::Enter(true),
        ToolEvent::Motion(Pos2::new(40.0, 50.0)),
        ToolEvent::Down,
        ToolEvent::Up,
    ] {
        contact.apply(1, event);
    }
    let buttons: Vec<_> = contact
        .pending
        .iter()
        .filter_map(|packet| match packet.kind {
            Kind::Button { pressed, .. } => Some(pressed),
            _ => None,
        })
        .collect();
    assert_eq!(buttons, [true, false]);
    assert!(!contact.down);
}

#[test]
fn foreign_surface_and_focus_cancel_require_new_contact() {
    let mut contact = Contact::default();
    for event in [
        ToolEvent::Supported(true),
        ToolEvent::Enter(false),
        ToolEvent::Motion(Pos2::new(40.0, 50.0)),
        ToolEvent::Down,
    ] {
        contact.apply(1, event);
    }
    assert!(!contact.down);
    assert!(!contact
        .pending
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
    contact.apply(1, ToolEvent::Enter(true));
    contact.apply(1, ToolEvent::Motion(Pos2::new(40.0, 50.0)));
    contact.apply(1, ToolEvent::Down);
    assert!(contact.down);
    contact.cancel(1);
    contact.apply(1, ToolEvent::Down);
    assert!(!contact.down);
    contact.apply(1, ToolEvent::Up);
    contact.apply(1, ToolEvent::Down);
    assert!(contact.down);
}

#[test]
fn native_contact_is_bounded_and_invalid_positions_never_enter_ink() {
    let mut contact = Contact::default();
    contact.apply(1, ToolEvent::Supported(true));
    contact.apply(1, ToolEvent::Enter(true));
    contact.apply(1, ToolEvent::Motion(Pos2::new(f32::NAN, 0.0)));
    contact.apply(1, ToolEvent::Down);
    assert!(!contact.down);
    for _ in 0..MAX_PACKETS + 2 {
        contact.apply(1, ToolEvent::Motion(Pos2::new(40.0, 50.0)));
    }
    assert!(contact.blocked);
    assert!(contact.pending.len() <= MAX_PACKETS);
    assert!(contact.pending.iter().all(|packet| packet.pos.is_finite()));
}
