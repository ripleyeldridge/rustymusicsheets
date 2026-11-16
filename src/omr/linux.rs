use super::{RecognitionArtifacts, RecognitionUpdate, Settings};
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use walkdir::WalkDir;

const LOG_LIMIT: u64 = 2 * 1024 * 1024;
const FILE_LIMIT: u64 = 64 * 1024 * 1024;
const TREE_LIMIT: u64 = 512 * 1024 * 1024;
const ENTRY_LIMIT: usize = 4096;
const RECOGNITION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);
type ProcessSlot = Arc<Mutex<Option<u32>>>;

/// Drop cancels and kills the active process group without joining the worker.
/// The worker reaps the child before releasing private staging.
pub struct RecognitionJob {
    cancel: Arc<AtomicBool>,
    updates: Receiver<RecognitionUpdate>,
    process: ProcessSlot,
}

pub(crate) fn kill_group(group: u32) {
    // No shell interpolation and no extra dependency. SIGKILL avoids hanging
    // on launchers/JVMs that ignore TERM. Child::kill is also used as fallback.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--"])
        .arg(format!("-{group}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl RecognitionJob {
    pub fn start(
        source: PathBuf,
        source_hash: String,
        output_base: PathBuf,
        settings: Settings,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        validate_hash(&source_hash)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let process = Arc::new(Mutex::new(None));
        let worker_process = process.clone();
        let (sender, updates) = mpsc::channel();
        thread::Builder::new()
            .name("audiveris".into())
            .spawn(move || {
                let progress = |text: &str| {
                    let _ = sender.send(RecognitionUpdate::Progress(text.into()));
                    repaint();
                };
                let result = recognize(
                    &source,
                    &source_hash,
                    &output_base,
                    &settings,
                    &worker_cancel,
                    worker_process,
                    progress,
                )
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(RecognitionUpdate::Finished(result));
                repaint();
            })
            .context("Cannot start recognition worker")?;
        Ok(Self {
            cancel,
            updates,
            process,
        })
    }

    pub fn poll(&mut self) -> Vec<RecognitionUpdate> {
        self.updates.try_iter().collect()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        // Synchronize with spawning so Drop cannot miss a newly launched JVM.
        // The worker still owns/reaps it; no UI-thread join or channel send.
        if let Some(group) = *self.process.lock().unwrap_or_else(|e| e.into_inner()) {
            kill_group(group);
        }
    }
}

impl Drop for RecognitionJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid source SHA-256 hash");
    }
    Ok(())
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Recognition cancelled");
    }
    Ok(())
}

