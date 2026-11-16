use anyhow::{ensure, Result};
use std::ffi::OsString;
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
    Executable,
    SoundFont,
    MusicXml,
    SaveMxl,
    SaveMusicXml,
    SaveMidi,
}

impl SelectionKind {
    pub fn is_save(self) -> bool {
        matches!(self, Self::SaveMxl | Self::SaveMusicXml | Self::SaveMidi)
    }
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Score => SCORE_EXTENSIONS,
            Self::SoundFont => &["sf2", "sf3"],
            Self::MusicXml => &["mxl", "musicxml", "xml"],
            Self::SaveMxl => &["mxl"],
            Self::SaveMusicXml => &["musicxml", "xml"],
            Self::SaveMidi => &["mid", "midi"],
            Self::Executable | Self::Folder => &[],
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Score => "Select music",
            Self::Folder => "Select folder",
            Self::Executable => "Select executable",
            Self::SoundFont => "Select SoundFont",
            Self::MusicXml => "Select MusicXML",
            Self::SaveMxl | Self::SaveMusicXml => "Save MusicXML",
            Self::SaveMidi => "Save MIDI",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: SelectionKind,
    pub directory: PathBuf,
    pub filename: Option<OsString>,
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
    let directory = if kind.is_save() && !preferred.is_dir() {
        preferred
            .parent()
            .and_then(|parent| parent.ancestors().find(|p| p.is_dir()))
            .and_then(|parent| parent.canonicalize().ok())
            .unwrap_or_else(|| initial_directory(preferred, home))
    } else {
        initial_directory(preferred, home)
    };
    let filename = if kind != SelectionKind::Folder && (kind.is_save() || preferred.is_file()) {
        preferred.file_name().map(|name| name.to_owned())
    } else {
        None
    };
    backend
        .select(&Request {
            kind,
            directory,
            filename,
        })?
        .map(|path| normalize_save_path(kind, path))
        .transpose()
}

fn normalize_save_path(kind: SelectionKind, mut path: PathBuf) -> Result<PathBuf> {
    if kind.is_save() {
        ensure!(
            path.file_name().is_some() && !path.is_dir(),
            "Choose an output filename, not a folder"
        );
        if path.extension().is_none() {
            path.set_extension(kind.extensions()[0]);
        }
        ensure!(
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| kind
                    .extensions()
                    .iter()
                    .any(|allowed| e.eq_ignore_ascii_case(allowed))),
            "Choose a {} filename ({})",
            kind.title(),
            kind.extensions().join(", ")
        );
    }
    Ok(path)
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
