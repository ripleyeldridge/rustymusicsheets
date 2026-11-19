use super::*;
use crate::annotations::{
    canvas::{displayed, normalized},
    model::Point,
};
use egui::{pos2, vec2, Event, LayerId, Pos2, RawInput, Rect, TouchDeviceId, TouchId, TouchPhase};
use sha2::{Digest, Sha256};

#[test]
fn repaired_source_can_be_explicitly_reverified_without_resetting_page_state() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    h.state_mut(0).dirty = None;
    h.state_mut(0).token = Some("existing raw token".into());
    let source = h.ann.binding.as_ref().unwrap().source.clone();
    std::fs::write(&source, b"generated damaged score").unwrap();
    h.ann.poll(&h.ctx);
    assert!(!h.ann.verified());
    let old_generation = h.ann.generation;
    let revision = h.state(0).revision;
    let history_len = h.state(0).undo.len();
    std::fs::write(&source, &h.original).unwrap();
    h.ann.reverify_source();
    assert_eq!(h.ann.generation, old_generation + 1);
    assert_eq!(h.ann.binding.as_ref().unwrap().generation, h.ann.generation);
    assert!(!h.ann.verified());
    let Some(Job::Verify {
        hash,
        path,
        generation,
    }) = h.ann.next_job()
    else {
        panic!("source hash recheck was not scheduled");
    };
    assert_eq!(path, source);
    assert_eq!(hash, format!("{:x}", Sha256::digest(&h.original)));
    // Completion from the failed session must not enable this new session.
    h.done(Done::Verify {
        hash: hash.clone(),
        generation: old_generation,
        result: Err("stale verification failure".into()),
    });
    assert!(h.ann.binding.as_ref().unwrap().pending);
    h.done(Done::Verify {
        hash,
        generation,
        result: Ok(stamp(&source).unwrap()),
    });
    assert!(h.ann.verified());
    assert!(h.ann.writable(0));
    assert_eq!(h.strokes(0), &[stroke(0.5)]);
    assert_eq!(h.state(0).token.as_deref(), Some("existing raw token"));
    assert_eq!(h.state(0).revision, revision);
    assert_eq!(h.state(0).undo.len(), history_len);
    h.source_unchanged();
}

#[test]
fn source_reverify_is_guarded_during_retained_drafts_and_pending_checks() {
    let mut h = Harness::new(1);
    h.frame(vec![pointer(pos2(30.0, 30.0), true)], &[surface(0)]);
    h.ann.binding.as_mut().unwrap().error = Some("source unavailable".into());
    let generation = h.ann.generation;
    h.ann.reverify_source();
    assert_eq!(h.ann.generation, generation);
    assert!(h.ann.drawing());
    h.ann.discard_unsaved();
    h.ann.binding.as_mut().unwrap().pending = true;
    h.ann.reverify_source();
    assert_eq!(h.ann.generation, generation);
    assert!(h.ann.binding.as_ref().unwrap().error.is_some());
}

#[test]
fn explicit_palette_colors_survive_pen_highlighter_switches() {
    let mut h = Harness::new(1);
    for tool in [Tool::Pen, Tool::Highlighter] {
        h.ann.tool = tool;
        for color in [[200, 25, 25], [20, 60, 200], [15, 15, 15], [255, 215, 0]] {
            h.ann.color = color;
            let (ink_tool, rgba, width) = h.ann.stroke_style();
            assert_eq!(&rgba[..3], &color);
            assert_eq!(rgba[3], if tool == Tool::Highlighter { 70 } else { 255 });
            assert_eq!(
                ink_tool,
                if tool == Tool::Highlighter {
                    InkTool::Highlighter
                } else {
                    InkTool::Pen
                }
            );
            assert!(width.is_finite() && (0.0001..=0.1).contains(&width));
        }
    }
}

