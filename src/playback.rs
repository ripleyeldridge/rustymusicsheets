//! Persistent, bounded FluidSynth transport. All subprocess I/O and sequencing
//! happens off the UI thread; the existing MIDI-file CLI player is independent.
use crate::{
    notation::{Composition, MidiOptions, TICKS_PER_QUARTER},
    omr::Settings,
};
use anyhow::{ensure, Context, Result};
use crossbeam_channel::{bounded, Sender};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Playing,
    Paused,
    Stopped,
    Finished,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub position: f64,
    pub duration: f64,
    pub error: Option<String>,
    pub source_page: Option<u32>,
    pub score_tick: u64,
    pub audio: Option<crate::audio::Diagnostics>,
    pub plugins: crate::audio::plugins::View,
}

#[derive(Debug)]
pub(crate) enum Command {
    Play,
    Pause,
    Stop,
    Seek(f64),
    Mix(String, bool, u8, u8),
    Tempo(f64),
    MasterVolume(u8),
    Plugin(crate::audio::plugins::Request),
}

pub struct Transport {
    commands: Sender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
    cancelled: Arc<AtomicBool>,
    process: Arc<Mutex<Option<u32>>>,
}

impl Transport {
    pub fn start(settings: Settings, score: Composition, options: MidiOptions) -> Result<Self> {
        Self::start_with_master(settings, score, options, true, 100)
    }
    pub fn start_paused(
        settings: Settings,
        score: Composition,
        options: MidiOptions,
    ) -> Result<Self> {
        Self::start_with_master(settings, score, options, false, 100)
    }
    pub fn start_with_master(
        settings: Settings,
        score: Composition,
        options: MidiOptions,
        autoplay: bool,
        master_volume: u8,
    ) -> Result<Self> {
        Self::start_with_backend(
            settings,
            crate::audio::Settings::default(),
            score,
            options,
            autoplay,
            master_volume,
        )
    }
    pub fn start_with_backend(
        settings: Settings,
        audio: crate::audio::Settings,
        score: Composition,
        options: MidiOptions,
        autoplay: bool,
        master_volume: u8,
    ) -> Result<Self> {
        Self::start_with_sources(
            settings,
            audio,
            score,
            options,
            autoplay,
            master_volume,
            BTreeMap::new(),
        )
    }
    pub fn start_with_sources(
        settings: Settings,
        audio: crate::audio::Settings,
        score: Composition,
        options: MidiOptions,
        autoplay: bool,
        master_volume: u8,
        sources: BTreeMap<String, crate::audio::plugins::Source>,
    ) -> Result<Self> {
        ensure!(master_volume <= 100, "Master volume must be 0–100%");
        ensure!(
            audio.backend != crate::audio::Backend::Legacy || !sources.values().any(|s| s.is_plugin()),
            "VST3 assignments require the audio-host backend; select SoundFont explicitly to use legacy playback"
        );
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (
                settings,
                audio,
                score,
                options,
                autoplay,
                master_volume,
                sources,
            );
            anyhow::bail!("Synth transport is currently available only on Linux");
        }
        #[cfg(target_os = "linux")]
        {
            let plan = if audio.backend == crate::audio::Backend::Legacy {
                None
            } else {
                Some(crate::audio::plan::compile_with_sources(
                    &score, &options, &sources,
                )?)
            };
            let timeline = if plan.is_none() {
                let mut timeline = Timeline::new(&score, &options)?;
                timeline.autoplay = autoplay;
                timeline.master_volume = master_volume;
                Some(timeline)
            } else {
                None
            };
            let duration = plan.as_ref().map_or_else(
                || timeline.as_ref().unwrap().duration,
                |plan| plan.seconds_at(plan.duration_ticks),
            );
            let pages = score.page_map;
            let (commands, rx) = bounded(128);
            let snapshot = Arc::new(Mutex::new(Snapshot {
                state: State::Starting,
                position: 0.0,
                duration: duration / options.tempo_scale,
                error: None,
                source_page: None,
                score_tick: 0,
                audio: None,
                plugins: Default::default(),
            }));
            let cancelled = Arc::new(AtomicBool::new(false));
            let process = Arc::new(Mutex::new(None));
            let (view, stop, slot) = (snapshot.clone(), cancelled.clone(), process.clone());
            std::thread::Builder::new()
                .name("synth-transport".into())
                .spawn(move || {
                    let result = if let Some(plan) = plan {
                        crate::audio::supervisor::run(
                            crate::audio::supervisor::Session {
                                audio,
                                settings,
                                plan,
                                pages,
                                autoplay,
                                tempo_scale: options.tempo_scale,
                                master_volume,
                            },
                            rx,
                            &view,
                            &stop,
                            slot,
                        )
                    } else {
                        linux::run(
                            settings,
                            timeline.unwrap(),
                            options.tempo_scale,
                            rx,
                            &view,
                            &stop,
                            slot,
                        )
                    };
                    if let Err(error) = result {
                        if !stop.load(Ordering::Relaxed) {
                            let mut state = view.lock().unwrap_or_else(|e| e.into_inner());
                            state.state = State::Failed;
                            state.error = Some(format!("{error:#}"));
                        }
                    }
                })?;
            Ok(Self {
                commands,
                snapshot,
                cancelled,
                process,
            })
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn send(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Synth transport is busy or has stopped")
    }
    pub fn play(&self) -> Result<()> {
        self.send(Command::Play)
    }
    pub fn pause(&self) -> Result<()> {
        self.send(Command::Pause)
    }
    pub fn stop(&self) -> Result<()> {
        self.send(Command::Stop)
    }
    pub fn seek(&self, seconds: f64) -> Result<()> {
        ensure!(
            seconds.is_finite() && seconds.abs() <= 86400.0,
            "Invalid seek"
        );
        self.send(Command::Seek(seconds))
    }
    pub fn mix(&self, part: &str, enabled: bool, volume: u8, program: u8) -> Result<()> {
        ensure!(volume <= 127 && program <= 127, "Invalid MIDI mix");
        self.send(Command::Mix(part.into(), enabled, volume, program))
    }
    pub fn tempo(&self, scale: f64) -> Result<()> {
        ensure!(
            scale.is_finite() && (0.1..=4.0).contains(&scale),
            "Invalid tempo scale"
        );
        self.send(Command::Tempo(scale))
    }
    pub fn master_volume(&self, percent: u8) -> Result<()> {
        ensure!(percent <= 100, "Master volume must be 0–100%");
        self.send(Command::MasterVolume(percent))
    }
    pub fn plugin(&self, request: crate::audio::plugins::Request) -> Result<()> {
        self.send(Command::Plugin(request))
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        #[cfg(target_os = "linux")]
        if let Ok(slot) = self.process.try_lock() {
            if let Some(group) = *slot {
                crate::omr::linux::kill_group(group);
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Track {
    id: String,
    channel: u8,
    enabled: bool,
    volume: u8,
    program: u8,
}
#[derive(Clone, Debug)]
struct Span {
    start: f64,
    end: f64,
    channel: u8,
    pitch: u8,
    velocity: u8,
}
#[derive(Clone, Debug)]
struct Event {
    at: f64,
    channel: u8,
    pitch: u8,
    velocity: u8,
}

struct Timeline {
    autoplay: bool,
    master_volume: u8,
    tracks: Vec<Track>,
    spans: Vec<Span>,
    events: Vec<Event>,
    duration: f64,
    pages: Vec<(f64, u32)>,
    tempo_points: Vec<(u64, f64, f64)>,
}

impl Timeline {
    fn new(score: &Composition, options: &MidiOptions) -> Result<Self> {
        // Reuse the MIDI export validation for bounded IDs, notes and timing.
        let _ = crate::notation::to_midi(score, options)?;
        ensure!(
            score.parts.len() <= 15,
            "Live mixing currently supports up to 15 parts in one movement"
        );
        let mut tempos: BTreeMap<u64, f64> = BTreeMap::from([(0, 120.0)]);
        for tempo in &score.tempos {
            tempos.insert(tempo.tick, tempo.bpm);
        }
        let mut points = Vec::new();
        let (mut last_tick, mut last_bpm, mut seconds) = (0u64, 120.0, 0.0);
        for (tick, bpm) in tempos {
            seconds += (tick - last_tick) as f64 / f64::from(TICKS_PER_QUARTER) * 60.0 / last_bpm;
            points.push((tick, seconds, bpm));
            last_tick = tick;
            last_bpm = bpm;
        }
        let at = |tick: u64| {
            let index = points
                .partition_point(|point| point.0 <= tick)
                .saturating_sub(1);
            let (start, seconds, bpm) = points[index];
            seconds + (tick - start) as f64 / f64::from(TICKS_PER_QUARTER) * 60.0 / bpm
        };
        let mut timeline = Self {
            autoplay: true,
            master_volume: 100,
            tracks: vec![],
            spans: vec![],
            events: vec![],
            duration: at(score.duration_ticks),
            pages: score
                .page_map
                .as_ref()
                .map(|map| {
                    map.turns
                        .iter()
                        .map(|turn| (at(turn.tick), turn.page))
                        .collect()
                })
                .unwrap_or_default(),
            tempo_points: Vec::new(),
        };
        let total_notes: usize = score.parts.iter().map(|p| p.notes.len()).sum();
        ensure!(total_notes <= 250_000, "Too many live playback notes");
        for (index, part) in score.parts.iter().enumerate() {
            let channel = if index < 9 {
                index as u8
            } else {
                index as u8 + 1
            };
            timeline.tracks.push(Track {
                id: part.id.clone(),
                channel,
                enabled: options
                    .selected_parts
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&part.id)),
                volume: *options.volumes.get(&part.id).unwrap_or(&127),
                program: *options.programs.get(&part.id).unwrap_or(&part.program),
            });
            let mut pitches: BTreeMap<u8, Vec<(u64, u64, u8)>> = BTreeMap::new();
            for note in &part.notes {
                let end = note
                    .start_tick
                    .checked_add(note.duration_ticks)
                    .context("Note timing overflow")?;
                ensure!(
                    note.pitch < 128
                        && note.velocity > 0
                        && note.velocity < 128
                        && note.duration_ticks > 0
                        && end <= 960 * 60 * 60 * 24,
                    "Invalid live note"
                );
                pitches
                    .entry(note.pitch)
                    .or_default()
                    .push((note.start_tick, end, note.velocity));
            }
            for (pitch, mut intervals) in pitches {
                intervals.sort_unstable();
                let mut merged: Vec<(u64, u64, u8)> = vec![];
                for (start, end, velocity) in intervals {
                    if let Some(last) = merged.last_mut() {
                        if start < last.1 {
                            last.1 = last.1.max(end);
                            last.2 = last.2.max(velocity);
                            continue;
                        }
                    }
                    merged.push((start, end, velocity));
                }
                for (start, end, velocity) in merged {
                    let span = Span {
                        start: at(start),
                        end: at(end),
                        channel,
                        pitch,
                        velocity,
                    };
                    timeline.duration = timeline.duration.max(span.end);
                    timeline.events.push(Event {
                        at: span.start,
                        channel,
                        pitch,
                        velocity,
                    });
                    timeline.events.push(Event {
                        at: span.end,
                        channel,
                        pitch,
                        velocity: 0,
                    });
                    timeline.spans.push(span);
                }
            }
        }
        timeline.tempo_points = points;
        timeline.events.sort_by(|a, b| {
            a.at.total_cmp(&b.at)
                .then((a.velocity != 0).cmp(&(b.velocity != 0)))
        });
        Ok(timeline)
    }
    fn score_tick(&self, position: f64) -> u64 {
        let index = self
            .tempo_points
            .partition_point(|(_, at, _)| *at <= position)
            .saturating_sub(1);
        let (tick, seconds, bpm) = self.tempo_points[index];
        (tick as f64 + (position - seconds).max(0.0) * bpm / 60.0 * f64::from(TICKS_PER_QUARTER))
            .floor() as u64
    }

    fn source_page(&self, position: f64) -> Option<u32> {
        self.pages
            .partition_point(|(at, _)| *at <= position)
            .checked_sub(1)
            .map(|index| self.pages[index].1)
    }

    fn chase(&self, position: f64) -> Vec<Event> {
        self.spans
            .iter()
            .filter(|n| n.start <= position && position < n.end)
            .map(|n| Event {
                at: position,
                channel: n.channel,
                pitch: n.pitch,
                velocity: n.velocity,
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(test)]
mod tests;
