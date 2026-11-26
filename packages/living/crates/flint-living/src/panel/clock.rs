//! The warped clock drawn as the curve it is.

use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use super::state::LivingPropRow;
use super::{FROZEN, INK, PARTS};

/// Each branch level's phase, as a fraction of a step. Drawn, not computed with:
/// `flint_procgen::STAGGER_PER_LEVEL` is what the clock itself uses.
pub(super) const LEVEL_PHASE: f64 = 0.15;
/// How many levels the readout draws.
pub(super) const LEVELS_DRAWN: usize = 5;

/// How far toward the next pose a part is, `frac` of the way through a step: the
/// design's t', as `flint_core::step_clock::warp` works it out.
pub(super) fn eased(frac: f64, glide: f32) -> f32 {
    flint_core::step_clock::warp(frac.clamp(0.0, 0.999_999), glide).1
}

/// The warped clock as what it is: a step's worth of hold, then the ease to the
/// next pose, with the prop riding it. With a stagger, each branch level rides it
/// a little ahead, so what the dial means by "shimmer" is there to see.
pub(super) fn draw_clock(
    ui: &mut egui::Ui,
    row: &LivingPropRow,
    stagger: &str,
    frozen: bool,
    height: f32,
) {
    let width = ui.available_width().min(320.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    // Under the labels, so the curve's top never reads through them.
    let plot = Rect::from_min_max(
        rect.left_top() + Vec2::new(8.0, 20.0),
        rect.right_bottom() - Vec2::new(8.0, 8.0),
    );
    let at = |frac: f64, weight: f32| {
        Pos2::new(
            plot.left() + frac as f32 * plot.width(),
            plot.bottom() - weight * plot.height(),
        )
    };
    let visuals = ui.visuals();
    let faint = visuals.weak_text_color();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);

    // Where the hold ends and the glide begins.
    let hold_end = f64::from(1.0 - row.glide);
    painter.line_segment(
        [at(hold_end, 0.0), at(hold_end, 1.0)],
        Stroke::new(1.0_f32, faint),
    );

    let accent = if frozen { FROZEN } else { INK };
    let points: Vec<Pos2> = (0..=64)
        .map(|i| {
            let frac = f64::from(i) / 64.0;
            at(frac, eased(frac, row.glide))
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.5_f32, accent)));

    if stagger == "per_level" {
        for level in (1..LEVELS_DRAWN).rev() {
            let frac = (row.beats + level as f64 * LEVEL_PHASE).fract();
            painter.circle_filled(at(frac, eased(frac, row.glide)), 2.5, PARTS);
        }
    }
    let frac = row.beats.fract();
    let dot = at(frac, eased(frac, row.glide));
    painter.circle_filled(dot, 4.0, accent);

    let label = FontId::monospace(10.0);
    painter.text(
        rect.left_top() + Vec2::new(5.0, 2.0),
        Align2::LEFT_TOP,
        format!("beat {:.3}", row.beats),
        label.clone(),
        faint,
    );
    painter.text(
        rect.right_top() + Vec2::new(-5.0, 2.0),
        Align2::RIGHT_TOP,
        if frozen {
            "frozen".to_string()
        } else {
            format!("{:.2} s / {:.0}% glide", row.step, row.glide * 100.0)
        },
        label,
        if frozen { FROZEN } else { faint },
    );
}
