use super::{Candidate, Detection, Instrument};
use anyhow::{bail, Result};
use regex::Regex;
use std::sync::OnceLock;

const CATALOGUE: &[(&str, &[&str])] = &[
    ("English horn", &["english horn", "cor anglais"]),
    (
        "Double bass",
        &["double bass", "contrabass", "contrabbasso", "contrebasse"],
    ),
    ("Saxophone", &["saxophone", "sax", "saxofono", "saxophon"]),
    (
        "Cello",
        &[
            "cello",
            "violoncello",
            "violoncelle",
            "violoncelli",
            "vc",
            "vlc",
        ],
    ),
    ("Violin", &["violin", "violino", "violon", "vln", "vl"]),
    ("Viola", &["viola", "vla"]),
    (
        "Clarinet",
        &["clarinet", "clarinette", "clarinetto", "klarinette", "cl"],
    ),
    ("Flute", &["flute", "flauto", "flote", "fl"]),
    ("Piccolo", &["piccolo", "ottavino"]),
    ("Oboe", &["oboe", "hautbois", "ob"]),
    (
        "Bassoon",
        &["bassoon", "fagott", "fagotto", "basson", "bsn"],
    ),
    (
        "Contrabassoon",
        &["contrabassoon", "contrafagotto", "kontrafagott"],
    ),
    ("Horn", &["french horn", "horn", "corno", "hn"]),
    ("Trumpet", &["trumpet", "trompette", "tromba", "tpt"]),
    ("Trombone", &["trombone", "posaune", "tbn"]),
    ("Tuba", &["tuba"]),
    ("Euphonium", &["euphonium", "euph"]),
    ("Piano", &["piano", "pianoforte", "klavier", "pf", "pno"]),
    ("Organ", &["organ", "orgue", "orgel", "organo"]),
    ("Keyboard", &["keyboard", "synthesizer"]),
    ("Harp", &["harp", "harpe", "arpa"]),
    ("Guitar", &["guitar", "guitare", "guitarra", "chitarra"]),
    ("Bass guitar", &["bass guitar", "electric bass"]),
    ("Recorder", &["recorder", "blockflote", "flauto dolce"]),
    ("Accordion", &["accordion", "accordeon"]),
    ("Mandolin", &["mandolin", "mandolino"]),
    ("Banjo", &["banjo"]),
    ("Ukulele", &["ukulele"]),
    ("Timpani", &["timpani", "pauken", "timbales"]),
    ("Percussion", &["percussion", "percussioni", "perc"]),
    ("Drum kit", &["drum kit", "drum set", "drums"]),
    ("Marimba", &["marimba"]),
    ("Xylophone", &["xylophone", "xilofono"]),
    ("Vibraphone", &["vibraphone", "vibrafono"]),
    ("Glockenspiel", &["glockenspiel"]),
    ("Voice", &["voice", "vocal", "voix", "gesang"]),
];

struct Rule {
    name: &'static str,
    pattern: Regex,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules: Vec<_> = CATALOGUE
            .iter()
            .map(|(name, aliases)| {
                let pattern = aliases
                    .iter()
                    .map(|a| regex::escape(a))
                    .collect::<Vec<_>>()
                    .join("|");
                Rule {
                    name,
                    pattern: Regex::new(&format!(r"\b(?:{pattern})\b")).unwrap(),
                }
            })
            .collect();
        // Longer names consume their words before generic names (English horn/Horn).
        rules.sort_by_key(|rule| {
            std::cmp::Reverse(
                CATALOGUE
                    .iter()
                    .find(|(name, _)| *name == rule.name)
                    .unwrap()
                    .1
                    .iter()
                    .map(|a| a.len())
                    .max()
                    .unwrap_or(0),
            )
        });
        rules
    })
}

pub fn catalogue() -> Vec<&'static str> {
    let mut names: Vec<_> = CATALOGUE.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

