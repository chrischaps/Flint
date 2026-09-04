//! Curve and gradient editors: a small egui painter widget over
//! `(t, value)` keys in `t ∈ [0, 1]`.
//!
//! Interaction:
//! - drag a key to move it (t clamped between its neighbours, value to the range)
//! - double-click empty space to add a key
//! - right-click a key to remove it (never below two keys)
//! - hold Shift while dragging to lock t
//!
//! Pure egui; no engine dependencies, so the particle editor, the scene
//! inspector and game debug panels can all share it (ADR 0068).

use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// What happened this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurveResponse {
    /// Keys changed (dragging counts every frame).
    pub changed: bool,
    /// A drag ended or a key was added/removed — a good moment to push undo.
    pub committed: bool,
}

/// Curve sampling callback: `(keys, t) -> value`.
pub type Sampler = dyn Fn(&[[f32; 2]], f32) -> f32;

const KEY_RADIUS: f32 = 5.0;
const HIT_RADIUS: f32 = 9.0;

/// Editor for a scalar curve of `[t, v]` keys.
pub struct CurveEditor<'a> {
    keys: &'a mut Vec<[f32; 2]>,
    range: std::ops::RangeInclusive<f32>,
    height: f32,
    id: egui::Id,
    /// Sample the curve as the user will see it (smoothstep, step...).
    sampler: Option<&'a Sampler>,
    accent: Color32,
}

impl<'a> CurveEditor<'a> {
    pub fn new(id_salt: impl std::hash::Hash, keys: &'a mut Vec<[f32; 2]>) -> Self {
        Self {
            keys,
            range: 0.0..=1.0,
            height: 90.0,
            id: egui::Id::new(id_salt),
            sampler: None,
            accent: Color32::from_rgb(255, 176, 88),
        }
    }

