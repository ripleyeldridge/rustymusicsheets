use super::*;
use std::collections::HashSet;

fn frame(
    help: &mut Help,
    ctx: &egui::Context,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::TopBottomPanel::top("help_test_menu").show(ctx, |ui| help.menu(ui));
            if help.open {
                help.show(ctx);
            } else {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label("Test workspace");
                });
            }
        },
    )
}

#[test]
fn annotation_docs_preserve_originals_and_state_storage_and_device_limits() {
    let article = topics()
        .iter()
        .find(|t| t.id == "annotations")
        .unwrap()
        .body;
    for documentation in [
        include_str!("../../README.md"),
        include_str!("../../README.txt"),
        include_str!("../../assets/rustymusicsheets.1"),
        article,
    ] {
        let text = documentation
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for term in [
            "Stroke eraser",
            "Hand / Pan",
            "Save now",
            "annotations/",
            "ink-v1.json",
            "200,000",
            "10,000",
            "palm rejection",
            "Pressure",
        ] {
            assert!(
                text.to_lowercase().contains(&term.to_lowercase()),
                "Missing annotation guidance: {term}"
            );
        }
    }
    assert!(article.contains(&format!("Available in v{VERSION}")));
    assert!(article.contains("MIDI exports do not include drawn ink"));
}

#[test]
fn current_docs_and_embedded_music_reference_remain_consistent() {
    let article = |id| topics().iter().find(|topic| topic.id == id).unwrap().body;
    for term in [
        "Master volume",
        "Turn pages early by",
        "Browse",
        "Previous page / Next page",
        "Combine parts",
        "Uncombine",
        "Save and unload instruments",
        ".audio-v3.json",
        "Export original MusicXML",
    ] {
        assert!(
            article("music").contains(term),
            "Missing music documentation: {term}"
        );
        assert!(!search(term).is_empty(), "Feature not searchable: {term}");
    }
    let manual = include_str!("../../assets/rustymusicsheets.1").replace(r"\-", "-");
    for flag in [
        "--omr",
        "--inspect-musicxml",
        "--midi",
        "--program",
        "--play-seconds",
        "--soundfont",
        "--fluidsynth",
    ] {
        assert!(
            article("cli").contains(flag),
            "Missing embedded command: {flag}"
        );
        assert!(manual.contains(flag), "Missing man-page command: {flag}");
    }
    assert!(manual.contains(&format!("RustyMusicSheets {VERSION}")));
    assert!(include_str!("../../README.md").contains(&format!("**Version:** {VERSION}")));
    assert!(include_str!("../../README.txt")
        .starts_with(&format!("RustyMusicSheets {VERSION} preview")));
}

#[test]
fn published_release_docs_link_current_assets_and_distinguish_android_limits() {
    let release =
        format!("https://github.com/ripleyeldridge/rustymusicsheets/releases/tag/v{VERSION}");
    let apk = format!("RustyMusicSheets-{VERSION}-android-arm64-debug.apk");
    for documentation in [
        include_str!("../../README.md"),
        include_str!("../../README.txt"),
        include_str!("../../android/README.txt"),
    ] {
        assert!(documentation.contains(&release));
        for term in [&apk, "SHA256SUMS", "build-record.json"] {
            assert!(
                documentation.contains(term),
                "Missing release artifact: {term}"
            );
        }
        let normalized = documentation
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(normalized.contains("not executed on a device"));
        assert!(normalized.contains("debug-signed"));
        assert!(normalized.contains("signer"));
    }
    assert!(include_str!("../../README.md")
        .contains(&format!("## Latest published preview: v{VERSION}")));
    assert!(include_str!("../../README.txt").contains("contains 30 subjects"));
}

#[test]
fn installation_guides_cover_verified_package_upgrades_and_quarantine_retry() {
    let requirements = topics()
        .iter()
        .find(|topic| topic.id == "requirements")
        .unwrap()
        .body;
    for documentation in [
        include_str!("../../README.md"),
        include_str!("../../README.txt"),
        include_str!("../../assets/rustymusicsheets.1"),
        requirements,
    ] {
        let normalized = documentation
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for term in [
            "dpkg --print-architecture".to_owned(),
            "getconf GNU_LIBC_VERSION".to_owned(),
            "sha256sum --ignore-missing -c SHA256SUMS".to_owned(),
            format!("sudo apt install ./rustymusicsheets_{VERSION}-1_amd64.deb"),
            "annotations/".to_owned(),
            "Retry quarantined / rescan".to_owned(),
            "non-blocking notification".to_owned(),
        ] {
            assert!(
                normalized.contains(&term),
                "Missing upgrade guidance: {term}"
            );
        }
        assert!(!normalized.contains("preflight blocks new wrapper"));
    }
}

