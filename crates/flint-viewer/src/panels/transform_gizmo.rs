//! 3D transform gizmo — translate/rotate/scale manipulation handles as egui overlay
//!
//! Renders colored axis handles over the selected entity's position, projected
//! from 3D to screen space using the camera's view-projection matrix. Follows
//! the same pattern as ViewGizmo (egui painter overlay, not wgpu geometry).

use crate::undo::EditAction;
use flint_core::EntityId;
use flint_render::Camera;

// Colors (matching view gizmo for consistency)
const X_COLOR: egui::Color32 = egui::Color32::from_rgb(214, 67, 67);
const Y_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 172, 67);
const Z_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 118, 214);
const X_HOVER: egui::Color32 = egui::Color32::from_rgb(255, 100, 100);
const Y_HOVER: egui::Color32 = egui::Color32::from_rgb(100, 220, 100);
const Z_HOVER: egui::Color32 = egui::Color32::from_rgb(100, 150, 255);
const PLANE_ALPHA: u8 = 50;

const HANDLE_LENGTH: f32 = 80.0;
const ARROW_HEAD_SIZE: f32 = 10.0;
const HIT_THRESHOLD: f32 = 8.0;
const RING_RADIUS: f32 = 60.0;
const RING_SEGMENTS: usize = 48;
const SCALE_CUBE_SIZE: f32 = 6.0;
const PLANE_SIZE: f32 = 24.0;

/// Which transformation mode is active
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

/// Which axis or plane the user is interacting with
#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoAxis {
    X,
    Y,
    Z,
    XY,
    XZ,
    YZ,
}

/// Result of a gizmo drag operation
#[derive(Debug, Clone)]
pub struct GizmoDelta {
    pub entity_id: EntityId,
    pub position_delta: [f32; 3],
    pub rotation_delta: [f32; 3], // degrees
    pub scale_delta: [f32; 3],    // multiplicative factors (1.0 = no change)
}

/// 3D transform gizmo that renders as an egui overlay
pub struct TransformGizmo {
    pub mode: GizmoMode,
    hovered_axis: Option<GizmoAxis>,
    active_axis: Option<GizmoAxis>,
    drag_start_mouse: Option<egui::Pos2>,
    drag_start_transform: Option<([f32; 3], [f32; 3], [f32; 3])>, // pos, rot, scale
    drag_entity: Option<EntityId>,
    last_mouse_pos: Option<egui::Pos2>,
}

impl TransformGizmo {
    pub fn new() -> Self {
        Self {
            mode: GizmoMode::Translate,
            hovered_axis: None,
            active_axis: None,
            drag_start_mouse: None,
            drag_start_transform: None,
            drag_entity: None,
            last_mouse_pos: None,
        }
    }

    /// Whether the gizmo is currently being dragged
    pub fn is_dragging(&self) -> bool {
        self.active_axis.is_some()
    }

    /// Whether any handle is hovered
    pub fn is_hovered(&self) -> bool {
        self.hovered_axis.is_some()
    }

