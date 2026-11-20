//! Guest queue on eframe's existing display. Never reads the Wayland socket.
use super::{
    wayland_state::{Contact, ToolEvent},
    Kind, Packet, MAX_PACKETS,
};
use anyhow::{ensure, Result};
use eframe::egui::Pos2;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::collections::BTreeMap;
use wayland_client::{
    backend::ObjectId,
    delegate_noop, event_created_child,
    protocol::{wl_callback, wl_registry, wl_seat, wl_surface},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::{
        wp_cursor_shape_device_v1 as shape_device, wp_cursor_shape_manager_v1 as shape_manager,
    },
    tablet::zv2::client::{
        zwp_tablet_manager_v2 as manager, zwp_tablet_pad_dial_v2 as dial,
        zwp_tablet_pad_group_v2 as group, zwp_tablet_pad_ring_v2 as ring,
        zwp_tablet_pad_strip_v2 as strip, zwp_tablet_pad_v2 as pad,
        zwp_tablet_seat_v2 as tablet_seat, zwp_tablet_tool_v2 as tool, zwp_tablet_v2 as tablet,
    },
};

const MAX_OBJECTS: usize = 32;

#[cfg(test)]
#[path = "wayland_fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "wayland_tests.rs"]
mod tests;

struct Tool {
    proxy: tool::ZwpTabletToolV2,
    cursor: Option<shape_device::WpCursorShapeDeviceV1>,
    contact: Contact,
    source: u16,
}

enum Ignored {
    Pad(pad::ZwpTabletPadV2),
    Group(group::ZwpTabletPadGroupV2),
    Ring(ring::ZwpTabletPadRingV2),
    Strip(strip::ZwpTabletPadStripV2),
    Dial(dial::ZwpTabletPadDialV2),
}

impl Ignored {
    fn destroy(self) {
        match self {
            Self::Pad(p) => p.destroy(),
            Self::Group(p) => p.destroy(),
            Self::Ring(p) => p.destroy(),
            Self::Strip(p) => p.destroy(),
            Self::Dial(p) => p.destroy(),
        }
    }
}

#[derive(Default)]
pub(super) struct Stats {
    pub frames: u64,
    pub down: u64,
    pub up: u64,
    pub cancel: u64,
}

struct State {
    target: u32,
    registry: Option<wl_registry::WlRegistry>,
    registry_done: bool,
    manager: Option<(u32, manager::ZwpTabletManagerV2)>,
    cursor_manager: Option<(u32, shape_manager::WpCursorShapeManagerV1)>,
    seats: BTreeMap<u32, (wl_seat::WlSeat, Option<tablet_seat::ZwpTabletSeatV2>)>,
    tablets: BTreeMap<u32, tablet::ZwpTabletV2>,
    tools: BTreeMap<u32, Tool>,
    ignored: Vec<Ignored>,
    packets: Vec<Packet>,
    error: Option<String>,
    count: bool,
    stats: Stats,
    focused: bool,
}

impl State {
    fn new(target: u32) -> Self {
        Self {
            target,
            registry: None,
            registry_done: false,
            manager: None,
            cursor_manager: None,
            seats: BTreeMap::new(),
            tablets: BTreeMap::new(),
            tools: BTreeMap::new(),
            ignored: Vec::new(),
            packets: Vec::new(),
            error: None,
            count: false,
            stats: Stats::default(),
            focused: true,
        }
    }

    fn fail(&mut self, message: &str) {
        if self.error.is_none() {
            self.error = Some(message.into());
            self.packets.clear();
            self.packets.push(Packet {
                source: 0,
                pos: Pos2::ZERO,
                supplement: true,
                kind: Kind::Cancel,
            });
        }
    }

    fn bind_seats(&mut self, qh: &QueueHandle<Self>) {
        if let Some((_, manager)) = &self.manager {
            for (seat, tablet) in self.seats.values_mut() {
                if tablet.is_none() {
                    *tablet = Some(manager.get_tablet_seat(seat, qh, ()));
                }
            }
        }
        if let Some((_, manager)) = &self.cursor_manager {
            for tool in self.tools.values_mut() {
                if tool.cursor.is_none() {
                    tool.cursor = Some(manager.get_tablet_tool_v2(&tool.proxy, qh, ()));
                }
            }
        }
    }

