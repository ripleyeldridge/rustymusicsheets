use crate::{
    folder,
    help::Help,
    import::{self, Report},
    instruments::{
        self,
        job::{DetectionJob, Summary as DetectionSummary},
        review::Review,
        Instrument,
    },
    model::{Entry, Group, Paths, Preferences, Score},
    picker,
    power::{KeepAwake, PowerStatus},
    reader::{Reader, Turn},
    reconcile,
    render::{PageKey, PageRenderer},
    setlists::SetlistManager,
    source::{DocumentAccess, DocumentRef, LocalDocuments},
    storage::Library,
    text_input::{ImeCompatibility, TextEditFocusExt},
    watch::FolderWatcher,
};
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Vec2};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct ImportTask {
    rx: Receiver<ImportMessage>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Workspace {
    Library,
    Reader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorPanel {
    Settings,
    Metadata,
    Instruments,
}

enum ImportMessage {
    Progress(Report),
    Done(Result<Report, String>),
    FolderReady(Paths),
}

pub struct MusicApp {
    document_access: Arc<dyn DocumentAccess>,
    paths: Paths,
    db: Library,
    prefs: Preferences,
    scores: Vec<Score>,
    groups: Vec<Group>,
    entries: Vec<Entry>,
    selected_group: Option<i64>,
    show_all_to_add: bool,
    reader: Reader,
    renderer: PageRenderer,
    query: String,
    genre: String,
    sort: usize,
    import_path: String,
    import_task: Option<ImportTask>,
    report: Option<Report>,
    picker: Option<Receiver<Result<Option<PathBuf>, String>>>,
    editing: Option<Score>,
    group_name: String,
    new_collection: bool,
    left_editor: Option<EditorPanel>,
    performance: bool,
    fullscreen: bool,
    fit_width: bool,
    zoom: f32,
    jump: u32,
    status: String,
    error: Option<String>,
    bookmarks: Vec<(u32, String)>,
    save_page_at: Option<Instant>,
    smoke_deadline: Option<Instant>,
    text_input_active: bool,
    library_folder_input: String,
    picker_sets_library: bool,
    watcher: Option<FolderWatcher>,
    watch_error: Option<String>,
    changing_folder: bool,
    keep_awake: Option<KeepAwake>,
    power_status: PowerStatus,
    ime: ImeCompatibility,
    setlist_manager: Option<SetlistManager>,
    workspace: Workspace,
    show_options: bool,
    rescanning: bool,
    detection_job: Option<DetectionJob>,
    detection_summary: Option<DetectionSummary>,
    instrument_filter: Option<String>,
    instrument_review: Review,
    auto_detection_pending: std::collections::HashSet<i64>,
    help: Help,
}

impl MusicApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Paths,
        smoke_seconds: Option<u64>,
    ) -> Result<Self> {
        Self::from_context(&cc.egui_ctx, paths, smoke_seconds)
    }

    fn from_context(ctx: &egui::Context, paths: Paths, smoke_seconds: Option<u64>) -> Result<Self> {
        let db = Library::open(&paths)?;
        let mut paths = paths;
        paths.music = db.music_folder()?.or(paths.music);
        let scores = db.scores()?;
        let groups = db.groups()?;
        let (prefs, error) = match paths.preferences() {
            Ok(prefs) => (prefs, None),
            Err(error) => (
                Preferences::default(),
                Some(format!("Preferences: {error:#}")),
            ),
        };
        ctx.set_visuals(egui::Visuals::dark());
        ctx.style_mut(|style| {
            style.spacing.item_spacing = Vec2::new(9.0, 8.0);
            style.spacing.button_padding = Vec2::new(10.0, 7.0);
        });
        Ok(Self {
            document_access: Arc::new(LocalDocuments),
            library_folder_input: picker::initial_directory(
                &paths.score_dir(),
                &picker::home_directory(),
            )
            .display()
            .to_string(),
            power_status: PowerStatus {
                requested: prefs.keep_awake,
                message: if prefs.keep_awake {
                    "Requesting keep-awake protection…"
                } else {
                    "Keep awake disabled."
                }
                .into(),
                ..Default::default()
            },
            keep_awake: Some({
                let repaint = ctx.clone();
                KeepAwake::new(prefs.keep_awake, move || repaint.request_repaint())
            }),
            watcher: paths.music.as_ref().map(|_| {
                let repaint = ctx.clone();
                FolderWatcher::start(paths.clone(), move || repaint.request_repaint())
            }),
            picker_sets_library: false,
            watch_error: None,
            changing_folder: false,
            renderer: PageRenderer::new(paths.clone(), ctx.clone()),
            paths,
            db,
            prefs,
            scores,
            groups,
            entries: Vec::new(),
            selected_group: None,
            show_all_to_add: false,
            reader: Reader::default(),
            query: String::new(),
            genre: String::new(),
            sort: 0,
            import_path: String::new(),
            import_task: None,
            report: None,
            picker: None,
            editing: None,
            group_name: String::new(),
            new_collection: false,
            left_editor: None,
            performance: false,
            fullscreen: false,
            fit_width: false,
            zoom: 1.0,
            jump: 1,
            status: "Import PDFs or a folder to start your music library.".into(),
            error,
            bookmarks: Vec::new(),
            save_page_at: None,
            smoke_deadline: smoke_seconds.map(|n| Instant::now() + Duration::from_secs(n)),
            text_input_active: false,
            ime: ImeCompatibility::default(),
            setlist_manager: None,
            workspace: Workspace::Library,
            show_options: true,
            rescanning: false,
            detection_job: None,
            detection_summary: None,
            instrument_filter: None,
            instrument_review: Review::default(),
            auto_detection_pending: std::collections::HashSet::new(),
            help: Help::default(),
        })
    }

    pub fn new_mobile(
        cc: &eframe::CreationContext<'_>,
        paths: Paths,
        access: Arc<dyn DocumentAccess>,
        initial: Result<Option<DocumentRef>>,
    ) -> Result<Self> {
        let mut app = Self::from_context(&cc.egui_ctx, paths, None)?;
        app.document_access = access;
        app.show_options = false;
        app.status = "Android foundation: open a PNG/JPEG using your document app's Open with action. Native PDF/OCR and the in-app Android picker are not implemented yet.".into();
        match initial {
            Ok(Some(document)) => app.start_document_import(document, &cc.egui_ctx),
            Ok(None) => {}
            Err(error) => app.error = Some(format!("Cannot open launch document: {error:#}")),
        }
        Ok(app)
    }

    fn result<T>(&mut self, result: Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                None
            }
        }
    }

    fn reload(&mut self) {
        let untouched_draft = self
            .editing
            .as_ref()
            .filter(|draft| {
                self.scores
                    .iter()
                    .find(|score| score.id == draft.id)
                    .is_some_and(|score| draft.metadata_matches(score))
            })
            .map(|draft| draft.id);
        let old_documents: std::collections::HashSet<_> = self
            .scores
            .iter()
            .map(|score| (score.id, score.hash.clone()))
            .collect();
        let old_hash = self
            .scores
            .iter()
            .find(|s| Some(s.id) == self.reader.score_id)
            .map(|s| s.hash.clone());
        if let Some(scores) = self.result(self.db.scores()) {
            self.scores = scores;
        }
        if let Some(id) = untouched_draft {
            self.editing = self.scores.iter().find(|score| score.id == id).cloned();
            if self.editing.is_none() && self.left_editor == Some(EditorPanel::Metadata) {
                self.left_editor = None;
            }
        }
        let valid: std::collections::HashSet<_> = self.scores.iter().map(|s| s.id).collect();
        if self.prefs.detect_on_import {
            for score in &self.scores {
                if !old_documents.contains(&(score.id, score.hash.clone()))
                    && score.detection_status.is_empty()
                    && !score.instruments_manual
                {
                    self.auto_detection_pending.insert(score.id);
                }
            }
        }
        if self
            .instrument_review
            .score_id
            .is_some_and(|id| !valid.contains(&id))
        {
            self.instrument_review.open(None);
        }
        if self.reader.retain_existing(&valid) {
            self.save_page_at = None;
            self.bookmarks.clear();
            self.jump = 1;
            self.renderer.invalidate_requests();
            self.workspace = Workspace::Library;
            self.performance = false;
            self.status = "The open score was removed by Rescan.".into();
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|score| !valid.contains(&score.id))
        {
            self.editing = None;
            if self.left_editor == Some(EditorPanel::Metadata) {
                self.left_editor = None;
            }
        }
        if let Some(score) = self
            .scores
            .iter()
            .find(|s| Some(s.id) == self.reader.score_id)
        {
            if old_hash.as_ref() != Some(&score.hash) {
                self.reader.load(score.id, score.pages, self.reader.page);
                self.jump = self.reader.page + 1;
                self.renderer.invalidate_requests();
            }
        }
        if let Some(groups) = self.result(self.db.groups()) {
            self.groups = groups;
        }
        self.refresh_entries();
    }

    fn refresh_entries(&mut self) {
        if let Some(group) = self.selected_group {
            if let Some(entries) = self.result(self.db.entries(group)) {
                self.entries = entries;
            }
        } else {
            self.entries.clear();
        }
    }

    fn persist_page(&mut self) {
        if let Some(id) = self.reader.score_id {
            let result = self.db.remember_page(id, self.reader.page);
            if result.as_ref().err().is_some_and(|error| {
                matches!(
                    error.downcast_ref::<rusqlite::Error>(),
                    Some(rusqlite::Error::QueryReturnedNoRows)
                )
            }) {
                self.save_page_at = None;
                return;
            }
            if self.result(result).is_some() {
                if let Some(score) = self.scores.iter_mut().find(|s| s.id == id) {
                    score.last_page = self.reader.page;
                }
                self.save_page_at = None;
            }
        }
    }

    fn load_score(&mut self, id: i64, page: Option<u32>) {
        self.persist_page();
        if let Some(score) = self.scores.iter().find(|s| s.id == id) {
            self.reader
                .load(id, score.pages, page.unwrap_or(score.last_page));
            self.workspace = Workspace::Reader;
            self.jump = self.reader.page + 1;
            self.renderer.invalidate_requests();
            self.bookmarks = self.result(self.db.bookmarks(id)).unwrap_or_default();
            self.status.clear();
        }
    }

    fn changed_page(&mut self) {
        self.jump = self.reader.page + 1;
        self.save_page_at = Some(Instant::now() + Duration::from_millis(300));
        self.renderer.invalidate_requests();
        self.status.clear();
    }

    fn turn(&mut self, forward: bool) {
        // Save the old score's position before Reader changes sequence index.
        self.persist_page();
        match self.reader.turn(forward) {
            Turn::Page => self.changed_page(),
            Turn::Score(id) => {
                let page = if forward {
                    0
                } else {
                    self.scores
                        .iter()
                        .find(|s| s.id == id)
                        .map_or(0, |s| s.pages.saturating_sub(1))
                };
                self.load_score(id, Some(page));
            }
            Turn::Boundary => {
                self.status = if forward {
                    "End of score / setlist"
                } else {
                    "Beginning of score / setlist"
                }
                .into();
            }
        }
    }

    fn start_import(&mut self, sources: Vec<PathBuf>, ctx: &egui::Context) {
        if self.detection_job.is_some() {
            self.status = "Finish or cancel instrument detection before importing.".into();
            return;
        }
        if self.import_task.is_some() || sources.is_empty() {
            return;
        }
        self.watcher.take();
        let (tx, rx) = bounded(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let paths = self.paths.clone();
        let recursive = self.prefs.recursive;
        let ctx = ctx.clone();
        let worker = std::thread::spawn(move || {
            let result = import::batch(&sources, recursive, &paths, &worker_cancel, |report| {
                let _ = tx.try_send(ImportMessage::Progress(report.clone()));
                ctx.request_repaint();
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(ImportMessage::Done(result));
            ctx.request_repaint();
        });
        self.report = Some(Report::default());
        self.import_task = Some(ImportTask {
            rx,
            cancel,
            worker: Some(worker),
        });
    }

    fn start_document_import(&mut self, source: DocumentRef, ctx: &egui::Context) {
        if self.import_task.is_some() || self.detection_job.is_some() {
            self.status = "Wait for the current library task before importing a document.".into();
            return;
        }
        self.watcher.take();
        let (tx, rx) = bounded(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = cancel.clone();
        let paths = self.paths.clone();
        let access = self.document_access.clone();
        let repaint = ctx.clone();
        let worker = std::thread::spawn(move || {
            let result = (|| -> Result<Report> {
                let db = Library::open(&paths)?;
                let outcome =
                    import::import_document(&source, access.as_ref(), &paths, &db, &stopping)?;
                Ok(Report {
                    imported: usize::from(outcome == import::Outcome::Imported),
                    skipped: usize::from(outcome == import::Outcome::Duplicate),
                    ..Default::default()
                })
            })();
            let result = match result {
                Err(_) if stopping.load(Ordering::Relaxed) => Ok(Report {
                    cancelled: true,
                    ..Default::default()
                }),
                result => result.map_err(|error| format!("{error:#}")),
            };
            let _ = tx.send(ImportMessage::Done(result));
            repaint.request_repaint();
        });
        self.report = Some(Report::default());
        self.import_task = Some(ImportTask {
            rx,
            cancel,
            worker: Some(worker),
        });
    }

    fn start_watching(&mut self, ctx: &egui::Context) {
        self.watcher.take();
        if self.paths.music.is_some() {
            let repaint = ctx.clone();
            self.watcher = Some(FolderWatcher::start(self.paths.clone(), move || {
                repaint.request_repaint()
            }));
        }
    }

    fn start_rescan(&mut self, ctx: &egui::Context) {
        if self.detection_job.is_some() {
            self.status = "Finish or cancel instrument detection before Rescan.".into();
            return;
        }
        if self.import_task.is_some() {
            return;
        }
        if self.editing.as_ref().is_some_and(|draft| {
            self.scores
                .iter()
                .find(|score| score.id == draft.id)
                .is_none_or(|score| !draft.metadata_matches(score))
        }) {
            self.left_editor = Some(EditorPanel::Metadata);
            self.status = "Save or cancel the metadata draft before rescanning.".into();
            return;
        }
        self.persist_page();
        self.watcher.take();
        let paths = self.paths.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = bounded(8);
        let repaint = ctx.clone();
        let worker = std::thread::spawn(move || {
            let result = reconcile::rescan(&paths, &worker_cancel, |report| {
                let _ = tx.try_send(ImportMessage::Progress(report.clone()));
                repaint.request_repaint();
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(ImportMessage::Done(result));
            repaint.request_repaint();
        });
        self.rescanning = true;
        self.status.clear();
        self.report = Some(Report::default());
        self.import_task = Some(ImportTask {
            rx,
            cancel,
            worker: Some(worker),
        });
    }

    fn change_music_folder(&mut self, destination: PathBuf, ctx: &egui::Context) {
        if self.detection_job.is_some() {
            self.status = "Finish or cancel instrument detection before changing folders.".into();
            return;
        }
        if self.import_task.is_some() {
            return;
        }
        self.watcher.take();
        let paths = self.paths.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = bounded(8);
        let repaint = ctx.clone();
        let worker = std::thread::spawn(move || {
            let result = folder::select(&paths, &destination, &worker_cancel, |current| {
                let _ = tx.try_send(ImportMessage::Progress(Report {
                    current,
                    ..Default::default()
                }));
                repaint.request_repaint();
            });
            match result {
                Ok(paths) => {
                    let _ = tx.send(ImportMessage::FolderReady(paths));
                    let _ = tx.send(ImportMessage::Done(Ok(Report::default())));
                }
                Err(error) => {
                    let _ = tx.send(ImportMessage::Done(Err(format!("{error:#}"))));
                }
            }
            repaint.request_repaint();
        });
        self.changing_folder = true;
        self.report = Some(Report::default());
        self.import_task = Some(ImportTask {
            rx,
            cancel,
            worker: Some(worker),
        });
    }

    fn request_keep_awake(&mut self) {
        self.power_status = PowerStatus {
            requested: self.prefs.keep_awake,
            message: if self.prefs.keep_awake {
                "Requesting keep-awake protection…"
            } else {
                "Releasing keep-awake protection…"
            }
            .into(),
            ..Default::default()
        };
        if let Some(power) = &self.keep_awake {
            power.set_enabled(self.prefs.keep_awake);
        }
    }

    fn choose(&mut self, folder: bool, ctx: &egui::Context) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        let preferred = if self.picker_sets_library {
            expand_path(self.library_folder_input.trim())
        } else if !self.import_path.trim().is_empty() {
            expand_path(self.import_path.trim())
        } else {
            self.paths.score_dir()
        };
        let home = picker::home_directory();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let kind = if folder {
                picker::SelectionKind::Folder
            } else {
                picker::SelectionKind::Score
            };
            let result = picker::select(kind, &preferred, &home).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.picker = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.renderer.poll(ctx);
        let mut detection_done = false;
        let mut detection_update = false;
        if let Some(job) = &self.detection_job {
            while let Ok(update) = job.updates.try_recv() {
                self.detection_summary = Some(update.summary);
                detection_done |= update.done;
                detection_update = true;
            }
        }
        if detection_done {
            self.detection_job.take();
            self.instrument_review.invalidate();
        }
        if detection_update {
            self.reload();
        }
        if let Some(power) = &self.keep_awake {
            while let Ok(status) = power.updates.try_recv() {
                self.power_status = status;
            }
        }
        let mut completed = None;
        let mut folder_ready = None;
        if let Some(task) = &self.import_task {
            while let Ok(message) = task.rx.try_recv() {
                match message {
                    ImportMessage::Progress(report) => self.report = Some(report),
                    ImportMessage::Done(result) => completed = Some(result),
                    ImportMessage::FolderReady(paths) => folder_ready = Some(paths),
                }
            }
        }
        if let Some(paths) = folder_ready {
            self.paths = paths;
            self.library_folder_input = self.paths.score_dir().display().to_string();
            self.renderer = PageRenderer::new(self.paths.clone(), ctx.clone());
            self.watch_error = None;
            self.status =
                "Music folder saved. Scanning existing PDFs and watching for new files.".into();
        }
        if let Some(result) = completed {
            if let Some(mut task) = self.import_task.take() {
                if let Some(worker) = task.worker.take() {
                    let _ = worker.join();
                }
            }
            match result {
                Ok(report) => self.report = Some(report),
                Err(error) => self.error = Some(error),
            }
            self.reload();
            self.changing_folder = false;
            self.rescanning = false;
            self.start_watching(ctx);
        }
        if let Some(result) = self.picker.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.picker = None;
            match result {
                Ok(Some(path)) => {
                    if self.picker_sets_library {
                        self.change_music_folder(path, ctx);
                    } else {
                        self.start_import(vec![path], ctx);
                    }
                }
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(update) = self
            .watcher
            .as_ref()
            .and_then(|w| w.updates.try_recv().ok())
        {
            match update {
                Ok(report) => {
                    self.watch_error = None;
                    if report.imported > 0 || report.failed > 0 {
                        self.report = Some(report);
                    }
                    self.reload();
                }
                Err(error) => self.watch_error = Some(error),
            }
        }
        if self.save_page_at.is_some_and(|at| Instant::now() >= at) {
            self.persist_page();
        }
        if self.prefs.detect_on_import
            && self.detection_job.is_none()
            && self.import_task.is_none()
            && self.editing.is_none()
            && self.left_editor != Some(EditorPanel::Instruments)
            && !self.auto_detection_pending.is_empty()
        {
            let pending: Vec<_> = self
                .scores
                .iter()
                .filter(|score| {
                    self.auto_detection_pending.contains(&score.id)
                        && score.detection_status.is_empty()
                        && !score.instruments_manual
                })
                .cloned()
                .collect();
            self.auto_detection_pending.clear();
            if !pending.is_empty() {
                self.start_detection(pending, true, false, ctx);
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.help.open_topic("welcome");
            return;
        }
        if self.help.open {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.help.open = false;
            }
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.performance = false;
            self.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if self.workspace != Workspace::Reader
            || self.text_input_active
            || (self.left_editor.is_some() && !self.performance)
            || self.setlist_manager.is_some()
        {
            return;
        }
        let next = Key::from_name(&self.prefs.next_key).unwrap_or(Key::PageDown);
        let previous = Key::from_name(&self.prefs.previous_key).unwrap_or(Key::PageUp);
        if ctx.input(|i| i.key_pressed(next) || i.key_pressed(Key::ArrowRight)) {
            self.turn(true);
        } else if ctx.input(|i| i.key_pressed(previous) || i.key_pressed(Key::ArrowLeft)) {
            self.turn(false);
        } else if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.reader.jump(0);
            self.changed_page();
        } else if ctx.input(|i| i.key_pressed(Key::End)) {
            self.reader.jump(self.reader.pages.saturating_sub(1));
            self.changed_page();
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(
                    RichText::new("RustyMusicSheets").color(Color32::from_rgb(237, 173, 94)),
                );
                ui.separator();
                self.help.menu(ui);
                if self.help.open {
                    return;
                }
                if ui
                    .add(
                        egui::Button::new("Library").selected(self.workspace == Workspace::Library),
                    )
                    .clicked()
                {
                    self.switch_workspace(Workspace::Library);
                }
                if ui
                    .add_enabled(
                        self.reader.score_id.is_some(),
                        egui::Button::new("Reader").selected(self.workspace == Workspace::Reader),
                    )
                    .clicked()
                {
                    self.switch_workspace(Workspace::Reader);
                }
                if ui
                    .add_enabled(
                        self.workspace == Workspace::Library,
                        egui::Button::new("Options")
                            .selected(self.show_options && self.left_editor.is_none()),
                    )
                    .clicked()
                {
                    if self.left_editor.take().is_some() {
                        self.show_options = true;
                    } else {
                        self.show_options = !self.show_options;
                    }
                }
                if ui.button("Setlists").clicked() {
                    self.open_setlist_manager(self.selected_group);
                }
                if ui.button("Performance").clicked() && self.reader.score_id.is_some() {
                    self.workspace = Workspace::Reader;
                    self.performance = true;
                    self.fullscreen = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                if ui
                    .add(
                        egui::Button::new("Settings")
                            .selected(self.left_editor == Some(EditorPanel::Settings)),
                    )
                    .clicked()
                {
                    self.left_editor = if self.left_editor == Some(EditorPanel::Settings) {
                        None
                    } else {
                        Some(EditorPanel::Settings)
                    };
                }
                if ui
                    .add_enabled(
                        !self.rescanning
                            && self.detection_job.is_none()
                            && (self.reader.score_id.is_some() || self.editing.is_some()),
                        egui::Button::new("Edit metadata")
                            .selected(self.left_editor == Some(EditorPanel::Metadata)),
                    )
                    .clicked()
                {
                    self.open_metadata_editor();
                }
                if ui.button("Retry renders").clicked() {
                    self.renderer.retry();
                }
            });
        });
    }

    fn display_name_options(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Display names", |ui| {
            ui.checkbox(
                &mut self.prefs.display_names.replace_characters,
                "Replace characters with spaces",
            );
            ui.horizontal(|ui| {
                ui.label("Characters:");
                self.text_input_active |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.prefs.display_names.characters)
                            .id_salt("display_name_characters")
                            .desired_width(140.0)
                            .hint_text("_"),
                    )
                    .text_edit_has_focus();
            });
            ui.label("Each character is replaced separately (e.g. _ or _-).");
            ui.checkbox(
                &mut self.prefs.display_names.remove_imslp_prefix,
                "Remove IMSLP/PMLP download prefixes",
            );
            let example = self
                .scores
                .iter()
                .find(|s| Some(s.id) == self.reader.score_id)
                .or_else(|| self.scores.first())
                .map_or("IMSLP237087-PMLP384177-Bach_Prelude_No.1", |s| {
                    s.title.as_str()
                });
            ui.add(egui::Label::new(format!("Original: {example}")).wrap());
            ui.add(
                egui::Label::new(format!(
                    "Preview: {}",
                    self.prefs.display_names.format(example)
                ))
                .wrap(),
            );
            ui.label("Display only; files and saved titles stay unchanged.");
            ui.label("Preview updates immediately. Save to keep these options after restart.");
            if ui.button("Save display options").clicked()
                && self
                    .result(self.paths.save_preferences(&self.prefs))
                    .is_some()
            {
                self.status = "Display-name preferences saved.".into();
            }
        });
    }

    fn start_detection(
        &mut self,
        scores: Vec<Score>,
        auto_fill: bool,
        show_review: bool,
        ctx: &egui::Context,
    ) -> bool {
        if scores.is_empty() || self.detection_job.is_some() || self.import_task.is_some() {
            self.status = "No eligible scores, or another library task is running.".into();
            return false;
        }
        if self.editing.as_ref().is_some_and(|draft| {
            self.scores
                .iter()
                .find(|s| s.id == draft.id)
                .is_none_or(|s| !draft.metadata_matches(s))
        }) {
            self.left_editor = Some(EditorPanel::Metadata);
            self.status = "Save or cancel the metadata draft before instrument detection.".into();
            return false;
        }
        self.editing = None;
        if show_review {
            self.instrument_review.open(scores.first().map(|s| s.id));
            self.left_editor = Some(EditorPanel::Instruments);
        }
        let repaint = ctx.clone();
        self.detection_summary = None;
        self.detection_job = Some(DetectionJob::start(
            self.paths.clone(),
            scores,
            self.prefs.detection_options.clone(),
            auto_fill,
            move || repaint.request_repaint(),
        ));
        true
    }

    fn instrument_options(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.collapsing("Instrument detection", |ui| {
            ui.label("Offline PDF text extraction, then OCR when needed. Manual runs create review proposals.");
            ui.colored_label(Color32::YELLOW, instruments::DETECTION_TIME_WARNING);
            let idle = self.import_task.is_none() && self.detection_job.is_none();
            if ui.add_enabled(idle && self.reader.score_id.is_some(), egui::Button::new("Detect current score")).clicked() {
                let scores = self.scores.iter().filter(|s| Some(s.id) == self.reader.score_id).cloned().collect();
                self.start_detection(scores, false, true, ctx);
            }
            if ui.add_enabled(idle, egui::Button::new("Detect filtered scores")).clicked() {
                self.start_detection(self.visible_scores(), false, true, ctx);
            }
            if ui.add_enabled(idle, egui::Button::new("Detect entire library")).clicked() {
                self.start_detection(self.scores.clone(), false, true, ctx);
            }
            if ui.button("Review instrument results").clicked() {
                self.instrument_review.open(self.reader.score_id);
                self.left_editor = Some(EditorPanel::Instruments);
            }
            if ui.checkbox(&mut self.prefs.detect_on_import,
                "Detect new imports; fill only clear, unassigned instruments").changed() {
                self.result(self.paths.save_preferences(&self.prefs));
            }
            ui.label("Manual choices are never overwritten. Bare Alto/Tenor/Bass labels need review.");
        });
    }

    fn instrument_review_panel(&mut self, ui: &mut egui::Ui) {
        let dirty = self.editing.as_ref().is_some_and(|draft| {
            self.scores
                .iter()
                .find(|s| s.id == draft.id)
                .is_none_or(|s| !draft.metadata_matches(s))
        });
        if !dirty {
            self.editing = None;
        }
        if dirty {
            ui.colored_label(
                Color32::YELLOW,
                "Save or cancel the metadata draft before detection or applying instruments.",
            );
            if ui.button("Return to metadata draft").clicked() {
                self.left_editor = Some(EditorPanel::Metadata);
            }
        }
        let busy = self.detection_job.is_some() || self.import_task.is_some() || dirty;
        let outcome = self.instrument_review.show(
            ui,
            &self.db,
            &self.paths,
            &self.scores,
            &self.prefs.display_names,
            busy,
        );
        if let Some(error) = outcome.error {
            self.error = Some(error);
        }
        if outcome.applied {
            self.reload();
            self.status = "Instrument choices confirmed and protected.".into();
        }
        if outcome.close {
            self.left_editor = None;
        }
        if let Some(id) = outcome.edit {
            if self.editing.as_ref().is_some_and(|draft| draft.id != id) {
                self.status = "Finish or cancel the existing metadata draft first.".into();
                self.left_editor = Some(EditorPanel::Metadata);
            } else {
                if self.editing.is_none() {
                    self.editing = self.scores.iter().find(|s| s.id == id).cloned();
                }
                self.left_editor = Some(EditorPanel::Metadata);
            }
        }
        if let Some(id) = outcome.detect {
            let scores = self.scores.iter().filter(|s| s.id == id).cloned().collect();
            let ctx = ui.ctx().clone();
            self.start_detection(scores, false, true, &ctx);
        }
    }

    fn edit_instruments(ui: &mut egui::Ui, score: &mut Score, editable: bool) {
        ui.separator();
        ui.heading("Instruments");
        ui.label("A score can have several instruments. Subtype and part are optional.");
        ui.add_enabled_ui(editable, |ui| {
            let mut remove = None;
            for (index, instrument) in score.instruments.iter_mut().enumerate() {
                ui.push_id(("instrument_editor", index), |ui| {
                    ui.group(|ui| {
                        ui.label("Instrument name");
                        let mut changed = ui
                            .add(
                                egui::TextEdit::singleline(&mut instrument.name)
                                    .id(egui::Id::new(("instrument_name", score.id, index)))
                                    .desired_width((ui.available_width() - 8.0).max(24.0)),
                            )
                            .changed();
                        egui::ComboBox::from_id_salt("standard_instrument")
                            .selected_text("Choose standard name…")
                            .show_ui(ui, |ui| {
                                for name in instruments::catalogue() {
                                    if ui.button(name).clicked() {
                                        instrument.name = name.into();
                                        changed = true;
                                    }
                                }
                            });
                        ui.label("Subtype (e.g. Alto)");
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut instrument.variant)
                                    .id(egui::Id::new(("instrument_variant", score.id, index)))
                                    .desired_width((ui.available_width() - 8.0).max(24.0)),
                            )
                            .changed();
                        ui.label("Part number");
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut instrument.part)
                                    .id(egui::Id::new(("instrument_part", score.id, index)))
                                    .desired_width((ui.available_width() - 8.0).max(24.0)),
                            )
                            .changed();
                        if changed {
                            score.instruments_manual = true;
                        }
                        if ui.button("Remove instrument").clicked() {
                            remove = Some(index);
                        }
                    });
                });
            }
            if let Some(index) = remove {
                score.instruments.remove(index);
                score.instruments_manual = true;
            }
            if score.instruments.len() < 32 && ui.button("+ Add instrument").clicked() {
                score.instruments.push(Instrument {
                    name: "Saxophone".into(),
                    ..Default::default()
                });
                score.instruments_manual = true;
            }
            ui.checkbox(
                &mut score.instruments_manual,
                "Protect manual instrument choices (even if empty)",
            );
            ui.label(
                "Instrument edits turn protection on when saved. Unlock in a later save if wanted.",
            );
        });
    }

    fn visible_scores(&self) -> Vec<Score> {
        let mut scores: Vec<_> = self
            .scores
            .iter()
            .filter(|score| {
                score.matches_display(&self.query, &self.prefs.display_names.format(&score.title))
                    && (self.genre.is_empty() || score.genre == self.genre)
                    && self.instrument_filter.as_ref().is_none_or(|name| {
                        if name.is_empty() {
                            score.instruments.is_empty()
                        } else {
                            score
                                .instruments
                                .iter()
                                .any(|instrument| &instrument.name == name)
                        }
                    })
                    && (self.selected_group.is_none()
                        || self.show_all_to_add
                        || self.entries.iter().any(|entry| entry.score_id == score.id))
            })
            .cloned()
            .collect();
        match self.sort {
            1 => scores.sort_by_cached_key(|s| {
                (
                    s.composer.to_lowercase(),
                    self.prefs.display_names.format(&s.title).to_lowercase(),
                    s.id,
                )
            }),
            2 => scores.sort_by_key(|s| std::cmp::Reverse(s.id)),
            _ => scores.sort_by_cached_key(|s| {
                (
                    self.prefs.display_names.format(&s.title).to_lowercase(),
                    s.id,
                )
            }),
        }
        scores
    }

    fn switch_workspace(&mut self, workspace: Workspace) {
        if workspace == Workspace::Reader && self.reader.score_id.is_none() {
            return;
        }
        if workspace == Workspace::Library && self.workspace == Workspace::Reader {
            self.persist_page();
            self.renderer.invalidate_requests();
        }
        self.workspace = workspace;
        self.performance = false;
    }

    fn library_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Score library");
            ui.horizontal(|ui| {
                self.text_input_active |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.query)
                            .id(egui::Id::new("library_search"))
                            .desired_width((ui.available_width() - 90.0).max(80.0))
                            .hint_text("Search titles, composers, tags, and other metadata…"),
                    )
                    .text_edit_has_focus();
                if ui
                    .add_enabled(!self.query.is_empty(), egui::Button::new("Clear"))
                    .clicked()
                {
                    self.query.clear();
                    ui.memory_mut(|memory| memory.request_focus(egui::Id::new("library_search")));
                }
            });
            ui.horizontal_wrapped(|ui| {
                let mut selected = self.selected_group;
                let name = self
                    .groups
                    .iter()
                    .find(|g| Some(g.id) == selected)
                    .map_or("All scores", |g| g.name.as_str())
                    .to_owned();
                egui::ComboBox::from_id_salt("group_filter")
                    .width(160.0)
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut selected, None, "All scores");
                        for group in &self.groups {
                            ui.selectable_value(
                                &mut selected,
                                Some(group.id),
                                format!("{}: {}", group.kind, group.name),
                            );
                        }
                    });
                let mut names: Vec<_> = self
                    .scores
                    .iter()
                    .flat_map(|s| s.instruments.iter())
                    .map(|i| i.name.clone())
                    .collect();
                names.sort();
                names.dedup();
                let instrument_selected = match &self.instrument_filter {
                    None => "All instruments",
                    Some(name) if name.is_empty() => "Unassigned",
                    Some(name) => name,
                };
                egui::ComboBox::from_id_salt("instrument_filter")
                    .width(130.0)
                    .selected_text(instrument_selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.instrument_filter, None, "All instruments");
                        ui.selectable_value(
                            &mut self.instrument_filter,
                            Some(String::new()),
                            "Unassigned",
                        );
                        for name in names {
                            ui.selectable_value(
                                &mut self.instrument_filter,
                                Some(name.clone()),
                                name,
                            );
                        }
                    });
                if selected != self.selected_group {
                    self.selected_group = selected;
                    self.show_all_to_add = false;
                    self.refresh_entries();
                }
                egui::ComboBox::from_id_salt("sort")
                    .width(100.0)
                    .selected_text(["Title", "Composer", "Newest"][self.sort])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sort, 0, "Title");
                        ui.selectable_value(&mut self.sort, 1, "Composer");
                        ui.selectable_value(&mut self.sort, 2, "Newest");
                    });
                let mut genres: Vec<_> = self
                    .scores
                    .iter()
                    .map(|s| s.genre.clone())
                    .filter(|g| !g.is_empty())
                    .collect();
                genres.sort();
                genres.dedup();
                egui::ComboBox::from_id_salt("genre")
                    .width(130.0)
                    .selected_text(if self.genre.is_empty() {
                        "All genres"
                    } else {
                        &self.genre
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.genre, String::new(), "All genres");
                        for genre in genres {
                            ui.selectable_value(&mut self.genre, genre.clone(), genre);
                        }
                    });
            });
            if self.selected_group.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.show_all_to_add,
                        "Show all scores to add to this list",
                    );
                    if ui.button("Manage list…").clicked() {
                        self.open_setlist_manager(self.selected_group);
                    }
                });
            }
            let visible = self.visible_scores();
            ui.label(format!(
                "{} scores · {} matching · select a score to open Reader",
                self.scores.len(),
                visible.len()
            ));
            ui.separator();
            if visible.is_empty() {
                ui.label(if self.scores.is_empty() {
                    "Import a PDF or music folder using Library options to start."
                } else {
                    "No matching scores. Clear the search or change the list/genre filters."
                });
            }
            egui::ScrollArea::vertical()
                .id_salt("scores")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .auto_shrink([false, false])
                .show_rows(ui, 64.0, visible.len(), |ui, range| {
                    for score in &visible[range] {
                        ui.push_id(score.id, |ui| {
                            ui.horizontal(|ui| {
                                let title = self.prefs.display_names.format(&score.title);
                                let mut details = if score.composer.is_empty() {
                                    format!("{} pages", score.pages)
                                } else {
                                    format!("{} · {} pages", score.composer, score.pages)
                                };
                                if !score.instruments.is_empty() {
                                    details.push_str(&format!(
                                        " · {}",
                                        score
                                            .instruments
                                            .iter()
                                            .map(|i| i.label())
                                            .collect::<Vec<_>>()
                                            .join("; ")
                                    ));
                                }
                                let reserved = if self.selected_group.is_some() {
                                    85.0
                                } else {
                                    0.0
                                } + if score.detection_status.is_empty() {
                                    0.0
                                } else {
                                    80.0
                                };
                                let response = ui
                                    .add_sized(
                                        [(ui.available_width() - reserved).max(80.0), 64.0],
                                        egui::Button::new(
                                            RichText::new(format!("{title} — {details}"))
                                                .size(16.0),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Truncate)
                                        .selected(self.reader.score_id == Some(score.id)),
                                    )
                                    .on_hover_text(format!(
                                        "Original title: {}\nFile: {}",
                                        score.title,
                                        score.file_path.display()
                                    ));
                                if response.clicked() {
                                    self.reader.sequence.clear();
                                    self.reader.sequence_index = 0;
                                    self.load_score(score.id, None);
                                }
                                if !score.detection_status.is_empty()
                                    && ui
                                        .button("Review")
                                        .on_hover_text("Review detected instruments for this file")
                                        .clicked()
                                {
                                    self.instrument_review.open(Some(score.id));
                                    self.left_editor = Some(EditorPanel::Instruments);
                                }
                                if let Some(group) = self.selected_group {
                                    if ui
                                        .button("+ Add")
                                        .on_hover_text(
                                            "Add to selected list without opening the score",
                                        )
                                        .clicked()
                                    {
                                        self.result(self.db.add_entry(group, score.id));
                                        self.refresh_entries();
                                    }
                                }
                            });
                        });
                    }
                });
        });
    }

    fn workspace_view(&mut self, ctx: &egui::Context) {
        if self.help.open {
            self.text_input_active |= self.help.show(ctx);
        } else if self.performance || self.workspace == Workspace::Reader {
            self.score_view(ctx);
        } else {
            self.library_view(ctx);
        }
    }

    fn sidebar_visible(&self) -> bool {
        !self.help.open
            && !self.performance
            && (self.left_editor.is_some()
                || (self.workspace == Workspace::Library && self.show_options))
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        let maximum_width = (ctx.available_rect().width() * 0.40).clamp(180.0, 360.0);
        egui::SidePanel::left("library")
            .default_width(340.0_f32.min(maximum_width))
            .min_width(220.0_f32.min(maximum_width))
            .max_width(maximum_width)
            .resizable(true)
            .show(ctx, |ui| {
                if self.left_editor == Some(EditorPanel::Settings) {
                    self.settings_panel(ui);
                    return;
                }
                if self.left_editor == Some(EditorPanel::Metadata) {
                    self.metadata_panel(ui);
                    return;
                }
                if self.left_editor == Some(EditorPanel::Instruments) {
                    self.instrument_review_panel(ui);
                    return;
                }
                ui.heading("Library options");
                egui::ScrollArea::vertical()
                    .id_salt("library_controls")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label("Music library folder");
                        self.text_input_active |= ui
                            .add(
                                egui::TextEdit::singleline(&mut self.library_folder_input)
                                    .id_salt("library_folder_path")
                                    .desired_width(260.0)
                                    .hint_text("Folder for your music PDFs"),
                            )
                            .text_edit_has_focus();
                        ui.horizontal(|ui| {
                            let idle = self.import_task.is_none()
                                && self.picker.is_none()
                                && self.detection_job.is_none();
                            if ui.add_enabled(idle, egui::Button::new("Choose…")).clicked() {
                                self.picker_sets_library = true;
                                self.choose(true, ctx);
                            }
                            if ui
                                .add_enabled(idle, egui::Button::new("Set folder"))
                                .clicked()
                                && !self.library_folder_input.trim().is_empty()
                            {
                                self.change_music_folder(
                                    expand_path(self.library_folder_input.trim()),
                                    ctx,
                                );
                            }
                            if ui
                                .add_enabled(
                                    idle && self.paths.music.is_some(),
                                    egui::Button::new("Rescan"),
                                )
                                .clicked()
                            {
                                self.start_rescan(ctx);
                            }
                        });
                        if let Some(error) = &self.watch_error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        } else if self.changing_folder {
                            ui.label("Preparing music folder… Original files are kept.");
                        } else if self.watcher.is_some() {
                            ui.label("Watching folder and subfolders for new PDFs (every 2s)");
                        } else {
                            ui.label("Set a folder to enable automatic PDF importing.");
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            let enabled = self.import_task.is_none() && self.picker.is_none();
                            if ui
                                .add_enabled(enabled, egui::Button::new("Import folder…"))
                                .clicked()
                            {
                                self.picker_sets_library = false;
                                self.choose(true, ctx);
                            }
                            if ui.add_enabled(enabled, egui::Button::new("PDF…")).clicked() {
                                self.picker_sets_library = false;
                                self.choose(false, ctx);
                            }
                        });
                        ui.horizontal(|ui| {
                            self.text_input_active |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.import_path)
                                        .id_salt("import_path")
                                        .hint_text("Or paste a PDF / folder path")
                                        .desired_width((ui.available_width() - 90.0).max(80.0)),
                                )
                                .text_edit_has_focus();
                            if ui
                                .add_enabled(
                                    self.import_task.is_none(),
                                    egui::Button::new("Import"),
                                )
                                .clicked()
                                && !self.import_path.trim().is_empty()
                            {
                                let path = expand_path(self.import_path.trim());
                                self.start_import(vec![path], ctx);
                            }
                        });
                        ui.checkbox(&mut self.prefs.recursive, "Include subfolders");
                        self.display_name_options(ui);
                        self.instrument_options(ui, ctx);
                        ui.separator();
                        ui.collapsing("Create setlist / collection", |ui| {
                            self.text_input_active |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.group_name)
                                        .id_salt("new_group_name"),
                                )
                                .text_edit_has_focus();
                            ui.checkbox(
                                &mut self.new_collection,
                                "Collection (otherwise ordered setlist)",
                            );
                            if ui.button("Create").clicked() {
                                let kind = if self.new_collection {
                                    "collection"
                                } else {
                                    "setlist"
                                };
                                if let Some(id) =
                                    self.result(self.db.create_group(&self.group_name, kind))
                                {
                                    self.selected_group = Some(id);
                                    self.show_all_to_add = true;
                                    self.group_name.clear();
                                    self.reload();
                                }
                            }
                        });
                        if let Some(group) = self.selected_group {
                            self.group_tools(ui, group);
                        }
                    });
            });
    }

    fn open_setlist_manager(&mut self, group: Option<i64>) {
        self.reload();
        self.setlist_manager = Some(SetlistManager::new(&self.groups, group));
        self.performance = false;
    }

    fn play_group(&mut self, group: i64, index: usize) {
        let Some(entries) = self.result(self.db.entries(group)) else {
            return;
        };
        let Some(entry) = entries.get(index) else {
            self.status = "This list is empty or the entry was removed.".into();
            return;
        };
        let setlist = self
            .groups
            .iter()
            .any(|g| g.id == group && g.kind == "setlist");
        self.reader.sequence = if setlist {
            entries.iter().map(|e| e.score_id).collect()
        } else {
            Vec::new()
        };
        self.reader.sequence_index = if setlist { index } else { 0 };
        self.load_score(entry.score_id, Some(0));
    }

    fn manage_setlists(&mut self, ctx: &egui::Context) {
        if let Some(mut manager) = self.setlist_manager.take() {
            let outcome = manager.show(ctx, &mut self.db, &self.scores, &self.prefs.display_names);
            if outcome.changed {
                self.selected_group = outcome.selected;
                self.reload();
            }
            if let Some(error) = outcome.error {
                self.error = Some(error);
            }
            if let Some((group, index)) = outcome.play {
                self.play_group(group, index);
            }
            if outcome.open {
                self.setlist_manager = Some(manager);
            }
        }
    }

    fn group_tools(&mut self, ui: &mut egui::Ui, group: i64) {
        let is_setlist = self
            .groups
            .iter()
            .any(|g| g.id == group && g.kind == "setlist");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Manage this list…").clicked() {
                self.open_setlist_manager(Some(group));
            }
            if ui
                .add_enabled(
                    !self.entries.is_empty(),
                    egui::Button::new(if is_setlist {
                        "Play list"
                    } else {
                        "Open first score"
                    }),
                )
                .clicked()
            {
                self.play_group(group, 0);
            }
        });
    }

    fn reader_controls(&mut self, ui: &mut egui::Ui, score: &Score) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("◀ Previous").clicked() {
                self.turn(false);
            }
            if ui.button("Next ▶").clicked() {
                self.turn(true);
            }
            ui.label(format!("Page {} / {}", self.reader.page + 1, score.pages));
            let response = ui.add(egui::DragValue::new(&mut self.jump).range(1..=score.pages));
            self.text_input_active |= response.text_edit_has_focus();
            if response.changed() {
                self.reader.jump(self.jump.saturating_sub(1));
                self.changed_page();
            }
            if ui
                .checkbox(&mut self.reader.two_pages, "Two pages")
                .changed()
            {
                self.renderer.invalidate_requests();
            }
            ui.checkbox(&mut self.fit_width, "Fit width");
            ui.add(egui::Slider::new(&mut self.zoom, 0.5..=3.0).text("Zoom"));
            if ui.button("Reset zoom").clicked() {
                self.zoom = 1.0;
            }
            if ui.button("Bookmark").clicked() {
                self.result(self.db.toggle_bookmark(score.id, self.reader.page));
                self.bookmarks = self.result(self.db.bookmarks(score.id)).unwrap_or_default();
            }
            ui.menu_button("Bookmarks", |ui| {
                if self.bookmarks.is_empty() {
                    ui.label("No bookmarks yet");
                }
                for (page, label) in self.bookmarks.clone() {
                    if ui.button(label).clicked() {
                        self.reader.jump(page);
                        self.changed_page();
                        ui.close_menu();
                    }
                }
            });
        });
        if !self.reader.sequence.is_empty() {
            ui.label(format!(
                "Setlist item {} of {}",
                self.reader.sequence_index + 1,
                self.reader.sequence.len()
            ));
        }
    }

    fn score_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(score) = self
                .scores
                .iter()
                .find(|s| Some(s.id) == self.reader.score_id)
                .cloned()
            else {
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    ui.add_space(100.0);
                    ui.heading("Your music. Ready to play.");
                    ui.label("Import a PDF folder, then select a score from the library.");
                    ui.label("Original files stay untouched. Your library works offline.");
                });
                return;
            };
            if !self.performance {
                let controls_height = (ui.available_height() * 0.3).clamp(28.0, 140.0);
                egui::ScrollArea::vertical()
                    .id_salt("reader_controls")
                    .max_height(controls_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.heading(self.prefs.display_names.format(&score.title));
                        self.reader_controls(ui, &score);
                    });
                ui.separator();
            }
            let available = ui.available_size();
            let count = if self.reader.two_pages && self.reader.page + 1 < score.pages {
                2
            } else {
                1
            };
            let pixels = ((available.x.max(available.y) * ctx.pixels_per_point() * self.zoom)
                as u32)
                .clamp(1024, 2560);
            // Coarse buckets prevent a separate render for every window-resize pixel.
            let pixels = pixels.div_ceil(512) * 512;
            let pages: Vec<_> = (0..count)
                .map(|offset| PageKey {
                    hash: score.hash.clone(),
                    page: self.reader.page + offset,
                    pixels,
                })
                .collect();
            for key in &pages {
                self.renderer.request(key.clone());
            }
            for page in [
                self.reader.page.checked_sub(1),
                self.reader.page.checked_add(count),
                self.reader.page.checked_add(count + 1),
            ]
            .into_iter()
            .flatten()
            .filter(|&p| p < score.pages)
            {
                self.renderer.request(PageKey {
                    hash: score.hash.clone(),
                    page,
                    pixels,
                });
            }
            egui::ScrollArea::both()
                .id_salt(("page_scroll", score.id, self.reader.page))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for key in pages {
                            if let Some(texture) = self.renderer.texture(&key) {
                                let native = texture.size_vec2();
                                let width = ((available.x - 20.0) / count as f32).max(64.0);
                                let scale = if self.fit_width {
                                    width / native.x
                                } else {
                                    (width / native.x)
                                        .min((available.y - 12.0).max(64.0) / native.y)
                                } * self.zoom;
                                let response = ui.add(
                                    egui::Image::new(&texture)
                                        .fit_to_exact_size(native * scale)
                                        .sense(egui::Sense::click()),
                                );
                                if response.clicked() {
                                    if let Some(pos) = response.interact_pointer_pos() {
                                        self.turn(pos.x >= response.rect.center().x);
                                    }
                                }
                            } else if let Some(error) = self.renderer.failure(&key) {
                                ui.vertical(|ui| {
                                    ui.colored_label(Color32::LIGHT_RED, error);
                                });
                            } else {
                                ui.allocate_ui(
                                    Vec2::new((available.x / count as f32).max(64.0), 120.0),
                                    |ui| {
                                        ui.spinner();
                                        ui.label(format!("Rendering page {}…", key.page + 1));
                                    },
                                );
                            }
                        }
                    });
                });
        });
    }

    fn open_metadata_editor(&mut self) {
        if self.rescanning || self.detection_job.is_some() {
            self.status = "Wait for the library task to finish before editing metadata.".into();
            return;
        }
        if let Some(draft) = &self.editing {
            if self.reader.score_id.is_some_and(|id| id != draft.id) {
                self.status =
                    "Finish or cancel the existing metadata draft before editing another score."
                        .into();
            }
        } else {
            self.editing = self
                .scores
                .iter()
                .find(|score| Some(score.id) == self.reader.score_id)
                .cloned();
        }
        if self.editing.is_some() {
            self.left_editor = Some(EditorPanel::Metadata);
        }
    }

    fn save_settings(&mut self) {
        if self.prefs.next_key == self.prefs.previous_key {
            self.error = Some("Next and previous must use different keys".into());
        } else if self
            .result(self.paths.save_preferences(&self.prefs))
            .is_some()
        {
            self.status = "Preferences saved.".into();
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.horizontal_wrapped(|ui| {
            ui.heading("Settings");
            if ui.button("Close settings").clicked() {
                self.left_editor = None;
            }
        });
        if ui.button("Save preferences").clicked() {
            self.save_settings();
        }
        ui.separator();
        egui::ScrollArea::vertical().id_salt("settings_editor")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false]).show(ui, |ui| {

                ui.heading("Power management");
                if ui.checkbox(&mut self.prefs.keep_awake,
                    "Prevent screen dimming/timeout and suspend while running").changed() {
                    if self.result(self.paths.save_preferences(&self.prefs)).is_some() {
                        self.request_keep_awake();
                    } else { self.prefs.keep_awake = !self.prefs.keep_awake; }
                }
                ui.label("Applies immediately and is saved automatically. Released on disable or exit.");
                if self.prefs.keep_awake {
                    ui.colored_label(
                        if self.power_status.complete() { Color32::LIGHT_GREEN } else { Color32::YELLOW },
                        &self.power_status.message,
                    );
                    if !self.power_status.complete() && ui.button("Retry keep-awake request").clicked() {
                        self.request_keep_awake();
                    }
                    ui.label("Desktop policy, forced sleep, lid handling, and critical-battery protection may override this request.");
                }
                ui.separator();
                ui.label("Map a keyboard-emulating USB/Bluetooth pedal.");
                ui.label("ArrowLeft / ArrowRight also turn pages. F11: fullscreen. Esc: exit performance.");
                let keys = ["PageDown", "PageUp", "Space", "Enter", "ArrowDown", "ArrowUp",
                    "ArrowLeft", "ArrowRight", "N", "P", "B"];
                for (label, value) in [
                    ("Next page", &mut self.prefs.next_key),
                    ("Previous page", &mut self.prefs.previous_key),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        egui::ComboBox::from_id_salt(label).selected_text(value.clone()).show_ui(ui, |ui| {
                            for key in keys { ui.selectable_value(value, key.into(), key); }
                        });
                    });
                }
                ui.checkbox(&mut self.prefs.recursive, "Import subfolders by default");
                ui.separator();
                ui.heading("Instrument detection");
                ui.horizontal(|ui| {
                    ui.label("Initial pages:");
                    ui.add(egui::DragValue::new(&mut self.prefs.detection_options.pages).range(1..=6));
                });
                ui.label("Tesseract languages (installed language data required):");
                ui.text_edit_singleline(&mut self.prefs.detection_options.language);
                ui.label("Default eng; examples for installed extra data: eng+deu or eng+fra.");
                ui.checkbox(&mut self.prefs.detect_on_import, "Detect new imports and fill clear unassigned values");
                ui.separator();
                ui.label(format!("Music folder: {}", self.paths.score_dir().display()));
                ui.label(format!("Metadata: {}", self.paths.data.display()));
                ui.label(format!("Version {} — PDF library / reader milestone.", env!("CARGO_PKG_VERSION")));
                ui.label("Annotations, audio, MIDI, sync and full feature parity are not implemented yet.");
            });
    }

    fn save_metadata(&mut self, score: &Score) -> bool {
        if self.rescanning || self.detection_job.is_some() {
            return false;
        }
        if self.result(self.db.save_score(score)).is_none() {
            return false;
        }
        self.editing = None;
        self.left_editor = None;
        self.reload();
        self.status = "Score metadata saved.".into();
        true
    }

    fn cancel_metadata(&mut self) {
        self.editing = None;
        self.left_editor = None;
    }

    fn metadata_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Edit metadata");
            if ui
                .button("Back")
                .on_hover_text("Keep this draft and return to the previous panel")
                .clicked()
            {
                self.left_editor = None;
            }
        });
        let Some(mut score) = self.editing.take() else {
            ui.label("Select a score to edit its metadata.");
            return;
        };
        let mut save = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            save = ui
                .add_enabled(
                    !self.rescanning && self.detection_job.is_none(),
                    egui::Button::new("Save metadata"),
                )
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
        ui.add(egui::Label::new(format!("Editing: {}", score.title)).truncate());
        if ui.button("Review / detect instruments").clicked() {
            self.instrument_review.open(Some(score.id));
            self.left_editor = Some(EditorPanel::Instruments);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("metadata_editor")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (label, value) in [
                    ("Title", &mut score.title),
                    ("Composer", &mut score.composer),
                    ("Artist", &mut score.artist),
                    ("Genre", &mut score.genre),
                    ("Key", &mut score.musical_key),
                    ("Tags", &mut score.tags),
                ] {
                    ui.label(label);
                    self.text_input_active |= ui
                        .add(
                            egui::TextEdit::singleline(value)
                                .interactive(!self.rescanning && self.detection_job.is_none())
                                .id(egui::Id::new(("metadata_field", score.id, label)))
                                .desired_width((ui.available_width() - 8.0).max(24.0)),
                        )
                        .text_edit_has_focus();
                }
                ui.label("Notes");
                self.text_input_active |= ui
                    .add(
                        egui::TextEdit::multiline(&mut score.notes)
                            .interactive(!self.rescanning && self.detection_job.is_none())
                            .id(egui::Id::new(("metadata_notes", score.id)))
                            .desired_width((ui.available_width() - 8.0).max(24.0))
                            .desired_rows(5),
                    )
                    .text_edit_has_focus();
                Self::edit_instruments(
                    ui,
                    &mut score,
                    !self.rescanning && self.detection_job.is_none(),
                );
            });
        if cancel {
            self.cancel_metadata();
        } else if !save || !self.save_metadata(&score) {
            self.editing = Some(score);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Action needed")
                .open(&mut open)
                .max_width((ctx.screen_rect().width() - 24.0).max(180.0))
                .max_height((ctx.screen_rect().height() - 48.0).max(96.0))
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, error);
                });
            if !open {
                self.error = None;
            }
        }
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(summary) = &self.detection_summary {
                    ui.label(format!(
                        "Instruments: {}/{} · {} failed{}",
                        summary.processed,
                        summary.total,
                        summary.failed,
                        if summary.cancelled {
                            " · cancelled"
                        } else {
                            ""
                        }
                    ))
                    .on_hover_text(&summary.current);
                    if !summary.errors.is_empty() {
                        ui.menu_button("Detection errors", |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(240.0)
                                .show(ui, |ui| {
                                    for error in &summary.errors {
                                        ui.label(error);
                                    }
                                });
                        });
                    }
                }
                if let Some(job) = &self.detection_job {
                    ui.spinner();
                    if ui.button("Cancel detection").clicked() {
                        job.cancel();
                    }
                }
                if self.prefs.keep_awake {
                    ui.colored_label(
                        if self.power_status.complete() {
                            Color32::LIGHT_GREEN
                        } else {
                            Color32::YELLOW
                        },
                        if self.power_status.complete() {
                            "Keep awake"
                        } else {
                            "Keep awake: check Settings"
                        },
                    )
                    .on_hover_text(&self.power_status.message);
                }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
                if let Some(report) = &self.report {
                    ui.label(format!(
                        "{} imported · {} duplicates · {} removed · {} failed{}",
                        report.imported,
                        report.skipped,
                        report.removed,
                        report.failed,
                        if report.cancelled {
                            " · cancelled"
                        } else {
                            ""
                        }
                    ));
                    if let Some(backup) = &report.backup {
                        ui.menu_button("Metadata backup", |ui| {
                            ui.label("Before-rescan metadata backup:");
                            ui.monospace(backup.display().to_string());
                            ui.label("Existing PDFs are never deleted by Rescan.");
                        });
                    }
                    if !report.errors.is_empty() {
                        ui.menu_button("Import errors", |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(250.0)
                                .show(ui, |ui| {
                                    ui.label("First 100 errors:");
                                    for error in &report.errors {
                                        ui.label(error);
                                    }
                                });
                        });
                    }
                }
                if let Some(task) = &self.import_task {
                    ui.spinner();
                    if ui
                        .button(if self.rescanning {
                            "Cancel rescan"
                        } else {
                            "Cancel import"
                        })
                        .clicked()
                    {
                        task.cancel.store(true, Ordering::Relaxed);
                    }
                    if let Some(report) = &self.report {
                        ui.label(&report.current);
                    }
                }
                if self.performance {
                    if ui.button("Exit performance").clicked() {
                        self.performance = false;
                        self.fullscreen = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                    ui.label(format!("{} / {}", self.reader.page + 1, self.reader.pages));
                    ui.label("Tap left/right page halves to turn · Esc to leave performance");
                }
            });
        });
    }
}

