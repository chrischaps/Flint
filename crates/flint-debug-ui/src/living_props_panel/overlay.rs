//! The panel in the world (#248): the pose overlay's labels, the picked
//! tree's rings and window, and which prop a click lands on.
//!
//! The projection is a world point through the camera's `view_proj` onto
//! egui's screen, the same maths `flint-viewer`'s gizmos do with their own
//! copy.

use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use super::state::LivingPropRow;
use super::{LivingPropsDebugPanel, INK, PARTS};

impl LivingPropsDebugPanel {
    /// Paint the pose overlay, if it's on: each prop's lineage and poses over its
    /// crown, after a small dial of where it stands in its step. The nearest get
    /// to speak first and a label that would land on one stays quiet, so a grove
    /// reads as a grove: every prop is in the panel's Props section regardless.
    pub fn paint_overlay(&mut self, ctx: &egui::Context, view_proj: &[[f32; 4]; 4]) {
        if self.selected.is_some() {
            self.paint_picked(ctx, view_proj);
        }
        self.paint_labels(ctx, view_proj);
    }

    /// Each prop's lineage and poses, if the pose overlay is on.
    pub(super) fn paint_labels(&self, ctx: &egui::Context, view_proj: &[[f32; 4]; 4]) {
        if !self.state.overlay {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("living_pose_overlay"),
        ));
        let screen = ctx.content_rect();
        let mut placed: Vec<(f32, Pos2, &LivingPropRow)> = self
            .state
            .rows
            .iter()
            .filter(|row| self.labelled(row))
            .filter_map(|row| {
                let (at, depth) = project(row, view_proj, screen)?;
                Some((depth, at, row))
            })
            .collect();
        placed.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken: Vec<Rect> = Vec::with_capacity(placed.len());
        for (depth, at, row) in placed {
            let near = (1.0 - (depth - 10.0) / 60.0).clamp(0.25, 1.0);
            let label = Label::of(&painter, at, row, near);
            if taken.iter().any(|other| other.intersects(label.area)) {
                continue;
            }
            taken.push(label.area);
            label.paint(&painter, at, row, near);
        }
    }

    /// The picked tree's marks and window (#248): a ring on the ground under
    /// it and, fainter, under every tree of its species in view, and beside it a
    /// window of what it reads, with its species' sliders. A tree off screen
    /// draws nothing and the panel says so.
    pub(super) fn paint_picked(&mut self, ctx: &egui::Context, view_proj: &[[f32; 4]; 4]) {
        let Some(row) = self.picked_row().cloned() else {
            self.picked_on_screen = None;
            return;
        };
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("living_picked_marks"),
        ));
        let kin = Stroke::new(1.0_f32, shaded(INK, 0.4));
        for other in (self.state.rows.iter())
            .filter(|other| other.species == row.species && other.name != row.name)
        {
            paint_ring(&painter, other, view_proj, screen, 1.0, kin);
        }
        paint_ring(
            &painter,
            &row,
            view_proj,
            screen,
            1.0,
            Stroke::new(2.0_f32, INK),
        );
        paint_ring(
            &painter,
            &row,
            view_proj,
            screen,
            0.82,
            Stroke::new(1.0_f32, shaded(INK, 0.55)),
        );
        let Some(anchor) = anchor_of(&row, view_proj, screen) else {
            self.picked_on_screen = Some(false);
            return;
        };
        self.picked_on_screen = Some(true);
        let room = match self.panel_left.filter(|_| self.open) {
            Some(left) if left > screen.left() + 200.0 && left < screen.right() => {
                Rect::from_min_max(screen.min, Pos2::new(left - 6.0, screen.max.y))
            }
            _ => screen,
        };
        let right = anchor.x < room.center().x;
        let (pivot, at) = if right {
            (Align2::LEFT_CENTER, anchor + Vec2::new(28.0, 0.0))
        } else {
            (Align2::RIGHT_CENTER, anchor - Vec2::new(28.0, 0.0))
        };
        let shown = egui::Area::new(egui::Id::new("living_picked_window"))
            .order(egui::Order::Middle)
            .fixed_pos(at)
            .pivot(pivot)
            .constrain_to(room.shrink(4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| self.picked_window(ui, &row));
            });
        // A thread from the crown to the window, so a grove of them reads.
        let window = shown.response.rect;
        let edge = Pos2::new(
            if right { window.left() } else { window.right() },
            anchor.y.clamp(
                window.top() + 8.0,
                (window.bottom() - 8.0).max(window.top() + 8.0),
            ),
        );
        painter.line_segment([anchor, edge], Stroke::new(1.0_f32, shaded(INK, 0.6)));
        painter.circle_filled(anchor, 3.0, INK);
    }
}

