use super::*;
use crate::notation::{Note, Part, Tempo};

#[test]
#[ignore = "Requires real FluidSynth audio and RMS_TEST_MUSICXML with original-page metadata"]
#[cfg(target_os = "linux")]
fn real_synth_page_follow_smoke() {
    let path = std::env::var_os("RMS_TEST_MUSICXML").expect("Set RMS_TEST_MUSICXML");
    let score = crate::notation::load(std::path::Path::new(&path)).unwrap();
    let timeline = Timeline::new(&score, &MidiOptions::default()).unwrap();
    let map = score
        .page_map
        .clone()
        .expect("Original-page metadata required");
    let pages = timeline
        .pages
        .windows(2)
        .find(|p| p[0].1 != p[1].1)
        .expect("Need two original pages");
    let (boundary, first, second) = (pages[1].0, pages[0].1, pages[1].1);
    let transition = map.turns.iter().find(|turn| turn.page == second).unwrap();
    let measure = map.measure_ticks.binary_search(&transition.tick).unwrap();
    let preview_tick = map.measure_ticks[measure - 1];
    let index = timeline
        .tempo_points
        .partition_point(|(tick, _, _)| *tick <= preview_tick)
        .saturating_sub(1);
    let (tick, seconds, bpm) = timeline.tempo_points[index];
    let preview_seconds =
        seconds + (preview_tick - tick) as f64 / f64::from(TICKS_PER_QUARTER) * 60.0 / bpm;
    assert_eq!(map.page_at(preview_tick, 0), Some(first));
    assert_eq!(map.page_at(preview_tick, 1), Some(second));
    println!("One-measure read-ahead: {preview_seconds:.3}s, actual page boundary: {boundary:.3}s");
    assert!(boundary > 0.1);
    let mut reader = crate::reader::Reader::default();
    reader.load(
        1,
        *score
            .page_map
            .as_ref()
            .unwrap()
            .source_pages
            .iter()
            .max()
            .unwrap()
            + 1,
        first,
    );
    let transport =
        Transport::start_paused(Settings::default(), score, MidiOptions::default()).unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(boundary - 0.05).unwrap();
    let before = wait(&transport, |s| {
        s.state == State::Paused && s.position >= boundary - 0.06 && s.source_page == Some(first)
    });
    assert!(reader.follow_page(map.page_at(before.score_tick, 1).unwrap()));
    assert_eq!(
        reader.page, second,
        "Reader previews before the synth reaches the next page"
    );
    transport.play().unwrap();
    let snapshot = wait(&transport, |s| {
        s.state == State::Playing && s.source_page == Some(second)
    });
    assert!(
        !reader.follow_page(map.page_at(snapshot.score_tick, 1).unwrap()),
        "Preview must not bounce at the actual page boundary"
    );
    assert_eq!(reader.page, second);
    transport.seek(-(boundary - preview_seconds + 0.5)).unwrap();
    let snapshot = wait(&transport, |s| s.source_page == Some(first));
    assert!(reader.follow_page(snapshot.source_page.unwrap()));
    assert_eq!(reader.page, first);
    transport.pause().unwrap();
    let frozen = wait(&transport, |s| s.state == State::Paused);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(transport.snapshot().position, frozen.position);
    assert_eq!(transport.snapshot().source_page, Some(first));
}

#[test]
#[cfg(target_os = "linux")]
fn source_pages_use_the_transport_clock_across_pauses_seeks_and_tempo_changes() {
    use crate::notation::{PageMap, PageTurn};
    let mut c = score();
    c.page_map = Some(PageMap {
        source_pages: vec![0, 1],
        turns: vec![
            PageTurn { tick: 0, page: 0 },
            PageTurn {
                tick: 960 * 8,
                page: 1,
            },
            PageTurn {
                tick: 960 * 12,
                page: 0,
            },
        ],
        measure_ticks: (0..32).map(|n| n * 960).collect(),
    });
    let timeline = Timeline::new(&c, &MidiOptions::default()).unwrap();
    assert_eq!(timeline.source_page(3.99), Some(0));
    assert_eq!(timeline.source_page(4.0), Some(1));
    assert_eq!(timeline.source_page(8.0), Some(0));
    let root = tempfile::tempdir().unwrap();
    let transport = Transport::start_paused(fake(root.path()), c, MidiOptions::default()).unwrap();
    wait(&transport, |s| {
        s.state == State::Stopped && s.source_page == Some(0)
    });
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(5.0).unwrap();
    wait(&transport, |s| {
        s.position == 5.0 && s.source_page == Some(1)
    });
    assert_eq!(transport.snapshot().score_tick, 9 * 960);
    transport.tempo(0.5).unwrap();
    wait(&transport, |s| {
        s.position == 10.0 && s.source_page == Some(1)
    });
    assert_eq!(transport.snapshot().score_tick, 9 * 960);
    transport.seek(8.0).unwrap();
    wait(&transport, |s| {
        s.position == 18.0 && s.source_page == Some(0)
    });
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(transport.snapshot().source_page, Some(0));
    transport.stop().unwrap();
    wait(&transport, |s| {
        s.state == State::Stopped && s.position == 0.0
    });
}

