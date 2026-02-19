//! Interactive 3D translate gizmo for the viewer.
//!
//! Renders axis arrows and plane handles as an egui overlay projected from 3D.
//! Supports click-drag to reposition entities along constrained axes or planes,
//! with an undo stack for reverting changes.

use crate::projection::*;
use flint_render::Camera;

// Colors — same hue family as the view gizmo for visual consistency
const X_COLOR: egui::Color32 = egui::Color32::from_rgb(214, 67, 67);
const Y_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 172, 67);
const Z_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 118, 214);
const X_BRIGHT: egui::Color32 = egui::Color32::from_rgb(255, 100, 100);
const Y_BRIGHT: egui::Color32 = egui::Color32::from_rgb(100, 230, 100);
const Z_BRIGHT: egui::Color32 = egui::Color32::from_rgb(100, 150, 255);
const PLANE_ALPHA: u8 = 60;
const PLANE_HOVER_ALPHA: u8 = 120;
const DIM_ALPHA: f32 = 0.25;

/// Which axis or plane the gizmo is operating on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
    XY,
    XZ,
    YZ,
}

impl GizmoAxis {
    fn axis_dir(&self) -> Option<[f32; 3]> {
        match self {
            GizmoAxis::X => Some([1.0, 0.0, 0.0]),
            GizmoAxis::Y => Some([0.0, 1.0, 0.0]),
            GizmoAxis::Z => Some([0.0, 0.0, 1.0]),
            _ => None,
        }
    }

}

/// A recorded transform change for undo/redo.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub entity_id: u64,
    pub entity_name: String,
    pub old_position: [f32; 3],
    pub new_position: [f32; 3],
}

/// Interactive translate gizmo state.
pub struct TransformGizmo {
    pub hovered_axis: Option<GizmoAxis>,
    active_axis: Option<GizmoAxis>,
    dragging: bool,
    drag_start_world_pos: [f32; 3],
    drag_plane_hit: [f32; 3],
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    pub modified: bool,
}

