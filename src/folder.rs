use crate::{model::Paths, storage::Library};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use walkdir::WalkDir;

pub fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("Operation cancelled");
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Publish without overwriting any existing file, including a symlink.
pub fn publish(
    staged: tempfile::NamedTempFile,
    root: &Path,
    preferred: &Path,
    hash: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    if preferred
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("Invalid destination file name");
    }
    let preferred = root.join(preferred);
    let parent = preferred.parent().context("Missing parent folder")?;
    // Reject symlinked subdirectories so no write can escape the selected folder.
    let mut current = root.to_owned();
    for part in parent.strip_prefix(root)?.components() {
        current.push(part);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("Destination contains a symlink: {}", current.display());
        }
    }
    std::fs::create_dir_all(parent)?;
    staged.as_file().sync_all()?;
    let mut staged = staged;
    for attempt in 0..1000 {
        if cancel.load(Ordering::Relaxed) {
            bail!("Operation cancelled");
        }
        let target = match attempt {
            0 => preferred.clone(),
            1 => root.join(format!("{hash}.pdf")),
            n => root.join(format!("{hash}-{n}.pdf")),
        };
        if std::fs::symlink_metadata(&target).is_ok() {
            if std::fs::symlink_metadata(&target)?.is_file() && hash_file(&target, cancel)? == hash
            {
                return Ok(target.strip_prefix(root)?.to_owned());
            }
            continue;
        }
        match staged.persist_noclobber(&target) {
            Ok(_) => return Ok(target.strip_prefix(root)?.to_owned()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                staged = error.file;
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    bail!("Cannot find an available destination filename")
}

/// All file copies finish before one transaction switches the folder and paths.
pub fn select(
    paths: &Paths,
    destination: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(String),
) -> Result<Paths> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Folder change cancelled");
    }
    std::fs::create_dir_all(destination).context("Cannot create music library folder")?;
    let destination = destination.canonicalize()?;
    let probe = tempfile::NamedTempFile::new_in(&destination)
        .context("Music library folder is not writable")?;
    drop(probe);
    let mut db = Library::open(paths)?;
    let scores = db.scores()?;
    let mut existing = HashMap::new();
    // Reuse matching PDFs already in the new folder instead of copying them again.
    if !scores.is_empty() {
        for entry in WalkDir::new(&destination).follow_links(false) {
            if cancel.load(Ordering::Relaxed) {
                bail!("Folder change cancelled");
            }
            let entry = entry?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("pdf"))
            {
                progress(format!("Checking {}", entry.path().display()));
                if let Ok(hash) = hash_file(entry.path(), cancel) {
                    existing
                        .entry(hash)
                        .or_insert(entry.path().strip_prefix(&destination)?.to_owned());
                }
            }
        }
    }
    let mut files = Vec::new();
    for score in scores {
        if cancel.load(Ordering::Relaxed) {
            bail!("Folder change cancelled");
        }
        if let Some(relative) = existing.get(&score.hash) {
            files.push((score.id, relative.clone()));
            continue;
        }
        let source = db.pdf_path(paths, &score.hash)?;
        progress(format!("Copying {}", source.display()));
        let mut staged = tempfile::NamedTempFile::new_in(&destination)?;
        let mut input = File::open(&source)
            .with_context(|| format!("Cannot copy existing score {}", source.display()))?;
        let mut buffer = [0; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Folder change cancelled");
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            staged.write_all(&buffer[..n])?;
        }
        if hash_file(staged.path(), cancel)? != score.hash {
            bail!("Existing score changed on disk: {}", source.display());
        }
        let preferred = if score.file_path.as_os_str().is_empty() {
            PathBuf::from(format!("{}.pdf", score.hash))
        } else {
            score.file_path
        };
        let relative = publish(staged, &destination, &preferred, &score.hash, cancel)?;
        files.push((score.id, relative));
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Folder change cancelled");
    }
    db.set_music_folder(&destination, &files)?;
    let mut updated = paths.clone();
    updated.music = Some(destination);
    Ok(updated)
}
