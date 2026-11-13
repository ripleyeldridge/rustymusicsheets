use anyhow::Result;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as platform;
#[cfg(any(test, not(any(target_os = "linux", windows, target_os = "android"))))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows, target_os = "android")))]
use unsupported as platform;

pub const SCORE_EXTENSIONS: &[&str] = &["pdf", "png", "jpg", "jpeg"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Score,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: SelectionKind,
    pub directory: PathBuf,
}

/// Implementations return None for user cancellation, and Err for reported failures.
pub trait FilePicker {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>>;
}

pub struct SystemPicker;

impl FilePicker for SystemPicker {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>> {
        platform::Backend.select(request)
    }
}

pub fn select(kind: SelectionKind, preferred: &Path, home: &Path) -> Result<Option<PathBuf>> {
    select_with(&SystemPicker, kind, preferred, home)
}

fn select_with(
    backend: &impl FilePicker,
    kind: SelectionKind,
    preferred: &Path,
    home: &Path,
) -> Result<Option<PathBuf>> {
    backend.select(&Request {
        kind,
        directory: initial_directory(preferred, home),
    })
}

fn fallback_directory() -> PathBuf {
    std::env::current_dir()
        .ok()
        .filter(|path| path.is_dir())
        .or_else(|| {
            let temp = std::env::temp_dir();
            temp.is_dir().then_some(temp)
        })
        .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
}

pub fn home_directory() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_owned())
        .filter(|home| home.is_dir())
        .unwrap_or_else(fallback_directory)
}

pub fn initial_directory(preferred: &Path, home: &Path) -> PathBuf {
    preferred
        .canonicalize()
        .ok()
        .and_then(|path| {
            if path.is_dir() {
                Some(path)
            } else if path.is_file() {
                path.parent().map(Path::to_owned)
            } else {
                None
            }
        })
        .or_else(|| home.canonicalize().ok().filter(|path| path.is_dir()))
        .unwrap_or_else(fallback_directory)
}

#[cfg(test)]
mod tests;
