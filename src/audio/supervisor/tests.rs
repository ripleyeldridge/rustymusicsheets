use super::*;
use crate::{
    audio,
    notation::{self, MidiOptions},
    playback::Transport,
};
use std::os::unix::fs::PermissionsExt;
#[test]
fn bridge_editor_advisory_survives_successful_scan_load_and_playback() {
    let root = tempfile::tempdir().unwrap();
    let (settings, mut audio) = fake(root.path(), "normal");
    audio.yabridge = audio::yabridge::tests::advisory_profile(root.path());
    let wrappers = root.path().join("wrappers");
    let bundle = audio::plugins::fixture(&wrappers, "Synth");
    let mut catalogue = audio::discovery::Catalogue::default();
    let cancel = AtomicBool::new(false);
    for retry in [false, false, true] {
        let notifications = catalogue
            .scan(&audio, std::slice::from_ref(&bundle), &cancel, retry)
            .unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].contains("far left"));
        assert!(catalogue.entries[0].failure.is_none());
        assert_eq!(catalogue.entries[0].classes.len(), 2);
    }
    let sources = BTreeMap::from([(
        "P1".into(),
        audio::plugins::Source::Vst3 {
            path: bundle.clone(),
            class_id: "A".repeat(32),
            module_sha256: audio::plugins::inspect_bundle(&bundle).unwrap(),
            gain: 0.0,
            component_state: None,
            controller_state: None,
        },
    )]);
    let transport = Transport::start_with_sources(
        settings,
        audio,
        score(),
        MidiOptions::default(),
        false,
        0,
        sources,
    )
    .unwrap();
    let stopped = wait(&transport, |s| s.state == State::Stopped);
    let notification = stopped.plugins.bridge_notification.unwrap();
    assert!(notification.contains("far left"));
    assert!(stopped.error.is_none() && stopped.plugins.operation_error.is_none());
    assert!(!stopped.plugins.busy);
    transport.play().unwrap();
    let playing = wait(&transport, |s| s.state == State::Playing);
    assert_eq!(
        playing.plugins.bridge_notification.as_ref(),
        Some(&notification)
    );
    assert!(playing.error.is_none() && playing.plugins.operation_error.is_none());
    assert!(!root.path().join(".wine").exists());
}
#[test]
fn editor_wait_is_bounded_and_preserves_remaining_command_budget() {
    let now = Instant::now();
    let original = now.checked_sub(Duration::from_secs(1)).unwrap();
    let mut pending = VecDeque::from([(2, 0, original)]);
    let mut wait = EditorWait::default();
    wait.keepalive(now).unwrap();
    wait.keepalive(now + Duration::from_secs(6)).unwrap();
    wait.finish(now + Duration::from_secs(7), &mut pending)
        .unwrap();
    assert_eq!(pending[0].2, original + Duration::from_secs(7));
    assert!(wait.since.is_none());
    wait.keepalive(now).unwrap();
    assert!(wait.keepalive(now + EDITOR_CALLBACK_TIMEOUT).is_err());
    assert!(wait
        .finish(now + EDITOR_CALLBACK_TIMEOUT, &mut pending)
        .is_err());
}

#[test]
fn editor_keepalives_allow_a_long_modal_callback_then_resume_commands() {
    use audio::plugins::Request;
    let root = tempfile::tempdir().unwrap();
    let (transport, _, _) = plugin_session(root.path(), "editor_return");
    wait(&transport, |s| s.state == State::Stopped);
    let started = Instant::now();
    transport
        .plugin(Request::Editor {
            part_id: "P1".into(),
            open: true,
        })
        .unwrap();
    let busy = wait(&transport, |s| s.plugins.editor_busy);
    assert!(busy.plugins.busy);
    let resumed = wait(&transport, |s| !s.plugins.busy);
    assert!(started.elapsed() >= Duration::from_secs(6));
    assert_eq!(resumed.state, State::Stopped);
    assert!(resumed.plugins.operation_error.is_none());
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 0,
        })
        .unwrap();
    wait(&transport, |s| s.plugins.parameters.contains_key("P1"));
    transport.play().unwrap();
    wait(&transport, |s| s.state == State::Playing);
}