    /// Get the initial transform when the drag started (for undo coalescing)
    pub fn drag_start_transform(&self) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
        self.drag_start_transform
    }

    /// Get the entity currently being dragged
    pub fn drag_entity(&self) -> Option<EntityId> {
        self.drag_entity
    }

    /// Draw the gizmo and handle interaction. Returns a delta to apply if the user dragged.
    ///
    /// `render_rect` is the wgpu render viewport (typically full screen) used for 3D→2D projection.
    /// `clip_rect` is the visible panel area (central region between side panels) used for paint clipping.
    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        camera: &Camera,
        entity_id: EntityId,
        entity_pos: [f32; 3],
        entity_rot: [f32; 3],
        entity_scale: [f32; 3],
        render_rect: egui::Rect,
        clip_rect: egui::Rect,
    ) -> Option<GizmoDelta> {
        let vp = camera.view_projection_matrix();
        let screen_pos = project_point(&vp, entity_pos, render_rect);

        // Don't draw if entity is behind camera
        if screen_pos.is_none() {
            return None;
        }
        let center = screen_pos.unwrap();

        // Scale handles inversely with camera distance for consistent screen size
        let cam_dist = {
            let dx = camera.position.x - entity_pos[0];
            let dy = camera.position.y - entity_pos[1];
            let dz = camera.position.z - entity_pos[2];
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let scale_factor = (cam_dist / 10.0).clamp(0.3, 3.0);

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("transform_gizmo"),
        )).with_clip_rect(clip_rect);

        // Project axis directions to screen space using the same VP matrix as the center point
        let axis_dirs = self.compute_axis_screen_dirs(&vp, entity_pos, center, render_rect);

        // Check pointer position
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let primary_released = ctx.input(|i| i.pointer.primary_released());

        // Hit testing (only respond to clicks within the visible clip area)
        if self.active_axis.is_none() {
            self.hovered_axis = None;
            if let Some(pos) = pointer_pos {
                if clip_rect.contains(pos) {
                    self.hovered_axis = self.hit_test(center, pos, &axis_dirs, scale_factor);
                }
            }
        }

        // Start drag
        if primary_pressed && self.hovered_axis.is_some() {
            self.active_axis = self.hovered_axis;
            self.drag_start_mouse = pointer_pos;
            self.drag_start_transform = Some((entity_pos, entity_rot, entity_scale));
            self.drag_entity = Some(entity_id);
            self.last_mouse_pos = pointer_pos;
        }

        // Compute drag delta
        let mut result = None;
        if let (Some(axis), Some(current_pos), true) = (self.active_axis, pointer_pos, primary_down) {
            if let Some(last_pos) = self.last_mouse_pos {
                let mouse_delta = current_pos - last_pos;
                if mouse_delta.length() > 0.0 {
                    result = Some(self.compute_delta(
                        entity_id,
                        axis,
                        mouse_delta,
                        &axis_dirs,
                        cam_dist,
                        render_rect.height(),
                    ));
                }
            }
            self.last_mouse_pos = Some(current_pos);
        }

        // End drag
        if primary_released && self.active_axis.is_some() {
            self.active_axis = None;
            self.drag_start_mouse = None;
            self.last_mouse_pos = None;
            // drag_start_transform kept for undo coalescing — cleared externally
        }

        // Render
        match self.mode {
            GizmoMode::Translate => self.draw_translate(&painter, center, &axis_dirs, scale_factor),
            GizmoMode::Rotate => self.draw_rotate(&painter, center, &axis_dirs, scale_factor),
            GizmoMode::Scale => self.draw_scale(&painter, center, &axis_dirs, scale_factor),
        }

        // Set cursor
        if self.active_axis.is_some() || self.hovered_axis.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }

        result
    }

    /// Clear the stored drag start transform (call after pushing undo)
    pub fn clear_drag_start(&mut self) {
        self.drag_start_transform = None;
        self.drag_entity = None;
    }

    fn compute_axis_screen_dirs(
        &self,
        vp: &[[f32; 4]; 4],
        entity_pos: [f32; 3],
        center: egui::Pos2,
        viewport_rect: egui::Rect,
    ) -> [[f32; 2]; 3] {
        // Project points along each world axis through the full VP matrix so that
        // perspective is accounted for (matching how the gizmo center is projected).
        let world_axes: [[f32; 3]; 3] = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let mut screen_dirs = [[0.0f32; 2]; 3];

        for (i, axis) in world_axes.iter().enumerate() {
            let world_tip = [
                entity_pos[0] + axis[0],
                entity_pos[1] + axis[1],
                entity_pos[2] + axis[2],
            ];
            if let Some(tip_screen) = project_point(vp, world_tip, viewport_rect) {
                let dx = tip_screen.x - center.x;
                let dy = tip_screen.y - center.y;
                let len = (dx * dx + dy * dy).sqrt();
                if len > 1e-6 {
                    screen_dirs[i] = [dx / len, dy / len];
                }
            }
        }

        screen_dirs
    }

    fn hit_test(
        &self,
        center: egui::Pos2,
        mouse: egui::Pos2,
        axis_dirs: &[[f32; 2]; 3],
        scale: f32,
    ) -> Option<GizmoAxis> {
        let length = HANDLE_LENGTH * scale;

        match self.mode {
            GizmoMode::Translate => {
                // Test plane handles first (small squares at axis intersections)
                let plane_len = PLANE_SIZE * scale;
                let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];
                let planes = [
                    (GizmoAxis::XY, 0, 1),
                    (GizmoAxis::XZ, 0, 2),
                    (GizmoAxis::YZ, 1, 2),
                ];

                for (plane_axis, a, b) in &planes {
                    let da = egui::vec2(axis_dirs[*a][0], axis_dirs[*a][1]) * plane_len * 0.5;
                    let db = egui::vec2(axis_dirs[*b][0], axis_dirs[*b][1]) * plane_len * 0.5;
                    let plane_center = center + da + db;
                    if (mouse - plane_center).length() < plane_len * 0.5 {
                        return Some(*plane_axis);
                    }
                }

                // Test axis lines
                for (i, axis) in axes.iter().enumerate() {
                    let end = center + egui::vec2(axis_dirs[i][0], axis_dirs[i][1]) * length;
                    if point_to_segment_dist(mouse, center, end) < HIT_THRESHOLD {
                        return Some(*axis);
                    }
                }
                None
            }

            GizmoMode::Rotate => {
                let radius = RING_RADIUS * scale;
                let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

                // For rotation, test distance to projected ring
                // Simplified: test distance from mouse to the arc
                for (i, axis) in axes.iter().enumerate() {
                    // The ring for axis i lies in the plane perpendicular to axis i
                    // We approximate by testing distance from center
                    let dist = (mouse - center).length();
                    if (dist - radius).abs() < HIT_THRESHOLD * 1.5 {
                        // Check if mouse is near the axis ring by looking at the angle
                        let dm = mouse - center;
                        let angle = dm.y.atan2(dm.x);

                        // The ring for axis i is projected differently depending on view
                        // Simplified: accept any angle for now, differentiate by which
                        // sector the mouse is in
                        let sector = ((angle + std::f32::consts::PI) / (std::f32::consts::TAU / 3.0)) as usize;
                        if sector == i || (i == 2 && sector >= 2) {
                            return Some(*axis);
                        }
                    }
                }

                // More forgiving: check each axis ring with wider tolerance
                let dist = (mouse - center).length();
                if (dist - radius).abs() < HIT_THRESHOLD * 2.0 {
                    // Pick the ring whose axis is most perpendicular to view
                    let forward = [0.0, 0.0, 1.0]; // approximate
                    let mut best = GizmoAxis::Y;
                    let mut best_dot = 0.0f32;
                    let world_axes: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                    let gizmo_axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];
                    for (i, wa) in world_axes.iter().enumerate() {
                        let d = (wa[0] * forward[0] + wa[1] * forward[1] + wa[2] * forward[2]).abs();
                        // Higher dot = axis more parallel to view = ring more visible
                        let ring_visibility = 1.0 - d;
                        if ring_visibility > best_dot {
                            best_dot = ring_visibility;
                            best = gizmo_axes[i];
                        }
                    }
                    return Some(best);
                }

                None
            }

            GizmoMode::Scale => {
                let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];
                for (i, axis) in axes.iter().enumerate() {
                    let end = center + egui::vec2(axis_dirs[i][0], axis_dirs[i][1]) * length;
                    if point_to_segment_dist(mouse, center, end) < HIT_THRESHOLD {
                        return Some(*axis);
                    }
                }
                None
            }
        }
    }

    fn compute_delta(
        &self,
        entity_id: EntityId,
        axis: GizmoAxis,
        mouse_delta: egui::Vec2,
        axis_dirs: &[[f32; 2]; 3],
        cam_dist: f32,
        viewport_height: f32,
    ) -> GizmoDelta {
        // World-space scaling: convert pixel drag to world units
        let pixel_to_world = cam_dist / viewport_height * 2.0;

        match self.mode {
            GizmoMode::Translate => {
                let mut pos_delta = [0.0f32; 3];
                let indices = axis_indices(axis);

                for &i in &indices {
                    let screen_dir = egui::vec2(axis_dirs[i][0], axis_dirs[i][1]);
                    let projected = mouse_delta.dot(screen_dir);
                    pos_delta[i] = projected * pixel_to_world;
                }

                GizmoDelta {
                    entity_id,
                    position_delta: pos_delta,
                    rotation_delta: [0.0; 3],
                    scale_delta: [1.0; 3],
                }
            }

            GizmoMode::Rotate => {
                let mut rot_delta = [0.0f32; 3];
                let indices = axis_indices(axis);

                for &i in &indices {
                    // Use horizontal mouse motion for rotation
                    let degrees_per_pixel = 0.5;
                    let screen_dir = egui::vec2(axis_dirs[i][0], axis_dirs[i][1]);
                    let perp = egui::vec2(-screen_dir[1], screen_dir[0]);
                    let projected = mouse_delta.dot(perp);
                    rot_delta[i] = projected * degrees_per_pixel;
                }

                GizmoDelta {
                    entity_id,
                    position_delta: [0.0; 3],
                    rotation_delta: rot_delta,
                    scale_delta: [1.0; 3],
                }
            }

            GizmoMode::Scale => {
                let mut scale_delta = [1.0f32; 3];
                let indices = axis_indices(axis);

                for &i in &indices {
                    let screen_dir = egui::vec2(axis_dirs[i][0], axis_dirs[i][1]);
                    let projected = mouse_delta.dot(screen_dir);
                    let factor = 1.0 + projected * 0.005;
                    scale_delta[i] = factor.clamp(0.01, 100.0);
                }

                GizmoDelta {
                    entity_id,
                    position_delta: [0.0; 3],
                    rotation_delta: [0.0; 3],
                    scale_delta,
                }
            }
        }
    }

    fn draw_translate(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        axis_dirs: &[[f32; 2]; 3],
        scale: f32,
    ) {
        let length = HANDLE_LENGTH * scale;
        let head = ARROW_HEAD_SIZE * scale;
        let colors = [X_COLOR, Y_COLOR, Z_COLOR];
        let hover_colors = [X_HOVER, Y_HOVER, Z_HOVER];
        let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

        // Draw plane handles
        let planes: [(GizmoAxis, usize, usize); 3] = [
            (GizmoAxis::XY, 0, 1),
            (GizmoAxis::XZ, 0, 2),
            (GizmoAxis::YZ, 1, 2),
        ];

        for (plane_axis, a, b) in &planes {
            let plane_len = PLANE_SIZE * scale;
            let da = egui::vec2(axis_dirs[*a][0], axis_dirs[*a][1]) * plane_len;
            let db = egui::vec2(axis_dirs[*b][0], axis_dirs[*b][1]) * plane_len;
            let c = colors[*a];

            let is_active = self.active_axis == Some(*plane_axis) || self.hovered_axis == Some(*plane_axis);
            let alpha = if is_active { PLANE_ALPHA + 40 } else { PLANE_ALPHA };

            let fill = egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);
            let p0 = center;
            let p1 = center + da;
            let p2 = center + da + db;
            let p3 = center + db;

            painter.add(egui::Shape::convex_polygon(
                vec![p0, p1, p2, p3],
                fill,
                egui::Stroke::NONE,
            ));
        }

        // Draw axis arrows
        for (i, axis) in axes.iter().enumerate() {
            let dir = egui::vec2(axis_dirs[i][0], axis_dirs[i][1]);
            let end = center + dir * length;
            let is_active = self.active_axis == Some(*axis) || self.hovered_axis == Some(*axis);
            let color = if is_active { hover_colors[i] } else { colors[i] };
            let width = if is_active { 3.0 } else { 2.0 };

            // Line
            painter.line_segment([center, end], egui::Stroke::new(width, color));

            // Arrow head
            let perp = egui::vec2(-dir.y, dir.x);
            let tip = end + dir * head;
            let left = end + perp * head * 0.4;
            let right = end - perp * head * 0.4;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, left, right],
                color,
                egui::Stroke::NONE,
            ));
        }

        // Center dot
        painter.circle_filled(center, 3.0, egui::Color32::WHITE);
    }

    fn draw_rotate(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        axis_dirs: &[[f32; 2]; 3],
        scale: f32,
    ) {
        let radius = RING_RADIUS * scale;
        let colors = [X_COLOR, Y_COLOR, Z_COLOR];
        let hover_colors = [X_HOVER, Y_HOVER, Z_HOVER];
        let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

        // Draw three rings, each perpendicular to its axis
        for (i, axis) in axes.iter().enumerate() {
            let is_active = self.active_axis == Some(*axis) || self.hovered_axis == Some(*axis);
            let color = if is_active { hover_colors[i] } else { colors[i] };
            let width = if is_active { 2.5 } else { 1.5 };

            // Get the two axes perpendicular to this one
            let a = (i + 1) % 3;
            let b = (i + 2) % 3;
            let dir_a = egui::vec2(axis_dirs[a][0], axis_dirs[a][1]);
            let dir_b = egui::vec2(axis_dirs[b][0], axis_dirs[b][1]);

            // Draw ring as line segments
            let mut points = Vec::with_capacity(RING_SEGMENTS + 1);
            for j in 0..=RING_SEGMENTS {
                let angle = (j as f32 / RING_SEGMENTS as f32) * std::f32::consts::TAU;
                let p = center + dir_a * (angle.cos() * radius) + dir_b * (angle.sin() * radius);
                points.push(p);
            }

            for j in 0..RING_SEGMENTS {
                painter.line_segment(
                    [points[j], points[j + 1]],
                    egui::Stroke::new(width, color),
                );
            }
        }

        // Center dot
        painter.circle_filled(center, 3.0, egui::Color32::WHITE);
    }

    fn draw_scale(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        axis_dirs: &[[f32; 2]; 3],
        scale: f32,
    ) {
        let length = HANDLE_LENGTH * scale;
        let cube = SCALE_CUBE_SIZE * scale;
        let colors = [X_COLOR, Y_COLOR, Z_COLOR];
        let hover_colors = [X_HOVER, Y_HOVER, Z_HOVER];
        let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

        for (i, axis) in axes.iter().enumerate() {
            let dir = egui::vec2(axis_dirs[i][0], axis_dirs[i][1]);
            let end = center + dir * length;
            let is_active = self.active_axis == Some(*axis) || self.hovered_axis == Some(*axis);
            let color = if is_active { hover_colors[i] } else { colors[i] };
            let width = if is_active { 3.0 } else { 2.0 };

            // Line
            painter.line_segment([center, end], egui::Stroke::new(width, color));

            // Cube at end
            let half = cube / 2.0;
            painter.rect_filled(
                egui::Rect::from_center_size(end, egui::vec2(half * 2.0, half * 2.0)),
                0.0,
                color,
            );
        }

        // Center cube
        painter.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(6.0, 6.0)),
            0.0,
            egui::Color32::WHITE,
        );
    }
}

