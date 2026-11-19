//! Read-only bridge preflight. Never installs Wine, synchronizes wrappers or edits prefixes.
//! `compatible` permits an explicit attempt; it is not a plugin compatibility certificate.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub tool: String,
    pub wrapper_root: String,
    pub wine_loader: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tool: String::new(),
            wrapper_root: "~/.vst3/yabridge".into(),
            wine_loader: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub summary: String,
    /// No known-bad pair was found and both version probes succeeded.
    pub compatible: bool,
    pub wine_version: Option<String>,
    pub bridge_version: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let expression = regex::Regex::new(r"(?m)(\d+)\.(\d+)(?:\.(\d+))?").ok()?;
        let values = expression.captures(text)?;
        Some(Self(
            values.get(1)?.as_str().parse().ok()?,
            values.get(2)?.as_str().parse().ok()?,
            values.get(3).map_or(Some(0), |v| v.as_str().parse().ok())?,
        ))
    }
    fn display(self) -> String {
        format!("{}.{}.{}", self.0, self.1, self.2)
    }
}

fn incompatible_editor_versions(wine: Version, bridge: Version) -> bool {
    // Wine 9.22 introduced the embedding regression. A new major version
    // does not supply the missing fix in an older yabridge build.
    wine >= Version(9, 22, 0) && bridge <= Version(5, 1, 1)
}
fn validate_settings(settings: &Settings) -> Result<()> {
    for text in [
        &settings.tool,
        &settings.wrapper_root,
        &settings.wine_loader,
    ] {
        ensure!(
            text.len() <= 4096 && !text.contains('\0'),
            "Invalid bridge executable/root path"
        );
    }
    Ok(())
}

fn expand(path: &str) -> PathBuf {
    crate::app::expand_path(path.trim())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn within(path: &Path, root: &Path) -> bool {
    absolute(path)
        .ok()
        .zip(absolute(root).ok())
        .is_some_and(|(path, root)| path.starts_with(root))
        || path
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .is_some_and(|(path, root)| path.starts_with(root))
}

/// Classification only: never replace a bundle locator with its canonical target.
pub fn is_wrapper(bundle: &Path, settings: &Settings) -> bool {
    if !settings.wrapper_root.trim().is_empty() && within(bundle, &expand(&settings.wrapper_root)) {
        return true;
    }
    if let Some(home) = std::env::var_os("HOME") {
        if within(bundle, &PathBuf::from(home).join(".vst3/yabridge")) {
            return true;
        }
    }
    if bundle
        .components()
        .any(|part| part.as_os_str() == "yabridge")
    {
        return true;
    }
    // Renamed/copied chainloaders outside the advertised roots can still identify
    // themselves. Only inspect a bounded regular ELF prefix, never arbitrary text.
    let Some(stem) = bundle.file_stem() else {
        return false;
    };
    let mut library_name = stem.to_os_string();
    library_name.push(".so");
    let payload = bundle.join("Contents/x86_64-linux").join(library_name);
    #[cfg(target_os = "linux")]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt;
        File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(payload)
    };
    #[cfg(not(target_os = "linux"))]
    let opened = File::open(payload);
    let Ok(mut file) = opened else {
        return false;
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let mut magic = [0; 4];
    if file.read_exact(&mut magic).is_err() || &magic != b"\x7fELF" {
        return false;
    }
    let mut bytes = Vec::new();
    if file.take(2 * 1024 * 1024).read_to_end(&mut bytes).is_err() {
        return false;
    }
    bytes
        .windows(8)
        .any(|window| window.eq_ignore_ascii_case(b"yabridge"))
}

pub fn check_bundle(bundle: &Path, settings: &Settings) -> Result<()> {
    validate_settings(settings)?;
    if !is_wrapper(bundle, settings) {
        return Ok(());
    }
    let report = diagnostics(settings)?;
    ensure!(report.compatible, "{}", report.summary);
    Ok(())
}

/// Called only for an owned helper, not to modify the application/global environment.
pub fn apply_environment(command: &mut Command, settings: &Settings) -> Result<()> {
    validate_settings(settings)?;
    command.env_remove("WINEPREFIX");
    if !settings.wine_loader.trim().is_empty() {
        let loader = expand(&settings.wine_loader);
        command.env("WINELOADER", loader);
    }
    Ok(())
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_executable(value: &str) -> Option<PathBuf> {
    let value = expand(value);
    if value.components().count() > 1 || value.is_absolute() {
        return executable(&value).then(|| absolute(&value).ok()).flatten();
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|root| root.join(&value))
            .find(|file| executable(file))
    })
}

fn tool_path(settings: &Settings) -> Option<PathBuf> {
    if !settings.tool.trim().is_empty() {
        return find_executable(&settings.tool);
    }
    find_executable("yabridgectl").or_else(|| {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".local/share/yabridge/yabridgectl"))
            .filter(|file| executable(file))
    })
}