    pub fn range(mut self, range: std::ops::RangeInclusive<f32>) -> Self {
        self.range = range;
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Use the engine's interpolation for the drawn polyline; default is
    /// linear between keys.
    pub fn sampler(mut self, f: &'a Sampler) -> Self {
        self.sampler = Some(f);
        self
    }

    pub fn accent(mut self, c: Color32) -> Self {
        self.accent = c;
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> CurveResponse {
        let CurveEditor {
            keys,
            range,
            height,
            id,
            sampler,
            accent,
        } = self;
        let mut resp = CurveResponse::default();
        if keys.is_empty() {
            keys.push([0.0, *range.start()]);
            keys.push([1.0, *range.end()]);
            resp.changed = true;
        }
        keys.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));

        let width = ui.available_width().max(120.0);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let inner = rect.shrink2(Vec2::new(KEY_RADIUS + 2.0, KEY_RADIUS + 2.0));
        let (lo, hi) = (*range.start(), *range.end());
        let span = (hi - lo).max(1e-6);

        let to_screen = |t: f32, v: f32| -> Pos2 {
            Pos2::new(
                inner.left() + t.clamp(0.0, 1.0) * inner.width(),
                inner.bottom() - ((v - lo) / span).clamp(0.0, 1.0) * inner.height(),
            )
        };
        let from_screen = |p: Pos2| -> [f32; 2] {
            [
                ((p.x - inner.left()) / inner.width()).clamp(0.0, 1.0),
                (lo + (1.0 - (p.y - inner.top()) / inner.height()) * span)
                    .clamp(lo.min(hi), hi.max(lo)),
            ]
        };

        // Background + grid
        let visuals = ui.visuals();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        let grid = visuals.weak_text_color().linear_multiply(0.25);
        for i in 0..=4 {
            let f = i as f32 / 4.0;
            let x = inner.left() + f * inner.width();
            let y = inner.top() + f * inner.height();
            painter.line_segment(
                [Pos2::new(x, inner.top()), Pos2::new(x, inner.bottom())],
                Stroke::new(1.0, grid),
            );
            painter.line_segment(
                [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
                Stroke::new(1.0, grid),
            );
        }

        // Sampled polyline
        let samples = 64;
        let mut pts = Vec::with_capacity(samples + 1);
        for i in 0..=samples {
            let t = i as f32 / samples as f32;
            let v = match sampler {
                Some(f) => f(keys, t),
                None => linear_sample(keys, t),
            };
            pts.push(to_screen(t, v));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(2.0, accent)));

        // Keys, with interaction
        let pointer = response.hover_pos();
        let mut hovered: Option<usize> = None;
        if let Some(p) = pointer {
            let mut best = HIT_RADIUS * HIT_RADIUS;
            for (i, k) in keys.iter().enumerate() {
                let d = to_screen(k[0], k[1]).distance_sq(p);
                if d < best {
                    best = d;
                    hovered = Some(i);
                }
            }
        }

        // Drag state persisted across frames.
        let drag_key: Option<usize> = ui.data(|d| d.get_temp(id));
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let shift = ui.input(|i| i.modifiers.shift);

        let mut active = drag_key;
        if active.is_none() && response.drag_started() {
            if let Some(h) = hovered {
                active = Some(h);
                ui.data_mut(|d| d.insert_temp(id, h));
            }
        }
        if let Some(i) = active {
            if primary_down {
                if let Some(p) = ui.input(|inp| inp.pointer.interact_pos()) {
                    let [mut t, v] = from_screen(p);
                    if shift {
                        t = keys[i][0];
                    }
                    // Keep order: clamp between neighbours.
                    let lo_t = if i > 0 { keys[i - 1][0] + 1e-4 } else { 0.0 };
                    let hi_t = if i + 1 < keys.len() {
                        keys[i + 1][0] - 1e-4
                    } else {
                        1.0
                    };
                    let t = t.clamp(lo_t.min(hi_t), hi_t.max(lo_t));
                    if keys[i] != [t, v] {
                        keys[i] = [t, v];
                        resp.changed = true;
                    }
                }
            } else {
                ui.data_mut(|d| d.remove::<usize>(id));
                resp.committed = true;
            }
        }

        // Add / remove
        if response.double_clicked() && drag_key.is_none() {
            if let Some(p) = response.interact_pointer_pos() {
                if hovered.is_none() {
                    keys.push(from_screen(p));
                    keys.sort_by(|a, b| {
                        a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    resp.changed = true;
                    resp.committed = true;
                }
            }
        }
        if response.secondary_clicked() {
            if let Some(h) = hovered {
                if keys.len() > 2 {
                    keys.remove(h);
                    resp.changed = true;
                    resp.committed = true;
                }
            }
        }

        for (i, k) in keys.iter().enumerate() {
            let p = to_screen(k[0], k[1]);
            let is_active = active == Some(i) || hovered == Some(i);
            let fill = if is_active { Color32::WHITE } else { accent };
            painter.circle_filled(p, KEY_RADIUS, fill);
            painter.circle_stroke(p, KEY_RADIUS, Stroke::new(1.0, visuals.extreme_bg_color));
        }

        if let Some(h) = hovered {
            let k = keys[h];
            response
                .clone()
                .on_hover_text(format!("t = {:.2}   v = {:.3}", k[0], k[1]));
        } else if response.hovered() {
            response
                .clone()
                .on_hover_text("double-click: add key · right-click: remove · shift-drag: lock t");
        }

        resp
    }
}

/// Linear sample of `[t, v]` keys (sorted by t).
pub fn linear_sample(keys: &[[f32; 2]], t: f32) -> f32 {
    if keys.is_empty() {
        return 0.0;
    }
    if t <= keys[0][0] {
        return keys[0][1];
    }
    let last = keys[keys.len() - 1];
    if t >= last[0] {
        return last[1];
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a[0] && t <= b[0] {
            let span = (b[0] - a[0]).max(1e-6);
            let u = (t - a[0]) / span;
            return a[1] + (b[1] - a[1]) * u;
        }
    }
    last[1]
}

/// Editor for an RGBA gradient of `(t, rgba)` keys.
pub struct GradientEditor<'a> {
    keys: &'a mut Vec<(f32, [f32; 4])>,
    id: egui::Id,
    height: f32,
}

impl<'a> GradientEditor<'a> {
    pub fn new(id_salt: impl std::hash::Hash, keys: &'a mut Vec<(f32, [f32; 4])>) -> Self {
        Self {
            keys,
            id: egui::Id::new(id_salt),
            height: 26.0,
        }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> CurveResponse {
        let GradientEditor { keys, id, height } = self;
        let mut resp = CurveResponse::default();
        if keys.is_empty() {
            keys.push((0.0, [1.0, 1.0, 1.0, 1.0]));
            keys.push((1.0, [1.0, 1.0, 1.0, 0.0]));
            resp.changed = true;
        }
        keys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let width = ui.available_width().max(120.0);
        let handle_h = 14.0;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width, height + handle_h),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), height));
        let margin = 6.0;
        let x_of = |t: f32| bar.left() + margin + t.clamp(0.0, 1.0) * (bar.width() - 2.0 * margin);
        let t_of =
            |x: f32| ((x - bar.left() - margin) / (bar.width() - 2.0 * margin)).clamp(0.0, 1.0);

        // Alpha checker under the bar
        let cell = 6.0;
        let mut x = bar.left();
        let mut row = 0;
        while x < bar.right() {
            let mut y = bar.top();
            let mut col = row;
            while y < bar.bottom() {
                let c = if col % 2 == 0 {
                    Color32::from_gray(70)
                } else {
                    Color32::from_gray(110)
                };
                let r = Rect::from_min_max(
                    Pos2::new(x, y),
                    Pos2::new((x + cell).min(bar.right()), (y + cell).min(bar.bottom())),
                );
                painter.rect_filled(r, 0.0, c);
                y += cell;
                col += 1;
            }
            x += cell;
            row += 1;
        }

        // Sampled gradient strips
        let steps = 96;
        for i in 0..steps {
            let t0 = i as f32 / steps as f32;
            let t1 = (i + 1) as f32 / steps as f32;
            let c = sample_gradient(keys, (t0 + t1) * 0.5);
            let col = Color32::from_rgba_unmultiplied(
                (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                (c[3].clamp(0.0, 1.0) * 255.0) as u8,
            );
            let r = Rect::from_min_max(
                Pos2::new(bar.left() + t0 * bar.width(), bar.top()),
                Pos2::new(bar.left() + t1 * bar.width(), bar.bottom()),
            );
            painter.rect_filled(r, 0.0, col);
        }
        painter.rect_stroke(bar, 3.0, Stroke::new(1.0, ui.visuals().weak_text_color()));

        // Handles
        let pointer = response.hover_pos();
        let mut hovered: Option<usize> = None;
        if let Some(p) = pointer {
            let mut best = HIT_RADIUS * HIT_RADIUS * 1.5;
            for (i, k) in keys.iter().enumerate() {
                let hp = Pos2::new(x_of(k.0), bar.bottom() + handle_h * 0.5);
                let d = hp.distance_sq(p);
                if d < best {
                    best = d;
                    hovered = Some(i);
                }
            }
        }
        let drag_key: Option<usize> = ui.data(|d| d.get_temp(id));
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let mut active = drag_key;
        if active.is_none() && response.drag_started() {
            if let Some(h) = hovered {
                active = Some(h);
                ui.data_mut(|d| d.insert_temp(id, h));
            }
        }
        if let Some(i) = active {
            if primary_down {
                if let Some(p) = ui.input(|inp| inp.pointer.interact_pos()) {
                    let lo_t = if i > 0 { keys[i - 1].0 + 1e-4 } else { 0.0 };
                    let hi_t = if i + 1 < keys.len() {
                        keys[i + 1].0 - 1e-4
                    } else {
                        1.0
                    };
                    let t = t_of(p.x).clamp(lo_t.min(hi_t), hi_t.max(lo_t));
                    if keys[i].0 != t {
                        keys[i].0 = t;
                        resp.changed = true;
                    }
                }
            } else {
                ui.data_mut(|d| d.remove::<usize>(id));
                resp.committed = true;
            }
        }
        if response.double_clicked() && drag_key.is_none() && hovered.is_none() {
            if let Some(p) = response.interact_pointer_pos() {
                let t = t_of(p.x);
                let c = sample_gradient(keys, t);
                keys.push((t, c));
                keys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                resp.changed = true;
                resp.committed = true;
            }
        }
        if response.secondary_clicked() {
            if let Some(h) = hovered {
                if keys.len() > 2 {
                    keys.remove(h);
                    resp.changed = true;
                    resp.committed = true;
                }
            }
        }
        for (i, k) in keys.iter().enumerate() {
            let hp = Pos2::new(x_of(k.0), bar.bottom() + handle_h * 0.5);
            let fill = Color32::from_rgba_unmultiplied(
                (k.1[0].clamp(0.0, 1.0) * 255.0) as u8,
                (k.1[1].clamp(0.0, 1.0) * 255.0) as u8,
                (k.1[2].clamp(0.0, 1.0) * 255.0) as u8,
                255,
            );
            let outline = if active == Some(i) || hovered == Some(i) {
                Color32::WHITE
            } else {
                Color32::from_gray(30)
            };
            let tri = vec![
                Pos2::new(hp.x, bar.bottom()),
                Pos2::new(hp.x - 6.0, bar.bottom() + handle_h),
                Pos2::new(hp.x + 6.0, bar.bottom() + handle_h),
            ];
            painter.add(egui::Shape::convex_polygon(
                tri,
                fill,
                Stroke::new(1.0, outline),
            ));
        }

        // Colour pickers for each key, in a compact row below the bar.
        ui.horizontal_wrapped(|ui| {
            for (i, k) in keys.iter_mut().enumerate() {
                let before = k.1;
                ui.label(format!("{:.2}", k.0));
                ui.push_id((id, i), |ui| {
                    ui.color_edit_button_rgba_unmultiplied(&mut k.1);
                });
                if k.1 != before {
                    resp.changed = true;
                    resp.committed = true;
                }
            }
        });

        if response.hovered() && hovered.is_none() {
            response.clone().on_hover_text(
                "double-click: add key · right-click handle: remove · drag handle: move",
            );
        }
        resp
    }
}

