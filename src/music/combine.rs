use super::*;
use crate::notation::PartGroupPreview;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct Editor {
    pub open: bool,
    generation: u64,
    members: BTreeSet<String>,
    target: String,
    name: String,
    preview: Option<Preview>,
}

struct Preview {
    group: PartGroup,
    summary: PartGroupPreview,
    snapshot: Vec<u8>,
}

impl Music {
    pub(crate) fn open_combine(&mut self) {
        self.combine_editor = Editor {
            open: true,
            generation: self.generation,
            ..Default::default()
        };
    }

    pub(crate) fn combine_open(&self) -> bool {
        self.combine_editor.open
    }

    pub(crate) fn close_combine(&mut self) {
        self.combine_editor.open = false;
    }

    fn structural_ready(&self) -> Result<()> {
        anyhow::ensure!(
            !self.plugin_edits_lost,
            "Instrument operation was cancelled; reload the movement before changing parts"
        );
        anyhow::ensure!(
            !self.mix_rejected,
            "Recover the rejected saved mix before changing parts"
        );
        anyhow::ensure!(
            self.original_composition.is_some(),
            "Load original notation first"
        );
        anyhow::ensure!(
            self.loading.is_none()
                && self.deferred_load.is_none()
                && self.job.is_none()
                && !self.path_picker_active,
            "Finish loading, recognition or the file chooser first"
        );
        anyhow::ensure!(self.playback.is_none(), "Save and unload instruments first");
        #[cfg(target_os = "linux")]
        {
            anyhow::ensure!(
                self.retiring.is_empty(),
                "Waiting for instruments to finish saving"
            );
            anyhow::ensure!(self.shutdown_error.is_none(), "Instrument save failed or was cancelled. Reload the movement before changing its layout");
        }
        anyhow::ensure!(
            !self.plugin_unsaved,
            "Unsaved instrument edits: reload the movement or save and unload first"
        );
        Ok(())
    }

    fn apply_groups(&mut self, generation: u64, groups: Vec<PartGroup>) -> Result<()> {
        self.structural_ready()?;
        anyhow::ensure!(
            generation == self.generation,
            "Movement changed; review a new preview"
        );
        let composition = notation::apply_part_groups(
            self.original_composition
                .as_ref()
                .context("Missing original notation")?,
            &groups,
        )?;
        let mut saved = self.saved_mix();
        saved.groups = groups.clone();
        self.persist_mix(&saved)?;
        self.composition = Some(composition);
        self.part_groups = groups;
        self.generation = self.generation.wrapping_add(1);
        self.mix_dirty = None;
        self.notice = "Part layout saved. Original notes and XML are unchanged. Close Combine parts, then press Play to restart from the beginning.".into();
        Ok(())
    }

    fn prepare_combine(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.loading.is_none() && self.job.is_none() && !self.path_picker_active,
            "Finish the current operation first"
        );
        #[cfg(target_os = "linux")]
        {
            anyhow::ensure!(
                self.retiring.is_empty() && self.shutdown_error.is_none(),
                "Wait for saving to complete, or reload after a failed save"
            );
        }
        if let Some(transport) = &self.playback {
            let snapshot = transport.snapshot();
            anyhow::ensure!(
                !snapshot.plugins.busy
                    && !snapshot.plugins.editor_busy
                    && !matches!(snapshot.state, State::Starting | State::Failed),
                "Finish or cancel the instrument dialog/state operation first"
            );
        } else {
            anyhow::ensure!(
                !self.plugin_edits_lost,
                "Reload the movement to recover from discarded unsaved edits"
            );
        }
        self.mix_dirty = Some(std::time::Instant::now());
        self.save_mix()?;
        if self.playback.is_none()
            || !self
                .effective_sources()
                .values()
                .any(|source| source.is_plugin())
        {
            self.plugin_unsaved = false;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.combine_cancel = Some(cancel.clone());
        self.stop();
        self.combine_cancel = Some(cancel);
        Ok(())
    }

