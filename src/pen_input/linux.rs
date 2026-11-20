use super::{Kind, Packet};
use eframe::egui::{self, Pos2};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use x11rb::{
    protocol::{
        xinput::{self, ConnectionExt as _, DeviceClassData, DeviceType},
        xproto::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;

pub(super) struct Reader {
    connection: RustConnection,
    window: u32,
    styli: BTreeMap<u16, u16>,
    refreshed: Instant,
    tracker: Tracker,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    source: u16,
    pos: Pos2,
    down: bool,
}

#[derive(Default)]
struct Tracker(Option<Sample>);

impl Tracker {
    fn update(&mut self, current: Option<Sample>) -> Vec<Packet> {
        let mut packets = Vec::with_capacity(3);
        let previous = self.0.take();
        let same_source = previous
            .zip(current)
            .is_some_and(|(a, b)| a.source == b.source);
        if previous.is_some() && !same_source {
            packets.push(Packet {
                source: 0,
                pos: Pos2::ZERO,
                supplement: true,
                kind: Kind::Cancel,
            });
        }
        if let Some(sample) = current {
            if !same_source || previous.is_some_and(|old| old.pos != sample.pos) {
                packets.push(Packet {
                    source: sample.source,
                    pos: sample.pos,
                    supplement: true,
                    kind: Kind::Move,
                });
            }
            let was_down = same_source && previous.is_some_and(|old| old.down);
            // Do not reconstruct a press held while entering/focusing the app.
            if same_source && sample.down != was_down {
                packets.push(Packet {
                    source: sample.source,
                    pos: sample.pos,
                    // This requests fallback contact, not a claim about an XI
                    // event flag. The caller first checks toolkit ownership.
                    supplement: true,
                    kind: Kind::Button {
                        button: 1,
                        pressed: sample.down,
                    },
                });
            }
        }
        self.0 = current;
        packets
    }
}

pub(super) fn stylus_type(atom: &[u8]) -> bool {
    atom.eq_ignore_ascii_case(b"STYLUS") || atom.eq_ignore_ascii_case(b"PEN")
}

/// Master-device class source IDs identify the physical device currently
/// supplying pointer state. Never infer "pen" just from cursor position/name.
fn active_stylus(info: &xinput::XIDeviceInfo, styli: &BTreeMap<u16, u16>) -> Option<u16> {
    if !info.enabled || info.type_ != DeviceType::MASTER_POINTER {
        return None;
    }
    let mut sources = info.classes.iter().filter_map(|class| match class.data {
        DeviceClassData::Button(_) | DeviceClassData::Valuator(_) => Some(class.sourceid),
        _ => None,
    });
    let source = sources.next()?;
    (sources.all(|other| other == source) && styli.get(&source) == Some(&info.deviceid))
        .then_some(source)
}

fn owned_sample(
    source: u16,
    pointer: &xinput::XIQueryPointerReply,
    root_pointer: &xinput::XIQueryPointerReply,
    top_level: u32,
    size: (u16, u16),
) -> Option<Sample> {
    let pos = egui::pos2(
        pointer.win_x as f32 / 65536.0,
        pointer.win_y as f32 / 65536.0,
    );
    // Check actual stacking/hit ownership, not just the underlying window's
    // rectangle: a foreign window may cover it without taking keyboard focus.
    (pointer.same_screen
        && root_pointer.same_screen
        && root_pointer.child == top_level
        && pos.x >= 0.0
        && pos.y >= 0.0
        && pos.x < f32::from(size.0)
        && pos.y < f32::from(size.1))
    .then(|| Sample {
        source,
        pos,
        down: pointer
            .buttons
            .first()
            .is_some_and(|bits| bits & (1 << 1) != 0),
    })
}

impl Reader {
    pub(super) fn from_context(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let window = match cc.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).map_err(|e| e.to_string())?,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Wayland(_) => return Err("Native Wayland: this pinned toolkit has no tablet-v2 pen contact handler. Restart with --pen-x11 to try X11/XWayland; finger/mouse input is unchanged.".into()),
            _ => return Err("No supported X11 window handle; pen supplement unavailable.".into()),
        };
        Self::connect(window).map_err(|e| format!("X11 pen fallback unavailable: {e}"))
    }

    fn connect(window: u32) -> anyhow::Result<Self> {
        let (connection, _) = x11rb::connect(None)?;
        connection.xinput_xi_query_version(2, 0)?.reply()?;
        // Deliberately NO XISelectEvents, XSelectInput, grab or raw/root input
        // subscription. Even selecting XI_ButtonPress on another connection
        // can acquire an implicit grab and starve the toolkit of its events.
        connection.get_window_attributes(window)?.reply()?;
        let mut reader = Self {
            connection,
            window,
            styli: BTreeMap::new(),
            refreshed: Instant::now(),
            tracker: Tracker::default(),
        };
        reader.refresh()?;
        Ok(reader)
    }

    fn refresh(&mut self) -> anyhow::Result<()> {
        let devices = self.connection.xinput_list_input_devices()?.reply()?;
        let enabled = self.connection.xinput_xi_query_device(0u16)?.reply()?.infos;
        anyhow::ensure!(
            devices.devices.len() <= 256 && enabled.len() <= 256,
            "Too many input devices"
        );
        let mut styli = BTreeMap::new();
        for device in devices.devices {
            if device.device_type == 0 {
                continue;
            }
            let atom = self
                .connection
                .get_atom_name(device.device_type)?
                .reply()?
                .name;
            if !stylus_type(&atom) {
                continue;
            }
            if let Some(info) = enabled.iter().find(|info| {
                info.deviceid == u16::from(device.device_id)
                    && info.enabled
                    && info.type_ == DeviceType::SLAVE_POINTER
            }) {
                styli.insert(info.deviceid, info.attachment);
            }
        }
        anyhow::ensure!(styli.len() <= 32, "Too many stylus devices");
        self.styli = styli;
        self.refreshed = Instant::now();
        Ok(())
    }

    pub(super) fn stylus_count(&self) -> usize {
        self.styli.len()
    }

    pub(super) fn current_contact(&self) -> Option<(u16, Pos2, bool)> {
        self.tracker
            .0
            .map(|sample| (sample.source, sample.pos, sample.down))
    }

    pub(super) fn poll(&mut self, focused: bool) -> anyhow::Result<Vec<Packet>> {
        if !focused {
            return Ok(self.tracker.update(None));
        }
        if self.refreshed.elapsed() >= Duration::from_secs(1) {
            self.refresh()?;
        }
        let sample = self.sample()?;
        Ok(self.tracker.update(sample))
    }

    fn sample(&self) -> anyhow::Result<Option<Sample>> {
        if self.styli.is_empty() {
            return Ok(None);
        }
        let geometry = self.connection.get_geometry(self.window)?.reply()?;
        let mut top_level = self.window;
        let mut found_root = false;
        for _ in 0..16 {
            let tree = self.connection.query_tree(top_level)?.reply()?;
            if tree.parent == tree.root {
                found_root = true;
                break;
            }
            top_level = tree.parent;
        }
        anyhow::ensure!(found_root, "Window ancestry exceeds its bound");
        let masters: BTreeSet<_> = self.styli.values().copied().collect();
        for master in masters {
            let devices = self
                .connection
                .xinput_xi_query_device(master)?
                .reply()?
                .infos;
            anyhow::ensure!(devices.len() <= 1, "Invalid master-device reply");
            let Some(source) = devices
                .first()
                .and_then(|info| active_stylus(info, &self.styli))
            else {
                continue;
            };
            let pointer = self
                .connection
                .xinput_xi_query_pointer(self.window, master)?
                .reply()?;
            let root_pointer = self
                .connection
                .xinput_xi_query_pointer(geometry.root, master)?
                .reply()?;
            if let Some(sample) = owned_sample(
                source,
                &pointer,
                &root_pointer,
                top_level,
                (geometry.width, geometry.height),
            ) {
                return Ok(Some(sample));
            }
        }
        Ok(None)
    }
}
