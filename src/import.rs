use crate::{document, folder, model::Paths, storage::Library};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use walkdir::WalkDir;

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub current: String,
    pub cancelled: bool,
    pub removed: usize,
    pub backup: Option<PathBuf>,
    pub filesystem_errors: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Imported,
    Duplicate,
}

pub fn import_document(
    source: &crate::source::DocumentRef,
    access: &dyn crate::source::DocumentAccess,
    paths: &Paths,
    db: &Library,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    if let Some(path) = source.as_path() {
        return import_file(path, paths, db, cancel);
    }
    let staged = crate::source::stage(source, access, &paths.cache, cancel)?;
    import_file(&staged.path, paths, db, cancel)
}

pub fn import_file(
    path: &Path,
    paths: &Paths,
    db: &Library,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let kind =
        document::Kind::of(path).context("Supported score formats are PDF, PNG, JPG, and JPEG")?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Import cancelled");
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        bail!("Not a regular file (symbolic links are not imported)");
    }
    let mut input = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let root = paths
        .score_dir()
        .canonicalize()
        .context("Music library folder is unavailable")?;
    let source = path.canonicalize()?;
    let inside = source.strip_prefix(&root).ok().map(Path::to_owned);
    let mut staged = tempfile::NamedTempFile::new_in(&root)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("Import cancelled");
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        staged.write_all(&buffer[..count])?;
    }
    let hash = format!("{:x}", hasher.finalize());
    let after = std::fs::metadata(path)?;
    if metadata.len() != after.len() || metadata.modified()? != after.modified()? {
        bail!("File is still changing; it will be retried by the folder watcher");
    }
    if db.has_hash(&hash)? {
        // A repeated import also repairs a missing managed copy.
        if !db.pdf_path(paths, &hash)?.exists() {
            let relative = if let Some(relative) = inside {
                relative
            } else {
                folder::publish(
                    staged,
                    &root,
                    Path::new(&format!("{hash}.{}", kind.extension())),
                    &hash,
                    cancel,
                )?
            };
            db.relocate(&hash, &relative)?;
        }
        return Ok(Outcome::Duplicate);
    }
    let info = document::info(staged.path(), kind, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Import cancelled");
    }
    staged.as_file().sync_all()?;
    let relative = if let Some(relative) = inside {
        relative
    } else {
        let preferred = if paths.music.is_some() {
            PathBuf::from(path.file_name().context("Missing PDF filename")?)
        } else {
            PathBuf::from(format!("{hash}.{}", kind.extension()))
        };
        folder::publish(staged, &root, &preferred, &hash, cancel)?
    };
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    // A complete immutable file is published before its database record.
    // If insertion fails it remains an unreferenced, recoverable file.
    db.insert_file(&hash, &title, info.pages, &info.author, &relative)?;
    Ok(Outcome::Imported)
}

pub fn batch(
    sources: &[PathBuf],
    recursive: bool,
    paths: &Paths,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Report),
) -> Result<Report> {
    let db = Library::open(paths)?;
    let mut report = Report::default();
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        if std::fs::symlink_metadata(source)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            record_error(
                &mut report,
                format!("{}: symlink skipped", source.display()),
            );
            continue;
        }
        let depth = if recursive { usize::MAX } else { 1 };
        let walker = WalkDir::new(source).follow_links(false).max_depth(depth);
        for entry in walker {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                progress(&report);
                return Ok(report);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.filesystem_errors = true;
                    record_error(&mut report, error.to_string());
                    progress(&report);
                    continue;
                }
            };
            if !entry.file_type().is_file() || !document::supported(entry.path()) {
                continue;
            }
            report.current = entry.path().display().to_string();
            progress(&report);
            match import_file(entry.path(), paths, &db, cancel) {
                Ok(Outcome::Imported) => report.imported += 1,
                Ok(Outcome::Duplicate) => report.skipped += 1,
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    report.cancelled = true;
                    progress(&report);
                    return Ok(report);
                }
                Err(error) => {
                    report.filesystem_errors |= error
                        .chain()
                        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some());
                    let message = format!("{}: {error:#}", entry.path().display());
                    record_error(&mut report, message);
                }
            }
            progress(&report);
        }
    }
    report.current.clear();
    report.cancelled = cancel.load(Ordering::Relaxed);
    Ok(report)
}

fn record_error(report: &mut Report, message: String) {
    report.failed += 1;
    // Keep memory bounded even for very large or damaged source trees.
    if report.errors.len() < 100 {
        report.errors.push(message);
    }
}
