//! Bounded, offline MusicXML interpretation and deliberately limited MIDI playback.
//!
//! Durations in MusicXML are authoritative (including tuplets). Playback is not
//! a notation engraver: unsupported expression/navigation is reported as warnings.
use anyhow::{bail, ensure, Context, Result};
use roxmltree::{Document, Node, ParsingOptions};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path};

mod midi;
mod pagination;
pub use midi::to_midi;
#[cfg(test)]
mod tests;

pub const TICKS_PER_QUARTER: u16 = 960;
const MAX_INPUT: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_EVENTS: usize = 250_000;
const MAX_MEASURES: usize = 20_000;
const MAX_TICKS: u64 = 960 * 60 * 60 * 24;

#[derive(Clone, Debug)]
pub struct Composition {
    pub title: String,
    pub parts: Vec<Part>,
    pub tempos: Vec<Tempo>,
    pub duration_ticks: u64,
    pub warnings: Vec<String>,
    pub page_map: Option<PageMap>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageTurn {
    pub tick: u64,
    /// Zero-based physical page in the original source document.
    pub page: u32,
}

#[derive(Clone, Debug)]
pub struct PageMap {
    pub source_pages: Vec<u32>,
    pub turns: Vec<PageTurn>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub id: String,
    pub name: String,
    /// Zero-based General MIDI program.
    pub program: u8,
    pub staves: u16,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub pitch: u8,
    pub velocity: u8,
    pub staff: u16,
    pub voice: String,
}

#[derive(Clone, Debug)]
pub struct Tempo {
    pub tick: u64,
    pub bpm: f64,
}

#[derive(Clone, Debug)]
pub struct MidiOptions {
    /// None selects all parts; Some(empty) is an error.
    pub selected_parts: Option<Vec<String>>,
    /// Overrides keyed by MusicXML part ID; zero-based General MIDI programs.
    pub programs: BTreeMap<String, u8>,
    /// Per-part MIDI CC7 volume, 0..127.
    pub volumes: BTreeMap<String, u8>,
    pub tempo_scale: f64,
}

impl Default for MidiOptions {
    fn default() -> Self {
        Self {
            selected_parts: None,
            programs: BTreeMap::new(),
            volumes: BTreeMap::new(),
            tempo_scale: 1.0,
        }
    }
}

fn warn(warnings: &mut Vec<String>, message: impl Into<String>) {
    let message = message.into();
    if warnings.len() < 256 && !warnings.contains(&message) {
        warnings.push(message);
    }
}

/// Read plain MusicXML or a compressed MusicXML archive without extracting files.
pub fn load(path: &Path) -> Result<Composition> {
    let mut file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= MAX_ARCHIVE,
        "Score input is too large"
    );
    let mut magic = [0; 4];
    let n = file.read(&mut magic)?;
    file.rewind()?;
    let xml = if n == 4 && &magic == b"PK\x03\x04" {
        read_archive(file)?
    } else {
        read_bounded(file, MAX_INPUT)?
    };
    parse_musicxml(&xml)
}

fn read_bounded(reader: impl Read, limit: u64) -> Result<String> {
    let mut data = Vec::new();
    reader.take(limit + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= limit,
        "Decompressed score input is too large"
    );
    String::from_utf8(data).context("MusicXML must be UTF-8")
}

