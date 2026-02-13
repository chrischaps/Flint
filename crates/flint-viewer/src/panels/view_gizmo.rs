//! View orientation gizmo — Blender-style 3D axis widget for snapping to orthographic views
//!
//! Renders a small interactive gizmo in the top-right of the viewport showing the
//! current camera orientation. Click axis endpoints to snap to orthographic views
//! (Front, Back, Left, Right, Top, Bottom). Drag the background to orbit.

use flint_render::Camera;

// Layout
const GIZMO_DIAMETER: f32 = 120.0;
const AXIS_LENGTH: f32 = 36.0;
const POS_CAP_RADIUS: f32 = 13.0;
const NEG_CAP_RADIUS: f32 = 5.5;
const HIT_EXPAND: f32 = 5.0;
const LABEL_HEIGHT: f32 = 16.0;
const MARGIN: f32 = 12.0;

// Axis colors
const X_COLOR: egui::Color32 = egui::Color32::from_rgb(214, 67, 67);
const Y_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 172, 67);
const Z_COLOR: egui::Color32 = egui::Color32::from_rgb(67, 118, 214);
const X_DIM: egui::Color32 = egui::Color32::from_rgb(128, 48, 48);
const Y_DIM: egui::Color32 = egui::Color32::from_rgb(48, 96, 48);
const Z_DIM: egui::Color32 = egui::Color32::from_rgb(48, 72, 128);

// Background
const BG_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(22, 22, 32, 150);
const BG_STROKE_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(65, 65, 85, 90);

/// Precomputed camera data for gizmo rendering (avoids holding a reference to Camera)
pub struct CameraView {
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub orthographic: bool,
}

impl CameraView {
    pub fn from_camera(camera: &Camera) -> Self {
        Self {
            right: camera.right_vector(),
            up: camera.up_vector(),
            forward: camera.forward_vector(),
            yaw: camera.yaw,
            pitch: camera.pitch,
            orthographic: camera.orthographic,
        }
    }
}

/// Action returned by the gizmo for the viewer to process
#[derive(Clone, Copy, Debug)]
pub enum GizmoAction {
    /// Smoothly snap camera to a preset orthographic view
    SnapToView { yaw: f32, pitch: f32 },
    /// Orbit camera by a delta (from gizmo drag)
    OrbitDelta { dyaw: f32, dpitch: f32 },
    /// Switch back to perspective projection (clicked gizmo background)
    SwitchToPerspective,
}

#[derive(Clone, Copy, PartialEq)]
enum AxisId {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

struct AxisDef {
    dir: [f32; 3],
    label: &'static str,
    color: egui::Color32,
    id: AxisId,
    positive: bool,
}

/// Blender-style view orientation gizmo
pub struct ViewGizmo {
    drag_started_on_axis: bool,
}

impl ViewGizmo {
    pub fn new() -> Self {
        Self {
            drag_started_on_axis: false,
        }
    }

    /// Draw the gizmo and return any user interaction
    pub fn draw(&mut self, ctx: &egui::Context, cam: &CameraView) -> Option<GizmoAction> {
        let central_rect = ctx.available_rect();
        let gizmo_pos = egui::pos2(
            central_rect.right() - GIZMO_DIAMETER - MARGIN,
            central_rect.top() + MARGIN,
        );

        let area_resp = egui::Area::new(egui::Id::new("view_gizmo"))
            .fixed_pos(gizmo_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| self.draw_content(ui, cam));

        area_resp.inner
    }