    fn ignored(&mut self, object: Ignored) {
        if self.ignored.len() >= MAX_OBJECTS * 4 {
            object.destroy();
            self.fail("Tablet pad object limit exceeded");
        } else {
            self.ignored.push(object);
        }
    }

    fn shutdown(&mut self) {
        for (_, tool) in std::mem::take(&mut self.tools) {
            if let Some(cursor) = tool.cursor {
                cursor.destroy();
            }
            tool.proxy.destroy();
        }
        for object in std::mem::take(&mut self.ignored).into_iter().rev() {
            object.destroy();
        }
        for (_, tablet) in std::mem::take(&mut self.tablets) {
            tablet.destroy();
        }
        for (_, (seat, tablet)) in std::mem::take(&mut self.seats) {
            if let Some(tablet) = tablet {
                tablet.destroy();
            }
            seat.release();
        }
        if let Some((_, manager)) = self.manager.take() {
            manager.destroy();
        }
        if let Some((_, manager)) = self.cursor_manager.take() {
            manager.destroy();
        }
        // wl_registry has no destructor request. The guest backend frees its
        // owned local proxy; never destroy or manage the borrowed wl_surface.
        self.registry.take();
        self.packets.clear();
    }
}

pub(super) struct Reader {
    queue: EventQueue<State>,
    state: State,
    connection: Connection,
}

