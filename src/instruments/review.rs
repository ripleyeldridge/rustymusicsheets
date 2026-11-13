use super::Detection;
use crate::{
    display_names::DisplayNames,
    model::{Paths, Score},
    storage::Library,
};
use eframe::egui::{self, Color32, Id};

#[derive(Default)]
pub struct Review {
    pub score_id: Option<i64>,
    loaded: Option<(i64, String)>,
    result: Option<Detection>,
    selected: Vec<bool>,
    replace_manual: bool,
}

#[derive(Default)]
pub struct Outcome {
    pub detect: Option<i64>,
    pub edit: Option<i64>,
    pub applied: bool,
    pub close: bool,
    pub error: Option<String>,
}

impl Review {
    pub fn open(&mut self, id: Option<i64>) {
        self.score_id = id;
        self.invalidate();
    }
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        db: &Library,
        paths: &Paths,
        scores: &[Score],
        display: &DisplayNames,
        busy: bool,
    ) -> Outcome {
        let mut outcome = Outcome::default();
        ui.horizontal_wrapped(|ui| {
            ui.heading("Instruments");
            outcome.close = ui.button("Close review").clicked();
        });
        egui::ScrollArea::vertical().id_salt("instrument_review_body")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false]).show(ui, |ui| {
        if self.score_id.is_none() {
            self.score_id = scores
                .iter()
                .find(|s| !s.detection_status.is_empty())
                .or_else(|| scores.first())
                .map(|s| s.id);
        }
        let label = scores
            .iter()
            .find(|s| Some(s.id) == self.score_id)
            .map_or_else(|| "Choose a score".into(), |s| display.format(&s.title));
        let previous = self.score_id;
        egui::ComboBox::from_id_salt("instrument_review_score")
            .width(ui.available_width() - 20.0)
            .selected_text(label)
            .show_ui(ui, |ui| {
                for score in scores {
                    ui.selectable_value(
                        &mut self.score_id,
                        Some(score.id),
                        format!(
                            "{} [{}]",
                            display.format(&score.title),
                            score.detection_status
                        ),
                    );
                }
            });
        if previous != self.score_id {
            self.invalidate();
        }
        let Some(score) = scores.iter().find(|s| Some(s.id) == self.score_id) else {
            ui.label("Import or select a score first.");
            return;
        };
        ui.colored_label(Color32::YELLOW, super::DETECTION_TIME_WARNING);
        if ui
            .add_enabled(!busy, egui::Button::new("Detect / rescan instruments"))
            .clicked()
        {
            outcome.detect = Some(score.id);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Edit instruments manually"))
            .clicked()
        {
            outcome.edit = Some(score.id);
        }
        let key = (score.id, score.hash.clone());
        if self.loaded.as_ref() != Some(&key) {
            match db.detection(score.id, &score.hash) {
                Ok(result) => {
                    self.selected = result
                        .as_ref()
                        .map_or_else(Vec::new, |r| r.candidates.iter().map(|c| c.clear).collect());
                    self.result = result;
                    self.replace_manual = false;
                    self.loaded = Some(key);
                }
                Err(error) => outcome.error = Some(format!("{error:#}")),
            }
        }
        if score.instruments_manual {
            ui.colored_label(
                Color32::YELLOW,
                "Current instrument choices are manually protected.",
            );
            ui.checkbox(
                &mut self.replace_manual,
                "Replace my current manual choices",
            );
        }
        let selected: Vec<_> = self.result.as_ref().map_or_else(Vec::new, |r| {
            r.candidates
                .iter()
                .zip(&self.selected)
                .filter(|(_, chosen)| **chosen)
                .map(|(candidate, _)| candidate.instrument.clone())
                .collect()
        });
        let allowed =
            !busy && !selected.is_empty() && (!score.instruments_manual || self.replace_manual);
        if ui
            .add_enabled(allowed, egui::Button::new("Apply selected instruments"))
            .clicked()
        {
            let apply = (|| -> anyhow::Result<()> {
                if let Some(result) = &self.result {
                    let metadata = std::fs::metadata(db.pdf_path(paths, &score.hash)?)?;
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| u64::try_from(d.as_nanos()).ok());
                    if metadata.len() != result.file_size || modified != result.modified_ns {
                        anyhow::bail!(
                            "File changed since detection; rescan the library and detect again"
                        );
                    }
                }
                db.apply_instruments(score.id, &score.hash, &selected)
            })();
            match apply {
                Ok(()) => {
                    outcome.applied = true;
                    self.replace_manual = false;
                }
                Err(error) => outcome.error = Some(format!("{error:#}")),
            }
        }
        ui.label("Applying confirms these choices and protects them from automatic changes.");
        ui.separator();
                ui.label(format!("Current: {}", if score.instruments.is_empty() { "Unassigned".into() }
                    else { score.instruments.iter().map(|i| i.label()).collect::<Vec<_>>().join("; ") }));
                if let Some(result) = &self.result {
                    if let Some(error) = &result.error { ui.colored_label(Color32::LIGHT_RED, error); }
                    if result.candidates.is_empty() { ui.label("No unambiguous instrument candidates were found. You can enter instruments manually."); }
                    ui.label(format!("{} initial page(s) scanned", result.pages));
                    for (index, candidate) in result.candidates.iter().enumerate() {
                        ui.push_id(Id::new(("instrument_candidate", index)), |ui| {
                            ui.group(|ui| {
                                ui.add_enabled(!busy, egui::Checkbox::new(&mut self.selected[index], candidate.instrument.label()));
                                ui.label(if candidate.clear { "Clear label (heuristic)" } else { "Needs review" });
                                ui.label(format!("Page {} · {}", candidate.page, candidate.method));
                                if let Some(confidence) = candidate.ocr_confidence {
                                    ui.label(format!("OCR text confidence: {confidence:.0}/100 (not categorization probability)"));
                                }
                                ui.add(egui::Label::new(format!("Read: {}", candidate.evidence)).wrap());
                            });
                        });
                    }
                    for warning in &result.warnings { ui.colored_label(Color32::YELLOW, warning); }
                } else { ui.label("No detection for this file version yet. Use Detect above."); }
            });
        outcome
    }
}

#[cfg(test)]
mod tests;
