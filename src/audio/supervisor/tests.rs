use super::*;
use crate::{
    audio,
    notation::{self, MidiOptions},
    playback::Transport,
};
use std::os::unix::fs::PermissionsExt;

fn score() -> notation::Composition {
    let mut score = notation::parse_musicxml(r#"<score-partwise><part-list><score-part id="P1"><part-name>Cello</part-name></score-part></part-list><part id="P1"><measure number="1"><attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>3</octave></pitch><duration>32</duration></note></measure></part></score-partwise>"#).unwrap();
    score.page_map = Some(PageMap {
        source_pages: vec![0, 1],
        turns: vec![
            notation::PageTurn { tick: 0, page: 0 },
            notation::PageTurn {
                tick: 1920,
                page: 1,
            },
        ],
        measure_ticks: (0..=8).map(|n| n * 3840).collect(),
    });
    score
}

#[test]
#[ignore = "Requires RMS_TEST_AUDIO_HOST, a real SoundFont and a running PipeWire/JACK server; master stays muted"]
fn real_audio_host_muted_smoke() {
    let helper = std::env::var("RMS_TEST_AUDIO_HOST").expect("Set RMS_TEST_AUDIO_HOST");
    let mut settings = crate::omr::Settings::default();
    if let Ok(bank) = std::env::var("RMS_TEST_SOUNDFONT") {
        settings.soundfont = bank;
    }
    let audio = audio::Settings {
        backend: if std::env::var("RMS_TEST_AUDIO_BACKEND").as_deref() == Ok("jack") {
            Backend::Jack
        } else {
            Backend::PipeWire
        },
        host_executable: helper,
    };
    let mut composition = score();
    let mut piano = composition.parts[0].clone();
    piano.id = "P2".into();
    piano.staves = 2;
    piano.program = 0;
    composition.parts.push(piano);
    let transport = Transport::start_with_backend(
        settings,
        audio,
        composition,
        MidiOptions::default(),
        false,
        0,
    )
    .unwrap();
    let initial = wait(&transport, |s| s.state == State::Stopped);
    let diagnostics = initial.audio.unwrap();
    assert!((8_000..=384_000).contains(&diagnostics.sample_rate));
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(1.5).unwrap();
    wait(&transport, |s| {
        s.state == State::Paused && (s.position - 1.5).abs() < 0.001
    });
    transport.tempo(0.5).unwrap();
    wait(&transport, |s| (s.position - 3.0).abs() < 0.001);
    transport.mix("P1", true, 0, 42).unwrap();
    transport.mix("P1", true, 64, 42).unwrap();
    transport.mix("P2", false, 96, 0).unwrap();
    transport.mix("P2", true, 96, 0).unwrap();
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 3.05
    });
    transport.pause().unwrap();
    let paused = wait(&transport, |s| s.state == State::Paused);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(paused.position, transport.snapshot().position);
    transport.stop().unwrap();
    wait(&transport, |s| {
        s.state == State::Stopped && s.score_tick == 0
    });
    println!(
        "Muted real-host transport passed: {} Hz, {} reported latency frames, {} xruns",
        diagnostics.sample_rate,
        diagnostics.latency_frames,
        transport.snapshot().audio.unwrap().xruns
    );
}

