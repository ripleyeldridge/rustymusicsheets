//! Explicit, bounded discovery. Every module executes in a fresh helper process.
use super::{
    ipc::{Connection, VERSION},
    plugins::{inspect_bundle, Class},
    Settings,
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const MAX_CANDIDATES: usize = 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_CATALOGUE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub module_sha256: String,
    pub profile: String,
    pub classes: Vec<Class>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
}

impl Catalogue {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let file = fs::File::open(path)?;
        ensure!(
            file.metadata()?.len() <= MAX_CATALOGUE,
            "Oversized plugin catalogue"
        );
        let catalogue: Self = serde_json::from_reader(file)?;
        ensure!(
            catalogue.entries.len() <= MAX_ENTRIES,
            "Too many catalogue entries"
        );
        for entry in &catalogue.entries {
            ensure!(
                entry.path.is_absolute()
                    && entry.path.to_str().is_some()
                    && super::plugins::hex(&entry.module_sha256, 64, false)
                    && super::plugins::hex(&entry.profile, 64, false)
                    && entry.classes.len() <= 128
                    && entry.failure.as_ref().is_none_or(|s| s.len() <= 2048),
                "Invalid plugin catalogue entry"
            );
            for class in &entry.classes {
                class.validate()?;
            }
        }
        Ok(catalogue)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("Catalogue has no directory")?;
        fs::create_dir_all(parent)?;
        let raw = serde_json::to_vec(self)?;
        ensure!(
            raw.len() as u64 <= MAX_CATALOGUE && self.entries.len() <= MAX_ENTRIES,
            "Catalogue limit reached"
        );
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&raw)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
    pub fn scan(
        &mut self,
        settings: &Settings,
        candidates: &[PathBuf],
        cancelled: &AtomicBool,
        retry: bool,
    ) -> Result<Vec<String>> {
        ensure!(
            candidates.len() <= MAX_CANDIDATES,
            "Too many plugin candidates"
        );
        let mut profile = profile(settings)?;
        let mut notifications = Vec::new();
        if candidates
            .iter()
            .any(|p| super::yabridge::is_wrapper(p, &settings.yabridge))
        {
            let report = super::yabridge::diagnostics(&settings.yabridge)?;
            if let Some(notification) = report.notification {
                notifications.push(notification);
            }
            profile = format!(
                "{:x}",
                Sha256::digest(
                    format!(
                        "{profile}|{:?}|{:?}",
                        report.wine_version, report.bridge_version
                    )
                    .as_bytes()
                )
            );
        }
        for path in candidates {
            ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
            let inspection = inspect_bundle(path);
            let hash = match &inspection {
                Ok(hash) => hash.clone(),
                Err(_) => {
                    let mut payload_name = path.file_stem().unwrap_or_default().to_os_string();
                    payload_name.push(".so");
                    let payload = path.join("Contents/x86_64-linux").join(payload_name);
                    let signature = |p: &Path| {
                        fs::symlink_metadata(p)
                            .ok()
                            .map(|m| (m.len(), m.modified().ok()))
                    };
                    format!(
                        "{:x}",
                        Sha256::digest(
                            format!("{path:?}|{:?}|{:?}", signature(path), signature(&payload))
                                .as_bytes()
                        )
                    )
                }
            };
            if !retry
                && self.entries.iter().any(|entry| {
                    &entry.path == path && entry.module_sha256 == hash && entry.profile == profile
                })
            {
                continue;
            }
            let result = match inspection {
                Ok(_) => scan_module(settings, path, &hash, cancelled),
                Err(error) => Err(error),
            };
            ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
            let (classes, failure) = match result {
                Ok(classes) => (classes, None),
                Err(error) => {
                    let mut message = format!("{error:#}");
                    while message.len() > 2048 {
                        message.pop();
                    }
                    (Vec::new(), Some(message))
                }
            };
            self.entries.retain(|entry| &entry.path != path);
            ensure!(
                self.entries.len() < MAX_ENTRIES,
                "Plugin catalogue limit reached"
            );
            self.entries.push(Entry {
                path: path.clone(),
                module_sha256: hash,
                profile: profile.clone(),
                classes,
                failure,
            });
        }
        Ok(notifications)
    }
}

fn profile(settings: &Settings) -> Result<String> {
    let helper = super::supervisor::executable(settings)?;
    let metadata = helper.metadata()?;
    let data = format!(
        "{:?}|{:?}|{:?}|{:?}|v2",
        settings.yabridge,
        helper,
        metadata.len(),
        metadata.modified()?
    );
    Ok(format!("{:x}", Sha256::digest(data.as_bytes())))
}