/// Linear RGBA gradient sample.
pub fn sample_gradient(keys: &[(f32, [f32; 4])], t: f32) -> [f32; 4] {
    if keys.is_empty() {
        return [1.0; 4];
    }
    if t <= keys[0].0 {
        return keys[0].1;
    }
    let last = keys[keys.len() - 1];
    if t >= last.0 {
        return last.1;
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.0 && t <= b.0 {
            let span = (b.0 - a.0).max(1e-6);
            let u = (t - a.0) / span;
            let mut out = [0.0; 4];
            for (o, (x, y)) in out.iter_mut().zip(a.1.iter().zip(b.1.iter())) {
                *o = x + (y - x) * u;
            }
            return out;
        }
    }
    last.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_sample_interpolates_and_clamps() {
        let keys = vec![[0.0, 0.0], [0.5, 1.0], [1.0, 0.0]];
        assert_eq!(linear_sample(&keys, -1.0), 0.0);
        assert!((linear_sample(&keys, 0.25) - 0.5).abs() < 1e-6);
        assert!((linear_sample(&keys, 0.5) - 1.0).abs() < 1e-6);
        assert!((linear_sample(&keys, 0.75) - 0.5).abs() < 1e-6);
        assert_eq!(linear_sample(&keys, 2.0), 0.0);
    }

    #[test]
    fn gradient_sample_midpoint() {
        let keys = vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, 1.0, 0.0])];
        let c = sample_gradient(&keys, 0.5);
        assert!(
            (c[0] - 0.5).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6 && (c[3] - 0.5).abs() < 1e-6
        );
    }
}