    fn recover_rejected_mix(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.mix_rejected && self.playback.is_none(),
            "Unload instruments before recovering"
        );
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            self.retiring.is_empty(),
            "Wait for instrument saving to finish"
        );
        let path = [
            self.mix_path.as_ref(),
            self.v2_mix_path.as_ref(),
            self.legacy_mix_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find(|path| path.exists())
        .context("Rejected sidecar disappeared; reload the movement")?;
        let input = std::fs::File::open(path)?;
        anyhow::ensure!(
            input.metadata()?.len() <= MAX_MIX,
            "Oversized rejected mix; move it aside manually before reloading"
        );
        let mut backup = tempfile::Builder::new()
            .prefix("rejected-audio-mix-")
            .suffix(".json")
            .tempfile_in(path.parent().context("No sidecar directory")?)?;
        let copied = std::io::copy(&mut input.take(MAX_MIX + 1), &mut backup)?;
        anyhow::ensure!(
            copied <= MAX_MIX,
            "Rejected sidecar grew during backup; nothing replaced"
        );
        backup.as_file().sync_all()?;
        let (_, backup_path) = backup.keep()?;
        self.mix_rejected = false;
        if let Err(error) = self.persist_mix(&self.saved_mix()) {
            self.mix_rejected = true;
            return Err(error);
        }
        self.mix_dirty = None;
        self.error = None;
        self.notice = format!(
            "Current layout/settings saved. Rejected mix preserved at {}.",
            backup_path.display()
        );
        Ok(())
    }

    fn apply_preview(&mut self, generation: u64, preview: &Preview) -> Result<()> {
        anyhow::ensure!(
            preview.snapshot == serde_json::to_vec(&self.saved_mix())?,
            "Settings or groups changed; preview again before applying"
        );
        let mut groups = self.part_groups.clone();
        groups.push(preview.group.clone());
        self.apply_groups(generation, groups)
    }

    pub(super) fn combine_ui(&mut self, ctx: &egui::Context) {
        if !self.combine_editor.open {
            return;
        }
        let mut editor = std::mem::take(&mut self.combine_editor);
        if editor.generation != self.generation {
            editor = Editor {
                open: true,
                generation: self.generation,
                ..Default::default()
            };
        }
        let mut open = true;
        enum Action {
            Prepare,
            #[cfg(target_os = "linux")]
            Cancel,
            Apply,
            Groups(Vec<PartGroup>),
            Recover,
            #[cfg(target_os = "linux")]
            Reload,
        }
        let mut action = None;
        egui::Window::new("Combine parts")
            .open(&mut open)
            .default_size([560.0, 480.0])
            .min_width(260.0)
            .max_width((ctx.screen_rect().width() - 16.0).max(260.0))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.label("Explicitly reunite OMR fragments. Notes remain at their original times; no silence is removed and no cross-part ties are inferred.");
                ui.label("The retained target supplies the instrument, gain, volume and mute setting. Other member settings are kept for Uncombine.");
                if self.mix_rejected && ui.button("Recover using current layout (back up rejected mix)").clicked() { action = Some(Action::Recover); }
                let ready = self.structural_ready();
                if let Err(error) = &ready { ui.colored_label(Color32::YELLOW, error.to_string()); }
                #[cfg(target_os = "linux")]
                if (self.plugin_edits_lost || self.shutdown_error.is_some()) && self.xml.is_some()
                    && ui.add_enabled(self.retiring.is_empty(), egui::Button::new("Reload movement (discard unsaved edits)")).clicked() {
                    action = Some(Action::Reload);
                }
                if ui.button("Save and unload instruments").clicked() { action = Some(Action::Prepare); }
                #[cfg(target_os = "linux")]
                if !self.retiring.is_empty() {
                    ui.spinner();
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    if ui.button("Cancel saving (keep last checkpoint)").clicked() { action = Some(Action::Cancel); }
                }
                let occupied: BTreeSet<_> = self.part_groups.iter().flat_map(|g| g.members.iter()).collect();
                let mut changed = false;
                if let Some(original) = &self.original_composition {
                    for part in &original.parts {
                        if occupied.contains(&part.id) { continue; }
                        let mut on = editor.members.contains(&part.id);
                        let start = part.notes.iter().map(|n| n.start_tick).min();
                        let end = part.notes.iter().map(|n| n.start_tick + n.duration_ticks).max();
                        if ui.checkbox(&mut on, format!("{} · {} · {} staves · {} notes · ticks {:?}–{:?}", part.id, part.name, part.staves, part.notes.len(), start, end)).changed() {
                            if on { editor.members.insert(part.id.clone()); } else { editor.members.remove(&part.id); }
                            changed = true;
                        }
                    }
                }
                egui::ComboBox::from_id_salt("combine_target").selected_text(
                    if editor.target.is_empty() { "Choose retained target" } else { &editor.target })
                    .show_ui(ui, |ui| {
                        for id in &editor.members {
                            changed |= ui.selectable_value(&mut editor.target, id.clone(), id).changed();
                        }
                    });
                ui.label("Combined display name (1–256 bytes):");
                let response = ui.add(egui::TextEdit::singleline(&mut editor.name).char_limit(256));
                self.controls_active |= response.has_focus();
                changed |= response.changed();
                if changed { editor.preview = None; }
                let group = PartGroup { target_id: editor.target.clone(), members: editor.members.iter().cloned().collect(), name: editor.name.clone() };
                if ui.add_enabled(editor.members.len() >= 2 && editor.members.contains(&editor.target), egui::Button::new("Preview combination")).clicked() {
                    let result = (|| -> Result<Preview> {
                        let summary = notation::preview_part_group(self.original_composition.as_ref().context("No notation")?, &group)?;
                        Ok(Preview { group, summary, snapshot: serde_json::to_vec(&self.saved_mix())? })
                    })();
                    match result { Ok(preview) => editor.preview = Some(preview), Err(error) => self.error = Some(format!("{error:#}")) }
                }
                if let Some(preview) = &editor.preview {
                    let p = &preview.summary;
                    ui.label(format!("{} notes · ticks {:?}–{:?} · {} simultaneous pairs · {} unison pairs · {} exact duplicate pairs", p.note_count, p.start_tick, p.end_tick, p.overlapping_note_pairs, p.overlapping_unison_pairs, p.exact_duplicate_pairs));
                    ui.colored_label(Color32::YELLOW, "Simultaneous material sounds together. Playback/MIDI coalesces overlapping unisons; articulation or loudness may change. Raw notes are never deleted.");
                    ui.label("Review member settings; differences are NOT merged:");
                    for id in &preview.group.members {
                        let source = self.sources.get(id);
                        let instrument = match source {
                            Some(crate::audio::plugins::Source::Vst3 { path, class_id, gain, .. }) => format!("VST3 {} · class {} · gain {gain}", path.display(), class_id),
                            _ => program_label(*self.programs.get(id).unwrap_or(&0)),
                        };
                        ui.label(format!("{}{} · {} · volume {}% · enabled {}", id,
                            if *id == preview.group.target_id { " (retained)" } else { "" },
                            instrument, self.volumes.get(id).copied().unwrap_or(100),
                            self.selected.get(id).copied().unwrap_or(true)));
                    }
                    if ui.add_enabled(ready.is_ok(), egui::Button::new("Apply combination")).clicked() { action = Some(Action::Apply); }
                }
                ui.separator();
                ui.label("Existing combinations. Uncombine restores layout, not subsequent instrument edits.");
                for (index, group) in self.part_groups.iter().enumerate() {
                    ui.label(format!("{} · target {} · {}", group.name, group.target_id, group.members.join(", ")));
                    if ui.add_enabled(ready.is_ok(), egui::Button::new(format!("Uncombine {}", group.name))).clicked() {
                        let mut groups = self.part_groups.clone();
                        groups.remove(index);
                        action = Some(Action::Groups(groups));
                    }
                }
                if ui.add_enabled(ready.is_ok() && !self.part_groups.is_empty(), egui::Button::new("Restore original part layout")).clicked() { action = Some(Action::Groups(vec![])); }
                if let Some(error) = &self.error { ui.colored_label(Color32::LIGHT_RED, error); }
                ui.small("Playback and MIDI use the combined layout. Export original MusicXML does not contain combinations.");
            });
        let result = match action {
            Some(Action::Prepare) => self.prepare_combine(),
            #[cfg(target_os = "linux")]
            Some(Action::Cancel) => {
                if let Some(cancel) = &self.combine_cancel {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(())
            }
            Some(Action::Apply) => editor
                .preview
                .as_ref()
                .context("Preview required")
                .and_then(|preview| self.apply_preview(editor.generation, preview)),
            Some(Action::Groups(groups)) => self.apply_groups(editor.generation, groups),
            Some(Action::Recover) => self.recover_rejected_mix(),
            #[cfg(target_os = "linux")]
            Some(Action::Reload) => {
                self.deferred_load = self.xml.clone().map(|path| (path, false));
                Ok(())
            }
            None => Ok(()),
        };
        if let Err(error) = result {
            self.error = Some(format!("Combining parts: {error:#}"));
        }
        editor.open = open;
        self.combine_editor = editor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::plugins::Source;

    fn fixture() -> (tempfile::TempDir, Music) {
        let root = tempfile::tempdir().unwrap();
        let original = notation::parse_musicxml(r#"<score-partwise><part-list>
          <score-part id="A"><part-name>Full name</part-name></score-part>
          <score-part id="B"><part-name>Abbreviation</part-name></score-part></part-list>
          <part id="A"><measure number="1"><attributes><divisions>1</divisions></attributes>
          <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration></note></measure></part>
          <part id="B"><measure number="1"><attributes><divisions>1</divisions></attributes>
          <note><rest/><duration>1</duration></note></measure>
          <measure number="2"><note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note></measure></part>
          </score-partwise>"#).unwrap();
        let mut music = music_for(original);
        music.mix_path = Some(root.path().join("mix/audio-v3.json"));
        music.v2_mix_path = Some(root.path().join("mix/audio-v2.json"));
        music.legacy_mix_path = Some(root.path().join("mix/legacy.json"));
        (root, music)
    }

    fn music_for(original: Composition) -> Music {
        Music {
            binding: Some((1, "a".repeat(64))),
            music_hash: Some("b".repeat(64)),
            selected: original
                .parts
                .iter()
                .map(|p| (p.id.clone(), true))
                .collect(),
            programs: original
                .parts
                .iter()
                .map(|p| (p.id.clone(), p.program))
                .collect(),
            volumes: original.parts.iter().map(|p| (p.id.clone(), 100)).collect(),
            composition: Some(original.clone()),
            original_composition: Some(original),
            tempo_scale: 1.0,
            master_volume: 100,
            ..Default::default()
        }
    }

    fn group() -> PartGroup {
        PartGroup {
            target_id: "A".into(),
            members: vec!["A".into(), "B".into()],
            name: "Combined".into(),
        }
    }

    fn preview(music: &Music) -> Preview {
        Preview {
            summary: notation::preview_part_group(
                music.original_composition.as_ref().unwrap(),
                &group(),
            )
            .unwrap(),
            group: group(),
            snapshot: serde_json::to_vec(&music.saved_mix()).unwrap(),
        }
    }

    fn notes(composition: &Composition) -> Vec<(u64, u64, u8, u8, u16, String)> {
        let mut notes: Vec<_> = composition
            .parts
            .iter()
            .flat_map(|p| p.notes.iter())
            .map(|n| {
                (
                    n.start_tick,
                    n.duration_ticks,
                    n.pitch,
                    n.velocity,
                    n.staff,
                    n.voice.clone(),
                )
            })
            .collect();
        notes.sort();
        notes
    }

    #[test]
    fn v3_roundtrip_hidden_sources_and_uncombine_preserve_original_settings() {
        let (_root, mut music) = fixture();
        let original = music.original_composition.clone().unwrap();
        let hidden = Source::Vst3 {
            path: "/missing-but-retained.vst3".into(),
            class_id: "A".repeat(32),
            module_sha256: "b".repeat(64),
            gain: 0.3,
            component_state: None,
            controller_state: None,
        };
        music.sources.insert("B".into(), hidden.clone());
        music.volumes.insert("B".into(), 23);
        music.apply_preview(0, &preview(&music)).unwrap();
        assert_eq!(music.composition.as_ref().unwrap().parts.len(), 1);
        assert!(music.effective_sources().is_empty());
        assert!(!music.options().programs.contains_key("B"));
        assert!(!music.options().volumes.contains_key("B"));
        assert_eq!(
            music.options().selected_parts.unwrap(),
            vec!["A".to_string()]
        );
        assert_eq!(
            u16::from_be_bytes(music.midi().unwrap()[10..12].try_into().unwrap()),
            2
        );
        let plan = crate::audio::plan::compile_with_sources(
            music.composition.as_ref().unwrap(),
            &music.options(),
            &music.effective_sources(),
        )
        .unwrap();
        assert_eq!(plan.parts.len(), 1);
        assert!(!plan.parts[0].source.is_plugin());
        assert_eq!(notes(&original), notes(music.composition.as_ref().unwrap()));
        music.part_groups.clear();
        music.composition = Some(original.clone());
        music.load_mix().unwrap();
        assert_eq!(music.part_groups, vec![group()]);
        assert_eq!(music.sources["B"], hidden);
        music.volumes.insert("A".into(), 37);
        music.apply_groups(music.generation, vec![]).unwrap();
        assert_eq!(music.volumes["A"], 37);
        assert_eq!(music.volumes["B"], 23);
        assert_eq!(music.sources["B"], hidden);
        let restored = music.composition.as_ref().unwrap();
        assert_eq!(
            restored
                .parts
                .iter()
                .map(|p| (&p.id, &p.name))
                .collect::<Vec<_>>(),
            original
                .parts
                .iter()
                .map(|p| (&p.id, &p.name))
                .collect::<Vec<_>>()
        );
        assert_eq!(notes(restored), notes(&original));
    }

    #[test]
    fn failed_save_stale_preview_and_retirement_leave_layout_and_manifest_untouched() {
        let (root, mut music) = fixture();
        music.mix_dirty = Some(std::time::Instant::now());
        music.save_mix().unwrap();
        let path = music.mix_path.clone().unwrap();
        let prior = std::fs::read(&path).unwrap();
        let draft = preview(&music);
        music.volumes.insert("A".into(), 47);
        assert!(music.apply_preview(0, &draft).is_err());
        assert!(music.apply_preview(99, &preview(&music)).is_err());
        let blocker = root.path().join("not-a-directory");
        std::fs::write(&blocker, b"keep").unwrap();
        music.mix_path = Some(blocker.join("sidecar.json"));
        assert!(music.apply_preview(0, &preview(&music)).is_err());
        assert!(music.part_groups.is_empty());
        assert_eq!(music.generation, 0);
        assert_eq!(music.composition.as_ref().unwrap().parts.len(), 2);
        assert_eq!(std::fs::read(&path).unwrap(), prior);
        #[cfg(target_os = "linux")]
        {
            let (_tx, rx) = bounded(1);
            music.retiring.push(rx);
            assert!(music.structural_ready().is_err());
            music.retiring.clear();
            music.shutdown_error = Some("Cancelled".into());
            assert!(music.structural_ready().is_err());
            assert!(music.prepare_combine().is_err());
        }
    }

    #[test]
    fn migrates_v1_and_v2_without_overwriting_them_and_rejects_invalid_v3_atomically() {
        for version in [0, 2] {
            let (_root, mut music) = fixture();
            let old = if version == 0 {
                music.legacy_mix_path.clone().unwrap()
            } else {
                music.v2_mix_path.clone().unwrap()
            };
            std::fs::create_dir_all(old.parent().unwrap()).unwrap();
            let mut saved = music.saved_mix();
            saved.version = version;
            saved.programs.insert("B".into(), 42);
            let bytes = serde_json::to_vec(&saved).unwrap();
            std::fs::write(&old, &bytes).unwrap();
            music.load_mix().unwrap();
            assert_eq!(music.programs["B"], 42);
            music.apply_preview(0, &preview(&music)).unwrap();
            assert_eq!(std::fs::read(&old).unwrap(), bytes);
            let v3 = music.mix_path.clone().unwrap();
            let mut saved = music.saved_mix();
            saved.music_hash = Some("stale".into());
            std::fs::write(&v3, serde_json::to_vec(&saved).unwrap()).unwrap();
            assert!(music.load_mix().is_err());
            assert_eq!(music.part_groups, vec![group()]);
            assert_eq!(music.programs["B"], 42);
            saved.music_hash = music.music_hash.clone();
            saved.groups[0].members.push("unknown".into());
            saved.volumes.insert("A".into(), 1);
            std::fs::write(&v3, serde_json::to_vec(&saved).unwrap()).unwrap();
            assert!(music.load_mix().is_err());
            assert_eq!(music.volumes["A"], 100);
            assert_eq!(music.part_groups, vec![group()]);
        }
    }

    #[test]
    fn combine_controls_are_scroll_reachable_and_apply_requires_a_preview() {
        let (_root, mut music) = fixture();
        music.open_combine();
        let ctx = egui::Context::default();
        let labels = |music: &mut Music| {
            let mut seen = BTreeSet::new();
            for frame in 0..45 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 360.0),
                        )),
                        events: if frame > 3 {
                            vec![
                                egui::Event::PointerMoved(egui::pos2(280.0, 180.0)),
                                egui::Event::MouseWheel {
                                    unit: egui::MouseWheelUnit::Point,
                                    delta: egui::vec2(0.0, -70.0),
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                    |ctx| music.combine_ui(ctx),
                );
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        let visible = shape
                            .clip_rect
                            .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                        if visible.height() > 8.0 && visible.width() > 10.0 {
                            seen.insert(text.galley.job.text.clone());
                        }
                    }
                }
            }
            seen
        };
        let seen = labels(&mut music);
        assert!(seen.contains("Preview combination"), "{seen:?}");
        assert!(!seen.contains("Apply combination"));
        music.combine_editor.preview = Some(preview(&music));
        let seen = labels(&mut music);
        assert!(seen.contains("Apply combination"), "{seen:?}");
        music.apply_preview(0, &preview(&music)).unwrap();
        music.open_combine();
        let seen = labels(&mut music);
        assert!(seen.contains("Uncombine Combined"), "{seen:?}");
        assert!(seen.contains("Restore original part layout"), "{seen:?}");
    }

    #[test]
    fn rejected_sidecar_is_read_only_until_explicit_backed_up_recovery() {
        let (_root, mut music) = fixture();
        let path = music.mix_path.clone().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rejected = b"{malformed sidecar with potentially recoverable settings";
        std::fs::write(&path, rejected).unwrap();
        assert!(music.load_mix().is_err());
        music.volumes.insert("A".into(), 38);
        music.mix_dirty = Some(std::time::Instant::now());
        assert!(music.save_mix().is_err());
        assert!(music.prepare_combine().is_err());
        assert!(music.apply_preview(0, &preview(&music)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), rejected);
        music.recover_rejected_mix().unwrap();
        assert!(!music.mix_rejected);
        assert_eq!(music.volumes["A"], 38);
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("rejected-audio-mix-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), rejected);
        music.load_mix().unwrap();
        assert_eq!(music.volumes["A"], 38);
        assert!(music.structural_ready().is_ok());
    }

    #[test]
    fn assignment_only_prepare_saves_without_loading_but_lost_live_edits_block_it() {
        let (_root, mut music) = fixture();
        music.plugin_unsaved = true;
        music.sources.insert("A".into(), Source::Soundfont);
        music.prepare_combine().unwrap();
        assert!(!music.plugin_unsaved);
        assert!(music.playback.is_none());
        assert!(music.structural_ready().is_ok());
        music.plugin_edits_lost = true;
        assert!(music.prepare_combine().is_err());
        assert!(music.structural_ready().is_err());
        assert!(music.apply_preview(0, &preview(&music)).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn movement_load_waits_for_retirement_and_does_not_inherit_old_failures() {
        let (root, mut music) = fixture();
        let source = root.path().join("score.pdf");
        std::fs::write(&source, b"generated source").unwrap();
        let hash =
            crate::folder::hash_file(&source, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        music.binding = Some((1, hash));
        music.source = Some(source);
        let xml = root.path().join("new.xml");
        std::fs::write(&xml, r#"<score-partwise><part-list><score-part id="N"><part-name>New</part-name></score-part></part-list><part id="N"><measure number="1"><note><rest/><duration>1</duration></note></measure></part></score-partwise>"#).unwrap();
        let paths = Paths {
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            config: root.path().join("config"),
            music: None,
        };
        paths.create().unwrap();
        let ctx = egui::Context::default();
        let (tx, rx) = bounded(1);
        music.retiring.push(rx);
        music.start_load(xml.clone(), true, &paths, &ctx);
        assert!(music.loading.is_none());
        assert_eq!(music.deferred_load.as_ref().unwrap().0, xml);
        tx.send(Err("Previous movement checkpoint cancelled".into()))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            music.poll(&paths, &ctx);
            if music.loading.is_none() && music.deferred_load.is_none() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", music.error);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(music.composition.as_ref().unwrap().parts[0].id, "N");
        assert!(music.shutdown_error.is_none(), "{:?}", music.shutdown_error);
        assert!(music.error.is_none(), "{:?}", music.error);
    }

    #[test]
    #[ignore = "Set RMS_TEST_COMBINE_MUSICXML to the private five-part duet; reads notation only"]
    fn private_duet_five_to_two_preserves_all_notes_timing_and_page_mapping() {
        let path = PathBuf::from(
            std::env::var_os("RMS_TEST_COMBINE_MUSICXML").expect("Set RMS_TEST_COMBINE_MUSICXML"),
        );
        let hash =
            || crate::folder::hash_file(&path, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        let before = hash();
        let original = notation::load(&path).unwrap();
        assert_eq!(original.parts.len(), 5);
        let groups = vec![
            PartGroup {
                target_id: "P4".into(),
                members: vec!["P1".into(), "P3".into(), "P4".into()],
                name: "Violin".into(),
            },
            PartGroup {
                target_id: "P5".into(),
                members: vec!["P2".into(), "P5".into()],
                name: "Cello".into(),
            },
        ];
        for group in &groups {
            let preview = notation::preview_part_group(&original, group).unwrap();
            assert_eq!(preview.overlapping_note_pairs, 0);
            println!(
                "{}: {} notes, ticks {:?}–{:?}, zero cross-fragment overlaps",
                group.name, preview.note_count, preview.start_tick, preview.end_tick
            );
        }
        let root = tempfile::tempdir().unwrap();
        let mut music = music_for(original.clone());
        music.mix_path = Some(root.path().join("private.audio-v3.json"));
        music.programs.insert("P4".into(), 40);
        music.programs.insert("P5".into(), 42);
        music.apply_groups(0, groups).unwrap();
        let combined = music.composition.as_ref().unwrap();
        assert_eq!(
            combined
                .parts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["P4", "P5"]
        );
        assert_eq!(notes(combined), notes(&original));
        assert_eq!(combined.duration_ticks, original.duration_ticks);
        assert_eq!(combined.duration_ticks, 250800);
        assert_eq!(
            combined
                .tempos
                .iter()
                .map(|t| (t.tick, t.bpm.to_bits()))
                .collect::<Vec<_>>(),
            original
                .tempos
                .iter()
                .map(|t| (t.tick, t.bpm.to_bits()))
                .collect::<Vec<_>>()
        );
        match (&combined.page_map, &original.page_map) {
            (Some(a), Some(b)) => {
                assert_eq!(a.source_pages, b.source_pages);
                assert_eq!(a.turns, b.turns);
                assert_eq!(a.measure_ticks, b.measure_ticks);
            }
            (None, None) => {}
            _ => panic!("Page map changed"),
        }
        let midi = music.midi().unwrap();
        assert_eq!(u16::from_be_bytes(midi[10..12].try_into().unwrap()), 3);
        let plan = crate::audio::plan::compile(combined, &music.options()).unwrap();
        assert_eq!(plan.parts.len(), 2);
        assert_eq!(plan.duration_ticks, original.duration_ticks);
        music.apply_groups(1, vec![]).unwrap();
        assert_eq!(notes(music.composition.as_ref().unwrap()), notes(&original));
        assert_eq!(hash(), before);
        println!("Duet: 5 → 2 → 5 parts, {} raw notes, {} ticks; MIDI and native plans valid; original file unchanged", notes(&original).len(), original.duration_ticks);
    }
}
