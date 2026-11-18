use super::{
    canvas::{Canvas, Surface},
    model::{InkTool, PageInk, Stroke},
    store,
};
#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
use anyhow::{ensure, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use eframe::egui::{self, Color32, Key};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant, SystemTime},
};

const CACHE_PAGES: usize = 8;
const HISTORY_BYTES: usize = 4 * 1024 * 1024;
const HISTORY_STEPS: usize = 100;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
const WORKER_ERROR: &str =
    "Annotation worker disconnected; retry to restart it. Unsaved ink remains in memory.";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PageKey {
    hash: String,
    page: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: SystemTime,
}

fn stamp(path: &Path) -> Result<Stamp> {
    let metadata = std::fs::metadata(path)?;
    ensure!(metadata.is_file(), "Score is not a regular file");
    Ok(Stamp {
        bytes: metadata.len(),
        modified: metadata.modified()?,
    })
}

struct Binding {
    hash: String,
    pages: u32,
    source: PathBuf,
    stamp: Option<Stamp>,
    pending: bool,
    error: Option<String>,
    generation: u64,
}

struct PageState {
    pages: u32,
    ink: Option<PageInk>,
    token: Option<String>,
    load_error: Option<String>,
    save_error: Option<String>,
    pending: bool,
    writing: bool,
    dirty: Option<Instant>,
    revision: u64,
    undo: VecDeque<Vec<Stroke>>,
    redo: Vec<Vec<Stroke>>,
    recovery: bool,
}

impl PageState {
    fn new(pages: u32) -> Self {
        Self {
            pages,
            ink: None,
            token: None,
            load_error: None,
            save_error: None,
            pending: false,
            writing: false,
            dirty: None,
            revision: 0,
            undo: VecDeque::new(),
            redo: Vec::new(),
            recovery: false,
        }
    }

    fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > HISTORY_STEPS
            || self
                .undo
                .iter()
                .chain(self.redo.iter())
                .map(|v| stroke_bytes(v))
                .sum::<usize>()
                > HISTORY_BYTES
        {
            if self.undo.pop_front().is_none() {
                self.redo.remove(0);
            }
        }
    }
}

fn stroke_bytes(strokes: &[Stroke]) -> usize {
    std::mem::size_of_val(strokes)
        + strokes
            .iter()
            .map(|s| s.points.len() * std::mem::size_of::<super::model::Point>())
            .sum::<usize>()
}

enum Job {
    Verify {
        hash: String,
        path: PathBuf,
        generation: u64,
    },
    Load {
        key: PageKey,
        pages: u32,
    },
    Save {
        key: PageKey,
        ink: PageInk,
        pages: u32,
        expected: Option<String>,
        revision: u64,
    },
    Recover {
        key: PageKey,
        pages: u32,
        expected: Option<String>,
    },
}

enum Done {
    Verify {
        hash: String,
        generation: u64,
        result: Result<Stamp, String>,
    },
    Load {
        key: PageKey,
        result: Result<store::Loaded, String>,
    },
    Save {
        key: PageKey,
        revision: u64,
        result: Result<store::Saved, String>,
    },
    Recover {
        key: PageKey,
        result: Result<store::Recovered, String>,
    },
}

#[derive(Clone, Copy)]
enum PageAction {
    Clear,
    Reload,
    Recover,
}

struct Confirmation {
    key: PageKey,
    generation: u64,
    revision: u64,
    token: Option<String>,
    action: PageAction,
}

pub struct Annotations {
    binding: Option<Binding>,
    generation: u64,
    pages: BTreeMap<PageKey, PageState>,
    visible: BTreeSet<PageKey>,
    tx: Sender<Job>,
    rx: Receiver<Done>,
    busy: bool,
    worker_failed: bool,
    data: PathBuf,
    ctx: egui::Context,
    force_save: bool,
    editing: bool,
    text_input_active: bool,
    pub(super) show: bool,
    pub(super) tool: Tool,
    pub(super) color: [u8; 3],
    pub(super) thickness: f32,
    canvas: Canvas,
    selected_page: Option<u32>,
    confirmation: Option<Confirmation>,
    pub(super) notice: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tool {
    Pen,
    Highlighter,
    Eraser,
    Hand,
}

impl Annotations {
    pub fn new(data: PathBuf, ctx: egui::Context) -> Self {
        let (tx, rx) = Self::spawn_worker(data.clone(), ctx.clone());
        Self {
            binding: None,
            generation: 0,
            pages: BTreeMap::new(),
            visible: BTreeSet::new(),
            tx,
            rx,
            busy: false,
            worker_failed: false,
            data,
            ctx,
            force_save: false,
            editing: false,
            text_input_active: false,
            show: true,
            tool: Tool::Pen,
            color: [200, 25, 25],
            thickness: 2.5,
            canvas: Canvas::default(),
            selected_page: None,
            confirmation: None,
            notice: String::new(),
        }
    }