/// No recursive symlink traversal. Stop at bundles instead of entering resources.
pub fn enumerate(roots: &[String], cancelled: &AtomicBool) -> Result<Vec<PathBuf>> {
    ensure!(roots.len() <= 32, "Choose at most 32 plugin roots");
    let mut result = BTreeSet::new();
    let mut visited = 0usize;
    let mut queue = std::collections::VecDeque::new();
    for root in roots {
        let root = crate::app::expand_path(root.trim());
        ensure!(
            root.is_absolute() && root.is_dir(),
            "Plugin root is not an absolute existing folder"
        );
        queue.push_back((root, 0));
    }
    while let Some((directory, depth)) = queue.pop_front() {
        ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
        if directory.extension().is_some_and(|e| e == "vst3") {
            result.insert(directory);
            ensure!(
                result.len() <= MAX_CANDIDATES,
                "More than 1024 plugin bundles"
            );
            continue;
        }
        // Never enumerate a Wine prefix, even if the user chose a broad root.
        if directory.join("system.reg").exists() || directory.join("dosdevices").is_dir() {
            continue;
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            visited += 1;
            ensure!(
                visited <= 50_000,
                "Plugin search exceeds 50000 entries; choose a narrower root"
            );
            ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "vst3") && path.is_dir() {
                result.insert(path);
                ensure!(
                    result.len() <= MAX_CANDIDATES,
                    "More than 1024 plugin bundles"
                );
            } else if depth < 8 && entry.file_type()?.is_dir() {
                queue.push_back((path, depth + 1));
            }
        }
    }
    Ok(result.into_iter().collect())
}

