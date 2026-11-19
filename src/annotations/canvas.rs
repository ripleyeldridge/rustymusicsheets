use super::{
    controller::{Annotations, Tool},
    model::{stroke_hit, Point, Stroke, MAX_POINTS, MAX_STROKES},
};
use eframe::egui::{self, Color32, Event, LayerId, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase};
const MAX_EVENTS: usize = 4096;

#[derive(Clone, Copy)]
pub struct Surface {
    pub page: u32,
    pub rect: Rect,
    pub clip: Rect,
    pub layer: LayerId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Pointer,
    Touch(TouchDeviceId, TouchId),
}

enum Change {
    Draw(Stroke),
    Erase { strokes: Vec<Stroke>, last: Point },
}

struct Gesture {
    identity: (String, u64),
    sealed: bool,
    surface: Surface,
    owner: Owner,
    change: Change,
}

#[derive(Default)]
pub(super) struct Canvas {
    gesture: Option<Gesture>,
    contacts: Vec<(TouchDeviceId, TouchId)>,
    primary_touch: Option<(TouchDeviceId, TouchId)>,
    suspended: bool,
}

pub(super) fn normalized(rect: Rect, pos: Pos2) -> Option<Point> {
    if !pos.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 || !rect.contains(pos) {
        return None;
    }
    Some(Point {
        x: ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        y: ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
    })
}

pub(super) fn displayed(rect: Rect, point: Point) -> Pos2 {
    egui::pos2(
        rect.left() + point.x * rect.width(),
        rect.top() + point.y * rect.height(),
    )
}

impl Canvas {
    pub(super) fn active(&self) -> bool {
        self.gesture.is_some()
    }

    pub(super) fn discard(&mut self) {
        self.gesture = None;
        self.contacts.clear();
        self.primary_touch = None;
        self.suspended = true;
    }

    pub(super) fn finish(&mut self, annotations: &mut Annotations) {
        let Some(mut gesture) = self.gesture.take() else {
            return;
        };
        gesture.sealed = true;
        if annotations.gesture_identity().as_ref() != Some(&gesture.identity) {
            self.gesture = Some(gesture);
            annotations.notice = "Unfinished ink belongs to a different score session; explicitly discard it before continuing.".into();
            return;
        }
        let strokes = match &gesture.change {
            Change::Draw(stroke) => {
                let Some(ink) = annotations.ink(gesture.surface.page) else {
                    self.gesture = Some(gesture);
                    annotations.notice = "Unable to finish ink: page identity is unavailable. Save/recover or explicitly discard this gesture.".into();
                    return;
                };
                let mut strokes = ink.strokes.clone();
                strokes.push(stroke.clone());
                strokes
            }
            Change::Erase { strokes, .. } => strokes.clone(),
        };
        if let Err(error) = annotations.replace_strokes(gesture.surface.page, strokes) {
            annotations.notice = format!("Unfinished ink retained: {error:#}. Resolve the page error or explicitly discard the gesture.");
            self.gesture = Some(gesture);
        }
    }

    fn begin(
        &mut self,
        annotations: &mut Annotations,
        ctx: &egui::Context,
        surfaces: &[Surface],
        owner: Owner,
        pos: Pos2,
    ) {
        self.finish(annotations);
        if self.gesture.is_some() {
            return;
        }
        let Some(surface) = surfaces
            .iter()
            .find(|surface| {
                surface.clip.contains(pos)
                    && surface.rect.contains(pos)
                    && ctx
                        .layer_id_at(pos)
                        .is_none_or(|layer| layer == surface.layer)
            })
            .copied()
        else {
            return;
        };
        annotations.select_page(surface.page);
        if !annotations.writable(surface.page) {
            annotations.notice = "Page ink is loading, verifying or read-only. Resolve its save/load error before drawing.".into();
            return;
        }
        let Some(point) = normalized(surface.rect, pos) else {
            return;
        };
        let ink = annotations.ink(surface.page).unwrap();
        let change = if annotations.tool == Tool::Eraser {
            let mut strokes = ink.strokes.clone();
            let radius = (annotations.thickness * 2.0).max(8.0) / 1000.0;
            strokes.retain(|stroke| {
                !stroke_hit(
                    stroke,
                    point,
                    point,
                    radius,
                    surface.rect.height() / surface.rect.width(),
                )
            });
            Change::Erase {
                strokes,
                last: point,
            }
        } else {
            if ink.strokes.len() >= MAX_STROKES
                || ink.strokes.iter().map(|s| s.points.len()).sum::<usize>() >= MAX_POINTS
            {
                annotations.notice =
                    "Page ink limit reached. Undo/erase existing marks before adding more.".into();
                return;
            }
            let (tool, color, width) = annotations.stroke_style();
            Change::Draw(Stroke {
                tool,
                color,
                width,
                points: vec![point],
            })
        };
        let Some(identity) = annotations.gesture_identity() else {
            return;
        };
        self.gesture = Some(Gesture {
            identity,
            sealed: false,
            surface,
            owner,
            change,
        });
        annotations.notice.clear();
    }