/// Which prop is under `at` on screen (#248), the nearest if several are: its
/// index in `rows`. Each prop is the screen-space shape of its tree, its trunk
/// line from foot to crown top, widening toward the crown to its reach.
/// Approximate, since a prop has no collider and its shape keeps changing;
/// the cycle key covers a miss.
pub fn pick_prop(
    rows: &[LivingPropRow],
    view_proj: &[[f32; 4]; 4],
    screen: Rect,
    at: Pos2,
) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, row) in rows.iter().enumerate() {
        let Some(depth) = hit_depth(row, view_proj, screen, at) else {
            continue;
        };
        if best.is_none_or(|(nearest, _)| depth < nearest) {
            best = Some((depth, i));
        }
    }
    best.map(|(_, i)| i)
}

/// How far a prop's reach is, sideways: its rest crown's, or a guess from its
/// height before its lineage has built.
pub(super) fn reach_of(row: &LivingPropRow) -> f32 {
    if row.reach > 0.0 {
        row.reach
    } else {
        row.height * 0.3
    }
}

/// How far off the camera `row` is where `at` lands on it, if it does.
pub(super) fn hit_depth(
    row: &LivingPropRow,
    m: &[[f32; 4]; 4],
    screen: Rect,
    at: Pos2,
) -> Option<f32> {
    let [x, y, z] = row.position;
    let (foot, near_foot) = project_point([x, y, z], m, screen)?;
    let (top, near_top) = project_point([x, y + row.height.max(0.5), z], m, screen)?;
    let line = top - foot;
    let t = if line.length_sq() > 0.0 {
        ((at - foot).dot(line) / line.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let depth = near_foot + (near_top - near_foot) * t;
    // The trunk's a third of the crown's width; the crown's top is all of it.
    let radius = (reach_of(row) * focal_of(m, screen) / depth * (0.3 + 0.7 * t)).max(6.0);
    ((foot + line * t).distance(at) <= radius).then_some(depth)
}

/// Points per metre at a metre off the camera, across the screen: the length of
/// clip x's gradient, since a view is a rotation.
pub(super) fn focal_of(m: &[[f32; 4]; 4], screen: Rect) -> f32 {
    let gx = m[0][0] * m[0][0] + m[1][0] * m[1][0] + m[2][0] * m[2][0];
    gx.sqrt() * screen.width() * 0.5
}

/// Where a world point lands on screen and how far off it is, if it's in front
/// of the camera: on screen or not.
pub(super) fn project_point(p: [f32; 3], m: &[[f32; 4]; 4], screen: Rect) -> Option<(Pos2, f32)> {
    let clip = |i: usize| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i];
    let w = clip(3);
    if w <= 0.1 {
        return None;
    }
    let (x, y) = (clip(0) / w, clip(1) / w);
    Some((
        Pos2::new(
            screen.left() + (x * 0.5 + 0.5) * screen.width(),
            screen.top() + (0.5 - y * 0.5) * screen.height(),
        ),
        w,
    ))
}

/// Where a picked tree's window hangs from: its crown top, or partway down if
/// that's off screen, or its foot. `None` when none of them is on screen.
pub(super) fn anchor_of(row: &LivingPropRow, m: &[[f32; 4]; 4], screen: Rect) -> Option<Pos2> {
    let [x, y, z] = row.position;
    [1.0, 0.5, 0.0].into_iter().find_map(|up| {
        let (at, _) = project_point([x, y + row.height * up, z], m, screen)?;
        screen.contains(at).then_some(at)
    })
}

/// A ring on the ground around a tree, at its reach, in perspective. Nothing
/// if any of it is behind the camera or all of it off screen.
pub(super) fn paint_ring(
    painter: &egui::Painter,
    row: &LivingPropRow,
    m: &[[f32; 4]; 4],
    screen: Rect,
    scale: f32,
    stroke: Stroke,
) {
    const SIDES: usize = 40;
    let reach = reach_of(row).max(0.4) * scale;
    let [x, y, z] = row.position;
    let points: Option<Vec<Pos2>> = (0..SIDES)
        .map(|i| {
            let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
            let p = [x + reach * a.cos(), y + 0.05, z + reach * a.sin()];
            project_point(p, m, screen).map(|(at, _)| at)
        })
        .collect();
    let Some(points) = points else {
        return;
    };
    if !points.iter().any(|p| screen.contains(*p)) {
        return;
    }
    painter.add(Shape::closed_line(points, stroke));
}

/// Where a prop's label goes on screen, and how far off it is: the top of its
/// crown, where distance spreads a grove's labels up the screen. Partway up the
/// trunks, they'd all sit in one line at the horizon.
pub(super) fn project(row: &LivingPropRow, m: &[[f32; 4]; 4], screen: Rect) -> Option<(Pos2, f32)> {
    let p = [
        row.position[0],
        row.position[1] + row.height,
        row.position[2],
    ];
    let clip = |i: usize| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i];
    let w = clip(3);
    if w <= 0.1 {
        return None;
    }
    let (x, y) = (clip(0) / w, clip(1) / w);
    if x.abs() > 1.1 || y.abs() > 1.1 {
        return None;
    }
    Some((
        Pos2::new(
            screen.left() + (x * 0.5 + 0.5) * screen.width(),
            screen.top() + (0.5 - y * 0.5) * screen.height(),
        ),
        w,
    ))
}

