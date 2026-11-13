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
    assert_eq!(topics().len(), 27);
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
        "lists",
        "list-order",
        "reading",
        "bookmarks",
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
    assert!(ids("Alto Violoncello").contains(&"instrument-manual"));
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