impl Reader {
    pub(super) fn from_context(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let RawDisplayHandle::Wayland(display) = cc.display_handle()?.as_raw() else {
            anyhow::bail!("Not a native Wayland display");
        };
        let RawWindowHandle::Wayland(window) = cc.window_handle()?.as_raw() else {
            anyhow::bail!("Not a native Wayland surface");
        };
        // SAFETY: eframe 0.31.1/Glow calls App::on_exit before destroying its
        // glutin/window state and drops App before that state. MusicApp explicitly
        // drops this guest in on_exit and Drop. No worker retains backend clones.
        // The guest neither closes the display nor takes ownership of its surface.
        let backend = unsafe {
            wayland_backend::sys::client::Backend::from_foreign_display(
                display.display.as_ptr().cast(),
            )
        };
        // SAFETY: CreationContext supplies a live root wl_surface for this
        // lookup. Keep only its numeric identity on this connection, never a
        // borrowed surface proxy/pointer past initialization.
        let target = unsafe {
            ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                window.surface.as_ptr().cast(),
            )?
        };
        Self::from_guest(Connection::from_backend(backend), target)
    }

    fn from_guest(connection: Connection, target: ObjectId) -> Result<Self> {
        let queue = connection.new_event_queue::<State>();
        let qh = queue.handle();
        let mut state = State::new(target.protocol_id());
        state.registry = Some(connection.display().get_registry(&qh, ()));
        // Async sentinel only: unlike registry_queue_init/roundtrip this never
        // reads the shared socket or recursively dispatches toolkit events.
        connection.display().sync(&qh, ());
        connection.flush()?;
        Ok(Self {
            queue,
            state,
            connection,
        })
    }

    pub(super) fn poll(&mut self, focused: bool, count: bool) -> Result<Vec<Packet>> {
        self.state.count = count;
        self.state.focused = focused;
        if !focused {
            for tool in self.state.tools.values_mut() {
                if tool.contact.down {
                    tool.contact.cancel(tool.source);
                    self.state.packets.append(&mut tool.contact.pending);
                }
            }
        }
        // winit alone reads the socket. dispatch_pending also dispatches this
        // guest's private libwayland queue, never winit's queue.
        self.queue.dispatch_pending(&mut self.state)?;
        self.connection.flush()?;
        if let Some(error) = &self.state.error {
            anyhow::bail!("{error}");
        }
        ensure!(
            self.state.packets.len() < MAX_PACKETS,
            "Wayland pen sample budget exceeded"
        );
        Ok(std::mem::take(&mut self.state.packets))
    }

    pub(super) fn status(&self) -> String {
        if !self.state.registry_done {
            return "Native Wayland: waiting for registry / tablet-v2 initialization".into();
        }
        if self.state.manager.is_none() {
            return "Native Wayland: compositor does not advertise tablet-v2".into();
        }
        format!(
            "Native Wayland: tablet-v2 bound · {} seat(s) · {} tablet(s) · {} tool(s) · own-surface proximity: {} · tip: {}",
            self.state.seats.len(), self.state.tablets.len(), self.state.tools.len(),
            self.state.tools.values().any(|t| t.contact.inside),
            if self.state.tools.values().any(|t| t.contact.down) { "down" } else { "up" },
        )
    }

    pub(super) fn hover(&self) -> Option<Pos2> {
        self.state.tools.values().find_map(|tool| {
            (tool.contact.inside && tool.contact.supported)
                .then_some(tool.contact.pos)
                .flatten()
        })
    }

    pub(super) fn stats(&self) -> &Stats {
        &self.state.stats
    }
    pub(super) fn reset_stats(&mut self) {
        self.state.stats = Stats::default();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.state.shutdown();
        let _ = self.connection.flush();
        // Queue/state/connection drop while eframe's display is still live.
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "zwp_tablet_manager_v2" if state.manager.is_none() => {
                        state.manager = Some((name, registry.bind(name, 1, qh, ())));
                    }
                    "wp_cursor_shape_manager_v1" if state.cursor_manager.is_none() => {
                        state.cursor_manager = Some((name, registry.bind(name, 1, qh, ())));
                    }
                    "wl_seat" if version >= 5 && state.seats.len() < MAX_OBJECTS => {
                        state
                            .seats
                            .insert(name, (registry.bind(name, 5, qh, ()), None));
                    }
                    _ => {}
                }
                state.bind_seats(qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if state.seats.contains_key(&name)
                    || state.manager.as_ref().is_some_and(|(id, _)| *id == name)
                {
                    state.fail(
                        "Wayland tablet seat/manager removed; restart to reinitialize safely",
                    );
                }
                if state
                    .cursor_manager
                    .as_ref()
                    .is_some_and(|(id, _)| *id == name)
                {
                    if let Some((_, manager)) = state.cursor_manager.take() {
                        manager.destroy();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.registry_done = true;
    }
}

impl Dispatch<tablet_seat::ZwpTabletSeatV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &tablet_seat::ZwpTabletSeatV2,
        event: tablet_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            tablet_seat::Event::TabletAdded { id } => {
                if state.tablets.len() >= MAX_OBJECTS {
                    id.destroy();
                    state.fail("Too many tablets");
                } else {
                    state.tablets.insert(id.id().protocol_id(), id);
                }
            }
            tablet_seat::Event::ToolAdded { id } => {
                if state.tools.len() >= MAX_OBJECTS {
                    id.destroy();
                    state.fail("Too many pen tools");
                    return;
                }
                let source = (1..=MAX_OBJECTS as u16)
                    .find(|source| state.tools.values().all(|tool| tool.source != *source))
                    .unwrap();
                state.tools.insert(
                    id.id().protocol_id(),
                    Tool {
                        proxy: id,
                        cursor: None,
                        contact: Contact::default(),
                        source,
                    },
                );
                state.bind_seats(qh);
            }
            tablet_seat::Event::PadAdded { id } => state.ignored(Ignored::Pad(id)),
            _ => {}
        }
    }
    event_created_child!(State, tablet_seat::ZwpTabletSeatV2, [
        0 => (tablet::ZwpTabletV2, ()), 1 => (tool::ZwpTabletToolV2, ()), 2 => (pad::ZwpTabletPadV2, ())
    ]);
}

