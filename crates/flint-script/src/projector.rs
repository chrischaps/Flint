//! Projecting with the frame's camera from native code (ADR 0086).
//!
//! `world_to_screen` and `screen_to_ground` read the camera the host hands
//! the script system each frame, which is the one the frame renders with by
//! the time `on_draw_ui` runs (ADR 0075). That camera lives in the script
//! context, which is private to `flint-script`, so a native function a game
//! registers had no way to project with it. [`Projector`] is a cloneable
//! handle on it: take one from `ScriptSystem::projector` (or
//! `PlayerApp::projector`), move it into a registered closure, and call
//! [`Projector::view`] inside the closure.
//!
//! Called from a script function, the view is the same camera the script's
//! own `world_to_screen` sees, so the two agree to the bit. Called from
//! outside a script call (an extension's `pre_update`, say), it's the
//! camera the last update handed over: last frame's, like
//! `ExtensionCtx::camera`.

use std::sync::{Arc, Mutex};

use crate::context::ScriptCallContext;

/// A cloneable handle on the camera scripts project with. `Send + Sync`,
/// so it can be captured by functions registered on the Rhai engine.
#[derive(Clone)]
pub struct Projector {
    ctx: Arc<Mutex<ScriptCallContext>>,
}

impl Projector {
    pub(crate) fn new(ctx: Arc<Mutex<ScriptCallContext>>) -> Self {
        Self { ctx }
    }

    /// The camera and screen as they stand now. One lock; project as many
    /// points as you like with the copy.
    ///
    /// Takes the script context's lock, so don't call it from code the
    /// script system calls while holding that lock (the host's text
    /// measurer or terrain height callback). Native functions called from
    /// scripts are fine: the lock isn't held while they run.
    pub fn view(&self) -> View {
        View::of(&crate::lock_or_recover(&self.ctx))
    }

    /// `view().world_to_screen(p)`, for a single point.
    pub fn world_to_screen(&self, p: [f32; 3]) -> Option<ScreenPoint> {
        self.view().world_to_screen(p)
    }
}

/// A snapshot of the scripts' camera: its view-projection (column-major,
/// wgpu clip space) and the screen size in logical points, the space of
/// `screen_width()`, `mouse_x()` and the `draw_*` functions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub view_proj: [[f32; 4]; 4],
    pub width: f32,
    pub height: f32,
}

/// Where a world point lands on the screen: logical points from the top
/// left, y down, and its depth (clip w: distance along the view, in world
/// units, for a perspective camera).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f32,
    pub y: f32,
    pub depth: f32,
}

impl View {
    pub(crate) fn of(c: &ScriptCallContext) -> Self {
        View {
            view_proj: c.camera_view_proj,
            width: c.screen_width,
            height: c.screen_height,
        }
    }

    /// What Rhai's `world_to_screen` answers: `None` where it says
    /// `visible: false`, behind the near plane. A point off the edge of
    /// the screen but in front of the camera is `Some`, outside
    /// `0..width` / `0..height`.
    pub fn world_to_screen(&self, p: [f32; 3]) -> Option<ScreenPoint> {
        let clip = project_clip(&self.view_proj, p);
        if clip[3] <= 1e-4 {
            return None;
        }
        let (x, y) = clip_to_screen(clip, self.width, self.height);
        Some(ScreenPoint {
            x,
            y,
            depth: clip[3],
        })
    }

    /// What Rhai's `screen_to_ground` answers: where the ray through the
    /// screen point (x, y) meets the plane y = `plane_y`, as `[x, z]`.
    pub fn screen_to_ground(&self, x: f32, y: f32, plane_y: f32) -> Option<[f32; 2]> {
        flint_core::projection::screen_to_ground(
            &self.view_proj,
            x,
            y,
            self.width,
            self.height,
            plane_y,
        )
    }

    /// The screen's size in logical points.
    pub fn screen_size(&self) -> [f32; 2] {
        [self.width, self.height]
    }
}

/// World point → clip space with a column-major matrix.
pub(crate) fn project_clip(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (row, o) in out.iter_mut().enumerate() {
        *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
    }
    out
}