fn normalized(text: &str) -> String {
    let mut output = String::new();
    let mut previous = ' ';
    for c in text.to_lowercase().chars() {
        let c = match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'á' | 'â' | 'ä' => 'a',
            'ö' | 'ô' | 'ó' => 'o',
            'ü' | 'ù' | 'ú' => 'u',
            'ï' | 'î' | 'í' => 'i',
            'ç' => 'c',
            '♭' => 'b',
            '♯' => 's',
            _ => c,
        };
        if (previous.is_alphabetic() && c.is_ascii_digit())
            || (previous.is_ascii_digit() && c.is_alphabetic())
        {
            output.push(' ');
        }
        output.push(if c.is_alphanumeric() { c } else { ' ' });
        previous = c;
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn part(word: &str) -> Option<String> {
    if let Ok(number) = word.parse::<u32>() {
        return (number > 0 && number <= 32).then(|| number.to_string());
    }
    ["i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"]
        .iter()
        .position(|roman| *roman == word)
        .map(|n| (n + 1).to_string())
}

fn variant(name: &str, before: &str, after: &str) -> String {
    let allowed: &[&str] = match name {
        "Saxophone" => &["sopranino", "soprano", "alto", "tenor", "baritone", "bass"],
        "Clarinet" => &["bass", "contrabass"],
        "Flute" => &["alto", "bass"],
        "Trombone" => &["alto", "tenor", "bass"],
        "Recorder" | "Voice" => &["soprano", "alto", "tenor", "bass"],
        _ => &[],
    };
    before
        .split_whitespace()
        .rev()
        .take(2)
        .chain(after.split_whitespace().take(2))
        .find(|word| allowed.contains(word))
        .map(|word| {
            let mut chars = word.chars();
            chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
        })
        .unwrap_or_default()
}

pub fn line(text: &str, page: u32, method: &str, confidence: Option<f32>, result: &mut Detection) {
    if text.trim().is_empty() || result.candidates.len() >= 32 {
        return;
    }
    let text: String = text.trim().chars().take(180).collect();
    let normal = normalized(&text);
    let mut remaining = normal.clone();
    let mut found = Vec::new();
    for rule in rules() {
        for matched in rule.pattern.find_iter(&remaining.clone()) {
            let before = &normal[..matched.start()];
            let after = &normal[matched.end()..];
            let number = after
                .split_whitespace()
                .take(5)
                .take_while(|w| *w != "and")
                .find_map(part)
                .or_else(|| before.split_whitespace().rev().take(2).find_map(part))
                .unwrap_or_default();
            let explicit = matched
                .as_str()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count()
                >= 4
                || !number.is_empty();
            found.push((
                Instrument {
                    name: rule.name.into(),
                    variant: variant(rule.name, before, after),
                    part: number,
                },
                explicit,
            ));
        }
        // Preserve byte offsets for subsequent rules.
        remaining = rule
            .pattern
            .replace_all(&remaining, |captures: &regex::Captures<'_>| {
                " ".repeat(captures[0].len())
            })
            .into_owned();
    }
    if found.is_empty() {
        let ambiguous = normal
            .split_whitespace()
            .next()
            .is_some_and(|word| ["alto", "tenor", "bass", "soprano", "baritone"].contains(&word));
        if ambiguous && result.warnings.len() < 40 {
            let warning = format!("Page {page}: ambiguous label “{text}”; no instrument assigned.");
            if !result.warnings.contains(&warning) {
                result.warnings.push(warning);
            }
        }
        return;
    }
    let residue = remaining
        .split_whitespace()
        .filter(|word| {
            ![
                "alto",
                "tenor",
                "soprano",
                "sopranino",
                "baritone",
                "bass",
                "contrabass",
                "solo",
                "part",
                "in",
                "and",
                "bb",
                "eb",
                "ab",
                "bes",
                "es",
                "flat",
                "sharp",
                "b",
                "e",
                "a",
                "c",
                "d",
                "f",
                "g",
            ]
            .contains(word)
                && part(word).is_none()
        })
        .collect::<Vec<_>>();
    let clear = residue.is_empty()
        && normal.split_whitespace().count() <= 12
        && confidence.is_none_or(|score| score >= 70.0);
    for (instrument, explicit) in found {
        let clear = clear && explicit;
        if let Some(old) = result
            .candidates
            .iter_mut()
            .find(|old| old.instrument == instrument)
        {
            if clear && !old.clear {
                *old = Candidate {
                    instrument,
                    evidence: text.clone(),
                    page,
                    method: method.into(),
                    clear,
                    ocr_confidence: confidence,
                };
            }
        } else if result.candidates.len() < 32 {
            result.candidates.push(Candidate {
                instrument,
                evidence: text.clone(),
                page,
                method: method.into(),
                clear,
                ocr_confidence: confidence,
            });
        }
    }
}