    fn spawn_worker(data: PathBuf, ctx: egui::Context) -> (Sender<Job>, Receiver<Done>) {
        let (tx, requests) = bounded::<Job>(1);
        let (results, rx) = bounded(1);
        std::thread::spawn(move || {
            while let Ok(job) = requests.recv() {
                let done = match job {
                    Job::Verify {
                        hash,
                        path,
                        generation,
                    } => {
                        let result = (|| -> Result<Stamp> {
                            let before = stamp(&path)?;
                            ensure!(
                                crate::folder::hash_file(&path, &AtomicBool::new(false))? == hash,
                                "Score contents changed. Rescan/reopen before annotating"
                            );
                            ensure!(stamp(&path)? == before, "Score changed during verification");
                            Ok(before)
                        })()
                        .map_err(|e| format!("{e:#}"));
                        Done::Verify {
                            hash,
                            generation,
                            result,
                        }
                    }
                    Job::Load { key, pages } => {
                        let result = store::load(&data, &key.hash, key.page, pages)
                            .map_err(|e| format!("{e:#}"));
                        Done::Load { key, result }
                    }
                    Job::Save {
                        key,
                        ink,
                        pages,
                        expected,
                        revision,
                    } => {
                        let result = store::save(&data, &ink, pages, expected.as_deref())
                            .map_err(|e| format!("{e:#}"));
                        Done::Save {
                            key,
                            revision,
                            result,
                        }
                    }
                    Job::Recover {
                        key,
                        pages,
                        expected,
                    } => {
                        let empty = PageInk::empty(&key.hash, key.page);
                        let result = store::recover(&data, &empty, pages, expected.as_deref())
                            .map_err(|e| format!("{e:#}"));
                        Done::Recover { key, result }
                    }
                };
                if results.send(done).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        (tx, rx)
    }

    pub fn matches_score(&self, hash: &str, pages: u32, source: &Path) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|b| b.hash == hash && b.pages == pages && b.source == source)
    }

    pub fn bind(&mut self, hash: &str, pages: u32, source: &Path) {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.hash == hash && b.pages == pages && b.source == source)
        {
            return;
        }
        self.finish_gesture();
        if self.drawing() {
            self.notice =
                "Finish or explicitly discard unfinished ink before switching scores.".into();
            return;
        }
        self.save_now();
        self.editing = false;
        self.visible.clear();
        self.selected_page = None;
        self.confirmation = None;
        self.generation = self.generation.wrapping_add(1);
        self.binding = Some(Binding {
            hash: hash.into(),
            pages,
            source: source.into(),
            stamp: None,
            pending: false,
            error: None,
            generation: self.generation,
        });
    }

    pub fn unbind(&mut self) {
        self.finish_gesture();
        if self.drawing() {
            self.notice =
                "Finish or explicitly discard unfinished ink before leaving this score.".into();
            return;
        }
        self.save_now();
        self.binding = None;
        self.visible.clear();
        self.editing = false;
        self.confirmation = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) fn gesture_identity(&self) -> Option<(String, u64)> {
        self.binding
            .as_ref()
            .map(|b| (b.hash.clone(), b.generation))
    }

    fn autosave_delay(&self) -> Option<Duration> {
        if self.busy || self.worker_failed {
            return None;
        }
        self.pages
            .values()
            .filter(|p| !p.pending && p.save_error.is_none() && p.load_error.is_none())
            .filter_map(|p| p.dirty.map(|at| SAVE_DEBOUNCE.saturating_sub(at.elapsed())))
            .min()
    }

