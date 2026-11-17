use super::*;
use crate::omr::linux::ManagedProcess;
use crossbeam_channel::Receiver;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::Command as ProcessCommand,
    time::{Duration, Instant},
};

fn silence(input: &mut impl Write, tracks: &[Track]) -> Result<()> {
    for track in tracks {
        writeln!(input, "cc {} 120 0", track.channel)?;
    }
    input.flush()?;
    Ok(())
}
fn configure(input: &mut impl Write, track: &Track) -> Result<()> {
    writeln!(input, "prog {} {}", track.channel, track.program)?;
    writeln!(
        input,
        "cc {} 7 {}",
        track.channel,
        if track.enabled { track.volume } else { 0 }
    )?;
    input.flush()?;
    Ok(())
}
fn note(input: &mut impl Write, event: &Event) -> Result<()> {
    if event.velocity == 0 {
        writeln!(input, "noteoff {} {}", event.channel, event.pitch)?;
    } else {
        writeln!(
            input,
            "noteon {} {} {}",
            event.channel, event.pitch, event.velocity
        )?;
    }
    Ok(())
}

pub(super) fn run(
    settings: Settings,
    mut timeline: Timeline,
    mut scale: f64,
    commands: Receiver<super::Command>,
    view: &Arc<Mutex<Snapshot>>,
    cancelled: &AtomicBool,
    slot: Arc<Mutex<Option<u32>>>,
) -> Result<()> {
    let bank = Path::new(&settings.soundfont)
        .canonicalize()
        .context("Choose an existing SoundFont")?;
    ensure!(bank.is_file(), "SoundFont is not a regular file");
    let mut header = [0; 12];
    File::open(&bank)?.read_exact(&mut header)?;
    ensure!(
        &header[..4] == b"RIFF" && &header[8..] == b"sfbk",
        "Invalid SF2/SF3 SoundFont"
    );
    let private = tempfile::tempdir()?;
    let config = private.path().join("empty.conf");
    File::create(&config)?;
    let mut registration = slot.lock().unwrap_or_else(|e| e.into_inner());
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut process = ManagedProcess::interactive(
        ProcessCommand::new(&settings.fluidsynth_executable)
            .arg("-n")
            .arg("-f")
            .arg(config)
            .arg(bank),
        slot.clone(),
    )?;
    *registration = Some(process.id());
    drop(registration);
    let mut input = process.input()?;
    // Keep individual CC7 levels unchanged; 100% matches normal synth gain.
    writeln!(
        input,
        "gain {:.6}",
        0.2 * f64::from(timeline.master_volume) / 100.0
    )?;
    // A marker confirms that startup/configuration completed before the clock starts.
    for track in &timeline.tracks {
        configure(&mut input, track)?;
    }
    let marker = "RMS_TRANSPORT_READY";
    writeln!(input, "echo {marker}")?;
    input.flush()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        ensure!(process.poll()?, "Synth exited during startup");
        if process.output()?.contains(marker) {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Synth command interface did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut position, mut state) = (
        0.0,
        if timeline.autoplay {
            State::Playing
        } else {
            State::Stopped
        },
    );
    let mut last = Instant::now();
    if state == State::Playing {
        for event in timeline.chase(position) {
            note(&mut input, &event)?;
        }
    }
    let mut cursor = timeline.events.partition_point(|e| e.at <= position);
    input.flush()?;
    let mut health = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if health.elapsed() >= Duration::from_millis(100) {
            ensure!(process.poll()?, "Synth process exited unexpectedly");
            health = Instant::now();
        }
        let now = Instant::now();
        if state == State::Playing {
            position += (now - last).as_secs_f64() * scale;
        }
        last = now;
        // Drain only the bounded queue. Coalesce slider updates per track/tempo
        // while preserving transport actions in order.
        let pending: Vec<_> = commands.try_iter().collect();
        let mut mixes = BTreeMap::new();
        let mut tempo = None;
        let mut master = None;
        for command in pending {
            match command {
                super::Command::Plugin(_) => {
                    anyhow::bail!("VST3 commands require the audio-host backend")
                }
                super::Command::Mix(id, enabled, volume, program) => {
                    mixes.insert(id, (enabled, volume, program));
                }
                super::Command::Tempo(value) => tempo = Some(value),
                super::Command::MasterVolume(percent) => master = Some(percent),
                super::Command::Pause => {
                    silence(&mut input, &timeline.tracks)?;
                    state = State::Paused;
                }
                super::Command::Stop => {
                    silence(&mut input, &timeline.tracks)?;
                    position = 0.0;
                    cursor = 0;
                    state = State::Stopped;
                }
                super::Command::Play => {
                    if state != State::Playing {
                        if position >= timeline.duration {
                            position = 0.0;
                        }
                        for event in timeline.chase(position) {
                            note(&mut input, &event)?;
                        }
                        cursor = timeline.events.partition_point(|e| e.at <= position);
                        state = State::Playing;
                    }
                }
                super::Command::Seek(delta) => {
                    silence(&mut input, &timeline.tracks)?;
                    position = (position + delta * scale).clamp(0.0, timeline.duration);
                    cursor = timeline.events.partition_point(|e| e.at <= position);
                    if state == State::Playing {
                        for event in timeline.chase(position) {
                            note(&mut input, &event)?;
                        }
                    } else if state == State::Finished {
                        state = State::Paused;
                    }
                }
            }
        }
        if let Some(value) = tempo {
            scale = value;
        }
        if let Some(percent) = master {
            writeln!(input, "gain {:.6}", 0.2 * f64::from(percent) / 100.0)?;
        }
        for (id, (enabled, volume, program)) in mixes {
            let track = timeline
                .tracks
                .iter_mut()
                .find(|t| t.id == id)
                .context("Unknown synth part")?;
            let chase = state == State::Playing
                && ((!track.enabled && enabled)
                    || track.program != program
                    || (track.volume == 0 && volume > 0));
            if chase {
                writeln!(input, "cc {} 120 0", track.channel)?;
            }
            track.enabled = enabled;
            track.volume = volume;
            track.program = program;
            let channel = track.channel;
            configure(&mut input, track)?;
            if chase {
                for event in timeline
                    .chase(position)
                    .iter()
                    .filter(|e| e.channel == channel)
                {
                    note(&mut input, event)?;
                }
            }
        }
        if state == State::Playing {
            while let Some(event) = timeline.events.get(cursor).filter(|e| e.at <= position) {
                note(&mut input, event)?;
                cursor += 1;
            }
            if position >= timeline.duration {
                silence(&mut input, &timeline.tracks)?;
                position = timeline.duration;
                state = State::Finished;
            }
        }
        input.flush()?;
        *view.lock().unwrap_or_else(|e| e.into_inner()) = Snapshot {
            state,
            position: position.min(timeline.duration) / scale,
            duration: timeline.duration / scale,
            error: None,
            source_page: timeline.source_page(position.min(timeline.duration)),
            score_tick: timeline.score_tick(position.min(timeline.duration)),
            audio: None,
            plugins: Default::default(),
        };
        // ~5ms scheduling resolution; audio buffers add their own device latency.
        std::thread::sleep(Duration::from_millis(5));
    }
}