/// Create edit actions from a gizmo delta applied to current transform
pub fn apply_gizmo_delta(
    delta: &GizmoDelta,
    current_pos: [f32; 3],
    current_rot: [f32; 3],
    current_scale: [f32; 3],
) -> Vec<EditAction> {
    let mut actions = Vec::new();

    // Position
    if delta.position_delta.iter().any(|d| d.abs() > 1e-6) {
        let new_pos = [
            current_pos[0] + delta.position_delta[0],
            current_pos[1] + delta.position_delta[1],
            current_pos[2] + delta.position_delta[2],
        ];
        actions.push(EditAction {
            entity_id: delta.entity_id,
            component: "transform".to_string(),
            field: "position".to_string(),
            old_value: vec3_to_toml(current_pos),
            new_value: vec3_to_toml(new_pos),
        });
    }

    // Rotation
    if delta.rotation_delta.iter().any(|d| d.abs() > 1e-6) {
        let new_rot = [
            current_rot[0] + delta.rotation_delta[0],
            current_rot[1] + delta.rotation_delta[1],
            current_rot[2] + delta.rotation_delta[2],
        ];
        actions.push(EditAction {
            entity_id: delta.entity_id,
            component: "transform".to_string(),
            field: "rotation".to_string(),
            old_value: vec3_to_toml(current_rot),
            new_value: vec3_to_toml(new_rot),
        });
    }

    // Scale
    if delta.scale_delta.iter().any(|d| (d - 1.0).abs() > 1e-6) {
        let new_scale = [
            current_scale[0] * delta.scale_delta[0],
            current_scale[1] * delta.scale_delta[1],
            current_scale[2] * delta.scale_delta[2],
        ];
        actions.push(EditAction {
            entity_id: delta.entity_id,
            component: "transform".to_string(),
            field: "scale".to_string(),
            old_value: vec3_to_toml(current_scale),
            new_value: vec3_to_toml(new_scale),
        });
    }

    actions
}