fn safe_archive_path(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && !Path::new(name).is_absolute()
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn read_archive(reader: impl Read + Seek) -> Result<String> {
    let mut archive = zip::ZipArchive::new(reader).context("Invalid compressed MusicXML")?;
    ensure!(archive.len() <= 256, "Too many archive entries");
    let mut total = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        ensure!(safe_archive_path(entry.name()), "Unsafe archive entry path");
        ensure!(
            entry.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "Archive symbolic links are not supported"
        );
        total = total
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(
            entry.size() <= MAX_INPUT && total <= MAX_ARCHIVE,
            "Archive expands beyond size limit"
        );
    }
    let container = {
        let entry = archive
            .by_name("META-INF/container.xml")
            .context("Missing MusicXML container")?;
        read_bounded(entry, 1024 * 1024)?
    };
    let doc = xml_document(&container)?;
    let roots: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("rootfile"))
        .collect();
    let supported: Vec<_> = roots
        .iter()
        .filter(|n| {
            n.attribute("media-type")
                .is_none_or(|v| v == "application/vnd.recordare.musicxml+xml")
        })
        .collect();
    ensure!(
        supported.len() == 1,
        "Expected exactly one MusicXML rootfile; load movements separately"
    );
    let name = supported[0]
        .attribute("full-path")
        .context("Missing rootfile path")?;
    ensure!(safe_archive_path(name), "Unsafe MusicXML rootfile path");
    let entry = archive.by_name(name).context("Missing MusicXML rootfile")?;
    read_bounded(entry, MAX_INPUT)
}

fn xml_document(xml: &str) -> Result<Document<'_>> {
    ensure!(xml.len() as u64 <= MAX_INPUT, "MusicXML input is too large");
    // Ordinary public/system MusicXML DOCTYPEs are accepted. roxmltree never
    // fetches external DTDs; rejecting entity declarations also forbids internal
    // entity expansion and external entity references.
    ensure!(
        !xml.contains("<!ENTITY"),
        "XML entity declarations are not permitted"
    );
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: true,
            nodes_limit: 1_000_000,
        },
    )
    .context("Invalid MusicXML")
}

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(tag))
}

fn text<'a>(node: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(node, tag).and_then(|n| n.text()).map(str::trim)
}

fn number(node: Node<'_, '_>, tag: &str, default: f64) -> Result<f64> {
    match text(node, tag) {
        Some(value) => {
            let value: f64 = value.parse().with_context(|| format!("Invalid {tag}"))?;
            ensure!(value.is_finite(), "Nonfinite {tag}");
            Ok(value)
        }
        None => Ok(default),
    }
}

fn ticks(beats: f64) -> Result<u64> {
    ensure!(beats.is_finite() && beats >= 0.0, "Invalid musical time");
    let ticks = (beats * f64::from(TICKS_PER_QUARTER)).round();
    ensure!(ticks <= MAX_TICKS as f64, "Score timing exceeds limit");
    Ok(ticks as u64)
}

#[derive(Clone)]
struct RawNote {
    beat: f64,
    duration: f64,
    pitch: u8,
    staff: u16,
    voice: String,
    tie_start: bool,
    tie_stop: bool,
}

#[derive(Default)]
struct Measure {
    number: String,
    source_page: Option<u32>,
    duration: f64,
    notes: Vec<RawNote>,
    tempos: Vec<(f64, f64)>,
    forward_repeat: bool,
    backward_repeat: Option<u8>,
    endings: Vec<u8>,
}

