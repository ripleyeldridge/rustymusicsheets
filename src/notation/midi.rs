use super::{Composition, MidiOptions, MAX_EVENTS, MAX_TICKS, TICKS_PER_QUARTER};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;

/// Serialize a type-1 Standard MIDI File. Note-offs precede note-ons at the same
/// tick. Each selected musical part owns one pitched channel (never channel 9).
/// Overlapping unisons in one part are coalesced so a voice's note-off cannot
/// prematurely silence another voice on that same MIDI channel.
pub fn to_midi(composition: &Composition, options: &MidiOptions) -> Result<Vec<u8>> {
    ensure!(
        options.tempo_scale.is_finite() && (0.1..=4.0).contains(&options.tempo_scale),
        "Tempo scale must be within 0.1..4.0"
    );
    ensure!(
        composition.duration_ticks <= MAX_TICKS,
        "Score timing exceeds limit"
    );
    ensure!(composition.parts.len() <= 128, "Too many score parts");
    ensure!(
        composition.tempos.len() <= MAX_EVENTS,
        "Too many tempo events"
    );
    if let Some(ids) = &options.selected_parts {
        ensure!(!ids.is_empty(), "Select at least one part");
        ensure!(ids.len() <= 128, "Too many selected IDs");
        for id in ids {
            ensure!(
                composition.parts.iter().any(|p| &p.id == id),
                "Unknown selected part: {id}"
            );
        }
    }
    for (id, volume) in &options.volumes {
        ensure!(
            composition.parts.iter().any(|p| &p.id == id),
            "Unknown volume part: {id}"
        );
        ensure!(*volume <= 127, "MIDI volume outside 0..127");
    }
    for (id, program) in &options.programs {
        ensure!(
            composition.parts.iter().any(|p| &p.id == id),
            "Unknown program override part: {id}"
        );
        ensure!(*program < 128, "MIDI program outside 0..127");
    }
    let selected: Vec<_> = composition
        .parts
        .iter()
        .filter(|p| {
            options
                .selected_parts
                .as_ref()
                .is_none_or(|ids| ids.contains(&p.id))
        })
        .collect();
    ensure!(!selected.is_empty(), "Select at least one part");
    ensure!(
        selected.len() <= 15,
        "MIDI playback supports at most 15 pitched parts at once"
    );
    let mut total = composition.tempos.len();
    for part in &selected {
        total = total
            .checked_add(part.notes.len())
            .context("MIDI event count overflow")?;
        ensure!(total <= MAX_EVENTS, "Too many MIDI events");
    }
    let mut output = Vec::new();
    output.extend_from_slice(b"MThd");
    output.extend_from_slice(&6u32.to_be_bytes());
    output.extend_from_slice(&1u16.to_be_bytes());
    output.extend_from_slice(&((selected.len() + 1) as u16).to_be_bytes());
    output.extend_from_slice(&TICKS_PER_QUARTER.to_be_bytes());
    let mut tempo_map = BTreeMap::new();
    tempo_map.insert(0, 120.0);
    for tempo in &composition.tempos {
        ensure!(
            tempo.tick <= MAX_TICKS && tempo.bpm.is_finite() && tempo.bpm > 0.0,
            "Invalid tempo"
        );
        tempo_map.insert(tempo.tick, tempo.bpm);
    }
    let mut tempo_events = Vec::new();
    tempo_events.push((0, 0u8, meta(0x03, composition.title.as_bytes())?));
    for (tick, bpm) in tempo_map {
        let micros = (60_000_000.0 / (bpm * options.tempo_scale)).round();
        ensure!(
            (1.0..=16_777_215.0).contains(&micros),
            "Scaled tempo is outside MIDI's representable range"
        );
        let bytes = (micros as u32).to_be_bytes();
        tempo_events.push((
            tick,
            1,
            vec![0xff, 0x51, 0x03, bytes[1], bytes[2], bytes[3]],
        ));
    }
    write_track(&mut output, tempo_events, composition.duration_ticks)?;
    for (index, part) in selected.iter().enumerate() {
        let channel = if index < 9 {
            index as u8
        } else {
            index as u8 + 1
        };
        let program = *options.programs.get(&part.id).unwrap_or(&part.program);
        ensure!(program < 128, "Invalid MIDI program");
        let mut events = vec![
            (0, 0, meta(0x03, part.name.as_bytes())?),
            (0, 1, vec![0xc0 | channel, program]),
        ];
        if let Some(volume) = options.volumes.get(&part.id) {
            events.push((0, 1, vec![0xb0 | channel, 7, *volume]));
        }
        let mut pitches: BTreeMap<u8, Vec<(u64, u64, u8)>> = BTreeMap::new();
        for note in &part.notes {
            ensure!(
                note.pitch < 128 && note.velocity > 0 && note.velocity < 128,
                "Invalid MIDI note data"
            );
            let end = note
                .start_tick
                .checked_add(note.duration_ticks)
                .context("Note timing overflow")?;
            ensure!(
                note.duration_ticks > 0 && end <= MAX_TICKS,
                "Note timing exceeds limit"
            );
            pitches
                .entry(note.pitch)
                .or_default()
                .push((note.start_tick, end, note.velocity));
        }
        for (pitch, mut intervals) in pitches {
            intervals.sort_unstable();
            let mut merged: Vec<(u64, u64, u8)> = Vec::new();
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
                events.push((start, 3, vec![0x90 | channel, pitch, velocity]));
                events.push((end, 2, vec![0x80 | channel, pitch, 0]));
            }
        }
        write_track(&mut output, events, composition.duration_ticks)?;
    }
    Ok(output)
}

fn meta(kind: u8, data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() <= 16_384, "MIDI text field exceeds limit");
    let mut event = vec![0xff, kind];
    variable(&mut event, data.len() as u64)?;
    event.extend_from_slice(data);
    Ok(event)
}

fn write_track(output: &mut Vec<u8>, mut events: Vec<(u64, u8, Vec<u8>)>, end: u64) -> Result<()> {
    events.sort_by_key(|event| (event.0, event.1));
    let mut data = Vec::new();
    let mut previous = 0u64;
    for (tick, _, bytes) in events {
        ensure!(tick <= MAX_TICKS, "MIDI timing exceeds limit");
        variable(&mut data, tick - previous)?;
        data.extend_from_slice(&bytes);
        previous = tick;
    }
    variable(&mut data, end.saturating_sub(previous))?;
    data.extend_from_slice(&[0xff, 0x2f, 0]);
    output.extend_from_slice(b"MTrk");
    output.extend_from_slice(
        &u32::try_from(data.len())
            .context("MIDI track too large")?
            .to_be_bytes(),
    );
    output.extend_from_slice(&data);
    Ok(())
}

fn variable(output: &mut Vec<u8>, mut value: u64) -> Result<()> {
    ensure!(value <= 0x0fff_ffff, "MIDI delta time exceeds limit");
    let mut bytes = [0u8; 4];
    let mut pos = 3;
    bytes[pos] = (value & 0x7f) as u8;
    value >>= 7;
    while value != 0 {
        pos -= 1;
        bytes[pos] = ((value & 0x7f) as u8) | 0x80;
        value >>= 7;
    }
    output.extend_from_slice(&bytes[pos..]);
    Ok(())
}
