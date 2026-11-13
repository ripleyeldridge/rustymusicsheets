//! Input-method compatibility helpers for Linux virtual keyboards.
use eframe::egui::{Context, Event, ImeEvent, Response};

#[derive(Default)]
pub struct ImeCompatibility {
    composing: bool,
}

impl ImeCompatibility {
    pub fn prepare(&mut self, ctx: &Context) {
        ctx.input_mut(|input| {
            let events = std::mem::take(&mut input.events);
            for event in events {
                match &event {
                    Event::Ime(ImeEvent::Enabled) => self.composing = true,
                    Event::Ime(ImeEvent::Preedit(_)) => {
                        if !self.composing {
                            input.events.push(Event::Ime(ImeEvent::Enabled));
                        }
                        self.composing = true;
                    }
                    Event::Ime(ImeEvent::Commit(_)) => {
                        // Some Linux OSKs send Commit without any preedit.
                        // egui needs Enabled to anchor each commit at the
                        // current cursor rather than the previous composition.
                        if !self.composing {
                            input.events.push(Event::Ime(ImeEvent::Enabled));
                        }
                        self.composing = false;
                    }
                    Event::Ime(ImeEvent::Disabled) | Event::WindowFocused(false) => {
                        self.composing = false;
                    }
                    _ => {}
                }
                input.events.push(event);
            }
        });
    }
}

pub trait TextEditFocusExt {
    fn text_edit_has_focus(&self) -> bool;
}

impl TextEditFocusExt for Response {
    fn text_edit_has_focus(&self) -> bool {
        // An external keyboard window can temporarily have OS focus while
        // this editor remains the target. Do not enable page-turn shortcuts.
        self.ctx.memory(|memory| memory.has_focus(self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, Event, Id, ImeEvent, TextEdit};

    #[test]
    fn virtual_keyboard_commit_only_events_accept_more_than_one_character() {
        let ctx = egui::Context::default();
        let id = Id::new("search");
        let mut query = String::new();
        let mut ime = ImeCompatibility::default();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(TextEdit::singleline(&mut query).id(id));
            });
        });
        for character in "Bach".chars() {
            let input = egui::RawInput {
                events: vec![
                    Event::Ime(ImeEvent::Commit(character.to_string())),
                    Event::Ime(ImeEvent::Disabled),
                ],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                ime.prepare(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(TextEdit::singleline(&mut query).id(id));
                });
            });
        }
        assert_eq!(query, "Bach");
    }

    #[test]
    fn composition_unicode_paste_and_external_keyboard_focus_keep_working() {
        let ctx = egui::Context::default();
        let id = Id::new("composed_search");
        let mut query = String::new();
        let mut ime = ImeCompatibility::default();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let batches = [
            vec![],
            vec![
                Event::Ime(ImeEvent::Enabled),
                Event::Ime(ImeEvent::Preedit("é".into())),
            ],
            vec![
                Event::Ime(ImeEvent::Commit("é".into())),
                Event::Ime(ImeEvent::Disabled),
            ],
            vec![
                Event::WindowFocused(false),
                Event::Ime(ImeEvent::Commit("t".into())),
            ],
            vec![
                Event::WindowFocused(true),
                Event::Ime(ImeEvent::Commit("u".into())),
            ],
            vec![Event::Ime(ImeEvent::Commit("de".into()))],
            vec![Event::Paste(" 音楽".into())],
        ];
        for (index, events) in batches.into_iter().enumerate() {
            let _ = ctx.run(
                egui::RawInput {
                    focused: index != 3,
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ime.prepare(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui.add(TextEdit::singleline(&mut query).id(id));
                        assert!(response.text_edit_has_focus());
                    });
                },
            );
        }
        assert_eq!(query, "étude 音楽");
    }
}
