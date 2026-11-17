use crate::{
    app::expand_path,
    model::{Paths, Score},
    notation::{self, Composition, MidiOptions},
    omr::{RecognitionArtifacts, RecognitionJob, RecognitionUpdate, Settings},
    playback::{State, Transport},
    text_input::TextEditFocusExt,
};
use anyhow::{bail, Context, Result};
use crossbeam_channel::{bounded, Receiver};
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
mod paths;
#[cfg(target_os = "linux")]
mod plugins;
pub(crate) use paths::{PathField, PathRequest};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_EXPORT: u64 = 64 * 1024 * 1024;
const MAX_MIX: u64 = 1024 * 1024;

fn write_mix(output: &mut impl Write, mix: &SavedMix) -> Result<()> {
    struct Bounded<'a, W>(&'a mut W, usize);
    impl<W: Write> Write for Bounded<'_, W> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.1 {
                return Err(std::io::Error::other("Saved audio mix exceeds 1 MiB"));
            }
            let count = self.0.write(bytes)?;
            self.1 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
    serde_json::to_writer(Bounded(output, MAX_MIX as usize), mix)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct SavedResults {
    hash: String,
    movements: Vec<PathBuf>,
}

fn midi_volume(percent: u8) -> u8 {
    ((u16::from(percent.min(100)) * 127 + 50) / 100) as u8
}
fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn full_volume() -> u8 {
    100
}

#[derive(Clone, Serialize, Deserialize)]
struct SavedMix {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    score_hash: Option<String>,
    #[serde(default)]
    music_hash: Option<String>,
    #[serde(default)]
    sources: BTreeMap<String, crate::audio::plugins::Source>,
    selected: BTreeMap<String, bool>,
    programs: BTreeMap<String, u8>,
    volumes: BTreeMap<String, u8>,
    tempo_scale: f64,
    #[serde(default = "full_volume")]
    master_volume: u8,
    #[serde(default)]
    follow_pages: bool,
    #[serde(default)]
    turn_lead_measures: u16,
}

struct Loaded {
    composition: Composition,
    path: PathBuf,
    source_stamp: SourceStamp,
    music_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceStamp {
    size: u64,
    modified: std::time::SystemTime,
}

fn source_stamp(path: &Path) -> Result<SourceStamp> {
    let metadata = std::fs::metadata(path)?;
    anyhow::ensure!(metadata.is_file(), "Backing score is not a regular file");
    Ok(SourceStamp {
        size: metadata.len(),
        modified: metadata.modified()?,
    })
}
#[derive(Default)]
pub struct Music {
    pub open: bool,
    binding: Option<(i64, String)>,
    source: Option<PathBuf>,
    source_stamp: Option<SourceStamp>,
    job: Option<RecognitionJob>,
    loading: Option<Receiver<Result<Loaded, String>>>,
    playback: Option<Transport>,
    synth_settings: Option<(String, String)>,
    audio_settings: crate::audio::Settings,
    composition: Option<Composition>,
    xml: Option<PathBuf>,
    movements: Vec<PathBuf>,
    movement: usize,
    selected: BTreeMap<String, bool>,
    sources: BTreeMap<String, crate::audio::plugins::Source>,
    discovery_bundle: String,
    preset_paths: BTreeMap<String, String>,
    generation: u64,
    music_hash: Option<String>,
    checkpoint_id: u64,
    plugin_unsaved: bool,
    #[cfg(target_os = "linux")]
    shutdown_error: Option<String>,
    #[cfg(target_os = "linux")]
    catalogue: crate::audio::discovery::Catalogue,
    #[cfg(target_os = "linux")]
    catalogue_path: Option<PathBuf>,
    #[cfg(target_os = "linux")]
    plugin_task: Option<plugins::Task>,
    #[cfg(target_os = "linux")]
    helper_caps: Option<Vec<String>>,
    #[cfg(target_os = "linux")]
    discovery_notice: String,
    #[cfg(target_os = "linux")]
    discovery_error: Option<String>,
    #[cfg(target_os = "linux")]
    retiring: Vec<Receiver<Result<(), String>>>,
    manifest_guard: std::sync::Arc<std::sync::Mutex<()>>,
    programs: BTreeMap<String, u8>,
    volumes: BTreeMap<String, u8>,
    mix_path: Option<PathBuf>,
    legacy_mix_path: Option<PathBuf>,
    mix_dirty: Option<std::time::Instant>,
    pub controls_active: bool,
    tempo_scale: f64,
    master_volume: u8,
    follow_pages: bool,
    source_pages: u32,
    turn_lead_measures: u16,
    path_request: Option<PathRequest>,
    pub(crate) path_picker_active: bool,
    load_path: String,
    xml_destination: String,
    midi_destination: String,
    notice: String,
    error: Option<String>,
}

impl Music {
    pub(crate) fn configure_audio(&mut self, settings: &crate::audio::Settings) {
        if &self.audio_settings != settings {
            let active = self.playback.is_some();
            self.stop();
            self.audio_settings = settings.clone();
            #[cfg(target_os = "linux")]
            {
                self.helper_caps = None;
                self.plugin_task.take();
            }
            if active {
                self.notice =
                    "Audio backend changed. Press Play to start the selected engine.".into();
            }
        }
    }

    pub fn open_for(&mut self, score: &Score, source: &Path, paths: &Paths, ctx: &egui::Context) {
        self.open = true;
        self.prepare_for(score, source, paths, ctx);
    }

    fn page_follow_available(&self) -> bool {
        self.composition
            .as_ref()
            .and_then(|c| c.page_map.as_ref())
            .is_some_and(|map| {
                !map.turns.is_empty()
                    && !map.source_pages.is_empty()
                    && map
                        .source_pages
                        .iter()
                        .all(|page| *page < self.source_pages)
            })
    }

    fn page_follow_ui(&mut self, ui: &mut egui::Ui) {
        let available = self.page_follow_available();
        if ui.add_enabled(available, egui::Checkbox::new(&mut self.follow_pages, "Follow score pages"))
            .on_hover_text("Turn original score pages with the synth timeline, including repeats and seeks. Manual page turns disable following until you re-enable it.")
            .changed() {
            self.mix_dirty = Some(std::time::Instant::now());
        }
        if self.composition.is_some() && !available {
            ui.small("Automatic page turning unavailable: missing, conflicting or out-of-range original-page mapping. Re-recognize the original score; see warnings.");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Turn pages early by");
            let response=ui.add_enabled(available,egui::DragValue::new(&mut self.turn_lead_measures).range(0..=999));
            self.controls_active |= response.text_edit_has_focus() || response.is_pointer_button_down_on();
            if response.changed() {self.mix_dirty=Some(std::time::Instant::now());}
            ui.label("measures");
            response.on_hover_text("0 turns at the boundary. Counts played measures, including repeats. Lead is capped to avoid skipping an entire page.");
        });
    }

    pub fn manual_page_turn(&mut self) {
        if self.follow_pages {
            self.follow_pages = false;
            self.mix_dirty = Some(std::time::Instant::now());
            self.notice = "Automatic page turning disabled by manual navigation. Re-enable Follow score pages to resume.".into();
        }
    }

    pub fn follow_page(&self, score: &Score) -> Option<u32> {
        let snapshot = self.playback.as_ref()?.snapshot();
        self.follow_page_at(score, &snapshot)
    }

    fn follow_page_at(&self, score: &Score, snapshot: &crate::playback::Snapshot) -> Option<u32> {
        if !self.follow_pages
            || !self.page_follow_available()
            || self.binding.as_ref() != Some(&(score.id, score.hash.clone()))
            || self.source_pages != score.pages
            || !matches!(snapshot.state, State::Playing | State::Paused)
        {
            return None;
        }
        snapshot.source_page.filter(|page| *page < score.pages)?;
        self.composition
            .as_ref()?
            .page_map
            .as_ref()?
            .page_at(snapshot.score_tick, self.turn_lead_measures)
            .filter(|page| *page < score.pages)
    }

