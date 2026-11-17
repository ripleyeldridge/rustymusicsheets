//! Part-addressed musical data; deliberately independent of MIDI file channels.
use crate::notation::{Composition, MidiOptions, TICKS_PER_QUARTER};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PLAN_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PARTS: usize = 32;
const MAX_TICKS: u64 = 960 * 60 * 60 * 24;
const MAX_EVENTS: usize = 250_000;

#[derive(Debug, Serialize)]
pub struct Plan {
    pub version: u32,
    pub ticks_per_quarter: u16,
    pub duration_ticks: u64,
    pub tempos: Vec<Tempo>,
    pub parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
pub struct Tempo {
    pub tick: u64,
    pub bpm: f64,
}

#[derive(Debug, Serialize)]
pub struct Part {
    pub id: String,
    pub enabled: bool,
    pub volume: u8,
    pub program: u8,
    pub source: super::plugins::Source,
    pub notes: Vec<Note>,
}

#[derive(Debug, Serialize)]
pub struct Note {
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub pitch: u8,
    pub velocity: u8,
}

/// Validate the whole graph, including initially muted parts that can be enabled live.
pub fn compile(score: &Composition, options: &MidiOptions) -> Result<Plan> {
    compile_with_sources(score, options, &BTreeMap::new())
}

pub fn compile_with_sources(
    score: &Composition,
    options: &MidiOptions,
    sources: &BTreeMap<String, super::plugins::Source>,
) -> Result<Plan> {
    ensure!(
        !score.parts.is_empty() && score.parts.len() <= MAX_PARTS,
        "Audio host supports 1–32 parts"
    );
    ensure!(
        score.duration_ticks <= MAX_TICKS,
        "Score timing exceeds limit"
    );
    ensure!(
        options.tempo_scale.is_finite() && (0.1..=4.0).contains(&options.tempo_scale),
        "Invalid tempo scale"
    );
    let ids: BTreeSet<_> = score.parts.iter().map(|part| part.id.as_str()).collect();
    ensure!(ids.len() == score.parts.len(), "Duplicate part IDs");
    ensure!(
        sources.len() <= MAX_PARTS && sources.keys().all(|id| ids.contains(id.as_str())),
        "Unknown plugin part"
    );
    for source in sources.values() {
        source.validate()?;
    }
    ensure!(
        ids.iter()
            .all(|id| !id.is_empty() && id.len() <= 256 && !id.contains('\0')),
        "Invalid part ID"
    );
    if let Some(selected) = &options.selected_parts {
        ensure!(
            !selected.is_empty() && selected.len() <= MAX_PARTS,
            "Select at least one part"
        );
        ensure!(
            selected.iter().all(|id| ids.contains(id.as_str())),
            "Unknown selected part"
        );
    }
    ensure!(
        options.programs.len() <= MAX_PARTS && options.volumes.len() <= MAX_PARTS,
        "Too many part overrides"
    );
    ensure!(
        options
            .programs
            .iter()
            .all(|(id, value)| ids.contains(id.as_str()) && *value <= 127),
        "Invalid program override"
    );
    ensure!(
        options
            .volumes
            .iter()
            .all(|(id, value)| ids.contains(id.as_str()) && *value <= 127),
        "Invalid volume override"
    );
    let count = score
        .parts
        .iter()
        .try_fold(score.tempos.len(), |count, part| {
            count.checked_add(part.notes.len())
        })
        .context("Event count overflow")?;
    ensure!(count <= MAX_EVENTS, "Too many audio events");
    let mut tempos = BTreeMap::from([(0, 120.0)]);
    for tempo in &score.tempos {
        ensure!(
            tempo.tick <= MAX_TICKS
                && tempo.bpm.is_finite()
                && tempo.bpm > 0.0
                && tempo.bpm <= 60_000.0,
            "Invalid tempo"
        );
        tempos.insert(tempo.tick, tempo.bpm);
    }
    let mut plan = Plan {
        version: 2,
        ticks_per_quarter: TICKS_PER_QUARTER,
        duration_ticks: score.duration_ticks,
        tempos: tempos
            .into_iter()
            .map(|(tick, bpm)| Tempo { tick, bpm })
            .collect(),
        parts: Vec::with_capacity(score.parts.len()),
    };
    for part in &score.parts {
        let program = *options.programs.get(&part.id).unwrap_or(&part.program);
        ensure!(program <= 127, "Invalid instrument program");
        let mut pitches: BTreeMap<u8, Vec<(u64, u64, u8)>> = BTreeMap::new();
        for note in &part.notes {
            let end = note
                .start_tick
                .checked_add(note.duration_ticks)
                .context("Note timing overflow")?;
            ensure!(
                note.pitch <= 127
                    && (1..=127).contains(&note.velocity)
                    && note.duration_ticks > 0
                    && end <= MAX_TICKS,
                "Invalid audio note"
            );
            plan.duration_ticks = plan.duration_ticks.max(end);
            pitches
                .entry(note.pitch)
                .or_default()
                .push((note.start_tick, end, note.velocity));
        }
        let mut notes = Vec::new();
        for (pitch, mut spans) in pitches {
            spans.sort_unstable();
            let mut merged: Vec<(u64, u64, u8)> = Vec::new();
            for (start, end, velocity) in spans {
                if let Some(last) = merged.last_mut() {
                    if start < last.1 {
                        last.1 = last.1.max(end);
                        last.2 = last.2.max(velocity);
                        continue;
                    }
                }
                merged.push((start, end, velocity));
            }
            notes.extend(merged.into_iter().map(|(start, end, velocity)| Note {
                start_tick: start,
                duration_ticks: end - start,
                pitch,
                velocity,
            }));
        }
        notes.sort_unstable_by_key(|note| (note.start_tick, note.pitch));
        plan.parts.push(Part {
            id: part.id.clone(),
            enabled: options
                .selected_parts
                .as_ref()
                .is_none_or(|ids| ids.contains(&part.id)),
            volume: *options.volumes.get(&part.id).unwrap_or(&127),
            program,
            source: sources.get(&part.id).cloned().unwrap_or_default(),
            notes,
        });
    }
    plan.tempos
        .retain(|tempo| tempo.tick <= plan.duration_ticks);
    ensure!(
        plan.tempos.len()
            + plan
                .parts
                .iter()
                .map(|part| part.notes.len())
                .sum::<usize>()
            <= MAX_EVENTS,
        "Too many audio events including default tempo"
    );
    ensure!(
        plan.seconds_at(plan.duration_ticks).is_finite()
            && plan.seconds_at(plan.duration_ticks) <= 864_000.0,
        "Audio plan duration exceeds limit"
    );
    Ok(plan)
}

impl Plan {
    pub fn seconds_at(&self, tick: u64) -> f64 {
        let mut seconds = 0.0;
        for (index, tempo) in self.tempos.iter().enumerate() {
            if tick < tempo.tick {
                break;
            }
            let end = self
                .tempos
                .get(index + 1)
                .map_or(tick, |next| next.tick.min(tick));
            seconds +=
                (end - tempo.tick) as f64 / f64::from(self.ticks_per_quarter) * 60.0 / tempo.bpm;
            if end == tick {
                break;
            }
        }
        seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score() -> Composition {
        crate::notation::parse_musicxml(r#"<score-partwise><part-list><score-part id="P1"><part-name>Piano</part-name></score-part></part-list><part id="P1"><measure number="1"><attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>4</duration></note></measure></part></score-partwise>"#).unwrap()
    }

    #[test]
    fn independent_parts_exceed_midi_channel_count_without_changing_export() {
        let mut score = score();
        score.parts = (0..32)
            .map(|index| {
                let mut part = score.parts[0].clone();
                part.id = format!("P{index}");
                part
            })
            .collect();
        assert_eq!(
            compile(&score, &MidiOptions::default())
                .unwrap()
                .parts
                .len(),
            32
        );
        assert!(crate::notation::to_midi(&score, &MidiOptions::default()).is_err());
        score.parts.push(score.parts[0].clone());
        assert!(compile(&score, &MidiOptions::default()).is_err());
    }

    #[test]
    fn merges_only_overlapping_unisons_and_preserves_tempo_positions() {
        let mut score = score();
        let mut note = score.parts[0].notes[0].clone();
        note.start_tick = 960;
        note.velocity = 100;
        score.parts[0].notes.push(note);
        score.tempos = vec![crate::notation::Tempo {
            tick: 1920,
            bpm: 60.0,
        }];
        let plan = compile(&score, &MidiOptions::default()).unwrap();
        assert_eq!(plan.parts[0].notes.len(), 1);
        assert_eq!(plan.parts[0].notes[0].duration_ticks, 4800);
        assert_eq!(plan.parts[0].notes[0].velocity, 100);
        assert_eq!(plan.seconds_at(1920), 1.0);
        assert_eq!(plan.seconds_at(3840), 3.0);
    }

    #[test]
    fn rejects_bad_identity_timing_muted_notes_and_overrides() {
        let score = score();
        let mut bad = score.clone();
        bad.parts[0].id.clear();
        assert!(compile(&bad, &MidiOptions::default()).is_err());
        bad = score.clone();
        bad.parts[0].notes[0].duration_ticks = u64::MAX;
        assert!(compile(&bad, &MidiOptions::default()).is_err());
        let mut options = MidiOptions::default();
        options.volumes.insert("missing".into(), 64);
        assert!(compile(&score, &options).is_err());
        bad = score.clone();
        bad.tempos.push(crate::notation::Tempo {
            tick: 0,
            bpm: f64::NAN,
        });
        assert!(compile(&bad, &MidiOptions::default()).is_err());
    }
}
