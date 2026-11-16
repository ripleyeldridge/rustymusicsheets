//! Optional out-of-process audio engine. Ordinary builds need no native SDK.
use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
mod ipc;
pub mod plan;
#[cfg(target_os = "linux")]
pub(crate) mod supervisor;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Legacy,
    PipeWire,
    Jack,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy => "FluidSynth (legacy, default)",
            Self::PipeWire => "Audio host preview — PipeWire/JACK",
            Self::Jack => "Audio host preview — native JACK",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub backend: Backend,
    /// Empty selects the package-controlled companion, never an arbitrary PATH entry.
    pub host_executable: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostics {
    pub rendered_position: f64,
    pub sample_rate: u32,
    pub latency_frames: u64,
    pub xruns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_preferences_keep_legacy_default_and_host_settings_roundtrip() {
        let mut preferences: crate::model::Preferences = serde_json::from_str(
            r#"{"next_key":"PageDown","previous_key":"PageUp","recursive":true}"#,
        )
        .unwrap();
        assert_eq!(preferences.audio, Settings::default());
        assert_eq!(preferences.audio.backend, Backend::Legacy);
        preferences.audio = Settings {
            backend: Backend::PipeWire,
            host_executable: "/opt/host/rms-audio-host".into(),
        };
        let saved = serde_json::to_vec(&preferences).unwrap();
        let restored: crate::model::Preferences = serde_json::from_slice(&saved).unwrap();
        assert_eq!(preferences.audio, restored.audio);
        assert!(serde_json::from_str::<Settings>(r#"{"backend":"unrecognized"}"#).is_err());
    }
}
