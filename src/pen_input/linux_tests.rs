use super::*;
use x11rb::{
    connection::Connection,
    protocol::xproto::{CreateWindowAux, WindowClass},
};

#[test]
fn samples_require_actual_top_level_ownership_and_primary_contact() {
    let mut pointer = xinput::XIQueryPointerReply {
        same_screen: true,
        win_x: 20 * 65536,
        win_y: 30 * 65536,
        buttons: vec![1 << 1],
        ..Default::default()
    };
    let mut root = xinput::XIQueryPointerReply {
        same_screen: true,
        child: 100,
        ..Default::default()
    };
    assert_eq!(
        owned_sample(12, &pointer, &root, 100, (200, 400)),
        Some(Sample {
            source: 12,
            pos: egui::pos2(20.0, 30.0),
            down: true
        })
    );
    pointer.buttons = vec![1 << 3]; // Barrel/right button is not the tip.
    assert!(
        !owned_sample(12, &pointer, &root, 100, (200, 400))
            .unwrap()
            .down
    );
    root.child = 101; // Another window covers the reader.
    assert!(owned_sample(12, &pointer, &root, 100, (200, 400)).is_none());
    root.child = 100;
    pointer.win_x = -65536;
    assert!(owned_sample(12, &pointer, &root, 100, (200, 400)).is_none());
}

#[test]
fn entering_with_tip_down_requires_hover_then_a_new_press() {
    let mut tracker = Tracker::default();
    let held = Sample {
        source: 12,
        pos: Pos2::ZERO,
        down: true,
    };
    for _ in 0..2 {
        assert!(!tracker
            .update(Some(held))
            .iter()
            .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
        tracker.update(None);
    }
    tracker.update(Some(Sample {
        down: false,
        ..held
    }));
    assert!(tracker
        .update(Some(held))
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
}

#[test]
fn sampled_contact_has_one_start_and_end_and_never_starts_from_hover() {
    let mut tracker = Tracker::default();
    let hover = Sample {
        source: 12,
        pos: egui::pos2(20.0, 30.0),
        down: false,
    };
    assert!(tracker
        .update(Some(hover))
        .iter()
        .all(|p| matches!(p.kind, Kind::Move)));
    assert!(tracker.update(Some(hover)).is_empty());
    let contact = Sample {
        down: true,
        ..hover
    };
    let start = tracker.update(Some(contact));
    assert_eq!(start.len(), 1);
    assert!(matches!(
        start[0].kind,
        Kind::Button {
            button: 1,
            pressed: true
        }
    ));
    assert!(tracker.update(Some(contact)).is_empty());
    let end = tracker.update(Some(hover));
    assert_eq!(end.len(), 1);
    assert!(matches!(
        end[0].kind,
        Kind::Button {
            button: 1,
            pressed: false
        }
    ));
}

#[test]
fn loss_of_window_focus_or_source_cancels_and_requires_a_new_sample() {
    let mut tracker = Tracker::default();
    tracker.update(Some(Sample {
        source: 12,
        pos: Pos2::ZERO,
        down: true,
    }));
    assert!(matches!(tracker.update(None)[0].kind, Kind::Cancel));
    assert!(tracker.update(None).is_empty());
    tracker.update(Some(Sample {
        source: 12,
        pos: Pos2::ZERO,
        down: true,
    }));
    let changed = tracker.update(Some(Sample {
        source: 13,
        pos: Pos2::ZERO,
        down: false,
    }));
    assert!(matches!(changed[0].kind, Kind::Cancel));
    assert!(!changed
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
    assert!(changed.len() <= 3);
}

#[test]
fn only_a_recognized_active_physical_stylus_supplies_a_master_sample() {
    let mut master = xinput::XIDeviceInfo {
        deviceid: 2,
        type_: DeviceType::MASTER_POINTER,
        enabled: true,
        attachment: 3,
        name: Vec::new(),
        classes: vec![xinput::DeviceClass {
            len: 2,
            sourceid: 12,
            data: DeviceClassData::Button(xinput::DeviceClassDataButton {
                state: vec![0],
                labels: vec![0],
            }),
        }],
    };
    let styli = BTreeMap::from([(12, 2)]);
    assert_eq!(active_stylus(&master, &styli), Some(12));
    master.classes[0].sourceid = 6; // Mouse is active, even at the pen's last position.
    assert_eq!(active_stylus(&master, &styli), None);
    master.classes[0].sourceid = 12;
    master.enabled = false;
    assert_eq!(active_stylus(&master, &styli), None);
}

#[test]
#[ignore = "Requires X11/XWayland; uses only an unmapped test window and read-only queries"]
fn actual_reader_selects_no_events_and_preserves_toolkit_client_selections() {
    let (toolkit, screen) = x11rb::connect(None).unwrap();
    let window = toolkit.generate_id().unwrap();
    toolkit
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            toolkit.setup().roots[screen].root,
            0,
            0,
            100,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::default(),
        )
        .unwrap()
        .check()
        .unwrap();
    toolkit
        .xinput_xi_query_version(2, 0)
        .unwrap()
        .reply()
        .unwrap();
    let mask = xinput::XIEventMask::BUTTON_PRESS
        | xinput::XIEventMask::BUTTON_RELEASE
        | xinput::XIEventMask::MOTION;
    toolkit
        .xinput_xi_select_events(
            window,
            &[xinput::EventMask {
                deviceid: 1,
                mask: vec![mask],
            }],
        )
        .unwrap()
        .check()
        .unwrap();
    let mut reader = Reader::connect(window).unwrap();
    reader.poll(false).unwrap();
    let own = reader
        .connection
        .xinput_xi_get_selected_events(window)
        .unwrap()
        .reply()
        .unwrap();
    assert!(
        own.masks.is_empty(),
        "Supplement must never subscribe to input events"
    );
    let existing = toolkit
        .xinput_xi_get_selected_events(window)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(existing.masks.len(), 1);
    assert_eq!(u32::from(existing.masks[0].mask[0]), u32::from(mask));
    drop(reader);
    toolkit.destroy_window(window).unwrap().check().unwrap();
}
