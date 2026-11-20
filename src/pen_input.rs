//! Supplemental pen contact, not pressure or global device interception.
use eframe::egui::{self, Event, Pos2, TouchDeviceId, TouchId, TouchPhase};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(test)]
mod tests;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(any(target_os = "linux", test))]
mod wayland_state;

#[cfg(any(target_os = "linux", test))]
const MAX_PACKETS: usize = 4096;
const PEN_DEVICE: TouchDeviceId = TouchDeviceId(u64::MAX);

#[derive(Clone, Default)]
struct Provenance {
    contacts: Vec<(TouchDeviceId, TouchId)>,
    pointer_presses: Vec<Pos2>,
}

impl Drop for PenInput {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(any(target_os = "linux", test))]
fn toolkit_has_contact(ctx: &egui::Context, raw: &egui::RawInput, pos: Pos2) -> bool {
    let matches_pos = |other: Pos2| other.is_finite() && pos.distance(other) <= 1.0;
    raw.events.iter().any(|event| {
        matches!(event,
        Event::PointerButton {
            pos: other, button: egui::PointerButton::Primary, pressed: true, ..
        } if matches_pos(*other))
    }) || ctx.input(|input| {
        input.pointer.primary_down()
            && !input.any_touches()
            && input.pointer.latest_pos().is_some_and(matches_pos)
    })
}

fn append_supplement(events: &mut Vec<Event>, supplemental: Vec<Event>) {
    if supplemental.is_empty() {
        return;
    }
    // Some integration layers may also synthesize the missing button. Retain
    // their event rather than pressing twice; never broadly remove mouse input.
    let mut backend_buttons: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } = event
            {
                Some((*pos, *button, *pressed))
            } else {
                None
            }
        })
        .take(4096)
        .collect();
    for event in supplemental {
        let duplicate = if let Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } = &event
        {
            if let Some(index) = backend_buttons
                .iter()
                .position(|value| value == &(*pos, *button, *pressed))
            {
                backend_buttons.remove(index);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !duplicate {
            events.push(event);
        }
    }
}

/// Generated fixtures use exactly the production translator, never an OS device.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestPen(Translator);

#[cfg(test)]
impl TestPen {
    pub(crate) fn wayland_tap(
        &mut self,
        ctx: &egui::Context,
        start: Pos2,
        end: Pos2,
    ) -> Vec<Event> {
        use wayland_state::{Contact, ToolEvent};
        let mut contact = Contact::default();
        for event in [
            ToolEvent::Supported(true),
            ToolEvent::Enter(true),
            ToolEvent::Motion(start),
            ToolEvent::Down,
            ToolEvent::Motion(end),
            ToolEvent::Up,
        ] {
            contact.apply(1, event);
        }
        let mut events = Vec::new();
        let mut evidence = Provenance::default();
        for packet in contact.pending {
            self.0.translate(
                packet,
                ctx.zoom_factor(),
                egui::Modifiers::NONE,
                &mut events,
                &mut evidence,
            );
        }
        ctx.data_mut(|data| data.insert_temp(provenance_id(), evidence));
        events
    }
    pub(crate) fn normal_press(&mut self, ctx: &egui::Context, pos: Pos2) {
        let mut evidence = Provenance::default();
        let mut events = Vec::new();
        self.0.translate(
            Packet {
                source: 12,
                pos,
                supplement: false,
                kind: Kind::Button {
                    button: 1,
                    pressed: true,
                },
            },
            1.0,
            egui::Modifiers::NONE,
            &mut events,
            &mut evidence,
        );
        assert!(events.is_empty());
        ctx.data_mut(|data| data.insert_temp(provenance_id(), evidence));
    }
    pub(crate) fn events(
        &mut self,
        ctx: &egui::Context,
        samples: &[(Pos2, Option<bool>)],
    ) -> Vec<Event> {
        let mut events = Vec::new();
        let mut evidence = Provenance::default();
        for (pos, pressed) in samples {
            self.0.translate(
                Packet {
                    source: 12,
                    pos: *pos,
                    supplement: true,
                    kind: pressed.map_or(Kind::Move, |pressed| Kind::Button { button: 1, pressed }),
                },
                1.0,
                egui::Modifiers::NONE,
                &mut events,
                &mut evidence,
            );
        }
        if let Some((_, id)) = self.0.active {
            evidence.contacts.push((PEN_DEVICE, id));
        }
        ctx.data_mut(|data| data.insert_temp(provenance_id(), evidence));
        events
    }
}

