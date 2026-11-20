use super::*;
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;

// All fixtures are generated here; no user's score or derived OMR data is used.
fn score(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">
<score-partwise version="4.0"><work><work-title>Generated test</work-title></work>
<part-list><score-part id="P1"><part-name>Piano</part-name><midi-instrument id="I1"><midi-program>1</midi-program></midi-instrument></score-part></part-list>
<part id="P1">{body}</part></score-partwise>"#
    )
}

#[test]
fn midi_exports_independent_channel_volume() {
    let mut c = simple();
    let mut second = c.parts[0].clone();
    second.id = "P2".into();
    c.parts.push(second);
    let mut options = MidiOptions::default();
    options.volumes.insert("P1".into(), 0);
    options.volumes.insert("P2".into(), 47);
    let tracks = midi_tracks(&to_midi(&c, &options).unwrap());
    assert!(tracks[1]
        .iter()
        .any(|e| e.status == 0xb0 && e.data == [7, 0]));
    assert!(tracks[2]
        .iter()
        .any(|e| e.status == 0xb1 && e.data == [7, 47]));
    options.volumes.insert("P2".into(), 128);
    assert!(to_midi(&c, &options).is_err());
}

#[test]
fn shared_alternative_endings_apply_to_unmarked_accompaniment() {
    let first = format!(
        r#"
<measure number="1" implicit="yes"><barline><repeat direction="forward"/></barline>{}</measure>
<measure number="2" implicit="yes"><barline><ending number="1" type="start"/></barline>{}<barline><ending number="1" type="stop"/><repeat direction="backward"/></barline></measure>
<measure number="3" implicit="yes"><barline><ending number="2" type="start"/></barline>{}<barline><ending number="2" type="stop"/></barline></measure>"#,
        pitched("C", 1, ""),
        pitched("D", 1, ""),
        pitched("E", 1, "")
    );
    let second = format!(
        r#"
<measure number="1" implicit="yes"><barline><repeat direction="forward"/></barline>{}</measure>
<measure number="2" implicit="yes">{}<barline><repeat direction="backward"/></barline></measure>
<measure number="3" implicit="yes">{}</measure>"#,
        pitched("F", 1, ""),
        pitched("G", 1, ""),
        pitched("A", 1, "")
    );
    let c = parse_musicxml(&two_part_score(&first, &second)).unwrap();
    assert_eq!(
        c.parts[1].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [65, 67, 65, 69]
    );
    assert_eq!(c.duration_ticks, 3840);
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("endings from P1") && w.contains("all parts")));
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("repeat/endings differ")));
}

fn two_part_score(first: &str, second: &str) -> String {
    score(first)
        .replace(
            "</part-list>",
            "<score-part id=\"P2\"><part-name>Accompaniment</part-name></score-part></part-list>",
        )
        .replace(
            "</score-partwise>",
            &format!("<part id=\"P2\">{second}</part></score-partwise>"),
        )
}

#[test]
fn empty_implicit_measure_inherits_shared_duration_without_padding_or_drift() {
    let timed = format!(
        r#"<measure number="1"><attributes><divisions>2</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>{}</measure>
<measure number="2" implicit="yes">{}</measure>
<measure number="3">{}</measure>"#,
        pitched("C", 8, ""),
        pitched("D", 6, ""),
        pitched("E", 8, "")
    );
    let missing = format!(
        r#"<measure number="1"><attributes><divisions>2</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>{}</measure>
<measure number="2" implicit="yes"><print new-system="yes"/><attributes><divisions>4</divisions><clef><sign>F</sign><line>4</line></clef></attributes><direction><sound tempo="90"/></direction></measure>
<measure number="3">{}</measure>"#,
        pitched("F", 8, ""),
        pitched("G", 16, "")
    );
    for empty_first in [false, true] {
        let xml = if empty_first {
            two_part_score(&missing, &timed)
        } else {
            two_part_score(&timed, &missing)
        };
        let c = parse_musicxml(&xml).unwrap();
        let empty = &c.parts[usize::from(!empty_first)];
        let timed = &c.parts[usize::from(empty_first)];
        assert_eq!(c.duration_ticks, 11 * 960);
        assert_eq!(
            empty.notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            [0, 7 * 960]
        );
        assert_eq!(
            empty.notes[1].duration_ticks,
            4 * 960,
            "Attributes in the empty measure still apply"
        );
        assert_eq!(
            timed.notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            [0, 4 * 960, 7 * 960]
        );
        assert!(c
            .tempos
            .iter()
            .any(|tempo| tempo.tick == 4 * 960 && tempo.bpm == 90.));
        assert!(c
            .warnings
            .iter()
            .any(|w| w.contains("empty implicit") && w.contains(&empty.id)));
        assert!(to_midi(&c, &MidiOptions::default()).is_ok());
    }
}