impl Dispatch<tool::ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &tool::ZwpTabletToolV2,
        event: tool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        if matches!(event, tool::Event::Removed) {
            if let Some(tool) = state.tools.remove(&key) {
                state.packets.push(Packet {
                    source: tool.source,
                    pos: Pos2::ZERO,
                    supplement: true,
                    kind: Kind::Cancel,
                });
                if let Some(cursor) = tool.cursor {
                    cursor.destroy();
                }
                tool.proxy.destroy();
            }
            return;
        }
        let Some(tool) = state.tools.get_mut(&key) else {
            return;
        };
        let action = match event {
            tool::Event::Type { tool_type } => Some(ToolEvent::Supported(matches!(
                tool_type,
                WEnum::Value(
                    tool::Type::Pen | tool::Type::Pencil | tool::Type::Brush | tool::Type::Airbrush
                )
            ))),
            tool::Event::ProximityIn {
                serial, surface, ..
            } => {
                let ours = surface.id().protocol_id() == state.target;
                if ours && tool.contact.supported {
                    if let Some(cursor) = &tool.cursor {
                        cursor.set_shape(serial, shape_device::Shape::Crosshair);
                    } else {
                        proxy.set_cursor(serial, None, 0, 0);
                    }
                }
                Some(ToolEvent::Enter(ours))
            }
            tool::Event::Motion { x, y } => Some(ToolEvent::Motion(egui_pos(x, y))),
            tool::Event::Down { .. } if state.focused => {
                if state.count {
                    state.stats.down += 1;
                }
                Some(ToolEvent::Down)
            }
            tool::Event::Up => {
                if state.count {
                    state.stats.up += 1;
                }
                Some(ToolEvent::Up)
            }
            tool::Event::ProximityOut => {
                if state.count {
                    state.stats.cancel += 1;
                }
                Some(ToolEvent::Leave)
            }
            tool::Event::Frame { .. } => {
                if state.count {
                    state.stats.frames += 1;
                }
                if state.packets.len() + tool.contact.pending.len() >= MAX_PACKETS {
                    state.fail("Wayland pen frame budget exceeded");
                } else {
                    state.packets.append(&mut tool.contact.pending);
                }
                return;
            }
            _ => None, // Pressure, serial numbers, tilt and side buttons are ignored.
        };
        if let Some(action) = action {
            tool.contact.apply(tool.source, action);
            if tool.contact.overflowed {
                state.fail("Wayland tool input limit exceeded; contact cancelled");
            }
        }
    }
}

fn egui_pos(x: f64, y: f64) -> Pos2 {
    Pos2::new(x as f32, y as f32)
}

impl Dispatch<tablet::ZwpTabletV2, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &tablet::ZwpTabletV2,
        event: tablet::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, tablet::Event::Removed) {
            state.tablets.remove(&proxy.id().protocol_id());
            proxy.destroy();
            for tool in state.tools.values_mut() {
                tool.contact.cancel(tool.source);
                state.packets.append(&mut tool.contact.pending);
            }
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore manager::ZwpTabletManagerV2);
delegate_noop!(State: ignore shape_manager::WpCursorShapeManagerV1);
delegate_noop!(State: ignore shape_device::WpCursorShapeDeviceV1);

impl Dispatch<pad::ZwpTabletPadV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &pad::ZwpTabletPadV2,
        event: pad::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let pad::Event::Group { pad_group } = event {
            state.ignored(Ignored::Group(pad_group));
        }
    }
    event_created_child!(State, pad::ZwpTabletPadV2, [0 => (group::ZwpTabletPadGroupV2, ())]);
}
impl Dispatch<group::ZwpTabletPadGroupV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &group::ZwpTabletPadGroupV2,
        event: group::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            group::Event::Ring { ring } => state.ignored(Ignored::Ring(ring)),
            group::Event::Strip { strip } => state.ignored(Ignored::Strip(strip)),
            group::Event::Dial { dial } => state.ignored(Ignored::Dial(dial)),
            _ => {}
        }
    }
    event_created_child!(State, group::ZwpTabletPadGroupV2, [
        1 => (ring::ZwpTabletPadRingV2, ()), 2 => (strip::ZwpTabletPadStripV2, ()), 6 => (dial::ZwpTabletPadDialV2, ())
    ]);
}
delegate_noop!(State: ignore ring::ZwpTabletPadRingV2);
delegate_noop!(State: ignore strip::ZwpTabletPadStripV2);
delegate_noop!(State: ignore dial::ZwpTabletPadDialV2);