    pub fn prepare_for(
        &mut self,
        score: &Score,
        source: &Path,
        paths: &Paths,
        ctx: &egui::Context,
    ) {
        let binding = Some((score.id, score.hash.clone()));
        if self.binding != binding
            || self.source.as_deref() != Some(source)
            || self.source_pages != score.pages
        {
            self.reset();
            self.binding = binding;
            self.source = Some(source.to_owned());
            self.source_pages = score.pages;
            self.follow_pages = false;
            self.turn_lead_measures = 0;
            self.tempo_scale = 1.0;
            self.master_volume = 100;
            self.xml_destination.clear();
            self.midi_destination = paths
                .data
                .join("exports")
                .join(format!(
                    "{}-selection.mid",
                    &score.hash[..score.hash.len().min(12)]
                ))
                .display()
                .to_string();
            match read_results(paths, &score.hash) {
                Ok(Some(movements)) => {
                    self.movements = movements;
                    if self.movements.len() == 1 {
                        self.start_load(self.movements[0].clone(), false, paths, ctx);
                    } else {
                        self.notice =
                            "Choose a recognized movement and click Load movement.".into();
                    }
                }
                Ok(None) => {}
                Err(error) => self.error = Some(format!("Saved OMR results: {error:#}")),
            }
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        let _ = self.save_mix();
        self.stop();
        self.source = None;
        self.source_stamp = None;
        self.source_pages = 0;
        self.follow_pages = false;
        self.job.take();
        self.loading.take();
        self.composition = None;
        self.xml = None;
        self.movements.clear();
        self.movement = 0;
        self.selected.clear();
        self.sources.clear();
        self.preset_paths.clear();
        self.music_hash = None;
        self.plugin_unsaved = false;
        self.programs.clear();
        self.volumes.clear();
        self.mix_path = None;
        self.legacy_mix_path = None;
        self.mix_dirty = None;
        self.controls_active = false;
        self.notice.clear();
        self.error = None;
        self.load_path.clear();
    }

    pub fn invalidate_unless(&mut self, current: Option<(i64, String)>) {
        if self.binding.is_some() && self.binding != current {
            self.reset();
            self.binding = None;
            self.open = false;
        }
    }

    pub fn stop(&mut self) {
        if self
            .playback
            .as_ref()
            .is_some_and(|p| !p.snapshot().plugins.dirty_parts.is_empty())
        {
            self.notice = "Stopped with unsaved plugin edits. Last valid saved checkpoint retained; Save while paused before closing to preserve edits.".into();
        }
        if let Some(transport) = self.playback.take() {
            #[cfg(target_os = "linux")]
            self.retire_transport(transport);
        }
        self.checkpoint_id = 0;
        self.synth_settings = None;
    }

    pub fn poll(&mut self, paths: &Paths, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        self.poll_plugins(paths);
        #[cfg(target_os = "linux")]
        let saving_plugins = !self.retiring.is_empty();
        #[cfg(not(target_os = "linux"))]
        let saving_plugins = false;
        if self
            .mix_dirty
            .is_some_and(|at| at.elapsed() >= std::time::Duration::from_millis(250))
            && !saving_plugins
        {
            if let Err(error) = self.save_mix() {
                self.error = Some(format!("Saving part mix: {error:#}"));
            }
        }
        if let (Some(source), Some(stamp)) = (&self.source, &self.source_stamp) {
            if source_stamp(source).ok().as_ref() != Some(stamp) {
                self.stop();
                self.composition = None;
                self.xml = None;
                self.source_stamp = None;
                self.generation = self.generation.wrapping_add(1);
                self.error = Some("The backing score changed or is unavailable. Rescan and reopen Music / OMR before using these results.".into());
            }
        }
        let updates = self.job.as_mut().map(|job| job.poll()).unwrap_or_default();
        for update in updates {
            match update {
                RecognitionUpdate::Progress(message) => self.notice = message,
                RecognitionUpdate::Finished(result) => {
                    self.job.take();
                    match result {
                        Ok(artifacts) => {
                            if self.binding.as_ref().map(|b| &b.1) != Some(&artifacts.source_hash) {
                                continue;
                            }
                            if let Err(error) = save_results(paths, &artifacts) {
                                self.error = Some(format!("Saving result index: {error:#}"));
                            }
                            self.movements = artifacts.movements;
                            self.movement = 0;
                            self.notice = format!(
                                "Recognition produced {} movement(s). Review before relying on playback.",
                                self.movements.len()
                            );
                            if self.movements.len() == 1 {
                                self.start_load(self.movements[0].clone(), false, paths, ctx);
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        }
        if let Some(result) = self.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.loading = None;
            match result {
                Ok(loaded) => {
                    self.stop();
                    self.master_volume = 100;
                    self.follow_pages = false;
                    self.turn_lead_measures = 0;
                    self.tempo_scale = 1.0;
                    self.source_stamp = Some(loaded.source_stamp);
                    self.selected = loaded
                        .composition
                        .parts
                        .iter()
                        .map(|part| (part.id.clone(), true))
                        .collect();
                    self.volumes = loaded
                        .composition
                        .parts
                        .iter()
                        .map(|part| (part.id.clone(), 100))
                        .collect();
                    self.legacy_mix_path = self.binding.as_ref().map(|(_, hash)| {
                        paths
                            .data
                            .join("omr-mix")
                            .join(hash)
                            .join(format!("{}.json", loaded.music_hash))
                    });
                    self.mix_path = self.legacy_mix_path.as_ref().map(|path| {
                        path.with_file_name(format!("{}.audio-v2.json", loaded.music_hash))
                    });
                    self.programs = loaded
                        .composition
                        .parts
                        .iter()
                        .map(|part| (part.id.clone(), part.program))
                        .collect();
                    self.error = None;
                    self.sources.clear();
                    self.plugin_unsaved = false;
                    self.preset_paths.clear();
                    self.music_hash = Some(loaded.music_hash.clone());
                    if let Err(error) = self.load_mix() {
                        self.error = Some(format!("Saved part mix ignored: {error:#}"));
                    }
                    let extension = loaded
                        .path
                        .extension()
                        .and_then(|v| v.to_str())
                        .unwrap_or("mxl");
                    self.xml_destination = paths
                        .data
                        .join("exports")
                        .join(format!("recognized-score.{extension}"))
                        .display()
                        .to_string();
                    self.xml = Some(loaded.path);
                    self.composition = Some(loaded.composition);
                    if !self.page_follow_available() {
                        self.follow_pages = false;
                    }
                    self.notice =
                        "Music loaded. Select parts and instruments; recognition is unverified."
                            .into();
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(snapshot) = self.playback.as_ref().map(Transport::snapshot) {
            if snapshot.state == State::Failed {
                self.stop();
                self.error = snapshot.error;
            }
        }
    }

    fn start_load(&mut self, path: PathBuf, copy: bool, paths: &Paths, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        if let Err(error) = self.save_mix() {
            self.error = Some(format!("Saving previous movement's mix: {error:#}"));
            return;
        }
        self.stop();
        self.generation = self.generation.wrapping_add(1);
        let root = paths.data.join("omr-corrected");
        let Some((_, hash)) = &self.binding else {
            return;
        };
        let Some(source) = self.source.clone() else {
            return;
        };
        let hash = hash.clone();
        let (tx, rx) = bounded(1);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Loaded> {
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let stamp = source_stamp(&source)?;
                anyhow::ensure!(
                    crate::folder::hash_file(&source, &cancel)? == hash,
                    "Backing score changed; rescan before loading notation"
                );
                let path = if copy {
                    std::fs::create_dir_all(&root)?;
                    let extension = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_ascii_lowercase)
                        .filter(|e| ["mxl", "xml", "musicxml"].contains(&e.as_str()))
                        .context("Choose a .mxl, .musicxml, or .xml file")?;
                    let mut file = tempfile::Builder::new()
                        .suffix(&format!(".{extension}"))
                        .tempfile_in(root)?;
                    let mut input = std::fs::File::open(&path)?;
                    let copied = std::io::copy(
                        &mut Read::by_ref(&mut input).take(MAX_EXPORT + 1),
                        &mut file,
                    )?;
                    anyhow::ensure!(
                        copied <= MAX_EXPORT,
                        "MusicXML exceeds the 64 MiB size limit"
                    );
                    file.flush()?;
                    // Parse the private copy before keeping it, not a mutable external path.
                    let composition = notation::load(file.path())?;
                    let music_hash = crate::folder::hash_file(file.path(), &cancel)?;
                    anyhow::ensure!(
                        source_stamp(&source)? == stamp
                            && crate::folder::hash_file(&source, &cancel)? == hash,
                        "Backing score changed while loading notation"
                    );
                    let (_, path) = file.keep()?;
                    return Ok(Loaded {
                        composition,
                        path,
                        source_stamp: stamp,
                        music_hash,
                    });
                } else {
                    path
                };
                let composition = notation::load(&path)?;
                let music_hash = crate::folder::hash_file(&path, &cancel)?;
                anyhow::ensure!(
                    source_stamp(&source)? == stamp
                        && crate::folder::hash_file(&source, &cancel)? == hash,
                    "Backing score changed while loading notation"
                );
                Ok(Loaded {
                    composition,
                    path,
                    source_stamp: stamp,
                    music_hash,
                })
            })();
            let _ = tx.send(result.map_err(|error| format!("Loading MusicXML: {error:#}")));
            repaint.request_repaint();
        });
        self.loading = Some(rx);
        self.notice = "Loading musical notation…".into();
    }

    fn options(&self) -> MidiOptions {
        MidiOptions {
            selected_parts: Some(
                self.selected
                    .iter()
                    .filter(|(_, on)| **on)
                    .map(|(id, _)| id.clone())
                    .collect(),
            ),
            programs: self.programs.clone(),
            volumes: self
                .volumes
                .iter()
                .map(|(id, percent)| (id.clone(), midi_volume(*percent)))
                .collect(),
            tempo_scale: self.tempo_scale,
        }
    }

    fn midi(&self) -> Result<Vec<u8>> {
        notation::to_midi(
            self.composition
                .as_ref()
                .context("Load musical notation first")?,
            &self.options(),
        )
    }

    fn save_mix(&mut self) -> Result<()> {
        if self.mix_dirty.is_none() {
            return Ok(());
        }
        let Some(path) = &self.mix_path else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            self.retiring.is_empty(),
            "A previous plugin session is saving; retry after it completes"
        );
        let guard = self.manifest_guard.clone();
        let _guard = guard.try_lock().map_err(|_| {
            anyhow::anyhow!("A shutdown checkpoint is committing this movement; retry Save")
        })?;
        std::fs::create_dir_all(path.parent().context("Missing mix directory")?)?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        write_mix(
            &mut file,
            &SavedMix {
                version: 2,
                score_hash: self.binding.as_ref().map(|(_, hash)| hash.clone()),
                music_hash: self.music_hash.clone(),
                sources: self.sources.clone(),
                selected: self.selected.clone(),
                programs: self.programs.clone(),
                volumes: self.volumes.clone(),
                tempo_scale: self.tempo_scale,
                master_volume: self.master_volume,
                follow_pages: self.follow_pages,
                turn_lead_measures: self.turn_lead_measures,
            },
        )?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        #[cfg(target_os = "linux")]
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
        self.mix_dirty = None;
        Ok(())
    }

    fn load_mix(&mut self) -> Result<()> {
        let Some(path) = &self.mix_path else {
            return Ok(());
        };
        let path = if path.exists() {
            path
        } else if let Some(legacy) = &self.legacy_mix_path {
            if !legacy.exists() {
                return Ok(());
            }
            legacy
        } else {
            return Ok(());
        };
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(file.metadata()?.len() <= MAX_MIX, "Oversized saved mix");
        let mix: SavedMix = serde_json::from_reader(file.take(MAX_MIX + 1))?;
        anyhow::ensure!(
            mix.version <= 2 && mix.sources.len() <= 32,
            "Unsupported saved mix version or too many assignments"
        );
        if !mix.sources.is_empty() {
            anyhow::ensure!(
                mix.version == 2
                    && mix.score_hash == self.binding.as_ref().map(|(_, h)| h.clone())
                    && mix.music_hash == self.music_hash,
                "Plugin assignments belong to different score/MusicXML; reconcile explicitly"
            );
            anyhow::ensure!(
                mix.sources.keys().all(|id| self.selected.contains_key(id)),
                "Plugin assignment references an unknown part; reconcile explicitly"
            );
            for source in mix.sources.values() {
                source.validate()?;
            }
        }
        anyhow::ensure!(
            mix.selected.len() <= 128
                && mix.programs.len() <= 128
                && mix.volumes.len() <= 128
                && mix.programs.values().all(|v| *v <= 127)
                && mix.volumes.values().all(|v| *v <= 100)
                && mix.tempo_scale.is_finite()
                && (0.1..=4.0).contains(&mix.tempo_scale),
            "Invalid saved mix"
        );
        anyhow::ensure!(mix.master_volume <= 100, "Invalid saved master volume");
        anyhow::ensure!(mix.turn_lead_measures <= 999, "Invalid page-turn lead");
        for (id, value) in mix.selected {
            if let Some(current) = self.selected.get_mut(&id) {
                *current = value;
            }
        }
        for (id, value) in mix.programs {
            if let Some(current) = self.programs.get_mut(&id) {
                *current = value;
            }
        }
        for (id, value) in mix.volumes {
            if let Some(current) = self.volumes.get_mut(&id) {
                *current = value;
            }
        }
        self.tempo_scale = mix.tempo_scale;
        self.master_volume = mix.master_volume;
        self.follow_pages = mix.follow_pages;
        self.turn_lead_measures = mix.turn_lead_measures;
        self.sources = mix.sources;
        Ok(())
    }

    fn command_result(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.error = Some(format!("Playback: {error:#}"));
        }
    }

    fn recognize(
        &mut self,
        score: &Score,
        source: &Path,
        paths: &Paths,
        settings: &Settings,
        ctx: &egui::Context,
    ) {
        self.stop();
        let repaint = ctx.clone();
        match RecognitionJob::start(
            source.to_owned(),
            score.hash.clone(),
            paths.data.clone(),
            settings.clone(),
            move || repaint.request_repaint(),
        ) {
            Ok(job) => {
                self.job = Some(job);
                self.notice = "Recognizing music…".into();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn cancel_plugin_operation(&mut self) {
        drop(self.playback.take());
        self.checkpoint_id = 0;
        self.synth_settings = None;
        self.notice = "Plugin operation cancelled. Last valid saved checkpoint retained; unsaved edits may be lost.".into();
    }

    fn start_transport(&mut self, settings: &Settings) -> Result<()> {
        self.start_transport_mode(settings, true)
    }
    fn start_transport_mode(&mut self, settings: &Settings, play: bool) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            anyhow::ensure!(
                self.retiring.is_empty(),
                "Finishing previous plugin checkpoint; retry Play when saving completes"
            );
            self.refresh_saved_plugin_states()?;
        }
        let score = self
            .composition
            .as_ref()
            .context("Recognize or load music first")?
            .clone();
        self.playback = Some(Transport::start_with_sources(
            settings.clone(),
            self.audio_settings.clone(),
            score,
            self.options(),
            play,
            self.master_volume,
            self.sources.clone(),
        )?);
        self.synth_settings = Some((
            settings.fluidsynth_executable.clone(),
            settings.soundfont.clone(),
        ));
        self.error = None;
        Ok(())
    }

    fn transport_ui(&mut self, ui: &mut egui::Ui, settings: &Settings, compact: bool) {
        let engine = (
            settings.fluidsynth_executable.clone(),
            settings.soundfont.clone(),
        );
        if self.playback.is_some() && self.synth_settings.as_ref() != Some(&engine) {
            self.stop();
            self.notice =
                "Synth engine settings changed. Press Play to load the new SoundFont/engine."
                    .into();
        }
        let snapshot = self.playback.as_ref().map(Transport::snapshot);
        let state = snapshot.as_ref().map(|s| s.state);
        let ready = self.composition.is_some()
            && self.loading.is_none()
            && self.job.is_none()
            && !snapshot.as_ref().is_some_and(|s| s.plugins.busy);
        let seekable =
            ready && state != Some(State::Starting) && self.selected.values().any(|on| *on);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(seekable, egui::Button::new("Rewind 5s"))
                .clicked()
            {
                let result = self.seek_transport(settings, -5.0);
                self.command_result(result);
            }
            let playing = state == Some(State::Playing);
            if self.sources.values().any(|s| s.is_plugin())
                && self.playback.is_none()
                && ui.button("Load instruments paused").clicked()
            {
                let result = self.start_transport_mode(settings, false);
                self.command_result(result);
            }
            let label = if playing {
                "Pause"
            } else if compact {
                "Play"
            } else {
                "Play selected parts"
            };
            if ui
                .add_enabled(
                    ready
                        && state != Some(State::Starting)
                        && (playing || self.selected.values().any(|on| *on)),
                    egui::Button::new(label),
                )
                .clicked()
            {
                let result = if matches!(state, Some(State::Failed)) {
                    self.stop();
                    self.start_transport(settings)
                } else if let Some(transport) = &self.playback {
                    if playing {
                        transport.pause()
                    } else {
                        transport.play()
                    }
                } else {
                    self.start_transport(settings)
                };
                self.command_result(result);
            }
            if ui
                .add_enabled(seekable, egui::Button::new("Fast-forward 5s"))
                .clicked()
            {
                let result = self.seek_transport(settings, 5.0);
                self.command_result(result);
            }
            if ui
                .add_enabled(
                    self.playback.is_some(),
                    egui::Button::new(if compact { "Stop" } else { "Stop audio" }),
                )
                .clicked()
            {
                if snapshot.as_ref().is_some_and(|s| s.plugins.busy) {
                    self.cancel_plugin_operation();
                } else if state == Some(State::Starting) {
                    self.stop();
                } else {
                    let result = self.playback.as_ref().unwrap().stop();
                    self.command_result(result);
                }
            }
            if let Some(snapshot) = snapshot {
                ui.label(format!(
                    "{} / {} · {:?}",
                    clock(snapshot.position),
                    clock(snapshot.duration),
                    snapshot.state
                ));
                if let Some(audio) = snapshot.audio {
                    ui.small(format!(
                        "{} Hz · estimated output latency {:.1} ms · {} xruns",
                        audio.sample_rate,
                        audio.latency_frames as f64 * 1000.0 / f64::from(audio.sample_rate),
                        audio.xruns,
                    ));
                }
            }
        });
        self.master_volume_ui(ui, ready);
        #[cfg(target_os = "linux")]
        self.plugin_save_ui(ui);
        self.page_follow_ui(ui);
    }

    fn master_volume_ui(&mut self, ui: &mut egui::Ui, ready: bool) {
        if ui.add_enabled(ready, egui::Slider::new(&mut self.master_volume, 0..=100)
            .text("Master volume %"))
            .on_hover_text("Controls all instruments together without changing their individual levels. Playback output only.")
            .changed() {
            self.mix_dirty = Some(std::time::Instant::now());
            if let Some(transport) = &self.playback {
                let result = transport.master_volume(self.master_volume);
                self.command_result(result);
            }
        }
    }

    fn seek_transport(&mut self, settings: &Settings, seconds: f64) -> Result<()> {
        if self.playback.is_none() {
            self.start_transport_mode(settings, false)?;
        }
        self.playback.as_ref().unwrap().seek(seconds)
    }

    fn mixer_ui(&mut self, ui: &mut egui::Ui) {
        let parts: Vec<_> = self
            .composition
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .map(|p| {
                        (
                            p.id.clone(),
                            p.name.clone(),
                            p.staves,
                            p.notes.len(),
                            p.program,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (index, (id, name, staves, notes, default_program)) in parts.into_iter().enumerate() {
            let mut changed = false;
            ui.push_id(&id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .checkbox(self.selected.entry(id.clone()).or_insert(true), &name)
                        .changed();
                    ui.label(format!("{staves} staff/staves · {notes} notes"));
                });
                #[cfg(target_os = "linux")]
                self.plugin_part_ui(ui, &id, index as u32);
                ui.add_enabled_ui(
                    !self.sources.get(&id).is_some_and(|s| s.is_plugin()),
                    |ui| {
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    self.volumes.entry(id.clone()).or_insert(100),
                                    0..=100,
                                )
                                .text("Volume %"),
                            )
                            .changed();
                        let program = self.programs.entry(id.clone()).or_insert(default_program);
                        ui.horizontal_wrapped(|ui| {
                            let mut display = u16::from(*program) + 1;
                            ui.label("GM instrument:");
                            if ui
                                .add(egui::DragValue::new(&mut display).range(1..=128))
                                .changed()
                            {
                                *program = (display - 1) as u8;
                                changed = true;
                            }
                            egui::ComboBox::from_id_salt("instrument")
                                .selected_text(program_label(*program))
                                .show_ui(ui, |ui| {
                                    for (value, name) in [
                                        (0, "Piano"),
                                        (40, "Violin"),
                                        (41, "Viola"),
                                        (42, "Cello"),
                                        (43, "Contrabass"),
                                        (48, "Strings"),
                                        (73, "Flute"),
                                    ] {
                                        changed |=
                                            ui.selectable_value(program, value, name).changed();
                                    }
                                });
                        });
                    },
                );
            });
            if changed {
                self.mix_dirty = Some(std::time::Instant::now());
                if let Some(transport) = &self.playback {
                    let result = transport.mix(
                        &id,
                        self.selected[&id],
                        midi_volume(self.volumes[&id]),
                        self.programs[&id],
                    );
                    self.command_result(result);
                }
            }
        }
        if ui
            .add(egui::Slider::new(&mut self.tempo_scale, 0.25..=2.0).text("Tempo multiplier"))
            .changed()
        {
            self.mix_dirty = Some(std::time::Instant::now());
            if let Some(transport) = &self.playback {
                let result = transport.tempo(self.tempo_scale);
                self.command_result(result);
            }
        }
        ui.label("Part choices, volumes and instruments are saved for this recognized movement. Changes apply live.");
    }

    pub fn performance_controls(
        &mut self,
        ui: &mut egui::Ui,
        score: &Score,
        source: &Path,
        paths: &Paths,
        settings: &Settings,
        busy: bool,
    ) {
        self.prepare_for(score, source, paths, ui.ctx());
        ui.horizontal_wrapped(|ui| {
            ui.strong("Accompaniment");
            if self.composition.is_none()
                && self.job.is_none()
                && self.loading.is_none()
                && (self.movements.is_empty() || self.error.is_some())
                && ui
                    .add_enabled(
                        !busy && cfg!(target_os = "linux"),
                        egui::Button::new("Recognize music"),
                    )
                    .clicked()
            {
                self.recognize(score, source, paths, settings, ui.ctx());
            }
            if let Some(job) = &self.job {
                ui.spinner();
                ui.label("Recognizing…");
                if ui.button("Cancel recognition").clicked() {
                    job.cancel();
                }
            } else if self.loading.is_some() {
                ui.spinner();
                ui.label("Loading recognized music…");
            } else if self.composition.is_some() {
                ui.label("Recognized · unverified");
            }
            let response = ui.menu_button("Parts / volume", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| self.mixer_ui(ui));
            });
            self.controls_active = response.inner.is_some();
            if ui.button("Music settings / results").clicked() {
                self.open = true;
            }
        });
        if self.movements.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("performance_movement")
                    .selected_text(format!("Movement {}", self.movement + 1))
                    .show_ui(ui, |ui| {
                        for (index, path) in self.movements.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.movement,
                                index,
                                path.file_name().unwrap_or_default().to_string_lossy(),
                            );
                        }
                    });
                if ui
                    .add_enabled(
                        self.loading.is_none() && self.job.is_none(),
                        egui::Button::new("Load movement"),
                    )
                    .clicked()
                {
                    if let Some(path) = self.movements.get(self.movement) {
                        self.start_load(path.clone(), false, paths, ui.ctx());
                    }
                }
            });
        }
        self.transport_ui(ui, settings, true);
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    /// Returns true when the caller should persist edited settings.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        score: Option<(&Score, &Path)>,
        paths: &Paths,
        settings: &mut Settings,
        audio_settings: &mut crate::audio::Settings,
        library_busy: bool,
    ) -> bool {
        self.configure_audio(audio_settings);
        if !self.open {
            return false;
        }
        let mut open = self.open;
        let mut save_settings = false;
        egui::Window::new("Music / OMR")
            .open(&mut open)
            .default_size([620.0, 560.0])
            .min_width(280.0)
            .max_width(ctx.screen_rect().width().max(280.0) - 16.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                if !cfg!(target_os = "linux") {
                    ui.label("Audiveris recognition and FluidSynth playback are currently Linux-only.");
                    return;
                }
                let Some((score, source)) = score else {
                    ui.label("Open a library score first.");
                    return;
                };
                ui.heading(&score.title);
                ui.colored_label(Color32::YELLOW, "OMR output is unverified. Review notes, rhythm, repeats, and part grouping.");
                ui.label("Original scores and library metadata are never rewritten. Legacy FluidSynth remains the default; VST3 instruments require an SDK-enabled audio-host helper.");
                ui.collapsing("Engine settings", |ui| {
                    if let Some(field)=settings_ui(ui, settings, self.path_picker_active) {
                        self.browse_settings(field,settings);
                    }
                    if let Some(field) = audio_settings_ui(ui, audio_settings, self.path_picker_active) {
                        self.browse_audio_setting(field, audio_settings);
                    }
                    #[cfg(target_os = "linux")]
                    self.plugin_discovery_ui(ui, audio_settings);
                    save_settings = ui.button("Save music settings").clicked();
                });
                self.configure_audio(audio_settings);
                let idle = self.job.is_none() && self.loading.is_none() && !library_busy && !self.path_picker_active;
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(idle, egui::Button::new("Recognize with Audiveris")).clicked() {
                        self.stop();
                        let repaint = ctx.clone();
                        match RecognitionJob::start(
                            source.to_owned(), score.hash.clone(), paths.data.clone(),
                            settings.clone(), move || repaint.request_repaint(),
                        ) {
                            Ok(job) => {
                                self.job = Some(job);
                                self.notice = "Starting Audiveris… This can take several minutes.".into();
                                self.error = None;
                            }
                            Err(error) => self.error = Some(format!("{error:#}")),
                        }
                    }
                    if let Some(job) = &self.job {
                        ui.spinner();
                        if ui.button("Cancel recognition").clicked() {
                            job.cancel();
                        }
                    }
                    if self.loading.is_some() {
                        ui.spinner();
                    }
                });
                if library_busy {
                    ui.label("Wait for import, folder movement, or rescan before recognizing.");
                }
                if !self.notice.is_empty() {
                    ui.label(&self.notice);
                }
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                if self.movements.len() > 1 {
                    egui::ComboBox::from_id_salt("omr_movement")
                        .selected_text(format!("Movement {}", self.movement + 1))
                        .show_ui(ui, |ui| {
                            for (index, path) in self.movements.iter().enumerate() {
                                ui.selectable_value(&mut self.movement, index, path.file_name().unwrap_or_default().to_string_lossy());
                            }
                        });
                    if ui.add_enabled(idle, egui::Button::new("Load movement")).clicked() {
                        if let Some(path) = self.movements.get(self.movement) {
                            self.start_load(path.clone(), false, paths, ctx);
                        }
                    }
                }
                ui.collapsing("Load corrected or existing MusicXML", |ui| {
                    ui.label("Choose or paste a .mxl / .musicxml / .xml path. A private copy is loaded; the original is untouched.");
                    if paths::entry(ui,PathField::LoadMusicXml,&mut self.load_path,idle) {
                        self.request_path(PathField::LoadMusicXml,&self.load_path.clone());
                    }
                    if ui.add_enabled(idle && !self.load_path.trim().is_empty(), egui::Button::new("Load MusicXML")).clicked() {
                        self.start_load(expand_path(self.load_path.trim()), true, paths, ctx);
                    }
                });
                let Some(composition) = &self.composition else { return; };
                ui.separator();
                ui.heading(if composition.title.is_empty() { "Recognized parts" } else { &composition.title });
                ui.label(format!("{} part(s) · {} quarter-note beats including expanded repeats",
                    composition.parts.len(), composition.duration_ticks as f64 / f64::from(notation::TICKS_PER_QUARTER)));
                if composition.tempos.is_empty() {
                    ui.label("No recognized tempo: playback defaults to 120 BPM before scaling.");
                } else {
                    ui.label(format!("Initial playback tempo: {:.1} BPM", composition.tempos[0].bpm * self.tempo_scale));
                }
                ui.collapsing("Playback limitations and recognition warnings", |ui| {
                    for warning in &composition.warnings {
                        ui.colored_label(Color32::YELLOW, warning);
                    }
                    ui.label("Pause/resume, ±5-second seeking and optional Follow score pages are available below. Page following needs consistent original-page metadata. No microphone-based score tracking or metronome. Plugin state excludes external sample libraries and licenses. MusicXML preserves notation; synth playback is an approximation.");
                });
                self.mixer_ui(ui);
                self.transport_ui(ui,settings,false);
                ui.separator();
                ui.label("MusicXML destination (same format as the recognized file):");
                let compressed=self.xml.as_ref().and_then(|path|path.extension()).is_some_and(|ext|ext.eq_ignore_ascii_case("mxl"));
                let target=PathField::ExportMusicXml(compressed);
                if paths::entry(ui,target,&mut self.xml_destination,idle) {
                    self.request_path(target,&self.xml_destination.clone());
                }
                if ui.add_enabled(idle, egui::Button::new("Export MusicXML")).clicked() {
                    let result = self.xml.as_ref().context("No recognized MusicXML").and_then(|source| {
                        let destination = expand_path(self.xml_destination.trim());
                        let compressed = source.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxl"));
                        let target_compressed = destination.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxl"));
                        anyhow::ensure!(compressed == target_compressed, "Use .mxl for compressed MusicXML; .musicxml or .xml for plain XML");
                        prepare_export_dir(paths)?;
                        copy_new(source, &destination)
                    });
                    self.export_result(result);
                }
                ui.label("MIDI destination (selected parts, instruments, and tempo):");
                if paths::entry(ui,PathField::ExportMidi,&mut self.midi_destination,idle) {
                    self.request_path(PathField::ExportMidi,&self.midi_destination.clone());
                }
                if ui.add_enabled(idle, egui::Button::new("Export selected MIDI")).clicked() {
                    let result = self.midi().and_then(|bytes| {
                        prepare_export_dir(paths)?;
                        write_new(&expand_path(self.midi_destination.trim()), &bytes)
                    });
                    self.export_result(result);
                }
                ui.label("Existing destination files are never overwritten. Choose a different filename to export again.");
            });
        self.open = open;
        save_settings
    }

    fn export_result(&mut self, result: Result<()>) {
        match result {
            Ok(()) => {
                self.notice = "Export saved; source files remain untouched.".into();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Export: {error:#}")),
        }
    }
}

pub(crate) fn settings_ui(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    busy: bool,
) -> Option<PathField> {
    let mut selected = None;
    ui.label("Audiveris executable (path only, no shell arguments):");
    if paths::entry(
        ui,
        PathField::Audiveris,
        &mut settings.audiveris_executable,
        !busy,
    ) {
        selected = Some(PathField::Audiveris);
    }
    ui.label("FluidSynth executable:");
    if paths::entry(
        ui,
        PathField::FluidSynth,
        &mut settings.fluidsynth_executable,
        !busy,
    ) {
        selected = Some(PathField::FluidSynth);
    }
    ui.label("SoundFont (.sf2 / .sf3):");
    if paths::entry(ui, PathField::SoundFont, &mut settings.soundfont, !busy) {
        selected = Some(PathField::SoundFont);
    }
    ui.label("Install Audiveris separately, including its Tesseract language data. Recognition runs locally. Audiveris is AGPLv3; FluidSynth is LGPL.");
    selected
}

pub(crate) fn audio_settings_ui(
    ui: &mut egui::Ui,
    settings: &mut crate::audio::Settings,
    busy: bool,
) -> Option<PathField> {
    use crate::audio::Backend;
    ui.separator();
    ui.label("Playback backend (Linux):");
    egui::ComboBox::from_id_salt("audio_backend")
        .selected_text(settings.backend.label())
        .show_ui(ui, |ui| {
            for backend in [Backend::Legacy, Backend::PipeWire, Backend::Jack] {
                ui.selectable_value(&mut settings.backend, backend, backend.label());
            }
        });
    ui.label("Audio-host executable (empty uses the installed companion):");
    let browse = paths::entry(
        ui,
        PathField::AudioHost,
        &mut settings.host_executable,
        !busy,
    );
    let mut selected = browse.then_some(PathField::AudioHost);
    ui.small("VST3 needs an SDK-enabled rms-audio-host, not jackd, yabridgectl or Wine. Probe helper capabilities below; the reply must include vst3.");
    ui.small("Use native JACK for an already-running JACK server. PipeWire/JACK instead launches the helper through pw-jack.");
    for (field, label, value) in [
        (
            PathField::BridgeTool,
            "Existing yabridgectl executable (read-only diagnostics):",
            &mut settings.yabridge.tool,
        ),
        (
            PathField::BridgeRoot,
            "Existing generated Linux wrapper root:",
            &mut settings.yabridge.wrapper_root,
        ),
        (
            PathField::WineLoader,
            "Wine loader executable (helper-scoped, no WINEPREFIX override):",
            &mut settings.yabridge.wine_loader,
        ),
    ] {
        ui.label(label);
        if paths::entry(ui, field, value, !busy) {
            selected = Some(field);
        }
    }
    ui.small("Requires a separately built rms-audio-host and existing PipeWire-JACK/native JACK output. No automatic fallback, plugin installation or Wine changes.");
    selected
}

fn program_label(program: u8) -> String {
    match program {
        0 => "Piano".into(),
        40 => "Violin".into(),
        41 => "Viola".into(),
        42 => "Cello".into(),
        43 => "Contrabass".into(),
        48 => "Strings".into(),
        73 => "Flute".into(),
        _ => format!("GM {}", u16::from(program) + 1),
    }
}

fn prepare_export_dir(paths: &Paths) -> Result<()> {
    std::fs::create_dir_all(paths.data.join("exports"))?;
    Ok(())
}

pub fn write_new(destination: &Path, bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(
        !destination.as_os_str().is_empty(),
        "Enter a destination filename"
    );
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(destination)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "Cannot create {}; existing files are not overwritten",
                destination.display()
            )
        })?;
    Ok(())
}