#[test]
fn cancelling_a_modal_editor_retains_the_previous_checkpoint() {
    use audio::plugins::Request;
    let root = tempfile::tempdir().unwrap();
    let (transport, _, _) = plugin_session(root.path(), "editor_hang");
    wait(&transport, |s| s.state == State::Stopped);
    transport
        .plugin(Request::Checkpoint {
            destination: root.path().join("saved"),
        })
        .unwrap();
    let saved = wait(&transport, |s| s.plugins.checkpoint.is_some());
    let descriptor = saved.plugins.checkpoint.unwrap().1[0]
        .component_state
        .clone()
        .unwrap();
    let bytes = fs::read(&descriptor.path).unwrap();
    transport
        .plugin(Request::Editor {
            part_id: "P1".into(),
            open: true,
        })
        .unwrap();
    wait(&transport, |s| s.plugins.editor_busy);
    let started = Instant::now();
    drop(transport);
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(fs::read(descriptor.path).unwrap(), bytes);
}

#[test]
fn unnegotiated_or_malformed_editor_keepalives_fail_closed() {
    use audio::plugins::Request;
    for mode in ["editor_no_cap", "editor_bad_id", "editor_bad_busy"] {
        let root = tempfile::tempdir().unwrap();
        let (transport, _, _) = plugin_session(root.path(), mode);
        wait(&transport, |s| s.state == State::Stopped);
        transport
            .plugin(Request::Editor {
                part_id: "P1".into(),
                open: true,
            })
            .unwrap();
        let failed = wait(&transport, |s| s.state == State::Failed);
        assert!(failed.error.unwrap().contains("editor keepalive"));
    }
}

#[test]
fn an_editor_keepalive_does_not_disable_the_missing_heartbeat_watchdog() {
    use audio::plugins::Request;
    let root = tempfile::tempdir().unwrap();
    let (transport, _, _) = plugin_session(root.path(), "editor_silent");
    wait(&transport, |s| s.state == State::Stopped);
    transport
        .plugin(Request::Editor {
            part_id: "P1".into(),
            open: true,
        })
        .unwrap();
    wait(&transport, |s| s.plugins.editor_busy);
    let failed = wait(&transport, |s| s.state == State::Failed);
    assert!(failed.error.unwrap().contains("heartbeat"));
}

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

pub(crate) fn plugin_session(
    root: &std::path::Path,
    mode: &str,
) -> (
    Transport,
    BTreeMap<String, audio::plugins::Source>,
    audio::Settings,
) {
    let (mut settings, audio) = fake(root, mode);
    settings.soundfont = "/no-soundfont-required".into();
    let bundle = audio::plugins::fixture(root, "Synth été.v1");
    let hash = audio::plugins::inspect_bundle(&bundle).unwrap();
    let mut composition = score();
    let mut second = composition.parts[0].clone();
    second.id = "P2".into();
    second.staves = 2;
    composition.parts.push(second);
    let sources = ["P1", "P2"]
        .map(|id| {
            (
                id.into(),
                audio::plugins::Source::Vst3 {
                    path: bundle.clone(),
                    class_id: "A".repeat(32),
                    module_sha256: hash.clone(),
                    gain: 0.5,
                    component_state: None,
                    controller_state: None,
                },
            )
        })
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let transport = Transport::start_with_sources(
        settings,
        audio.clone(),
        composition,
        MidiOptions::default(),
        false,
        0,
        sources.clone(),
    )
    .unwrap();
    (transport, sources, audio)
}