// Sample strict triangle interiors, excluding shared edges, to count actual
// coverage layers rather than mistaking shared mesh vertices for alpha overdraw.
fn mesh_coverage(mesh: &egui::epaint::Mesh, p: Pos2) -> Vec<f32> {
    let mut hits = Vec::new();
    let cross = |a: egui::Vec2, b: egui::Vec2| a.x * b.y - a.y * b.x;
    for triangle in mesh.indices.chunks_exact(3) {
        let [a, b, c] =
            [triangle[0], triangle[1], triangle[2]].map(|index| mesh.vertices[index as usize]);
        let determinant = cross(b.pos - a.pos, c.pos - a.pos);
        if determinant.abs() < 0.000001 {
            continue;
        }
        let u = cross(b.pos - p, c.pos - p) / determinant;
        let v = cross(c.pos - p, a.pos - p) / determinant;
        let w = 1.0 - u - v;
        if u > 0.00001 && v > 0.00001 && w > 0.00001 {
            hits.push(
                (u * a.color.a() as f32 + v * b.color.a() as f32 + w * c.color.a() as f32) / 255.0,
            );
        }
    }
    hits
}

#[test]
fn round_caps_extend_both_ends_without_dark_highlighter_overdraw() {
    use crate::annotations::canvas::stroke_mesh;
    for alpha in [70, 255] {
        for angle in [0.0, 0.31, 1.5] {
            for pixels_per_point in [1.0, 2.0, 3.0] {
                let rotation = egui::emath::Rot2::from_angle(angle);
                let rotate = |p: Pos2| (rotation * p.to_vec2()).to_pos2();
                let color = Color32::from_rgba_unmultiplied(200, 25, 25, alpha);
                let mesh = stroke_mesh(
                    vec![rotate(pos2(0.0, 0.0)), rotate(pos2(20.0, 0.0))],
                    10.0,
                    color,
                    pixels_per_point,
                );
                assert!(mesh.vertices.iter().all(|v| v.pos.is_finite()));
                // Both semicaps and the body retain exactly one alpha layer,
                // including immediately on either side of the shared cap seam.
                for point in [
                    pos2(-2.31, 1.13),
                    pos2(10.37, 1.13),
                    pos2(22.31, 1.13),
                    pos2(-0.001, 1.13),
                    pos2(0.001, 1.13),
                ] {
                    let coverage = mesh_coverage(&mesh, rotate(point));
                    assert_eq!(
                        coverage.len(),
                        1,
                        "alpha={alpha}, angle={angle}, point={point:?}"
                    );
                    assert!((coverage[0] - alpha as f32 / 255.0).abs() < 0.0001);
                }
                assert!(mesh_coverage(&mesh, rotate(pos2(-4.9, 4.9))).is_empty());
                assert!(mesh_coverage(&mesh, rotate(pos2(24.9, 4.9))).is_empty());
            }
        }
    }
}

#[test]
fn round_ribbon_handles_dots_duplicates_thin_lines_and_sharp_bends() {
    use crate::annotations::canvas::stroke_mesh;
    let color = Color32::from_rgba_unmultiplied(255, 215, 0, 70);
    for points in [
        vec![],
        vec![pos2(10.0, 10.0); 4],
        vec![pos2(10.0, 10.0), pos2(30.0, 30.0), pos2(10.0, 10.0)],
        vec![
            pos2(10.0, 10.0),
            pos2(10.0, 10.0),
            pos2(30.0, 30.0),
            pos2(31.0, 5.0),
        ],
    ] {
        for width in [0.2, 1.0, 10.0] {
            let mesh = stroke_mesh(points.clone(), width, color, 1.0);
            assert!(mesh
                .vertices
                .iter()
                .all(|v| v.pos.is_finite() && v.color.a() <= 70));
            assert!(mesh
                .indices
                .iter()
                .all(|i| (*i as usize) < mesh.vertices.len()));
            assert!(mesh.vertices.len() <= points.len() * 4 + 262);
        }
    }
}

// Every path in these tests is a newly generated private temporary fixture.
struct Harness {
    ann: Annotations,
    ctx: egui::Context,
    root: tempfile::TempDir,
    jobs: Receiver<Job>,
    results: Sender<Done>,
    original: Vec<u8>,
}

