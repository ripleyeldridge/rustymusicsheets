use super::*;
use crate::audio::plugins::{Request, Source};

#[cfg(target_os = "linux")]
type TaskOutput = (
    crate::audio::discovery::Catalogue,
    String,
    Option<Vec<String>>,
);

#[cfg(target_os = "linux")]
pub(super) struct Task {
    result: Receiver<Result<TaskOutput, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_os = "linux")]
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Music {
    fn can_replace_plugin_source(&self) -> bool {
        self.retiring.is_empty()
            && self.playback.as_ref().is_none_or(|transport| {
                !self.plugin_unsaved && transport.snapshot().plugins.dirty_parts.is_empty()
            })
    }

    pub(super) fn refresh_saved_plugin_states(&mut self) -> Result<()> {
        if !self.sources.values().any(Source::is_plugin) {
            return Ok(());
        }
        let Some(path) = &self.mix_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(file.metadata()?.len() <= MAX_MIX, "Oversized saved mix");
        let saved: SavedMix = serde_json::from_reader(file.take(MAX_MIX + 1))?;
        anyhow::ensure!(
            saved.version == 2
                && saved.score_hash == self.binding.as_ref().map(|(_, hash)| hash.clone())
                && saved.music_hash == self.music_hash,
            "Plugin checkpoint identity changed; reload movement"
        );
        for (id, prior) in saved.sources {
            prior.validate()?;
            if let (
                Some(Source::Vst3 {
                    class_id,
                    component_state,
                    controller_state,
                    ..
                }),
                Source::Vst3 {
                    class_id: saved_class,
                    component_state: component,
                    controller_state: controller,
                    ..
                },
            ) = (self.sources.get_mut(&id), prior)
            {
                if *class_id == saved_class {
                    *component_state = component;
                    *controller_state = controller;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn has_plugin_session(&self) -> bool {
        self.playback.is_some() && self.sources.values().any(Source::is_plugin)
    }
    pub(crate) fn begin_shutdown(&mut self) {
        self.shutdown_error = None;
        if let Err(error) = self.save_mix() {
            self.error = Some(format!("Saving before shutdown: {error:#}"));
        }
        self.stop();
    }
    pub(crate) fn shutdown_pending(&self) -> bool {
        !self.retiring.is_empty()
    }
    pub(crate) fn shutdown_error(&self) -> Option<&str> {
        self.shutdown_error.as_deref()
    }
    pub(super) fn retire_transport(&mut self, transport: Transport) {
        use std::time::{Duration, Instant};
        if !self.sources.values().any(Source::is_plugin)
            || matches!(transport.snapshot().state, State::Starting | State::Failed)
        {
            return;
        }
        let Some(path) = self.mix_path.clone() else {
            return;
        };
        if self.retiring.len() >= 4 {
            self.error = Some("Too many pending shutdown checkpoints; last saved state retained. Save explicitly before switching again.".into());
            return;
        }
        let mut mix = SavedMix {
            version: 2,
            score_hash: self.binding.as_ref().map(|(_, h)| h.clone()),
            music_hash: self.music_hash.clone(),
            sources: self.sources.clone(),
            selected: self.selected.clone(),
            programs: self.programs.clone(),
            volumes: self.volumes.clone(),
            tempo_scale: self.tempo_scale,
            master_volume: self.master_volume,
            follow_pages: self.follow_pages,
            turn_lead_measures: self.turn_lead_measures,
        };
        let (tx, rx) = bounded(1);
        self.retiring.push(rx);
        let guard = self.manifest_guard.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let parent = path.parent().context("Missing movement directory")?;
                let previous = match std::fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                };
                transport.pause()?;
                let deadline = Instant::now() + Duration::from_secs(5);
                while !matches!(transport.snapshot().state, State::Paused | State::Stopped | State::Finished) {
                    let snapshot = transport.snapshot();
                    anyhow::ensure!(snapshot.state != State::Failed, "{}", snapshot.error.unwrap_or_default());
                    anyhow::ensure!(Instant::now() < deadline, "Pause before shutdown checkpoint timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
                let last = transport.snapshot().plugins.checkpoint.as_ref().map(|(id, _)| *id);
                transport.plugin(Request::Checkpoint { destination: parent.join("states") })?;
                let deadline = Instant::now() + Duration::from_secs(60);
                let parts = loop {
                    let snapshot = transport.snapshot();
                    anyhow::ensure!(snapshot.state != State::Failed, "{}", snapshot.error.unwrap_or_default());
                    if let Some((id, parts)) = snapshot.plugins.checkpoint {
                        if Some(id) != last { break parts; }
                    }
                    anyhow::ensure!(Instant::now() < deadline, "Shutdown checkpoint timed out; previous saved state retained");
                    std::thread::sleep(Duration::from_millis(10));
                };
                for part in parts {
                    let Some(Source::Vst3 { class_id, component_state, controller_state, .. }) = mix.sources.get_mut(&part.part_id) else { anyhow::bail!("Unknown checkpoint part") };
                    anyhow::ensure!(*class_id == part.class_id, "Checkpoint class mismatch");
                    *component_state = part.component_state;
                    *controller_state = part.controller_state;
                }
                std::fs::create_dir_all(parent)?;
                let mut output = tempfile::NamedTempFile::new_in(parent)?;
                write_mix(&mut output, &mix)?;
                output.as_file().sync_all()?;
                // Never replace a newer explicit save or another session's manifest.
                let _guard = guard.lock().unwrap_or_else(|e| e.into_inner());
                let current = std::fs::read(&path).ok();
                anyhow::ensure!(current == previous, "Movement changed while saving shutdown state; previous/newer manifest retained. Save explicitly before switching");
                output.persist(&path)?;
                std::fs::File::open(parent)?.sync_all()?;
                Ok(())
            })().map_err(|e| format!("Unsaved plugin changes during shutdown: {e:#}"));
            let _ = tx.send(result);
            // Owned transport/process teardown is on this worker, never egui.
        });
    }
    #[cfg(target_os = "linux")]
    pub(super) fn poll_plugins(&mut self, paths: &Paths) {
        let was_retiring = !self.retiring.is_empty();
        self.retiring.retain(|rx| match rx.try_recv() {
            Ok(Ok(())) => false,
            Ok(Err(error)) => {
                self.shutdown_error = Some(error.clone());
                self.error = Some(error);
                false
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => false,
            Err(crossbeam_channel::TryRecvError::Empty) => true,
        });
        if was_retiring && self.retiring.is_empty() && self.playback.is_none() {
            if let Err(error) = self.refresh_saved_plugin_states() {
                self.error = Some(format!(
                    "Saved plugin checkpoint changed; reload movement: {error:#}"
                ));
            }
        }
        if self.catalogue_path.is_none() {
            let path = paths.data.join("plugins/catalogue-v2.json");
            match crate::audio::discovery::Catalogue::load(&path) {
                Ok(catalogue) => self.catalogue = catalogue,
                Err(error) => self.error = Some(format!("Plugin catalogue: {error:#}")),
            }
            self.catalogue_path = Some(path);
        }
        if let Some(result) = self
            .plugin_task
            .as_ref()
            .and_then(|t| t.result.try_recv().ok())
        {
            self.plugin_task = None;
            match result {
                Ok((catalogue, notice, capabilities)) => {
                    if let Some(caps) = capabilities {
                        self.helper_caps = Some(caps);
                    }
                    self.catalogue = catalogue;
                    self.notice = notice;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(snapshot) = self.playback.as_ref().map(Transport::snapshot) {
            if let Some(error) = snapshot.plugins.operation_error {
                self.error = Some(error);
            }
            if let Some((id, parts)) = snapshot.plugins.checkpoint {
                if id != self.checkpoint_id {
                    let mut updated = self.sources.clone();
                    for part in parts {
                        if let Some(Source::Vst3 {
                            class_id,
                            component_state,
                            controller_state,
                            ..
                        }) = updated.get_mut(&part.part_id)
                        {
                            if *class_id != part.class_id {
                                self.error =
                                    Some("Checkpoint class changed; state was not saved".into());
                                return;
                            }
                            *component_state = part.component_state;
                            *controller_state = part.controller_state;
                        }
                    }
                    let previous = std::mem::replace(&mut self.sources, updated);
                    self.mix_dirty = Some(std::time::Instant::now());
                    match self.save_mix() {
                        Ok(()) => {
                            self.checkpoint_id = id;
                            self.plugin_unsaved = false;
                            self.notice = "Plugin assignments and separate component/controller state saved. External samples/licenses are not bundled.".into();
                        }
                        Err(error) => {
                            self.sources = previous;
                            self.error = Some(format!("Checkpoint manifest save failed; previous saved state retained: {error:#}"));
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn start_plugin_task(
        &mut self,
        mode: &str,
        direct: Option<PathBuf>,
        retry: bool,
        ctx: &egui::Context,
    ) {
        if self.plugin_task.is_some() {
            return;
        }
        let settings = self.audio_settings.clone();
        let mut catalogue = self.catalogue.clone();
        let path = self.catalogue_path.clone();
        let mode = mode.to_owned();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancel.clone();
        let (tx, rx) = bounded(1);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<_> {
                if mode == "diagnostics" {
                    let report = crate::audio::yabridge::diagnostics(&settings.yabridge)?;
                    let message = format!("{} · Wine {:?} · bridge {:?}\n{}", report.summary, report.wine_version, report.bridge_version, report.warnings.join("\n"));
                    return Ok((catalogue, message, None));
                }
                let caps = crate::audio::discovery::probe(&settings, &stop)?;
                if mode == "probe" {
                    return Ok((catalogue, format!("Audio-host v2 capabilities: {}{}", caps.join(", "), if caps.iter().any(|c| c == "vst3") { "" } else { ". Built without VST3; rebuild the helper with the pinned SDK." }), Some(caps)));
                }
                anyhow::ensure!(caps.iter().any(|c| c == "vst3"), "Helper built without VST3. Rebuild with the pinned SDK before choosing a plugin");
                let candidates = match direct {
                    Some(path) => vec![path],
                    None => crate::audio::discovery::enumerate(&settings.plugin_roots, &stop)?,
                };
                for candidate in &candidates {
                    catalogue.scan(&settings, std::slice::from_ref(candidate), &stop, retry)?;
                    if let Some(path) = &path { catalogue.save(path)?; }
                }
                let failures = catalogue.entries.iter().filter(|e| e.failure.is_some()).count();
                Ok((catalogue, format!("Scanned {} candidate(s); {failures} quarantined. Choose the exact instrument class below. Retry is explicit.", candidates.len()), Some(caps)))
            })().map_err(|e| format!("Plugin operation: {e:#}"));
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.plugin_task = Some(Task { result: rx, cancel });
    }

    #[cfg(target_os = "linux")]
    pub(super) fn plugin_settings_ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut crate::audio::Settings,
    ) {
        ui.collapsing("VST3 discovery / existing yabridge diagnostics", |ui| {
            ui.label("No automatic startup scan. Loading/scanning executes third-party code in a private helper. Native Linux x86_64 VST3 instruments only.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Use standard Linux roots").clicked() {
                    let mut roots = vec![crate::picker::home_directory().join(".vst3"), PathBuf::from("/usr/lib/vst3"), PathBuf::from("/usr/local/lib/vst3"), PathBuf::from("/usr/lib64/vst3"), PathBuf::from("/usr/local/lib64/vst3")];
                    roots.retain(|p| p.is_dir());
                    settings.plugin_roots = roots.iter().map(|p| p.display().to_string()).collect();
                }
                if ui.button("Use existing yabridge wrappers").clicked() {
                    let root = if settings.yabridge.wrapper_root.is_empty() { crate::picker::home_directory().join(".vst3/yabridge").display().to_string() } else { settings.yabridge.wrapper_root.clone() };
                    if !settings.plugin_roots.contains(&root) && settings.plugin_roots.len() < 32 { settings.plugin_roots.push(root); }
                }
                if ui.add_enabled(settings.plugin_roots.len() < 32, egui::Button::new("Add search folder")).clicked() { settings.plugin_roots.push(String::new()); }
            });
            let mut remove = None;
            for (index, root) in settings.plugin_roots.iter_mut().enumerate() {
                if paths::entry(ui, PathField::PluginRoot(index as u32), root, !self.path_picker_active) {
                    self.request_path(PathField::PluginRoot(index as u32), root);
                }
                if ui.small_button(format!("Remove folder {}", index + 1)).clicked() { remove = Some(index); }
            }
            if let Some(index) = remove { settings.plugin_roots.remove(index); }
            self.configure_audio(settings);
            if self.plugin_task.is_some() {
                ui.spinner();
                if ui.button("Cancel plugin operation").clicked() { self.plugin_task.take(); }
            } else {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Probe helper capabilities").clicked() { self.start_plugin_task("probe", None, false, ui.ctx()); }
                    if ui.button("Read-only bridge diagnostics").clicked() { self.start_plugin_task("diagnostics", None, false, ui.ctx()); }
                    if ui.button("Scan selected roots").clicked() { self.start_plugin_task("scan", None, false, ui.ctx()); }
                    if ui.button("Retry quarantined / rescan").clicked() { self.start_plugin_task("scan", None, true, ui.ctx()); }
                });
            }
            for entry in &self.catalogue.entries {
                if let Some(error) = &entry.failure { ui.colored_label(Color32::YELLOW, format!("{} — quarantined: {error}", entry.path.display())); }
            }
        });
    }

    #[cfg(target_os = "linux")]
    pub(super) fn plugin_part_ui(&mut self, ui: &mut egui::Ui, id: &str, index: u32) {
        let mut source = self.sources.get(id).cloned().unwrap_or_default();
        let before = source.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label("Instrument source:");
            if ui
                .selectable_label(!source.is_plugin(), "SoundFont / GM")
                .clicked()
            {
                source = Source::Soundfont;
            }
            ui.label(if source.is_plugin() {
                "VST3"
            } else {
                "Choose a VST3 class below"
            });
        });
        if self.audio_settings.backend == crate::audio::Backend::Legacy {
            if source.is_plugin() {
                ui.colored_label(Color32::YELLOW, "VST3 assignment retained, but legacy playback is disabled until you explicitly choose SoundFont or an audio-host backend.");
            }
        } else {
            let path = self.bundle_paths.entry(id.into()).or_default();
            if path.is_empty() {
                if let Source::Vst3 { path: assigned, .. } = &source {
                    *path = assigned.display().to_string();
                }
            }
            ui.label("Direct .vst3 bundle folder (Browse only fills the path):");
            if paths::entry(
                ui,
                PathField::PluginBundle(index),
                path,
                !self.path_picker_active,
            ) {
                let path = path.clone();
                self.request_path(PathField::PluginBundle(index), &path);
            }
            if ui
                .add_enabled(
                    self.plugin_task.is_none(),
                    egui::Button::new("Inspect this bundle"),
                )
                .clicked()
            {
                let path = expand_path(self.bundle_paths[id].trim());
                self.start_plugin_task("scan", Some(path), false, ui.ctx());
            }
            let label = match &source {
                Source::Soundfont => "SoundFont (no plugin selected)".into(),
                Source::Vst3 { path, class_id, .. } => format!("{} · {}", path.display(), class_id),
            };
            if self
                .helper_caps
                .as_ref()
                .is_some_and(|caps| caps.iter().any(|c| c == "vst3"))
            {
                egui::ComboBox::from_id_salt(("plugin-class", id)).selected_text(label).width(420.0).show_ui(ui, |ui| {
                for entry in &self.catalogue.entries {
                    if entry.failure.is_some() { continue; }
                    for class in &entry.classes {
                        if ui.selectable_label(matches!(&source, Source::Vst3 { path, class_id, .. } if path == &entry.path && class_id == &class.class_id),
                            format!("{} · {} · {} · {}", class.name, class.vendor, entry.path.display(), class.class_id)).clicked()
                        {
                            source = source.select_class(entry.path.clone(), class.class_id.clone(), entry.module_sha256.clone());
                        }
                    }
                }
            });
            } else {
                ui.small("Probe/inspect with an SDK-enabled v2 helper before choosing an instrument. A core-only helper cannot load VST3.");
            }
        }
        if source != before && !self.can_replace_plugin_source() {
            self.error = Some("Pause and Save plugin assignments and state before changing an instrument. A previous session may still be saving; its checkpoint is retained.".into());
            source = before.clone();
        }
        if source != before {
            self.stop();
            self.generation = self.generation.wrapping_add(1);
            self.sources.insert(id.into(), source.clone());
            self.mix_dirty = Some(std::time::Instant::now());
            self.plugin_unsaved = true;
            self.notice = "Instrument assignment changed; press Play to rebuild. Save while paused/stopped to capture plugin state.".into();
        }
        if let Source::Vst3 { path, gain, .. } = &mut source {
            if !path.is_dir() {
                ui.colored_label(Color32::LIGHT_RED, "Missing plugin. Locate and inspect its bundle, then select the exact class. No SoundFont fallback is performed.");
            }
            if ui
                .add(egui::Slider::new(gain, 0.0..=1.0).text("Post-plugin audio gain"))
                .changed()
            {
                self.sources.insert(id.into(), source.clone());
                self.mix_dirty = Some(std::time::Instant::now());
                if self.playback.is_some() {
                    self.send_plugin(Request::Gain {
                        part_id: id.into(),
                        gain: match source {
                            Source::Vst3 { gain, .. } => gain,
                            _ => 1.0,
                        },
                    });
                }
            }
            self.plugin_controls_ui(ui, id, index);
        }
    }

    #[cfg(target_os = "linux")]
    fn send_plugin(&mut self, request: Request) {
        let result = self
            .playback
            .as_ref()
            .context("Start playback paused or stopped to configure the plugin")
            .and_then(|p| p.plugin(request));
        self.command_result(result);
    }

    #[cfg(target_os = "linux")]
    fn plugin_controls_ui(&mut self, ui: &mut egui::Ui, id: &str, index: u32) {
        let snapshot = self.playback.as_ref().map(Transport::snapshot);
        let active = snapshot
            .as_ref()
            .is_some_and(|s| !matches!(s.state, State::Starting | State::Failed));
        let paused = snapshot
            .as_ref()
            .is_some_and(|s| matches!(s.state, State::Paused | State::Stopped | State::Finished));
        let editor = snapshot
            .as_ref()
            .is_some_and(|s| s.plugins.capabilities.iter().any(|c| c == "editor_x11"));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(active && editor, egui::Button::new("Open editor"))
                .clicked()
            {
                self.send_plugin(Request::Editor {
                    part_id: id.into(),
                    open: true,
                });
            }
            if ui
                .add_enabled(active && editor, egui::Button::new("Close editor"))
                .clicked()
            {
                self.send_plugin(Request::Editor {
                    part_id: id.into(),
                    open: false,
                });
            }
            if ui
                .add_enabled(active, egui::Button::new("Generic parameters"))
                .clicked()
            {
                self.send_plugin(Request::Parameters {
                    part_id: id.into(),
                    offset: 0,
                });
            }
        });
        if !editor {
            ui.small("Custom editor unavailable in this helper/display. Use generic parameters or a class-matching preset.");
        }
        let preset = self.preset_paths.entry(id.into()).or_default();
        if paths::entry(
            ui,
            PathField::PluginPreset(index),
            preset,
            !self.path_picker_active,
        ) {
            let path = preset.clone();
            self.request_path(PathField::PluginPreset(index), &path);
        }
        if ui
            .add_enabled(paused, egui::Button::new("Load selected .vstpreset"))
            .clicked()
        {
            self.plugin_unsaved = true;
            self.send_plugin(Request::Preset {
                part_id: id.into(),
                path: expand_path(self.preset_paths[id].trim()),
            });
        }
        if let Some((total, offset, parameters)) = snapshot
            .as_ref()
            .and_then(|s| s.plugins.parameters.get(id))
            .cloned()
        {
            ui.label(format!(
                "Parameters {}–{} of {total}",
                offset + 1,
                offset + parameters.len() as u32
            ));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(offset > 0, egui::Button::new("Previous parameters"))
                    .clicked()
                {
                    self.send_plugin(Request::Parameters {
                        part_id: id.into(),
                        offset: offset.saturating_sub(128),
                    });
                }
                if ui
                    .add_enabled(offset + 128 < total, egui::Button::new("Next parameters"))
                    .clicked()
                {
                    self.send_plugin(Request::Parameters {
                        part_id: id.into(),
                        offset: offset + 128,
                    });
                }
            });
            for mut parameter in parameters {
                // SDK ParameterInfo::kIsReadOnly == 1 << 1.
                let mut slider = egui::Slider::new(&mut parameter.value, 0.0..=1.0)
                    .text(format!("{} {}", parameter.title, parameter.units));
                if parameter.step_count > 0 {
                    slider = slider.step_by(1.0 / f64::from(parameter.step_count));
                }
                if ui.add_enabled(parameter.flags & 2 == 0, slider).changed() {
                    self.plugin_unsaved = true;
                    self.send_plugin(Request::Parameter {
                        part_id: id.into(),
                        param_id: parameter.id,
                        value: parameter.value,
                    });
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn plugin_save_ui(&mut self, ui: &mut egui::Ui) {
        if !self.sources.values().any(Source::is_plugin) {
            return;
        }
        let snapshot = self.playback.as_ref().map(Transport::snapshot);
        if self.plugin_unsaved
            || snapshot
                .as_ref()
                .is_some_and(|s| !s.plugins.dirty_parts.is_empty())
        {
            ui.colored_label(
                Color32::YELLOW,
                "Unsaved plugin changes. Pause/stop and Save before switching score or closing.",
            );
        }
        let can_save = snapshot.as_ref().is_some_and(|s| {
            !s.plugins.busy && matches!(s.state, State::Paused | State::Stopped | State::Finished)
        });
        if snapshot.as_ref().is_some_and(|s| s.plugins.busy) {
            ui.spinner();
            ui.small("Plugin state operation in progress. Stop audio cancels it and retains the last valid checkpoint.");
        }
        if ui
            .add_enabled(
                can_save,
                egui::Button::new("Save plugin assignments and state"),
            )
            .clicked()
        {
            if let Some(path) = &self.mix_path {
                let destination = path.parent().unwrap().join("states");
                self.send_plugin(Request::Checkpoint { destination });
            }
        }
        ui.small("MIDI export uses the retained GM selections; it does not contain plugin sounds or opaque state.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_or_retiring_plugin_sessions_refuse_structural_replacement() {
        let root = tempfile::tempdir().unwrap();
        let (transport, sources, _) =
            crate::audio::supervisor::tests::plugin_session(root.path(), "normal");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while transport.snapshot().state != State::Stopped {
            assert_ne!(transport.snapshot().state, State::Failed);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut music = Music {
            sources,
            playback: Some(transport),
            ..Default::default()
        };
        assert!(music.can_replace_plugin_source());
        music.plugin_unsaved = true;
        assert!(!music.can_replace_plugin_source());
        music.plugin_unsaved = false;
        music
            .playback
            .as_ref()
            .unwrap()
            .plugin(Request::Parameter {
                part_id: "P1".into(),
                param_id: 0,
                value: 0.3,
            })
            .unwrap();
        while music
            .playback
            .as_ref()
            .unwrap()
            .snapshot()
            .plugins
            .dirty_parts
            .is_empty()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!music.can_replace_plugin_source());
        drop(music.playback.take());
        let (_tx, rx) = bounded(1);
        music.retiring.push(rx);
        assert!(!music.can_replace_plugin_source());
    }
}