impl eframe::App for MusicApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ime.prepare(ctx);
        self.poll(ctx);
        self.shortcuts(ctx);
        self.text_input_active = false;
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !self.help.open && !dropped.is_empty() {
            self.start_import(dropped, ctx);
        }
        if !self.performance || self.help.open {
            self.toolbar(ctx);
            if self.sidebar_visible() {
                self.sidebar(ctx);
            }
        }
        self.status_bar(ctx);
        self.workspace_view(ctx);
        if !self.help.open {
            self.manage_setlists(ctx);
            self.dialogs(ctx);
        }
        if self.smoke_deadline.is_some_and(|at| Instant::now() >= at) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Ensures delayed saves, cancelled imports, and full worker queues make progress.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl Drop for MusicApp {
    fn drop(&mut self) {
        self.detection_job.take();
        self.keep_awake.take();
        self.persist_page();
        self.watcher.take();
        if let Some(mut task) = self.import_task.take() {
            task.cancel.store(true, Ordering::Relaxed);
            drop(task.rx);
            if let Some(worker) = task.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

pub fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> (tempfile::TempDir, egui::Context, MusicApp) {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: root.path().join("data"),
            config: root.path().join("config"),
            cache: root.path().join("cache"),
            music: None,
        };
        paths.create().unwrap();
        let ctx = egui::Context::default();
        let app = MusicApp::from_context(&ctx, paths, None).unwrap();
        (root, ctx, app)
    }

    #[test]
    fn background_folder_selection_persists_and_reading_keeps_watching() {
        let (root, ctx, mut app) = test_app();
        let destination = root.path().join("Selected music folder");
        app.change_music_folder(destination.clone(), &ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.import_task.is_some() {
            app.poll(&ctx);
            assert!(Instant::now() < deadline, "Folder change failed to finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.paths.music, Some(destination.clone()));
        assert_eq!(app.db.music_folder().unwrap(), Some(destination));
        assert!(app.watcher.is_some());
        let id = app
            .db
            .insert("reading-test", "Reading test", 2, "")
            .unwrap();
        app.reload();
        app.load_score(id, None);
        assert!(
            app.watcher.is_some(),
            "Opening a score must not stop automatic import"
        );
    }

    #[test]
    fn missing_saved_library_starts_at_home_without_rewriting_score_locations() {
        let (root, ctx, mut app) = test_app();
        let missing = root.path().join("Unplugged music library");
        app.db.set_music_folder(&missing, &[]).unwrap();
        let paths = app.paths.clone();
        drop(app);
        let restarted = MusicApp::from_context(&ctx, paths, None).unwrap();
        assert_eq!(
            restarted.library_folder_input,
            picker::home_directory().display().to_string()
        );
        assert_eq!(restarted.paths.music, Some(missing.clone()));
        assert_eq!(restarted.db.music_folder().unwrap(), Some(missing.clone()));
        assert!(!missing.exists());
    }

    #[test]
    fn shrinking_window_keeps_the_score_list_visible() {
        let (_root, ctx, mut app) = test_app();
        app.db
            .insert("resize", "Resize regression score", 2, "")
            .unwrap();
        app.reload();
        fn visible(shape: &egui::Shape, clip: egui::Rect) -> bool {
            match shape {
                egui::Shape::Text(text)
                    if text.galley.job.text.contains("Resize regression score") =>
                {
                    let intersection =
                        clip.intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    intersection.width() > 40.0 && intersection.height() > 8.0
                }
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| visible(shape, clip)),
                _ => false,
            }
        }
        for size in [
            [1280.0, 900.0],
            [800.0, 500.0],
            [640.0, 360.0],
            [1280.0, 900.0],
        ] {
            let mut shown = false;
            for _ in 0..3 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::from(size),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        app.toolbar(ctx);
                        app.sidebar(ctx);
                        app.status_bar(ctx);
                        app.library_view(ctx);
                    },
                );
                shown = output
                    .shapes
                    .iter()
                    .any(|shape| visible(&shape.shape, shape.clip_rect));
            }
            assert!(shown, "Score list was clipped after resizing to {size:?}");
        }
    }

    #[test]
    fn small_window_can_scroll_to_the_last_score() {
        let (_root, ctx, mut app) = test_app();
        for index in 0..100 {
            app.db
                .insert(
                    &format!("resize-{index}"),
                    &format!("Resize score {index:03}"),
                    2,
                    "",
                )
                .unwrap();
        }
        app.reload();
        let mut reached_last = false;
        for index in 0..40 {
            let events = if index > 2 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(500.0, 300.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -600.0),
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
                    app.toolbar(ctx);
                    app.sidebar(ctx);
                    app.status_bar(ctx);
                    app.library_view(ctx);
                },
            );
            reached_last |= output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text.contains("Resize score 099") => {
                    let visible = shape
                        .clip_rect
                        .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    visible.height() > 8.0 && visible.width() > 40.0
                }
                _ => false,
            });
        }
        assert!(
            reached_last,
            "The last score must remain reachable after shrinking"
        );
    }

    #[test]
    fn small_settings_window_can_scroll_to_save_preferences() {
        let (_root, ctx, mut app) = test_app();
        app.left_editor = Some(EditorPanel::Settings);
        let mut reachable = false;
        for index in 0..15 {
            let events = if index > 2 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(300.0, 200.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -120.0),
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
                |ctx| app.sidebar(ctx),
            );
            reachable |= output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Save preferences" => {
                    let visible = shape
                        .clip_rect
                        .intersect(egui::Rect::from_min_size(text.pos, text.galley.size()));
                    visible.height() > 8.0 && visible.width() > 40.0
                }
                _ => false,
            });
        }
        assert!(
            reachable,
            "Settings save action must remain reachable by scrolling"
        );
    }

    #[test]
    fn left_editors_replace_options_and_preserve_metadata_drafts() {
        let (_root, ctx, mut app) = test_app();
        let score = app.db.insert("editor", "Original title", 3, "").unwrap();
        app.reload();
        app.load_score(score, Some(1));
        app.switch_workspace(Workspace::Library);
        fn panel_labels(app: &mut MusicApp, ctx: &egui::Context) -> Vec<String> {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.sidebar(ctx),
            );
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                    _ => None,
                })
                .collect()
        }
        let labels = panel_labels(&mut app, &ctx);
        assert!(labels.iter().any(|s| s == "Library options"));
        app.left_editor = Some(EditorPanel::Settings);
        let labels = panel_labels(&mut app, &ctx);
        assert!(labels.iter().any(|s| s == "Power management"));
        assert!(!labels.iter().any(|s| s == "Library options"));
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().title = "Draft title".into();
        let labels = panel_labels(&mut app, &ctx);
        assert!(labels.iter().any(|s| s == "Save metadata"));
        assert!(!labels
            .iter()
            .any(|s| s == "Power management" || s == "Library options"));
        app.left_editor = Some(EditorPanel::Settings);
        app.open_metadata_editor();
        assert_eq!(app.editing.as_ref().unwrap().title, "Draft title");
        let draft = app.editing.as_ref().unwrap().clone();
        assert!(app.save_metadata(&draft));
        assert!(app.editing.is_none() && app.left_editor.is_none());
        assert!(app.sidebar_visible());
        assert_eq!(app.db.scores().unwrap()[0].title, "Draft title");
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().notes = "Discard this".into();
        app.cancel_metadata();
        assert!(app.db.scores().unwrap()[0].notes.is_empty());
        app.switch_workspace(Workspace::Reader);
        assert!(!app.sidebar_visible());
        app.left_editor = Some(EditorPanel::Settings);
        assert!(app.sidebar_visible());
        app.left_editor = None;
        assert!(!app.sidebar_visible());
        assert_eq!(app.reader.page, 1);
    }

    #[test]
    fn help_preserves_reader_and_metadata_and_blocks_page_keys() {
        let (_root, ctx, mut app) = test_app();
        let id = app
            .db
            .insert("help-score", "Help test score", 5, "")
            .unwrap();
        app.reload();
        app.load_score(id, Some(2));
        app.query = "Help test".into();
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().notes = "Unsaved draft".into();
        press(&ctx, &mut app, Key::F1);
        assert!(app.help.open);
        assert!(!app.sidebar_visible());
        press(&ctx, &mut app, Key::PageDown);
        press(&ctx, &mut app, Key::Home);
        assert_eq!(app.reader.page, 2);
        assert_eq!(app.workspace, Workspace::Reader);
        assert_eq!(app.query, "Help test");
        assert_eq!(app.editing.as_ref().unwrap().notes, "Unsaved draft");
        press(&ctx, &mut app, Key::Escape);
        assert!(!app.help.open);
        assert_eq!(app.left_editor, Some(EditorPanel::Metadata));
        assert!(app.sidebar_visible());
        app.cancel_metadata();
        press(&ctx, &mut app, Key::PageDown);
        assert_eq!(app.reader.page, 3);
        app.performance = true;
        app.fullscreen = true;
        press(&ctx, &mut app, Key::F1);
        press(&ctx, &mut app, Key::Escape);
        assert!(app.performance && app.fullscreen);
        assert_eq!(app.reader.page, 3);
    }

    #[test]
    fn metadata_sidebar_accepts_virtual_keyboard_input_without_saving_early() {
        let (_root, ctx, mut app) = test_app();
        let id = app.db.insert("ime-editor", "Original", 2, "").unwrap();
        app.reload();
        app.load_score(id, Some(0));
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().title.clear();
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("metadata_field", id, "Title"))));
        for character in std::iter::once(None).chain("Bach".chars().map(Some)) {
            let events = character.map_or_else(Vec::new, |c| {
                vec![
                    egui::Event::Ime(egui::ImeEvent::Commit(c.to_string())),
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ]
            });
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.ime.prepare(ctx);
                    app.sidebar(ctx);
                },
            );
        }
        assert_eq!(app.editing.as_ref().unwrap().title, "Bach");
        assert_eq!(app.db.scores().unwrap()[0].title, "Original");
        let draft = app.editing.as_ref().unwrap().clone();
        assert!(app.save_metadata(&draft));
        assert_eq!(app.db.scores().unwrap()[0].title, "Bach");
    }

    #[test]
    fn rescan_reloads_open_reader_and_removes_stale_metadata_and_list_references() {
        let (_root, ctx, mut app) = test_app();
        let id = app.db.insert("missing-pdf", "Deleted PDF", 2, "").unwrap();
        let group = app.db.create_group("Concert", "setlist").unwrap();
        for _ in 0..2 {
            app.db.add_entry(group, id).unwrap();
        }
        app.db.toggle_bookmark(id, 1).unwrap();
        app.reload();
        app.play_group(group, 1);
        app.open_metadata_editor(); // Unmodified draft is safe to close if its score is removed.
        app.start_rescan(&ctx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.import_task.is_some() {
            app.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.report.as_ref().unwrap().removed, 1);
        assert!(app
            .report
            .as_ref()
            .unwrap()
            .backup
            .as_ref()
            .unwrap()
            .exists());
        assert_eq!(app.workspace, Workspace::Library);
        assert!(app.reader.score_id.is_none() && app.reader.sequence.is_empty());
        assert!(app.editing.is_none() && app.left_editor.is_none());
        assert!(app.db.entries(group).unwrap().is_empty());
        assert!(app.db.bookmarks(id).unwrap().is_empty());
    }

    #[test]
    fn rescan_does_not_discard_a_dirty_metadata_draft() {
        let (_root, ctx, mut app) = test_app();
        let id = app.db.insert("dirty", "Original", 2, "").unwrap();
        app.reload();
        app.load_score(id, Some(0));
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().notes = "Unsaved notes".into();
        app.left_editor = None; // User returned to Library options.
        app.start_rescan(&ctx);
        assert!(app.import_task.is_none());
        assert_eq!(app.left_editor, Some(EditorPanel::Metadata));
        assert_eq!(app.editing.as_ref().unwrap().notes, "Unsaved notes");
        assert_eq!(app.db.scores().unwrap().len(), 1);
    }

    #[test]
    fn library_pane_does_not_grow_across_frames() {
        let (_root, ctx, mut app) = test_app();
        app.db
            .insert("pane-test", "A score in the library", 2, "")
            .unwrap();
        app.reload();
        for _ in 0..100 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 900.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                app.sidebar(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(
                        ui.available_width() > 700.0,
                        "Library sidebar crowded out the reader: {}",
                        ui.available_width()
                    );
                });
            });
        }
    }

    fn press(ctx: &egui::Context, app: &mut MusicApp, key: Key) {
        let input = egui::RawInput {
            events: vec![
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.shortcuts(ctx));
    }

    #[test]
    fn page_keys_work_with_button_focus_but_not_while_typing() {
        let (_root, ctx, mut app) = test_app();
        let id = app.db.insert("test", "Keyboard score", 3, "").unwrap();
        app.reload();
        app.load_score(id, None);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("button")));
        press(&ctx, &mut app, Key::PageDown);
        assert_eq!(app.reader.page, 1);
        app.text_input_active = true;
        press(&ctx, &mut app, Key::PageDown);
        assert_eq!(app.reader.page, 1, "Typing must not change the score page");
        app.text_input_active = false;
        app.prefs.previous_key = "B".into();
        press(&ctx, &mut app, Key::B);
        assert_eq!(app.reader.page, 0);
        app.prefs.next_key = "Space".into();
        press(&ctx, &mut app, Key::Space);
        assert_eq!(app.reader.page, 1);
    }

    #[test]
    fn virtual_keyboard_can_filter_the_real_library_across_many_frames() {
        let (_root, ctx, mut app) = test_app();
        let bach = app.db.insert("bach", "Bach Prelude", 3, "").unwrap();
        app.db
            .insert("beethoven", "Beethoven Sonata", 2, "")
            .unwrap();
        app.reload();
        app.load_score(bach, Some(0));
        app.switch_workspace(Workspace::Library);
        app.prefs.next_key = "A".into();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("library_search")));
        for (index, character) in std::iter::once(None)
            .chain("Bach".chars().map(Some))
            .enumerate()
        {
            let mut events = Vec::new();
            if let Some(character) = character {
                events.push(egui::Event::Ime(egui::ImeEvent::Commit(
                    character.to_string(),
                )));
                events.push(egui::Event::Ime(egui::ImeEvent::Disabled));
                if character == 'a' {
                    events.push(egui::Event::Key {
                        key: Key::A,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            app.reload(); // Folder-watch refreshes must not reset the search editor.
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1280.0, 900.0),
                    )),
                    focused: index != 2,
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.ime.prepare(ctx);
                    app.shortcuts(ctx);
                    app.text_input_active = false;
                    app.sidebar(ctx);
                    app.library_view(ctx);
                },
            );
        }
        assert_eq!(app.query, "Bach");
        assert_eq!(app.visible_scores().len(), 1);
        assert_eq!(app.visible_scores()[0].id, bach);
        assert_eq!(
            app.reader.page, 0,
            "Typed letters must not act as pedal shortcuts"
        );
    }

    #[test]
    fn library_reader_tabs_preserve_search_and_reading_position() {
        let (_root, ctx, mut app) = test_app();
        assert_eq!(app.workspace, Workspace::Library);
        app.switch_workspace(Workspace::Reader);
        assert_eq!(
            app.workspace,
            Workspace::Library,
            "Reader needs an open score"
        );
        let score = app.db.insert("tabs", "Bach Prelude", 4, "").unwrap();
        app.reload();
        app.load_score(score, Some(2));
        assert_eq!(app.workspace, Workspace::Reader);
        app.query = "Bach".into();
        app.sort = 1;
        app.switch_workspace(Workspace::Library);
        assert_eq!(app.workspace, Workspace::Library);
        assert_eq!(app.query, "Bach");
        assert_eq!(app.sort, 1);
        assert_eq!(app.reader.page, 2);
        assert_eq!(app.db.scores().unwrap()[0].last_page, 2);
        press(&ctx, &mut app, Key::PageDown);
        assert_eq!(
            app.reader.page, 2,
            "Library navigation must not turn hidden score pages"
        );
        app.switch_workspace(Workspace::Reader);
        press(&ctx, &mut app, Key::PageDown);
        assert_eq!(app.reader.page, 3);
        assert_eq!(app.query, "Bach");
    }

    #[test]
    fn scores_are_in_the_main_pane_and_options_can_be_hidden() {
        let (_root, ctx, mut app) = test_app();
        app.db.insert("wide", "Wide browser score", 2, "").unwrap();
        app.reload();
        for options in [true, false] {
            app.show_options = options;
            let mut clip = None;
            for _ in 0..3 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        app.toolbar(ctx);
                        if app.show_options {
                            app.sidebar(ctx);
                        }
                        app.status_bar(ctx);
                        app.library_view(ctx);
                    },
                );
                clip = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.job.text.contains("Wide browser score") =>
                    {
                        Some(shape.clip_rect)
                    }
                    _ => None,
                });
            }
            let clip = clip.expect("Score should be rendered");
            assert!(
                clip.width() > 700.0,
                "Scores should use the large central pane"
            );
            if options {
                assert!(
                    clip.left() > 200.0,
                    "Scores should be right of the options panel"
                );
            } else {
                assert!(
                    clip.left() < 30.0 && clip.width() > 1200.0,
                    "Hidden options should give the browser full width"
                );
            }
        }
    }

    #[test]
    fn central_browser_opens_scores_and_adds_to_lists_without_opening() {
        let (_root, ctx, mut app) = test_app();
        let score = app.db.insert("open", "Open browser score", 3, "").unwrap();
        app.db.remember_page(score, 2).unwrap();
        let group = app.db.create_group("Concert", "setlist").unwrap();
        app.reload();
        ctx.style_mut(|style| style.animation_time = 0.0);
        fn draw(
            app: &mut MusicApp,
            ctx: &egui::Context,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.toolbar(ctx);
                    app.sidebar(ctx);
                    app.status_bar(ctx);
                    app.library_view(ctx);
                },
            )
        }
        fn click(app: &mut MusicApp, ctx: &egui::Context, text: &str) {
            let output = draw(app, ctx, vec![]);
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(label) if label.galley.job.text.starts_with(text) => {
                        Some(label.pos + label.galley.size() / 2.0)
                    }
                    _ => None,
                })
                .expect("Browser button must be rendered");
            draw(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            draw(
                app,
                ctx,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        for _ in 0..3 {
            draw(&mut app, &ctx, vec![]);
        }
        click(&mut app, &ctx, "Open browser score");
        assert_eq!(app.workspace, Workspace::Reader);
        assert_eq!(app.reader.score_id, Some(score));
        assert_eq!(app.reader.page, 2);
        app.switch_workspace(Workspace::Library);
        app.selected_group = Some(group);
        app.show_all_to_add = true;
        app.refresh_entries();
        click(&mut app, &ctx, "+ Add");
        assert_eq!(app.workspace, Workspace::Library);
        assert_eq!(app.db.entries(group).unwrap()[0].score_id, score);
    }

    #[test]
    fn instrument_filters_support_multi_instrument_scores_and_manual_edits() {
        let (_root, _ctx, mut app) = test_app();
        let a = app.db.insert("a", "Duet", 1, "").unwrap();
        let b = app.db.insert("b", "Sax part", 1, "").unwrap();
        let c = app.db.insert("c", "Unassigned", 1, "").unwrap();
        app.db
            .apply_instruments(
                a,
                "a",
                &[
                    Instrument {
                        name: "Cello".into(),
                        ..Default::default()
                    },
                    Instrument {
                        name: "Piano".into(),
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        app.db
            .apply_instruments(
                b,
                "b",
                &[Instrument {
                    name: "Saxophone".into(),
                    variant: "Alto".into(),
                    part: "1".into(),
                }],
            )
            .unwrap();
        app.reload();
        for name in ["Cello", "Piano"] {
            app.instrument_filter = Some(name.into());
            assert_eq!(
                app.visible_scores()
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>(),
                [a]
            );
        }
        app.instrument_filter = Some(String::new());
        assert_eq!(app.visible_scores()[0].id, c);
        app.instrument_filter = None;
        app.query = "Alto Saxophone".into();
        assert_eq!(app.visible_scores()[0].id, b);
        app.load_score(b, Some(0));
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().instruments[0].variant = "Tenor".into();
        let draft = app.editing.as_ref().unwrap().clone();
        assert!(app.save_metadata(&draft));
        assert!(
            app.db
                .scores()
                .unwrap()
                .iter()
                .find(|s| s.id == b)
                .unwrap()
                .instruments_manual
        );
        app.query = "Tenor".into();
        assert_eq!(app.visible_scores()[0].id, b);
    }

    #[test]
    fn external_document_import_runs_in_background_without_losing_reader_or_draft() {
        struct Documents(Vec<u8>);
        impl DocumentAccess for Documents {
            fn metadata(&self, _source: &DocumentRef) -> Result<crate::source::Metadata> {
                Ok(crate::source::Metadata {
                    display_name: Some("External cello.png".into()),
                    mime_type: Some("image/png".into()),
                    size: Some(self.0.len() as u64),
                })
            }
            fn open(&self, _source: &DocumentRef) -> Result<Box<dyn std::io::Read + Send>> {
                Ok(Box::new(std::io::Cursor::new(self.0.clone())))
            }
        }
        let (_root, ctx, mut app) = test_app();
        let old = app
            .db
            .insert("existing-doc", "Existing score", 3, "")
            .unwrap();
        app.reload();
        app.load_score(old, Some(1));
        app.open_metadata_editor();
        app.editing.as_mut().unwrap().notes = "Keep my draft".into();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(40, 60, image::Rgb([255, 255, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        app.document_access = Arc::new(Documents(bytes.into_inner()));
        app.start_document_import(
            DocumentRef::Content(
                crate::source::ContentUri::parse("content://test.documents/document/score")
                    .unwrap(),
            ),
            &ctx,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.import_task.is_some() {
            app.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.report.as_ref().unwrap().imported, 1);
        assert_eq!(app.scores.len(), 2);
        assert_eq!(app.reader.score_id, Some(old));
        assert_eq!(app.reader.page, 1);
        assert_eq!(app.editing.as_ref().unwrap().notes, "Keep my draft");
        assert_eq!(std::fs::read_dir(&app.paths.cache).unwrap().count(), 0);
    }

    #[test]
    fn automatic_detection_queues_only_new_imports_and_waits_for_metadata_editing() {
        let (root, ctx, mut app) = test_app();
        let old = app
            .db
            .insert("old", "Existing unscanned part", 1, "")
            .unwrap();
        app.reload();
        app.prefs.detect_on_import = true;
        app.load_score(old, Some(0));
        app.open_metadata_editor();
        let source = root.path().join("New.png");
        image::RgbImage::from_pixel(100, 140, image::Rgb([255, 255, 255]))
            .save(&source)
            .unwrap();
        app.start_import(vec![source], &ctx);
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.import_task.is_some() {
            app.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            app.detection_job.is_none(),
            "Do not interrupt an open metadata editor"
        );
        assert_eq!(app.auto_detection_pending.len(), 1);
        app.cancel_metadata();
        app.poll(&ctx);
        assert!(app.detection_job.is_some());
        while app.detection_job.is_some() {
            app.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.detection_summary.as_ref().unwrap().total, 1);
        assert!(app
            .scores
            .iter()
            .find(|s| s.id == old)
            .unwrap()
            .detection_status
            .is_empty());
        assert!(!app
            .scores
            .iter()
            .find(|s| s.id != old)
            .unwrap()
            .detection_status
            .is_empty());
    }

    #[test]
    fn manager_playback_preserves_repeated_entries_and_snapshot_order() {
        let (_root, _ctx, mut app) = test_app();
        let a = app.db.insert("a", "First", 2, "").unwrap();
        let b = app.db.insert("b", "Second", 2, "").unwrap();
        let group = app.db.create_group("Concert", "setlist").unwrap();
        for score in [a, b, a] {
            app.db.add_entry(group, score).unwrap();
        }
        app.reload();
        app.play_group(group, 1);
        assert_eq!(app.reader.sequence, [a, b, a]);
        assert_eq!(app.reader.sequence_index, 1);
        assert_eq!(app.reader.score_id, Some(b));
        app.db.move_entry(group, 0, 1).unwrap();
        assert_eq!(
            app.reader.sequence,
            [a, b, a],
            "Do not change a live playback snapshot"
        );
        app.play_group(group, 0);
        assert_eq!(app.reader.sequence, [b, a, a]);
    }

    #[test]
    fn display_options_sort_and_search_without_changing_metadata() {
        let (_root, ctx, mut app) = test_app();
        let raw_z = "IMSLP100-PMLP111-Zebra_Sonata";
        let raw_a = "IMSLP900-PMLP222-Alpha_Sonata";
        let z = app.db.insert("z", raw_z, 2, "").unwrap();
        let a = app.db.insert("a", raw_a, 2, "").unwrap();
        app.reload();
        assert_eq!(app.visible_scores()[0].id, z);
        app.prefs.display_names.replace_characters = true;
        app.prefs.display_names.remove_imslp_prefix = true;
        assert_eq!(app.visible_scores()[0].id, a);
        app.query = "Alpha Sonata".into();
        assert_eq!(app.visible_scores().len(), 1);
        app.query = "IMSLP900".into();
        assert_eq!(
            app.visible_scores()[0].id,
            a,
            "Original identifiers stay searchable"
        );
        let saved = app.db.scores().unwrap();
        assert_eq!(saved.iter().find(|s| s.id == a).unwrap().title, raw_a);
        app.paths.save_preferences(&app.prefs).unwrap();
        let restarted = MusicApp::from_context(&ctx, app.paths.clone(), None).unwrap();
        assert_eq!(restarted.prefs.display_names.format(raw_a), "Alpha Sonata");
        assert_eq!(restarted.visible_scores()[0].id, a);
    }

    #[test]
    fn setlist_transitions_start_scores_at_correct_pages() {
        let (_root, _ctx, mut app) = test_app();
        let a = app.db.insert("a", "First", 3, "").unwrap();
        let b = app.db.insert("b", "Second", 2, "").unwrap();
        app.reload();
        app.reader.sequence = vec![a, b, a];
        app.load_score(a, Some(2));
        app.turn(true);
        assert_eq!((app.reader.score_id, app.reader.page), (Some(b), 0));
        assert_eq!(
            app.db
                .scores()
                .unwrap()
                .iter()
                .find(|s| s.id == a)
                .unwrap()
                .last_page,
            2
        );
        app.turn(false);
        assert_eq!((app.reader.score_id, app.reader.page), (Some(a), 2));
        app.turn(true);
        app.turn(true);
        assert_eq!((app.reader.score_id, app.reader.page), (Some(b), 1));
        app.turn(true);
        assert_eq!((app.reader.score_id, app.reader.page), (Some(a), 0));
        assert_eq!(app.reader.sequence_index, 2);
        app.turn(true);
        app.turn(true);
        app.turn(true);
        assert_eq!(app.status, "End of score / setlist");
    }
}