    fn dispatch(&mut self, job: Job) {
        match self.tx.try_send(job) {
            Ok(()) => self.busy = true,
            Err(TrySendError::Full(job)) => {
                // Do not leave a request marked pending when it was never queued.
                self.release_unsent(&job);
                self.notice = "Annotation worker queue busy; pending ink will retry.".into();
                self.ctx.request_repaint_after(Duration::from_millis(30));
            }
            Err(TrySendError::Disconnected(job)) => {
                self.release_unsent(&job);
                self.worker_lost();
            }
        }
    }

    fn release_unsent(&mut self, job: &Job) {
        match job {
            Job::Verify {
                hash, generation, ..
            } => {
                if let Some(b) = self
                    .binding
                    .as_mut()
                    .filter(|b| b.hash == *hash && b.generation == *generation)
                {
                    b.pending = false;
                }
            }
            Job::Load { key, .. } | Job::Save { key, .. } | Job::Recover { key, .. } => {
                if let Some(p) = self.pages.get_mut(key) {
                    p.pending = false;
                    p.writing = false;
                }
            }
        }
    }

    fn worker_lost(&mut self) {
        self.busy = false;
        self.worker_failed = true;
        self.notice = WORKER_ERROR.into();
        if let Some(b) = &mut self.binding {
            if b.pending {
                b.pending = false;
                b.error = Some(WORKER_ERROR.into());
            }
        }
        for p in self.pages.values_mut() {
            if p.writing || p.dirty.is_some() || p.recovery {
                p.save_error = Some(WORKER_ERROR.into());
            } else if p.pending {
                p.load_error = Some(WORKER_ERROR.into());
            }
            p.pending = false;
            p.writing = false;
            // A queued recovery remains explicit unsaved work until retry/discard.
        }
    }

    fn key(&self, page: u32) -> Option<PageKey> {
        let binding = self.binding.as_ref()?;
        (page < binding.pages).then(|| PageKey {
            hash: binding.hash.clone(),
            page,
        })
    }

    pub fn begin_view(&mut self) {
        self.visible.clear();
    }