impl Harness {
    fn new(pages: u32) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("generated.pdf");
        let original = b"%PDF-1.4 generated test score only\n".to_vec();
        std::fs::write(&source, &original).unwrap();
        let hash = format!("{:x}", Sha256::digest(&original));
        let ctx = egui::Context::default();
        let (tx, jobs) = bounded(1);
        let (results, rx) = bounded(8);
        let mut ann = Annotations {
            binding: Some(Binding {
                hash,
                pages,
                source: source.clone(),
                stamp: Some(stamp(&source).unwrap()),
                pending: false,
                error: None,
                generation: 1,
            }),
            generation: 1,
            pages: BTreeMap::new(),
            visible: BTreeSet::new(),
            tx,
            rx,
            busy: false,
            worker_failed: false,
            data: root.path().join("data"),
            ctx: ctx.clone(),
            force_save: false,
            editing: true,
            text_input_active: false,
            show: true,
            tool: Tool::Pen,
            color: [1, 2, 3],
            thickness: 2.5,
            canvas: Canvas::default(),
            selected_page: Some(0),
            confirmation: None,
            notice: String::new(),
        };
        for page in 0..pages.min(2) {
            ann.request_page(page);
            let key = ann.key(page).unwrap();
            ann.pages.get_mut(&key).unwrap().ink = Some(PageInk::empty(&key.hash, page));
        }
        Self {
            ann,
            ctx,
            root,
            jobs,
            results,
            original,
        }
    }
    fn key(&self, page: u32) -> PageKey {
        self.ann.key(page).unwrap()
    }
    fn state(&self, page: u32) -> &PageState {
        &self.ann.pages[&self.key(page)]
    }
    fn state_mut(&mut self, page: u32) -> &mut PageState {
        let key = self.key(page);
        self.ann.pages.get_mut(&key).unwrap()
    }
    fn strokes(&self, page: u32) -> &[Stroke] {
        &self.state(page).ink.as_ref().unwrap().strokes
    }
    fn source_unchanged(&self) {
        assert_eq!(
            std::fs::read(&self.ann.binding.as_ref().unwrap().source).unwrap(),
            self.original
        );
    }
    fn frame(&mut self, events: Vec<Event>, surfaces: &[Surface]) {
        self.frame_options(events, surfaces, true, false);
    }
    fn frame_options(
        &mut self,
        events: Vec<Event>,
        surfaces: &[Surface],
        focused: bool,
        blocked: bool,
    ) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 1000.0))),
            events,
            focused,
            ..Default::default()
        };
        let _ = self
            .ctx
            .run(input, |ctx| self.ann.canvas(ctx, surfaces, blocked));
    }
    fn done(&mut self, done: Done) {
        self.results.send(done).unwrap();
        self.ann.poll(&self.ctx);
    }
}