/// Clip space → HUD pixels (origin top-left, y down).
pub(crate) fn clip_to_screen(clip: [f32; 4], w: f32, h: f32) -> (f32, f32) {
    let inv_w = 1.0 / clip[3];
    let nx = clip[0] * inv_w;
    let ny = clip[1] * inv_w;
    ((nx * 0.5 + 0.5) * w, (1.0 - (ny * 0.5 + 0.5)) * h)
}

#[cfg(test)]
mod tests {
    use crate::ScriptSystem;
    use rhai::Map;

    /// A perspective camera at (0, 10, 20) looking at the origin.
    fn camera() -> [[f32; 4]; 4] {
        let eye = [0.0f32, 10.0, 20.0];
        let f = normalize([-eye[0], -eye[1], -eye[2]]);
        let s = normalize(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(s, f);
        let view = [
            [s[0], u[0], -f[0], 0.0],
            [s[1], u[1], -f[1], 0.0],
            [s[2], u[2], -f[2], 0.0],
            [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
        ];
        // wgpu right-handed perspective, depth 0..1.
        let (fovy, aspect, near, far) = (1.0f32, 16.0 / 9.0, 0.1, 1000.0);
        let h = 1.0 / (fovy * 0.5).tan();
        let r = far / (near - far);
        let proj = [
            [h / aspect, 0.0, 0.0, 0.0],
            [0.0, h, 0.0, 0.0],
            [0.0, 0.0, r, -1.0],
            [0.0, 0.0, r * near, 0.0],
        ];
        let mut out = [[0.0; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (r, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| proj[k][r] * view[c][k]).sum();
            }
        }
        out
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    fn normalize(a: [f32; 3]) -> [f32; 3] {
        let l = dot(a, a).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    }

    fn system() -> ScriptSystem {
        let mut s = ScriptSystem::new();
        s.set_camera_view_proj(camera());
        s.set_screen_size(1280.0, 720.0);
        s
    }

    #[test]
    fn the_target_lands_mid_screen() {
        let p = system().projector().world_to_screen([0.0, 0.0, 0.0]).unwrap();
        assert!((p.x - 640.0).abs() < 1e-3 && (p.y - 360.0).abs() < 1e-3, "{p:?}");
        assert!((p.depth - 500f32.sqrt()).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn behind_the_camera_is_none() {
        assert_eq!(system().projector().world_to_screen([0.0, 20.0, 40.0]), None);
    }

    #[test]
    fn it_follows_the_camera_the_host_sets() {
        let mut s = system();
        let projector = s.projector();
        let before = projector.view();
        s.set_screen_size(1920.0, 1080.0);
        assert_eq!(projector.view().screen_size(), [1920.0, 1080.0]);
        assert_ne!(projector.view(), before);
    }

    #[test]
    fn a_native_function_agrees_with_world_to_screen() {
        let mut s = system();
        let projector = s.projector();
        s.rhai_engine_mut().register_fn("native_project", move |x: f64, y: f64, z: f64| {
            let mut m = Map::new();
            if let Some(p) = projector.world_to_screen([x as f32, y as f32, z as f32]) {
                m.insert("x".into(), (p.x as f64).into());
                m.insert("y".into(), (p.y as f64).into());
                m.insert("depth".into(), (p.depth as f64).into());
            }
            m
        });
        let engine = s.rhai_engine_mut();
        for p in [[0.0f64, 0.0, 0.0], [3.0, 1.5, -4.0], [-12.0, 0.0, 7.5]] {
            let script = format!(
                "let a = world_to_screen({:?}, {:?}, {:?}); let b = native_project({:?}, {:?}, {:?}); \
                 a.x == b.x && a.y == b.y && a.depth == b.depth",
                p[0], p[1], p[2], p[0], p[1], p[2]
            );
            assert!(engine.eval::<bool>(&script).unwrap(), "{p:?}");
        }
    }

    #[test]
    fn screen_to_ground_inverts_world_to_screen() {
        let view = system().projector().view();
        let p = view.world_to_screen([4.0, 0.0, -3.0]).unwrap();
        let [x, z] = view.screen_to_ground(p.x, p.y, 0.0).unwrap();
        assert!((x - 4.0).abs() < 1e-3 && (z + 3.0).abs() < 1e-3, "{x} {z}");
    }
}