impl TransformGizmo {
    pub fn new() -> Self {
        Self {
            hovered_axis: None,
            active_axis: None,
            dragging: false,
            drag_start_world_pos: [0.0; 3],
            drag_plane_hit: [0.0; 3],
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            modified: false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Pick test: determine which gizmo axis (if any) is under the cursor.
    pub fn pick(
        &self,
        camera: &Camera,
        screen_size: [f32; 2],
        mx: f32,
        my: f32,
        entity_world_pos: [f32; 3],
    ) -> Option<GizmoAxis> {
        let (ray_o, ray_d) = screen_to_world_ray(camera, screen_size, mx, my);
        let gizmo_scale = camera.distance * 0.08;
        let axis_threshold = camera.distance * 0.015;
        let plane_threshold = camera.distance * 0.02;

        // Check plane handles first (smaller targets, higher priority)
        for axis in &[GizmoAxis::XY, GizmoAxis::XZ, GizmoAxis::YZ] {
            let center = self.plane_handle_center(entity_world_pos, gizmo_scale, *axis);
            let dist = ray_point_distance(ray_o, ray_d, center);
            if dist < plane_threshold {
                return Some(*axis);
            }
        }

        // Check single-axis arrows
        let mut best: Option<(GizmoAxis, f32)> = None;
        for axis in &[GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
            let dir = axis.axis_dir().unwrap();
            let tip = [
                entity_world_pos[0] + dir[0] * gizmo_scale,
                entity_world_pos[1] + dir[1] * gizmo_scale,
                entity_world_pos[2] + dir[2] * gizmo_scale,
            ];
            // Test distance from ray to the line segment (origin → tip)
            let dist = ray_segment_distance(ray_o, ray_d, entity_world_pos, tip);
            if dist < axis_threshold {
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((*axis, dist));
                }
            }
        }

        best.map(|(a, _)| a)
    }

    /// Begin a drag operation on the given axis.
    pub fn begin_drag(
        &mut self,
        axis: GizmoAxis,
        camera: &Camera,
        screen_size: [f32; 2],
        mx: f32,
        my: f32,
        entity_world_pos: [f32; 3],
    ) {
        self.active_axis = Some(axis);
        self.dragging = true;
        self.drag_start_world_pos = entity_world_pos;

        // Compute initial plane hit for delta calculation
        let (ray_o, ray_d) = screen_to_world_ray(camera, screen_size, mx, my);
        let (plane_n, plane_d) = self.drag_plane(axis, entity_world_pos, camera);
        self.drag_plane_hit = ray_plane_intersect(ray_o, ray_d, plane_n, plane_d)
            .unwrap_or(entity_world_pos);
    }

    /// Handle ongoing drag — returns new world position for the entity if changed.
    pub fn handle_drag(
        &self,
        camera: &Camera,
        screen_size: [f32; 2],
        mx: f32,
        my: f32,
    ) -> Option<[f32; 3]> {
        let axis = self.active_axis?;
        if !self.dragging {
            return None;
        }

        let (ray_o, ray_d) = screen_to_world_ray(camera, screen_size, mx, my);
        let (plane_n, plane_d) = self.drag_plane(axis, self.drag_start_world_pos, camera);

        let hit = ray_plane_intersect(ray_o, ray_d, plane_n, plane_d)?;

        // Delta from initial hit
        let delta = [
            hit[0] - self.drag_plane_hit[0],
            hit[1] - self.drag_plane_hit[1],
            hit[2] - self.drag_plane_hit[2],
        ];

        // Apply constraint
        let constrained = match axis {
            GizmoAxis::X => [delta[0], 0.0, 0.0],
            GizmoAxis::Y => [0.0, delta[1], 0.0],
            GizmoAxis::Z => [0.0, 0.0, delta[2]],
            GizmoAxis::XY => [delta[0], delta[1], 0.0],
            GizmoAxis::XZ => [delta[0], 0.0, delta[2]],
            GizmoAxis::YZ => [0.0, delta[1], delta[2]],
        };

        Some([
            self.drag_start_world_pos[0] + constrained[0],
            self.drag_start_world_pos[1] + constrained[1],
            self.drag_start_world_pos[2] + constrained[2],
        ])
    }

    /// Finalize a drag, pushing an undo entry.
    pub fn end_drag(&mut self, entity_id: u64, name: &str, new_position: [f32; 3]) {
        if self.dragging {
            self.undo_stack.push(UndoEntry {
                entity_id,
                entity_name: name.to_string(),
                old_position: self.drag_start_world_pos,
                new_position,
            });
            self.redo_stack.clear();
            self.modified = true;
        }
        self.dragging = false;
        self.active_axis = None;
    }

    /// Cancel an in-progress drag, returning the original position.
    pub fn cancel_drag(&mut self) -> Option<[f32; 3]> {
        if self.dragging {
            self.dragging = false;
            self.active_axis = None;
            return Some(self.drag_start_world_pos);
        }
        None
    }

    /// Undo the last transform change. Returns the entry to apply.
    pub fn undo(&mut self) -> Option<UndoEntry> {
        let entry = self.undo_stack.pop()?;
        self.redo_stack.push(entry.clone());
        self.modified = !self.undo_stack.is_empty();
        Some(entry)
    }

    /// Redo a previously undone change. Returns the entry to apply.
    pub fn redo(&mut self) -> Option<UndoEntry> {
        let entry = self.redo_stack.pop()?;
        self.undo_stack.push(entry.clone());
        self.modified = true;
        Some(entry)
    }

    /// Push an undo entry from an external source (e.g. inspector DragValue).
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_stack.push(entry);
        self.redo_stack.clear();
        self.modified = true;
    }

    /// Update hovered axis from mouse position.
    pub fn update_hover(
        &mut self,
        camera: &Camera,
        screen_size: [f32; 2],
        mx: f32,
        my: f32,
        entity_world_pos: [f32; 3],
    ) {
        if self.dragging {
            return;
        }
        self.hovered_axis = self.pick(camera, screen_size, mx, my, entity_world_pos);
    }

