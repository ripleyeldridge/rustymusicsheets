use crate::{
    folder,
    import::{self, Report},
    model::{Entry, Group, Paths, Preferences, Score},
    reader::{Reader, Turn},
    render::{PageKey, PageRenderer},
    storage::Library,
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

enum ImportMessage {
    Progress(Report),
    Done(Result<Report, String>),
    FolderReady(Paths),
}

pub struct MusicApp {
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
    settings_open: bool,
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
            library_folder_input: paths.score_dir().display().to_string(),
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
            settings_open: false,
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
        })
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
        let old_hash = self
            .scores
            .iter()
            .find(|s| Some(s.id) == self.reader.score_id)
            .map(|s| s.hash.clone());
        if let Some(scores) = self.result(self.db.scores()) {
            self.scores = scores;
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
            if self
                .result(self.db.remember_page(id, self.reader.page))
                .is_some()
            {
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

    fn start_watching(&mut self, ctx: &egui::Context) {
        self.watcher.take();
        if self.paths.music.is_some() {
            let repaint = ctx.clone();
            self.watcher = Some(FolderWatcher::start(self.paths.clone(), move || {
                repaint.request_repaint()
            }));
        }
    }

    fn change_music_folder(&mut self, destination: PathBuf, ctx: &egui::Context) {
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

    fn choose(&mut self, folder: bool, ctx: &egui::Context) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Option<PathBuf>> {
                let mut command = std::process::Command::new("zenity");
                command.args(["--file-selection", "--title=RustyMusicSheets — Import"]);
                if folder {
                    command.arg("--directory");
                } else {
                    command.arg("--file-filter=PDF files | *.pdf *.PDF");
                }
                let output = command.output().map_err(|e| {
                    anyhow::anyhow!("File picker unavailable ({e}); paste a path instead")
                })?;
                if !output.status.success() {
                    return Ok(None);
                }
                use std::os::unix::ffi::OsStringExt;
                let mut bytes = output.stdout;
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                }
                Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.picker = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.renderer.poll(ctx);
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
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.performance = false;
            self.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if self.text_input_active || self.editing.is_some() || self.settings_open {
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
                if ui.button("Library").clicked() {
                    self.performance = false;
                }
                if ui.button("Performance").clicked() && self.reader.score_id.is_some() {
                    self.performance = true;
                    self.fullscreen = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                if ui.button("Edit metadata").clicked() {
                    self.editing = self
                        .scores
                        .iter()
                        .find(|s| Some(s.id) == self.reader.score_id)
                        .cloned();
                }
                if ui.button("Retry renders").clicked() {
                    self.renderer.retry();
                }
            });
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("library")
            .default_width(340.0)
            .min_width(280.0)
            .max_width(420.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Music library");
                ui.label("Music library folder");
                self.text_input_active |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.library_folder_input)
                            .desired_width(260.0)
                            .hint_text("Folder for your music PDFs"),
                    )
                    .has_focus();
                ui.horizontal(|ui| {
                    let idle = self.import_task.is_none() && self.picker.is_none();
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
                        self.start_watching(ctx);
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
                                .hint_text("Or paste a PDF / folder path")
                                .desired_width((ui.available_width() - 90.0).max(80.0)),
                        )
                        .has_focus();
                    if ui
                        .add_enabled(self.import_task.is_none(), egui::Button::new("Import"))
                        .clicked()
                        && !self.import_path.trim().is_empty()
                    {
                        let path = expand_path(self.import_path.trim());
                        self.start_import(vec![path], ctx);
                    }
                });
                ui.checkbox(&mut self.prefs.recursive, "Include subfolders");
                ui.separator();
                let selected_name = self
                    .groups
                    .iter()
                    .find(|g| Some(g.id) == self.selected_group)
                    .map_or("All scores", |g| g.name.as_str())
                    .to_owned();
                let mut selected = self.selected_group;
                egui::ComboBox::from_id_salt("group_filter")
                    .selected_text(selected_name)
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
                if selected != self.selected_group {
                    self.selected_group = selected;
                    self.show_all_to_add = false;
                    self.refresh_entries();
                }
                ui.collapsing("Create setlist / collection", |ui| {
                    self.text_input_active |=
                        ui.text_edit_singleline(&mut self.group_name).has_focus();
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
                        if let Some(id) = self.result(self.db.create_group(&self.group_name, kind))
                        {
                            self.selected_group = Some(id);
                            self.show_all_to_add = true;
                            self.group_name.clear();
                            self.reload();
                        }
                    }
                });
                self.text_input_active |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Search all metadata…"),
                    )
                    .has_focus();
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("sort")
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
                        .filter(|s| !s.is_empty())
                        .collect();
                    genres.sort();
                    genres.dedup();
                    egui::ComboBox::from_id_salt("genre")
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
                if let Some(group) = self.selected_group {
                    self.group_editor(ui, group);
                    ui.checkbox(
                        &mut self.show_all_to_add,
                        "Show all scores to add to this group",
                    );
                }
                ui.separator();
                let mut visible: Vec<_> = self
                    .scores
                    .iter()
                    .filter(|s| {
                        s.matches(&self.query)
                            && (self.genre.is_empty() || s.genre == self.genre)
                            && (self.selected_group.is_none()
                                || self.show_all_to_add
                                || self.entries.iter().any(|entry| entry.score_id == s.id))
                    })
                    .cloned()
                    .collect();
                if self.sort == 1 {
                    visible.sort_by_key(|s| (s.composer.to_lowercase(), s.title.to_lowercase()));
                } else if self.sort == 2 {
                    visible.sort_by_key(|s| std::cmp::Reverse(s.id));
                }
                ui.label(format!(
                    "{} scores · {} matching",
                    self.scores.len(),
                    visible.len()
                ));
                egui::ScrollArea::vertical().id_salt("scores").show_rows(
                    ui,
                    60.0,
                    visible.len(),
                    |ui, range| {
                        for score in &visible[range] {
                            ui.horizontal(|ui| {
                                let text = if score.composer.is_empty() {
                                    format!("{}\n{} pages", score.title, score.pages)
                                } else {
                                    format!(
                                        "{}\n{} · {} pages",
                                        score.title, score.composer, score.pages
                                    )
                                };
                                let response = ui.add_sized(
                                    [ui.available_width() - 38.0, 52.0],
                                    egui::Button::new(text)
                                        .selected(self.reader.score_id == Some(score.id)),
                                );
                                if response.clicked() {
                                    self.reader.sequence.clear();
                                    self.reader.sequence_index = 0;
                                    self.load_score(score.id, None);
                                }
                                if let Some(group) = self.selected_group {
                                    if ui
                                        .button("+")
                                        .on_hover_text("Add to selected setlist / collection")
                                        .clicked()
                                    {
                                        self.result(self.db.add_entry(group, score.id));
                                        self.refresh_entries();
                                    }
                                }
                            });
                        }
                    },
                );
            });
    }

    fn group_editor(&mut self, ui: &mut egui::Ui, group: i64) {
        let is_setlist = self
            .groups
            .iter()
            .any(|g| g.id == group && g.kind == "setlist");
        ui.collapsing(
            if is_setlist {
                "Setlist order / play"
            } else {
                "Collection members"
            },
            |ui| {
                ui.label("Use + beside scores below to add them here.");
                egui::ScrollArea::vertical()
                    .id_salt("entries")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (index, entry) in self.entries.clone().iter().enumerate() {
                            let title = self
                                .scores
                                .iter()
                                .find(|s| s.id == entry.score_id)
                                .map_or("Missing score", |s| s.title.as_str())
                                .to_owned();
                            ui.horizontal(|ui| {
                                if ui.button(format!("{}. {}", index + 1, title)).clicked() {
                                    self.reader.sequence = if is_setlist {
                                        self.entries.iter().map(|e| e.score_id).collect()
                                    } else {
                                        Vec::new()
                                    };
                                    self.reader.sequence_index = if is_setlist { index } else { 0 };
                                    self.load_score(entry.score_id, Some(0));
                                }
                                if is_setlist {
                                    for (label, offset) in [("↑", -1), ("↓", 1)] {
                                        if ui.small_button(label).clicked() {
                                            let result = self.db.move_entry(group, index, offset);
                                            self.result(result);
                                            self.refresh_entries();
                                        }
                                    }
                                }
                                if ui
                                    .small_button("×")
                                    .on_hover_text("Remove this entry")
                                    .clicked()
                                {
                                    self.result(self.db.remove_entry(entry.id));
                                    self.refresh_entries();
                                }
                            });
                        }
                    });
                ui.menu_button("Delete this group…", |ui| {
                    ui.label("Scores and PDFs will be kept.");
                    if ui.button("Confirm delete group").clicked() {
                        self.result(self.db.delete_group(group));
                        self.selected_group = None;
                        self.reload();
                        ui.close_menu();
                    }
                });
            },
        );
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
            self.text_input_active |= response.has_focus();
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
                ui.heading(&score.title);
                self.reader_controls(ui, &score);
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

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut score) = self.editing.take() {
            let mut open = true;
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Score metadata")
                .open(&mut open)
                .resizable(true)
                .show(ctx, |ui| {
                    egui::Grid::new("metadata").num_columns(2).show(ui, |ui| {
                        for (label, value) in [
                            ("Title", &mut score.title),
                            ("Composer", &mut score.composer),
                            ("Artist", &mut score.artist),
                            ("Genre", &mut score.genre),
                            ("Key", &mut score.musical_key),
                            ("Tags", &mut score.tags),
                        ] {
                            ui.label(label);
                            ui.text_edit_singleline(value);
                            ui.end_row();
                        }
                    });
                    ui.label("Notes");
                    ui.text_edit_multiline(&mut score.notes);
                    ui.horizontal(|ui| {
                        save = ui.button("Save").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if save {
                if self.result(self.db.save_score(&score)).is_some() {
                    self.reload();
                } else {
                    self.editing = Some(score);
                }
            } else if open && !cancel {
                self.editing = Some(score);
            }
        }
        if self.settings_open {
            let mut open = true;
            egui::Window::new("Settings / pedal mappings").open(&mut open).show(ctx, |ui| {
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
                if ui.button("Save preferences").clicked() {
                    if self.prefs.next_key == self.prefs.previous_key {
                        self.error = Some("Next and previous must use different keys".into());
                    } else {
                        self.result(self.paths.save_preferences(&self.prefs));
                    }
                }
                ui.separator();
                ui.label(format!("Music folder: {}", self.paths.score_dir().display()));
                ui.label(format!("Metadata: {}", self.paths.data.display()));
                ui.label("Version 0.1 — PDF library / reader milestone.");
                ui.label("Annotations, audio, MIDI, sync and full feature parity are not implemented yet.");
            });
            self.settings_open = open;
        }
        if let Some(error) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Action needed")
                .open(&mut open)
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
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
                if let Some(report) = &self.report {
                    ui.label(format!(
                        "{} imported · {} duplicates · {} failed{}",
                        report.imported,
                        report.skipped,
                        report.failed,
                        if report.cancelled {
                            " · cancelled"
                        } else {
                            ""
                        }
                    ));
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
                    if ui.button("Cancel import").clicked() {
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
        if !dropped.is_empty() {
            self.start_import(dropped, ctx);
        }
        if !self.performance {
            self.toolbar(ctx);
            self.sidebar(ctx);
        }
        self.status_bar(ctx);
        self.score_view(ctx);
        self.dialogs(ctx);
        if self.smoke_deadline.is_some_and(|at| Instant::now() >= at) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Ensures delayed saves, cancelled imports, and full worker queues make progress.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl Drop for MusicApp {
    fn drop(&mut self) {
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
