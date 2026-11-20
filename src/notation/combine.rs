//! Explicit, reversible overlays of original parts on their existing timeline.
use super::{Composition, Note, Part, MAX_EVENTS, MAX_TICKS};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

const MAX_PARTS: usize = 128;
const MAX_TEXT_BYTES: usize = 256;

/// Members always refer to original IDs, including the retained target ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartGroup {
    pub target_id: String,
    pub members: Vec<String>,
    pub name: String,
}

/// Counts describe raw notes, before the existing playback unison policy.
/// Pair counts include only notes from different original member parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartGroupPreview {
    pub target_id: String,
    pub name: String,
    pub members: Vec<String>,
    pub note_count: usize,
    pub start_tick: Option<u64>,
    pub end_tick: Option<u64>,
    /// Positive-duration intersections; adjacent half-open intervals do not overlap.
    pub overlapping_note_pairs: u64,
    /// Intersections with equal MIDI pitch, regardless of voice/staff/velocity.
    pub overlapping_unison_pairs: u64,
    /// Equal values for every raw Note field, including voice/staff/velocity.
    pub exact_duplicate_pairs: u64,
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_TEXT_BYTES && !id.chars().any(char::is_control)
}

fn validate_original(original: &Composition) -> Result<BTreeMap<&str, usize>> {
    ensure!(
        !original.parts.is_empty() && original.parts.len() <= MAX_PARTS,
        "Invalid original part count"
    );
    ensure!(
        original.duration_ticks <= MAX_TICKS,
        "Score timing exceeds limit"
    );
    let mut ids = BTreeMap::new();
    let mut events = original.tempos.len();
    ensure!(events <= MAX_EVENTS, "Too many score events");
    for (index, part) in original.parts.iter().enumerate() {
        ensure!(valid_id(&part.id), "Invalid original part ID");
        ensure!(
            ids.insert(part.id.as_str(), index).is_none(),
            "Duplicate original part ID"
        );
        ensure!(
            part.program <= 127 && (1..=128).contains(&part.staves),
            "Invalid original part metadata"
        );
        events = events
            .checked_add(part.notes.len())
            .context("Score event count overflow")?;
        ensure!(events <= MAX_EVENTS, "Too many score events");
        for note in &part.notes {
            let end = note
                .start_tick
                .checked_add(note.duration_ticks)
                .context("Note timing overflow")?;
            ensure!(
                note.duration_ticks > 0 && end <= original.duration_ticks && end <= MAX_TICKS,
                "Invalid note timing"
            );
            ensure!(
                note.pitch <= 127 && note.velocity <= 127 && (1..=128).contains(&note.staff),
                "Invalid original note metadata"
            );
        }
    }
    for tempo in &original.tempos {
        ensure!(
            tempo.tick <= MAX_TICKS && tempo.bpm.is_finite() && (1.0..=1000.0).contains(&tempo.bpm),
            "Invalid original tempo"
        );
    }
    Ok(ids)
}

fn validate_groups(original: &Composition, groups: &[PartGroup]) -> Result<Vec<Vec<usize>>> {
    let ids = validate_original(original)?;
    ensure!(groups.len() <= MAX_PARTS / 2, "Too many part groups");
    let mut occupied = [false; MAX_PARTS];
    let mut result = Vec::with_capacity(groups.len());
    for group in groups {
        ensure!(
            !group.name.trim().is_empty()
                && group.name.len() <= MAX_TEXT_BYTES
                && !group.name.chars().any(char::is_control),
            "Invalid combined part name (use 1..256 bytes without controls)"
        );
        ensure!(valid_id(&group.target_id), "Invalid target part ID");
        ensure!(
            (2..=original.parts.len()).contains(&group.members.len()),
            "A part group requires at least two original members"
        );
        ensure!(
            group.members.contains(&group.target_id),
            "Group target must be a member"
        );
        let mut members = Vec::with_capacity(group.members.len());
        for id in &group.members {
            ensure!(valid_id(id), "Invalid member part ID");
            let &index = ids.get(id.as_str()).context("Unknown original part ID")?;
            ensure!(!occupied[index], "Repeated or overlapping group member");
            occupied[index] = true;
            members.push(index);
        }
        // Original order, never caller selection order, breaks equal-note ties.
        members.sort_unstable();
        result.push(members);
    }
    Ok(result)
}

