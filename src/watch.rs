use crate::{
    document,
    import::{self, Outcome, Report},
    model::Paths,
    storage::Library,
};
use anyhow::{bail, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};
use walkdir::WalkDir;

pub const INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    size: u64,
    modified: SystemTime,
}

struct Observed {
    fingerprint: Fingerprint,
    stable_since: Instant,
    handled: bool,
    retry_after: Instant,
}

#[derive(Default)]
pub struct Scanner {
    files: HashMap<PathBuf, Observed>,
}

impl Scanner {
    pub fn scan(
        &mut self,
        paths: &Paths,
        cancel: &AtomicBool,
        now: Instant,
        settle: Duration,
    ) -> Result<Report> {
        let root = paths.score_dir();
        if !root.is_dir() {
            bail!("Music folder is unavailable: {}", root.display());
        }
        let db = Library::open(paths)?;
        let mut report = Report::default();
        let mut present = HashSet::new();
        for entry in WalkDir::new(&root).follow_links(false) {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                return Ok(report);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.failed += 1;
                    if report.errors.len() < 100 {
                        report.errors.push(error.to_string());
                    }
                    continue;
                }
            };
            if !entry.file_type().is_file() || !document::supported(entry.path()) {
                continue;
            }
            let path = entry.into_path();
            present.insert(path.clone());
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.failed += 1;
                    if report.errors.len() < 100 {
                        report.errors.push(format!("{}: {error}", path.display()));
                    }
                    continue;
                }
            };
            let fingerprint = Fingerprint {
                size: metadata.len(),
                modified: metadata.modified()?,
            };
            let observed = self.files.entry(path.clone()).or_insert_with(|| Observed {
                fingerprint: fingerprint.clone(),
                stable_since: now,
                handled: false,
                retry_after: now,
            });
            if observed.fingerprint != fingerprint {
                observed.fingerprint = fingerprint;
                observed.stable_since = now;
                observed.handled = false;
                observed.retry_after = now;
            }
            if observed.handled
                || now.duration_since(observed.stable_since) < settle
                || now < observed.retry_after
            {
                continue;
            }
            match import::import_file(&path, paths, &db, cancel) {
                Ok(Outcome::Imported) => {
                    report.imported += 1;
                    observed.handled = true;
                }
                Ok(Outcome::Duplicate) => {
                    report.skipped += 1;
                    observed.handled = true;
                }
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    report.cancelled = true;
                    return Ok(report);
                }
                Err(error) => {
                    report.failed += 1;
                    if report.errors.len() < 100 {
                        report.errors.push(format!("{}: {error:#}", path.display()));
                    }
                    observed.retry_after = now + Duration::from_secs(30);
                }
            }
        }
        self.files.retain(|path, _| present.contains(path));
        Ok(report)
    }
}

pub struct FolderWatcher {
    pub updates: Receiver<Result<Report, String>>,
    stop: Sender<()>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl FolderWatcher {
    pub fn start(paths: Paths, repaint: impl Fn() + Send + 'static) -> Self {
        let (stop, stop_rx) = bounded(1);
        let (tx, updates) = bounded(1);
        let overflow = updates.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let mut scanner = Scanner::default();
            loop {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let result = scanner
                    .scan(&paths, &worker_cancel, Instant::now(), INTERVAL)
                    .map_err(|error| format!("{error:#}"));
                if let Err(crossbeam_channel::TrySendError::Full(value)) = tx.try_send(result) {
                    let _ = overflow.try_recv();
                    let _ = tx.try_send(value);
                }
                repaint();
                if stop_rx.recv_timeout(INTERVAL).is_ok() || worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
            }
        });
        Self {
            updates,
            stop,
            cancel,
            worker: Some(worker),
        }
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