fn recognize(
    source: &Path,
    hash: &str,
    output_base: &Path,
    settings: &Settings,
    cancel: &AtomicBool,
    process_slot: ProcessSlot,
    progress: impl Fn(&str),
) -> Result<RecognitionArtifacts> {
    progress("Checking source");
    cancelled(cancel)?;
    let extension = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "pdf" | "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp"
    ) {
        bail!("Audiveris requires a PDF or supported raster image");
    }
    if !source.is_file() || fs::metadata(source)?.len() > TREE_LIMIT {
        bail!("Source is not a regular file or exceeds the 512 MiB limit");
    }
    if crate::folder::hash_file(source, cancel)? != hash {
        bail!("Source changed since it was selected");
    }
    let stage = tempfile::tempdir().context("Cannot create private score staging directory")?;
    let staged = stage.path().join(format!("score.{extension}"));
    let mut input = File::open(source)?;
    let mut output = File::create(&staged)?;
    let mut buffer = [0; 64 * 1024];
    let mut copied = 0u64;
    loop {
        cancelled(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > TREE_LIMIT {
            bail!("Source exceeds the 512 MiB limit");
        }
        output.write_all(&buffer[..count])?;
    }
    drop(output);
    if crate::folder::hash_file(&staged, cancel)? != hash
        || crate::folder::hash_file(source, cancel)? != hash
    {
        bail!("Source changed while making its private copy");
    }
    // The supplied base is application data, never the source's directory.
    let root = output_base.join("omr").join(hash.to_ascii_lowercase());
    fs::create_dir_all(&root).context("Cannot create recognition artifact directory")?;
    let root = root.canonicalize()?;
    let run = tempfile::Builder::new().prefix("run-").tempdir_in(root)?;
    fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700))?;
    let exports = run.path().join("exports");
    fs::create_dir(&exports)?;
    progress("Transcribing with Audiveris (results require review)");
    let mut command = Command::new(&settings.audiveris_executable);
    command
        .args(["-batch", "-transcribe", "-export", "-save", "-output"])
        .arg(&exports)
        .arg("--")
        .arg(&staged)
        .current_dir(run.path());
    let mut slot = process_slot.lock().unwrap_or_else(|e| e.into_inner());
    cancelled(cancel)?;
    let mut process = ManagedProcess::spawn(&mut command, RECOGNITION_TIMEOUT)?;
    *slot = Some(process.group);
    process.registration = Some(process_slot.clone());
    drop(slot);
    loop {
        cancelled(cancel)?;
        inspect_tree(run.path())?;
        if !process.poll()? {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    cancelled(cancel)?;
    if crate::folder::hash_file(source, cancel)? != hash
        || crate::folder::hash_file(&staged, cancel)? != hash
    {
        bail!("Source changed during recognition; results discarded");
    }
    progress("Checking exported movements");
    process.persist_logs(run.path())?;
    inspect_tree(run.path())?;
    let movements = inspect_tree(&exports)?;
    if movements.is_empty() {
        bail!("Audiveris produced no MusicXML movements");
    }
    if movements
        .iter()
        .any(|path| fs::metadata(path).map_or(true, |m| m.len() == 0))
    {
        bail!("Audiveris exported an empty or unreadable MusicXML movement");
    }
    cancelled(cancel)?;
    // Keep the unique private output only after the entire job succeeds.
    let _retained = run.keep();
    Ok(RecognitionArtifacts {
        source_hash: hash.into(),
        movements,
    })
}

/// Refuse symlinks, special files, deep trees, and oversized output. Never follow
/// helper-created links while searching for MusicXML.
fn inspect_tree(root: &Path) -> Result<Vec<PathBuf>> {
    let mut total = 0u64;
    let mut movements = Vec::new();
    for (index, entry) in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        if index >= ENTRY_LIMIT {
            bail!("Helper output exceeds the file-count limit");
        }
        let entry = entry?;
        if entry.depth() > 16 {
            bail!("Helper output exceeds the directory-depth limit");
        }
        let kind = entry.file_type();
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            bail!("Helper output contains a symlink or special file");
        }
        if kind.is_file() {
            let size = entry.metadata()?.len();
            total = total.saturating_add(size);
            if size > FILE_LIMIT || total > TREE_LIMIT {
                bail!("Helper output exceeds the size limit");
            }
            let extension = entry
                .path()
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if ["xml", "musicxml", "mxl"]
                .iter()
                .any(|ext| extension.eq_ignore_ascii_case(ext))
            {
                movements.push(entry.path().to_owned());
            }
        }
    }
    movements.sort();
    Ok(movements)
}

/// A child in its own process group; logs are files, never undrained pipes.
pub(crate) struct ManagedProcess {
    child: Option<Child>,
    group: u32,
    stdout: File,
    stderr: File,
    started: Instant,
    timeout: Duration,
    _logs: TempDir,
    registration: Option<ProcessSlot>,
}

impl ManagedProcess {
    fn persist_logs(&mut self, root: &Path) -> Result<()> {
        for (file, name) in [
            (&mut self.stdout, "engine-stdout.log"),
            (&mut self.stderr, "engine-stderr.log"),
        ] {
            if file.metadata()?.len() > LOG_LIMIT {
                bail!("Helper log output exceeded the 2 MiB limit");
            }
            file.seek(SeekFrom::Start(0))?;
            let mut output = File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(root.join(name))?;
            std::io::copy(&mut file.take(LOG_LIMIT), &mut output)?;
        }
        Ok(())
    }
    fn spawn(command: &mut Command, timeout: Duration) -> Result<Self> {
        Self::spawn_inner(command, timeout, false)
    }

    pub(crate) fn interactive(command: &mut Command, slot: ProcessSlot) -> Result<Self> {
        let mut process = Self::spawn_inner(command, PLAYBACK_TIMEOUT, true)?;
        process.registration = Some(slot);
        Ok(process)
    }

    pub(crate) fn id(&self) -> u32 {
        self.group
    }

    pub(crate) fn input(&mut self) -> Result<std::process::ChildStdin> {
        self.child
            .as_mut()
            .and_then(|child| child.stdin.take())
            .context("Missing synth command pipe")
    }

    pub(crate) fn output(&mut self) -> Result<String> {
        read_log(&mut self.stdout)
    }