#[test]
fn music_guides_distinguish_packaged_host_source_builds_and_v3_layout() {
    let embedded = ["requirements", "music", "audio-host"]
        .iter()
        .map(|id| topics().iter().find(|topic| topic.id == *id).unwrap().body)
        .collect::<Vec<_>>()
        .join(" ");
    for documentation in [
        include_str!("../../README.md"),
        include_str!("../../README.txt"),
        include_str!("../../assets/rustymusicsheets.1"),
        &embedded,
    ] {
        let normalized = documentation
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for term in [
            "/usr/libexec/rustymusicsheets/rms-audio-host",
            "SDK-enabled",
            "2.43",
            "--vst3-sdk",
            "editor_keepalive",
            "sfizz",
            "Save and unload instruments",
            "Uncombine",
            ".audio-v3.json",
            "Export original MusicXML",
        ] {
            assert!(
                normalized.contains(term),
                "Missing v0.4 music guidance: {term}"
            );
        }
        assert!(!normalized.contains("New metadata uses .audio-v2.json"));
        assert!(!normalized.contains("New mix metadata uses `.audio-v2.json`"));
    }
}

#[test]
fn audio_host_setup_documents_dependencies_activation_and_separate_builds() {
    let article = topics()
        .iter()
        .find(|topic| topic.id == "audio-host")
        .unwrap()
        .body;
    for documentation in [
        include_str!("../../README.md"),
        include_str!("../../README.txt"),
        include_str!("../../assets/rustymusicsheets.1"),
        article,
    ] {
        for term in [
            "libjack-jackd2-dev",
            "libfluidsynth-dev",
            "libssl-dev",
            "pipewire-jack",
            "cargo build --locked --release",
            "scripts/build-audio-host.py --test",
            "target/audio-host/rms-audio-host",
            "--core-only",
            "FluidSynth (legacy",
            "9.22 and newer",
            "11.x",
            "5.1.1",
            "non-blocking notification",
            "far left",
        ] {
            assert!(
                documentation.contains(term),
                "Missing audio-host setup instruction: {term}"
            );
        }
    }
}

fn visible_text(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|shape| {
        if let egui::Shape::Text(label) = &shape.shape {
            let bounds = egui::Rect::from_min_size(label.pos, label.galley.size());
            let visible = bounds.intersect(shape.clip_rect);
            if label.galley.job.text.contains(needle)
                && visible.height() > 8.0
                && visible.width() > 20.0
            {
                return Some(visible.center());
            }
        }
        None
    })
}

fn click(help: &mut Help, ctx: &egui::Context, size: [f32; 2], label: &str) {
    for _ in 0..3 {
        frame(help, ctx, size, vec![]);
    }
    let output = frame(help, ctx, size, vec![]);
    let pos = visible_text(&output, label).unwrap_or_else(|| panic!("Missing control: {label}"));
    for pressed in [true, false] {
        frame(
            help,
            ctx,
            size,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn embedded_articles_are_unique_and_cover_all_workflows() {
    let ids: HashSet<_> = topics().iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), topics().len());
    assert_eq!(topics().len(), 30);
    for id in [
        "welcome",
        "layout",
        "requirements",
        "formats",
        "import",
        "folders",
        "watching",
        "rescan",
        "browse",
        "metadata",
        "display",
        "instrument-detect",
        "instrument-apply",
        "instrument-manual",
        "instrument-auto",
        "music",
        "audio-host",
        "lists",
        "list-order",
        "reading",
        "bookmarks",
        "annotations",
        "performance",
        "shortcuts",
        "settings",
        "backups",
        "cli",
        "troubleshooting",
        "limits",
        "about",
    ] {
        assert!(ids.contains(id), "Missing {id}");
    }
    for topic in topics() {
        assert!(!topic.category.is_empty());
        assert!(!topic.title.is_empty());
        assert!(topic.body.len() > 200, "Empty article: {}", topic.id);
    }
}

#[test]
fn search_matches_body_keywords_and_all_terms_case_insensitively() {
    let ids = |query| {
        search(query)
            .iter()
            .map(|&i| topics()[i].id)
            .collect::<Vec<_>>()
    };
    assert!(ids("manually protected replacement").contains(&"instrument-apply"));
    assert!(ids("WaL").contains(&"backups"));
    assert!(ids("USB Bluetooth").contains(&"shortcuts"));
    assert!(ids("large libraries").contains(&"instrument-detect"));
    assert!(ids("audio host latency").contains(&"audio-host"));
    assert!(ids("Alto Violoncello").contains(&"instrument-manual"));
    assert!(ids("Surface Pen").contains(&"annotations"));
    assert!(ids("ink-v1.json").contains(&"annotations"));
    assert_eq!(search("  \t ").len(), topics().len());
    assert!(search("no_such_help_word").is_empty());
    assert!(ids(VERSION).contains(&"about"));
    assert!(ids("Ripley Eldridge").contains(&"about"));
    assert!(ids(AUTHOR_URL).contains(&"about"));
}

#[test]
fn about_displays_release_identity_at_small_and_large_sizes() {
    assert_eq!(AUTHOR, "Ripley Eldridge");
    assert_eq!(AUTHOR_URL, "https://github.com/ripleyeldridge");
    assert_eq!(RELEASE_YEAR, "2026");
    let ctx = egui::Context::default();
    let mut help = Help::default();
    help.open_topic("about");
    for size in [[1280.0, 900.0], [640.0, 360.0], [800.0, 500.0]] {
        for _ in 0..3 {
            frame(&mut help, &ctx, size, vec![]);
        }
        let output = frame(&mut help, &ctx, size, vec![]);
        for label in [
            format!("Version: {VERSION}"),
            format!("Author: {AUTHOR}"),
            format!("Release year: {RELEASE_YEAR}"),
            "Close help".into(),
        ] {
            assert!(visible_text(&output, &label).is_some(), "{size:?}: {label}");
        }
    }
}

#[test]
fn about_github_link_targets_the_author_profile() {
    let ctx = egui::Context::default();
    let mut help = Help::default();
    help.open_topic("about");
    let size = [1280.0, 900.0];
    for _ in 0..3 {
        frame(&mut help, &ctx, size, vec![]);
    }
    let output = frame(&mut help, &ctx, size, vec![]);
    let pos = visible_text(&output, "GitHub: ripleyeldridge").unwrap();
    for pressed in [true, false] {
        let output = frame(
            &mut help,
            &ctx,
            size,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        if !pressed {
            assert!(output.platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::OpenUrl(url) if url.url == AUTHOR_URL)
            ));
        }
    }
}