fn wine_path(settings: &Settings) -> Option<PathBuf> {
    if !settings.wine_loader.trim().is_empty() {
        return find_executable(&settings.wine_loader);
    }
    if let Some(loader) = std::env::var_os("WINELOADER") {
        return loader.to_str().and_then(find_executable);
    }
    find_executable("wine")
}

#[cfg(target_os = "linux")]
fn probe(path: &Path, argument: &str, settings: &Settings) -> Result<String> {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    let mut command = Command::new(path);
    command.arg(argument);
    apply_environment(&mut command, settings)?;
    let slot = Arc::new(Mutex::new(None));
    let mut process = crate::omr::linux::ManagedProcess::companion(&mut command, slot)?;
    let started = Instant::now();
    while process.poll()? {
        ensure!(
            started.elapsed() < Duration::from_secs(4),
            "Bridge diagnostic command timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = process.output()?;
    ensure!(
        output.len() <= 16 * 1024,
        "Bridge diagnostic output exceeded 16 KiB"
    );
    Ok(output)
}

#[cfg(not(target_os = "linux"))]
fn probe(_path: &Path, _argument: &str, _settings: &Settings) -> Result<String> {
    anyhow::bail!("yabridge diagnostics are available only on Linux")
}

pub fn diagnostics(settings: &Settings) -> Result<Report> {
    validate_settings(settings)?;
    let mut report = Report {
        summary: String::new(),
        compatible: false,
        wine_version: None,
        bridge_version: None,
        warnings: vec![],
    };
    let Some(tool) = tool_path(settings) else {
        report.summary =
            "yabridgectl was not found. Browse to its executable; shell aliases are not used."
                .into();
        return Ok(report);
    };
    let Some(wine) = wine_path(settings) else {
        report.summary = "Wine loader was not found. Choose an existing compatible Wine runtime; no runtime will be installed automatically.".into();
        return Ok(report);
    };
    let bridge_output =
        probe(&tool, "--version", settings).context("Checking yabridgectl version")?;
    let wine_output =
        probe(&wine, "--version", settings).context("Checking Wine loader version")?;
    let bridge = Version::parse(&bridge_output).context("Cannot parse yabridgectl version")?;
    let wine = Version::parse(&wine_output).context("Cannot parse Wine version")?;
    report.bridge_version = Some(bridge.display());
    report.wine_version = Some(wine.display());
    if !settings.wrapper_root.trim().is_empty()
        && expand(&settings.wrapper_root).join("yabridge").is_dir()
    {
        report.warnings.push("A nested generated yabridge tree exists. Review source registrations and duplicate wrappers manually; generated output should not be registered as plugin source. Nothing was changed.".into());
    }
    if incompatible_editor_versions(wine, bridge) {
        report.summary = format!(
            "Incompatible yabridge profile: Wine {} with yabridge {}. Wine 9.22 and newer (including 10.x and 11.x) have known embedded-editor mouse input/coordinate failures with yabridge 5.1.1 or earlier, even when audio plays. Upgrading only to 5.1.1 does not fix this. Select a separately verified compatible runtime/bridge profile; Wine and prefixes were not changed.",
            wine.display(), bridge.display(),
        );
        report
            .warnings
            .push("Native Linux VST3 instruments do not need Wine or yabridge.".into());
        return Ok(report);
    }
    report.compatible = true;
    report.summary = format!(
        "Version preflight permits an explicit scan: Wine {}, yabridge {}. Individual plugin audio/editor/state compatibility remains unverified.",
        wine.display(), bridge.display(),
    );
    report.warnings.push("Tool version does not prove every generated wrapper is current. Register/sync changes, vendor installs and license activation remain user-directed.".into());
    report.warnings.push("A Wine prefix is configuration isolation, not a security sandbox. One Wine loader is selected for the complete helper session.".into());
    if wine != Version(9, 21, 0) || bridge != Version(5, 1, 1) {
        report.warnings.push("This is not the exact upstream Wine Staging 9.21/yabridge 5.1.1 baseline; validate audio, editor and state before relying on it.".into());
    }
    if let Ok(registrations) = probe(&tool, "list", settings) {
        let home_output = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".vst3"));
        if registrations.lines().map(str::trim).any(|line| {
            home_output
                .as_ref()
                .is_some_and(|root| Path::new(line) == root)
                || (!settings.wrapper_root.trim().is_empty()
                    && Path::new(line) == expand(&settings.wrapper_root))
        }) {
            report.warnings.push("A generated .vst3 wrapper/output directory is registered as a source in yabridgectl. Review possible nested/duplicate wrappers manually; no registrations were changed.".into());
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires installed Wine/yabridgectl; only read-only version/list probes, never plugin loading"]
    #[cfg(target_os = "linux")]
    fn installed_profile_diagnostics_without_loading_a_plugin() {
        let report = diagnostics(&Settings::default()).unwrap();
        assert!(report.wine_version.is_some() && report.bridge_version.is_some());
        if let Some((wine, bridge)) = report
            .wine_version
            .as_deref()
            .and_then(Version::parse)
            .zip(report.bridge_version.as_deref().and_then(Version::parse))
        {
            if incompatible_editor_versions(wine, bridge) {
                assert!(!report.compatible);
            }
        }
        println!("{}", report.summary);
        for warning in report.warnings {
            println!("{warning}");
        }
    }

    #[test]
    fn parses_distro_versions_without_guessing_missing_versions() {
        assert_eq!(
            Version::parse("wine-10.0 (Ubuntu 10.0~repack)"),
            Some(Version(10, 0, 0))
        );
        assert_eq!(Version::parse("yabridgectl 5.1.1"), Some(Version(5, 1, 1)));
        assert_eq!(
            Version::parse("wine-11.22 (Staging)"),
            Some(Version(11, 22, 0))
        );
        assert_eq!(Version::parse("unknown"), None);
    }
    #[test]
    fn editor_regression_has_no_wine_major_version_upper_bound() {
        for wine in [
            Version(9, 22, 0),
            Version(9, 23, 0),
            Version(10, 0, 0),
            Version(10, 22, 1),
            Version(11, 0, 0),
            Version(11, 22, 0),
            Version(12, 0, 0),
        ] {
            for bridge in [Version(5, 0, 5), Version(5, 1, 0), Version(5, 1, 1)] {
                assert!(
                    incompatible_editor_versions(wine, bridge),
                    "{wine:?}/{bridge:?}"
                );
            }
        }
        for wine in [Version(8, 0, 0), Version(9, 21, 0), Version(9, 21, 99)] {
            assert!(!incompatible_editor_versions(wine, Version(5, 1, 1)));
        }
        // Unknown newer bridge versions retain the existing explicit-attempt
        // policy, not a claim that their plugins or editors have been verified.
        for bridge in [Version(5, 1, 2), Version(5, 2, 0), Version(6, 0, 0)] {
            assert!(!incompatible_editor_versions(Version(11, 22, 0), bridge));
        }
    }

    #[test]
    fn native_plugin_does_not_require_working_wine_or_bridge_tools() {
        let settings = Settings {
            tool: "/missing/tool".into(),
            wrapper_root: "/some/wrappers".into(),
            wine_loader: "/missing/wine".into(),
        };
        assert!(check_bundle(Path::new("/native/Test.vst3"), &settings).is_ok());
    }

    #[test]
    fn environment_overrides_are_scoped_to_the_child_command() {
        let settings = Settings {
            wine_loader: "/opt/runtime/wine".into(),
            ..Default::default()
        };
        let mut command = Command::new("/bin/true");
        apply_environment(&mut command, &settings).unwrap();
        let env: Vec<_> = command.get_envs().collect();
        assert!(env
            .iter()
            .any(|(key, value)| *key == "WINEPREFIX" && value.is_none()));
        assert!(env.iter().any(|(key, value)| *key == "WINELOADER"
            && *value == Some(std::ffi::OsStr::new("/opt/runtime/wine"))));
    }

    #[test]
    fn marker_and_root_classification_do_not_canonicalize_the_selected_locator() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Copy.v1.vst3");
        let contents = bundle.join("Contents/x86_64-linux");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Copy.v1.so"),
            b"\x7fELF\0binary yabridge marker",
        )
        .unwrap();
        let settings = Settings {
            wrapper_root: root.path().join("wrappers").display().to_string(),
            ..Default::default()
        };
        assert!(is_wrapper(&bundle, &settings));
        assert!(is_wrapper(
            &root.path().join("wrappers/Other.vst3"),
            &settings
        ));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn known_bad_profile_is_blocked_without_changing_any_prefix() {
        use std::os::unix::fs::PermissionsExt;
        for (wine_version, bridge_version) in
            [("10.0", "5.1.0"), ("11.0", "5.1.1"), ("11.22", "5.1.1")]
        {
            let root = tempfile::tempdir().unwrap();
            let tool = root.path().join("yabridgectl");
            let wine = root.path().join("wine");
            std::fs::write(
                &tool,
                format!("#!/bin/sh\nprintf 'yabridgectl {bridge_version}\\n'\n"),
            )
            .unwrap();
            std::fs::write(
                &wine,
                format!("#!/bin/sh\nprintf 'wine-{wine_version} (Staging fixture)\\n'\n"),
            )
            .unwrap();
            for path in [&tool, &wine] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let wrappers = root.path().join("wrappers");
            let settings = Settings {
                tool: tool.display().to_string(),
                wrapper_root: wrappers.display().to_string(),
                wine_loader: wine.display().to_string(),
            };
            let report = diagnostics(&settings).unwrap();
            assert!(!report.compatible);
            assert!(report
                .summary
                .contains("Upgrading only to 5.1.1 does not fix"));
            assert!(report.summary.contains(&format!("Wine {wine_version}.")));
            assert!(report.summary.contains("including 10.x and 11.x"));
            assert!(report.summary.contains("even when audio plays"));
            assert!(check_bundle(&wrappers.join("Amped.vst3"), &settings).is_err());
            assert!(!root.path().join(".wine").exists());
        }
    }
}