    /// Draw the gizmo overlay on the egui painter.
    pub fn draw_overlay(
        &self,
        painter: &egui::Painter,
        camera: &Camera,
        screen_size: [f32; 2],
        entity_world_pos: [f32; 3],
    ) {
        let gizmo_scale = camera.distance * 0.08;

        let center_screen = match world_to_screen(camera, screen_size, entity_world_pos) {
            Some(p) => p,
            None => return,
        };

        // Draw plane handles first (behind arrows)
        for axis in &[GizmoAxis::XY, GizmoAxis::XZ, GizmoAxis::YZ] {
            self.draw_plane_handle(painter, camera, screen_size, entity_world_pos, gizmo_scale, *axis);
        }

        // Draw axis arrows sorted by depth (far first)
        let axes = [
            (GizmoAxis::X, [1.0f32, 0.0, 0.0], X_COLOR, X_BRIGHT),
            (GizmoAxis::Y, [0.0, 1.0, 0.0], Y_COLOR, Y_BRIGHT),
            (GizmoAxis::Z, [0.0, 0.0, 1.0], Z_COLOR, Z_BRIGHT),
        ];

        let mut sorted: Vec<_> = axes.iter().collect();
        sorted.sort_by(|a, b| {
            let da = point_depth(camera, [
                entity_world_pos[0] + a.1[0] * gizmo_scale,
                entity_world_pos[1] + a.1[1] * gizmo_scale,
                entity_world_pos[2] + a.1[2] * gizmo_scale,
            ]);
            let db = point_depth(camera, [
                entity_world_pos[0] + b.1[0] * gizmo_scale,
                entity_world_pos[1] + b.1[1] * gizmo_scale,
                entity_world_pos[2] + b.1[2] * gizmo_scale,
            ]);
            // Draw far first (lowest depth = farthest)
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

        for (axis, dir, color, bright) in sorted {
            let tip_world = [
                entity_world_pos[0] + dir[0] * gizmo_scale,
                entity_world_pos[1] + dir[1] * gizmo_scale,
                entity_world_pos[2] + dir[2] * gizmo_scale,
            ];

            let tip_screen = match world_to_screen(camera, screen_size, tip_world) {
                Some(p) => p,
                None => continue,
            };

            let is_hovered = self.hovered_axis == Some(*axis);
            let is_active = self.active_axis == Some(*axis);
            let is_other_active = self.active_axis.is_some() && !is_active;

            let line_color = if is_active {
                *bright
            } else if is_hovered {
                *bright
            } else if is_other_active {
                color.gamma_multiply(DIM_ALPHA)
            } else {
                *color
            };

            let thickness = if is_active || is_hovered { 3.5 } else { 2.5 };

            // Axis line
            painter.line_segment(
                [center_screen, tip_screen],
                egui::Stroke::new(thickness, line_color),
            );

            // Arrow tip triangle
            let arrow_dir = egui::vec2(
                tip_screen.x - center_screen.x,
                tip_screen.y - center_screen.y,
            );
            let arrow_len = arrow_dir.length();
            if arrow_len > 1.0 {
                let norm = arrow_dir / arrow_len;
                let perp = egui::vec2(-norm.y, norm.x);
                let tip_size = if is_hovered || is_active { 10.0 } else { 8.0 };
                let base = tip_screen - norm * tip_size;
                let left = base + perp * tip_size * 0.4;
                let right = base - perp * tip_size * 0.4;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip_screen, left, right],
                    line_color,
                    egui::Stroke::NONE,
                ));
            }
        }