fn provenance_id() -> egui::Id {
    egui::Id::new("rms-native-pen-input")
}

pub(crate) fn is_pen_touch(ctx: &egui::Context, device: TouchDeviceId, id: TouchId) -> bool {
    ctx.data(|data| {
        data.get_temp::<Provenance>(provenance_id())
            .is_some_and(|frame| frame.contacts.contains(&(device, id)))
    })
}

pub(crate) fn is_pen_press(ctx: &egui::Context, pos: Pos2) -> bool {
    ctx.data(|data| {
        data.get_temp::<Provenance>(provenance_id())
            .is_some_and(|frame| frame.pointer_presses.contains(&pos))
    })
}

#[derive(Clone, Copy)]
#[cfg(any(target_os = "linux", test))]
enum Kind {
    Move,
    Button { button: u32, pressed: bool },
    Cancel,
}

#[derive(Clone, Copy)]
#[cfg(any(target_os = "linux", test))]
struct Packet {
    source: u16,
    pos: Pos2,
    supplement: bool,
    kind: Kind,
}

#[derive(Default)]
struct Translator {
    active: Option<(u16, TouchId)>,
    #[cfg(any(target_os = "linux", test))]
    serial: u64,
}

impl Translator {
    fn cancel(&mut self, events: &mut Vec<Event>, evidence: &mut Provenance) {
        if let Some((_, id)) = self.active.take() {
            evidence.contacts.push((PEN_DEVICE, id));
            // Cancellation must release egui's synthetic button without clicking
            // a toolbar control or adding an off-page sample to the saved ink.
            let pos = egui::pos2(-1_000_000.0, -1_000_000.0);
            events.push(Event::Touch {
                device_id: PEN_DEVICE,
                id,
                phase: TouchPhase::Cancel,
                pos,
                force: None,
            });
            events.push(button(pos, false, egui::Modifiers::NONE));
            events.push(Event::PointerGone);
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn translate(
        &mut self,
        packet: Packet,
        scale: f32,
        modifiers: egui::Modifiers,
        events: &mut Vec<Event>,
        evidence: &mut Provenance,
    ) {
        if matches!(packet.kind, Kind::Cancel) {
            if packet.source == 0
                || self
                    .active
                    .is_some_and(|(source, _)| source == packet.source)
            {
                self.cancel(events, evidence);
            }
            return;
        }
        if !scale.is_finite() || scale <= 0.0 || !packet.pos.is_finite() {
            self.cancel(events, evidence);
            return;
        }
        let pos = packet.pos / scale;
        if !pos.is_finite() {
            self.cancel(events, evidence);
            return;
        }
        let phase = match packet.kind {
            Kind::Button { button: 1, pressed } => {
                if pressed {
                    if self.active.is_some() {
                        return;
                    }
                    // Normal pen buttons already arrive through winit. Record
                    // provenance for canvas arbitration, but do not inject them.
                    if !packet.supplement {
                        evidence.pointer_presses.push(pos);
                        return;
                    }
                    self.serial = self.serial.wrapping_add(1);
                    self.active = Some((packet.source, TouchId(self.serial)));
                    TouchPhase::Start
                } else if self
                    .active
                    .is_some_and(|(source, _)| source == packet.source)
                {
                    TouchPhase::End
                } else {
                    return;
                }
            }
            Kind::Move
                if self
                    .active
                    .is_some_and(|(source, _)| source == packet.source) =>
            {
                TouchPhase::Move
            }
            _ => return, // Hover and side buttons are never tip contact.
        };
        let (_, id) = self.active.unwrap();
        if !evidence.contacts.contains(&(PEN_DEVICE, id)) {
            evidence.contacts.push((PEN_DEVICE, id));
        }
        events.push(Event::Touch {
            device_id: PEN_DEVICE,
            id,
            phase,
            pos,
            force: None,
        });
        events.push(Event::PointerMoved(pos));
        match phase {
            TouchPhase::Start => events.push(button(pos, true, modifiers)),
            TouchPhase::End => {
                events.push(button(pos, false, modifiers));
                self.active = None;
            }
            _ => {}
        }
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

#[derive(Default)]
struct Counts {
    pointer_moves: u64,
    pointer_down: u64,
    pointer_up: u64,
    touch_start: u64,
    touch_end: u64,
    pen_moves: u64,
    pen_down: u64,
    pen_up: u64,
    supplemented: u64,
}

#[cfg(any(target_os = "linux", test))]
fn settle_contact(
    mut packet: Packet,
    current: Option<(u16, Pos2, bool)>,
    toolkit_owns: bool,
) -> Option<Packet> {
    let (source, pos, down) = current?;
    if source != packet.source || !down {
        return None;
    }
    if toolkit_owns {
        packet.pos = pos;
        packet.supplement = false;
    }
    Some(packet)
}

/// All diagnostic counters are session-only and opt-in. No text or positions
/// are displayed, persisted or logged.
pub struct PenInput {
    #[cfg(target_os = "linux")]
    native: Option<linux::Reader>,
    #[cfg(target_os = "linux")]
    wayland: Option<wayland::Reader>,
    translator: Translator,
    #[cfg(target_os = "linux")]
    pending: Option<Packet>,
    status: String,
    diagnostic: bool,
    counts: Counts,
}

impl Default for PenInput {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            native: None,
            #[cfg(target_os = "linux")]
            wayland: None,
            translator: Translator::default(),
            #[cfg(target_os = "linux")]
            pending: None,
            status: "Native pen supplement is unavailable on this backend. Mouse/touch input remains unchanged.".into(),
            diagnostic: false,
            counts: Counts::default(),
        }
    }
}

impl PenInput {
    pub(crate) fn support_summary(&self) -> &str {
        &self.status
    }
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(target_os = "linux")]
        {
            let mut input = Self::default();
            use raw_window_handle::HasWindowHandle as _;
            if matches!(
                cc.window_handle().map(|handle| handle.as_raw()),
                Ok(raw_window_handle::RawWindowHandle::Wayland(_))
            ) {
                match wayland::Reader::from_context(cc) {
                    Ok(reader) => {
                        input.status = reader.status();
                        input.wayland = Some(reader);
                    }
                    Err(error) => {
                        input.status =
                            format!("Native Wayland pen initialization failed: {error:#}")
                    }
                }
                return input;
            }
            match linux::Reader::from_context(cc) {
                Ok(reader) => {
                    input.status = "X11/XWayland: read-only, focused-window pen-state fallback. No input event subscriptions or competing button grabs.".into();
                    input.native = Some(reader);
                }
                Err(error) => input.status = error,
            }
            input
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cc;
            Self::default()
        }
    }