pub fn text(text: &str, page: u32, method: &str, result: &mut Detection) {
    for row in text.lines().filter(|line| !line.trim().is_empty()).take(60) {
        line(row, page, method, None, result);
    }
}

pub fn normalize_manual(values: &[Instrument]) -> Result<Vec<Instrument>> {
    let mut output = Vec::new();
    for value in values {
        let name = value.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty()
            || name.chars().count() > 80
            || value.variant.chars().count() > 64
            || value.part.chars().count() > 16
        {
            bail!("Instrument needs a name (up to 80 characters); subtype/part are limited to 64/16 characters");
        }
        let key = normalized(&name);
        let canonical = CATALOGUE
            .iter()
            .find(|(n, aliases)| normalized(n) == key || aliases.iter().any(|a| *a == key));
        output.push(Instrument {
            name: canonical.map_or(name, |(name, _)| name.to_string()),
            variant: value.variant.trim().into(),
            part: part(&value.part.to_lowercase()).unwrap_or_else(|| value.part.trim().into()),
        });
    }
    output.sort();
    output.dedup();
    if output.len() > 32 {
        bail!("A score supports up to 32 instrument entries");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_parts_subtypes_and_multiple_instruments_are_recognized() {
        let mut result = Detection::default();
        text(
            "Alto Saxophone 1\nSax 2\nVioloncello II\nCello II\nPiano\nViolin I",
            1,
            "PDF text",
            &mut result,
        );
        assert!(result.candidates.iter().all(|c| c.clear));
        assert!(result.candidates.iter().any(|c| c.instrument
            == Instrument {
                name: "Saxophone".into(),
                variant: "Alto".into(),
                part: "1".into()
            }));
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|c| c.instrument.name == "Cello")
                .count(),
            1
        );
        assert!(result
            .candidates
            .iter()
            .any(|c| c.instrument.name == "Piano"));
    }

    #[test]
    fn ambiguous_ranges_clefs_and_prose_are_not_automatically_assigned() {
        let mut result = Detection::default();
        text("Alto 1\nBass\nTenor clef\nConcerto for cello and piano\nEnglish horn\nSaxophone concerto", 1, "PDF text", &mut result);
        assert_eq!(result.warnings.len(), 3);
        assert!(result
            .candidates
            .iter()
            .filter(|c| c.clear)
            .all(|c| c.instrument.name == "English horn"));
        assert!(!result
            .candidates
            .iter()
            .any(|c| c.instrument.name == "Horn"));
        assert!(result
            .candidates
            .iter()
            .any(|c| c.instrument.name == "Cello" && !c.clear));
    }

    #[test]
    fn low_confidence_ocr_and_manual_empty_names_are_not_clear() {
        let mut result = Detection::default();
        line("Cello", 1, "OCR", Some(35.0), &mut result);
        assert!(!result.candidates[0].clear);
        assert!(normalize_manual(&[Instrument::default()]).is_err());
        assert_eq!(
            normalize_manual(&[Instrument {
                name: "violoncello".into(),
                part: "II".into(),
                ..Default::default()
            }])
            .unwrap()[0]
                .name,
            "Cello"
        );
    }
}