fn surface(page: u32) -> Surface {
    let rect = Rect::from_min_size(pos2(20.0 + page as f32 * 220.0, 20.0), vec2(200.0, 400.0));
    Surface {
        page,
        rect,
        clip: rect,
        layer: LayerId::background(),
    }
}
fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}
fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch {
        device_id: TouchDeviceId(1),
        id: TouchId(id),
        phase,
        pos,
        force: Some(0.5),
    }
}
fn stroke(x: f32) -> Stroke {
    Stroke {
        tool: InkTool::Pen,
        color: [10, 20, 30, 255],
        width: 0.005,
        points: vec![Point { x, y: 0.5 }],
    }
}
fn drive_until(ann: &mut Annotations, ctx: &egui::Context, ready: impl Fn(&Annotations) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        ann.poll(ctx);
        if ready(ann) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "worker timed out: {}",
            ann.notice
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn ordered_one_frame_samples_release_and_dot() {
    let mut h = Harness::new(1);
    let s = surface(0);
    h.frame(
        vec![
            pointer(pos2(30.0, 30.0), true),
            Event::PointerMoved(pos2(50.0, 50.0)),
            Event::PointerMoved(pos2(80.0, 90.0)),
            pointer(pos2(110.0, 130.0), false),
        ],
        &[s],
    );
    assert!(!h.ann.drawing());
    assert_eq!(h.strokes(0)[0].points.len(), 4);
    assert_eq!(h.strokes(0)[0].points[0], Point { x: 0.05, y: 0.025 });
    assert_eq!(h.strokes(0)[0].points[3], Point { x: 0.45, y: 0.275 });
    h.frame(
        vec![
            pointer(pos2(70.0, 80.0), true),
            pointer(pos2(70.0, 80.0), false),
        ],
        &[s],
    );
    assert_eq!(h.strokes(0).len(), 2);
    assert_eq!(h.strokes(0)[1].points.len(), 1);
    assert!(h.ann.has_unsaved());
    h.source_unchanged();
}

#[test]
fn touch_mouse_duplicates_and_secondary_contacts_are_not_extra_strokes() {
    let mut h = Harness::new(1);
    let s = surface(0);
    h.frame(
        vec![
            pointer(pos2(30.0, 30.0), true),
            touch(1, TouchPhase::Start, pos2(30.0, 30.0)),
            touch(2, TouchPhase::Start, pos2(200.0, 300.0)),
            touch(2, TouchPhase::Move, pos2(190.0, 290.0)),
            touch(1, TouchPhase::Move, pos2(60.0, 60.0)),
            Event::PointerMoved(pos2(60.0, 60.0)),
            touch(1, TouchPhase::End, pos2(80.0, 80.0)),
            pointer(pos2(80.0, 80.0), false),
            touch(2, TouchPhase::Move, pos2(180.0, 280.0)),
            touch(3, TouchPhase::Start, pos2(150.0, 200.0)),
            touch(2, TouchPhase::End, pos2(180.0, 280.0)),
            touch(3, TouchPhase::End, pos2(150.0, 200.0)),
        ],
        &[s],
    );
    assert_eq!(h.strokes(0).len(), 1);
    assert_eq!(h.strokes(0)[0].points.len(), 3);
    assert!(!h.ann.drawing());
}

#[test]
fn primary_pointer_survives_secondary_buttons_and_extra_touch() {
    let mut h = Harness::new(1);
    let s = surface(0);
    h.frame(vec![pointer(pos2(30.0, 30.0), true)], &[s]);
    h.frame(
        vec![
            touch(1, TouchPhase::Start, pos2(190.0, 300.0)),
            Event::PointerButton {
                pos: pos2(190.0, 300.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::PointerMoved(pos2(50.0, 50.0)),
            pointer(pos2(70.0, 70.0), false),
            touch(1, TouchPhase::End, pos2(190.0, 300.0)),
        ],
        &[s],
    );
    assert_eq!(h.strokes(0).len(), 1);
    assert_eq!(h.strokes(0)[0].points.len(), 3);
}

#[test]
fn capture_is_owned_by_start_page_and_clipped() {
    let mut h = Harness::new(2);
    let a = surface(0);
    let b = surface(1);
    h.frame(
        vec![
            pointer(pos2(225.0, 30.0), true),
            pointer(pos2(225.0, 30.0), false),
        ],
        &[a, b],
    );
    assert!(h.strokes(0).is_empty() && h.strokes(1).is_empty());
    h.frame(
        vec![
            pointer(pos2(30.0, 30.0), true),
            Event::PointerMoved(pos2(260.0, 50.0)),
            pointer(pos2(280.0, 70.0), false),
        ],
        &[a, b],
    );
    assert_eq!(h.strokes(0).len(), 1);
    assert!(h.strokes(1).is_empty());
    let clipped = Surface {
        clip: Rect::from_min_max(pos2(20.0, 100.0), pos2(220.0, 420.0)),
        ..a
    };
    h.frame(
        vec![
            pointer(pos2(30.0, 30.0), true),
            pointer(pos2(30.0, 30.0), false),
        ],
        &[clipped],
    );
    assert_eq!(h.strokes(0).len(), 1);
}

#[test]
fn resize_focus_pointer_loss_and_blockers_end_gestures() {
    for ending in 0..5 {
        let mut h = Harness::new(1);
        let s = surface(0);
        h.frame(vec![pointer(pos2(30.0, 30.0), true)], &[s]);
        assert!(h.ann.drawing());
        match ending {
            0 => h.frame(
                vec![Event::PointerMoved(pos2(60.0, 60.0))],
                &[Surface {
                    rect: s.rect.translate(vec2(10.0, 0.0)),
                    ..s
                }],
            ),
            1 => h.frame_options(vec![Event::WindowFocused(false)], &[s], false, false),
            2 => h.frame(vec![Event::PointerGone], &[s]),
            3 => h.frame_options(vec![], &[s], true, true),
            _ => {
                h.ann.tool = Tool::Hand;
                h.frame(vec![], &[s]);
            }
        }
        assert!(!h.ann.drawing());
        assert_eq!(h.strokes(0).len(), 1);
        assert_eq!(h.strokes(0)[0].points.len(), 1);
    }
}

#[test]
fn physical_coordinate_roundtrips_at_aspects_scales_and_offsets() {
    for aspect in [0.5, 1.0, 1.414, 2.0] {
        for scale in [0.5, 1.0, 2.0, 3.0] {
            let rect = Rect::from_min_size(
                pos2(-125.0, 333.0),
                vec2(200.0 * scale, 200.0 * aspect * scale),
            );
            for point in [
                Point { x: 0.0, y: 0.0 },
                Point { x: 0.27, y: 0.63 },
                Point { x: 1.0, y: 1.0 },
            ] {
                let roundtrip = normalized(rect, displayed(rect, point)).unwrap();
                assert!((roundtrip.x - point.x).abs() < 0.00001);
                assert!((roundtrip.y - point.y).abs() < 0.00001);
            }
        }
    }
}

#[test]
fn read_only_rejection_never_accepts_input_or_autosaves_defaults() {
    let mut h = Harness::new(1);
    h.state_mut(0).ink = None;
    h.state_mut(0).pending = true;
    let key = h.key(0);
    h.done(Done::Load {
        key: key.clone(),
        result: Ok(store::Loaded {
            ink: PageInk::empty(&key.hash, 0),
            token: Some("raw".into()),
            error: Some("invalid annotation JSON".into()),
        }),
    });
    let s = surface(0);
    h.frame(
        vec![
            pointer(pos2(30.0, 30.0), true),
            pointer(pos2(60.0, 60.0), false),
        ],
        &[s],
    );
    assert!(h.strokes(0).is_empty());
    assert!(!h.ann.writable(0));
    assert!(!h.ann.has_unsaved());
    h.ann.save_now();
    assert!(h.ann.next_job().is_none());
    h.source_unchanged();
}

#[test]
fn retained_draft_is_sealed_blocks_switch_and_can_be_explicitly_discarded() {
    let mut h = Harness::new(1);
    let s = surface(0);
    h.frame(vec![pointer(pos2(30.0, 30.0), true)], &[s]);
    h.state_mut(0).save_error = Some("generated conflict".into());
    h.frame(vec![pointer(pos2(60.0, 60.0), false)], &[s]);
    assert!(h.ann.drawing());
    let old_hash = h.key(0).hash;
    let source = h.ann.binding.as_ref().unwrap().source.clone();
    h.ann.bind(&"b".repeat(64), 1, &source);
    assert_eq!(h.key(0).hash, old_hash);
    h.ann.unbind();
    assert!(h.ann.binding.is_some());
    h.frame(vec![Event::PointerMoved(pos2(200.0, 300.0))], &[s]);
    h.state_mut(0).save_error = None;
    h.ann.finish_gesture();
    assert_eq!(h.strokes(0)[0].points.len(), 2);
    assert!(!h.ann.drawing());
    h.ann.discard_unsaved();
    assert!(!h.ann.has_unsaved());
    assert!(!h.ann.drawing());
    assert!(!h.ann.editing());
    h.source_unchanged();
}

#[test]
fn forced_identity_change_never_commits_draft_to_new_score() {
    let mut h = Harness::new(1);
    h.frame(vec![pointer(pos2(30.0, 30.0), true)], &[surface(0)]);
    let old_key = h.key(0);
    let binding = h.ann.binding.as_mut().unwrap();
    binding.hash = "b".repeat(64);
    binding.generation += 1;
    h.ann.generation += 1;
    let new_key = h.key(0);
    let mut state = PageState::new(1);
    state.ink = Some(PageInk::empty(&new_key.hash, 0));
    h.ann.pages.insert(new_key, state);
    h.ann.finish_gesture();
    assert!(h.ann.drawing());
    assert!(h.strokes(0).is_empty());
    assert!(h.ann.pages[&old_key]
        .ink
        .as_ref()
        .unwrap()
        .strokes
        .is_empty());
    h.ann.discard_unsaved();
    assert!(!h.ann.drawing());
}

#[test]
fn discard_is_refused_while_writes_pending_but_not_for_loads() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    h.state_mut(0).pending = true;
    h.state_mut(0).writing = true;
    assert!(h.ann.writes_pending());
    h.ann.discard_unsaved();
    assert!(h.ann.has_unsaved());
    h.state_mut(0).writing = false;
    assert!(!h.ann.writes_pending());
    h.ann.discard_unsaved();
    assert!(!h.ann.has_unsaved());
}

#[test]
fn dirty_debounce_schedules_a_wakeup_and_force_save_bypasses_it() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    let delay = h.ann.autosave_delay().unwrap();
    assert!(delay > Duration::ZERO && delay <= SAVE_DEBOUNCE);
    assert!(h.ann.next_job().is_none());
    h.ann.save_now();
    assert!(matches!(h.ann.next_job(), Some(Job::Save { .. })));
    assert!(h.ann.writes_pending());
    assert!(h.ann.autosave_delay().is_none());
}

#[test]
fn newer_revision_stays_dirty_after_older_save_and_uses_acknowledged_token() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.2)]).unwrap();
    h.ann.save_now();
    let Some(Job::Save { revision, .. }) = h.ann.next_job() else {
        panic!("save missing")
    };
    h.ann
        .replace_strokes(0, vec![stroke(0.2), stroke(0.7)])
        .unwrap();
    h.ann.force_save = false;
    h.done(Done::Save {
        key: h.key(0),
        revision,
        result: Ok(store::Saved {
            token: "acknowledged".into(),
            durability_warning: None,
        }),
    });
    assert!(h.state(0).dirty.is_some());
    assert_eq!(h.strokes(0).len(), 2);
    assert!(!h.ann.writes_pending());
    h.ann.save_now();
    let Some(Job::Save {
        expected,
        revision: next,
        ..
    }) = h.ann.next_job()
    else {
        panic!("second save missing")
    };
    assert_eq!(expected.as_deref(), Some("acknowledged"));
    assert!(next > revision);
}