fn one_shot(
    settings: &Settings,
    mode: &str,
    request: Option<Value>,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Value> {
    let helper = super::supervisor::executable(settings)?;
    let private = super::supervisor::private_directory()?;
    let socket = private.path().join("control");
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut command = Command::new(helper);
    command
        .arg(mode)
        .arg("--socket")
        .arg(&socket)
        .arg("--parent-pid")
        .arg(std::process::id().to_string());
    super::yabridge::apply_environment(&mut command, &settings.yabridge)?;
    let mut process =
        crate::omr::linux::ManagedProcess::companion(&mut command, Arc::new(Mutex::new(None)))?;
    let started = Instant::now();
    let token = format!(
        "{:x}",
        Sha256::digest(socket.as_os_str().as_encoded_bytes())
    );
    let mut connection = loop {
        ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
        ensure!(
            started.elapsed() < timeout,
            "Plugin helper connection timed out"
        );
        ensure!(process.poll()?, "Plugin helper exited");
        match listener.accept() {
            Ok((stream, _)) => break Connection::new(stream, process.id())?,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    connection.queue(&super::supervisor::envelope(
        json!({"type":"hello"}),
        &token,
        0,
        0,
    ))?;
    let mut ready = false;
    loop {
        ensure!(!cancelled.load(Ordering::Relaxed), "Plugin scan cancelled");
        ensure!(
            started.elapsed() < timeout,
            "Plugin helper timed out; quarantined until retry or module/runtime change"
        );
        ensure!(process.poll()?, "Plugin helper crashed/exited");
        connection.flush()?;
        if let Some(reply) = connection.receive()? {
            ensure!(
                reply["version"] == VERSION
                    && reply["session"] == token
                    && reply["epoch"] == 0
                    && reply["request_id"] == if ready { 1 } else { 0 },
                "Plugin helper protocol/session/request mismatch; rebuild the v2 helper"
            );
            if reply["type"] == "error" || reply["type"] == "operation_error" {
                bail!(
                    "Plugin helper: {}",
                    reply["message"]
                        .as_str()
                        .unwrap_or("operation failed")
                        .chars()
                        .take(2048)
                        .collect::<String>()
                );
            }
            if !ready {
                ensure!(reply["type"] == "hello", "Expected helper hello");
                let capabilities = reply["capabilities"]
                    .as_array()
                    .context("Missing helper capabilities")?;
                ensure!(
                    capabilities.len() <= 32
                        && capabilities
                            .iter()
                            .all(|c| c.as_str().is_some_and(|s| s.len() <= 128)),
                    "Invalid capabilities"
                );
                for required in ["soundfont", "jack"] {
                    ensure!(
                        capabilities.iter().any(|c| c == required),
                        "Helper lacks {required}"
                    );
                }
                if let Some(request) = &request {
                    ensure!(
                        capabilities.iter().any(|c| c == "vst3"),
                        "Helper was built without VST3. Rebuild with the pinned VST3 SDK"
                    );
                    connection.queue(&super::supervisor::envelope(
                        request.clone(),
                        &token,
                        1,
                        0,
                    ))?;
                    ready = true;
                } else {
                    let _ = connection.queue(&super::supervisor::envelope(
                        json!({"type":"shutdown"}),
                        &token,
                        1,
                        0,
                    ));
                    let _ = connection.flush();
                    return Ok(reply);
                }
            } else {
                let _ = connection.queue(&super::supervisor::envelope(
                    json!({"type":"shutdown"}),
                    &token,
                    2,
                    0,
                ));
                let _ = connection.flush();
                return Ok(reply);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn probe(settings: &Settings, cancelled: &AtomicBool) -> Result<Vec<String>> {
    let result = one_shot(settings, "probe", None, cancelled, Duration::from_secs(10))?;
    Ok(serde_json::from_value(result["capabilities"].clone())?)
}

pub fn scan_module(
    settings: &Settings,
    path: &Path,
    hash: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<Class>> {
    super::yabridge::check_bundle(path, &settings.yabridge)?;
    ensure!(inspect_bundle(path)? == hash, "Module changed before scan");
    let timeout = if super::yabridge::is_wrapper(path, &settings.yabridge) {
        120
    } else {
        30
    };
    let result = one_shot(
        settings,
        "scan",
        Some(json!({"type":"scan","path":path,"module_sha256":hash})),
        cancelled,
        Duration::from_secs(timeout),
    )?;
    ensure!(
        result["type"] == "scan_result"
            && result["path"] == path.to_str().unwrap()
            && result["module_sha256"] == hash,
        "Scan result identity mismatch"
    );
    let classes: Vec<Class> = serde_json::from_value(result["classes"].clone())?;
    ensure!(
        !classes.is_empty() && classes.len() <= 128,
        "No supported instrument classes or too many classes"
    );
    let mut ids = BTreeSet::new();
    for class in &classes {
        class.validate()?;
        ensure!(ids.insert(&class.class_id), "Duplicate VST3 class");
    }
    ensure!(inspect_bundle(path)? == hash, "Module changed during scan");
    Ok(classes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_probe_scan_multiple_classes_and_quarantine_are_persistent() {
        let root = tempfile::tempdir().unwrap();
        let (_, settings) = super::super::supervisor::tests::fake(root.path(), "normal");
        let cancel = AtomicBool::new(false);
        assert!(probe(&settings, &cancel).unwrap().contains(&"vst3".into()));
        let bundle = super::super::plugins::fixture(root.path(), "Synth été.v1");
        let mut catalogue = Catalogue::default();
        catalogue
            .scan(&settings, std::slice::from_ref(&bundle), &cancel, false)
            .unwrap();
        assert_eq!(catalogue.entries[0].classes.len(), 2);
        let path = root.path().join("catalogue.json");
        catalogue.save(&path).unwrap();
        assert_eq!(Catalogue::load(&path).unwrap().entries.len(), 1);
        let (_, crashing) = super::super::supervisor::tests::fake(root.path(), "scan_crash");
        catalogue
            .scan(&crashing, std::slice::from_ref(&bundle), &cancel, true)
            .unwrap();
        assert!(catalogue.entries[0].failure.is_some());
        let before = fs::read_to_string(root.path().join("commands")).unwrap();
        catalogue
            .scan(&crashing, std::slice::from_ref(&bundle), &cancel, false)
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("commands")).unwrap(),
            before
        );
        catalogue.save(&path).unwrap();
        assert!(Catalogue::load(&path).unwrap().entries[0].failure.is_some());
    }
    #[test]
    fn cancellation_timeouts_duplicate_classes_and_symlink_loops_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        let bundle = super::super::plugins::fixture(root.path(), "Synth");
        std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();
        let nested = bundle.join("Contents/nested.vst3");
        fs::create_dir(&nested).unwrap();
        let prefix = root.path().join("wine");
        fs::create_dir(&prefix).unwrap();
        fs::write(prefix.join("system.reg"), b"generated").unwrap();
        super::super::plugins::fixture(&prefix, "NotScanned");
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            enumerate(&[root.path().display().to_string()], &cancel).unwrap(),
            vec![bundle.clone()]
        );
        let (_, settings) = super::super::supervisor::tests::fake(root.path(), "duplicate");
        let hash = inspect_bundle(&bundle).unwrap();
        assert!(scan_module(&settings, &bundle, &hash, &cancel)
            .unwrap_err()
            .to_string()
            .contains("Duplicate"));
        let (_, settings) = super::super::supervisor::tests::fake(root.path(), "scan_hang");
        let stop = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stop.store(true, Ordering::Relaxed);
        });
        let at = Instant::now();
        assert!(scan_module(&settings, &bundle, &hash, &cancel)
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert!(at.elapsed() < Duration::from_secs(2));
        thread.join().unwrap();
        cancel.store(false, Ordering::Relaxed);
        let result = one_shot(
            &settings,
            "scan",
            Some(json!({"type":"scan","path":bundle,"module_sha256":hash})),
            &cancel,
            Duration::from_millis(150),
        );
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