#[test]
fn empty_implicit_pickup_uses_the_other_parts_short_pickup() {
    let attributes = "<attributes><divisions>4</divisions><time><beats>6</beats><beat-type>8</beat-type></time></attributes>";
    let first = format!(
        r#"<measure number="0" implicit="yes">{attributes}</measure><measure number="1">{}</measure>"#,
        pitched("C", 12, "")
    );
    let second = format!(
        r#"<measure number="0" implicit="yes">{attributes}{}</measure><measure number="1">{}</measure>"#,
        pitched("D", 2, ""),
        pitched("E", 12, "")
    );
    let c = parse_musicxml(&two_part_score(&first, &second)).unwrap();
    assert_eq!(c.parts[0].notes[0].start_tick, 480);
    assert_eq!(c.parts[1].notes[1].start_tick, 480);
    assert_eq!(c.duration_ticks, 3360);
}

#[test]
fn empty_implicit_without_shared_timing_remains_an_error() {
    for xml in [
        score(r#"<measure number="1" implicit="yes"><print/></measure>"#),
        two_part_score(
            r#"<measure number="1" implicit="yes"/>"#,
            r#"<measure number="1" implicit="yes"><direction><sound tempo="90"/></direction></measure>"#,
        ),
    ] {
        assert!(
            parse_musicxml(&xml).is_err(),
            "Do not invent a silent bar or discard a measure without timing"
        );
    }
}

#[test]
fn empty_implicit_retains_tempo_and_shared_repeat_markings() {
    let first = format!(
        r#"<measure number="1" implicit="yes"><direction><sound tempo="90"/></direction><barline><repeat direction="forward"/></barline></measure><measure number="2" implicit="yes">{}</measure>"#,
        pitched("C", 1, "")
    );
    let second = format!(
        r#"<measure number="1" implicit="yes">{}</measure><measure number="2" implicit="yes">{}<barline><repeat direction="backward"/></barline></measure>"#,
        pitched("D", 1, ""),
        pitched("E", 1, "")
    );
    let c = parse_musicxml(&two_part_score(&first, &second)).unwrap();
    assert_eq!(c.duration_ticks, 3840);
    assert_eq!(
        c.parts[0]
            .notes
            .iter()
            .map(|n| n.start_tick)
            .collect::<Vec<_>>(),
        [960, 2880]
    );
    assert_eq!(
        c.tempos.iter().map(|t| (t.tick, t.bpm)).collect::<Vec<_>>(),
        [(0, 90.), (1920, 90.)]
    );
}
#[test]
fn shared_clock_and_repeats_prevent_cross_part_drift() {
    let first = format!(
        "<measure number=\"1\">{}</measure><measure number=\"2\" implicit=\"yes\">{}</measure>",
        pitched("C", 5, ""),
        pitched("D", 1, "")
    );
    let second = format!(
        "<measure number=\"1\"><barline><repeat direction=\"forward\"/></barline>{}</measure><measure number=\"2\" implicit=\"yes\">{}<barline><repeat direction=\"backward\"/></barline></measure>",
        pitched("E", 4, ""), pitched("F", 1, "")
    );
    let c = parse_musicxml(&two_part_score(&first, &second)).unwrap();
    let starts: Vec<_> = c.parts[0].notes.iter().map(|n| n.start_tick).collect();
    assert_eq!(starts, [0, 4800, 5760, 10560]);
    assert_eq!(
        starts,
        c.parts[1]
            .notes
            .iter()
            .map(|n| n.start_tick)
            .collect::<Vec<_>>()
    );
    assert_eq!(c.duration_ticks, 11520);
    assert!(c.warnings.iter().any(|w| w.contains("durations differ")));
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("repeat/endings differ")));
    assert!(c.warnings.iter().any(|w| w.contains("overfull")));
}