#[test]
#[cfg(target_os = "linux")]
fn master_volume_changes_gain_without_changing_part_balance_or_position() {
    let root = tempfile::tempdir().unwrap();
    let mut options = MidiOptions::default();
    options.volumes.insert("P1".into(), 32);
    options.volumes.insert("P2".into(), 96);
    let settings = fake(root.path());
    assert!(
        Transport::start_with_master(settings.clone(), score(), options.clone(), false, 101)
            .is_err()
    );
    let transport = Transport::start_with_master(settings, score(), options, false, 0).unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    let log_path = root.path().join("commands");
    let initial = std::fs::read_to_string(&log_path).unwrap();
    assert!(initial.starts_with("gain 0.000000\n"), "{initial}");
    assert!(initial.contains("cc 0 7 32\n") && initial.contains("cc 1 7 96\n"));
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 0.03
    });
    transport.master_volume(50).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !std::fs::read_to_string(&log_path)
        .unwrap()
        .contains("gain 0.100000\n")
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let position = transport.snapshot().position;
    transport.master_volume(100).unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > position + 0.02
    });
    assert!(transport.master_volume(101).is_err());
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("gain 0.200000\n"));
    assert_eq!(log.matches("cc 0 7 ").count(), 1);
    assert_eq!(log.matches("cc 1 7 ").count(), 1);
}

fn score() -> Composition {
    Composition {
        title: "Generated transport fixture".into(),
        parts: vec![
            Part {
                id: "P1".into(),
                name: "Cello".into(),
                program: 42,
                staves: 1,
                notes: vec![Note {
                    start_tick: 0,
                    duration_ticks: 960 * 32,
                    pitch: 48,
                    velocity: 80,
                    staff: 1,
                    voice: "1".into(),
                }],
            },
            Part {
                id: "P2".into(),
                name: "Piano".into(),
                program: 0,
                staves: 2,
                notes: vec![Note {
                    start_tick: 960,
                    duration_ticks: 960 * 16,
                    pitch: 60,
                    velocity: 80,
                    staff: 2,
                    voice: "2".into(),
                }],
            },
        ],
        tempos: vec![
            Tempo {
                tick: 0,
                bpm: 120.0,
            },
            Tempo {
                tick: 960 * 8,
                bpm: 60.0,
            },
        ],
        duration_ticks: 960 * 32,
        warnings: vec![],
        page_map: None,
    }
}