fn fake(root: &std::path::Path, mode: &str) -> (crate::omr::Settings, audio::Settings) {
    let host = root.join("audio-host");
    let script = format!(
        "#!/usr/bin/python3\nMODE = {mode:?}\nLOG = {}\n",
        serde_json::to_string(&root.join("commands")).unwrap()
    ) + r#"
import hashlib, json, os, select, socket, stat, struct, sys, time
args = dict(zip(sys.argv[2::2], sys.argv[3::2]))
assert sys.argv[1] == 'serve'
assert int(args['--parent-pid']) == os.getppid()
s = socket.socket(socket.AF_UNIX)
s.connect(args['--socket'])
peer = struct.unpack('3i', s.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
assert peer[0] == os.getppid() and peer[1] == os.geteuid()
def exact(n):
    data = b''
    while len(data) < n:
        new = s.recv(n-len(data))
        if not new: sys.exit(0)
        data += new
    return data
def receive():
    size = struct.unpack('!I', exact(4))[0]
    assert 0 < size <= 1048576
    return json.loads(exact(size))
hello = receive()
session = hello['session']
request, epoch = 0, 0
def send(obj, old=False):
    obj.update(version=2 if MODE=='version' else 1, session='bad' if MODE=='session' else session,
               request_id=request, epoch=epoch-1 if MODE=='bad_epoch' and request>1 else epoch)
    data = json.dumps(obj).encode()
    s.sendall(struct.pack('!I',len(data))+data)
send(dict(type='hello',capabilities=['soundfont','jack']))
if MODE in ('version','session'): time.sleep(20); sys.exit(0)
load = receive()
assert load['type']=='load'
path = load['plan_path']
assert stat.S_IMODE(os.stat(os.path.dirname(path)).st_mode)==0o700
assert stat.S_IMODE(os.stat(path).st_mode)==0o600
raw = open(path,'rb').read()
assert len(raw)==load['plan_bytes'] and hashlib.sha256(raw).hexdigest()==load['plan_sha256']
plan = json.loads(raw)
request = load['request_id']
duration = 0.
for index, tempo in enumerate(plan['tempos']):
    if tempo['tick'] > plan['duration_ticks']: break
    end = min(plan['duration_ticks'], plan['tempos'][index+1]['tick']) if index+1<len(plan['tempos']) else plan['duration_ticks']
    duration += (end-tempo['tick'])/960*60/tempo['bpm']
position = 0.
scale = load['tempo_scale']
state = 'playing' if load['autoplay'] else 'stopped'
def status():
    send(dict(type='status',state=state,position=position,rendered_position=position,
              duration=duration,score_tick=int(position*1920),
              tempo_scale=scale,sample_rate=48000,latency_frames=480,xruns=0))
status()
with open(LOG,'a') as log:
    log.write(json.dumps(dict(type='loaded',pid=os.getpid(),path=path,parts=len(plan['parts'])))+'\n')
if MODE=='heartbeat': time.sleep(30); sys.exit(0)
while True:
    if not select.select([s],[],[],.02)[0]:
        status()
        continue
    command = receive()
    with open(LOG,'a') as log: log.write(json.dumps(command)+'\n')
    kind = command['type']
    if kind=='play' and MODE=='crash': os._exit(7)
    if kind=='seek':
        # A previously queued status must not jump to the next page after a seek.
        saved = position
        position = duration
        status()
        position = saved
        time.sleep(.04)
    request, epoch = command['request_id'], command['epoch']
    if kind=='play': state = 'playing'
    elif kind=='pause': state = 'paused'
    elif kind=='stop': position=0.; state='stopped'
    elif kind=='seek': position=max(0.,min(duration,position+command['delta']*scale))
    elif kind=='tempo': scale=command['scale']
    elif kind=='shutdown': sys.exit(0)
    elif kind not in ('master_volume','mix'): raise RuntimeError(kind)
    status()
"#;
    fs::write(&host, script).unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let bank = root.join("bank.sf2");
    fs::write(&bank, b"RIFFxxxxsfbk").unwrap();
    (
        crate::omr::Settings {
            soundfont: bank.to_str().unwrap().into(),
            ..Default::default()
        },
        audio::Settings {
            backend: Backend::Jack,
            host_executable: host.to_str().unwrap().into(),
        },
    )
}

fn wait(transport: &Transport, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = transport.snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(snapshot.state != State::Failed, "{:?}", snapshot.error);
        assert!(Instant::now() < deadline, "Timed out: {snapshot:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn supervisor_preserves_transport_pages_volume_commands_and_cleans_owned_process() {
    let root = tempfile::tempdir().unwrap();
    let (settings, audio) = fake(root.path(), "normal");
    let transport =
        Transport::start_with_backend(settings, audio, score(), MidiOptions::default(), false, 75)
            .unwrap();
    let initial = wait(&transport, |s| s.state == State::Stopped);
    assert_eq!(initial.source_page, Some(0));
    assert_eq!(initial.audio.unwrap().latency_frames, 480);
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(2.0).unwrap();
    wait(&transport, |s| {
        s.position == 2.0 && s.source_page == Some(1)
    });
    transport.tempo(0.5).unwrap();
    let slower = wait(&transport, |s| s.position == 4.0);
    assert_eq!(slower.score_tick, 3840);
    transport.seek(-3.0).unwrap();
    wait(&transport, |s| {
        s.position == 1.0 && s.source_page == Some(0)
    });
    transport.mix("P1", false, 32, 42).unwrap();
    transport.master_volume(25).unwrap();
    transport.play().unwrap();
    wait(&transport, |s| s.state == State::Playing);
    transport.stop().unwrap();
    wait(&transport, |s| {
        s.state == State::Stopped && s.position == 0.0
    });
    let log = fs::read_to_string(root.path().join("commands")).unwrap();
    assert!(log.contains("\"volume\": 32") && log.contains("\"percent\": 25"));
    let loaded: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    let plan = PathBuf::from(loaded["path"].as_str().unwrap());
    let pid = loaded["pid"].as_i64().unwrap() as libc::pid_t;
    drop(transport);
    let deadline = Instant::now() + Duration::from_secs(3);
    while plan.exists() {
        assert!(Instant::now() < deadline, "Private plan not cleaned up");
        std::thread::sleep(Duration::from_millis(5));
    }
    // SAFETY: signal zero only checks the existence of the test's recorded child.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn rejects_protocol_and_session_mismatch_without_legacy_fallback() {
    for mode in ["version", "session"] {
        let root = tempfile::tempdir().unwrap();
        let (settings, audio) = fake(root.path(), mode);
        let transport = Transport::start_with_backend(
            settings,
            audio,
            score(),
            MidiOptions::default(),
            false,
            100,
        )
        .unwrap();
        let failed = wait(&transport, |s| s.state == State::Failed);
        assert!(failed.error.unwrap().contains("mismatch"));
    }
}

#[test]
fn crashed_host_and_missing_executable_fail_only_the_transport() {
    let root = tempfile::tempdir().unwrap();
    let (settings, mut audio) = fake(root.path(), "crash");
    let transport = Transport::start_with_backend(
        settings.clone(),
        audio.clone(),
        score(),
        MidiOptions::default(),
        false,
        100,
    )
    .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    transport.play().unwrap();
    assert!(wait(&transport, |s| s.state == State::Failed)
        .error
        .is_some());
    audio.host_executable = root.path().join("missing").display().to_string();
    let missing =
        Transport::start_with_backend(settings, audio, score(), MidiOptions::default(), false, 100)
            .unwrap();
    assert!(wait(&missing, |s| s.state == State::Failed)
        .error
        .unwrap()
        .contains("not installed"));
}

#[test]
fn watchdog_terminates_a_live_but_unresponsive_host() {
    let root = tempfile::tempdir().unwrap();
    let (settings, audio) = fake(root.path(), "heartbeat");
    let transport =
        Transport::start_with_backend(settings, audio, score(), MidiOptions::default(), false, 100)
            .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    let failed = wait(&transport, |s| s.state == State::Failed);
    assert!(failed.error.unwrap().contains("heartbeat"));
}

#[test]
fn acknowledgement_cannot_use_a_stale_epoch_to_bypass_the_command_watchdog() {
    let root = tempfile::tempdir().unwrap();
    let (settings, audio) = fake(root.path(), "bad_epoch");
    let transport =
        Transport::start_with_backend(settings, audio, score(), MidiOptions::default(), false, 100)
            .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    transport.play().unwrap();
    let failed = wait(&transport, |s| s.state == State::Failed);
    assert!(failed.error.unwrap().contains("wrong transport epoch"));
}

#[test]
fn graph_plan_is_not_limited_to_fifteen_parts() {
    let root = tempfile::tempdir().unwrap();
    let (settings, audio) = fake(root.path(), "normal");
    let mut score = score();
    score.parts = (0..32)
        .map(|index| {
            let mut part = score.parts[0].clone();
            part.id = format!("P{index}");
            part
        })
        .collect();
    let transport =
        Transport::start_with_backend(settings, audio, score, MidiOptions::default(), false, 100)
            .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(log) = fs::read_to_string(root.path().join("commands")) {
            assert_eq!(
                serde_json::from_str::<Value>(log.lines().next().unwrap()).unwrap()["parts"],
                32
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn status_and_plan_writer_reject_unbounded_values() {
    let mut status = Status {
        state: "paused".into(),
        position: 0.0,
        rendered_position: 0.0,
        duration: 10.0,
        score_tick: 0,
        tempo_scale: 1.0,
        sample_rate: 48000,
        latency_frames: 480,
        xruns: 0,
    };
    assert_eq!(
        validate_status(&status, 10.0, 19200).unwrap(),
        State::Paused
    );
    status.position = f64::NAN;
    assert!(validate_status(&status, 10.0, 19200).is_err());
    status.position = 0.0;
    status.latency_frames = u64::MAX;
    assert!(validate_status(&status, 10.0, 19200).is_err());
    let mut file = tempfile::tempfile().unwrap();
    let mut writer = LimitedWriter {
        writer: &mut file,
        remaining: 2,
    };
    assert!(writer.write_all(b"123").is_err());
}
