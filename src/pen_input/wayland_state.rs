//! Protocol-independent, ordered tablet-v2 contact state.
use super::{Kind, Packet, MAX_PACKETS};
use eframe::egui::Pos2;

#[derive(Clone, Copy)]
pub(super) enum ToolEvent {
    Supported(bool),
    Enter(bool),
    Motion(Pos2),
    Down,
    Up,
    Leave,
}

#[derive(Default)]
pub(super) struct Contact {
    pub supported: bool,
    pub inside: bool,
    pub down: bool,
    pub pos: Option<Pos2>,
    pub pending: Vec<Packet>,
    pub blocked: bool,
    pub overflowed: bool,
}

impl Contact {
    pub fn apply(&mut self, source: u16, event: ToolEvent) {
        let kind = match event {
            ToolEvent::Supported(value) => {
                self.supported = value;
                if value {
                    return;
                }
                self.inside = false;
                self.down = false;
                Kind::Cancel
            }
            ToolEvent::Enter(ours) => {
                self.inside = ours && self.supported;
                self.pos = None;
                self.down = false;
                self.blocked = !self.inside;
                Kind::Cancel
            }
            ToolEvent::Motion(pos) if self.inside && !self.blocked && pos.is_finite() => {
                self.pos = Some(pos);
                Kind::Move
            }
            ToolEvent::Down if self.inside && !self.blocked && !self.down && self.pos.is_some() => {
                self.down = true;
                Kind::Button {
                    button: 1,
                    pressed: true,
                }
            }
            ToolEvent::Up if self.down => {
                self.down = false;
                Kind::Button {
                    button: 1,
                    pressed: false,
                }
            }
            ToolEvent::Up => {
                self.blocked = !self.inside || !self.supported;
                return;
            }
            ToolEvent::Leave => {
                self.inside = false;
                self.down = false;
                self.pos = None;
                self.blocked = true;
                Kind::Cancel
            }
            _ => return,
        };
        if self.pending.len() >= MAX_PACKETS {
            self.overflowed = true;
            self.cancel(source);
            self.blocked = true;
            return;
        }
        self.pending.push(Packet {
            source,
            pos: self.pos.unwrap_or(Pos2::ZERO),
            supplement: true,
            kind,
        });
    }

    pub fn cancel(&mut self, source: u16) {
        self.pending.clear();
        self.pending.push(Packet {
            source,
            pos: Pos2::ZERO,
            supplement: true,
            kind: Kind::Cancel,
        });
        self.down = false;
        self.blocked = true;
    }
}