#[test]
fn old_page_save_errors_remain_global_and_retry_preserves_ink() {
    let mut h = Harness::new(2);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    h.ann.selected_page = Some(1);
    h.done(Done::Save {
        key: h.key(0),
        revision: 1,
        result: Err("generated write failure".into()),
    });
    assert!(h.ann.has_save_error());
    assert!(h.ann.has_unsaved());
    assert!(!h.ann.writable(0));
    h.ann.save_now();
    assert!(!h.ann.has_save_error());
    assert_eq!(h.strokes(0).len(), 1);
    assert!(matches!(h.ann.next_job(),Some(Job::Save{key,..}) if key.page==0));
}

#[test]
fn full_channel_releases_unsent_flags_and_can_retry() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    h.ann.force_save = true;
    h.ann
        .tx
        .try_send(Job::Load {
            key: h.key(0),
            pages: 1,
        })
        .unwrap();
    let job = h.ann.next_job().unwrap();
    h.ann.dispatch(job);
    assert!(!h.state(0).pending);
    assert!(!h.ann.writes_pending());
    assert!(h.ann.has_unsaved());
    h.jobs.try_recv().unwrap();
    h.ann.poll(&h.ctx);
    assert!(h.ann.writes_pending());
    assert!(matches!(h.jobs.try_recv(), Ok(Job::Save { .. })));
}