fn vec3_to_toml(v: [f32; 3]) -> toml::Value {
    toml::Value::Array(vec![
        toml::Value::Float(v[0] as f64),
        toml::Value::Float(v[1] as f64),
        toml::Value::Float(v[2] as f64),
    ])
}

/// Project a 3D world point to screen coordinates
fn project_point(
    vp: &[[f32; 4]; 4],
    point: [f32; 3],
    viewport: egui::Rect,
) -> Option<egui::Pos2> {
    let clip = [
        vp[0][0] * point[0] + vp[1][0] * point[1] + vp[2][0] * point[2] + vp[3][0],
        vp[0][1] * point[0] + vp[1][1] * point[1] + vp[2][1] * point[2] + vp[3][1],
        vp[0][2] * point[0] + vp[1][2] * point[1] + vp[2][2] * point[2] + vp[3][2],
        vp[0][3] * point[0] + vp[1][3] * point[1] + vp[2][3] * point[2] + vp[3][3],
    ];

    // Behind camera
    if clip[3] <= 0.0 {
        return None;
    }

    let ndc_x = clip[0] / clip[3];
    let ndc_y = clip[1] / clip[3];

    // NDC to screen coordinates
    let screen_x = viewport.left() + (ndc_x + 1.0) * 0.5 * viewport.width();
    let screen_y = viewport.top() + (1.0 - ndc_y) * 0.5 * viewport.height();

    Some(egui::pos2(screen_x, screen_y))
}

/// Point-to-line-segment distance
fn point_to_segment_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let t = ap.dot(ab) / ab.dot(ab);
    let t = t.clamp(0.0, 1.0);
    let closest = a + ab * t;
    (p - closest).length()
}

/// Get the axis indices for a given gizmo axis
fn axis_indices(axis: GizmoAxis) -> Vec<usize> {
    match axis {
        GizmoAxis::X => vec![0],
        GizmoAxis::Y => vec![1],
        GizmoAxis::Z => vec![2],
        GizmoAxis::XY => vec![0, 1],
        GizmoAxis::XZ => vec![0, 2],
        GizmoAxis::YZ => vec![1, 2],
    }
}