    pub fn request_page(&mut self, page: u32) {
        let Some(key) = self.key(page) else {
            return;
        };
        self.visible.insert(key.clone());
        if self.pages.contains_key(&key) {
            return;
        }
        if self.pages.len() >= CACHE_PAGES {
            let candidate = self
                .pages
                .iter()
                .find(|(key, state)| {
                    !self.visible.contains(*key)
                        && !state.pending
                        && !state.recovery
                        && state.dirty.is_none()
                })
                .map(|(key, _)| key.clone());
            if let Some(key) = candidate {
                self.pages.remove(&key);
            } else {
                self.notice = "Annotation cache full. Save pending pages or resolve save errors before opening more ink.".into();
                return;
            }
        }
        let pages = self.binding.as_ref().unwrap().pages;
        self.pages.insert(key, PageState::new(pages));
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| !i.focused) {
            self.finish_gesture();
        }
        loop {
            let done = match self.rx.try_recv() {
                Ok(done) => done,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker_lost();
                    break;
                }
            };
            self.busy = false;
            match done {
                Done::Verify {
                    hash,
                    generation,
                    result,
                } => {
                    if let Some(binding) = self
                        .binding
                        .as_mut()
                        .filter(|b| b.hash == hash && b.generation == generation)
                    {
                        binding.pending = false;
                        match result {
                            Ok(stamp) => binding.stamp = Some(stamp),
                            Err(error) => binding.error = Some(error),
                        }
                    }
                }
                Done::Load { key, result } => {
                    if let Some(page) = self.pages.get_mut(&key) {
                        page.pending = false;
                        match result {
                            Ok(loaded) => {
                                page.ink = Some(loaded.ink);
                                page.token = loaded.token;
                                page.load_error = loaded.error;
                            }
                            Err(error) => page.load_error = Some(error),
                        }
                    }
                }
                Done::Save {
                    key,
                    revision,
                    result,
                } => {
                    if let Some(page) = self.pages.get_mut(&key) {
                        page.pending = false;
                        page.writing = false;
                        match result {
                            Ok(saved) => {
                                page.token = Some(saved.token);
                                page.save_error = None;
                                if page.revision == revision {
                                    page.dirty = None;
                                }
                                if let Some(warning) = saved.durability_warning {
                                    self.notice = warning;
                                }
                            }
                            Err(error) => page.save_error = Some(error),
                        }
                    }
                }
                Done::Recover { key, result } => {
                    if let Some(page) = self.pages.get_mut(&key) {
                        page.pending = false;
                        page.writing = false;
                        page.recovery = false;
                        match result {
                            Ok(recovered) => {
                                page.ink = Some(PageInk::empty(&key.hash, key.page));
                                page.token = Some(recovered.saved.token);
                                page.load_error = None;
                                page.save_error = None;
                                page.dirty = None;
                                page.undo.clear();
                                page.redo.clear();
                                self.notice = format!(
                                    "Rejected ink backed up at {}.{}",
                                    recovered.backup.display(),
                                    recovered
                                        .saved
                                        .durability_warning
                                        .map(|s| format!(" {s}"))
                                        .unwrap_or_default()
                                );
                            }
                            Err(error) => page.load_error = Some(error),
                        }
                    }
                }
            }
        }
        let invalid = self.binding.as_ref().is_some_and(|binding| {
            binding
                .stamp
                .as_ref()
                .is_some_and(|saved| stamp(&binding.source).ok().as_ref() != Some(saved))
        });
        if invalid {
            self.finish_gesture();
            self.editing = false;
            if let Some(binding) = &mut self.binding {
                binding.stamp = None;
                binding.error = Some("Backing score changed or disappeared. Rescan/reopen before displaying or editing ink.".into());
            }
        }
        if !self.busy && !self.worker_failed {
            if let Some(job) = self.next_job() {
                self.dispatch(job);
            }
        }
        if !self.has_unsaved() {
            self.force_save = false;
        }
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if let Some(delay) = self.autosave_delay() {
            ctx.request_repaint_after(delay);
        }
    }

    fn next_job(&mut self) -> Option<Job> {
        for (key, page) in &mut self.pages {
            if page.pending {
                continue;
            }
            if page.recovery {
                page.pending = true;
                page.writing = true;
                return Some(Job::Recover {
                    key: key.clone(),
                    pages: page.pages,
                    expected: page.token.clone(),
                });
            }
            if page.load_error.is_none()
                && page.save_error.is_none()
                && page
                    .dirty
                    .is_some_and(|at| self.force_save || at.elapsed() >= SAVE_DEBOUNCE)
            {
                let ink = page.ink.as_ref()?.clone();
                page.pending = true;
                page.writing = true;
                return Some(Job::Save {
                    key: key.clone(),
                    ink,
                    pages: page.pages,
                    expected: page.token.clone(),
                    revision: page.revision,
                });
            }
        }
        if let Some(binding) = &mut self.binding {
            if binding.stamp.is_none() && binding.error.is_none() && !binding.pending {
                binding.pending = true;
                return Some(Job::Verify {
                    hash: binding.hash.clone(),
                    path: binding.source.clone(),
                    generation: binding.generation,
                });
            }
        }
        for key in &self.visible {
            if let Some(page) = self.pages.get_mut(key) {
                if page.ink.is_none() && page.load_error.is_none() && !page.pending {
                    page.pending = true;
                    return Some(Job::Load {
                        key: key.clone(),
                        pages: page.pages,
                    });
                }
            }
        }
        None
    }

    pub fn editing(&self) -> bool {
        self.editing
    }
    pub fn text_input_active(&self) -> bool {
        self.text_input_active
    }
    pub fn compact_controls(&mut self, ui: &mut egui::Ui) -> bool {
        self.text_input_active = false;
        if ui.button("Annotate").clicked() {
            self.editing = true;
            self.show = true;
            return true;
        }
        ui.checkbox(&mut self.show, "Show ink");
        false
    }
    pub fn drawing(&self) -> bool {
        self.canvas.active()
    }
    pub fn captures_page_drag(&self) -> bool {
        self.editing && self.show && self.tool != Tool::Hand
    }
    pub fn writes_pending(&self) -> bool {
        self.pages.values().any(|p| p.pending && p.writing)
    }
    pub fn read_mode(&mut self) {
        self.finish_gesture();
        self.editing = false;
        self.text_input_active = false;
        self.save_now();
    }

    pub fn has_unsaved(&self) -> bool {
        self.drawing() || self.pages.values().any(|p| p.dirty.is_some() || p.recovery)
    }

    pub fn save_now(&mut self) {
        if self.worker_failed {
            let (tx, rx) = Self::spawn_worker(self.data.clone(), self.ctx.clone());
            self.tx = tx;
            self.rx = rx;
            self.worker_failed = false;
            if let Some(b) = &mut self.binding {
                if b.error.as_deref() == Some(WORKER_ERROR) {
                    b.error = None;
                }
            }
        }
        self.force_save = true;
        for page in self.pages.values_mut() {
            page.save_error = None;
            if page.ink.is_none() {
                page.load_error = None;
            }
        }
    }

    pub fn discard_unsaved(&mut self) {
        if self.writes_pending() {
            self.notice = "Wait for pending annotation writes before discarding.".into();
            return;
        }
        self.canvas.discard();
        self.pages.retain(|_, p| p.dirty.is_none() && !p.recovery);
        self.confirmation = None;
        self.force_save = false;
        self.editing = false;
    }

    pub fn has_save_error(&self) -> bool {
        self.pages
            .values()
            .any(|p| (p.dirty.is_some() || p.recovery) && p.save_error.is_some())
    }

    fn reverify_source(&mut self) {
        if self.drawing() {
            self.notice =
                "Finish or explicitly discard unfinished ink before reverifying its source.".into();
            return;
        }
        let Some(binding) = self
            .binding
            .as_mut()
            .filter(|b| b.error.is_some() && !b.pending)
        else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        binding.generation = self.generation;
        binding.stamp = None;
        binding.error = None;
        self.confirmation = None;
        self.notice =
            "Rechecking the original score's exact content hash before enabling ink.".into();
        self.ctx.request_repaint();
    }

    fn verified(&self) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|b| b.stamp.is_some() && b.error.is_none())
    }

    pub(crate) fn writable(&self, page: u32) -> bool {
        self.verified()
            && self
                .key(page)
                .and_then(|k| self.pages.get(&k))
                .is_some_and(|p| {
                    p.ink.is_some()
                        && p.load_error.is_none()
                        && p.save_error.is_none()
                        && !p.recovery
                })
    }

    pub(super) fn ink(&self, page: u32) -> Option<&PageInk> {
        self.verified().then_some(())?;
        self.pages.get(&self.key(page)?)?.ink.as_ref()
    }

    pub(super) fn replace_strokes(&mut self, page: u32, strokes: Vec<Stroke>) -> Result<()> {
        ensure!(
            self.writable(page),
            "Page ink is loading, read-only or has an unresolved save error"
        );
        let key = self.key(page).context("Page identity changed")?;
        let state = self.pages.get_mut(&key).unwrap();
        let ink = state.ink.as_ref().unwrap();
        if ink.strokes == strokes {
            return Ok(());
        }
        let mut candidate = ink.clone();
        candidate.strokes = strokes;
        candidate.validate(state.pages)?;
        ensure!(
            serde_json::to_vec(&candidate)?.len() <= super::model::MAX_BYTES,
            "Page ink exceeds its saved-size limit"
        );
        let before = std::mem::replace(&mut state.ink, Some(candidate))
            .unwrap()
            .strokes;
        state.undo.push_back(before);
        state.redo.clear();
        state.trim_history();
        state.revision = state.revision.wrapping_add(1);
        state.dirty = Some(Instant::now());
        self.ctx.request_repaint_after(SAVE_DEBOUNCE);
        self.selected_page = Some(page);
        Ok(())
    }

    fn history(&mut self, redo: bool) {
        self.finish_gesture();
        let Some(page) = self.selected_page else {
            return;
        };
        if !self.writable(page) {
            return;
        }
        let state = self.pages.get_mut(&self.key(page).unwrap()).unwrap();
        let target = if redo {
            state.redo.pop()
        } else {
            state.undo.pop_back()
        };
        if let Some(strokes) = target {
            let previous = std::mem::replace(&mut state.ink.as_mut().unwrap().strokes, strokes);
            if redo {
                state.undo.push_back(previous);
            } else {
                state.redo.push(previous);
            }
            state.trim_history();
            state.revision = state.revision.wrapping_add(1);
            state.dirty = Some(Instant::now());
            self.ctx.request_repaint_after(SAVE_DEBOUNCE);
        }
    }

    pub fn finish_gesture(&mut self) {
        let mut canvas = std::mem::take(&mut self.canvas);
        canvas.finish(self);
        self.canvas = canvas;
    }

    pub fn canvas(&mut self, ctx: &egui::Context, surfaces: &[Surface], blocked: bool) {
        let mut canvas = std::mem::take(&mut self.canvas);
        canvas.frame(self, ctx, surfaces, blocked);
        self.canvas = canvas;
    }

    pub fn shortcuts(&mut self, ctx: &egui::Context, blocked: bool) -> bool {
        if !self.editing || blocked {
            return false;
        }
        let command = egui::Modifiers::COMMAND;
        if ctx.input_mut(|i| i.consume_key(command | egui::Modifiers::SHIFT, Key::Z)) {
            self.history(true);
            return true;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::Z)) {
            self.history(false);
            return true;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::Y)) {
            self.history(true);
            return true;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::S)) {
            self.finish_gesture();
            self.save_now();
            return true;
        }
        false
    }

    fn request_action(&mut self, action: PageAction) {
        if matches!(action, PageAction::Clear) {
            self.finish_gesture();
            if self.drawing() {
                return;
            }
        }
        let Some(key) = self.selected_page.and_then(|page| self.key(page)) else {
            return;
        };
        let Some(state) = self.pages.get(&key) else {
            return;
        };
        self.confirmation = Some(Confirmation {
            key,
            generation: self.generation,
            revision: state.revision,
            token: state.token.clone(),
            action,
        });
    }

    fn confirm_action(&mut self) {
        let Some(action) = self.confirmation.take() else {
            return;
        };
        let current = self.selected_page.and_then(|page| self.key(page));
        let valid = current.as_ref() == Some(&action.key)
            && self.generation == action.generation
            && self.pages.get(&action.key).is_some_and(|p| {
                p.revision == action.revision && p.token == action.token && !p.pending
            });
        if !valid {
            self.notice =
                "Page, revision or saved state changed; request the page action again.".into();
            return;
        }
        match action.action {
            PageAction::Clear => {
                if self.drawing() {
                    self.notice =
                        "Finish or discard the unfinished gesture before clearing.".into();
                } else if let Err(error) = self.replace_strokes(action.key.page, vec![]) {
                    self.notice = error.to_string();
                }
            }
            PageAction::Reload => {
                self.canvas.discard();
                self.pages.remove(&action.key);
                self.request_page(action.key.page);
            }
            PageAction::Recover => {
                if !self.verified() {
                    self.notice = "Verify the backing score before recovery.".into();
                    return;
                }
                let state = self.pages.get_mut(&action.key).unwrap();
                if state.load_error.is_some() && state.token.is_some() {
                    state.recovery = true;
                    state.save_error = None;
                } else {
                    self.notice = "Reset requires a bounded, readable rejected sidecar. Oversized files must be manually preserved and moved before reloading.".into();
                }
            }
        }
    }

    /// Returns true on entering annotation mode.
    pub fn toolbar(&mut self, ui: &mut egui::Ui, current_page: u32) -> bool {
        self.text_input_active = false;
        if self
            .selected_page
            .is_none_or(|p| !self.visible.iter().any(|k| k.page == p))
        {
            self.selected_page = Some(current_page);
        }
        let mut entered = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(if self.editing { "Done" } else { "Annotate" })
                .clicked()
            {
                if self.editing {
                    self.read_mode();
                } else {
                    self.editing = true;
                    self.show = true;
                    entered = true;
                }
            }
            ui.checkbox(&mut self.show, "Show ink");
            let status = if self.has_save_error() {
                "Unsaved ink — save failed"
            } else if self.has_unsaved() {
                "Saving ink…"
            } else if self.busy {
                "Loading/verifying ink…"
            } else {
                "Ink saved"
            };
            ui.add_sized(
                [150.0, ui.spacing().interact_size.y],
                egui::Label::new(egui::RichText::new(status).small()).truncate(),
            )
            .on_hover_text(status);
            if ui.button("Save now").clicked() {
                self.finish_gesture();
                self.save_now();
            }
            if self.drawing() && ui.button("Discard unfinished gesture").clicked() {
                self.canvas.discard();
                self.notice = "Unfinished gesture discarded; completed page ink retained.".into();
            }
        });
        if self.editing {
            ui.horizontal_wrapped(|ui| {
                for (tool, label) in [
                    (Tool::Pen, "Pen"),
                    (Tool::Highlighter, "Highlighter"),
                    (Tool::Eraser, "Stroke eraser"),
                    (Tool::Hand, "Hand / Pan"),
                ] {
                    if ui.selectable_label(self.tool == tool, label).clicked() {
                        self.finish_gesture();
                        self.tool = tool;
                    }
                }
                for (color, name) in [
                    ([200, 25, 25], "Red"),
                    ([20, 60, 200], "Blue"),
                    ([15, 15, 15], "Black"),
                    ([255, 215, 0], "Yellow"),
                ] {
                    let foreground = if name == "Yellow" {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    };
                    let label = egui::RichText::new(name).color(foreground);
                    if ui
                        .add(
                            egui::Button::new(label)
                                .fill(Color32::from_rgb(color[0], color[1], color[2])),
                        )
                        .clicked()
                    {
                        self.color = color;
                    }
                }
                let width =
                    ui.add(egui::Slider::new(&mut self.thickness, 0.5..=12.0).text("Width"));
                self.text_input_active |= width.ctx.memory(|memory| memory.has_focus(width.id));
                if ui.button("Undo").clicked() {
                    self.history(false);
                }
                if ui.button("Redo").clicked() {
                    self.history(true);
                }
                if ui.button("Clear page…").clicked() {
                    self.request_action(PageAction::Clear);
                }
            });
            if let Some(page) = self.selected_page {
                ui.small(format!(
                    "Ink page {} · following off · constant width · no guaranteed palm rejection",
                    page + 1
                ));
            }
            if !self.show {
                ui.colored_label(Color32::YELLOW, "Show ink before drawing or erasing.");
            }
        }
        if let Some(error) = self.binding.as_ref().and_then(|b| b.error.as_ref()) {
            ui.colored_label(Color32::LIGHT_RED, error);
            let allowed = !self.drawing() && self.binding.as_ref().is_some_and(|b| !b.pending);
            if ui
                .add_enabled(allowed, egui::Button::new("Reverify original score"))
                .clicked()
            {
                self.reverify_source();
            }
        }
        // Save failures must remain visible even after navigating to another page.
        let failures: Vec<_> = self
            .pages
            .iter()
            .filter_map(|(key, p)| {
                p.save_error.as_ref().map(|e| {
                    format!(
                        "Unsaved ink · score {} · page {}: {e}",
                        key.hash.get(..8).unwrap_or("invalid"),
                        key.page + 1
                    )
                })
            })
            .collect();
        for error in &failures {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if (!failures.is_empty() || self.worker_failed)
            && ui.button("Retry all pending saves").clicked()
        {
            self.save_now();
        }
        let error = self
            .selected_page
            .and_then(|p| self.key(p))
            .and_then(|k| self.pages.get(&k))
            .and_then(|p| p.load_error.as_ref().or(p.save_error.as_ref()))
            .cloned();
        if let Some(error) = error {
            ui.colored_label(Color32::LIGHT_RED, error);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Retry save / load").clicked() {
                    self.save_now();
                }
                if ui.button("Reload saved page…").clicked() {
                    self.request_action(PageAction::Reload);
                }
                if ui.button("Back up rejected ink / reset…").clicked() {
                    self.request_action(PageAction::Recover);
                }
            });
        }
        if let Some(action) = &self.confirmation {
            let text = format!("Confirm page {} (score {}): clear is undoable; reload discards unsaved page edits; reset backs up rejected ink.",
                action.key.page + 1, action.key.hash.get(..8).unwrap_or("invalid"));
            ui.group(|ui| {
                ui.label(text);
                if ui.button("Confirm page action").clicked() {
                    self.confirm_action();
                }
                if ui.button("Cancel page action").clicked() {
                    self.confirmation = None;
                }
            });
        }
        if !self.notice.is_empty() {
            ui.colored_label(Color32::YELLOW, &self.notice);
        }
        entered
    }

    pub(super) fn select_page(&mut self, page: u32) {
        self.selected_page = Some(page);
    }

    pub(super) fn stroke_style(&self) -> (InkTool, [u8; 4], f32) {
        let highlighter = self.tool == Tool::Highlighter;
        let color = self.color;
        (
            if highlighter {
                InkTool::Highlighter
            } else {
                InkTool::Pen
            },
            [
                color[0],
                color[1],
                color[2],
                if highlighter { 70 } else { 255 },
            ],
            (self.thickness * if highlighter { 5.0 } else { 1.0 }) / 1000.0,
        )
    }
}