    pub fn raw_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if raw.viewport_id != egui::ViewportId::ROOT {
            return;
        }
        let mut evidence = Provenance::default();
        let mut supplemental = Vec::new();
        let focused = raw.focused
            && !raw
                .events
                .iter()
                .any(|event| matches!(event, Event::WindowFocused(false)));
        #[cfg(target_os = "linux")]
        let pending = self.pending.take();
        #[cfg(target_os = "linux")]
        if let Some(reader) = &mut self.wayland {
            match reader.poll(focused, self.diagnostic) {
                Ok(packets) => {
                    self.status = reader.status();
                    for packet in packets {
                        if self.diagnostic {
                            match packet.kind {
                                Kind::Move => self.counts.pen_moves += 1,
                                Kind::Button {
                                    button: 1,
                                    pressed: true,
                                } => self.counts.pen_down += 1,
                                Kind::Button {
                                    button: 1,
                                    pressed: false,
                                } => self.counts.pen_up += 1,
                                _ => {}
                            }
                        }
                        if focused {
                            if matches!(packet.kind, Kind::Move) && self.translator.active.is_none()
                            {
                                supplemental
                                    .push(Event::PointerMoved(packet.pos / ctx.zoom_factor()));
                            }
                            // Surface-local Wayland coordinates are already logical.
                            // Contact is explicit: do not apply X11 polling delay.
                            self.translator.translate(
                                packet,
                                ctx.zoom_factor(),
                                raw.modifiers,
                                &mut supplemental,
                                &mut evidence,
                            );
                        }
                    }
                }
                Err(error) => {
                    self.status =
                        format!("Native Wayland pen stopped: {error:#}. Restart to retry.");
                    self.translator.cancel(&mut supplemental, &mut evidence);
                    self.wayland = None;
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        #[cfg(target_os = "linux")]
        if let Some(reader) = &mut self.native {
            match reader.poll(focused) {
                Ok(packets) if packets.len() < MAX_PACKETS => {
                    let scale = raw
                        .viewports
                        .get(&raw.viewport_id)
                        .and_then(|v| v.native_pixels_per_point)
                        .unwrap_or(1.0)
                        * ctx.zoom_factor();
                    // Queries can observe a tip before winit dispatches its
                    // queued press. Give toolkit delivery one frame to catch up.
                    let current = reader.current_contact();
                    if focused {
                        if let Some(packet) = pending.and_then(|packet| {
                            settle_contact(
                                packet,
                                current,
                                current.is_some_and(|(_, pos, _)| {
                                    toolkit_has_contact(ctx, raw, pos / scale)
                                }),
                            )
                        }) {
                            self.translator.translate(
                                packet,
                                scale,
                                raw.modifiers,
                                &mut supplemental,
                                &mut evidence,
                            );
                        }
                    }
                    for mut packet in packets {
                        if self.diagnostic {
                            match packet.kind {
                                Kind::Move => self.counts.pen_moves += 1,
                                Kind::Button {
                                    button: 1,
                                    pressed: true,
                                } => self.counts.pen_down += 1,
                                Kind::Button {
                                    button: 1,
                                    pressed: false,
                                } => self.counts.pen_up += 1,
                                _ => {}
                            }
                        }
                        if focused {
                            if matches!(
                                packet.kind,
                                Kind::Button {
                                    button: 1,
                                    pressed: true
                                }
                            ) {
                                if toolkit_has_contact(ctx, raw, packet.pos / scale) {
                                    packet.supplement = false;
                                } else if self.translator.active.is_none() {
                                    self.pending = Some(packet);
                                    continue;
                                }
                            }
                            if matches!(
                                packet.kind,
                                Kind::Cancel | Kind::Button { pressed: false, .. }
                            ) {
                                self.pending = None;
                            }
                            self.translator.translate(
                                packet,
                                scale,
                                raw.modifiers,
                                &mut supplemental,
                                &mut evidence,
                            );
                        }
                    }
                }
                result => {
                    self.status = match result {
                        Err(error) => format!("Pen supplement stopped: {error}. Restart the app to retry; mouse/touch remain available."),
                        _ => "Pen sample budget exceeded; fallback stopped safely. Restart the app to retry.".into(),
                    };
                    self.translator.cancel(&mut supplemental, &mut evidence);
                    self.native = None;
                }
            }
        }
        if !focused {
            self.translator.cancel(&mut supplemental, &mut evidence);
        }
        if let Some((_, id)) = self.translator.active {
            evidence.contacts.push((PEN_DEVICE, id));
        }
        if self.diagnostic {
            for event in &raw.events {
                match event {
                    Event::PointerMoved(_) => self.counts.pointer_moves += 1,
                    Event::PointerButton { pressed: true, .. } => self.counts.pointer_down += 1,
                    Event::PointerButton { pressed: false, .. } => self.counts.pointer_up += 1,
                    Event::Touch {
                        phase: TouchPhase::Start,
                        ..
                    } => self.counts.touch_start += 1,
                    Event::Touch {
                        phase: TouchPhase::End | TouchPhase::Cancel,
                        ..
                    } => self.counts.touch_end += 1,
                    _ => {}
                }
            }
            self.counts.supplemented += supplemental
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::Touch {
                            phase: TouchPhase::Start,
                            ..
                        }
                    )
                })
                .count() as u64;
        }
        append_supplement(&mut raw.events, supplemental);
        ctx.data_mut(|data| data.insert_temp(provenance_id(), evidence));
        #[cfg(target_os = "linux")]
        if let Some(reader) = &self.native {
            ctx.request_repaint_after(if reader.stylus_count() > 0 {
                std::time::Duration::from_millis(16)
            } else {
                std::time::Duration::from_secs(1)
            });
        }
    }

    pub fn shutdown(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.wayland.take();
            self.native.take();
            self.pending = None;
        }
    }

    pub fn paint_cursor(&self, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        if let Some(pos) = self.wayland.as_ref().and_then(|reader| reader.hover()) {
            let pos = pos / ctx.zoom_factor();
            let painter = ctx
                .layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("native-pen-hover"),
                ))
                .with_clip_rect(ctx.screen_rect());
            for (width, color) in [(3.0, egui::Color32::BLACK), (1.0, egui::Color32::WHITE)] {
                painter.line_segment(
                    [pos - egui::vec2(6.0, 0.0), pos + egui::vec2(6.0, 0.0)],
                    (width, color),
                );
                painter.line_segment(
                    [pos - egui::vec2(0.0, 6.0), pos + egui::vec2(0.0, 6.0)],
                    (width, color),
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
    }

    pub fn diagnostic_ui(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Pen / touch input diagnostic", |ui| {
            ui.label(&self.status);
            #[cfg(target_os = "linux")]
            if let Some(reader) = &self.native {
                ui.label(format!("Recognized stylus devices: {}", reader.stylus_count()));
                ui.label(match reader.current_contact() {
                    Some((_, _, true)) => "Sampled native tip: down",
                    Some((_, _, false)) => "Sampled native tip: up / hovering",
                    None => "Sampled native tip: not over this focused window",
                });
            }
            #[cfg(target_os = "linux")]
            ui.label("Use the Wayland Pen launcher / --pen-wayland for native tablet-v2; --pen-x11 explicitly selects the separate X11 fallback. No global mappings are changed.");
            #[cfg(not(target_os = "linux"))]
            ui.label("This build uses the toolkit's pointer/touch input. The native X11 pen supplement is Linux-only.");
            if ui.checkbox(&mut self.diagnostic, "Count input events (session only)").changed() {
                self.counts = Counts::default();
                #[cfg(target_os = "linux")]
                if let Some(reader) = &mut self.wayland { reader.reset_stats(); }
            }
            if self.diagnostic {
                #[cfg(target_os = "linux")]
                if let Some(reader) = &self.wayland {
                    let stats = reader.stats();
                    ui.label(format!("Wayland tablet-v2: {} frames · {} down · {} up · {} cancel/out", stats.frames, stats.down, stats.up, stats.cancel));
                }
                ui.label(format!(
                    "Backend pointer: {} moves · {} presses · {} releases\nBackend touch: {} starts · {} ends/cancels\nNative stylus samples: {} moves · {} tip presses · {} tip releases\nSupplemented pen contacts: {}",
                    self.counts.pointer_moves, self.counts.pointer_down, self.counts.pointer_up,
                    self.counts.touch_start, self.counts.touch_end, self.counts.pen_moves,
                    self.counts.pen_down, self.counts.pen_up, self.counts.supplemented,
                ));
                if ui.button("Reset input counters").clicked() {
                    self.counts = Counts::default();
                    #[cfg(target_os = "linux")]
                    if let Some(reader) = &mut self.wayland { reader.reset_stats(); }
                }
            }
            ui.small("Native Wayland uses tablet-v2 down/up frames; the separate X11 fallback samples state and can miss very short taps. Ordinary toolkit events are never intercepted.");
            ui.small("Hover must not draw. If movement increases but tip presses stay at zero, contact is not reaching this input path. Counters exclude text, keys, score contents and global input. Physical pen behavior still needs testing.");
        });
    }
}
