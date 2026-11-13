use crate::{
    display_names::DisplayNames,
    model::{Group, Score},
    storage::Library,
};
use eframe::egui::{self, Color32, Id};

#[derive(Default)]
pub struct SetlistManager {
    selected: Option<i64>,
    name: String,
    collection: bool,
    query: String,
    confirm_delete: bool,
}

pub struct Outcome {
    pub open: bool,
    pub changed: bool,
    pub selected: Option<i64>,
    pub play: Option<(i64, usize)>,
    pub error: Option<String>,
}

enum Action {
    Create,
    Rename(i64),
    Duplicate(i64, String),
    Delete(i64),
    Add(i64, i64),
    Remove(i64),
    Move(i64, usize, isize),
    Play(i64, usize),
}

impl SetlistManager {
    pub fn new(groups: &[Group], selected: Option<i64>) -> Self {
        let mut manager = Self::default();
        let selected = groups
            .iter()
            .find(|g| Some(g.id) == selected)
            .or_else(|| groups.iter().find(|g| g.kind == "setlist"))
            .or_else(|| groups.first());
        manager.select(selected);
        manager
    }

    fn select(&mut self, group: Option<&Group>) {
        self.selected = group.map(|g| g.id);
        self.name = group.map_or_else(String::new, |g| g.name.clone());
        self.collection = group.is_some_and(|g| g.kind == "collection");
        self.confirm_delete = false;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        db: &mut Library,
        scores: &[Score],
        display: &DisplayNames,
    ) -> Outcome {
        let mut outcome = Outcome {
            open: true,
            changed: false,
            selected: self.selected,
            play: None,
            error: None,
        };
        let groups = match db.groups() {
            Ok(groups) => groups,
            Err(error) => {
                outcome.error = Some(format!("{error:#}"));
                return outcome;
            }
        };
        if self.selected.is_some() && !groups.iter().any(|g| Some(g.id) == self.selected) {
            self.select(None);
        }
        let mut action = None;
        let mut open = true;
        let height = (ctx.screen_rect().height() - 300.0).clamp(120.0, 360.0);
        egui::Window::new("Manage setlists and collections")
            .id(Id::new("setlist_manager")).open(&mut open)
            .default_width(900.0).min_width(300.0)
            .max_width((ctx.screen_rect().width() - 24.0).max(180.0))
            .max_height((ctx.screen_rect().height() - 48.0).max(96.0))
            .vscroll(true).resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Existing list:");
                    let mut selected = self.selected;
                    let label = groups.iter().find(|g| Some(g.id) == selected)
                        .map_or("New list", |g| g.name.as_str());
                    egui::ComboBox::from_id_salt("manager_selected_group")
                        .selected_text(label).width(250.0).show_ui(ui, |ui| {
                            for group in &groups {
                                ui.selectable_value(&mut selected, Some(group.id),
                                    format!("{}: {}", group.kind, group.name));
                            }
                        });
                    if selected != self.selected {
                        self.select(groups.iter().find(|g| Some(g.id) == selected));
                    }
                    if ui.button("New list…").clicked() { self.select(None); }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Name:");
                    ui.add(egui::TextEdit::singleline(&mut self.name)
                        .id(Id::new("manager_group_name")).desired_width(280.0));
                    if let Some(id) = self.selected {
                        if ui.button("Rename").clicked() { action = Some(Action::Rename(id)); }
                        if ui.button("Duplicate").clicked() {
                            if let Some(group) = groups.iter().find(|g| g.id == id) {
                                action = Some(Action::Duplicate(id, copy_name(&groups, group)));
                            }
                        }
                        if ui.button("Delete list…").clicked() { self.confirm_delete = true; }
                    } else {
                        ui.checkbox(&mut self.collection, "Collection (otherwise setlist)");
                        if ui.button("Create list").clicked() { action = Some(Action::Create); }
                    }
                });
                if self.confirm_delete {
                    ui.group(|ui| {
                        ui.colored_label(Color32::LIGHT_RED, "Delete this list? Scores and PDFs will be kept.");
                        ui.horizontal(|ui| {
                            if ui.button("Confirm delete list").clicked() {
                                if let Some(id) = self.selected { action = Some(Action::Delete(id)); }
                            }
                            if ui.button("Cancel deletion").clicked() { self.confirm_delete = false; }
                        });
                    });
                }
                let Some(group) = self.selected else {
                    ui.label("Name your new list and click Create list, or select an existing list above.");
                    return;
                };
                let entries = match db.entries(group) {
                    Ok(entries) => entries,
                    Err(error) => { outcome.error = Some(format!("{error:#}")); return; }
                };
                ui.horizontal(|ui| {
                    if ui.add_enabled(!entries.is_empty(), egui::Button::new(
                        if self.collection { "Open first score" } else { "Play setlist" }
                    )).clicked() { action = Some(Action::Play(group, 0)); }
                    ui.label(format!("{} entries", entries.len()));
                });
                ui.label("Score/order changes save immediately. Use Rename to save a new name; Play uses the current order.");
                ui.separator();
                let stacked = ui.available_width() < 650.0;
                ui.columns(if stacked { 1 } else { 2 }, |columns| {
                    columns[0].heading(if self.collection { "Collection members" } else { "Setlist order" });
                    egui::ScrollArea::vertical().id_salt("manager_entries")
                        .max_height(height).show(&mut columns[0], |ui| {
                            if entries.is_empty() { ui.label("Add scores from the library on the right."); }
                            for (index, entry) in entries.iter().enumerate() {
                                ui.push_id(entry.id, |ui| {
                                    let title = scores.iter().find(|s| s.id == entry.score_id)
                                        .map(|s| display.format(&s.title))
                                        .unwrap_or_else(|| "Missing score".into());
                                    ui.group(|ui| {
                                        ui.add(egui::Label::new(format!("{}. {}", index + 1, title)).truncate());
                                        ui.horizontal_wrapped(|ui| {
                                            if !self.collection {
                                                if ui.add_enabled(index > 0, egui::Button::new("Up")).clicked() {
                                                    action = Some(Action::Move(group, index, -1));
                                                }
                                                if ui.add_enabled(index + 1 < entries.len(), egui::Button::new("Down")).clicked() {
                                                    action = Some(Action::Move(group, index, 1));
                                                }
                                            }
                                            if ui.button("Remove").clicked() {
                                                action = Some(Action::Remove(entry.id));
                                            }
                                            if ui.button("Play here").clicked() {
                                                action = Some(Action::Play(group, index));
                                            }
                                        });
                                    });
                                });
                            }
                        });
                    let library_column = if stacked { 0 } else { 1 };
                    if stacked { columns[0].separator(); }
                    columns[library_column].heading("Add scores from library");
                    columns[library_column].add(egui::TextEdit::singleline(&mut self.query)
                        .id(Id::new("manager_score_search")).hint_text("Search scores…"));
                    columns[library_column].label(if self.collection {
                        "Collections keep one copy of each score."
                    } else { "Add a score again to repeat it in this setlist." });
                    let mut available: Vec<_> = scores.iter()
                        .filter(|s| s.matches_display(&self.query, &display.format(&s.title))).collect();
                    available.sort_by_cached_key(|s| (display.format(&s.title).to_lowercase(), s.id));
                    egui::ScrollArea::vertical().id_salt("manager_available")
                        .max_height(height).show_rows(&mut columns[library_column], 40.0, available.len(), |ui, range| {
                            for score in &available[range] {
                                ui.push_id(score.id, |ui| {
                                    ui.horizontal(|ui| {
                                        if ui.button("+ Add").clicked() {
                                            action = Some(Action::Add(group, score.id));
                                        }
                                        ui.add(egui::Label::new(display.format(&score.title)).truncate())
                                            .on_hover_text(&score.title);
                                    });
                                });
                            }
                        });
                });
            });
        if let Some(action) = action {
            if let Action::Play(group, index) = action {
                outcome.play = Some((group, index));
                open = false;
            } else {
                let deleted = matches!(action, Action::Delete(_));
                let result = match action {
                    Action::Create => db
                        .create_group(
                            &self.name,
                            if self.collection {
                                "collection"
                            } else {
                                "setlist"
                            },
                        )
                        .map(Some),
                    Action::Rename(id) => db.rename_group(id, &self.name).map(|_| None),
                    Action::Duplicate(id, name) => db.duplicate_group(id, &name).map(Some),
                    Action::Delete(id) => db.delete_group(id).map(|_| None),
                    Action::Add(group, score) => db.add_entry(group, score).map(|_| None),
                    Action::Remove(entry) => db.remove_entry(entry).map(|_| None),
                    Action::Move(group, index, offset) => {
                        db.move_entry(group, index, offset).map(|_| None)
                    }
                    Action::Play(_, _) => unreachable!(),
                };
                match result {
                    Ok(new_group) => {
                        outcome.changed = true;
                        self.confirm_delete = false;
                        if deleted {
                            self.select(None);
                        } else if let Some(id) = new_group {
                            if let Ok(groups) = db.groups() {
                                self.select(groups.iter().find(|g| g.id == id));
                            }
                        } else {
                            self.name = self.name.trim().to_owned();
                        }
                    }
                    Err(error) => outcome.error = Some(format!("{error:#}")),
                }
            }
        }
        outcome.open = open;
        outcome.selected = self.selected;
        outcome
    }
}

fn copy_name(groups: &[Group], group: &Group) -> String {
    let mut name = format!("{} (copy)", group.name);
    let mut number = 2;
    while groups
        .iter()
        .any(|g| g.kind == group.kind && g.name == name)
    {
        name = format!("{} (copy {number})", group.name);
        number += 1;
    }
    name
}

#[cfg(test)]
mod tests;
