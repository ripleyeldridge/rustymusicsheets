use crate::text_input::TextEditFocusExt;
use eframe::egui::{self, Align, Layout, Vec2};
use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHOR: &str = env!("CARGO_PKG_AUTHORS");
pub const RELEASE_YEAR: &str = "2026";

pub struct Topic {
    pub id: &'static str,
    pub category: &'static str,
    pub title: &'static str,
    pub body: &'static str,
    search_text: String,
}

pub fn topics() -> &'static [Topic] {
    static TOPICS: OnceLock<Vec<Topic>> = OnceLock::new();
    TOPICS.get_or_init(|| {
        include_str!("../assets/help.txt")
            .split("\n@")
            .map(|block| {
                let (header, body) = block.split_once('\n').expect("Help article body");
                let fields: Vec<_> = header.trim_start_matches('@').split('|').collect();
                assert_eq!(fields.len(), 4, "Help topic header");
                let identity = if fields[0] == "about" {
                    format!(" {VERSION} {AUTHOR} {RELEASE_YEAR}")
                } else {
                    String::new()
                };
                Topic {
                    id: fields[0],
                    category: fields[1],
                    title: fields[2],
                    body: body.trim(),
                    search_text: format!("{header}\n{body}{identity}").to_lowercase(),
                }
            })
            .collect()
    })
}

pub fn search(query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    topics()
        .iter()
        .enumerate()
        .filter(|(_, topic)| terms.iter().all(|term| topic.search_text.contains(term)))
        .map(|(index, _)| index)
        .collect()
}

#[derive(Default)]
pub struct Help {
    pub open: bool,
    query: String,
    selected: usize,
    compact_topics: bool,
}

impl Help {
    pub fn open_topic(&mut self, id: &str) {
        self.selected = topics().iter().position(|t| t.id == id).unwrap_or(0);
        self.query.clear();
        self.compact_topics = id == "welcome";
        self.open = true;
    }

    pub fn menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Help", |ui| {
            for (label, id) in [
                ("Help contents (F1)", "welcome"),
                ("Keyboard shortcuts", "shortcuts"),
                ("About RustyMusicSheets", "about"),
            ] {
                if ui.button(label).clicked() {
                    self.open_topic(id);
                    ui.close_menu();
                }
            }
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut typing = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.horizontal_wrapped(|ui| {
                ui.heading("RustyMusicSheets Help");
                if ui.button("Close help").clicked() {
                    self.open = false;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Search help:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new("help_search"))
                        .hint_text("Search every topic…")
                        .desired_width((ui.available_width() - 85.0).max(100.0)),
                );
                typing |= response.text_edit_has_focus();
                if response.changed() {
                    self.compact_topics = true;
                }
                if ui.button("Clear").clicked() {
                    self.query.clear();
                }
            });
            let matches = search(&self.query);
            let searching = !self.query.trim().is_empty();
            if searching {
                ui.label(format!("{} matching topics", matches.len()));
            } else {
                ui.label("Offline help · Expand a category and click a subject · Esc: return");
            }
            ui.separator();
            if ui.available_width() < 820.0 {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.compact_topics, true, "Topics");
                    ui.selectable_value(&mut self.compact_topics, false, "Article");
                });
                if self.compact_topics {
                    self.tree(ui, &matches, searching);
                } else {
                    self.article(ui);
                }
            } else {
                ui.horizontal_top(|ui| {
                    let height = ui.available_height();
                    ui.allocate_ui_with_layout(
                        Vec2::new(280.0, height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(280.0);
                            self.tree(ui, &matches, searching);
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        Layout::top_down(Align::Min),
                        |ui| self.article(ui),
                    );
                });
            }
        });
        typing
    }

    fn tree(&mut self, ui: &mut egui::Ui, matches: &[usize], searching: bool) {
        egui::ScrollArea::vertical()
            .id_salt(("help_tree", searching))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if matches.is_empty() {
                    ui.label("No matching help topics. Try fewer words or Clear search.");
                }
                let mut categories = Vec::new();
                for &index in matches {
                    let category = topics()[index].category;
                    if !categories.contains(&category) {
                        categories.push(category);
                    }
                }
                for category in categories {
                    egui::CollapsingHeader::new(category)
                        .id_salt(("help_category", category, searching))
                        .default_open(true)
                        .open(searching.then_some(true))
                        .show(ui, |ui| {
                            for &index in matches {
                                let topic = &topics()[index];
                                if topic.category == category
                                    && ui
                                        .add(
                                            egui::Button::new(topic.title)
                                                .selected(self.selected == index)
                                                .wrap(),
                                        )
                                        .clicked()
                                {
                                    self.selected = index;
                                    self.compact_topics = false;
                                }
                            }
                        });
                }
            });
    }

    fn article(&mut self, ui: &mut egui::Ui) {
        let topic = &topics()[self.selected];
        egui::ScrollArea::vertical()
            .id_salt(("help_article", topic.id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(topic.category);
                ui.heading(topic.title);
                if topic.id == "about" {
                    ui.label(format!("Version: {VERSION}"));
                    ui.label(format!("Author: {AUTHOR}"));
                    ui.label(format!("Release year: {RELEASE_YEAR}"));
                    ui.label("License: MIT");
                    ui.separator();
                }
                for paragraph in topic.body.split("\n\n") {
                    ui.add(egui::Label::new(paragraph).wrap().selectable(true));
                    ui.add_space(4.0);
                }
                ui.separator();
                ui.strong("Related subjects");
                for (index, other) in topics().iter().enumerate() {
                    if other.category == topic.category
                        && index != self.selected
                        && ui.link(other.title).clicked()
                    {
                        self.selected = index;
                    }
                }
                if ui.link("Help contents").clicked() {
                    self.query.clear();
                    self.compact_topics = true;
                    self.selected = 0;
                }
            });
    }
}

#[cfg(test)]
mod tests;