    fn moved(&mut self, annotations: &mut Annotations, owner: Owner, pos: Pos2) {
        let Some(gesture) = self
            .gesture
            .as_mut()
            .filter(|g| g.owner == owner && !g.sealed)
        else {
            return;
        };
        if !gesture.surface.clip.contains(pos) {
            self.finish(annotations);
            return;
        }
        let Some(point) = normalized(gesture.surface.rect, pos) else {
            self.finish(annotations);
            return;
        };
        match &mut gesture.change {
            Change::Draw(stroke) => {
                let old = stroke.points.last().unwrap();
                let distance = displayed(gesture.surface.rect, *old).distance(pos);
                if distance < 0.2 {
                    return;
                }
                let existing = annotations.ink(gesture.surface.page).map_or(0, |p| {
                    p.strokes.iter().map(|s| s.points.len()).sum::<usize>()
                });
                if existing + stroke.points.len() >= MAX_POINTS {
                    annotations.notice = "Page point limit reached; this gesture ended without deleting earlier ink.".into();
                    self.finish(annotations);
                    return;
                }
                stroke.points.push(point);
            }
            Change::Erase { strokes, last } => {
                let radius = (annotations.thickness * 2.0).max(8.0) / 1000.0;
                strokes.retain(|stroke| {
                    !stroke_hit(
                        stroke,
                        *last,
                        point,
                        radius,
                        gesture.surface.rect.height() / gesture.surface.rect.width(),
                    )
                });
                *last = point;
            }
        }
    }