/// Overlay explicit groups without inferring ties or removing any raw notes.
/// Pass the immutable original composition, not a previously combined result.
/// Empty groups preserve the exact original row and note ordering.
pub fn apply_part_groups(original: &Composition, groups: &[PartGroup]) -> Result<Composition> {
    let members = validate_groups(original, groups)?;
    let mut owner = [None; MAX_PARTS];
    for (group_index, indices) in members.iter().enumerate() {
        for &index in indices {
            owner[index] = Some(group_index);
        }
    }
    let mut parts = Vec::with_capacity(original.parts.len());
    for (index, part) in original.parts.iter().enumerate() {
        match owner[index] {
            None => parts.push(part.clone()),
            Some(group_index) if members[group_index][0] == index => {
                let group = &groups[group_index];
                let target = original
                    .parts
                    .iter()
                    .find(|part| part.id == group.target_id)
                    .context("Unknown target part ID")?;
                let mut combined = Part {
                    id: target.id.clone(),
                    name: group.name.clone(),
                    program: target.program,
                    staves: 1,
                    notes: Vec::new(),
                };
                for &member in &members[group_index] {
                    let source = &original.parts[member];
                    combined.staves = combined.staves.max(source.staves);
                    combined.notes.extend(source.notes.iter().cloned());
                }
                // Stable sort preserves original part/note order for equal keys.
                combined
                    .notes
                    .sort_by_key(|note| (note.start_tick, note.pitch));
                parts.push(combined);
            }
            Some(_) => {}
        }
    }
    let mut result = original.clone();
    result.parts = parts;
    Ok(result)
}