#[test]
fn clicking_search_result_opens_its_article_in_both_layouts() {
    for size in [[1280.0, 900.0], [640.0, 360.0]] {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut help = Help::default();
        help.open_topic("welcome");
        help.query = "Bookmarks remembered".into();
        click(&mut help, &ctx, size, "Bookmarks and remembered positions");
        assert_eq!(topics()[help.selected].id, "bookmarks");
        assert!(!help.compact_topics);
        let output = frame(&mut help, &ctx, size, vec![]);
        assert!(visible_text(&output, "click Bookmark to toggle").is_some());
        click(&mut help, &ctx, size, "Close help");
        assert!(!help.open);
    }
}

#[test]
fn search_editor_and_no_results_clear_action_work() {
    let ctx = egui::Context::default();
    let mut help = Help::default();
    help.open_topic("welcome");
    let size = [640.0, 360.0];
    frame(&mut help, &ctx, size, vec![]);
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("help_search")));
    frame(
        &mut help,
        &ctx,
        size,
        vec![egui::Event::Text("no_such_help_word".into())],
    );
    assert_eq!(help.query, "no_such_help_word");
    let output = frame(&mut help, &ctx, size, vec![]);
    assert!(visible_text(&output, "No matching help topics").is_some());
    click(&mut help, &ctx, size, "Clear");
    assert!(help.query.is_empty());
    let output = frame(&mut help, &ctx, size, vec![]);
    assert!(visible_text(&output, "Getting started").is_some());
}

#[test]
fn compact_topic_tree_and_long_article_can_scroll_to_the_end() {
    for article in [false, true] {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut help = Help::default();
        help.open_topic(if article {
            "troubleshooting"
        } else {
            "welcome"
        });
        let mut reached = false;
        for i in 0..40 {
            let events = if i > 2 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(450.0, 280.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -300.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                vec![]
            };
            let output = frame(&mut help, &ctx, [640.0, 360.0], events);
            assert!(visible_text(&output, "Close help").is_some());
            reached |= visible_text(
                &output,
                if article {
                    "Picker missing?"
                } else {
                    "About RustyMusicSheets"
                },
            )
            .is_some();
        }
        assert!(reached, "Last item unreachable; article={article}");
    }
}

#[test]
fn menu_opens_about_and_keyboard_reference() {
    for (label, id) in [
        ("About RustyMusicSheets", "about"),
        ("Keyboard shortcuts", "shortcuts"),
        ("Help contents (F1)", "welcome"),
    ] {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut help = Help::default();
        click(&mut help, &ctx, [1280.0, 900.0], "Help");
        click(&mut help, &ctx, [1280.0, 900.0], label);
        assert!(help.open, "Menu action did not open help: {label}");
        assert_eq!(topics()[help.selected].id, id);
    }
}