#[test]
fn disconnected_worker_releases_flags_and_explicit_retry_restarts() {
    let mut h = Harness::new(1);
    h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    h.state_mut(0).pending = true;
    h.state_mut(0).writing = true;
    let (_, dummy) = bounded(1);
    let old = std::mem::replace(&mut h.jobs, dummy);
    drop(old);
    drop(h.results.clone());
    let (dead, rx) = bounded(1);
    drop(dead);
    h.ann.rx = rx;
    h.ann.poll(&h.ctx);
    assert!(h.ann.worker_failed);
    assert!(!h.ann.writes_pending());
    assert!(h.ann.has_save_error());
    assert!(h.ann.has_unsaved());
    h.ann.save_now();
    drive_until(&mut h.ann, &h.ctx, |a| !a.has_unsaved());
    assert!(!h.ann.worker_failed);
    assert!(!h.ann.has_save_error());
    h.source_unchanged();
}

#[test]
fn stale_source_completion_and_source_changes_do_not_enable_editing() {
    let mut h = Harness::new(1);
    let old = h.ann.binding.as_ref().unwrap().stamp.clone().unwrap();
    h.done(Done::Verify {
        hash: h.key(0).hash,
        generation: 0,
        result: Err("stale".into()),
    });
    assert!(h.ann.verified());
    h.ann.binding.as_mut().unwrap().stamp = None;
    h.ann.binding.as_mut().unwrap().pending = true;
    let source = h.ann.binding.as_ref().unwrap().source.clone();
    std::fs::write(&source, b"changed generated fixture with different length").unwrap();
    h.done(Done::Verify {
        hash: h.key(0).hash,
        generation: 1,
        result: Ok(old),
    });
    assert!(!h.ann.verified());
    assert!(!h.ann.editing());
    assert!(!h.ann.writable(0));
}

