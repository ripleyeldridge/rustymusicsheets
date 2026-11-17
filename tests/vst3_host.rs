#![cfg(target_os = "linux")]
//! Explicit live tests use only the generated SDK fixture and keep master muted.
use rustymusicsheets::{
    audio::{
        self,
        plugins::{Request, Source},
    },
    notation::{Composition, MidiOptions, Note, Part},
    omr,
    playback::{Snapshot, State, Transport},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

fn wait(transport: &Transport, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = transport.snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(snapshot.state != State::Failed, "{:?}", snapshot.error);
        assert!(
            snapshot.plugins.operation_error.is_none(),
            "{:?}",
            snapshot.plugins.operation_error
        );
        assert!(Instant::now() < until, "Timed out: {snapshot:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn setup() -> (audio::Settings, Composition, Source) {
    let helper = std::env::var("RMS_TEST_AUDIO_HOST").expect("Set RMS_TEST_AUDIO_HOST");
    let bundle = PathBuf::from(
        std::env::var("RMS_TEST_VST3_BUNDLE")
            .expect("Set RMS_TEST_VST3_BUNDLE to generated fixture"),
    );
    let settings = audio::Settings {
        backend: audio::Backend::PipeWire,
        host_executable: helper,
        ..Default::default()
    };
    let cancelled = AtomicBool::new(false);
    let capabilities = audio::discovery::probe(&settings, &cancelled).unwrap();
    assert!(capabilities.iter().any(|cap| cap == "vst3"));
    let mut catalogue = audio::discovery::Catalogue::default();
    catalogue
        .scan(&settings, &[bundle.clone()], &cancelled, false)
        .unwrap();
    let entry = &catalogue.entries[0];
    assert!(entry.failure.is_none(), "{:?}", entry.failure);
    assert_eq!(entry.classes.len(), 1);
    assert_eq!(
        entry.classes[0].class_id, "6C73687465737431A1023456789ABCDE",
        "Use only the generated deterministic test instrument"
    );
    let source = Source::Vst3 {
        path: bundle,
        class_id: entry.classes[0].class_id.clone(),
        module_sha256: entry.module_sha256.clone(),
        gain: 0.5,
        component_state: None,
        controller_state: None,
    };
    let parts = ["P1", "P2"]
        .into_iter()
        .map(|id| Part {
            id: id.into(),
            name: id.into(),
            program: 0,
            staves: if id == "P2" { 2 } else { 1 },
            notes: vec![Note {
                start_tick: 0,
                duration_ticks: 960 * 16,
                pitch: if id == "P2" { 60 } else { 48 },
                velocity: 80,
                staff: 1,
                voice: "1".into(),
            }],
        })
        .collect();
    (
        settings,
        Composition {
            title: "Generated VST3 integration fixture".into(),
            parts,
            tempos: vec![],
            duration_ticks: 960 * 16,
            warnings: vec![],
            page_map: None,
        },
        source,
    )
}

#[test]
#[ignore = "Requires SDK-enabled RMS_TEST_AUDIO_HOST, generated RMS_TEST_VST3_BUNDLE and running PipeWire; master stays muted"]
fn actual_vst3_two_instances_restore_paused_parameter_state_without_soundfont() {
    let (audio, score, source) = setup();
    let mut sources = BTreeMap::from([("P1".into(), source.clone()), ("P2".into(), source)]);
    let settings = omr::Settings {
        soundfont: "/no-bank-required".into(),
        ..Default::default()
    };
    let transport = Transport::start_with_sources(
        settings.clone(),
        audio.clone(),
        score.clone(),
        MidiOptions::default(),
        false,
        0,
        sources.clone(),
    )
    .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    for (id, value) in [("P1", 0.2), ("P2", 0.8)] {
        transport
            .plugin(Request::Parameter {
                part_id: id.into(),
                param_id: 7,
                value,
            })
            .unwrap();
    }
    wait(&transport, |s| s.plugins.dirty_parts.len() == 2);
    let private = tempfile::tempdir().unwrap();
    transport
        .plugin(Request::Checkpoint {
            destination: private.path().join("states"),
        })
        .unwrap();
    let saved = wait(&transport, |s| s.plugins.checkpoint.is_some());
    for part in saved.plugins.checkpoint.unwrap().1 {
        let expected = if part.part_id == "P1" { 0.2 } else { 0.8 };
        let descriptor = part.component_state.as_ref().unwrap();
        let bytes = std::fs::read(&descriptor.path).unwrap();
        assert_eq!(
            f64::from_le_bytes(bytes.try_into().unwrap()),
            expected,
            "Paused capture must update actual processor, not only controller"
        );
        if let Source::Vst3 {
            component_state,
            controller_state,
            ..
        } = sources.get_mut(&part.part_id).unwrap()
        {
            *component_state = part.component_state;
            *controller_state = part.controller_state;
        }
    }
    drop(transport);
    let restored = Transport::start_with_sources(
        settings,
        audio,
        score,
        MidiOptions::default(),
        false,
        0,
        sources,
    )
    .unwrap();
    wait(&restored, |s| s.state == State::Stopped);
    for (id, expected) in [("P1", 0.2), ("P2", 0.8)] {
        restored
            .plugin(Request::Parameters {
                part_id: id.into(),
                offset: 0,
            })
            .unwrap();
        let snapshot = wait(&restored, |s| s.plugins.parameters.contains_key(id));
        assert_eq!(snapshot.plugins.parameters[id].2[0].value, expected);
    }
    restored.play().unwrap();
    let playing = wait(&restored, |s| {
        s.state == State::Playing && s.position > 0.05
    });
    let prior_latency = playing.audio.unwrap().latency_frames;
    restored
        .plugin(Request::Parameter {
            part_id: "P2".into(),
            param_id: 8,
            value: 1.0,
        })
        .unwrap();
    let paused = wait(&restored, |s| {
        s.state == State::Paused
            && s.audio
                .as_ref()
                .is_some_and(|audio| audio.latency_frames >= prior_latency + 16)
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(paused.position, restored.snapshot().position);
    restored.stop().unwrap();
    let end = wait(&restored, |s| {
        s.state == State::Stopped && s.position == 0.0
    });
    println!("Real VST3 two-instance checkpoint/restore passed at {} Hz, {} latency frames, {} xruns (muted)", end.audio.as_ref().unwrap().sample_rate, end.audio.as_ref().unwrap().latency_frames, end.audio.as_ref().unwrap().xruns);
}

#[test]
#[ignore = "Requires SDK-enabled helper, generated fixture, real SoundFont and PipeWire; master stays muted"]
fn actual_vst3_and_soundfont_share_a_live_transport() {
    let (audio, score, source) = setup();
    let mut settings = omr::Settings::default();
    if let Ok(bank) = std::env::var("RMS_TEST_SOUNDFONT") {
        settings.soundfont = bank;
    }
    let sources = BTreeMap::from([("P1".into(), source), ("P2".into(), Source::Soundfont)]);
    let transport = Transport::start_with_sources(
        settings,
        audio,
        score,
        MidiOptions::default(),
        false,
        0,
        sources,
    )
    .unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 0.1
    });
    transport.mix("P2", false, 64, 0).unwrap();
    transport.mix("P2", true, 64, 0).unwrap();
    transport.tempo(0.5).unwrap();
    wait(&transport, |s| s.duration == 16.0);
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(1.0).unwrap();
    wait(&transport, |s| s.state == State::Paused && s.position > 1.0);
    transport.stop().unwrap();
    let end = wait(&transport, |s| {
        s.state == State::Stopped && s.position == 0.0
    });
    println!(
        "Mixed real VST3/SoundFont transport passed at {} Hz (muted)",
        end.audio.as_ref().unwrap().sample_rate
    );
}
