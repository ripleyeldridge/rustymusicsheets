//! Explicit reconciliation only. The automatic watcher never prunes records.
use crate::{
    document,
    import::{self, Report},
    model::Paths,
    storage::Library,
};
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use walkdir::WalkDir;

struct Root {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Root {
    fn capture(paths: &Paths) -> Result<Self> {
        let path = paths
            .score_dir()
            .canonicalize()
            .context("Music folder is unavailable; no scores were removed")?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_dir() {
            bail!("Music folder is not a directory; no scores were removed");
        }
        fs::read_dir(&path).context("Music folder is not readable; no scores were removed")?;
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn validate(&self, paths: &Paths) -> Result<()> {
        let current = Self::capture(paths)?;
        if current.path != self.path || current.device != self.device || current.inode != self.inode
        {
            bail!("Music folder changed during rescan; cleanup cancelled");
        }
        let configured = Library::open(paths)?.music_folder()?;
        if configured != paths.music {
            bail!("Configured music folder changed during rescan; cleanup cancelled");
        }
        Ok(())
    }

    fn readable_tree(&self, cancel: &AtomicBool) -> Result<()> {
        for entry in WalkDir::new(&self.path).follow_links(false) {
            if cancel.load(Ordering::Relaxed) {
                bail!("Rescan cancelled");
            }
            let entry = entry.context("Folder traversal failed; cleanup cancelled")?;
            if entry.file_type().is_file() && document::supported(entry.path()) {
                // Do not treat an inaccessible file or subtree as deleted.
                File::open(entry.path()).with_context(|| {
                    format!("Cannot read {}; cleanup cancelled", entry.path().display())
                })?;
            }
        }
        Ok(())
    }
}

pub fn rescan(
    paths: &Paths,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Report),
) -> Result<Report> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(Report {
            cancelled: true,
            ..Default::default()
        });
    }
    let root = Root::capture(paths)?;
    root.validate(paths)?;
    root.readable_tree(cancel)?;
    // Import/reconnect first: a renamed identical PDF keeps metadata and memberships.
    let mut report = import::batch(&[root.path.clone()], true, paths, cancel, &mut progress)?;
    if report.cancelled || cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    if report.filesystem_errors {
        report
            .errors
            .push("Missing-score cleanup skipped because the scan had filesystem errors.".into());
        return Ok(report);
    }
    root.validate(paths)?;
    root.readable_tree(cancel)?;
    let mut db = Library::open(paths)?;
    let mut missing = Vec::new();
    for score in db.scores()? {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        let path = db.pdf_path(paths, &score.hash)?;
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => missing.push(score),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing.push(score),
            Err(error) => {
                return Err(error).context("Cannot verify missing PDF; cleanup cancelled")
            }
        }
    }
    report.current = "Reconciling missing PDFs and backing up metadata…".into();
    progress(&report);
    let result = db.prune_missing(paths, &missing, cancel, || root.validate(paths));
    if result.is_err() && cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    let (removed, backup) = result?;
    report.removed = removed;
    report.backup = backup;
    report.current.clear();
    progress(&report);
    Ok(report)
}