#[test]
fn confirmations_bind_page_hash_revision_token_and_generation() {
    for action in [PageAction::Clear, PageAction::Reload, PageAction::Recover] {
        for change in 0..4 {
            let mut h = Harness::new(2);
            h.ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
            h.ann.request_action(action);
            assert!(h.ann.confirmation.is_some());
            match change {
                0 => h.ann.selected_page = Some(1),
                1 => h.state_mut(0).revision += 1,
                2 => h.state_mut(0).token = Some("new token".into()),
                _ => h.ann.generation += 1,
            }
            h.ann.confirm_action();
            assert_eq!(h.strokes(0).len(), 1);
            assert!(!h.state(0).recovery);
            assert!(h.ann.notice.contains("changed"));
        }
    }
}

#[test]
fn confirmed_clear_is_undoable_and_history_is_bounded() {
    let mut h = Harness::new(1);
    for index in 0..HISTORY_STEPS + 10 {
        h.ann
            .replace_strokes(0, vec![stroke(index as f32 / 200.0)])
            .unwrap();
    }
    assert!(h.state(0).undo.len() <= HISTORY_STEPS);
    h.ann.request_action(PageAction::Clear);
    h.ann.confirm_action();
    assert!(h.strokes(0).is_empty());
    h.ann.history(false);
    assert_eq!(h.strokes(0).len(), 1);
    h.ann.history(true);
    assert!(h.strokes(0).is_empty());
    for count in 1..9 {
        let mut mark = stroke(0.5);
        mark.points = vec![Point { x: 0.5, y: 0.5 }; count * 20_000];
        h.ann.replace_strokes(0, vec![mark]).unwrap();
    }
    assert!(
        h.state(0)
            .undo
            .iter()
            .chain(&h.state(0).redo)
            .map(|s| stroke_bytes(s))
            .sum::<usize>()
            <= HISTORY_BYTES
    );
    h.source_unchanged();
}

#[test]
fn bounded_cache_never_evicts_unsaved_or_pending_pages() {
    let mut h = Harness::new(20);
    for page in 0..CACHE_PAGES as u32 {
        let key = h.ann.key(page).unwrap();
        let mut state = PageState::new(20);
        state.ink = Some(PageInk::empty(&key.hash, page));
        state.dirty = Some(Instant::now());
        h.ann.pages.insert(key, state);
    }
    h.ann.begin_view();
    h.ann.request_page(8);
    assert_eq!(h.ann.pages.len(), CACHE_PAGES);
    assert!(!h.ann.pages.contains_key(&h.key(8)));
    assert!(h.ann.notice.contains("cache full"));
    h.state_mut(0).dirty = None;
    h.state_mut(0).pending = true;
    h.ann.request_page(8);
    assert!(h.ann.pages.contains_key(&h.key(0)));
    h.state_mut(0).pending = false;
    h.ann.request_page(8);
    assert!(!h.ann.pages.contains_key(&h.key(0)));
    assert!(h.ann.pages.contains_key(&h.key(8)));
}

