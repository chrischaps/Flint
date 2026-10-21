//! Projecting with the frame's camera from native code (ADR 0087).
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
//!
//! A game whose ground isn't flat registers it with [`Projector::set_ground`]
//! (ADR 0095): a [`GroundField`], a heightmap placed in the world. Then
//! [`Projector::screen_to_heightfield`] finds the ground under a screen
//! point on a hill, where `screen_to_ground` would cut a plane at the wrong
//! height, and [`Projector::ground_height`] reads it anywhere. Scripts get
//! the same as `screen_to_heightfield(x, y)` and `ground_height(x, z)`.
//!
//! What the renderer draws as ray targets (ADR 0105) is here too:
//! [`Projector::ray_scene`] to cast along any line, and
//! [`Projector::screen_to_mesh`] for the drawn thing under a screen point.

use std::sync::{Arc, Mutex};

pub use flint_core::ray_mesh::{RayHit, RayScene};
pub use flint_terrain::GroundField;

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

    /// Register the ground scripts and [`Projector::screen_to_heightfield`]
    /// pick against, or clear it with `None`. Call again to replace it (a
    /// field built off the main thread, say). It's the game's: it survives
    /// scene changes. Takes the script context's lock, as [`Projector::view`]
    /// does.
    pub fn set_ground(&self, ground: Option<Arc<GroundField>>) {
        crate::lock_or_recover(&self.ctx).ground = ground;
    }

    /// The ground [`Projector::set_ground`] registered.
    pub fn ground(&self) -> Option<Arc<GroundField>> {
        crate::lock_or_recover(&self.ctx).ground.clone()
    }

    /// Where the ray through the screen point (x, y) first meets the
    /// registered ground, as a world point. `None` with no ground
    /// registered, or where the ray misses it: the sky, or past its edge.
    /// One lock, then the march runs without it.
    pub fn screen_to_heightfield(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        let (view, ground) = {
            let c = crate::lock_or_recover(&self.ctx);
            (View::of(&c), c.ground.clone())
        };
        view.screen_to_heightfield(x, y, ground.as_deref()?)
    }

    /// World y of the registered ground at (x, z). `None` with no ground
    /// registered, or off its edge.
    pub fn ground_height(&self, x: f32, z: f32) -> Option<f32> {
        self.ground()?.height_at(x, z)
    }

    /// The meshes drawn as ray targets, where the last frame drew them
    /// (ADR 0105): cast through it from native code, as scripts'
    /// `raycast_meshes` does. Take it once and cast as often as you like.
    pub fn ray_scene(&self) -> Arc<RayScene> {
        crate::lock_or_recover(&self.ctx).ray_scene.clone()
    }

    /// The nearest ray target drawn under the screen point (x, y): picking
    /// against what's drawn rather than the ground (ADR 0105). `None` where
    /// the ray meets none.
    pub fn screen_to_mesh(&self, x: f32, y: f32) -> Option<RayHit> {
        let (view, scene) = {
            let c = crate::lock_or_recover(&self.ctx);
            (View::of(&c), c.ray_scene.clone())
        };
        let (origin, dir) = view.screen_ray(x, y)?;
        scene.cast(origin, dir, f32::INFINITY)
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

    /// The world ray through the screen point (x, y), as `(origin,
    /// direction)`: origin on the near plane, direction unit length.
    /// `None` for an empty screen or a singular camera.
    pub fn screen_ray(&self, x: f32, y: f32) -> Option<([f32; 3], [f32; 3])> {
        if self.width.is_nan() || self.height.is_nan() || self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let ndc = flint_core::projection::screen_to_ndc(x, y, self.width, self.height);
        flint_core::projection::unproject_ray(&self.view_proj, ndc)
    }

    /// Where the ray through the screen point (x, y) first meets `ground`,
    /// as a world point (see [`GroundField::ray`]).
    pub fn screen_to_heightfield(&self, x: f32, y: f32, ground: &GroundField) -> Option<[f32; 3]> {
        let (origin, dir) = self.screen_ray(x, y)?;
        ground.ray(origin, dir)
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
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
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
        let p = system()
            .projector()
            .world_to_screen([0.0, 0.0, 0.0])
            .unwrap();
        assert!(
            (p.x - 640.0).abs() < 1e-3 && (p.y - 360.0).abs() < 1e-3,
            "{p:?}"
        );
        assert!((p.depth - 500f32.sqrt()).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn behind_the_camera_is_none() {
        assert_eq!(
            system().projector().world_to_screen([0.0, 20.0, 40.0]),
            None
        );
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
        s.rhai_engine_mut()
            .register_fn("native_project", move |x: f64, y: f64, z: f64| {
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

    /// A slope rising towards -z, y = 2 - z / 2, over x and z in -20..20,
    /// sampled every 0.5.
    fn slope() -> std::sync::Arc<super::GroundField> {
        let texel = 0.5f32;
        let n = (40.0 / texel) as u32 + 1;
        let mut heights = Vec::with_capacity((n * n) as usize);
        for row in 0..n {
            let z = -20.0 + row as f32 * texel;
            for _col in 0..n {
                heights.push(2.0 - z / 2.0);
            }
        }
        std::sync::Arc::new(super::GroundField::new(
            flint_terrain::Heightmap::from_raw(heights, n, n),
            [-20.0, -20.0],
            [40.0, 40.0],
            1.0,
            0.0,
        ))
    }

    #[test]
    fn a_click_on_a_slope_lands_within_a_texel_of_the_true_point() {
        let s = system();
        let projector = s.projector();
        projector.set_ground(Some(slope()));
        let view = projector.view();
        for (x, z) in [
            (0.0f32, 0.0f32),
            (4.0, -3.0),
            (-6.5, 2.5),
            (8.0, 6.0),
            (-3.0, -9.0),
        ] {
            let truth = [x, 2.0 - z / 2.0, z];
            let p = view.world_to_screen(truth).unwrap();
            let hit = projector.screen_to_heightfield(p.x, p.y).unwrap();
            let err = (hit[0] - truth[0]).hypot(hit[2] - truth[2]);
            assert!(err < 0.5, "{truth:?} -> {hit:?}");
            // A flat pick at the height of the ground under the camera's
            // target is a long way off on the slope: what the field is for.
            if z.abs() > 2.0 {
                let flat = view.screen_to_ground(p.x, p.y, 2.0).unwrap();
                assert!((flat[1] - z).abs() > 1.0, "{z} {flat:?}");
            }
        }
        assert_eq!(projector.ground_height(4.0, -3.0), Some(3.5));
        assert_eq!(projector.ground_height(40.0, 0.0), None);
    }

    #[test]
    fn without_a_ground_nothing_is_picked() {
        let projector = system().projector();
        assert!(projector.screen_to_heightfield(640.0, 360.0).is_none());
        assert!(projector.ground_height(0.0, 0.0).is_none());
        projector.set_ground(Some(slope()));
        assert!(projector.screen_to_heightfield(640.0, 360.0).is_some());
        projector.set_ground(None);
        assert!(projector.screen_to_heightfield(640.0, 360.0).is_none());
    }

    #[test]
    fn scripts_pick_the_same_ground() {
        let mut s = system();
        let projector = s.projector();
        projector.set_ground(Some(slope()));
        let p = projector.view().world_to_screen([4.0, 3.5, -3.0]).unwrap();
        let rust = projector.screen_to_heightfield(p.x, p.y).unwrap();
        let script = format!(
            "let g = screen_to_heightfield({:?}, {:?}); [g[0], g[1], g[2], ground_height(4.0, -3.0)]",
            p.x as f64, p.y as f64
        );
        let engine = s.rhai_engine_mut();
        let out: Vec<f64> = engine
            .eval::<rhai::Array>(&script)
            .unwrap()
            .into_iter()
            .map(|v| v.as_float().unwrap())
            .collect();
        assert_eq!(out[..3], [rust[0] as f64, rust[1] as f64, rust[2] as f64]);
        assert_eq!(out[3], 3.5);
        assert!(engine
            .eval::<rhai::Dynamic>("ground_height(100.0, 0.0)")
            .unwrap()
            .is_unit());
        assert!(engine
            .eval::<rhai::Dynamic>("screen_to_heightfield(640.0, -5000.0)")
            .unwrap()
            .is_unit());
    }

    #[test]
    fn screen_to_ground_inverts_world_to_screen() {
        let view = system().projector().view();
        let p = view.world_to_screen([4.0, 0.0, -3.0]).unwrap();
        let [x, z] = view.screen_to_ground(p.x, p.y, 0.0).unwrap();
        assert!((x - 4.0).abs() < 1e-3 && (z + 3.0).abs() < 1e-3, "{x} {z}");
    }
    /// A 2 m box standing on the origin, as a scene of one ray target.
    fn a_box() -> std::sync::Arc<super::RayScene> {
        let p: Vec<[f32; 3]> = (0..8)
            .map(|k| {
                [
                    if k & 1 == 0 { -1.0 } else { 1.0 },
                    if k & 2 == 0 { 0.0 } else { 2.0 },
                    if k & 4 == 0 { -1.0 } else { 1.0 },
                ]
            })
            .collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let i: Vec<u32> = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
        let mut scene = super::RayScene::new();
        let mesh = std::sync::Arc::new(flint_core::ray_mesh::RayMesh::new(&p, &i));
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        scene.add(flint_core::EntityId::from_raw(42), mesh, identity);
        std::sync::Arc::new(scene)
    }

    #[test]
    fn a_click_picks_the_drawn_thing_under_it() {
        let mut s = system();
        let projector = s.projector();
        assert!(projector.screen_to_mesh(640.0, 360.0).is_none(), "no scene yet");
        s.set_ray_scene(a_box());
        // The camera looks at the origin from (0, 10, 20), so mid-screen
        // meets the box's top front edge region: its front face or its top.
        let hit = projector.screen_to_mesh(640.0, 360.0).unwrap();
        assert_eq!(hit.entity.raw(), 42);
        let back = projector.view().world_to_screen(hit.point).unwrap();
        assert!((back.x - 640.0).abs() < 0.01 && (back.y - 360.0).abs() < 0.01, "{back:?}");
        // The top, (0, 2, 0), is met at its own point, facing up.
        let top = projector.view().world_to_screen([0.0, 2.0, 0.0]).unwrap();
        let hit = projector.screen_to_mesh(top.x, top.y).unwrap();
        assert!((hit.point[1] - 2.0).abs() < 1e-3 && hit.normal[1] > 0.99, "{hit:?}");
        // Off to the side the ray misses.
        assert!(projector.screen_to_mesh(10.0, 10.0).is_none());
    }

    #[test]
    fn scripts_cast_through_the_same_scene() {
        let mut s = system();
        s.set_ray_scene(a_box());
        let engine = s.rhai_engine_mut();
        let out: rhai::Map = engine
            .eval("raycast_meshes(-5.0, 1.0, 0.0, 1.0, 0.0, 0.0, 100.0)")
            .unwrap();
        assert_eq!(out["entity"].as_int().unwrap(), 42);
        assert_eq!(out["distance"].as_float().unwrap(), 4.0);
        assert_eq!(out["point_x"].as_float().unwrap(), -1.0);
        assert_eq!(out["normal_x"].as_float().unwrap(), -1.0);
        for missed in [
            "raycast_meshes(-5.0, 1.0, 0.0, 1.0, 0.0, 0.0, 3.0)",
            "raycast_meshes(-5.0, 3.0, 0.0, 1.0, 0.0, 0.0, 100.0)",
            "screen_to_mesh(10.0, 10.0)",
        ] {
            assert!(engine.eval::<rhai::Dynamic>(missed).unwrap().is_unit(), "{missed}");
        }
        let picked: rhai::Map = engine.eval("screen_to_mesh(640.0, 360.0)").unwrap();
        assert_eq!(picked["entity"].as_int().unwrap(), 42);
    }
}
