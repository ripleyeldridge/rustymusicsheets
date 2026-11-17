//! Audiveris source-sheet annotations identify physical input pages. MusicXML
//! page-number is only a printed label, never a PDF page index.
use super::{child, Node, MAX_MEASURES};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct SourceLayout {
    pub source_pages: Vec<u32>,
    pub measure_pages: Vec<u32>,
}

pub(super) fn source_layout(root: Node<'_, '_>) -> Result<SourceLayout> {
    ensure!(
        root.descendants().any(|n| n.has_tag_name("software")
            && n.text().is_some_and(|s| s.trim().starts_with("Audiveris"))),
        "MusicXML has no Audiveris original-page provenance; recognize the original score"
    );
    let mut sheets = BTreeMap::new();
    for field in root
        .descendants()
        .filter(|n| n.has_tag_name("miscellaneous-field"))
    {
        let Some(number) = field
            .attribute("name")
            .and_then(|n| n.strip_prefix("source-sheet-"))
        else {
            continue;
        };
        let number: u32 = number
            .parse()
            .context("Invalid original source-sheet number")?;
        ensure!(number > 0, "Original source-sheet numbers must be positive");
        ensure!(
            sheets.len() < MAX_MEASURES,
            "Too many original source sheets"
        );
        let mut ids = BTreeSet::new();
        let mut previous = 0;
        for value in field.text().unwrap_or("").split_whitespace() {
            let id: u32 = value.parse().context("Invalid source system reference")?;
            ensure!(
                id > previous && ids.insert(id) && ids.len() <= MAX_MEASURES,
                "Ambiguous source system references"
            );
            previous = id;
        }
        ensure!(!ids.is_empty(), "Source sheet has no system references");
        ensure!(
            sheets.insert(number - 1, ids.len()).is_none(),
            "Duplicate source-sheet provenance"
        );
    }
    ensure!(
        !sheets.is_empty(),
        "MusicXML lacks original source-sheet references; recognize the original score"
    );
    let source_pages: Vec<_> = sheets.keys().copied().collect();
    let expected_systems: Vec<_> = sheets.values().copied().collect();
    let mut reference: Option<(Vec<String>, Vec<u32>)> = None;
    for part in root.children().filter(|n| n.has_tag_name("part")) {
        let measures: Vec<_> = part
            .children()
            .filter(|n| n.has_tag_name("measure"))
            .collect();
        ensure!(
            !measures.is_empty() && measures.len() <= MAX_MEASURES,
            "Invalid pagination measure count"
        );
        let labels: Vec<_> = measures
            .iter()
            .map(|n| n.attribute("number").unwrap_or("").to_owned())
            .collect();
        if !measures.iter().any(|n| child(*n, "print").is_some()) {
            continue;
        }
        ensure!(
            child(measures[0], "print").is_some(),
            "Initial page/system layout is missing"
        );
        let mut page = 0usize;
        let mut systems = vec![0usize; source_pages.len()];
        let mut measure_pages = Vec::with_capacity(measures.len());
        for (index, measure) in measures.iter().enumerate() {
            let prints: Vec<_> = measure
                .children()
                .filter(|n| n.has_tag_name("print"))
                .collect();
            ensure!(prints.len() <= 1, "Conflicting print records in a measure");
            let print = prints.first().copied();
            ensure!(
                print.is_none_or(|p| p.attribute("blank-page").is_none()),
                "Blank-page layout needs manual review"
            );
            let new_page =
                index > 0 && print.is_some_and(|p| p.attribute("new-page") == Some("yes"));
            if new_page {
                page += 1;
            }
            ensure!(
                page < source_pages.len(),
                "Page breaks disagree with original source sheets"
            );
            if index == 0
                || new_page
                || print.is_some_and(|p| p.attribute("new-system") == Some("yes"))
            {
                systems[page] += 1;
            }
            measure_pages.push(source_pages[page]);
        }
        ensure!(
            page + 1 == source_pages.len() && systems == expected_systems,
            "Page/system layout differs from original source-sheet provenance"
        );
        if let Some((known_labels, known_pages)) = &reference {
            ensure!(
                &labels == known_labels && &measure_pages == known_pages,
                "Pagination conflicts between parts"
            );
        } else {
            reference = Some((labels, measure_pages));
        }
    }
    let (labels, measure_pages) =
        reference.context("MusicXML has no usable original page breaks")?;
    for part in root.children().filter(|n| n.has_tag_name("part")) {
        let candidate: Vec<_> = part
            .children()
            .filter(|n| n.has_tag_name("measure"))
            .map(|n| n.attribute("number").unwrap_or("").to_owned())
            .collect();
        ensure!(candidate == labels, "Part measure alignment is ambiguous");
    }
    Ok(SourceLayout {
        source_pages,
        measure_pages,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{parse_musicxml, PageTurn};

    #[test]
    fn lead_time_counts_played_measures_without_bouncing_or_skipping_pages() {
        use super::super::PageMap;
        let map = PageMap {
            source_pages: vec![0, 1, 2],
            turns: vec![
                PageTurn { tick: 0, page: 0 },
                PageTurn { tick: 400, page: 1 },
                PageTurn { tick: 800, page: 0 },
                PageTurn {
                    tick: 1200,
                    page: 2,
                },
            ],
            measure_ticks: (0..16).map(|n| n * 100).collect(),
        };
        assert_eq!(map.page_at(399, 0), Some(0));
        assert_eq!(map.page_at(400, 0), Some(1));
        for tick in [300, 350, 399, 400, 450, 699] {
            assert_eq!(map.page_at(tick, 1), Some(1));
        }
        assert_eq!(map.page_at(299, 1), Some(0));
        assert_eq!(map.page_at(200, 2), Some(1));
        for tick in [700, 750, 799, 800, 850] {
            assert_eq!(map.page_at(tick, 1), Some(0));
        }
        assert_eq!(map.page_at(1100, 1), Some(2));
        assert_eq!(map.page_at(0, 999), Some(0));
        assert_eq!(map.page_at(99, 999), Some(0));
        assert_eq!(map.page_at(100, 999), Some(1));
        let short = PageMap {
            source_pages: vec![0, 1, 2],
            turns: vec![
                PageTurn { tick: 0, page: 0 },
                PageTurn { tick: 100, page: 1 },
                PageTurn { tick: 200, page: 2 },
            ],
            measure_ticks: vec![0, 100, 200],
        };
        assert_eq!(short.page_at(99, 999), Some(0));
        assert_eq!(short.page_at(100, 999), Some(1));
    }

    #[test]
    fn variable_length_measures_are_not_approximated_with_fixed_seconds() {
        use super::super::PageMap;
        let map = PageMap {
            source_pages: vec![0, 1],
            turns: vec![
                PageTurn { tick: 0, page: 0 },
                PageTurn { tick: 900, page: 1 },
            ],
            measure_ticks: vec![0, 100, 350, 400, 900, 1000],
        };
        assert_eq!(map.page_at(399, 1), Some(0));
        assert_eq!(map.page_at(400, 1), Some(1));
        assert_eq!(map.page_at(350, 2), Some(1));
    }

    fn xml(fields: &str, first: &str, second: Option<&str>) -> String {
        let extra = if second.is_some() {
            r#"<score-part id="P2"><part-name>Piano</part-name></score-part>"#
        } else {
            ""
        };
        let second = second
            .map(|body| format!(r#"<part id="P2">{body}</part>"#))
            .unwrap_or_default();
        format!(
            r#"<score-partwise><identification><encoding><software>Audiveris generated fixture</software></encoding>
        <miscellaneous>{fields}</miscellaneous></identification>
        <part-list><score-part id="P1"><part-name>Cello</part-name></score-part>{extra}</part-list>
        <part id="P1">{first}</part>{second}</score-partwise>"#
        )
    }
    fn sheet(number: u32, systems: &str) -> String {
        format!(
            r#"<miscellaneous-field name="source-sheet-{number}">{systems}</miscellaneous-field>"#
        )
    }
    fn measure(number: u32, print: &str, bars: &str) -> String {
        format!(
            r#"<measure number="{number}" implicit="yes">{print}<note><rest/><duration>1</duration></note>{bars}</measure>"#
        )
    }
    fn two_pages() -> String {
        measure(1, "<print/>", "") + &measure(2, r#"<print new-page="yes" page-number="99"/>"#, "")
    }

    #[test]
    fn original_sheet_numbers_handle_covers_gaps_and_printed_labels() {
        let fields = sheet(2, "3") + &sheet(4, "1");
        let score = parse_musicxml(&xml(&fields, &two_pages(), None)).unwrap();
        let map = score.page_map.unwrap();
        assert_eq!(map.source_pages, [1, 3]);
        assert_eq!(
            map.turns,
            [
                PageTurn { tick: 0, page: 1 },
                PageTurn { tick: 960, page: 3 }
            ]
        );
    }

    #[test]
    fn empty_implicit_measure_keeps_shared_original_page_boundary() {
        let fields = sheet(1, "1") + &sheet(2, "1");
        let first = measure(1, "<print/>", "")
            + r#"<measure number="2" implicit="yes"><print new-page="yes"/></measure>"#;
        let second = measure(1, "", "") + &measure(2, "", "");
        let c = parse_musicxml(&xml(&fields, &first, Some(&second))).unwrap();
        assert_eq!(c.duration_ticks, 1920);
        let map = c
            .page_map
            .expect("Rest placeholders must preserve ordinal page mapping");
        assert_eq!(map.measure_ticks, [0, 960]);
        assert_eq!(
            map.turns,
            [
                PageTurn { tick: 0, page: 0 },
                PageTurn { tick: 960, page: 1 }
            ]
        );
    }
    #[test]
    fn page_events_follow_repeats_alternative_endings_and_rest_only_boundaries() {
        let fields = sheet(1, "1") + &sheet(2, "1") + &sheet(3, "1");
        let first = measure(
            1,
            "<print/>",
            r#"<barline location="left"><repeat direction="forward"/></barline>"#,
        ) + &measure(
            2,
            r#"<print new-page="yes"/>"#,
            r#"<barline><ending number="1" type="start"/><repeat direction="backward"/></barline><barline><ending number="1" type="stop"/></barline>"#,
        ) + &measure(
            3,
            r#"<print new-page="yes"/>"#,
            r#"<barline><ending number="2" type="start"/></barline><barline><ending number="2" type="stop"/></barline>"#,
        );
        let score = parse_musicxml(&xml(&fields, &first, None)).unwrap();
        assert!(score.parts[0].notes.is_empty());
        assert_eq!(score.duration_ticks, 3840);
        let map = score.page_map.unwrap();
        assert_eq!(map.measure_ticks, vec![0, 960, 1920, 2880]);
        assert_eq!(
            map.turns,
            [
                PageTurn { tick: 0, page: 0 },
                PageTurn { tick: 960, page: 1 },
                PageTurn {
                    tick: 1920,
                    page: 0
                },
                PageTurn {
                    tick: 2880,
                    page: 2
                },
            ]
        );
    }

    #[test]
    fn absent_or_inconsistent_layout_disables_following_but_not_playback() {
        let good = sheet(1, "1") + &sheet(2, "1");
        for fields in [
            String::new(),
            sheet(1, "1 2") + &sheet(2, "1"),
            good.clone() + &sheet(1, "1"),
            sheet(0, "1"),
        ] {
            let score = parse_musicxml(&xml(&fields, &two_pages(), None)).unwrap();
            assert!(score.page_map.is_none());
            assert!(score
                .warnings
                .iter()
                .any(|w| w.contains("Automatic page turning unavailable")));
            assert_eq!(score.duration_ticks, 1920);
        }
        let conflict = measure(1, "<print/>", "") + &measure(2, r#"<print new-system="yes"/>"#, "");
        assert!(parse_musicxml(&xml(&good, &two_pages(), Some(&conflict)))
            .unwrap()
            .page_map
            .is_none());
        assert!(
            parse_musicxml(&xml(&good, &two_pages(), Some(&measure(1, "<print/>", ""))))
                .unwrap()
                .page_map
                .is_none()
        );
        let reflowed = two_pages().replace("new-page=\"yes\"", "new-page=\"yes\" blank-page=\"1\"");
        assert!(parse_musicxml(&xml(&good, &reflowed, None))
            .unwrap()
            .page_map
            .is_none());
    }

    #[test]
    fn subordinate_parts_can_omit_prints_but_must_match_measure_alignment() {
        let fields = sheet(1, "1") + &sheet(2, "1");
        let no_prints = measure(1, "", "") + &measure(2, "", "");
        let score = parse_musicxml(&xml(&fields, &no_prints, Some(&two_pages()))).unwrap();
        assert!(score.page_map.is_some());
        let wrong_labels = no_prints.replace(r#"number="2""#, r#"number="3""#);
        assert!(
            parse_musicxml(&xml(&fields, &wrong_labels, Some(&two_pages())))
                .unwrap()
                .page_map
                .is_none()
        );
    }
}