/// Count cross-source overlaps in O(N log N) time and O(N) bounded storage.
pub fn preview_part_group(original: &Composition, group: &PartGroup) -> Result<PartGroupPreview> {
    let members = validate_groups(original, std::slice::from_ref(group))?;
    let mut notes: Vec<(usize, &Note)> = members[0]
        .iter()
        .flat_map(|&source| {
            original.parts[source]
                .notes
                .iter()
                .map(move |n| (source, n))
        })
        .collect();
    notes.sort_by_key(|(_, note)| (note.start_tick, note.pitch));
    let mut preview = PartGroupPreview {
        target_id: group.target_id.clone(),
        name: group.name.clone(),
        members: group.members.clone(),
        note_count: notes.len(),
        start_tick: notes.first().map(|(_, note)| note.start_tick),
        end_tick: None,
        overlapping_note_pairs: 0,
        overlapping_unison_pairs: 0,
        exact_duplicate_pairs: 0,
    };
    let mut active: BinaryHeap<Reverse<(u64, usize, usize)>> = BinaryHeap::new();
    let mut total = 0u64;
    let mut sources = [0u64; MAX_PARTS];
    let mut pitches = [0u64; 128];
    let mut source_pitches = vec![[0u64; 128]; original.parts.len()];
    // Full raw-note keys; no note is removed, even when counted as a duplicate.
    let mut duplicates = BTreeMap::new();
    let mut source_duplicates = BTreeMap::new();
    for (source, note) in notes {
        while let Some(&Reverse((end, old_source, pitch))) = active.peek() {
            if end > note.start_tick {
                break;
            }
            active.pop();
            total -= 1;
            sources[old_source] -= 1;
            pitches[pitch] -= 1;
            source_pitches[old_source][pitch] -= 1;
        }
        let pitch = usize::from(note.pitch);
        preview.overlapping_note_pairs += total - sources[source];
        preview.overlapping_unison_pairs += pitches[pitch] - source_pitches[source][pitch];
        let key = (
            note.start_tick,
            note.duration_ticks,
            note.pitch,
            note.velocity,
            note.staff,
            note.voice.as_str(),
        );
        let all = duplicates.entry(key).or_insert(0u64);
        let own = source_duplicates.entry((source, key)).or_insert(0u64);
        preview.exact_duplicate_pairs += *all - *own;
        *all += 1;
        *own += 1;
        let end = note.start_tick + note.duration_ticks; // Checked by validation.
        preview.end_tick = Some(preview.end_tick.unwrap_or(0).max(end));
        active.push(Reverse((end, source, pitch)));
        total += 1;
        sources[source] += 1;
        pitches[pitch] += 1;
        source_pitches[source][pitch] += 1;
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::{parse_musicxml, PageMap, PageTurn, Tempo};

    fn note(start_tick: u64, duration_ticks: u64, pitch: u8) -> Note {
        Note {
            start_tick,
            duration_ticks,
            pitch,
            velocity: 80,
            staff: 1,
            voice: "1".into(),
        }
    }

    fn part(id: &str, notes: Vec<Note>) -> Part {
        Part {
            id: id.into(),
            name: "Same raw OMR name".into(),
            program: 40,
            staves: 1,
            notes,
        }
    }

    fn score(parts: Vec<Part>) -> Composition {
        Composition {
            title: "Generated".into(),
            parts,
            tempos: vec![Tempo { tick: 0, bpm: 96.0 }],
            duration_ticks: 100,
            warnings: vec!["original warning".into()],
            page_map: Some(PageMap {
                source_pages: vec![0, 2],
                turns: vec![
                    PageTurn { tick: 0, page: 0 },
                    PageTurn { tick: 50, page: 2 },
                ],
                measure_ticks: vec![0, 25, 50, 75],
            }),
        }
    }

    fn group(target: &str, members: &[&str]) -> PartGroup {
        PartGroup {
            target_id: target.into(),
            members: members.iter().map(|id| (*id).into()).collect(),
            name: "Explicit violin".into(),
        }
    }

    type NoteKey = (u64, u64, u8, u8, u16, String);

    fn note_key(note: &Note) -> NoteKey {
        (
            note.start_tick,
            note.duration_ticks,
            note.pitch,
            note.velocity,
            note.staff,
            note.voice.clone(),
        )
    }

    fn multiset(score: &Composition) -> Vec<NoteKey> {
        let mut keys: Vec<_> = score
            .parts
            .iter()
            .flat_map(|p| p.notes.iter().map(note_key))
            .collect();
        keys.sort();
        keys
    }

    fn assert_globals(original: &Composition, result: &Composition) {
        assert_eq!(original.title, result.title);
        assert_eq!(original.duration_ticks, result.duration_ticks);
        assert_eq!(original.warnings, result.warnings);
        let tempos = |score: &Composition| {
            score
                .tempos
                .iter()
                .map(|t| (t.tick, t.bpm.to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(tempos(original), tempos(result));
        let a = original.page_map.as_ref().unwrap();
        let b = result.page_map.as_ref().unwrap();
        assert_eq!(a.source_pages, b.source_pages);
        assert_eq!(a.turns, b.turns);
        assert_eq!(a.measure_ticks, b.measure_ticks);
    }

    #[test]
    fn sparse_overlay_preserves_gaps_metadata_globals_and_multiset() {
        let mut fragment = note(70, 5, 67);
        fragment.staff = 2;
        fragment.voice = "upper divisi".into();
        fragment.velocity = 103;
        let mut target = part("P3", vec![note(30, 10, 62)]);
        target.program = 41;
        target.staves = 2;
        let original = score(vec![
            part("P1", vec![fragment.clone(), note(0, 5, 60)]),
            part("P2", vec![note(10, 10, 48)]),
            target,
        ]);
        let grouping = group("P3", &["P3", "P1"]);
        let combined = apply_part_groups(&original, &[grouping.clone()]).unwrap();
        assert_eq!(
            combined
                .parts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["P3", "P2"]
        );
        assert_eq!(combined.parts[0].name, grouping.name);
        assert_eq!(combined.parts[0].program, 41);
        assert_eq!(combined.parts[0].staves, 2);
        assert_eq!(
            combined.parts[0]
                .notes
                .iter()
                .map(|n| n.start_tick)
                .collect::<Vec<_>>(),
            [0, 30, 70]
        );
        assert_eq!(note_key(&combined.parts[0].notes[2]), note_key(&fragment));
        assert_eq!(multiset(&original), multiset(&combined));
        assert_globals(&original, &combined);
        let preview = preview_part_group(&original, &grouping).unwrap();
        assert_eq!(
            (preview.note_count, preview.start_tick, preview.end_tick),
            (3, Some(0), Some(75))
        );
        assert_eq!(preview.overlapping_note_pairs, 0);
        assert_eq!(original.parts.len(), 3);
    }

    #[test]
    fn empty_groups_restore_exact_original_layout_without_name_merges_or_sorting() {
        let original = score(vec![
            part("A", vec![note(50, 5, 70), note(0, 5, 60)]),
            part("B", vec![note(0, 5, 60)]),
        ]);
        let unchanged = apply_part_groups(&original, &[]).unwrap();
        for (a, b) in original.parts.iter().zip(&unchanged.parts) {
            assert_eq!(
                (&a.id, &a.name, a.program, a.staves),
                (&b.id, &b.name, b.program, b.staves)
            );
            assert_eq!(
                a.notes.iter().map(note_key).collect::<Vec<_>>(),
                b.notes.iter().map(note_key).collect::<Vec<_>>()
            );
        }
        assert_eq!(unchanged.parts.len(), 2);
        assert_globals(&original, &unchanged);
        let _combined = apply_part_groups(&original, &[group("B", &["A", "B"])]).unwrap();
        assert_eq!(
            multiset(&apply_part_groups(&original, &[]).unwrap()),
            multiset(&original)
        );
    }

    #[test]
    fn chords_unisons_duplicates_and_boundaries_are_cross_source_only() {
        let mut voiced = note(0, 10, 60);
        voiced.voice = "2".into();
        let original = score(vec![
            part("A", vec![note(0, 10, 60), note(0, 10, 60), note(0, 10, 64)]),
            part("B", vec![note(0, 10, 60), voiced, note(10, 10, 60)]),
            part("C", vec![note(5, 2, 60)]),
        ]);
        let grouping = group("A", &["C", "B", "A"]);
        let preview = preview_part_group(&original, &grouping).unwrap();
        assert_eq!(preview.overlapping_note_pairs, 11); // A/B:6, A/C:3, B/C:2
        assert_eq!(preview.overlapping_unison_pairs, 8); // A/B:4, A/C:2, B/C:2
        assert_eq!(preview.exact_duplicate_pairs, 2);
        let combined = apply_part_groups(&original, &[grouping]).unwrap();
        assert_eq!(multiset(&original), multiset(&combined));
        assert_eq!(combined.parts[0].notes.len(), 7);
    }

    #[test]
    fn no_cross_id_ties_and_selection_order_does_not_change_stable_order() {
        let mut a = note(0, 10, 60);
        a.velocity = 77;
        let mut b = a.clone();
        b.velocity = 99;
        let original = score(vec![
            part("A", vec![a]),
            part("B", vec![b, note(10, 10, 60)]),
        ]);
        let one = apply_part_groups(&original, &[group("B", &["A", "B"])]).unwrap();
        let two = apply_part_groups(&original, &[group("B", &["B", "A"])]).unwrap();
        assert_eq!(
            one.parts[0].notes.iter().map(note_key).collect::<Vec<_>>(),
            two.parts[0].notes.iter().map(note_key).collect::<Vec<_>>()
        );
        assert_eq!(
            one.parts[0]
                .notes
                .iter()
                .map(|n| n.velocity)
                .collect::<Vec<_>>(),
            [77, 99, 80]
        );
        assert_eq!(one.parts[0].notes.len(), 3);
    }

    #[test]
    fn disjoint_groups_follow_earliest_original_positions_not_group_order() {
        let original = score(
            ["A", "B", "C", "D", "E"]
                .iter()
                .map(|id| part(id, vec![]))
                .collect(),
        );
        let result = apply_part_groups(
            &original,
            &[group("E", &["E", "B"]), group("C", &["C", "A"])],
        )
        .unwrap();
        assert_eq!(
            result
                .parts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["C", "E", "D"]
        );
        let preview = preview_part_group(&original, &group("C", &["C", "A"])).unwrap();
        assert_eq!(
            (preview.note_count, preview.start_tick, preview.end_tick),
            (0, None, None)
        );
    }

    #[test]
    fn already_expanded_repeats_and_page_mapping_stay_unchanged() {
        let xml = r#"<score-partwise version="4.0"><part-list>
          <score-part id="A"><part-name>A</part-name></score-part>
          <score-part id="B"><part-name>B</part-name></score-part></part-list>
          <part id="A"><measure number="1"><attributes><divisions>1</divisions></attributes>
            <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration></note>
            <barline><repeat direction="backward"/></barline></measure></part>
          <part id="B"><measure number="1"><attributes><divisions>1</divisions></attributes>
            <note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>
            <barline><repeat direction="backward"/></barline></measure></part></score-partwise>"#;
        let mut original = parse_musicxml(xml).unwrap();
        assert_eq!(original.parts[0].notes.len(), 2);
        // Generated original-document map follows the already-expanded clock.
        original.page_map = score(vec![]).page_map;
        let result = apply_part_groups(&original, &[group("B", &["A", "B"])]).unwrap();
        assert_eq!(result.parts[0].notes.len(), 4);
        assert_eq!(multiset(&original), multiset(&result));
        assert_globals(&original, &result);
    }

    #[test]
    fn rejects_invalid_stale_and_overlapping_groups_transactionally() {
        let original = score(vec![
            part("A", vec![]),
            part("B", vec![]),
            part("C", vec![]),
        ]);
        let mut invalid = vec![
            group("A", &[]),
            group("A", &["A"]),
            group("A", &["A", "A"]),
            group("A", &["A", "unknown"]),
            group("C", &["A", "B"]),
            group("unknown", &["A", "B"]),
            group("A", &["A", "bad\nid"]),
            group("A", &["A", &"x".repeat(257)]),
        ];
        for name in [
            "",
            " \u{2003} ",
            "bad\nname",
            "bad\u{7f}name",
            &"x".repeat(257),
        ] {
            let mut grouping = group("A", &["A", "B"]);
            grouping.name = name.into();
            invalid.push(grouping);
        }
        for grouping in invalid {
            assert!(
                apply_part_groups(&original, &[grouping.clone()]).is_err(),
                "{grouping:?}"
            );
            assert!(
                preview_part_group(&original, &grouping).is_err(),
                "{grouping:?}"
            );
        }
        assert!(apply_part_groups(
            &original,
            &[group("A", &["A", "B"]), group("C", &["B", "C"])]
        )
        .is_err());
        assert!(apply_part_groups(&original, &vec![group("A", &["A", "B"]); 65]).is_err());
        assert_eq!(original.parts.len(), 3);
    }

    #[test]
    fn validates_original_ids_parts_and_timing_before_any_transformation() {
        let original = score(vec![part("A", vec![note(0, 10, 60)]), part("B", vec![])]);
        let mut invalid = Vec::new();
        for id in ["A", "", "bad\nid", &"x".repeat(257)] {
            let mut altered = original.clone();
            altered.parts[1].id = id.into();
            invalid.push(altered);
        }
        let mut too_many = original.clone();
        too_many.parts = (0..129).map(|n| part(&format!("P{n}"), vec![])).collect();
        invalid.push(too_many);
        let mut empty = original.clone();
        empty.parts.clear();
        invalid.push(empty);
        for (start, duration) in [(0, 0), (99, 2), (u64::MAX, 1), (1, u64::MAX)] {
            let mut altered = original.clone();
            altered.parts[0].notes[0] = note(start, duration, 60);
            invalid.push(altered);
        }
        let mut too_long = original.clone();
        too_long.duration_ticks = MAX_TICKS + 1;
        invalid.push(too_long);
        let mut too_many_events = original.clone();
        too_many_events.parts[0].notes = vec![note(0, 1, 60); MAX_EVENTS];
        invalid.push(too_many_events); // Plus one tempo exceeds combined event limit.
        for altered in invalid {
            assert!(apply_part_groups(&altered, &[]).is_err());
            assert!(preview_part_group(&altered, &group("A", &["A", "B"])).is_err());
        }
    }

    #[test]
    fn dense_limit_counts_billions_of_pairs_without_pairwise_comparisons() {
        let count = (MAX_EVENTS - 1) / 2; // Leave room for the initial tempo.
        let original = score(vec![
            part("A", vec![note(0, 1, 60); count]),
            part("B", vec![note(0, 1, 60); count]),
        ]);
        let preview = preview_part_group(&original, &group("A", &["A", "B"])).unwrap();
        let pairs = (count as u64) * (count as u64);
        assert!(pairs > u64::from(u32::MAX));
        assert_eq!(preview.note_count, count * 2);
        assert_eq!(preview.overlapping_note_pairs, pairs);
        assert_eq!(preview.overlapping_unison_pairs, pairs);
        assert_eq!(preview.exact_duplicate_pairs, pairs);
    }

    #[test]
    fn maximum_names_and_ids_roundtrip_without_normalization() {
        let id = "x".repeat(256);
        let original = score(vec![part(&id, vec![]), part("B", vec![])]);
        let mut grouping = group(&id, &[&id, "B"]);
        grouping.name = "é".repeat(128);
        let json = serde_json::to_vec(&grouping).unwrap();
        assert_eq!(
            serde_json::from_slice::<PartGroup>(&json).unwrap(),
            grouping
        );
        let result = apply_part_groups(&original, &[grouping.clone()]).unwrap();
        assert_eq!(result.parts[0].name, grouping.name);
        assert_eq!(result.parts[0].id, id);
    }
}