    fn draw_content(&mut self, ui: &mut egui::Ui, cam: &CameraView) -> Option<GizmoAction> {
        let total_size = egui::vec2(GIZMO_DIAMETER, GIZMO_DIAMETER + LABEL_HEIGHT);
        let (response, painter) =
            ui.allocate_painter(total_size, egui::Sense::click_and_drag());

        let center = egui::pos2(
            response.rect.center().x,
            response.rect.top() + GIZMO_DIAMETER / 2.0,
        );
        let bg_radius = GIZMO_DIAMETER / 2.0 - 2.0;

        // Background circle with subtle border
        painter.circle_filled(center, bg_radius, BG_FILL);
        painter.circle_stroke(
            center,
            bg_radius,
            egui::Stroke::new(1.0, BG_STROKE_COLOR),
        );

        // Define all six axis endpoints
        let axes = [
            AxisDef { dir: [1.0, 0.0, 0.0], label: "X", color: X_COLOR, id: AxisId::PosX, positive: true },
            AxisDef { dir: [-1.0, 0.0, 0.0], label: "", color: X_DIM, id: AxisId::NegX, positive: false },
            AxisDef { dir: [0.0, 1.0, 0.0], label: "Y", color: Y_COLOR, id: AxisId::PosY, positive: true },
            AxisDef { dir: [0.0, -1.0, 0.0], label: "", color: Y_DIM, id: AxisId::NegY, positive: false },
            AxisDef { dir: [0.0, 0.0, 1.0], label: "Z", color: Z_COLOR, id: AxisId::PosZ, positive: true },
            AxisDef { dir: [0.0, 0.0, -1.0], label: "", color: Z_DIM, id: AxisId::NegZ, positive: false },
        ];

        // Sort back-to-front by depth (draw farthest first so closest is on top)
        let mut sorted: Vec<&AxisDef> = axes.iter().collect();
        sorted.sort_by(|a, b| {
            let da = Self::depth(cam, a.dir);
            let db = Self::depth(cam, b.dir);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Draw axes and detect hover
        let mut hovered_axis: Option<AxisId> = None;
        let pointer_pos = response.hover_pos();

        for axis in &sorted {
            let proj = Self::project(cam, axis.dir);
            let end = center + proj * AXIS_LENGTH;
            let cap_r = if axis.positive { POS_CAP_RADIUS } else { NEG_CAP_RADIUS };

            // Depth-based dimming: axes pointing away from camera are fainter
            let d = Self::depth(cam, axis.dir);
            let alpha = if d > 0.1 { 0.4 } else { 1.0 };

            // Axis line
            let line_color = axis.color.gamma_multiply(0.7 * alpha);
            painter.line_segment([center, end], egui::Stroke::new(2.0, line_color));

            // Hit test
            let is_hovered = pointer_pos
                .map(|p| (p - end).length() < cap_r + HIT_EXPAND)
                .unwrap_or(false);
            if is_hovered {
                hovered_axis = Some(axis.id);
            }

            // Endpoint cap
            let cap_color = axis.color.gamma_multiply(alpha);

            if is_hovered {
                // Soft glow behind hovered cap
                painter.circle_filled(end, cap_r + 4.0, cap_color.gamma_multiply(0.3));
            }

            painter.circle_filled(end, cap_r, cap_color);

            // Subtle specular highlight on positive caps
            if axis.positive {
                painter.circle_filled(
                    end + egui::vec2(-2.0, -2.5),
                    cap_r * 0.35,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, (35.0 * alpha) as u8),
                );
            }

            if is_hovered {
                painter.circle_stroke(
                    end,
                    cap_r + 1.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                );
            }

            // Letter label on positive caps
            if !axis.label.is_empty() {
                painter.text(
                    end,
                    egui::Align2::CENTER_CENTER,
                    axis.label,
                    egui::FontId::new(11.0, egui::FontFamily::Proportional),
                    egui::Color32::from_rgba_unmultiplied(
                        255,
                        255,
                        255,
                        (230.0 * alpha) as u8,
                    ),
                );
            }
        }

        // View name label below the gizmo circle
        let view_name = Self::current_view_name(cam);
        painter.text(
            egui::pos2(center.x, response.rect.top() + GIZMO_DIAMETER + 2.0),
            egui::Align2::CENTER_TOP,
            view_name,
            egui::FontId::new(10.0, egui::FontFamily::Proportional),
            egui::Color32::from_rgb(150, 150, 165),
        );

        // Cursor feedback
        if hovered_axis.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Track whether drag started on an axis cap (to avoid accidental orbit)
        if response.drag_started() {
            self.drag_started_on_axis = hovered_axis.is_some();
        }

        // Interaction
        let mut action = None;

        if response.clicked() {
            if let Some(axis_id) = hovered_axis {
                action = Some(Self::snap_for_axis(axis_id));
            } else if cam.orthographic {
                // Clicked gizmo background while in ortho → switch back to perspective
                action = Some(GizmoAction::SwitchToPerspective);
            }
        } else if response.dragged() && !self.drag_started_on_axis {
            let delta = response.drag_delta();
            if delta.length() > 0.0 {
                action = Some(GizmoAction::OrbitDelta {
                    dyaw: -delta.x * 0.01,
                    dpitch: -delta.y * 0.01,
                });
            }
        }

        action
    }

    /// Project a 3D world direction onto 2D gizmo coordinates
    fn project(cam: &CameraView, dir: [f32; 3]) -> egui::Vec2 {
        let x = dir[0] * cam.right[0] + dir[1] * cam.right[1] + dir[2] * cam.right[2];
        let y = dir[0] * cam.up[0] + dir[1] * cam.up[1] + dir[2] * cam.up[2];
        egui::vec2(x, -y) // negate Y because screen Y points down
    }

    /// Depth of a direction relative to camera (positive = pointing away = behind)
    fn depth(cam: &CameraView, dir: [f32; 3]) -> f32 {
        dir[0] * cam.forward[0] + dir[1] * cam.forward[1] + dir[2] * cam.forward[2]
    }

    /// Map an axis click to a preset camera orientation
    fn snap_for_axis(axis: AxisId) -> GizmoAction {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (yaw, pitch) = match axis {
            AxisId::PosX => (FRAC_PI_2, 0.0),  // Right
            AxisId::NegX => (-FRAC_PI_2, 0.0),  // Left
            AxisId::PosY => (0.0, 1.55),        // Top (near π/2, avoids gimbal lock)
            AxisId::NegY => (0.0, -1.55),       // Bottom
            AxisId::PosZ => (0.0, 0.0),         // Front
            AxisId::NegZ => (PI, 0.0),          // Back
        };
        GizmoAction::SnapToView { yaw, pitch }
    }

    /// Detect current view name including projection mode
    fn current_view_name(cam: &CameraView) -> &'static str {
        use std::f32::consts::{FRAC_PI_2, PI};
        let eps = 0.08;

        let named = if (cam.pitch - 1.55).abs() < eps {
            Some("Top")
        } else if (cam.pitch + 1.55).abs() < eps {
            Some("Bottom")
        } else if cam.pitch.abs() < eps {
            let yaw = normalize_angle(cam.yaw);
            if yaw.abs() < eps {
                Some("Front")
            } else if (yaw - PI).abs() < eps || (yaw + PI).abs() < eps {
                Some("Back")
            } else if (yaw - FRAC_PI_2).abs() < eps {
                Some("Right")
            } else if (yaw + FRAC_PI_2).abs() < eps {
                Some("Left")
            } else {
                None
            }
        } else {
            None
        };

        match (named, cam.orthographic) {
            (Some("Front"), true) => "Front Ortho",
            (Some("Back"), true) => "Back Ortho",
            (Some("Right"), true) => "Right Ortho",
            (Some("Left"), true) => "Left Ortho",
            (Some("Top"), true) => "Top Ortho",
            (Some("Bottom"), true) => "Bottom Ortho",
            (Some(name), false) => name,
            (None, true) => "User Ortho",
            (None, false) => "Perspective",
            _ => "Perspective",
        }
    }
}

fn normalize_angle(a: f32) -> f32 {
    use std::f32::consts::PI;
    let mut a = a % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    }
    if a < -PI {
        a += 2.0 * PI;
    }
    a
}