    pub(super) fn frame(
        &mut self,
        annotations: &mut Annotations,
        ctx: &egui::Context,
        surfaces: &[Surface],
        blocked: bool,
    ) {
        // Only copy fixed-size annotation events, never arbitrary text/paste data.
        let (events, overflow) = ctx.input(|i| {
            let mut events = Vec::new();
            for event in &i.events {
                if matches!(
                    event,
                    Event::Touch { .. }
                        | Event::PointerButton { .. }
                        | Event::PointerMoved(_)
                        | Event::PointerGone
                        | Event::WindowFocused(_)
                ) {
                    if events.len() == MAX_EVENTS {
                        return (events, true);
                    }
                    events.push(event.clone());
                }
            }
            (events, false)
        });
        let was_suspended = self.suspended;
        if was_suspended && ctx.input(|i| !i.pointer.any_down() && !i.any_touches()) {
            self.suspended = false;
        }
        if overflow {
            self.suspended = true;
            annotations.notice = "Too many input events; gesture ended safely. Release all contacts before drawing again.".into();
        }
        let enabled = annotations.captures_page_drag()
            && !blocked
            && !overflow
            && !was_suspended
            && ctx.input(|i| i.focused);
        if !enabled {
            self.finish(annotations);
            self.contacts.clear();
            self.primary_touch = None;
        } else {
            if self.gesture.as_ref().is_some_and(|gesture| {
                !surfaces.iter().any(|s| {
                    s.page == gesture.surface.page
                        && s.rect == gesture.surface.rect
                        && s.clip == gesture.surface.clip
                })
            }) {
                self.finish(annotations);
            }
            let touch_frame = events.iter().any(|e| matches!(e, Event::Touch { .. }))
                || !self.contacts.is_empty();
            for event in events {
                match event {
                    Event::Touch {
                        device_id,
                        id,
                        phase,
                        pos,
                        ..
                    } => {
                        let contact = (device_id, id);
                        match phase {
                            TouchPhase::Start => {
                                if !self.contacts.contains(&contact) && self.contacts.len() == 32 {
                                    self.finish(annotations);
                                    self.contacts.clear();
                                    self.primary_touch = None;
                                    self.suspended = true;
                                    annotations.notice = "Too many contacts; release all contacts before drawing again.".into();
                                    break;
                                }
                                let first = self.contacts.is_empty();
                                if !self.contacts.contains(&contact) && self.contacts.len() < 32 {
                                    self.contacts.push(contact);
                                }
                                if first
                                    && !self
                                        .gesture
                                        .as_ref()
                                        .is_some_and(|g| g.owner == Owner::Pointer)
                                {
                                    self.primary_touch = Some(contact);
                                    self.begin(
                                        annotations,
                                        ctx,
                                        surfaces,
                                        Owner::Touch(device_id, id),
                                        pos,
                                    );
                                }
                            }
                            TouchPhase::Move if self.primary_touch == Some(contact) => {
                                self.moved(annotations, Owner::Touch(device_id, id), pos)
                            }
                            TouchPhase::End | TouchPhase::Cancel => {
                                if self.primary_touch == Some(contact) {
                                    if phase == TouchPhase::End {
                                        self.moved(annotations, Owner::Touch(device_id, id), pos);
                                    }
                                    self.finish(annotations);
                                    self.primary_touch = None;
                                }
                                self.contacts.retain(|c| *c != contact);
                            }
                            _ => {}
                        }
                    }
                    Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        ..
                    } if !touch_frame
                        || self
                            .gesture
                            .as_ref()
                            .is_some_and(|g| g.owner == Owner::Pointer) =>
                    {
                        if pressed {
                            self.begin(annotations, ctx, surfaces, Owner::Pointer, pos);
                        } else {
                            self.moved(annotations, Owner::Pointer, pos);
                            self.finish(annotations);
                        }
                    }
                    Event::PointerMoved(pos)
                        if !touch_frame
                            || self
                                .gesture
                                .as_ref()
                                .is_some_and(|g| g.owner == Owner::Pointer) =>
                    {
                        self.moved(annotations, Owner::Pointer, pos)
                    }
                    Event::PointerGone
                        if !touch_frame
                            || self
                                .gesture
                                .as_ref()
                                .is_some_and(|g| g.owner == Owner::Pointer) =>
                    {
                        self.finish(annotations)
                    }
                    Event::WindowFocused(false) => {
                        self.finish(annotations);
                        self.contacts.clear();
                        self.primary_touch = None;
                    }
                    _ => {}
                }
            }
        }
        if annotations.show {
            for surface in surfaces {
                let painter = ctx
                    .layer_painter(surface.layer)
                    .with_clip_rect(surface.clip.intersect(surface.rect));
                let erased = self
                    .gesture
                    .as_ref()
                    .filter(|g| {
                        g.surface.page == surface.page
                            && annotations.gesture_identity().as_ref() == Some(&g.identity)
                    })
                    .and_then(|g| {
                        if let Change::Erase { strokes, .. } = &g.change {
                            Some(strokes.as_slice())
                        } else {
                            None
                        }
                    });
                if let Some(strokes) =
                    erased.or_else(|| annotations.ink(surface.page).map(|p| p.strokes.as_slice()))
                {
                    for stroke in strokes {
                        paint(&painter, surface.rect, stroke);
                    }
                }
                if let Some(Gesture {
                    change: Change::Draw(stroke),
                    ..
                }) = self.gesture.as_ref().filter(|g| {
                    g.surface.page == surface.page
                        && annotations.gesture_identity().as_ref() == Some(&g.identity)
                }) {
                    paint(&painter, surface.rect, stroke);
                }
            }
        }
    }
}

