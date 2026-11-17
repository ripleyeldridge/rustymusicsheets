//! Data-only VST3 models. No plugin code is loaded into the reader.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

pub const MAX_STATE: u64 = 64 * 1024 * 1024;
pub const MAX_SESSION_STATE: u64 = 256 * 1024 * 1024;
const MAX_MODULE: u64 = 512 * 1024 * 1024;

fn regular_file(path: &Path, max: u64) -> Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= max,
        "Not a regular file or exceeds size limit"
    );
    Ok(file)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateFile {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    #[default]
    Soundfont,
    Vst3 {
        path: PathBuf,
        class_id: String,
        module_sha256: String,
        gain: f64,
        component_state: Option<StateFile>,
        controller_state: Option<StateFile>,
    },
}

pub fn hex(value: &str, length: usize, uppercase: bool) -> bool {
    value.len() == length
        && value.bytes().all(|b| {
            b.is_ascii_digit()
                || if uppercase {
                    (b'A'..=b'F').contains(&b)
                } else {
                    (b'a'..=b'f').contains(&b)
                }
        })
}

impl Source {
    pub fn select_class(&self, path: PathBuf, class_id: String, module_sha256: String) -> Self {
        let (gain, component_state, controller_state) = match self {
            Self::Vst3 {
                class_id: previous,
                gain,
                component_state,
                controller_state,
                ..
            } if *previous == class_id => {
                (*gain, component_state.clone(), controller_state.clone())
            }
            _ => (1.0, None, None),
        };
        Self::Vst3 {
            path,
            class_id,
            module_sha256,
            gain,
            component_state,
            controller_state,
        }
    }
    pub fn is_plugin(&self) -> bool {
        matches!(self, Self::Vst3 { .. })
    }
    pub fn validate(&self) -> Result<()> {
        if let Self::Vst3 {
            path,
            class_id,
            module_sha256,
            gain,
            component_state,
            controller_state,
        } = self
        {
            ensure!(
                path.is_absolute() && path.to_str().is_some(),
                "Invalid bundle path"
            );
            ensure!(hex(class_id, 32, true), "Invalid VST3 class identity");
            ensure!(hex(module_sha256, 64, false), "Invalid module digest");
            ensure!(
                gain.is_finite() && (0.0..=1.0).contains(gain),
                "Invalid plugin gain"
            );
            let mut bytes = 0u64;
            for file in [component_state, controller_state].into_iter().flatten() {
                ensure!(
                    file.path.is_absolute() && hex(&file.sha256, 64, false),
                    "Invalid state descriptor"
                );
                bytes = bytes
                    .checked_add(file.bytes)
                    .context("State size overflow")?;
            }
            ensure!(bytes <= MAX_STATE, "Plugin state exceeds 64 MiB");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Class {
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
}

impl Class {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            hex(&self.class_id, 32, true)
                && self.name.len() <= 256
                && self.vendor.len() <= 256
                && self.version.len() <= 128
                && [&self.name, &self.vendor, &self.version]
                    .iter()
                    .all(|s| !s.chars().any(char::is_control)),
            "Invalid plugin class metadata"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Parameter {
    pub id: u32,
    pub title: String,
    pub units: String,
    pub value: f64,
    pub default_value: f64,
    pub step_count: i32,
    pub flags: u32,
}

impl Parameter {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.title.len() <= 256
                && self.units.len() <= 128
                && !self
                    .title
                    .chars()
                    .chain(self.units.chars())
                    .any(char::is_control)
                && self.value.is_finite()
                && (0.0..=1.0).contains(&self.value)
                && self.default_value.is_finite()
                && (0.0..=1.0).contains(&self.default_value)
                && self.step_count >= 0,
            "Invalid plugin parameter"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CheckpointPart {
    pub part_id: String,
    pub class_id: String,
    pub component_state: Option<StateFile>,
    pub controller_state: Option<StateFile>,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    pub busy: bool,
    pub capabilities: Vec<String>,
    pub dirty_parts: Vec<String>,
    pub parameters: BTreeMap<String, (u32, u32, Vec<Parameter>)>,
    pub operation_error: Option<String>,
    pub checkpoint: Option<(u64, Vec<CheckpointPart>)>,
}

#[derive(Clone, Debug)]
pub enum Request {
    Gain {
        part_id: String,
        gain: f64,
    },
    Parameters {
        part_id: String,
        offset: u32,
    },
    Parameter {
        part_id: String,
        param_id: u32,
        value: f64,
    },
    Editor {
        part_id: String,
        open: bool,
    },
    Preset {
        part_id: String,
        path: PathBuf,
    },
    Checkpoint {
        destination: PathBuf,
    },
}

/// Preserve the bundle locator, even when its ELF payload is a yabridge symlink.
/// Only the payload is opened for validation/hash; SDK loading receives `bundle`.
pub fn inspect_bundle(bundle: &Path) -> Result<String> {
    ensure!(
        bundle.is_absolute() && bundle.to_str().is_some(),
        "Choose an absolute Unicode bundle folder"
    );
    ensure!(
        !bundle
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "Bundle paths may not contain '..'"
    );
    ensure!(
        bundle.extension().is_some_and(|e| e == "vst3") && bundle.is_dir(),
        "Select a Linux .vst3 bundle folder, not a Windows .vst3, loose .dll or VST2 .so"
    );
    let stem = bundle.file_stem().context("Missing bundle name")?;
    let mut payload_name = stem.to_os_string();
    payload_name.push(".so");
    let payload = bundle.join("Contents/x86_64-linux").join(payload_name);
    let mut file = regular_file(&payload, MAX_MODULE).with_context(|| format!(
        "Missing Linux x86_64 VST3 payload {}; Windows originals and other architectures are unsupported",
        payload.display()
    ))?;
    let metadata = file.metadata()?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)
        .context("Truncated ELF module")?;
    ensure!(
        &header[..4] == b"\x7fELF"
            && header[4] == 2
            && header[5] == 1
            && u16::from_le_bytes([header[16], header[17]]) == 3
            && u16::from_le_bytes([header[18], header[19]]) == 62,
        "VST3 payload must be a Linux x86_64 little-endian ELF shared object, not PE/Windows or another architecture"
    );
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let bytes = std::io::copy(&mut file.by_ref().take(MAX_MODULE + 1), &mut hash)?;
    ensure!(
        bytes == metadata.len()
            && bytes <= MAX_MODULE
            && file.metadata()?.len() == metadata.len()
            && file.metadata()?.modified()? == metadata.modified()?,
        "Module changed or grew during fingerprinting"
    );
    Ok(format!("{:x}", hash.finalize()))
}

pub fn validate_preset(path: &Path, class: &str) -> Result<()> {
    ensure!(
        path.is_absolute() && path.to_str().is_some(),
        "Choose an absolute preset path"
    );
    let mut file = regular_file(path, MAX_STATE)?;
    let mut header = [0u8; 48];
    file.read_exact(&mut header)?;
    ensure!(
        &header[..4] == b"VST3" && &header[8..40] == class.as_bytes(),
        "Preset belongs to a different VST3 class or has an invalid header"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn fixture(root: &Path, name: &str) -> PathBuf {
    let bundle = root.join(format!("{name}.vst3"));
    let payload = bundle
        .join("Contents/x86_64-linux")
        .join(format!("{name}.so"));
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    let mut header = [0u8; 64];
    header[..4].copy_from_slice(b"\x7fELF");
    header[4] = 2;
    header[5] = 1;
    header[16] = 3;
    header[18] = 62;
    std::fs::write(payload, header).unwrap();
    bundle
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_class_reselection_and_locate_preserve_state_and_gain() {
        let state = StateFile {
            path: "/private/checkpoint".into(),
            sha256: "a".repeat(64),
            bytes: 12,
        };
        let source = Source::Vst3 {
            path: "/old.vst3".into(),
            class_id: "A".repeat(32),
            module_sha256: "a".repeat(64),
            gain: 0.37,
            component_state: Some(state.clone()),
            controller_state: Some(state.clone()),
        };
        for path in ["/old.vst3", "/relocated.vst3"] {
            let selected = source.select_class(path.into(), "A".repeat(32), "b".repeat(64));
            assert!(
                matches!(selected, Source::Vst3 { gain: 0.37, component_state: Some(ref component), controller_state: Some(ref controller), .. } if component == &state && controller == &state)
            );
        }
        assert!(matches!(
            source.select_class("/other.vst3".into(), "B".repeat(32), "c".repeat(64)),
            Source::Vst3 {
                gain: 1.0,
                component_state: None,
                controller_state: None,
                ..
            }
        ));
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn module_and_preset_fifos_are_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let root = tempfile::tempdir().unwrap();
        let bundle = fixture(root.path(), "FIFO");
        let module = bundle.join("Contents/x86_64-linux/FIFO.so");
        std::fs::remove_file(&module).unwrap();
        for path in [module.clone(), root.path().join("fifo.vstpreset")] {
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        }
        let started = std::time::Instant::now();
        assert!(inspect_bundle(&bundle).is_err());
        assert!(validate_preset(&root.path().join("fifo.vstpreset"), &"A".repeat(32)).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
    #[test]
    fn dotted_unicode_bundle_names_preserve_locator_and_validate_actual_elf() {
        let root = tempfile::tempdir().unwrap();
        let bundle = fixture(root.path(), "Été synth.v1");
        let hash = inspect_bundle(&bundle).unwrap();
        assert!(hex(&hash, 64, false));
        #[cfg(unix)]
        {
            let wrapper = root.path().join("wrapper.vst3");
            std::os::unix::fs::symlink(&bundle, &wrapper).unwrap();
            // Bundle stem must still match payload, so a renamed wrapper is invalid.
            assert!(inspect_bundle(&wrapper).is_err());
            let payload = bundle.join("Contents/x86_64-linux/Été synth.v1.so");
            let target = root.path().join("chainloader.so");
            std::fs::rename(&payload, &target).unwrap();
            std::os::unix::fs::symlink(target, payload).unwrap();
            assert_eq!(inspect_bundle(&bundle).unwrap(), hash);
        }
        for (name, bytes) in [
            ("windows.vst3", b"MZ".as_slice()),
            ("vst2.so", b"\x7fELF".as_slice()),
        ] {
            let path = root.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(inspect_bundle(&path).is_err());
        }
        let bad = fixture(root.path(), "arm");
        let payload = bad.join("Contents/x86_64-linux/arm.so");
        let mut bytes = std::fs::read(&payload).unwrap();
        bytes[18] = 183;
        std::fs::write(payload, bytes).unwrap();
        assert!(inspect_bundle(&bad)
            .unwrap_err()
            .to_string()
            .contains("x86_64"));
    }
    #[test]
    fn presets_are_class_checked_and_size_bounded_before_execution() {
        let root = tempfile::tempdir().unwrap();
        let preset = root.path().join("state.vstpreset");
        let class = "A".repeat(32);
        let mut bytes = [0u8; 48];
        bytes[..4].copy_from_slice(b"VST3");
        bytes[8..40].copy_from_slice(class.as_bytes());
        std::fs::write(&preset, bytes).unwrap();
        validate_preset(&preset, &class).unwrap();
        assert!(validate_preset(&preset, &"B".repeat(32)).is_err());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&preset)
            .unwrap()
            .set_len(MAX_STATE + 1)
            .unwrap();
        assert!(validate_preset(&preset, &class).is_err());
    }
}