#[test]
fn independent_plugin_state_survives_reload_with_parameters_gain_editors_and_presets() {
    use audio::plugins::{Request, Source};
    let root = tempfile::tempdir().unwrap();
    let (transport, mut sources, audio) = plugin_session(root.path(), "normal");
    wait(&transport, |s| s.state == State::Stopped);
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 128,
        })
        .unwrap();
    let page = wait(&transport, |s| s.plugins.parameters.contains_key("P1"));
    assert_eq!(page.plugins.parameters["P1"].0, 129);
    assert_eq!(page.plugins.parameters["P1"].2.len(), 1);
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 0,
        })
        .unwrap();
    wait(&transport, |s| s.plugins.parameters["P1"].1 == 0);
    for (id, value) in [("P1", 0.2), ("P2", 0.8)] {
        transport
            .plugin(Request::Parameter {
                part_id: id.into(),
                param_id: 0,
                value,
            })
            .unwrap();
    }
    wait(&transport, |s| s.plugins.dirty_parts.len() == 2);
    wait(&transport, |s| s.plugins.parameters["P1"].2[0].value == 0.2);
    for open in [true, false, true, false] {
        transport
            .plugin(Request::Editor {
                part_id: "P1".into(),
                open,
            })
            .unwrap();
    }
    transport
        .plugin(Request::Gain {
            part_id: "P2".into(),
            gain: 0.3,
        })
        .unwrap();
    let mut preset = [0u8; 48];
    preset[..4].copy_from_slice(b"VST3");
    preset[8..40].copy_from_slice("A".repeat(32).as_bytes());
    let preset_path = root.path().join("Preset été.vstpreset");
    fs::write(&preset_path, preset).unwrap();
    transport
        .plugin(Request::Preset {
            part_id: "P1".into(),
            path: preset_path,
        })
        .unwrap();
    // Let preset ack complete before issuing the serialized quiescent checkpoint.
    std::thread::sleep(Duration::from_millis(100));
    transport
        .plugin(Request::Checkpoint {
            destination: root.path().join("persistent-state"),
        })
        .unwrap();
    let saved = wait(&transport, |s| s.plugins.checkpoint.is_some());
    for part in saved.plugins.checkpoint.unwrap().1 {
        let Source::Vst3 {
            component_state,
            controller_state,
            ..
        } = sources.get_mut(&part.part_id).unwrap()
        else {
            unreachable!()
        };
        assert_ne!(part.component_state, part.controller_state);
        *component_state = part.component_state;
        *controller_state = part.controller_state;
    }
    drop(transport);
    let mut composition = score();
    let mut second = composition.parts[0].clone();
    second.id = "P2".into();
    composition.parts.push(second);
    let reloaded = Transport::start_with_sources(
        crate::omr::Settings::default(),
        audio,
        composition,
        MidiOptions::default(),
        false,
        0,
        sources,
    )
    .unwrap();
    wait(&reloaded, |s| s.state == State::Stopped);
    for (id, expected) in [("P1", 0.2), ("P2", 0.8)] {
        reloaded
            .plugin(Request::Parameters {
                part_id: id.into(),
                offset: 0,
            })
            .unwrap();
        let snapshot = wait(&reloaded, |s| s.plugins.parameters.contains_key(id));
        assert_eq!(snapshot.plugins.parameters[id].2[0].value, expected);
    }
}

#[test]
fn vst3_state_changes_invalidate_cached_parameter_page() {
    use audio::plugins::Request;
    let root = tempfile::tempdir().unwrap();
    let (transport, _, _) = plugin_session(root.path(), "preset_changes");
    wait(&transport, |s| s.state == State::Stopped);
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 0,
        })
        .unwrap();
    let initial = wait(&transport, |s| s.plugins.parameters.contains_key("P1"));
    assert_eq!(initial.plugins.parameters["P1"].2[0].value, 0.5);
    let mut preset = [0u8; 48];
    preset[..4].copy_from_slice(b"VST3");
    preset[8..40].copy_from_slice("A".repeat(32).as_bytes());
    let path = root.path().join("changed.vstpreset");
    fs::write(&path, preset).unwrap();
    transport
        .plugin(Request::Preset {
            part_id: "P1".into(),
            path,
        })
        .unwrap();
    let changed = wait(&transport, |s| !s.plugins.dirty_parts.is_empty());
    assert!(
        !changed.plugins.parameters.contains_key("P1"),
        "Do not display pre-preset parameter values"
    );
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 0,
        })
        .unwrap();
    let refreshed = wait(&transport, |s| s.plugins.parameters.contains_key("P1"));
    assert_eq!(refreshed.plugins.parameters["P1"].2[0].value, 0.63);
    transport
        .plugin(Request::Editor {
            part_id: "P1".into(),
            open: false,
        })
        .unwrap();
    wait(&transport, |s| !s.plugins.parameters.contains_key("P1"));
    transport
        .plugin(Request::Parameters {
            part_id: "P1".into(),
            offset: 0,
        })
        .unwrap();
    let edited = wait(&transport, |s| s.plugins.parameters.contains_key("P1"));
    assert_eq!(edited.plugins.parameters["P1"].2[0].value, 0.72);
}

