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
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    Hello {
        capabilities: Vec<String>,
    },
    Status(Status),
    Error {
        message: String,
    },
    Ack {
        operation: String,
        part_id: Option<String>,
    },
    OperationError {
        operation: String,
        part_id: Option<String>,
        message: String,
    },
    ParametersResult {
        part_id: String,
        total: u32,
        offset: u32,
        parameters: Vec<super::plugins::Parameter>,
    },
    CheckpointResult {
        parts: Vec<super::plugins::CheckpointPart>,
    },
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
    dirty_parts: Vec<String>,
}

pub(super) fn envelope(mut payload: Value, session: &str, request_id: u64, epoch: u64) -> Value {
    payload["version"] = json!(VERSION);
    payload["session"] = json!(session);
    payload["request_id"] = json!(request_id);
    payload["epoch"] = json!(epoch);
    payload
}

pub(super) fn private_directory() -> Result<tempfile::TempDir> {
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

pub(super) fn executable(settings: &super::Settings) -> Result<PathBuf> {
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
    let sources: BTreeMap<_, _> = session
        .plan
        .parts
        .iter()
        .map(|p| (p.id.clone(), p.source.clone()))
        .collect();
    let has_plugins = sources.values().any(|s| s.is_plugin());
    for source in sources.values() {
        if let super::plugins::Source::Vst3 {
            path,
            module_sha256,
            ..
        } = source
        {
            super::yabridge::check_bundle(path, &session.audio.yabridge)?;
            ensure!(
                super::plugins::inspect_bundle(path)? == *module_sha256,
                "Plugin module changed; scan and select the exact class again"
            );
        }
    }
    super::checkpoint::verify_restore(&sources)?;
    let bank = if sources.values().any(|s| !s.is_plugin()) {
        Some(soundfont(&session.settings.soundfont)?)
    } else {
        None
    };
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
        .as_ref()
        .map(|b| {
            b.to_str()
                .context("Resolved SoundFont path must be representable as UTF-8")
        })
        .transpose()?;
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
    super::yabridge::apply_environment(&mut command, &session.audio.yabridge)?;
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
    let mut pending: BTreeMap<u64, Value> = BTreeMap::new();
    let mut checkpoint: Option<(u64, tempfile::TempDir, PathBuf)> = None;
    let mut plugin_view = super::plugins::View::default();
    let mut transport_state = State::Starting;
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
            let operation_reply = !matches!(
                &reply.payload,
                Payload::Hello { .. } | Payload::Status(_) | Payload::Error { .. }
            );
            if operation_reply {
                let expected = outstanding
                    .iter()
                    .find(|(id, _, _)| *id == reply.request_id)
                    .context("Audio host returned an untracked operation")?;
                ensure!(
                    reply.request_id >= applied && reply.epoch == expected.1,
                    "Audio host acknowledged the wrong transport epoch"
                );
                let request = pending
                    .get(&reply.request_id)
                    .context("Missing operation request")?;
                match &reply.payload {
                    Payload::Ack { operation, part_id }
                    | Payload::OperationError {
                        operation, part_id, ..
                    } => {
                        ensure!(
                            request["type"] == *operation
                                && request.get("part_id").and_then(Value::as_str)
                                    == part_id.as_deref(),
                            "Operation reply identity mismatch"
                        );
                        if matches!(&reply.payload, Payload::Ack { .. }) {
                            ensure!(
                                request["type"] != "parameters" && request["type"] != "checkpoint",
                                "Expected operation result, not ack"
                            );
                            if request["type"] == "parameter" {
                                if let Some((_, _, parameters)) = request["part_id"]
                                    .as_str()
                                    .and_then(|id| plugin_view.parameters.get_mut(id))
                                {
                                    if let Some(parameter) = parameters.iter_mut().find(|p| {
                                        Some(u64::from(p.id)) == request["param_id"].as_u64()
                                    }) {
                                        parameter.value = request["value"]
                                            .as_f64()
                                            .context("Invalid acknowledged parameter")?;
                                    }
                                }
                            }
                        }
                    }
                    Payload::ParametersResult {
                        part_id,
                        total,
                        offset,
                        parameters,
                    } => {
                        ensure!(
                            request["type"] == "parameters"
                                && request["part_id"] == *part_id
                                && request["offset"] == *offset,
                            "Parameter reply identity mismatch"
                        );
                        ensure!(
                            *total <= 4096
                                && *offset <= *total
                                && parameters.len() <= 128
                                && *offset as usize + parameters.len() <= *total as usize,
                            "Invalid parameter page"
                        );
                        let mut ids = BTreeSet::new();
                        for parameter in parameters {
                            parameter.validate()?;
                            ensure!(ids.insert(parameter.id), "Duplicate parameter ID");
                        }
                    }
                    Payload::CheckpointResult { .. } => ensure!(
                        request["type"] == "checkpoint",
                        "Unexpected checkpoint reply"
                    ),
                    _ => {}
                }
                applied = reply.request_id;
                applied_epoch = reply.epoch;
                while outstanding.front().is_some_and(|(id, _, _)| *id <= applied) {
                    outstanding.pop_front();
                }
                pending.retain(|id, _| *id > applied);
                heartbeat = Instant::now();
            }
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
                    if has_plugins {
                        for required in ["vst3", "plugin_state", "parameters", "presets"] {
                            ensure!(
                                capabilities.iter().any(|c| c == required),
                                "Audio host lacks {required}; rebuild with the pinned VST3 SDK"
                            );
                        }
                    }
                    plugin_view.capabilities = capabilities;
                    ready = true;
                    request_id = 1;
                    pending.insert(1, json!({"type":"load"}));
                    outstanding.push_back((1, 0, Instant::now()));
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
                Payload::Ack { .. } => {}
                Payload::OperationError { message, .. } => {
                    plugin_view.operation_error = Some(message.chars().take(2048).collect());
                    if checkpoint
                        .as_ref()
                        .is_some_and(|(id, _, _)| *id == reply.request_id)
                    {
                        checkpoint = None;
                    }
                }
                Payload::ParametersResult {
                    part_id,
                    total,
                    offset,
                    parameters,
                } => {
                    plugin_view
                        .parameters
                        .insert(part_id, (total, offset, parameters));
                    plugin_view.operation_error = None;
                }
                Payload::CheckpointResult { mut parts } => {
                    let (id, staging, destination) =
                        checkpoint.take().context("Unrequested checkpoint")?;
                    ensure!(id == reply.request_id, "Checkpoint request mismatch");
                    match super::checkpoint::persist(
                        staging.path(),
                        &destination,
                        &sources,
                        &mut parts,
                    ) {
                        Ok(()) => {
                            plugin_view.checkpoint = Some((id, parts));
                            plugin_view.operation_error = None;
                        }
                        Err(error) => {
                            plugin_view.operation_error = Some(format!(
                                "Checkpoint rejected; previous saved state retained: {error:#}"
                            ))
                        }
                    }
                }
                Payload::Status(status) => {
                    ensure!(
                        ready && reply.request_id >= 1,
                        "Status before audio-host load"
                    );
                    ensure!(
                        reply.request_id >= applied,
                        "Audio host command acknowledgement moved backwards"
                    );
                    ensure!(
                        !pending.iter().any(|(id, request)| *id <= reply.request_id
                            && (request["type"] == "parameters"
                                || request["type"] == "checkpoint")),
                        "Audio host acknowledged an operation before its required result"
                    );
                    if checkpoint.is_some() || pending.values().any(|p| p["type"] == "preset_load")
                    {
                        ensure!(
                            status.state != "playing",
                            "Plugin state operation resumed processing unexpectedly"
                        );
                    }
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
                    pending.retain(|id, _| *id > applied);
                    heartbeat = Instant::now();
                    loaded = true;
                    if reply.epoch != epoch {
                        continue;
                    }
                    let state =
                        validate_status(&status, expected_duration, session.plan.duration_ticks)?;
                    ensure!(
                        status.dirty_parts.len() <= 32
                            && status
                                .dirty_parts
                                .iter()
                                .all(|id| sources.get(id).is_some_and(|s| s.is_plugin()))
                            && status.dirty_parts.iter().collect::<BTreeSet<_>>().len()
                                == status.dirty_parts.len(),
                        "Invalid dirty plugin parts"
                    );
                    plugin_view.dirty_parts = status.dirty_parts;
                    transport_state = state;
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
                        plugins: plugin_view.clone(),
                    };
                }
            }
        }
        if loaded {
            ensure!(
                heartbeat.elapsed()
                    < if checkpoint.is_some()
                        || pending.values().any(|p| p["type"] == "preset_load")
                    {
                        Duration::from_secs(60)
                    } else {
                        HEARTBEAT_TIMEOUT
                    },
                "Audio host heartbeat stopped; playback was terminated"
            );
            ensure!(
                outstanding.front().is_none_or(|(id, _, at)| at.elapsed()
                    < if pending
                        .get(id)
                        .is_some_and(|p| p["type"] == "checkpoint" || p["type"] == "preset_load")
                    {
                        Duration::from_secs(60)
                    } else {
                        COMMAND_TIMEOUT
                    }),
                "Audio host did not apply a transport command"
            );
            for _ in 0..128 {
                if outstanding.len() >= 128 {
                    break;
                }
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                if checkpoint.is_some() || pending.values().any(|p| p["type"] == "preset_load") {
                    plugin_view.operation_error = Some(
                        "Wait for the paused plugin operation to finish, or cancel playback".into(),
                    );
                    continue;
                }
                let (payload, advances_epoch) = match command {
                    Command::Plugin(request) => {
                        match plugin_request(
                            request,
                            &sources,
                            &plugin_view.capabilities,
                            transport_state,
                            private.path(),
                            request_id + 1,
                            &mut checkpoint,
                        ) {
                            Ok(payload) => (payload, false),
                            Err(error) => {
                                plugin_view.operation_error = Some(format!("{error:#}"));
                                continue;
                            }
                        }
                    }
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
                connection.queue(&envelope(payload.clone(), &token, request_id, epoch))?;
                pending.insert(request_id, payload);
                outstanding.push_back((request_id, epoch, Instant::now()));
            }
        } else {
            ensure!(
                started.elapsed() < if ready { LOAD_TIMEOUT } else { CONNECT_TIMEOUT },
                "Audio host handshake or SoundFont load timed out"
            );
        }
        plugin_view.busy =
            checkpoint.is_some() || pending.values().any(|p| p["type"] == "preset_load");
        view.lock().unwrap_or_else(|e| e.into_inner()).plugins = plugin_view.clone();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn plugin_request(
    request: super::plugins::Request,
    sources: &BTreeMap<String, super::plugins::Source>,
    capabilities: &[String],
    state: State,
    private: &std::path::Path,
    id: u64,
    checkpoint: &mut Option<(u64, tempfile::TempDir, PathBuf)>,
) -> Result<Value> {
    use super::plugins::{Request, Source};
    let require = |cap: &str| -> Result<()> {
        ensure!(
            capabilities.iter().any(|c| c == cap),
            "Audio host does not support {cap}"
        );
        Ok(())
    };
    let part = match &request {
        Request::Gain { part_id, .. }
        | Request::Parameters { part_id, .. }
        | Request::Parameter { part_id, .. }
        | Request::Editor { part_id, .. }
        | Request::Preset { part_id, .. } => Some(part_id),
        Request::Checkpoint { .. } => None,
    };
    if let Some(part) = part {
        ensure!(
            sources.get(part).is_some_and(Source::is_plugin),
            "Not a VST3 part"
        );
    }
    Ok(match request {
        Request::Gain { part_id, gain } => {
            require("vst3")?;
            ensure!(
                gain.is_finite() && (0.0..=1.0).contains(&gain),
                "Invalid gain"
            );
            json!({"type":"gain","part_id":part_id,"gain":gain})
        }
        Request::Parameters { part_id, offset } => {
            require("parameters")?;
            ensure!(offset <= 4096, "Parameter offset exceeds limit");
            json!({"type":"parameters","part_id":part_id,"offset":offset,"limit":128})
        }
        Request::Parameter {
            part_id,
            param_id,
            value,
        } => {
            require("parameters")?;
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "Invalid parameter value"
            );
            json!({"type":"parameter","part_id":part_id,"param_id":param_id,"value":value})
        }
        Request::Editor { part_id, open } => {
            require("editor_x11")?;
            json!({"type":if open {"editor_open"} else {"editor_close"},"part_id":part_id})
        }
        Request::Preset { part_id, path } => {
            require("presets")?;
            ensure!(
                matches!(state, State::Paused | State::Stopped | State::Finished),
                "Pause or stop before loading a preset"
            );
            let Some(Source::Vst3 { class_id, .. }) = sources.get(&part_id) else {
                unreachable!()
            };
            super::plugins::validate_preset(&path, class_id)?;
            json!({"type":"preset_load","part_id":part_id,"path":path})
        }
        Request::Checkpoint { destination } => {
            require("plugin_state")?;
            ensure!(
                matches!(state, State::Paused | State::Stopped | State::Finished),
                "Pause or stop before saving plugin state"
            );
            ensure!(
                destination.is_absolute(),
                "State destination must be absolute"
            );
            let staging = tempfile::Builder::new()
                .prefix("checkpoint-")
                .tempdir_in(private)?;
            fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))?;
            let value = json!({"type":"checkpoint","directory":staging.path()});
            *checkpoint = Some((id, staging, destination));
            value
        }
    })
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
pub(crate) mod tests;
