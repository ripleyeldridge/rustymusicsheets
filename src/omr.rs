//! External, offline Linux OMR and MIDI playback. No score is modified in place.
#[cfg(not(target_os = "linux"))]
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audiveris_executable: String,
    pub fluidsynth_executable: String,
    pub soundfont: String,
}

fn default_audiveris() -> String {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = |path: &std::path::Path| {
            path.is_file()
                && path
                    .metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        };
        if let Some(path) = std::env::var_os("PATH") {
            if std::env::split_paths(&path).any(|dir| executable(&dir.join("Audiveris"))) {
                return "Audiveris".into();
            }
        }
        let installed = std::path::Path::new("/opt/audiveris/bin/Audiveris");
        if executable(installed) {
            return installed.to_string_lossy().into_owned();
        }
    }
    "audiveris".into()
}
fn default_soundfont() -> String {
    #[cfg(target_os = "linux")]
    for candidate in [
        "/usr/share/sounds/sf2/default-GM.sf2",
        "/usr/share/sounds/sf2/FluidR3_GM.sf2",
        "/usr/share/sounds/sf3/default-GM.sf3",
    ] {
        if std::path::Path::new(candidate).is_file() {
            return candidate.into();
        }
    }
    String::new()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audiveris_executable: default_audiveris(),
            fluidsynth_executable: "fluidsynth".into(),
            soundfont: default_soundfont(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecognitionArtifacts {
    pub source_hash: String,
    /// Every exported movement, sorted by path. The caller must offer a choice.
    pub movements: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum RecognitionUpdate {
    Progress(String),
    Finished(std::result::Result<RecognitionArtifacts, String>),
}

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Playback, RecognitionJob};

#[cfg(not(target_os = "linux"))]
pub struct RecognitionJob;
#[cfg(not(target_os = "linux"))]
impl RecognitionJob {
    pub fn start(
        _source: PathBuf,
        _source_hash: String,
        _output_base: PathBuf,
        _settings: Settings,
        _repaint: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        bail!("Audiveris recognition is supported only on Linux")
    }
    pub fn poll(&mut self) -> Vec<RecognitionUpdate> {
        Vec::new()
    }
    pub fn cancel(&self) {}
}

#[cfg(not(target_os = "linux"))]
pub struct Playback;
#[cfg(not(target_os = "linux"))]
impl Playback {
    pub fn start(_settings: &Settings, _midi: &[u8]) -> Result<Self> {
        bail!("FluidSynth playback is supported only on Linux")
    }
    pub fn poll(&mut self) -> Result<bool> {
        bail!("FluidSynth playback is supported only on Linux")
    }
    pub fn stop(&mut self) {}
}