pub fn copy_new(source: &Path, destination: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    std::fs::File::open(source)?
        .take(MAX_EXPORT + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_EXPORT, "Export exceeds 64 MiB");
    write_new(destination, &bytes)
}

fn index_path(paths: &Paths, hash: &str) -> Result<PathBuf> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid score hash");
    }
    Ok(paths.data.join("omr-index").join(format!("{hash}.json")))
}

pub fn save_results(paths: &Paths, artifacts: &RecognitionArtifacts) -> Result<()> {
    let destination = index_path(paths, &artifacts.source_hash)?;
    std::fs::create_dir_all(destination.parent().unwrap())?;
    let mut file = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
    serde_json::to_writer(
        &mut file,
        &SavedResults {
            hash: artifacts.source_hash.clone(),
            movements: artifacts.movements.clone(),
        },
    )?;
    file.as_file().sync_all()?;
    file.persist(destination)?;
    Ok(())
}

fn read_results(paths: &Paths, hash: &str) -> Result<Option<Vec<PathBuf>>> {
    let path = index_path(paths, hash)?;
    if !path.exists() {
        return Ok(None);
    }
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= MAX_MIX,
        "Result index exceeds size limit"
    );
    let saved: SavedResults = serde_json::from_reader(file)?;
    anyhow::ensure!(
        saved.hash == hash && !saved.movements.is_empty() && saved.movements.len() <= 128,
        "Invalid result index"
    );
    let root = paths.data.join("omr").join(hash).canonicalize()?;
    for movement in &saved.movements {
        let resolved = movement.canonicalize()?;
        anyhow::ensure!(
            resolved.starts_with(&root) && resolved.is_file(),
            "Result is outside this score's OMR directory"
        );
    }
    Ok(Some(saved.movements))
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = r#"<score-partwise version="4.0"><part-list>
    <score-part id="P1"><part-name>Cello</part-name><midi-instrument id="I1"><midi-program>43</midi-program></midi-instrument></score-part>
    <score-part id="P2"><part-name>Piano</part-name></score-part></part-list>
    <part id="P1"><measure number="1"><attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes><note><pitch><step>C</step><octave>3</octave></pitch><duration>4</duration></note></measure></part>
    <part id="P2"><measure number="1"><attributes><divisions>1</divisions><staves>2</staves></attributes><note><pitch><step>E</step><octave>4</octave></pitch><duration>4</duration><voice>1</voice><staff>1</staff></note><backup><duration>4</duration></backup><note><pitch><step>C</step><octave>3</octave></pitch><duration>4</duration><voice>2</voice><staff>2</staff></note></measure></part></score-partwise>"#;

    fn fixture() -> (tempfile::TempDir, Paths, Score, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            config: root.path().join("config"),
            music: None,
        };
        paths.create().unwrap();
        let source = root.path().join("score.pdf");
        std::fs::write(&source, b"generated source stand-in").unwrap();
        let hash =
            crate::folder::hash_file(&source, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        let score = Score {
            id: 1,
            hash,
            title: "Generated score".into(),
            ..Default::default()
        };
        (root, paths, score, source)
    }

    fn finish_load(music: &mut Music, paths: &Paths, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while music.loading.is_some() {
            music.poll(paths, ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn vst3_movement_load_does_not_inherit_unsaved_plugin_edits() {
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("first.musicxml");
        let second = root.path().join("second.musicxml");
        std::fs::write(&xml, XML).unwrap();
        std::fs::write(&second, XML.replace("<step>C</step>", "<step>D</step>")).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.open_for(&score, &source, &paths, &ctx);
        music.start_load(xml, true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        assert!(music.error.is_none(), "{:?}", music.error);
        music.plugin_unsaved = true;
        music.start_load(second, true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        assert!(music.error.is_none(), "{:?}", music.error);
        assert!(
            !music.plugin_unsaved,
            "Previous movement edits must not block selecting a VST3 in the new movement"
        );
    }

    #[test]
    fn loads_private_musicxml_and_keeps_piano_staves_together() {
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("corrected.musicxml");
        std::fs::write(&xml, XML).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.open_for(&score, &source, &paths, &ctx);
        music.start_load(xml.clone(), true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        assert!(music.error.is_none(), "{:?}", music.error);
        let parts = &music.composition.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].staves, 2);
        assert_eq!(parts[1].notes.len(), 2);
        assert_ne!(music.xml.as_ref().unwrap(), &xml);
        music.selected.insert("P1".into(), false);
        let midi = music.midi().unwrap();
        assert_eq!(u16::from_be_bytes([midi[10], midi[11]]), 2);
        assert_eq!(std::fs::read_to_string(xml).unwrap(), XML);
        std::fs::write(&source, b"changed backing score").unwrap();
        music.poll(&paths, &ctx);
        assert!(music.composition.is_none());
        assert!(music
            .error
            .as_deref()
            .unwrap()
            .contains("backing score changed"));
    }

    #[test]
    fn loading_rejects_changed_source() {
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("corrected.musicxml");
        std::fs::write(&xml, XML).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.open_for(&score, &source, &paths, &ctx);
        std::fs::write(&source, b"source changed before loading").unwrap();
        music.start_load(xml, true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        assert!(music.composition.is_none());
        assert!(music
            .error
            .as_deref()
            .unwrap()
            .contains("Backing score changed"));
    }

    #[test]
    fn older_preferences_gain_default_music_settings() {
        let preferences: crate::model::Preferences = serde_json::from_str(
            r#"{"next_key":"PageDown","previous_key":"PageUp","recursive":true}"#,
        )
        .unwrap();
        assert!(!preferences.music.audiveris_executable.is_empty());
        assert_eq!(preferences.music.fluidsynth_executable, "fluidsynth");
    }

    #[test]
    fn older_saved_mix_defaults_to_full_master_volume() {
        let mix: SavedMix =
            serde_json::from_str(r#"{"selected":{},"programs":{},"volumes":{},"tempo_scale":1.0}"#)
                .unwrap();
        assert_eq!(mix.master_volume, 100);
        assert!(!mix.follow_pages);
        assert_eq!(mix.turn_lead_measures, 0);
    }

    #[test]
    fn performance_loads_cached_music_without_opening_settings_and_restores_mix() {
        let (_root, paths, score, source) = fixture();
        let xml = paths
            .data
            .join("omr")
            .join(&score.hash)
            .join("run-generated/score.musicxml");
        std::fs::create_dir_all(xml.parent().unwrap()).unwrap();
        std::fs::write(&xml, XML).unwrap();
        save_results(
            &paths,
            &RecognitionArtifacts {
                source_hash: score.hash.clone(),
                movements: vec![xml],
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.prepare_for(&score, &source, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        assert!(!music.open);
        assert!(music.job.is_none());
        assert_eq!(music.composition.as_ref().unwrap().parts[1].staves, 2);
        music.selected.insert("P1".into(), false);
        music.volumes.insert("P2".into(), 37);
        music.master_volume = 45;
        music.programs.insert("P1".into(), 42);
        music.mix_dirty = Some(std::time::Instant::now());
        music.save_mix().unwrap();
        let mut reloaded = Music::default();
        reloaded.prepare_for(&score, &source, &paths, &ctx);
        finish_load(&mut reloaded, &paths, &ctx);
        assert!(!reloaded.selected["P1"]);
        assert_eq!(reloaded.volumes["P2"], 37);
        assert_eq!(reloaded.master_volume, 45);
        assert_eq!(reloaded.options().volumes["P2"], 47);
        assert_eq!(reloaded.programs["P1"], 42);
        let mut labels = std::collections::HashSet::new();
        for _ in 0..4 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 360.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::TopBottomPanel::bottom("performance_test").show(ctx, |ui| {
                        reloaded.performance_controls(
                            ui,
                            &score,
                            &source,
                            &paths,
                            &Settings::default(),
                            false,
                        )
                    });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Score stays visible");
                    });
                },
            );
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let visible = shape
                        .clip_rect
                        .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    if visible.height() > 8.0 && visible.width() > 10.0 {
                        labels.insert(text.galley.job.text.clone());
                    }
                }
            }
        }
        for label in [
            "Play",
            "Stop",
            "Rewind 5s",
            "Fast-forward 5s",
            "Parts / volume",
            "Master volume %",
        ] {
            assert!(
                labels.contains(label),
                "Missing performance control {label}: {labels:?}"
            );
        }
        assert!(!labels.contains("Recognize music"));
        assert!(!reloaded.open);
    }

    #[test]
    fn unrecognized_performance_shows_recognition_and_rejects_invalid_mix() {
        let (_root, paths, score, source) = fixture();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        let mut visible = false;
        for _ in 0..3 {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    music.performance_controls(
                        ui,
                        &score,
                        &source,
                        &paths,
                        &Settings::default(),
                        false,
                    )
                });
            });
            visible|=output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(t) if t.galley.job.text=="Recognize music"));
        }
        assert!(visible);
        let file = paths.data.join("invalid-mix.json");
        std::fs::write(
            &file,
            r#"{"selected":{},"programs":{},"volumes":{"P1":255},"tempo_scale":1.0}"#,
        )
        .unwrap();
        music.mix_path = Some(file);
        assert!(music.load_mix().is_err());
    }

    #[test]
    fn music_controls_remain_reachable_in_a_small_reader_window() {
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("corrected.musicxml");
        std::fs::write(&xml, XML).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.open_for(&score, &source, &paths, &ctx);
        music.start_load(xml, true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        let mut settings = Settings::default();
        let mut seen = std::collections::HashSet::new();
        for frame in 0..35 {
            let events = if frame > 3 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(280.0, 180.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -90.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                vec![]
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 360.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    music.show(
                        ctx,
                        Some((&score, &source)),
                        &paths,
                        &mut settings,
                        &mut crate::audio::Settings::default(),
                        false,
                    );
                },
            );
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let visible = shape
                        .clip_rect
                        .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    if visible.height() > 8.0 && visible.width() > 20.0 {
                        seen.insert(text.galley.job.text.clone());
                    }
                }
            }
        }
        for label in [
            "Recognize with Audiveris",
            "Play selected parts",
            "Stop audio",
            "Export MusicXML",
            "Export selected MIDI",
        ] {
            assert!(
                seen.contains(label),
                "Unreachable music control: {label}; saw {seen:?}"
            );
        }
    }

    #[test]
    fn following_is_opt_in_bound_to_the_original_score_and_manual_turns_disable_it() {
        use crate::{
            notation::{PageMap, PageTurn},
            playback::Snapshot,
        };
        let (_root, paths, mut score, source) = fixture();
        score.pages = 2;
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.prepare_for(&score, &source, &paths, &ctx);
        let mut composition = notation::parse_musicxml(XML).unwrap();
        composition.page_map = Some(PageMap {
            source_pages: vec![0, 1],
            turns: vec![
                PageTurn { tick: 0, page: 0 },
                PageTurn {
                    tick: 1920,
                    page: 1,
                },
            ],
            measure_ticks: vec![0, 960, 1920, 2880],
        });
        music.composition = Some(composition);
        let mut snapshot = Snapshot {
            state: State::Playing,
            position: 1.0,
            duration: 2.0,
            error: None,
            source_page: Some(1),
            score_tick: 1920,
            audio: None,
            plugins: Default::default(),
        };
        assert_eq!(music.follow_page_at(&score, &snapshot), None);
        music.follow_pages = true;
        music.turn_lead_measures = 0;
        assert_eq!(music.follow_page_at(&score, &snapshot), Some(1));
        snapshot.score_tick = 960;
        snapshot.source_page = Some(0);
        assert_eq!(music.follow_page_at(&score, &snapshot), Some(0));
        music.turn_lead_measures = 1;
        for _ in 0..3 {
            assert_eq!(music.follow_page_at(&score, &snapshot), Some(1));
        }
        snapshot.score_tick = 1920;
        snapshot.source_page = Some(1);
        snapshot.state = State::Paused;
        assert_eq!(music.follow_page_at(&score, &snapshot), Some(1));
        for state in [
            State::Starting,
            State::Stopped,
            State::Finished,
            State::Failed,
        ] {
            snapshot.state = state;
            assert_eq!(music.follow_page_at(&score, &snapshot), None);
        }
        snapshot.state = State::Playing;
        snapshot.source_page = Some(2);
        assert_eq!(music.follow_page_at(&score, &snapshot), None);
        snapshot.source_page = Some(1);
        let mut other = score.clone();
        other.id += 1;
        assert_eq!(music.follow_page_at(&other, &snapshot), None);
        other = score.clone();
        other.hash = "changed".into();
        assert_eq!(music.follow_page_at(&other, &snapshot), None);
        music.manual_page_turn();
        assert!(!music.follow_pages);
        assert!(music.mix_dirty.is_some());
        music.follow_pages = true;
        music.mix_path = Some(paths.data.join("follow-mix.json"));
        music.save_mix().unwrap();
        music.follow_pages = false;
        music.load_mix().unwrap();
        assert!(music.follow_pages);
        assert_eq!(music.turn_lead_measures, 1);
        music.source_pages = 1;
        assert!(!music.page_follow_available());
        assert_eq!(music.follow_page_at(&score, &snapshot), None);
    }

    #[test]
    fn exports_never_overwrite_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("music.mid");
        write_new(&destination, b"MThd first").unwrap();
        assert!(write_new(&destination, b"replacement").is_err());
        assert_eq!(std::fs::read(destination).unwrap(), b"MThd first");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn changed_score_invalidates_loaded_music_and_pending_work() {
        let mut music = Music {
            open: true,
            binding: Some((1, "old".into())),
            notice: "Loaded".into(),
            ..Default::default()
        };
        music.invalidate_unless(Some((1, "new".into())));
        assert!(!music.open);
        assert!(music.binding.is_none());
        assert!(music.notice.is_empty());
    }

    #[test]
    fn result_index_rejects_traversal_and_foreign_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            config: root.path().join("config"),
            music: None,
        };
        paths.create().unwrap();
        assert!(index_path(&paths, "../../outside").is_err());
        let hash = "a".repeat(64);
        std::fs::create_dir_all(paths.data.join("omr").join(&hash)).unwrap();
        let outside = root.path().join("outside.musicxml");
        std::fs::write(&outside, "<score-partwise/>").unwrap();
        save_results(
            &paths,
            &RecognitionArtifacts {
                source_hash: hash.clone(),
                movements: vec![outside],
            },
        )
        .unwrap();
        assert!(read_results(&paths, &hash).is_err());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn plugin_shutdown_checkpoint_does_not_wait_on_ui_and_updates_the_sidecar() {
        use crate::audio::plugins::{Request, Source};
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("corrected.musicxml");
        std::fs::write(&xml, XML).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.prepare_for(&score, &source, &paths, &ctx);
        music.start_load(xml, true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        let (transport, sources, _) =
            crate::audio::supervisor::tests::plugin_session(root.path(), "normal");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while transport.snapshot().state != State::Stopped {
            assert_ne!(transport.snapshot().state, State::Failed);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        transport
            .plugin(Request::Parameter {
                part_id: "P2".into(),
                param_id: 0,
                value: 0.77,
            })
            .unwrap();
        music.sources = sources;
        music.playback = Some(transport);
        music.mix_dirty = Some(std::time::Instant::now());
        music.save_mix().unwrap();
        let at = std::time::Instant::now();
        music.begin_shutdown();
        assert!(
            at.elapsed() < std::time::Duration::from_millis(100),
            "Shutdown must not join helper on egui"
        );
        assert!(music.playback.is_none());
        while music.shutdown_pending() {
            music.poll(&paths, &ctx);
            assert!(std::time::Instant::now() < deadline, "{:?}", music.error);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(music.shutdown_error().is_none(), "{:?}", music.error);
        assert!(
            matches!(
                &music.sources["P2"],
                Source::Vst3 {
                    component_state: Some(_),
                    ..
                }
            ),
            "Completed retirement must refresh in-memory chunks before another manifest save"
        );
        music.sources.clear();
        music.load_mix().unwrap();
        let Source::Vst3 {
            component_state: Some(component),
            controller_state: Some(controller),
            ..
        } = &music.sources["P2"]
        else {
            panic!("Both state chunks must be persisted")
        };
        assert_ne!(component.path, controller.path);
        let bytes = std::fs::read(&component.path).unwrap();
        let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(state["value"], 0.77);
        assert!(music
            .mix_path
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".audio-v2.json"));
    }

    #[test]
    fn mix_loader_accepts_valid_metadata_above_old_256k_limit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("movement.audio-v2.json");
        let mut bytes = br#"{"selected":{},"programs":{},"volumes":{},"tempo_scale":1.0}"#.to_vec();
        bytes.resize(300 * 1024, b' ');
        std::fs::write(&path, bytes).unwrap();
        let mut music = Music {
            mix_path: Some(path),
            ..Default::default()
        };
        music.load_mix().unwrap();
    }

    #[test]
    fn two_plugin_assignments_reload_without_losing_gm_or_guessing_changed_identity() {
        use crate::audio::plugins::Source;
        let (root, paths, score, source) = fixture();
        let xml = root.path().join("corrected.musicxml");
        std::fs::write(&xml, XML).unwrap();
        let ctx = egui::Context::default();
        let mut music = Music::default();
        music.prepare_for(&score, &source, &paths, &ctx);
        music.start_load(xml.clone(), true, &paths, &ctx);
        finish_load(&mut music, &paths, &ctx);
        let legacy = music.legacy_mix_path.clone().unwrap();
        let original = br#"{"selected":{},"programs":{"P1":40},"volumes":{},"tempo_scale":1.0}"#;
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, original).unwrap();
        music.load_mix().unwrap();
        assert_eq!(music.programs["P1"], 40);
        assert_ne!(music.mix_path.as_ref(), Some(&legacy));
        for (id, gain, class) in [("P1", 0.2, "A"), ("P2", 0.8, "B")] {
            music.sources.insert(
                id.into(),
                Source::Vst3 {
                    path: PathBuf::from("/missing but retained.vst3"),
                    class_id: class.repeat(32),
                    module_sha256: "a".repeat(64),
                    gain,
                    component_state: None,
                    controller_state: None,
                },
            );
        }
        music.programs.insert("P1".into(), 42);
        music.mix_dirty = Some(std::time::Instant::now());
        music.save_mix().unwrap();
        let saved = music.sources.clone();
        music.sources.clear();
        music.load_mix().unwrap();
        assert_eq!(music.sources, saved);
        assert_eq!(music.programs["P1"], 42);
        assert_eq!(music.composition.as_ref().unwrap().parts[1].staves, 2);
        assert!(music
            .start_transport_mode(&Settings::default(), false)
            .unwrap_err()
            .to_string()
            .contains("require the audio-host"));
        assert!(music.midi().is_ok(), "MIDI remains an explicit GM export");
        music.sources.insert("P1".into(), Source::Soundfont);
        music.sources.insert("P2".into(), Source::Soundfont);
        music.mix_dirty = Some(std::time::Instant::now());
        music.save_mix().unwrap();
        music.sources = saved.clone();
        music.load_mix().unwrap();
        assert!(
            music.sources.values().all(|s| !s.is_plugin()),
            "Explicit SoundFont fallback must not resurrect a plugin from older manifests"
        );
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
        let fallback = music.sources.clone();
        music.sources.insert(
            "oversized".into(),
            Source::Vst3 {
                path: PathBuf::from(format!("/{}.vst3", "x".repeat(MAX_MIX as usize))),
                class_id: "A".repeat(32),
                module_sha256: "a".repeat(64),
                gain: 0.5,
                component_state: None,
                controller_state: None,
            },
        );
        let previous = std::fs::read(music.mix_path.as_ref().unwrap()).unwrap();
        music.mix_dirty = Some(std::time::Instant::now());
        assert!(music.save_mix().is_err());
        assert_eq!(
            std::fs::read(music.mix_path.as_ref().unwrap()).unwrap(),
            previous
        );
        music.sources = fallback.clone();
        music.music_hash = Some("changed MusicXML".into());
        assert!(music
            .load_mix()
            .unwrap_err()
            .to_string()
            .contains("reconcile"));
        assert_eq!(
            music.sources, fallback,
            "Rejected manifests must not overwrite assignments"
        );
    }
}