/// One ribbon with adjoining semicircular caps, not circles painted over a line.
/// Cap interiors and body interiors are disjoint, so translucent endpoints do
/// not darken. As with ordinary paths, genuine self-crossings can overlap.
pub(super) fn stroke_mesh(
    mut points: Vec<Pos2>,
    width: f32,
    color: Color32,
    pixels_per_point: f32,
) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    points.dedup();
    if points.is_empty() {
        return mesh;
    }
    let feather = pixels_per_point.recip();
    if points.len() == 1 {
        let mut dot = egui::epaint::tessellator::Path::default();
        dot.add_circle(points[0], width * 0.5);
        dot.fill(feather, color, &mut mesh);
        return mesh;
    }
    // Match egui's thin-line coverage policy instead of making tiny strokes
    // fully opaque merely because the antialias fringe is wider than the ink.
    let (inner, outer, color) = if width < feather {
        (0.0, feather, color.gamma_multiply(width / feather))
    } else {
        ((width - feather) * 0.5, (width + feather) * 0.5, color)
    };
    let first_direction = (points[1] - points[0]).normalized();
    let last = points.len() - 1;
    let last_direction = (points[last] - points[last - 1]).normalized();
    let mut previous = first_direction.rot90();
    for (index, point) in points.iter().copied().enumerate() {
        let next = points
            .get(index + 1)
            .map_or(previous, |p| (*p - point).normalized().rot90());
        let average = (previous + next).normalized();
        // Bound the miter at sharp bends/reversals; never produce a NaN spike.
        let denominator = average.dot(next);
        let normal = if denominator > 0.5 {
            average / denominator
        } else {
            next
        };
        for (radius, vertex_color) in [
            (outer, Color32::TRANSPARENT),
            (inner, color),
            (-inner, color),
            (-outer, Color32::TRANSPARENT),
        ] {
            mesh.colored_vertex(point + normal * radius, vertex_color);
        }
        if index > 0 {
            let a = ((index - 1) * 4) as u32;
            let b = (index * 4) as u32;
            for side in 0..3 {
                mesh.add_triangle(a + side, a + side + 1, b + side);
                mesh.add_triangle(a + side + 1, b + side, b + side + 1);
            }
        }
        previous = next;
    }
    let steps = ((outer * pixels_per_point * 8.0).sqrt().ceil() as usize).clamp(8, 64);
    for (point, direction) in [
        (points[0], -first_direction),
        (points[last], last_direction),
    ] {
        let normal = direction.rot90();
        let center = mesh.vertices.len() as u32;
        mesh.colored_vertex(point, color);
        for index in 0..=steps {
            let angle = std::f32::consts::PI * index as f32 / steps as f32;
            let radial = match index {
                0 => normal,
                n if n == steps => -normal,
                _ => normal * angle.cos() + direction * angle.sin(),
            };
            mesh.colored_vertex(point + radial * inner, color);
            mesh.colored_vertex(point + radial * outer, Color32::TRANSPARENT);
            if index > 0 {
                let a = center + 1 + (index as u32 - 1) * 2;
                let b = a + 2;
                mesh.add_triangle(center, b, a);
                mesh.add_triangle(a, b, a + 1);
                mesh.add_triangle(a + 1, b, b + 1);
            }
        }
    }
    mesh
}

fn paint(painter: &egui::Painter, rect: Rect, stroke: &Stroke) {
    let points: Vec<_> = stroke.points.iter().map(|p| displayed(rect, *p)).collect();
    let color = Color32::from_rgba_unmultiplied(
        stroke.color[0],
        stroke.color[1],
        stroke.color[2],
        stroke.color[3],
    );
    let width = (stroke.width * rect.width()).max(0.2);
    match points.len() {
        0 => {}
        1 => {
            painter.circle_filled(points[0], width / 2.0, color);
        }
        _ => {
            painter.add(stroke_mesh(
                points,
                width,
                color,
                painter.ctx().pixels_per_point(),
            ));
        }
    }
}