    fn spawn_inner(command: &mut Command, timeout: Duration, interactive: bool) -> Result<Self> {
        let logs = tempfile::tempdir()?;
        let stdout_path = logs.path().join("stdout");
        let stderr_path = logs.path().join("stderr");
        let stdout_writer = File::create(&stdout_path)?;
        let stderr_writer = File::create(&stderr_path)?;
        // Independent file descriptions: reading logs must not move the child's
        // write position (try_clone/dup would share that position).
        let stdout = File::open(stdout_path)?;
        let stderr = File::open(stderr_path)?;
        let child = command
            .stdin(if interactive {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(stdout_writer)
            .stderr(stderr_writer)
            .process_group(0)
            .spawn()
            .with_context(|| {
                format!(
                    "Cannot start helper {}",
                    command.get_program().to_string_lossy()
                )
            })?;
        let group = child.id();
        Ok(Self {
            child: Some(child),
            group,
            stdout,
            stderr,
            started: Instant::now(),
            timeout,
            _logs: logs,
            registration: None,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<bool> {
        if self.child.is_none() {
            return Ok(false);
        }
        let result = self.check();
        if !matches!(result, Ok(true)) {
            self.stop();
        }
        result
    }

    fn check(&mut self) -> Result<bool> {
        if self.stdout.metadata()?.len() > LOG_LIMIT || self.stderr.metadata()?.len() > LOG_LIMIT {
            bail!("Helper log output exceeded the 2 MiB limit");
        }
        if self.started.elapsed() >= self.timeout {
            bail!("Helper timed out");
        }
        self.check_audio_logs()?;
        let status = self
            .child
            .as_mut()
            .context("Helper is not running")?
            .try_wait()?;
        match status {
            None => Ok(true),
            Some(status) => {
                // Also terminate descendants left behind by an exited launcher.
                self.kill_group();
                self.check_status(status)?;
                Ok(false)
            }
        }
    }

    fn check_status(&mut self, status: ExitStatus) -> Result<()> {
        let stderr = read_log(&mut self.stderr)?;
        let stdout = read_log(&mut self.stdout)?;
        if !status.success() {
            bail!(
                "Helper failed ({status}): {}",
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            );
        }
        Ok(())
    }

    fn check_audio_logs(&mut self) -> Result<()> {
        // Some FluidSynth builds continue running or exit successfully even when
        // audio initialization failed. Do not mistake this for playback.
        let stderr = read_log(&mut self.stderr)?;
        let stdout = read_log(&mut self.stdout)?;
        let logs = format!("{stderr}\n{stdout}").to_ascii_lowercase();
        if [
            "failed to create the audio driver",
            "failed to create audio driver",
            "failed to open audio device",
            "audio device open failed",
            "no soundfonts have been loaded",
            "failed to load the soundfont",
            "fluidsynth: error:",
        ]
        .iter()
        .any(|message| logs.contains(message))
        {
            bail!(
                "Helper reported an audio or SoundFont error: {}",
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            );
        }
        Ok(())
    }

    fn kill_group(&self) {
        kill_group(self.group);
    }

    fn stop(&mut self) {
        let registration = self.registration.take();
        let mut slot = registration
            .as_ref()
            .map(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(slot) = slot.as_mut() {
            **slot = None;
        }
        if self.child.is_some() {
            self.kill_group();
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_log(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(LOG_LIMIT).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The caller must poll regularly (e.g. every repaint) to detect audio failures
/// and enforce runtime/log limits, and stop on score replacement or shutdown.
pub struct Playback {
    process: ManagedProcess,
    _private_files: Option<TempDir>,
}

impl Playback {
    pub fn start(settings: &Settings, midi: &[u8]) -> Result<Self> {
        if midi.len() < 14 || !midi.starts_with(b"MThd") || midi.len() as u64 > FILE_LIMIT {
            bail!("Invalid or oversized MIDI data");
        }
        let soundfont = Path::new(&settings.soundfont)
            .canonicalize()
            .context("Select an existing FluidSynth SoundFont")?;
        if !soundfont.is_file() {
            bail!("SoundFont is not a regular file");
        }
        let mut header = [0u8; 12];
        File::open(&soundfont)?
            .read_exact(&mut header)
            .context("SoundFont is too short")?;
        if &header[..4] != b"RIFF" || &header[8..] != b"sfbk" {
            bail!("SoundFont is not a RIFF SF2/SF3 bank");
        }
        let private_files = tempfile::tempdir()?;
        let path = private_files.path().join("playback.mid");
        fs::write(&path, midi)?;
        let process = ManagedProcess::spawn(
            Command::new(&settings.fluidsynth_executable)
                .arg("-ni")
                .arg(&soundfont)
                .arg(&path),
            PLAYBACK_TIMEOUT,
        )?;
        Ok(Self {
            process,
            _private_files: Some(private_files),
        })
    }

    pub fn poll(&mut self) -> Result<bool> {
        let result = self.process.poll();
        if !matches!(result, Ok(true)) {
            self._private_files.take();
        }
        result
    }

    pub fn stop(&mut self) {
        self.process.stop();
        self._private_files.take();
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