        // Center dot
        painter.circle_filled(center_screen, 3.0, egui::Color32::WHITE);
    }

    // --- Private helpers ---

    fn draw_plane_handle(
        &self,
        painter: &egui::Painter,
        camera: &Camera,
        screen_size: [f32; 2],
        entity_world_pos: [f32; 3],
        gizmo_scale: f32,
        axis: GizmoAxis,
    ) {
        let (dir_a, dir_b, color) = match axis {
            GizmoAxis::XY => ([1.0f32, 0.0, 0.0], [0.0f32, 1.0, 0.0], Z_COLOR),
            GizmoAxis::XZ => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Y_COLOR),
            GizmoAxis::YZ => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], X_COLOR),
            _ => return,
        };

        let is_hovered = self.hovered_axis == Some(axis);
        let is_active = self.active_axis == Some(axis);
        let is_other_active = self.active_axis.is_some() && !is_active;

        let offset = gizmo_scale * 0.3;

        // Four corners of the small square
        let corners_world: [[f32; 3]; 4] = [
            [
                entity_world_pos[0] + dir_a[0] * offset * 0.4 + dir_b[0] * offset * 0.4,
                entity_world_pos[1] + dir_a[1] * offset * 0.4 + dir_b[1] * offset * 0.4,
                entity_world_pos[2] + dir_a[2] * offset * 0.4 + dir_b[2] * offset * 0.4,
            ],
            [
                entity_world_pos[0] + dir_a[0] * offset + dir_b[0] * offset * 0.4,
                entity_world_pos[1] + dir_a[1] * offset + dir_b[1] * offset * 0.4,
                entity_world_pos[2] + dir_a[2] * offset + dir_b[2] * offset * 0.4,
            ],
            [
                entity_world_pos[0] + dir_a[0] * offset + dir_b[0] * offset,
                entity_world_pos[1] + dir_a[1] * offset + dir_b[1] * offset,
                entity_world_pos[2] + dir_a[2] * offset + dir_b[2] * offset,
            ],
            [
                entity_world_pos[0] + dir_a[0] * offset * 0.4 + dir_b[0] * offset,
                entity_world_pos[1] + dir_a[1] * offset * 0.4 + dir_b[1] * offset,
                entity_world_pos[2] + dir_a[2] * offset * 0.4 + dir_b[2] * offset,
            ],
        ];

        let corners_screen: Vec<egui::Pos2> = corners_world
            .iter()
            .filter_map(|c| world_to_screen(camera, screen_size, *c))
            .collect();

        if corners_screen.len() != 4 {
            return;
        }

        let alpha = if is_active || is_hovered {
            PLANE_HOVER_ALPHA
        } else if is_other_active {
            15
        } else {
            PLANE_ALPHA
        };

        let fill = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
        let stroke_color = if is_active || is_hovered {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 200)
        } else if is_other_active {
            egui::Color32::TRANSPARENT
        } else {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 100)
        };

        painter.add(egui::Shape::convex_polygon(
            corners_screen.clone(),
            fill,
            egui::Stroke::new(1.0, stroke_color),
        ));
    }

    fn plane_handle_center(&self, entity_world_pos: [f32; 3], gizmo_scale: f32, axis: GizmoAxis) -> [f32; 3] {
        let (dir_a, dir_b) = match axis {
            GizmoAxis::XY => ([1.0f32, 0.0, 0.0], [0.0f32, 1.0, 0.0]),
            GizmoAxis::XZ => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            GizmoAxis::YZ => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            _ => return entity_world_pos,
        };
        let offset = gizmo_scale * 0.3 * 0.7; // center of the square
        [
            entity_world_pos[0] + dir_a[0] * offset + dir_b[0] * offset,
            entity_world_pos[1] + dir_a[1] * offset + dir_b[1] * offset,
            entity_world_pos[2] + dir_a[2] * offset + dir_b[2] * offset,
        ]
    }

    /// Choose the constraint plane for dragging on the given axis.
    /// Returns (plane_normal, plane_d) where plane_d = dot(normal, point_on_plane).
    fn drag_plane(
        &self,
        axis: GizmoAxis,
        origin: [f32; 3],
        camera: &Camera,
    ) -> ([f32; 3], f32) {
        let normal = match axis {
            GizmoAxis::XY => [0.0, 0.0, 1.0],
            GizmoAxis::XZ => [0.0, 1.0, 0.0],
            GizmoAxis::YZ => [1.0, 0.0, 0.0],
            GizmoAxis::X | GizmoAxis::Y | GizmoAxis::Z => {
                // For single-axis drags, pick the plane most perpendicular to the camera
                let cam_fwd = camera.forward_vector();
                let axis_dir = axis.axis_dir().unwrap();

                // The constraint plane should contain the axis direction.
                // Choose the plane whose normal is most perpendicular to both
                // the axis and the camera forward.
                // We want a plane containing the axis — so normal must be perpendicular to axis.
                // Among such normals, pick the one most aligned with camera forward.
                let candidates = match axis {
                    GizmoAxis::X => [[0.0f32, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    GizmoAxis::Y => [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                    GizmoAxis::Z => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    _ => unreachable!(),
                };
                let _ = axis_dir; // used for matching above

                // Pick the candidate whose dot product with camera forward is largest
                let dot0 = (candidates[0][0] * cam_fwd[0]
                    + candidates[0][1] * cam_fwd[1]
                    + candidates[0][2] * cam_fwd[2])
                    .abs();
                let dot1 = (candidates[1][0] * cam_fwd[0]
                    + candidates[1][1] * cam_fwd[1]
                    + candidates[1][2] * cam_fwd[2])
                    .abs();

                if dot0 >= dot1 {
                    candidates[0]
                } else {
                    candidates[1]
                }
            }
        };

        let d = normal[0] * origin[0] + normal[1] * origin[1] + normal[2] * origin[2];
        (normal, d)
    }
}

/// Distance from a ray to a line segment (for axis picking).
fn ray_segment_distance(
    ray_o: [f32; 3],
    ray_d: [f32; 3],
    seg_a: [f32; 3],
    seg_b: [f32; 3],
) -> f32 {
    // Sample several points along the segment and find minimum distance
    let n = 10;
    let mut min_dist = f32::MAX;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let p = [
            seg_a[0] + (seg_b[0] - seg_a[0]) * t,
            seg_a[1] + (seg_b[1] - seg_a[1]) * t,
            seg_a[2] + (seg_b[2] - seg_a[2]) * t,
        ];
        let d = ray_point_distance(ray_o, ray_d, p);
        if d < min_dist {
            min_dist = d;
        }
    }
    min_dist
}