#[test]
fn inconsistent_measure_counts_labels_and_navigation_are_visible() {
    let first = format!(
        "<measure number=\"1\" implicit=\"yes\"><direction><sound dacapo=\"yes\"/></direction>{}</measure><measure number=\"2\" implicit=\"yes\">{}</measure>",
        pitched("C", 1, ""), pitched("D", 1, "")
    );
    let second = format!(
        "<measure number=\"2\" implicit=\"yes\">{}</measure>",
        pitched("E", 1, "")
    );
    let c = parse_musicxml(&two_part_score(&first, &second)).unwrap();
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("Measure counts differ")));
    assert!(c.warnings.iter().any(|w| w.contains("labels differ")));
    assert!(c.warnings.iter().any(|w| w.contains("navigation differs")));
    assert_eq!(c.duration_ticks, 1920);
    assert_eq!(c.parts[1].notes.len(), 1);
}

#[test]
fn missing_initial_tempo_is_distinguishable_from_explicit_120() {
    assert!(simple()
        .warnings
        .iter()
        .any(|w| w.contains("No initial tempo")));
    let c = parse_musicxml(&score(&format!(
        "<measure implicit=\"yes\"><direction><sound tempo=\"120\"/></direction>{}</measure>",
        pitched("C", 1, "")
    )))
    .unwrap();
    assert!(!c.warnings.iter().any(|w| w.contains("No initial tempo")));
}
#[test]
fn archive_declared_oversize_is_rejected_before_decompression() {
    let mut bytes = archive(&[("oversized.xml", "small")]);
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&((MAX_INPUT + 1) as u32).to_le_bytes());
    let error = read_archive(Cursor::new(bytes)).unwrap_err().to_string();
    assert!(error.contains("size limit"), "{error}");
}

fn pitched(step: &str, duration: u32, extra: &str) -> String {
    format!("<note><pitch><step>{step}</step><octave>4</octave></pitch><duration>{duration}</duration>{extra}</note>")
}

fn simple() -> Composition {
    parse_musicxml(&score(&format!(
        "<measure implicit=\"yes\">{}</measure>",
        pitched("C", 1, "")
    )))
    .unwrap()
}

#[test]
fn public_doctype_is_accepted_without_loading_dtd() {
    let composition = simple();
    assert_eq!(composition.title, "Generated test");
    assert_eq!(composition.parts.len(), 1);
    assert_eq!(composition.parts[0].notes[0].pitch, 60);
    assert!(!composition.warnings.is_empty());
}

