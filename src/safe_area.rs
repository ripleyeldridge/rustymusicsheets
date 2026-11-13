//! Convert platform safe rectangles into egui coordinates without moving pointer input.
use eframe::egui::{pos2, RawInput, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SafeArea {
    /// The native content surface in physical screen coordinates.
    pub surface: Rect,
    /// Unobscured window area in the same physical screen coordinates.
    pub visible: Rect,
}

impl SafeArea {
    pub fn project(self, viewport: Rect) -> Option<Rect> {
        if !self.surface.is_finite()
            || !self.visible.is_finite()
            || !viewport.is_finite()
            || !self.surface.is_positive()
            || !viewport.is_positive()
        {
            return None;
        }
        let visible = self.visible.intersect(self.surface);
        if !visible.is_positive() {
            return None;
        }
        let x = |value| {
            viewport.left()
                + (value - self.surface.left()) * viewport.width() / self.surface.width()
        };
        let y = |value| {
            viewport.top()
                + (value - self.surface.top()) * viewport.height() / self.surface.height()
        };
        Some(
            Rect::from_min_max(
                pos2(x(visible.left()), y(visible.top())),
                pos2(x(visible.right()), y(visible.bottom())),
            )
            .intersect(viewport),
        )
    }
}

#[derive(Default)]
pub struct RawBounds {
    full: Option<Rect>,
}

impl RawBounds {
    pub fn apply(&mut self, input: &mut RawInput, safe: Option<SafeArea>) {
        if let Some(full) = input.screen_rect {
            self.full = Some(full);
        }
        if let Some(full) = self.full {
            input.screen_rect = Some(safe.and_then(|safe| safe.project(full)).unwrap_or(full));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), eframe::egui::vec2(width, height))
    }

    #[test]
    fn status_and_navigation_bars_scale_with_density() {
        let safe = SafeArea {
            surface: rect(0.0, 0.0, 1080.0, 2400.0),
            visible: rect(0.0, 90.0, 1080.0, 2238.0),
        };
        assert_eq!(
            safe.project(rect(0.0, 0.0, 360.0, 800.0)),
            Some(rect(0.0, 30.0, 360.0, 746.0))
        );
    }

    #[test]
    fn already_inset_surface_is_not_padded_twice() {
        let safe = SafeArea {
            surface: rect(0.0, 90.0, 1080.0, 2238.0),
            visible: rect(0.0, 90.0, 1080.0, 2238.0),
        };
        assert_eq!(
            safe.project(rect(0.0, 0.0, 360.0, 746.0)),
            Some(rect(0.0, 0.0, 360.0, 746.0))
        );
    }

    #[test]
    fn landscape_cutout_and_keyboard_use_surface_relative_offsets() {
        let safe = SafeArea {
            surface: rect(200.0, 100.0, 2400.0, 1080.0),
            visible: rect(290.0, 140.0, 2238.0, 700.0),
        };
        let projected = safe.project(rect(0.0, 0.0, 800.0, 360.0)).unwrap();
        let expected = rect(30.0, 40.0 / 3.0, 746.0, 700.0 / 3.0);
        assert!((projected.min - expected.min).length() < 0.001);
        assert!((projected.max - expected.max).length() < 0.001);
    }

    #[test]
    fn repeated_frames_without_new_bounds_never_accumulate_insets() {
        let mut bounds = RawBounds::default();
        let safe = SafeArea {
            surface: rect(0.0, 0.0, 720.0, 1600.0),
            visible: rect(0.0, 60.0, 720.0, 1500.0),
        };
        let mut input = RawInput {
            screen_rect: Some(rect(0.0, 0.0, 360.0, 800.0)),
            ..Default::default()
        };
        bounds.apply(&mut input, Some(safe));
        let expected = input.screen_rect;
        for _ in 0..20 {
            let mut next = RawInput::default();
            bounds.apply(&mut next, Some(safe));
            assert_eq!(next.screen_rect, expected);
        }
        let mut resized = RawInput {
            screen_rect: Some(rect(0.0, 0.0, 720.0, 1600.0)),
            ..Default::default()
        };
        bounds.apply(&mut resized, Some(safe));
        assert_eq!(resized.screen_rect, Some(safe.visible));
    }

    #[test]
    fn unavailable_or_invalid_measurement_does_not_invent_a_viewport() {
        let mut bounds = RawBounds::default();
        let mut input = RawInput::default();
        bounds.apply(&mut input, None);
        assert_eq!(input.screen_rect, None);
        assert_eq!(
            SafeArea {
                surface: Rect::NOTHING,
                visible: Rect::NOTHING
            }
            .project(rect(0.0, 0.0, 360.0, 800.0)),
            None
        );
    }
    #[test]
    fn layout_is_constrained_but_touch_events_keep_native_coordinates() {
        let mut bounds = RawBounds::default();
        let safe = SafeArea {
            surface: rect(0.0, 0.0, 720.0, 1600.0),
            visible: rect(0.0, 80.0, 720.0, 1480.0),
        };
        let pointer = pos2(80.0, 65.0);
        let mut input = RawInput {
            screen_rect: Some(rect(0.0, 0.0, 360.0, 800.0)),
            events: vec![eframe::egui::Event::PointerMoved(pointer)],
            ..Default::default()
        };
        bounds.apply(&mut input, Some(safe));
        assert!(
            matches!(input.events[0], eframe::egui::Event::PointerMoved(pos) if pos == pointer)
        );
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run(input, |ctx| {
            assert_eq!(ctx.screen_rect(), rect(0.0, 40.0, 360.0, 740.0));
            eframe::egui::TopBottomPanel::top("safe_toolbar").show(ctx, |ui| {
                assert!(ui.button("Help").rect.top() >= 40.0);
            });
            eframe::egui::TopBottomPanel::bottom("safe_footer").show(ctx, |ui| {
                assert!(ui.button("Close").rect.bottom() <= 780.0);
            });
        });
    }
}