#[test]
fn cancelling_a_busy_checkpoint_drops_only_its_owned_helper() {
    use audio::plugins::Request;
    let root = tempfile::tempdir().unwrap();
    let (transport, _, _) = plugin_session(root.path(), "state_hang");
    wait(&transport, |s| s.state == State::Stopped);
    let destination = root.path().join("saved");
    std::fs::create_dir(&destination).unwrap();
    let previous = destination.join("previous.state");
    std::fs::write(&previous, b"previous valid checkpoint").unwrap();
    transport
        .plugin(Request::Checkpoint { destination })
        .unwrap();
    wait(&transport, |s| s.plugins.busy);
    let at = Instant::now();
    drop(transport);
    assert!(at.elapsed() < Duration::from_millis(200));
    assert_eq!(
        std::fs::read(previous).unwrap(),
        b"previous valid checkpoint"
    );
}

#[test]
fn plugin_capabilities_result_order_and_untrusted_checkpoint_fail_safely() {
    use audio::plugins::Request;
    for mode in ["core", "result_order", "bad_parameters", "bad_checkpoint"] {
        let root = tempfile::tempdir().unwrap();
        let (transport, _, _) = plugin_session(root.path(), mode);
        if mode == "core" {
            assert!(wait(&transport, |s| s.state == State::Failed)
                .error
                .unwrap()
                .contains("lacks vst3"));
            continue;
        }
        wait(&transport, |s| s.state == State::Stopped);
        if mode == "bad_checkpoint" {
            transport
                .plugin(Request::Checkpoint {
                    destination: root.path().join("saved"),
                })
                .unwrap();
            let snapshot = wait(&transport, |s| s.plugins.operation_error.is_some());
            assert!(snapshot.plugins.checkpoint.is_none());
            assert_eq!(snapshot.state, State::Stopped);
            assert!(!root.path().join("saved").exists());
        } else {
            transport
                .plugin(Request::Parameters {
                    part_id: "P1".into(),
                    offset: 0,
                })
                .unwrap();
            assert!(wait(&transport, |s| s.state == State::Failed)
                .error
                .is_some());
        }
    }
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
        ..Default::default()
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

pub(crate) fn fake(root: &std::path::Path, mode: &str) -> (crate::omr::Settings, audio::Settings) {
    let host = root.join("audio-host");
    let script = format!(
        "#!/usr/bin/python3\nMODE = {mode:?}\nLOG = {}\n",
        serde_json::to_string(&root.join("commands")).unwrap()
    ) + r#"
import hashlib, json, os, select, socket, stat, struct, sys, time
args = dict(zip(sys.argv[2::2], sys.argv[3::2]))
assert sys.argv[1] in ('serve','probe','scan')
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
    obj.update(version=1 if MODE=='version' else 2, session='bad' if MODE=='session' else session,
               request_id=request, epoch=epoch-1 if MODE=='bad_epoch' and request>1 else epoch)
    data = json.dumps(obj).encode()
    s.sendall(struct.pack('!I',len(data))+data)
caps = ['soundfont','jack'] if MODE=='core' else ['soundfont','jack','vst3','plugin_state','parameters','presets','editor_x11']
if MODE.startswith('editor_') and MODE!='editor_no_cap': caps.append('editor_keepalive')
send(dict(type='hello',capabilities=caps))
if MODE in ('version','session'): time.sleep(20); sys.exit(0)
if sys.argv[1]=='probe':
    assert receive()['type']=='shutdown'
    sys.exit(0)
if sys.argv[1]=='scan':
    scan=receive()
    request=scan['request_id']
    with open(LOG,'a') as log: log.write(json.dumps(scan)+'\n')
    if MODE=='scan_crash': os._exit(7)
    if MODE=='scan_hang': time.sleep(30); sys.exit(0)
    classes=[dict(class_id='A'*32,name='Generated synth',vendor='Test',version='1'),dict(class_id='B'*32,name='Other class',vendor='Test',version='1')]
    if MODE=='duplicate': classes.append(classes[0])
    send(dict(type='scan_result',path=scan['path'],module_sha256=scan['module_sha256'],classes=classes))
    assert receive()['type']=='shutdown'
    sys.exit(0)
load = receive()
assert load['type']=='load'
path = load['plan_path']
assert stat.S_IMODE(os.stat(os.path.dirname(path)).st_mode)==0o700
assert stat.S_IMODE(os.stat(path).st_mode)==0o600
raw = open(path,'rb').read()
assert len(raw)==load['plan_bytes'] and hashlib.sha256(raw).hexdigest()==load['plan_sha256']
plan = json.loads(raw)
assert plan['version']==2
assert all('source' in p for p in plan['parts'])
if all(p['source']['kind']=='vst3' for p in plan['parts']): assert load['soundfont'] is None
values = {}
for p in plan['parts']:
    if p['source']['kind']=='vst3':
        values[p['id']]=.5
        statefile=p['source'].get('component_state')
        if statefile: values[p['id']]=json.load(open(statefile['path']))['value']
dirty=[]
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
              tempo_scale=scale,sample_rate=48000,latency_frames=480,xruns=0,dirty_parts=dirty))