/// `c` at `a` of its strength. Blending is in display space, so this premultiplies
/// by hand (see CLAUDE.md).
pub(super) fn shaded(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_premultiplied(
        (f32::from(c.r()) * a) as u8,
        (f32::from(c.g()) * a) as u8,
        (f32::from(c.b()) * a) as u8,
        (255.0 * a) as u8,
    )
}

/// One prop's label, laid out: a dial, its lineage, and the two poses it's between.
pub(super) struct Label {
    galley: std::sync::Arc<egui::Galley>,
    /// Where the text starts.
    left: Pos2,
    /// What it covers, dial and all, for telling whether another would land on it.
    area: Rect,
    radius: f32,
}

impl Label {
    pub(super) fn of(painter: &egui::Painter, at: Pos2, row: &LivingPropRow, near: f32) -> Self {
        let font = FontId::monospace(9.0 + 2.0 * near);
        let radius = 3.0 + 3.0 * near;
        let text = format!(
            "L{}  {}>{}{}",
            row.lineage_seed,
            row.pose,
            row.pose + 1,
            if row.waiting { " ..." } else { "" }
        );
        let colour = shaded(Color32::from_rgb(244, 236, 220), near);
        let galley = painter.layout_no_wrap(text, font, colour);
        let left = at + Vec2::new(radius + 3.0, -galley.size().y * 0.5);
        let area = Rect::from_min_size(left, galley.size())
            .union(Rect::from_center_size(at, Vec2::splat(radius * 2.0)))
            .expand(2.0);
        Self {
            galley,
            left,
            area,
            radius,
        }
    }

    pub(super) fn paint(self, painter: &egui::Painter, at: Pos2, row: &LivingPropRow, near: f32) {
        painter.rect_filled(
            self.area,
            3.0,
            Color32::from_rgba_premultiplied(0, 0, 0, (150.0 * near) as u8),
        );
        painter.galley(self.left, self.galley, Color32::PLACEHOLDER);
        // A dial for its place in the step: the needle goes round once a beat.
        let radius = self.radius;
        painter.circle_stroke(at, radius, Stroke::new(1.0_f32, shaded(PARTS, near)));
        let turn = row.beats.fract() as f32 * std::f32::consts::TAU;
        let tip = at + Vec2::new(turn.sin(), -turn.cos()) * radius;
        painter.line_segment([at, tip], Stroke::new(1.5_f32, shaded(INK, near)));
    }
}
