use super::{
    ipc::{Connection, VERSION},
    plan::{Plan, MAX_PLAN_BYTES},
    Backend, Diagnostics,
};
use crate::{
    notation::PageMap,
    omr::linux::ManagedProcess,
    playback::{Command, Snapshot, State},
};
use anyhow::{bail, ensure, Context, Result};
use crossbeam_channel::Receiver;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Session {
    pub audio: super::Settings,
    pub settings: crate::omr::Settings,
    pub plan: Plan,
    pub pages: Option<PageMap>,
    pub autoplay: bool,
    pub master_volume: u8,
    pub tempo_scale: f64,
}

#[derive(Deserialize)]
struct Reply {
    version: u32,
    session: String,
    request_id: u64,
    epoch: u64,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Payload {
    Hello { capabilities: Vec<String> },
    Status(Status),
    Error { message: String },
}

#[derive(Deserialize)]
struct Status {
    state: String,
    position: f64,
    rendered_position: f64,
    duration: f64,
    score_tick: u64,
    tempo_scale: f64,
    sample_rate: u32,
    latency_frames: u64,
    xruns: u64,
}

fn envelope(mut payload: Value, session: &str, request_id: u64, epoch: u64) -> Value {
    payload["version"] = json!(VERSION);
    payload["session"] = json!(session);
    payload["request_id"] = json!(request_id);
    payload["epoch"] = json!(epoch);
    payload
}

fn private_directory() -> Result<tempfile::TempDir> {
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    ensure!(
        root.is_absolute(),
        "Audio runtime directory must be absolute"
    );
    // The new private directory, not a shared /tmp root, is the security boundary.
    let private = tempfile::Builder::new()
        .prefix("rms-audio-")
        .tempdir_in(root)?;
    fs::set_permissions(private.path(), fs::Permissions::from_mode(0o700))?;
    Ok(private)
}

fn executable(settings: &super::Settings) -> Result<PathBuf> {
    let path = if settings.host_executable.trim().is_empty() {
        PathBuf::from("/usr/libexec/rustymusicsheets/rms-audio-host")
    } else {
        crate::app::expand_path(settings.host_executable.trim())
    };
    ensure!(
        path.is_absolute(),
        "Choose an absolute audio-host executable path"
    );
    let path = path.canonicalize().context("Audio host is not installed. Build scripts/build-audio-host.py and choose target/audio-host/rms-audio-host, or select legacy playback")?;
    let metadata = path.metadata()?;
    ensure!(
        metadata.is_file() && metadata.mode() & 0o111 != 0,
        "Audio host is not an executable file"
    );
    Ok(path)
}

fn soundfont(path: &str) -> Result<PathBuf> {
    let path = crate::app::expand_path(path.trim())
        .canonicalize()
        .context("Choose an existing SoundFont")?;
    let mut file = File::open(&path)?;
    ensure!(
        file.metadata()?.is_file(),
        "SoundFont is not a regular file"
    );
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    ensure!(
        &header[..4] == b"RIFF" && &header[8..] == b"sfbk",
        "Invalid SF2/SF3 SoundFont"
    );
    Ok(path)
}

pub(crate) fn run(
    session: Session,
    commands: Receiver<Command>,
    view: &Arc<Mutex<Snapshot>>,
    cancelled: &AtomicBool,
    slot: Arc<Mutex<Option<u32>>>,
) -> Result<()> {
    let helper = executable(&session.audio)?;
    let bank = soundfont(&session.settings.soundfont)?;
    let private = private_directory()?;
    let socket = private.path().join("control");
    let listener = UnixListener::bind(&socket)
        .context("Creating private audio socket (check runtime path length)")?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let plan_path = private.path().join("plan.json");
    let mut plan_file = tempfile::NamedTempFile::new_in(private.path())?;
    {
        let mut writer = LimitedWriter {
            writer: plan_file.as_file_mut(),
            remaining: MAX_PLAN_BYTES,
        };
        serde_json::to_writer(&mut writer, &session.plan)?;
    }
    plan_file.as_file().sync_all()?;
    let mut digest = Sha256::new();
    let mut reader = File::open(plan_file.path())?;
    let plan_bytes = std::io::copy(&mut reader, &mut digest)?;
    let plan_sha256 = format!("{:x}", digest.finalize());
    plan_file.persist(&plan_path)?;
    let plan_name = plan_path
        .to_str()
        .context("Audio plan path must be representable as UTF-8")?;
    let bank_name = bank
        .to_str()
        .context("Resolved SoundFont path must be representable as UTF-8")?;
    let token = format!(
        "{:x}",
        Sha256::digest(socket.as_os_str().as_encoded_bytes())
    );
    let mut command = if session.audio.backend == Backend::PipeWire {
        let mut command = ProcessCommand::new("/usr/bin/pw-jack");
        command.arg(&helper);
        command
    } else {
        ProcessCommand::new(&helper)
    };
    command
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .arg("--parent-pid")
        .arg(std::process::id().to_string());
    command.env_remove("WINEPREFIX");
    let mut registration = slot.lock().unwrap_or_else(|e| e.into_inner());
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut process = ManagedProcess::companion(&mut command, slot.clone())?;
    *registration = Some(process.id());
    drop(registration);
    let started = Instant::now();
    let mut connection = loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        ensure!(process.poll()?, "Audio host exited before connecting");
        match listener.accept() {
            Ok((stream, _)) => break Connection::new(stream, process.id())?,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            started.elapsed() < CONNECT_TIMEOUT,
            "Audio host did not connect"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    connection.queue(&envelope(json!({"type":"hello"}), &token, 0, 0))?;
    let mut ready = false;
    let mut loaded = false;
    let mut request_id = 0u64;
    let mut epoch = 0u64;
    let mut applied = 0u64;
    let mut applied_epoch = 0u64;
    let mut heartbeat = Instant::now();
    let mut outstanding: VecDeque<(u64, u64, Instant)> = VecDeque::new();
    let expected_duration = session.plan.seconds_at(session.plan.duration_ticks);
    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = connection.queue(&envelope(
                json!({"type":"shutdown"}),
                &token,
                request_id + 1,
                epoch,
            ));
            let _ = connection.flush();
            return Ok(());
        }
        ensure!(process.poll()?, "Audio host exited unexpectedly");
        connection.flush()?;
        for _ in 0..64 {
            let Some(value) = connection.receive()? else {
                break;
            };
            let reply: Reply =
                serde_json::from_value(value).context("Invalid audio-host response")?;
            ensure!(
                reply.version == VERSION && reply.session == token,
                "Audio-host protocol/session mismatch"
            );
            ensure!(
                reply.request_id <= request_id && reply.epoch <= epoch,
                "Audio host acknowledged an unknown command"
            );
            match reply.payload {
                Payload::Hello { capabilities } => {
                    ensure!(
                        !ready && reply.request_id == 0 && reply.epoch == 0,
                        "Unexpected audio-host handshake"
                    );
                    ensure!(
                        capabilities.len() <= 32 && capabilities.iter().all(|cap| cap.len() <= 128),
                        "Invalid audio-host capabilities"
                    );
                    ensure!(
                        capabilities.iter().any(|cap| cap == "soundfont")
                            && capabilities.iter().any(|cap| cap == "jack"),
                        "Audio host lacks required SoundFont/JACK capabilities"
                    );
                    ready = true;
                    request_id = 1;
                    connection.queue(&envelope(json!({
                        "type":"load", "plan_path":plan_name, "plan_sha256":plan_sha256,
                        "plan_bytes":plan_bytes, "soundfont":bank_name, "autoplay":session.autoplay,
                        "master_volume":session.master_volume, "tempo_scale":session.tempo_scale,
                    }), &token, request_id, epoch))?;
                }
                Payload::Error { message } => bail!(
                    "Audio host: {}",
                    message.chars().take(2048).collect::<String>()
                ),
                Payload::Status(status) => {
                    ensure!(
                        ready && reply.request_id >= 1,
                        "Status before audio-host load"
                    );
                    ensure!(
                        reply.request_id >= applied,
                        "Audio host command acknowledgement moved backwards"
                    );
                    let expected_epoch = if reply.request_id == applied {
                        applied_epoch
                    } else if reply.request_id == 1 {
                        0
                    } else {
                        outstanding
                            .iter()
                            .find(|(id, _, _)| *id == reply.request_id)
                            .context("Audio host acknowledged an untracked request")?
                            .1
                    };
                    ensure!(
                        reply.epoch == expected_epoch,
                        "Audio host acknowledged the wrong transport epoch"
                    );
                    applied = reply.request_id;
                    applied_epoch = reply.epoch;
                    while outstanding.front().is_some_and(|(id, _, _)| *id <= applied) {
                        outstanding.pop_front();
                    }
                    heartbeat = Instant::now();
                    loaded = true;
                    if reply.epoch != epoch {
                        continue;
                    }
                    let state =
                        validate_status(&status, expected_duration, session.plan.duration_ticks)?;
                    let source_page = session
                        .pages
                        .as_ref()
                        .and_then(|map| map.page_at(status.score_tick, 0));
                    *view.lock().unwrap_or_else(|e| e.into_inner()) = Snapshot {
                        state,
                        position: status.position / status.tempo_scale,
                        duration: status.duration / status.tempo_scale,
                        error: None,
                        source_page,
                        score_tick: status.score_tick,
                        audio: Some(Diagnostics {
                            rendered_position: status.rendered_position / status.tempo_scale,
                            sample_rate: status.sample_rate,
                            latency_frames: status.latency_frames,
                            xruns: status.xruns,
                        }),
                    };
                }
            }
        }
        if loaded {
            ensure!(
                heartbeat.elapsed() < HEARTBEAT_TIMEOUT,
                "Audio host heartbeat stopped; playback was terminated"
            );
            ensure!(
                outstanding
                    .front()
                    .is_none_or(|(_, _, at)| at.elapsed() < COMMAND_TIMEOUT),
                "Audio host did not apply a transport command"
            );
            for _ in 0..128 {
                if outstanding.len() >= 128 {
                    break;
                }
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                let (payload, advances_epoch) = match command {
                    Command::Play => (json!({"type":"play"}), true),
                    Command::Pause => (json!({"type":"pause"}), true),
                    Command::Stop => (json!({"type":"stop"}), true),
                    Command::Seek(delta) => (json!({"type":"seek","delta":delta}), true),
                    Command::Tempo(scale) => (json!({"type":"tempo","scale":scale}), true),
                    Command::MasterVolume(percent) => {
                        (json!({"type":"master_volume","percent":percent}), false)
                    }
                    Command::Mix(part_id, enabled, volume, program) => {
                        ensure!(
                            session.plan.parts.iter().any(|part| part.id == part_id),
                            "Unknown audio part"
                        );
                        (
                            json!({"type":"mix","part_id":part_id,"enabled":enabled,"volume":volume,"program":program}),
                            false,
                        )
                    }
                };
                request_id += 1;
                if advances_epoch {
                    epoch += 1;
                }
                connection.queue(&envelope(payload, &token, request_id, epoch))?;
                outstanding.push_back((request_id, epoch, Instant::now()));
            }
        } else {
            ensure!(
                started.elapsed() < if ready { LOAD_TIMEOUT } else { CONNECT_TIMEOUT },
                "Audio host handshake or SoundFont load timed out"
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn validate_status(status: &Status, duration: f64, max_tick: u64) -> Result<State> {
    ensure!(
        status.position.is_finite()
            && status.rendered_position.is_finite()
            && status.duration.is_finite()
            && status.tempo_scale.is_finite()
            && (0.1..=4.0).contains(&status.tempo_scale)
            && (status.duration - duration).abs() < 0.001
            && (0.0..=duration + 0.001).contains(&status.position)
            && (0.0..=duration + 0.001).contains(&status.rendered_position)
            && status.score_tick <= max_tick
            && (8_000..=384_000).contains(&status.sample_rate)
            && status.latency_frames <= u64::from(status.sample_rate) * 10,
        "Invalid audio-host timing/latency status"
    );
    Ok(match status.state.as_str() {
        "playing" => State::Playing,
        "paused" => State::Paused,
        "stopped" => State::Stopped,
        "finished" => State::Finished,
        _ => bail!("Unknown audio-host transport state"),
    })
}

struct LimitedWriter<'a> {
    writer: &'a mut File,
    remaining: usize,
}
impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("Audio plan exceeds 64 MiB"));
        }
        let count = self.writer.write(bytes)?;
        self.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests;