/// Interpret score-partwise MusicXML. Unsupported constructs produce warnings.
pub fn parse_musicxml(xml: &str) -> Result<Composition> {
    let doc = xml_document(xml)?;
    let root = doc.root_element();
    ensure!(
        root.has_tag_name("score-partwise"),
        "Only score-partwise MusicXML is supported"
    );
    let title = root
        .descendants()
        .find(|n| n.has_tag_name("work-title"))
        .or_else(|| {
            root.descendants()
                .find(|n| n.has_tag_name("movement-title"))
        })
        .and_then(|n| n.text())
        .unwrap_or("Untitled score")
        .trim()
        .to_owned();
    let mut composition = Composition {
        title,
        parts: Vec::new(),
        tempos: Vec::new(),
        duration_ticks: 0,
        page_map: None,
        warnings: vec![
            "Recognition/notation playback is unverified; review the score and exported MIDI. Playback uses fixed velocity and no expressive timing."
                .into(),
        ],
    };
    let list = child(root, "part-list").context("MusicXML has no part-list")?;
    for node in list.children().filter(|n| n.has_tag_name("score-part")) {
        ensure!(composition.parts.len() < 128, "Too many score parts");
        let id = node.attribute("id").context("Part has no ID")?.to_owned();
        ensure!(!id.is_empty() && id.len() <= 256, "Invalid part ID");
        ensure!(
            !composition.parts.iter().any(|p| p.id == id),
            "Duplicate part ID"
        );
        let program = node
            .descendants()
            .find(|n| n.has_tag_name("midi-program"))
            .and_then(|n| n.text())
            .map(str::trim)
            .unwrap_or("1")
            .parse::<u16>()
            .context("Invalid MIDI program")?;
        ensure!(
            (1..=128).contains(&program),
            "MusicXML MIDI program outside 1..128"
        );
        if node.descendants().any(|n| n.has_tag_name("midi-unpitched")) {
            warn(
                &mut composition.warnings,
                "Unpitched percussion is unsupported; pitched GM playback only.",
            );
        }
        if node
            .children()
            .filter(|n| n.has_tag_name("midi-instrument"))
            .count()
            > 1
        {
            warn(
                &mut composition.warnings,
                "Multiple instruments within a part are unsupported; the first GM program is used.",
            );
        }
        composition.parts.push(Part {
            name: text(node, "part-name").unwrap_or(&id).to_owned(),
            id,
            program: (program - 1) as u8,
            staves: 1,
            notes: Vec::new(),
        });
    }
    ensure!(!composition.parts.is_empty(), "Score has no parts");
    ensure!(
        root.children().filter(|n| n.has_tag_name("part")).count() == composition.parts.len(),
        "Part-list and part content counts differ"
    );
    let mut event_count = 0usize;
    let mut all_measures = Vec::new();
    let mut first_navigation = None;
    for part in &mut composition.parts {
        let candidates: Vec<_> = root
            .children()
            .filter(|n| n.has_tag_name("part") && n.attribute("id") == Some(part.id.as_str()))
            .collect();
        ensure!(
            candidates.len() == 1,
            "Missing or duplicate part content for {}",
            part.id
        );
        let navigation = navigation_signature(candidates[0]);
        if let Some(first) = &first_navigation {
            if first != &navigation {
                warn(
                    &mut composition.warnings,
                    "Unsupported navigation differs between parts; D.C./D.S./Coda/Fine navigation is not performed.",
                );
            }
        } else {
            first_navigation = Some(navigation);
        }
        all_measures.push(parse_part(
            candidates[0],
            part,
            &mut composition.warnings,
            &mut event_count,
        )?);
    }
    let layout = match pagination::source_layout(root) {
        Ok(layout) => Some(layout),
        Err(error) => {
            warn(
                &mut composition.warnings,
                format!("Automatic page turning unavailable: {error:#}"),
            );
            None
        }
    };
    align_parts(
        &mut all_measures,
        &composition.parts,
        &mut composition.warnings,
    )?;
    if let Some(layout) = &layout {
        for measures in &mut all_measures {
            for (measure, page) in measures.iter_mut().zip(&layout.measure_pages) {
                measure.source_page = Some(*page);
            }
        }
    }
    let mut page_turns = Vec::new();
    for (index, (part, measures)) in composition.parts.iter_mut().zip(&all_measures).enumerate() {
        let duration = expand_part(
            measures,
            part,
            &mut composition.tempos,
            &mut composition.warnings,
            &mut event_count,
            if index == 0 {
                Some(&mut page_turns)
            } else {
                None
            },
        )?;
        composition.duration_ticks = composition.duration_ticks.max(duration);
    }
    if let Some(layout) = layout {
        composition.page_map = Some(PageMap {
            source_pages: layout.source_pages,
            turns: page_turns,
        });
    }
    composition.tempos.sort_by_key(|t| t.tick);
    composition
        .tempos
        .dedup_by(|a, b| a.tick == b.tick && (a.bpm - b.bpm).abs() < 0.001);
    if composition.tempos.is_empty() || composition.tempos[0].tick != 0 {
        warn(
            &mut composition.warnings,
            "No initial tempo marking; 120 BPM is assumed until the first explicit tempo.",
        );
        composition.tempos.insert(
            0,
            Tempo {
                tick: 0,
                bpm: 120.0,
            },
        );
    }
    if composition
        .tempos
        .windows(2)
        .any(|t| t[0].tick == t[1].tick)
    {
        warn(
            &mut composition.warnings,
            "Conflicting simultaneous tempo markings; last marking is used.",
        );
    }
    for tag in [
        "segno",
        "coda",
        "fine",
        "dynamics",
        "wedge",
        "pedal",
        "ornaments",
        "articulations",
        "fermata",
        "swing",
        "octave-shift",
        "measure-repeat",
        "scordatura",
        "glissando",
        "slide",
        "slur",
        "words",
        "instrument",
    ] {
        if root.descendants().any(|n| n.has_tag_name(tag)) {
            warn(
                &mut composition.warnings,
                format!("Unsupported {tag} expression/navigation is not performed."),
            );
        }
    }
    for node in root.descendants().filter(|n| n.has_tag_name("sound")) {
        if ["dacapo", "dalsegno", "tocoda", "fine", "forward-repeat"]
            .iter()
            .any(|a| node.attribute(*a).is_some())
        {
            warn(
                &mut composition.warnings,
                "D.C./D.S./Coda/Fine navigation is unsupported and not performed.",
            );
        }
    }
    Ok(composition)
}