#[test]
fn voices_chords_staff_grouping_and_rests() {
    let body = format!(
        r#"<measure>
<attributes><divisions>2</divisions><staves>2</staves><time><beats>4</beats><beat-type>4</beat-type></time></attributes>
{}
<note><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice><staff>1</staff></note>
<note><rest/><duration>6</duration><voice>1</voice></note>
<backup><duration>8</duration></backup>
{}
<forward><duration>2</duration></forward>
{}
</measure>"#,
        pitched("C", 2, "<voice>1</voice><staff>1</staff>"),
        pitched("G", 4, "<voice>2</voice><staff>2</staff>"),
        pitched("A", 2, "<voice>2</voice><staff>2</staff>")
    );
    let composition = parse_musicxml(&score(&body)).unwrap();
    let part = &composition.parts[0];
    assert_eq!(part.staves, 2);
    assert_eq!(part.notes.len(), 4);
    assert_eq!(
        part.notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
        [0, 0, 0, 2880]
    );
    assert_eq!(part.notes[2].duration_ticks, 1920);
    assert_eq!(part.notes[2].staff, 2);
    assert_eq!(part.notes[2].voice, "2");
    assert_eq!(composition.duration_ticks, 3840);
}

#[test]
fn divisions_changes_pickups_and_transposition() {
    let body = format!(
        r#"
<measure implicit="yes"><attributes><divisions>2</divisions><transpose><chromatic>-2</chromatic><octave-change>-1</octave-change></transpose></attributes>{}</measure>
<measure implicit="yes"><attributes><divisions>4</divisions></attributes>{}<attributes><divisions>8</divisions></attributes>{}</measure>"#,
        pitched("C", 1, ""),
        pitched("D", 4, ""),
        pitched("E", 8, "")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(
        c.parts[0].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [46, 48, 50]
    );
    assert_eq!(
        c.parts[0]
            .notes
            .iter()
            .map(|n| n.start_tick)
            .collect::<Vec<_>>(),
        [0, 480, 1440]
    );
    assert_eq!(c.duration_ticks, 2400);
}

#[test]
fn tie_chain_is_one_sounding_note() {
    let body = format!(
        r#"
<measure implicit="yes">{}</measure>
<measure implicit="yes">{}</measure>
<measure implicit="yes">{}</measure>"#,
        pitched("C", 1, "<tie type=\"start\"/>"),
        pitched("C", 1, "<tie type=\"stop\"/><tie type=\"start\"/>"),
        pitched("C", 1, "<notations><tied type=\"stop\"/></notations>")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(c.parts[0].notes.len(), 1);
    assert_eq!(c.parts[0].notes[0].duration_ticks, 2880);
}

#[test]
fn first_second_endings_expand_without_playing_first_ending_twice() {
    let body = format!(
        r#"
<measure implicit="yes"><barline location="left"><repeat direction="forward"/></barline>{}</measure>
<measure implicit="yes"><barline location="left"><ending number="1" type="start"/></barline>{}<barline location="right"><ending number="1" type="stop"/><repeat direction="backward"/></barline></measure>
<measure implicit="yes"><barline location="left"><ending number="2" type="start"/></barline>{}<barline location="right"><ending number="2" type="stop"/></barline></measure>"#,
        pitched("C", 1, ""),
        pitched("D", 1, ""),
        pitched("E", 1, "")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(
        c.parts[0].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [60, 62, 60, 64]
    );
    assert_eq!(c.duration_ticks, 3840);
}

#[test]
fn repeated_sections_with_implicit_start_and_consecutive_repeats() {
    let body = format!(
        r#"
<measure implicit="yes">{}<barline><repeat direction="backward" times="3"/></barline></measure>
<measure implicit="yes"><barline><repeat direction="forward"/></barline>{}<barline><repeat direction="backward"/></barline></measure>"#,
        pitched("C", 1, ""),
        pitched("D", 1, "")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(
        c.parts[0].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [60, 60, 60, 62, 62]
    );
}

#[test]
fn multimeasure_endings_and_tempo_repeats() {
    let body = format!(
        r#"
<measure implicit="yes"><direction><sound tempo="90"/></direction>{}</measure>
<measure implicit="yes"><barline><ending number="1" type="start"/></barline>{}</measure>
<measure implicit="yes">{}<barline><ending number="1" type="stop"/><repeat direction="backward"/></barline></measure>
<measure implicit="yes"><barline><ending number="2" type="start"/></barline><direction><direction-type><metronome><beat-unit>half</beat-unit><beat-unit-dot/><per-minute>40</per-minute></metronome></direction-type></direction>{}<barline><ending number="2" type="discontinue"/></barline></measure>"#,
        pitched("C", 1, ""),
        pitched("D", 1, ""),
        pitched("E", 1, ""),
        pitched("F", 1, "")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(
        c.parts[0].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [60, 62, 64, 60, 65]
    );
    assert_eq!(
        c.tempos.iter().map(|t| (t.tick, t.bpm)).collect::<Vec<_>>(),
        [(0, 90.0), (2880, 90.0), (3840, 120.0)]
    );
}

#[test]
fn rest_only_score_retains_duration() {
    let c = parse_musicxml(&score(
        "<measure><note><rest measure=\"yes\"/><duration>4</duration></note></measure>",
    ))
    .unwrap();
    assert!(c.parts[0].notes.is_empty());
    assert_eq!(c.duration_ticks, 3840);
    assert!(to_midi(&c, &MidiOptions::default()).is_ok());
}

#[test]
fn unsupported_navigation_is_explicit() {
    let c = parse_musicxml(&score(&format!(
        "<measure implicit=\"yes\"><direction><sound dacapo=\"yes\"/></direction>{}</measure>",
        pitched(
            "C",
            1,
            "<notations><articulations><staccato/></articulations><fermata/></notations>"
        )
    )))
    .unwrap();
    assert!(c.warnings.iter().any(|w| w.contains("D.C.")));
    assert!(c.warnings.iter().any(|w| w.contains("articulations")));
    assert!(c.warnings.iter().any(|w| w.contains("fermata")));
}

#[test]
fn malformed_xml_entities_and_invalid_timing_fail() {
    assert!(parse_musicxml("<score-partwise>").is_err());
    assert!(parse_musicxml("<!DOCTYPE score-partwise [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><score-partwise>&x;</score-partwise>").is_err());
    assert!(parse_musicxml(&score(
        "<measure><backup><duration>1</duration></backup></measure>"
    ))
    .is_err());
    assert!(parse_musicxml(&score(
        "<measure><attributes><divisions>0</divisions></attributes></measure>"
    ))
    .is_err());
    assert!(parse_musicxml(&score(
        "<measure><attributes><divisions>NaN</divisions></attributes></measure>"
    ))
    .is_err());
    assert!(parse_musicxml(&score(
        "<measure><barline><repeat direction=\"backward\" times=\"255\"/></barline></measure>"
    ))
    .is_err());
    assert!(parse_musicxml(&score(&format!(
        "<measure>{}</measure>",
        pitched("C", u32::MAX, "")
    )))
    .is_err());
    assert!(parse_musicxml(&score("<measure><note><chord/><pitch><step>C</step></pitch><duration>1</duration></note></measure>")).is_err());
}

#[test]
fn bounded_input_decompression_and_expansion() {
    assert!(read_bounded(Cursor::new(b"12345"), 4).is_err());
    assert!(parse_musicxml(&" ".repeat(MAX_INPUT as usize + 1)).is_err());
    let mut part = simple().parts.remove(0);
    let measure = Measure {
        duration: 1.0,
        notes: vec![RawNote {
            beat: 0.0,
            duration: 1.0,
            pitch: 60,
            staff: 1,
            voice: "1".into(),
            tie_start: false,
            tie_stop: false,
        }],
        backward_repeat: Some(8),
        ..Measure::default()
    };
    assert!(expand_part(
        &[measure],
        &mut part,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut (MAX_EVENTS - 2),
        None,
        None,
    )
    .is_err());
}

fn archive(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(data.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn compressed_musicxml_and_plain_load() {
    let xml = score(&format!(
        "<measure implicit=\"yes\">{}</measure>",
        pitched("C", 1, "")
    ));
    let container = r#"<container><rootfiles><rootfile full-path="scores/test.musicxml" media-type="application/vnd.recordare.musicxml+xml"/></rootfiles></container>"#;
    let bytes = archive(&[
        ("META-INF/container.xml", container),
        ("scores/test.musicxml", &xml),
    ]);
    let parsed = parse_musicxml(&read_archive(Cursor::new(&bytes)).unwrap()).unwrap();
    assert_eq!(parsed.parts[0].notes.len(), 1);
    let dir = tempfile::tempdir().unwrap();
    let mxl = dir.path().join("test.mxl");
    let plain = dir.path().join("test.musicxml");
    std::fs::write(&mxl, &bytes).unwrap();
    std::fs::write(&plain, &xml).unwrap();
    assert_eq!(
        load(&mxl).unwrap().duration_ticks,
        load(&plain).unwrap().duration_ticks
    );
}

#[test]
fn cross_staff_chords_and_compound_meter() {
    let body = format!(
        r#"<measure>
<attributes><staves>2</staves><time><beats>3+2</beats><beat-type>8</beat-type><beats>1</beats><beat-type>4</beat-type></time></attributes>
{}<note><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><staff>2</staff></note>
<note><rest/><duration>2.5</duration></note></measure>"#,
        pitched("C", 1, "<staff>1</staff>")
    );
    let c = parse_musicxml(&score(&body)).unwrap();
    assert_eq!(c.parts[0].notes.len(), 2);
    assert_eq!(c.parts[0].notes[1].start_tick, 0);
    assert_eq!(c.parts[0].notes[1].staff, 2);
    assert_eq!(c.duration_ticks, 3360);
}

#[test]
fn archive_traversal_missing_roots_and_multiple_movements_fail() {
    for name in [
        "../escape.xml",
        "/absolute.xml",
        "C:/x",
        r"a\..\b",
        "safe/../../bad",
    ] {
        assert!(!safe_archive_path(name));
        let bytes = archive(&[(name, "not extracted")]);
        assert!(read_archive(Cursor::new(bytes)).is_err());
    }
    let bytes = archive(&[("test.xml", "<score-partwise/>")]);
    assert!(read_archive(Cursor::new(bytes)).is_err());
    let container = "<container><rootfiles><rootfile full-path=\"one.xml\"/><rootfile full-path=\"two.xml\"/></rootfiles></container>";
    let bytes = archive(&[("META-INF/container.xml", container)]);
    assert!(read_archive(Cursor::new(bytes)).is_err());
}

#[derive(Debug)]
struct Event {
    tick: u64,
    status: u8,
    data: Vec<u8>,
}

fn variable(data: &[u8], pos: &mut usize) -> usize {
    let mut value = 0;
    loop {
        let byte = data[*pos];
        *pos += 1;
        value = (value << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return value;
        }
    }
}

fn midi_tracks(data: &[u8]) -> Vec<Vec<Event>> {
    assert_eq!(&data[..4], b"MThd");
    assert_eq!(u16::from_be_bytes([data[12], data[13]]), TICKS_PER_QUARTER);
    let count = u16::from_be_bytes([data[10], data[11]]);
    let mut offset = 14;
    let mut tracks = Vec::new();
    for _ in 0..count {
        assert_eq!(&data[offset..offset + 4], b"MTrk");
        let len = u32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let track = &data[offset + 8..offset + 8 + len];
        let mut pos = 0;
        let mut tick = 0;
        let mut events = Vec::new();
        while pos < track.len() {
            tick += variable(track, &mut pos) as u64;
            let status = track[pos];
            pos += 1;
            let length = if status == 0xff {
                let kind = track[pos];
                pos += 1;
                let len = variable(track, &mut pos);
                events.push(Event {
                    tick,
                    status: kind,
                    data: track[pos..pos + len].to_vec(),
                });
                len
            } else {
                let len = if status & 0xf0 == 0xc0 { 1 } else { 2 };
                events.push(Event {
                    tick,
                    status,
                    data: track[pos..pos + len].to_vec(),
                });
                len
            };
            pos += length;
        }
        tracks.push(events);
        offset += 8 + len;
    }
    assert_eq!(offset, data.len());
    tracks
}

#[test]
fn midi_selection_program_override_and_tempo_scale() {
    let mut c = simple();
    let mut second = c.parts[0].clone();
    second.id = "P2".into();
    second.name = "Cello".into();
    second.program = 42;
    c.parts.push(second);
    let mut options = MidiOptions {
        selected_parts: Some(vec!["P2".into()]),
        tempo_scale: 2.0,
        ..MidiOptions::default()
    };
    options.programs.insert("P2".into(), 43);
    let bytes = to_midi(&c, &options).unwrap();
    let tracks = midi_tracks(&bytes);
    assert_eq!(tracks.len(), 2);
    assert!(tracks[0]
        .iter()
        .any(|e| e.status == 0x51 && e.data == [0x03, 0xd0, 0x90]));
    assert!(tracks[1].iter().any(|e| e.status == 0xc0 && e.data == [43]));
    assert!(tracks[1]
        .iter()
        .any(|e| e.status == 3 && e.data == b"Cello"));
}

#[test]
fn midi_never_uses_percussion_channel_and_limits_part_count() {
    let mut c = simple();
    let base = c.parts[0].clone();
    c.parts = (0..15)
        .map(|i| {
            let mut p = base.clone();
            p.id = format!("P{i}");
            p
        })
        .collect();
    let tracks = midi_tracks(&to_midi(&c, &MidiOptions::default()).unwrap());
    for (i, track) in tracks.iter().skip(1).enumerate() {
        let channel = if i < 9 { i as u8 } else { i as u8 + 1 };
        assert_ne!(channel, 9);
        assert!(track.iter().any(|e| e.status == (0xc0 | channel)));
    }
    c.parts.push(base);
    assert!(to_midi(&c, &MidiOptions::default()).is_err());
}

#[test]
fn midi_note_off_precedes_retrigger_and_overlapping_unisons_merge() {
    let mut c = simple();
    let base = c.parts[0].notes[0].clone();
    let mut next = base.clone();
    next.start_tick = 960;
    c.parts[0].notes.push(next);
    c.duration_ticks = 1920;
    let tracks = midi_tracks(&to_midi(&c, &MidiOptions::default()).unwrap());
    let at: Vec<_> = tracks[1]
        .iter()
        .filter(|e| e.tick == 960)
        .map(|e| e.status)
        .collect();
    assert_eq!(at, [0x80, 0x90]);
    let mut overlap = base;
    overlap.start_tick = 480;
    overlap.duration_ticks = 960;
    c.parts[0].notes.push(overlap);
    let tracks = midi_tracks(&to_midi(&c, &MidiOptions::default()).unwrap());
    assert_eq!(tracks[1].iter().filter(|e| e.status == 0x90).count(), 1);
    assert_eq!(
        tracks[1].iter().find(|e| e.status == 0x80).unwrap().tick,
        1920
    );
}

#[test]
fn midi_invalid_options_and_overflow_are_errors() {
    let mut c = simple();
    for scale in [0.0, f64::NAN, f64::INFINITY, 5.0] {
        assert!(to_midi(
            &c,
            &MidiOptions {
                tempo_scale: scale,
                ..MidiOptions::default()
            }
        )
        .is_err());
    }
    for ids in [vec![], vec!["missing".into()]] {
        assert!(to_midi(
            &c,
            &MidiOptions {
                selected_parts: Some(ids),
                ..MidiOptions::default()
            }
        )
        .is_err());
    }
    let mut options = MidiOptions::default();
    options.programs.insert("P1".into(), 255);
    assert!(to_midi(&c, &options).is_err());
    c.parts[0].notes[0].start_tick = u64::MAX;
    assert!(to_midi(&c, &MidiOptions::default()).is_err());
}
