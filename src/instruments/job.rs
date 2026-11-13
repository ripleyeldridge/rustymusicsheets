use super::{extract, Detection, DetectionOptions};
use crate::{
    folder,
    model::{Paths, Score},
    storage::Library,
};
use crossbeam_channel::{bounded, Receiver};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub processed: usize,
    pub total: usize,
    pub applied: usize,
    pub failed: usize,
    pub current: String,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

pub struct Update {
    pub summary: Summary,
    pub done: bool,
}

pub struct DetectionJob {
    pub updates: Receiver<Update>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DetectionJob {
    pub fn start(
        paths: Paths,
        scores: Vec<Score>,
        options: DetectionOptions,
        auto_fill: bool,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, updates) = bounded(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = cancel.clone();
        let worker = std::thread::spawn(move || {
            let mut summary = Summary {
                total: scores.len(),
                ..Default::default()
            };
            let db = Library::open(&paths);
            match db {
                Err(error) => {
                    summary.failed = summary.total;
                    summary.errors.push(format!("{error:#}"));
                }
                Ok(db) => {
                    for score in scores {
                        if stopping.load(Ordering::Relaxed) {
                            summary.cancelled = true;
                            break;
                        }
                        let attempt = (|| -> anyhow::Result<bool> {
                            let path = db.pdf_path(&paths, &score.hash)?;
                            if folder::hash_file(&path, &stopping)? != score.hash {
                                anyhow::bail!("File changed on disk; rescan it before detection");
                            }
                            let result = extract::detect(
                                &path,
                                score.pages,
                                &options,
                                &stopping,
                                |stage| {
                                    summary.current = format!("{} — {stage}", score.title);
                                    let _ = tx.try_send(Update {
                                        summary: summary.clone(),
                                        done: false,
                                    });
                                    repaint();
                                },
                            )?;
                            if stopping.load(Ordering::Relaxed) {
                                anyhow::bail!("Detection cancelled");
                            }
                            if folder::hash_file(&path, &stopping)? != score.hash {
                                anyhow::bail!("File changed during detection; rescan and retry");
                            }
                            let applied =
                                db.store_detection(score.id, &score.hash, &result, auto_fill)?;
                            if let Some(error) = result.error {
                                anyhow::bail!("{error}");
                            }
                            Ok(applied)
                        })();
                        match attempt {
                            Ok(applied) => summary.applied += usize::from(applied),
                            Err(_) if stopping.load(Ordering::Relaxed) => {
                                summary.cancelled = true;
                                break;
                            }
                            Err(error) => {
                                summary.failed += 1;
                                if summary.errors.len() < 100 {
                                    summary.errors.push(format!("{}: {error:#}", score.title));
                                }
                                // Do not replace a partial extraction that was already stored.
                                if db.detection(score.id, &score.hash).ok().flatten().is_none() {
                                    let result = Detection {
                                        error: Some(format!("{error:#}")),
                                        ..Default::default()
                                    };
                                    let _ =
                                        db.store_detection(score.id, &score.hash, &result, false);
                                }
                            }
                        }
                        summary.processed += 1;
                        let _ = tx.try_send(Update {
                            summary: summary.clone(),
                            done: false,
                        });
                        repaint();
                    }
                }
            }
            summary.current.clear();
            let _ = tx.send(Update {
                summary,
                done: true,
            });
            repaint();
        });
        Self {
            updates,
            cancel,
            worker: Some(worker),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for DetectionJob {
    fn drop(&mut self) {
        self.cancel();
        let (_, empty) = bounded(0);
        drop(std::mem::replace(&mut self.updates, empty));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