fn navigation_signature(part: Node<'_, '_>) -> Vec<(usize, String)> {
    let mut signature = Vec::new();
    for (index, measure) in part
        .children()
        .filter(|n| n.has_tag_name("measure"))
        .enumerate()
    {
        for node in measure.descendants().filter(Node::is_element) {
            if ["segno", "coda", "fine"].contains(&node.tag_name().name()) {
                signature.push((index, node.tag_name().name().to_owned()));
            }
            if node.has_tag_name("sound") {
                for attribute in ["dacapo", "dalsegno", "tocoda", "fine", "forward-repeat"] {
                    if let Some(value) = node.attribute(attribute) {
                        signature.push((index, format!("{attribute}={value}")));
                    }
                }
            }
        }
    }
    signature
}

/// Keep accompaniment on one common clock even when OMR durations disagree.
/// The first specified repeat/endings value wins conflicts; missing markings
/// are shared across parts. Warnings make every such repair visible.
fn align_parts(
    parts: &mut [Vec<Measure>],
    models: &[Part],
    warnings: &mut Vec<String>,
) -> Result<()> {
    let count = parts.iter().map(Vec::len).max().unwrap_or(0);
    ensure!(
        count.saturating_mul(parts.len()) <= MAX_EVENTS,
        "Too many aligned part measures"
    );
    if parts.iter().any(|p| p.len() != count) {
        warn(
            warnings,
            "Measure counts differ between parts; absent trailing measures become rests. Verify bar alignment, especially any missing interior measure.",
        );
    }
    for part in parts.iter_mut() {
        part.resize_with(count, Measure::default);
    }
    for index in 0..count {
        let first = &parts[0][index];
        let duration = parts
            .iter()
            .map(|p| p[index].duration)
            .fold(0.0f64, f64::max);
        let forward = parts.iter().any(|p| p[index].forward_repeat);
        let backward = parts.iter().find_map(|p| p[index].backward_repeat);
        let endings = parts
            .iter()
            .find_map(|p| (!p[index].endings.is_empty()).then(|| p[index].endings.clone()))
            .unwrap_or_default();
        if parts
            .iter()
            .any(|p| (p[index].duration - duration).abs() > 0.000_001)
        {
            warn(warnings, format!(
                "Measure {} durations differ between parts; shared maximum length is used (review OMR timing).",
                index + 1
            ));
        }
        if parts.iter().any(|p| {
            !p[index].number.is_empty()
                && !first.number.is_empty()
                && p[index].number != first.number
        }) {
            warn(warnings, format!(
                "Measure {} labels differ between parts; ordinal alignment is used and must be reviewed.",
                index + 1
            ));
        }
        if parts.iter().any(|p| {
            p[index].forward_repeat != first.forward_repeat
                || p[index].backward_repeat != first.backward_repeat
                || p[index].endings != first.endings
        }) {
            let repeat_source = parts
                .iter()
                .position(|p| p[index].forward_repeat || p[index].backward_repeat.is_some())
                .map_or("none", |p| models[p].id.as_str());
            let ending_source = parts
                .iter()
                .position(|p| !p[index].endings.is_empty())
                .map_or("none", |p| models[p].id.as_str());
            warn(warnings, format!(
                "Measure {} repeat/endings differ between parts; a shared schedule applies to all parts (first specified repeats from {repeat_source}, endings from {ending_source}).",
                index + 1
            ));
        }
        for part in parts.iter_mut() {
            let measure = &mut part[index];
            measure.duration = duration;
            measure.forward_repeat = forward;
            measure.backward_repeat = backward;
            measure.endings.clone_from(&endings);
        }
    }
    Ok(())
}