#[test]
fn event_and_contact_overflow_end_capture_and_ignore_text_payloads() {
    let mut h = Harness::new(1);
    let s = surface(0);
    h.frame(
        vec![
            Event::Text("x".repeat(1024 * 1024)),
            pointer(pos2(30.0, 30.0), true),
        ],
        &[s],
    );
    assert!(h.ann.drawing());
    h.frame(vec![Event::PointerMoved(pos2(50.0, 50.0)); 5000], &[s]);
    assert!(!h.ann.drawing());
    assert_eq!(h.strokes(0).len(), 1);
    assert!(h.ann.notice.contains("Too many input events"));
    h.frame(vec![pointer(pos2(70.0, 70.0), false)], &[s]);
    h.frame(
        vec![
            pointer(pos2(80.0, 80.0), true),
            pointer(pos2(80.0, 80.0), false),
        ],
        &[s],
    );
    assert_eq!(h.strokes(0).len(), 2);
    let events = (0..40)
        .map(|id| touch(id, TouchPhase::Start, pos2(40.0, 40.0)))
        .collect();
    h.frame(events, &[s]);
    assert!(h.ann.notice.contains("Too many contacts"));
    assert!(!h.ann.drawing());
}

#[test]
fn actual_worker_detects_conflicts_retry_reload_and_preserves_source() {
    let h = Harness::new(1);
    let source = h.ann.binding.as_ref().unwrap().source.clone();
    let hash = h.key(0).hash;
    let data = h.root.path().join("real-worker-data");
    let mut ann = Annotations::new(data.clone(), h.ctx.clone());
    ann.bind(&hash, 1, &source);
    ann.request_page(0);
    drive_until(&mut ann, &h.ctx, |a| a.writable(0));
    ann.replace_strokes(0, vec![stroke(0.5)]).unwrap();
    let external = store::save(&data, &PageInk::empty(&hash, 0), 1, None).unwrap();
    ann.save_now();
    drive_until(&mut ann, &h.ctx, Annotations::has_save_error);
    assert!(ann.has_unsaved());
    let disk = store::load(&data, &hash, 0, 1).unwrap();
    assert_eq!(disk.token.as_deref(), Some(external.token.as_str()));
    assert!(disk.ink.strokes.is_empty());
    ann.save_now();
    drive_until(&mut ann, &h.ctx, Annotations::has_save_error);
    ann.selected_page = Some(0);
    ann.request_action(PageAction::Reload);
    ann.confirm_action();
    drive_until(&mut ann, &h.ctx, |a| a.writable(0));
    assert!(!ann.has_unsaved());
    assert_eq!(std::fs::read(&source).unwrap(), h.original);
}

#[test]
fn captures_drag_and_binding_predicates_are_exact() {
    let mut h = Harness::new(2);
    let b = h.ann.binding.as_ref().unwrap();
    assert!(h.ann.matches_score(&b.hash, 2, &b.source));
    assert!(!h.ann.matches_score(&b.hash, 1, &b.source));
    assert!(!h.ann.matches_score(&"b".repeat(64), 2, &b.source));
    assert!(!h.ann.matches_score(&b.hash, 2, Path::new("other.pdf")));
    assert!(h.ann.captures_page_drag());
    h.ann.tool = Tool::Hand;
    assert!(!h.ann.captures_page_drag());
    h.ann.tool = Tool::Pen;
    h.ann.show = false;
    assert!(!h.ann.captures_page_drag());
    h.ann.show = true;
    h.ann.editing = false;
    assert!(!h.ann.captures_page_drag());
}