status()
with open(LOG,'a') as log:
    log.write(json.dumps(dict(type='loaded',pid=os.getpid(),path=path,parts=len(plan['parts']),values=values))+'\n')
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
    elif kind=='parameters':
        if MODE=='result_order': status()
        offset=command['offset']
        parameters=[dict(id=i,title='Parameter '+str(i),units='',value=values[command['part_id']],default_value=.5,step_count=0,flags=2 if i==1 else 0) for i in range(offset,min(129,offset+command['limit']))]
        send(dict(type='parameters_result',part_id=command['part_id'],total=5000 if MODE=='bad_parameters' else 129,offset=offset,parameters=parameters))
    elif kind=='parameter':
        values[command['part_id']]=command['value']; dirty=list(values)
        send(dict(type='ack',operation=kind,part_id=command['part_id']))
    elif kind=='checkpoint':
        assert state!='playing'
        if MODE=='state_hang': time.sleep(30)
        parts=[]
        for i,p in enumerate(plan['parts']):
            if p['source']['kind']!='vst3': continue
            files=[]
            for component in ('component','controller'):
                path=os.path.join(command['directory'],str(i)+'.'+component)
                raw=json.dumps(dict(value=values[p['id']],part=p['id'],chunk=component)).encode()
                fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
                with os.fdopen(fd,'wb') as output: output.write(raw)
                files.append(dict(path=path,sha256=hashlib.sha256(raw).hexdigest(),bytes=len(raw)))
            parts.append(dict(part_id=p['id'],class_id=p['source']['class_id'],component_state=files[0],controller_state=files[1]))
        if MODE=='bad_checkpoint': parts[0]['component_state']['sha256']='0'*64
        send(dict(type='checkpoint_result',parts=parts))
        dirty=[]
    elif kind=='preset_load':
        assert state!='playing'
        if MODE=='preset_changes': values[command['part_id']]=.63
        dirty=list(values)
        send(dict(type='ack',operation=kind,part_id=command['part_id']))
    elif kind=='editor_open' and MODE.startswith('editor_'):
        assert 'editor_keepalive' in hello['client_capabilities']
        def busy():
            frame=dict(version=2,session=session,request_id=1 if MODE=='editor_bad_id' else 0,
                       epoch=0,type='editor_keepalive',editor_busy=MODE!='editor_bad_busy')
            raw=json.dumps(frame).encode()
            s.sendall(struct.pack('!I',len(raw))+raw)
        if MODE in ('editor_no_cap','editor_bad_id','editor_bad_busy','editor_silent'):
            busy(); time.sleep(30)
        elif MODE=='editor_hang':
            while True: busy(); time.sleep(.1)
        else:
            for _ in range(13): busy(); time.sleep(.5)
        send(dict(type='ack',operation=kind,part_id=command['part_id']))
    elif kind in ('editor_open','editor_close','gain'):
        if MODE=='preset_changes' and kind=='editor_close': values[command['part_id']]=.72
        send(dict(type='ack',operation=kind,part_id=command['part_id']))
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
            ..Default::default()
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
        dirty_parts: Vec::new(),
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