fn parse_part(
    node: Node<'_, '_>,
    part: &mut Part,
    warnings: &mut Vec<String>,
    event_count: &mut usize,
) -> Result<Vec<Measure>> {
    let mut result = Vec::new();
    let mut divisions = 1.0;
    let mut nominal = 4.0;
    let mut transpose: HashMap<u16, i16> = HashMap::new();
    let mut ending = Vec::new();
    for measure_node in node.children().filter(|n| n.has_tag_name("measure")) {
        ensure!(result.len() < MAX_MEASURES, "Too many measures");
        let mut measure = Measure {
            number: measure_node.attribute("number").unwrap_or("").to_owned(),
            endings: ending.clone(),
            ..Measure::default()
        };
        let mut cursor = 0.0f64;
        let mut extent = 0.0f64;
        let mut previous: Option<(String, f64)> = None;
        for item in measure_node.children().filter(Node::is_element) {
            match item.tag_name().name() {
                "attributes" => {
                    divisions = number(item, "divisions", divisions)?;
                    ensure!(
                        divisions > 0.0 && divisions <= 1_000_000.0,
                        "Invalid divisions"
                    );
                    if let Some(staves) = text(item, "staves") {
                        part.staves = staves.parse().context("Invalid staff count")?;
                        ensure!((1..=128).contains(&part.staves), "Invalid staff count");
                    }
                    if let Some(time) = child(item, "time") {
                        nominal = 0.0;
                        for beats in time.children().filter(|n| n.has_tag_name("beats")) {
                            let mut count = 0.0;
                            for component in beats.text().unwrap_or("").split('+') {
                                let value: f64 =
                                    component.trim().parse().context("Invalid meter beats")?;
                                ensure!(value.is_finite() && value > 0.0, "Invalid meter beats");
                                count += value;
                            }
                            let unit: f64 = beats
                                .next_sibling_element()
                                .filter(|n| n.has_tag_name("beat-type"))
                                .and_then(|n| n.text())
                                .context("Missing meter beat-type")?
                                .trim()
                                .parse()
                                .context("Invalid meter beat-type")?;
                            ensure!(unit.is_finite() && unit > 0.0, "Invalid meter beat-type");
                            nominal += count * 4.0 / unit;
                        }
                    }
                    for trans in item.children().filter(|n| n.has_tag_name("transpose")) {
                        let staff = trans.attribute("number").unwrap_or("0").parse::<u16>()?;
                        let shift = number(trans, "chromatic", 0.0)?
                            + 12.0 * number(trans, "octave-change", 0.0)?;
                        ensure!(
                            shift.fract() == 0.0 && shift.abs() <= 127.0,
                            "Invalid transposition"
                        );
                        transpose.insert(staff, shift as i16);
                        if child(trans, "double").is_some() {
                            warn(warnings, "Octave-doubling transposition is unsupported.");
                        }
                    }
                }
                "backup" | "forward" => {
                    previous = None;
                    let duration = number(item, "duration", 0.0)? / divisions;
                    ensure!(duration >= 0.0, "Negative timing duration");
                    if item.has_tag_name("backup") {
                        cursor -= duration;
                        ensure!(cursor >= -0.000_001, "Backup precedes measure start");
                        cursor = cursor.max(0.0);
                    } else {
                        cursor += duration;
                        extent = extent.max(cursor);
                    }
                    ticks(cursor)?;
                }
                "note" => {
                    *event_count += 1;
                    ensure!(*event_count <= MAX_EVENTS, "Too many score events");
                    if child(item, "grace").is_some() {
                        warn(warnings, "Grace notes are omitted from playback.");
                        continue;
                    }
                    let duration = number(item, "duration", 0.0)? / divisions;
                    ensure!(
                        duration > 0.0,
                        "Non-grace note/rest has no positive duration"
                    );
                    let staff = text(item, "staff").unwrap_or("1").parse::<u16>()?;
                    ensure!((1..=128).contains(&staff), "Invalid note staff");
                    part.staves = part.staves.max(staff);
                    let voice = text(item, "voice").unwrap_or("1").to_owned();
                    let chord = child(item, "chord").is_some();
                    let beat = if chord {
                        let (previous_voice, start) =
                            previous.as_ref().context("Chord has no preceding note")?;
                        ensure!(previous_voice == &voice, "Chord changes voice");
                        *start
                    } else {
                        previous = Some((voice.clone(), cursor));
                        let start = cursor;
                        cursor += duration;
                        start
                    };
                    extent = extent.max(beat + duration);
                    ticks(extent)?;
                    if child(item, "rest").is_some() {
                        continue;
                    }
                    let Some(pitch) = child(item, "pitch") else {
                        warn(warnings, "Unpitched note is omitted from playback.");
                        continue;
                    };
                    let step = match text(pitch, "step").unwrap_or("") {
                        "C" => 0,
                        "D" => 2,
                        "E" => 4,
                        "F" => 5,
                        "G" => 7,
                        "A" => 9,
                        "B" => 11,
                        _ => bail!("Invalid pitch step"),
                    };
                    let octave = number(pitch, "octave", 4.0)?;
                    let alter = number(pitch, "alter", 0.0)?;
                    let raw_pitch = 12.0 * (octave + 1.0)
                        + f64::from(step)
                        + alter
                        + f64::from(
                            *transpose
                                .get(&staff)
                                .or_else(|| transpose.get(&0))
                                .unwrap_or(&0),
                        );
                    ensure!(
                        (0.0..=127.0).contains(&raw_pitch),
                        "Pitch outside MIDI range"
                    );
                    if raw_pitch.fract() != 0.0 {
                        warn(
                            warnings,
                            "Microtonal pitches are rounded to the nearest MIDI semitone.",
                        );
                    }
                    let mut tie_start = false;
                    let mut tie_stop = false;
                    for tie in item
                        .descendants()
                        .filter(|n| n.has_tag_name("tie") || n.has_tag_name("tied"))
                    {
                        tie_start |= tie.attribute("type") == Some("start");
                        tie_stop |= tie.attribute("type") == Some("stop");
                    }
                    measure.notes.push(RawNote {
                        beat,
                        duration,
                        pitch: raw_pitch.round() as u8,
                        staff,
                        voice,
                        tie_start,
                        tie_stop,
                    });
                }
                "direction" => {
                    let offset = number(item, "offset", 0.0)? / divisions;
                    let at = (cursor + offset).max(0.0);
                    let sound = child(item, "sound").and_then(|s| s.attribute("tempo"));
                    let mut tempo = sound
                        .map(str::parse::<f64>)
                        .transpose()
                        .context("Invalid tempo")?;
                    if tempo.is_none() {
                        if let Some(metro) =
                            item.descendants().find(|n| n.has_tag_name("metronome"))
                        {
                            let factor = match text(metro, "beat-unit").unwrap_or("quarter") {
                                "whole" => 4.0,
                                "half" => 2.0,
                                "quarter" => 1.0,
                                "eighth" => 0.5,
                                "16th" => 0.25,
                                _ => {
                                    warn(
                                        warnings,
                                        "Unsupported metronome beat unit; quarter note assumed.",
                                    );
                                    1.0
                                }
                            };
                            let dots = metro
                                .children()
                                .filter(|n| n.has_tag_name("beat-unit-dot"))
                                .count()
                                .min(4);
                            let dotted = 2.0 - 0.5f64.powi(dots as i32);
                            tempo = Some(number(metro, "per-minute", 120.0)? * factor * dotted);
                        }
                    }
                    if let Some(bpm) = tempo {
                        ensure!(
                            bpm.is_finite() && (1.0..=1000.0).contains(&bpm),
                            "Tempo outside 1..1000 BPM"
                        );
                        measure.tempos.push((at, bpm));
                    }
                }
                "sound" => {
                    if let Some(value) = item.attribute("tempo") {
                        let bpm: f64 = value.parse()?;
                        ensure!(
                            bpm.is_finite() && (1.0..=1000.0).contains(&bpm),
                            "Invalid tempo"
                        );
                        measure.tempos.push((cursor, bpm));
                    }
                }
                "barline" => {
                    if let Some(e) = child(item, "ending") {
                        let numbers = parse_endings(e.attribute("number").unwrap_or(""))?;
                        match e.attribute("type") {
                            Some("start") => {
                                ending = numbers;
                                measure.endings = ending.clone();
                            }
                            Some("stop" | "discontinue") => {
                                if measure.endings.is_empty() {
                                    measure.endings = numbers;
                                }
                                ending.clear();
                            }
                            _ => warn(warnings, "Unsupported ending type."),
                        }
                    }
                    if let Some(repeat) = child(item, "repeat") {
                        if item.attribute("location") == Some("middle") {
                            warn(
                                warnings,
                                "Mid-measure repeats are unsupported; measure boundaries are used.",
                            );
                        }
                        match repeat.attribute("direction") {
                            Some("forward") => measure.forward_repeat = true,
                            Some("backward") => {
                                let times: u8 = repeat.attribute("times").unwrap_or("2").parse()?;
                                ensure!((2..=8).contains(&times), "Repeat count outside 2..8");
                                measure.backward_repeat = Some(times);
                            }
                            _ => warn(warnings, "Unsupported repeat direction."),
                        }
                    }
                }
                _ => {}
            }
        }
        // MusicXML implicit measures and initial short measures are pickups.
        // Normal measures retain their metrical length even with sparse voices.
        measure.duration = if measure_node.attribute("implicit") == Some("yes")
            || (result.is_empty() && extent > 0.0 && extent < nominal)
        {
            extent
        } else {
            if extent < nominal {
                warn(
                    warnings,
                    "An underfull non-pickup measure is padded to its meter; review OMR durations.",
                );
            }
            extent.max(nominal)
        };
        if nominal > 0.0 && extent > nominal + 0.000_001 {
            warn(
                warnings,
                "An overfull measure exceeds its meter; review OMR durations and voice timing.",
            );
        }
        ensure!(measure.duration > 0.0, "Empty implicit measure");
        ticks(measure.duration)?;
        result.push(measure);
    }
    ensure!(!result.is_empty(), "Part has no measures");
    Ok(result)
}

