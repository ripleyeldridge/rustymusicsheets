use super::*;
use crate::picker::SelectionKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PathField {
    Audiveris,
    FluidSynth,
    AudioHost,
    SoundFont,
    LoadMusicXml,
    ExportMusicXml(bool),
    ExportMidi,
}
impl PathField {
    pub fn kind(self) -> SelectionKind {
        match self {
            Self::Audiveris | Self::FluidSynth | Self::AudioHost => SelectionKind::Executable,
            Self::SoundFont => SelectionKind::SoundFont,
            Self::LoadMusicXml => SelectionKind::MusicXml,
            Self::ExportMusicXml(true) => SelectionKind::SaveMxl,
            Self::ExportMusicXml(false) => SelectionKind::SaveMusicXml,
            Self::ExportMidi => SelectionKind::SaveMidi,
        }
    }
    fn is_setting(self) -> bool {
        matches!(
            self,
            Self::Audiveris | Self::FluidSynth | Self::SoundFont | Self::AudioHost
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PathRequest {
    pub field: PathField,
    pub preferred: PathBuf,
    owner: Option<(i64, String, Option<PathBuf>)>,
}

pub(super) fn entry(
    ui: &mut egui::Ui,
    field: PathField,
    value: &mut String,
    enabled: bool,
) -> bool {
    ui.push_id(field, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .id_salt("path")
                    .desired_width((ui.available_width() - 90.0).max(40.0)),
            );
            ui.add_enabled(enabled, egui::Button::new("Browse…"))
                .clicked()
        })
        .inner
    })
    .inner
}