#[test]
#[cfg(target_os = "linux")]
fn dropping_during_startup_kills_the_command_helper() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake(root.path());
    std::fs::write(&settings.fluidsynth_executable, "#!/bin/sh\nsleep 60\n").unwrap();
    let transport = Transport::start(settings, score(), MidiOptions::default()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let pid = loop {
        if let Some(pid) = *transport.process.lock().unwrap() {
            break pid;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(transport.snapshot().state, State::Starting);
    drop(transport);
    while let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat.contains(") Z ") {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn seeking_before_play_is_silent_and_tempo_changes_keep_musical_position() {
    let root = tempfile::tempdir().unwrap();
    let transport =
        Transport::start_paused(fake(root.path()), score(), MidiOptions::default()).unwrap();
    wait(&transport, |s| s.state == State::Stopped);
    let log = root.path().join("commands");
    assert!(!std::fs::read_to_string(&log).unwrap().contains("noteon"));
    transport.seek(5.0).unwrap();
    wait(&transport, |s| s.position == 5.0);
    assert!(!std::fs::read_to_string(&log).unwrap().contains("noteon"));
    transport.tempo(0.5).unwrap();
    wait(&transport, |s| s.position == 10.0 && s.duration == 56.0);
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 10.01
    });
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    assert!(transport.seek(f64::NAN).is_err());
}

#[test]
fn timeline_respects_tempos_staves_and_held_notes() {
    let timeline = Timeline::new(&score(), &MidiOptions::default()).unwrap();
    assert_eq!(timeline.duration, 28.0);
    assert_eq!(timeline.tracks.len(), 2);
    assert_eq!(timeline.events[1].at, 0.5);
    assert_eq!(timeline.chase(5.0).len(), 2);
    assert!(timeline.chase(28.0).is_empty());
}

#[test]
fn channels_are_stable_when_a_part_is_muted_and_skip_percussion() {
    let mut score = score();
    for n in 2..15 {
        let mut part = score.parts[0].clone();
        part.id = format!("P{}", n + 1);
        score.parts.push(part);
    }
    let options = MidiOptions {
        selected_parts: Some(vec!["P2".into()]),
        ..Default::default()
    };
    let timeline = Timeline::new(&score, &options).unwrap();
    assert!(!timeline.tracks[0].enabled);
    assert!(timeline.tracks[1].enabled);
    assert_eq!(timeline.tracks[1].channel, 1);
    assert!(!timeline.tracks.iter().any(|t| t.channel == 9));
}

#[cfg(target_os = "linux")]
fn fake(root: &std::path::Path) -> Settings {
    use std::os::unix::fs::PermissionsExt;
    let helper = root.join("fake synth");
    let log = root.join("commands");
    std::fs::write(&helper,format!(
        "#!/bin/sh\nwhile IFS= read -r line; do\nprintf '%s\\n' \"$line\" >> '{}'\ncase \"$line\" in 'echo RMS_TRANSPORT_READY') printf 'RMS_TRANSPORT_READY\\n';; esac\ndone\n",
        log.display()
    )).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bank = root.join("bank.sf2");
    std::fs::write(&bank, b"RIFF0000sfbk").unwrap();
    Settings {
        fluidsynth_executable: helper.display().to_string(),
        soundfont: bank.display().to_string(),
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn wait(transport: &Transport, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let snapshot = transport.snapshot();
        assert_ne!(snapshot.state, State::Failed, "{:?}", snapshot.error);
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Transport timed out: {snapshot:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn pause_seek_live_volume_resume_and_stop_preserve_the_process() {
    let root = tempfile::tempdir().unwrap();
    let transport = Transport::start(fake(root.path()), score(), MidiOptions::default()).unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 0.03
    });
    let pid = *transport.process.lock().unwrap();
    transport.pause().unwrap();
    let frozen = wait(&transport, |s| s.state == State::Paused).position;
    std::thread::sleep(std::time::Duration::from_millis(60));
    assert_eq!(transport.snapshot().position, frozen);
    transport.seek(5.0).unwrap();
    wait(&transport, |s| {
        s.state == State::Paused && s.position >= frozen + 4.99
    });
    transport.mix("P1", false, 64, 42).unwrap();
    transport.mix("P2", true, 32, 0).unwrap();
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > frozen + 5.01
    });
    transport.seek(-999.0).unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position < 1.0
    });
    transport.pause().unwrap();
    wait(&transport, |s| s.state == State::Paused);
    transport.seek(999.0).unwrap();
    wait(&transport, |s| {
        s.position == s.duration && s.state == State::Paused
    });
    transport.stop().unwrap();
    wait(&transport, |s| {
        s.position == 0.0 && s.state == State::Stopped
    });
    assert_eq!(*transport.process.lock().unwrap(), pid);
    let log = std::fs::read_to_string(root.path().join("commands")).unwrap();
    assert!(log.contains("cc 0 120 0"));
    assert!(log.contains("cc 0 7 0"));
    assert!(log.contains("cc 1 7 32"));
    assert!(
        log.matches("noteon 0 48 80").count() >= 2,
        "Held cello note wasn't reconstructed: {log}"
    );
    drop(transport);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while let Some(pid) = pid {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        if stat.is_err() || stat.unwrap().contains(") Z ") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Synth wasn't stopped on drop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn natural_completion_and_invalid_engine_are_reported() {
    let root = tempfile::tempdir().unwrap();
    let mut short = score();
    short.duration_ticks = 96;
    for p in &mut short.parts {
        p.notes.clear();
    }
    let transport = Transport::start(fake(root.path()), short, MidiOptions::default()).unwrap();
    wait(&transport, |s| s.state == State::Finished);
    let mut bad = fake(root.path());
    bad.fluidsynth_executable = "/no/such/synth".into();
    let transport = Transport::start(bad, score(), MidiOptions::default()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while transport.snapshot().state != State::Failed {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(transport
        .snapshot()
        .error
        .unwrap()
        .contains("Cannot start helper"));
}

#[test]
#[ignore = "Requires a real FluidSynth sound device and RMS_TEST_MUSICXML"]
#[cfg(target_os = "linux")]
fn real_synth_transport_smoke() {
    let path = std::env::var_os("RMS_TEST_MUSICXML").expect("Set RMS_TEST_MUSICXML");
    let score = crate::notation::load(std::path::Path::new(&path)).unwrap();
    let mut options = MidiOptions::default();
    options.programs.insert("P1".into(), 42);
    let transport = Transport::start(Settings::default(), score, options).unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > 0.2
    });
    transport.pause().unwrap();
    let frozen = wait(&transport, |s| s.state == State::Paused).position;
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(transport.snapshot().position, frozen);
    transport.seek(10.0).unwrap();
    wait(&transport, |s| s.state == State::Paused && s.position > 9.9);
    transport.mix("P1", true, 32, 42).unwrap();
    transport.mix("P2", false, 127, 0).unwrap();
    transport.play().unwrap();
    wait(&transport, |s| {
        s.state == State::Playing && s.position > frozen + 10.2
    });
    transport.stop().unwrap();
    wait(&transport, |s| {
        s.state == State::Stopped && s.position == 0.0
    });
}