fn parse_endings(value: &str) -> Result<Vec<u8>> {
    let mut values = Vec::new();
    for token in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((a, b)) = token.split_once('-') {
            let a: u8 = a.trim().parse()?;
            let b: u8 = b.trim().parse()?;
            ensure!(a > 0 && b >= a && b <= 8, "Invalid ending range");
            values.extend(a..=b);
        } else {
            let number: u8 = token.parse().context("Invalid ending number")?;
            ensure!((1..=8).contains(&number), "Ending number outside 1..8");
            values.push(number);
        }
    }
    ensure!(!values.is_empty(), "Empty ending numbers");
    Ok(values)
}

fn expand_part(
    measures: &[Measure],
    part: &mut Part,
    tempos: &mut Vec<Tempo>,
    warnings: &mut Vec<String>,
    event_count: &mut usize,
    mut page_turns: Option<&mut Vec<PageTurn>>,
) -> Result<u64> {
    let mut pc = 0usize;
    let mut start = 0usize;
    let mut pass = 1u8;
    let mut in_repeat = false;
    let mut beat = 0.0;
    let mut steps = 0usize;
    let mut ties: HashMap<(u16, String, u8), usize> = HashMap::new();
    while pc < measures.len() {
        steps += 1;
        ensure!(steps <= MAX_MEASURES * 8, "Repeat expansion exceeds limit");
        let measure = &measures[pc];
        if !in_repeat && measure.endings.is_empty() {
            pass = 1;
        }
        if measure.forward_repeat && (!in_repeat || pc != start) {
            if in_repeat {
                warn(
                    warnings,
                    "Nested repeats are unsupported; innermost repeat start is used.",
                );
            }
            start = pc;
            pass = 1;
            in_repeat = true;
        } else if measure.forward_repeat {
            in_repeat = true;
        }
        let include = measure.endings.is_empty() || measure.endings.contains(&pass);
        if include {
            if let (Some(page), Some(turns)) = (measure.source_page, page_turns.as_deref_mut()) {
                if turns.last().is_none_or(|last| last.page != page) {
                    turns.push(PageTurn {
                        tick: ticks(beat)?,
                        page,
                    });
                }
            }
            for raw in &measure.notes {
                *event_count += 1;
                ensure!(
                    *event_count <= MAX_EVENTS,
                    "Expanded score has too many events"
                );
                let at = ticks(beat + raw.beat)?;
                let end = ticks(beat + raw.beat + raw.duration)?;
                let key = (raw.staff, raw.voice.clone(), raw.pitch);
                let tied_index = if raw.tie_stop {
                    ties.remove(&key)
                } else {
                    None
                };
                let index = if let Some(index) = tied_index {
                    let existing: &mut Note = &mut part.notes[index];
                    if existing.start_tick + existing.duration_ticks == at {
                        existing.duration_ticks = end - existing.start_tick;
                        index
                    } else {
                        warn(warnings, "Noncontiguous tie was not joined.");
                        part.notes.push(note_from_raw(raw, at, end));
                        part.notes.len() - 1
                    }
                } else {
                    if raw.tie_stop {
                        warn(warnings, "Tie stop without a matching start.");
                    }
                    part.notes.push(note_from_raw(raw, at, end));
                    part.notes.len() - 1
                };
                if raw.tie_start {
                    ties.insert(key, index);
                }
            }
            for &(offset, bpm) in &measure.tempos {
                *event_count += 1;
                ensure!(
                    *event_count <= MAX_EVENTS,
                    "Expanded score has too many events"
                );
                tempos.push(Tempo {
                    tick: ticks(beat + offset)?,
                    bpm,
                });
            }
            beat += measure.duration;
            ticks(beat)?;
        }
        if let Some(times) = measure.backward_repeat {
            if pass < times {
                pass += 1;
                in_repeat = true;
                pc = start;
                continue;
            }
            in_repeat = false;
            // Keep pass through the following alternative ending.
            start = pc + 1;
        }
        pc += 1;
    }
    if !ties.is_empty() {
        warn(
            warnings,
            "Unclosed ties end at their written note duration.",
        );
    }
    part.notes.sort_by_key(|n| (n.start_tick, n.pitch));
    Ok(ticks(beat)?.max(
        part.notes
            .iter()
            .map(|n| n.start_tick + n.duration_ticks)
            .max()
            .unwrap_or(0),
    ))
}

fn note_from_raw(raw: &RawNote, start_tick: u64, end_tick: u64) -> Note {
    Note {
        start_tick,
        duration_ticks: end_tick.saturating_sub(start_tick).max(1),
        pitch: raw.pitch,
        velocity: 80,
        staff: raw.staff,
        voice: raw.voice.clone(),
    }
}