impl Music {
    pub(crate) fn browse_audio_host(&mut self, settings: &crate::audio::Settings) {
        self.request_path(PathField::AudioHost, &settings.host_executable);
    }
    pub(crate) fn browse_settings(&mut self, field: PathField, settings: &Settings) {
        let value = match field {
            PathField::Audiveris => &settings.audiveris_executable,
            PathField::FluidSynth => &settings.fluidsynth_executable,
            PathField::SoundFont => &settings.soundfont,
            _ => return,
        };
        self.request_path(field, value);
    }
    pub(super) fn request_path(&mut self, field: PathField, value: &str) {
        if self.path_picker_active || self.path_request.is_some() {
            return;
        }
        let mut preferred = expand_path(value.trim());
        if field.kind() == SelectionKind::Executable
            && preferred.components().count() == 1
            && !preferred.is_file()
        {
            if let Some(path) = std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(&preferred))
                    .find(|file| file.is_file())
            }) {
                preferred = path;
            }
        }
        let owner = if field.is_setting() {
            None
        } else {
            self.binding
                .as_ref()
                .map(|(id, hash)| (*id, hash.clone(), self.xml.clone()))
        };
        self.path_request = Some(PathRequest {
            field,
            preferred,
            owner,
        });
    }
    pub(crate) fn take_path_request(&mut self) -> Option<PathRequest> {
        self.path_request.take()
    }

    pub(crate) fn apply_path(
        &mut self,
        request: PathRequest,
        path: PathBuf,
        settings: &mut Settings,
        audio: &mut crate::audio::Settings,
    ) -> Result<()> {
        if !request.field.is_setting() {
            let owner = self
                .binding
                .as_ref()
                .map(|(id, hash)| (*id, hash.clone(), self.xml.clone()));
            anyhow::ensure!(
                request.owner.is_some() && request.owner == owner,
                "The score or movement changed while the chooser was open; browse again"
            );
        }
        let value=path.to_str().context("This field requires a Unicode filename; choose a representable path rather than replacing filename bytes")?.to_owned();
        let kind = request.field.kind();
        if !kind.is_save() {
            anyhow::ensure!(path.is_file(), "Select an existing file");
            if kind == SelectionKind::Executable {
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::fs::PermissionsExt;
                    anyhow::ensure!(
                        path.metadata()?.permissions().mode() & 0o111 != 0,
                        "Select an executable file"
                    );
                }
            } else {
                anyhow::ensure!(
                    path.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|extension| kind
                            .extensions()
                            .iter()
                            .any(|allowed| extension.eq_ignore_ascii_case(allowed))),
                    "Choose a supported {} file",
                    kind.title()
                );
            }
        }
        match request.field {
            PathField::Audiveris => settings.audiveris_executable = value,
            PathField::FluidSynth => settings.fluidsynth_executable = value,
            PathField::AudioHost => audio.host_executable = value,
            PathField::SoundFont => settings.soundfont = value,
            PathField::LoadMusicXml => self.load_path = value,
            PathField::ExportMusicXml(_) => self.xml_destination = value,
            PathField::ExportMidi => self.midi_destination = value,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "linux")]
    fn audio_host_chooser_only_updates_global_audio_path() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("rms-audio-host");
        std::fs::write(&host, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut music = Music::default();
        let mut audio = crate::audio::Settings::default();
        let mut settings = Settings::default();
        let previous = settings.fluidsynth_executable.clone();
        music.browse_audio_host(&audio);
        let request = music.take_path_request().unwrap();
        assert_eq!(request.field.kind(), SelectionKind::Executable);
        music
            .apply_path(request, host.clone(), &mut settings, &mut audio)
            .unwrap();
        assert_eq!(audio.host_executable, host.to_str().unwrap());
        assert_eq!(audio.backend, crate::audio::Backend::Legacy);
        assert_eq!(settings.fluidsynth_executable, previous);
        assert!(
            music.playback.is_none(),
            "Selecting a path must not launch audio"
        );
    }

    #[test]
    fn global_settings_and_per_score_paths_route_without_automatic_side_effects() {
        let root = tempfile::tempdir().unwrap();
        let bank = root.path().join("Cello été.sf2");
        std::fs::write(&bank, b"generated bank stand-in").unwrap();
        let mut settings = Settings::default();
        let mut music = Music::default();
        music.browse_settings(PathField::SoundFont, &settings);
        let request = music.take_path_request().unwrap();
        assert_eq!(request.field.kind(), SelectionKind::SoundFont);
        music
            .apply_path(
                request,
                bank.clone(),
                &mut settings,
                &mut crate::audio::Settings::default(),
            )
            .unwrap();
        assert_eq!(settings.soundfont, bank.to_str().unwrap());
        music.binding = Some((1, "hash".into()));
        let xml = root.path().join("Corrected score.musicxml");
        std::fs::write(&xml, b"<score-partwise/>").unwrap();
        music.request_path(PathField::LoadMusicXml, "");
        let request = music.take_path_request().unwrap();
        music
            .apply_path(
                request,
                xml.clone(),
                &mut settings,
                &mut crate::audio::Settings::default(),
            )
            .unwrap();
        assert_eq!(music.load_path, xml.to_str().unwrap());
        assert!(music.loading.is_none(), "Browse must not silently load");
        for field in [PathField::ExportMusicXml(true), PathField::ExportMidi] {
            let name = if field == PathField::ExportMidi {
                "out.mid"
            } else {
                "out.mxl"
            };
            let output = root.path().join(name);
            music.request_path(field, "");
            let request = music.take_path_request().unwrap();
            music
                .apply_path(
                    request,
                    output.clone(),
                    &mut settings,
                    &mut crate::audio::Settings::default(),
                )
                .unwrap();
            assert!(!output.exists(), "Browse must not export");
        }
        music.request_path(PathField::ExportMidi, "");
        let request = music.take_path_request().unwrap();
        let previous = music.midi_destination.clone();
        music.binding = Some((2, "other".into()));
        assert!(music
            .apply_path(
                request,
                root.path().join("wrong-score.mid"),
                &mut settings,
                &mut crate::audio::Settings::default()
            )
            .is_err());
        assert_eq!(music.midi_destination, previous);
    }

    #[test]
    fn chooser_rejects_wrong_formats_and_non_executable_helpers_without_changing_fields() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-soundfont.pdf");
        std::fs::write(&file, b"test").unwrap();
        let mut music = Music::default();
        let mut settings = Settings::default();
        let old = settings.soundfont.clone();
        music.browse_settings(PathField::SoundFont, &settings);
        let request = music.take_path_request().unwrap();
        assert!(music
            .apply_path(
                request,
                file.clone(),
                &mut settings,
                &mut crate::audio::Settings::default()
            )
            .is_err());
        assert_eq!(settings.soundfont, old);
        #[cfg(target_os = "linux")]
        {
            music.browse_settings(PathField::Audiveris, &settings);
            let request = music.take_path_request().unwrap();
            assert!(music
                .apply_path(
                    request,
                    file,
                    &mut settings,
                    &mut crate::audio::Settings::default()
                )
                .is_err());
        }
    }

    #[test]
    #[cfg(unix)]
    fn string_fields_reject_unrepresentable_paths_instead_of_replacing_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let mut music = Music::default();
        let mut settings = Settings::default();
        let previous = settings.soundfont.clone();
        music.browse_settings(PathField::SoundFont, &settings);
        let request = music.take_path_request().unwrap();
        let file = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/score-\xff.sf2".to_vec(),
        ));
        assert!(music
            .apply_path(
                request,
                file,
                &mut settings,
                &mut crate::audio::Settings::default()
            )
            .unwrap_err()
            .to_string()
            .contains("Unicode"));
        assert_eq!(settings.soundfont, previous);
    }
}
