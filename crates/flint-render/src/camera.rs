//! 3D Camera with orbit and first-person controls

pub use flint_core::mat4_mul;
use flint_core::Vec3;

/// Camera operating mode
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CameraMode {
    /// Orbit around a target point (default for scene viewer)
    #[default]
    Orbit,
    /// First-person view at a position looking in yaw/pitch direction
    FirstPerson,
}

/// A 3D camera supporting orbit and first-person modes
pub struct Camera {
    /// Camera position
    pub position: Vec3,
    /// Target point the camera looks at
    pub target: Vec3,
    /// Up vector
    pub up: Vec3,
    /// Field of view in degrees
    pub fov: f32,
    /// Near clipping plane
    pub near: f32,
    /// Far clipping plane
    pub far: f32,
    /// Aspect ratio (width / height)
    pub aspect: f32,

    // Orbit control state
    /// Distance from target
    pub distance: f32,
    /// Horizontal angle in radians
    pub yaw: f32,
    /// Vertical angle in radians
    pub pitch: f32,

    /// Camera operating mode
    pub mode: CameraMode,

    /// Use orthographic projection (true) or perspective (false)
    pub orthographic: bool,

    /// Explicit orthographic half-height in world units.
    /// When > 0, `orthographic_matrix()` uses this directly instead of
    /// deriving from `distance * tan(fov/2)`. Set to 0.0 to use the legacy formula.
    pub ortho_height: f32,

    /// Clip-space (NDC) offset applied after projection, so the look-at point
    /// can land off the window centre — editors use it to centre the target
    /// in the part of the window their side panels leave visible. `[0, 0]`
    /// is a plain projection.
    pub ndc_offset: [f32; 2],
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(10.0, 10.0, 10.0),
            target: Vec3::ZERO,
            up: Vec3::UP,
            fov: 45.0,
            near: 0.1,
            far: 1000.0,
            aspect: 16.0 / 9.0,
            distance: 15.0,
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: std::f32::consts::FRAC_PI_6,
            mode: CameraMode::Orbit,
            orthographic: false,
            ortho_height: 0.0,
            ndc_offset: [0.0, 0.0],
        }
    }
}

impl Camera {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get camera position as an array for GPU upload
    pub fn position_array(&self) -> [f32; 3] {
        [self.position.x, self.position.y, self.position.z]
    }

    /// Update position based on orbit parameters
    pub fn update_orbit(&mut self) {
        let x = self.distance * self.pitch.cos() * self.yaw.sin();
        let y = self.distance * self.pitch.sin();
        let z = self.distance * self.pitch.cos() * self.yaw.cos();

        self.position = Vec3::new(self.target.x + x, self.target.y + y, self.target.z + z);
    }

    /// Orbit horizontally (rotate around target)
    pub fn orbit_horizontal(&mut self, delta: f32) {
        self.yaw += delta;
        self.update_orbit();
    }

    /// Orbit vertically (tilt up/down)
    pub fn orbit_vertical(&mut self, delta: f32) {
        self.pitch += delta;
        // Clamp pitch to avoid gimbal lock (1.56 ≈ 89.4° allows near-orthographic top/bottom views)
        self.pitch = self.pitch.clamp(-1.56, 1.56);
        self.update_orbit();
    }

    /// Zoom in/out
    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance - delta).max(1.0).min(500.0);
        self.update_orbit();
    }

    /// Update for first-person mode: set position directly, compute target from yaw/pitch
    pub fn update_first_person(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        self.mode = CameraMode::FirstPerson;
        self.position = position;
        self.yaw = yaw;
        self.pitch = pitch;

        // Compute forward direction from yaw/pitch
        let forward_x = pitch.cos() * yaw.sin();
        let forward_y = pitch.sin();
        let forward_z = pitch.cos() * yaw.cos();

        self.target = Vec3::new(
            position.x + forward_x,
            position.y + forward_y,
            position.z + forward_z,
        );
    }

    /// Pan the camera (move target)
    pub fn pan(&mut self, dx: f32, dy: f32) {
        // Calculate right and up vectors relative to camera
        let (_, right, up) = self.basis();

        self.target = self.target + right * dx + up * dy;
        self.update_orbit();
    }

    /// The view basis: `(forward, right, up)`, unit length.
    ///
    /// `right = forward × up`. Looking straight along `up` (a top-down camera)
    /// leaves that cross product zero, so the basis falls back to north (-Z)
    /// as the top of the screen: from above, east (+X) is on the right, as on
    /// a map. Without the fallback the view matrix collapses and nothing
    /// renders.
    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let f = (self.target - self.position).normalized();
        let mut s = f.cross(&self.up);
        if s.dot(&s) < 1e-12 {
            s = f.cross(&Vec3::new(0.0, 0.0, -1.0));
            if s.dot(&s) < 1e-12 {
                // `up` itself lies along Z, so -Z is no help either.
                s = f.cross(&Vec3::new(1.0, 0.0, 0.0));
            }
        }
        let s = s.normalized();
        let u = s.cross(&f);
        (f, s, u)
    }

    /// Get the view matrix (4x4, column-major)
    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        let (f, s, u) = self.basis();

        [
            [s.x, u.x, -f.x, 0.0],
            [s.y, u.y, -f.y, 0.0],
            [s.z, u.z, -f.z, 0.0],
            [
                -s.dot(&self.position),
                -u.dot(&self.position),
                f.dot(&self.position),
                1.0,
            ],
        ]
    }

    /// Get the projection matrix (4x4, column-major)
    pub fn projection_matrix(&self) -> [[f32; 4]; 4] {
        let mut m = if self.orthographic {
            self.orthographic_matrix()
        } else {
            self.perspective_matrix()
        };
        // clip.x += ox * clip.w (likewise y): a pure screen-space shift that
        // is valid for both projections because it rides the w row.
        let [ox, oy] = self.ndc_offset;
        if ox != 0.0 || oy != 0.0 {
            for col in m.iter_mut() {
                col[0] += ox * col[3];
                col[1] += oy * col[3];
            }
        }
        m
    }

    fn perspective_matrix(&self) -> [[f32; 4]; 4] {
        let fov_rad = self.fov.to_radians();
        let f = 1.0 / (fov_rad / 2.0).tan();

        let depth = self.far - self.near;

        // wgpu [0,1] depth convention: z_view=-near → 0, z_view=-far → 1
        // (matches orthographic_matrix; ADR 0055 — the former GL [-1,1] rows
        // clipped geometry nearer than 2×near and halved linearized depth)
        [
            [f / self.aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, -self.far / depth, -1.0],
            [0.0, 0.0, -(self.far * self.near) / depth, 0.0],
        ]
    }

    fn orthographic_matrix(&self) -> [[f32; 4]; 4] {
        // When ortho_height is set, use it directly as half-height; otherwise
        // size the volume so objects at `distance` appear the same size as in perspective.
        let half_h = if self.ortho_height > 0.0 {
            self.ortho_height
        } else {
            self.distance * (self.fov.to_radians() / 2.0).tan()
        };
        let half_w = half_h * self.aspect;
        let depth = self.far - self.near;

        // Column-major: m[col][row]
        // Maps depth to [0, 1] (wgpu convention): z_view=-near → 0, z_view=-far → 1
        [
            [1.0 / half_w, 0.0, 0.0, 0.0],
            [0.0, 1.0 / half_h, 0.0, 0.0],
            [0.0, 0.0, -1.0 / depth, 0.0],
            [0.0, 0.0, -self.near / depth, 1.0],
        ]
    }

    /// Get combined view-projection matrix
    pub fn view_projection_matrix(&self) -> [[f32; 4]; 4] {
        let view = self.view_matrix();
        let proj = self.projection_matrix();
        mat4_mul(&proj, &view)
    }

    /// Get camera right vector (world space)
    pub fn right_vector(&self) -> [f32; 3] {
        let (_, s, _) = self.basis();
        [s.x, s.y, s.z]
    }

    /// Get camera up vector (world space, perpendicular to both forward and right)
    pub fn up_vector(&self) -> [f32; 3] {
        let (_, _, u) = self.basis();
        [u.x, u.y, u.z]
    }

    /// Get camera forward direction (world space)
    pub fn forward_vector(&self) -> [f32; 3] {
        let f = (self.target - self.position).normalized();
        [f.x, f.y, f.z]
    }

    /// Get inverse of the combined view-projection matrix (for unprojecting)
    pub fn inverse_view_projection_matrix(&self) -> [[f32; 4]; 4] {
        let vp = self.view_projection_matrix();
        mat4_inverse(&vp)
    }

    /// Get inverse of the projection matrix (for reconstructing view-space positions from depth)
    pub fn inverse_projection_matrix(&self) -> [[f32; 4]; 4] {
        let proj = self.projection_matrix();
        mat4_inverse(&proj)
    }

    /// Unproject a screen-space NDC point into a world-space ray.
    /// `ndc_x` and `ndc_y` are in [-1, 1] (left-to-right, bottom-to-top).
    /// Returns `(origin, direction)` as `[f32; 3]` arrays: the origin is on
    /// the near plane and the direction is unit length. Works for perspective
    /// and orthographic cameras, and honours `ndc_offset`.
    pub fn unproject_ray(&self, ndc_x: f32, ndc_y: f32) -> ([f32; 3], [f32; 3]) {
        flint_core::projection::unproject_ray(&self.view_projection_matrix(), [ndc_x, ndc_y])
            .unwrap_or((self.position_array(), [0.0, -1.0, 0.0]))
    }

    /// The point on the horizontal plane `y = plane_y` under a screen point,
    /// as `[x, z]`, or `None` where the view doesn't reach the plane (above
    /// the horizon, a level orthographic view, a plane behind the camera).
    ///
    /// `(x, y)` has its origin at the viewport's top-left with y down, in the
    /// same unit as `width`/`height`: logical points with the logical size,
    /// or physical pixels with the physical size. Either works on a HiDPI
    /// display, but don't mix them.
    pub fn screen_to_ground(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        plane_y: f32,
    ) -> Option<[f32; 2]> {
        flint_core::projection::screen_to_ground(
            &self.view_projection_matrix(),
            x,
            y,
            width,
            height,
            plane_y,
        )
    }
}

/// Compute the inverse of a 4x4 column-major matrix using cofactor expansion
pub(crate) fn mat4_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Flatten column-major to indexable
    let s = |col: usize, row: usize| -> f32 { m[col][row] };

    let c00 = s(2, 2) * s(3, 3) - s(3, 2) * s(2, 3);
    let c02 = s(1, 2) * s(3, 3) - s(3, 2) * s(1, 3);
    let c03 = s(1, 2) * s(2, 3) - s(2, 2) * s(1, 3);

    let c04 = s(2, 1) * s(3, 3) - s(3, 1) * s(2, 3);
    let c06 = s(1, 1) * s(3, 3) - s(3, 1) * s(1, 3);
    let c07 = s(1, 1) * s(2, 3) - s(2, 1) * s(1, 3);

    let c08 = s(2, 1) * s(3, 2) - s(3, 1) * s(2, 2);
    let c10 = s(1, 1) * s(3, 2) - s(3, 1) * s(1, 2);
    let c11 = s(1, 1) * s(2, 2) - s(2, 1) * s(1, 2);

    let c12 = s(2, 0) * s(3, 3) - s(3, 0) * s(2, 3);
    let c14 = s(1, 0) * s(3, 3) - s(3, 0) * s(1, 3);
    let c15 = s(1, 0) * s(2, 3) - s(2, 0) * s(1, 3);

    let c16 = s(2, 0) * s(3, 2) - s(3, 0) * s(2, 2);
    let c18 = s(1, 0) * s(3, 2) - s(3, 0) * s(1, 2);
    let c19 = s(1, 0) * s(2, 2) - s(2, 0) * s(1, 2);

    let c20 = s(2, 0) * s(3, 1) - s(3, 0) * s(2, 1);
    let c22 = s(1, 0) * s(3, 1) - s(3, 0) * s(1, 1);
    let c23 = s(1, 0) * s(2, 1) - s(2, 0) * s(1, 1);

    let f0 = [c00, c00, c02, c03];
    let f1 = [c04, c04, c06, c07];
    let f2 = [c08, c08, c10, c11];
    let f3 = [c12, c12, c14, c15];
    let f4 = [c16, c16, c18, c19];
    let f5 = [c20, c20, c22, c23];

    let v0 = [s(1, 0), s(0, 0), s(0, 0), s(0, 0)];
    let v1 = [s(1, 1), s(0, 1), s(0, 1), s(0, 1)];
    let v2 = [s(1, 2), s(0, 2), s(0, 2), s(0, 2)];
    let v3 = [s(1, 3), s(0, 3), s(0, 3), s(0, 3)];

    let mut inv = [[0.0f32; 4]; 4];
    let sign_a = [1.0, -1.0, 1.0, -1.0];
    let sign_b = [-1.0, 1.0, -1.0, 1.0];

    for i in 0..4 {
        inv[0][i] = sign_a[i] * (v1[i] * f0[i] - v2[i] * f1[i] + v3[i] * f2[i]);
        inv[1][i] = sign_b[i] * (v0[i] * f0[i] - v2[i] * f3[i] + v3[i] * f4[i]);
        inv[2][i] = sign_a[i] * (v0[i] * f1[i] - v1[i] * f3[i] + v3[i] * f5[i]);
        inv[3][i] = sign_b[i] * (v0[i] * f2[i] - v1[i] * f4[i] + v2[i] * f5[i]);
    }

    let det = s(0, 0) * inv[0][0] + s(1, 0) * inv[0][1] + s(2, 0) * inv[0][2] + s(3, 0) * inv[0][3];

    if det.abs() < 1e-10 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }

    let inv_det = 1.0 / det;
    for col in &mut inv {
        for val in col.iter_mut() {
            *val *= inv_det;
        }
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1280.0;
    const H: f32 = 720.0;

    /// World point to screen (top-left origin, y down), as the renderer
    /// would draw it.
    fn project(cam: &Camera, p: [f32; 3]) -> [f32; 2] {
        let m = cam.view_projection_matrix();
        let mut c = [0.0f32; 4];
        for (row, o) in c.iter_mut().enumerate() {
            *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
        }
        assert!(c[3] > 0.0, "{p:?} is behind the camera");
        let ndc = [c[0] / c[3], c[1] / c[3]];
        [(ndc[0] * 0.5 + 0.5) * W, (0.5 - ndc[1] * 0.5) * H]
    }

    /// A camera orbiting a target well away from the origin, with a far
    /// plane 20,000 times its near plane (the precision trap).
    fn orbit_camera(pitch_deg: f32, yaw_deg: f32, orthographic: bool) -> Camera {
        let mut cam = Camera {
            target: Vec3::new(350.0, 0.0, -220.0),
            distance: 120.0,
            pitch: pitch_deg.to_radians(),
            yaw: yaw_deg.to_radians(),
            near: 0.1,
            far: 2000.0,
            fov: 50.0,
            aspect: W / H,
            orthographic,
            ortho_height: if orthographic { 80.0 } else { 0.0 },
            ..Camera::default()
        };
        cam.update_orbit();
        cam
    }

    /// A camera directly above `(x, z)`: forward is exactly -up.
    fn top_down(orthographic: bool) -> Camera {
        Camera {
            position: Vec3::new(10.0, 50.0, -5.0),
            target: Vec3::new(10.0, 0.0, -5.0),
            aspect: W / H,
            orthographic,
            ortho_height: if orthographic { 30.0 } else { 0.0 },
            ..Camera::default()
        }
    }

    /// Ground points spread over what the camera sees: project each one to
    /// the screen, then pick it back off the ground.
    fn assert_round_trips(cam: &Camera, plane_y: f32, label: &str) {
        let t = cam.target;
        let mut checked = 0;
        for i in -4..=4 {
            for j in -4..=4 {
                let p = [t.x + i as f32 * 9.0, plane_y, t.z + j as f32 * 9.0];
                let [sx, sy] = project(cam, p);
                if !(0.0..=W).contains(&sx) || !(0.0..=H).contains(&sy) {
                    continue;
                }
                let hit = cam
                    .screen_to_ground(sx, sy, W, H, plane_y)
                    .unwrap_or_else(|| panic!("{label}: no hit for {p:?} at ({sx}, {sy})"));
                let err = ((hit[0] - p[0]).powi(2) + (hit[1] - p[2]).powi(2)).sqrt();
                assert!(
                    err < 2e-3,
                    "{label}: {p:?} came back as {hit:?} ({err} m off)"
                );
                checked += 1;
            }
        }
        assert!(checked >= 20, "{label}: only {checked} points on screen");
    }

    #[test]
    fn perspective_round_trips_at_every_pitch() {
        for pitch in [15.0, 30.0, 45.0, 60.0, 75.0, 89.0, 90.0] {
            for yaw in [0.0, 37.0, 180.0, 290.0] {
                let cam = orbit_camera(pitch, yaw, false);
                assert_round_trips(&cam, 0.0, &format!("persp pitch {pitch} yaw {yaw}"));
                assert_round_trips(&cam, 1.5, &format!("persp pitch {pitch} yaw {yaw} y=1.5"));
            }
        }
    }

    #[test]
    fn orthographic_round_trips_at_every_pitch() {
        for pitch in [15.0, 30.0, 45.0, 60.0, 75.0, 89.0, 90.0] {
            for yaw in [0.0, 37.0, 180.0, 290.0] {
                let cam = orbit_camera(pitch, yaw, true);
                assert_round_trips(&cam, 0.0, &format!("ortho pitch {pitch} yaw {yaw}"));
            }
        }
        // The legacy size (ortho_height 0: derived from distance and fov).
        let mut cam = orbit_camera(50.0, 20.0, true);
        cam.ortho_height = 0.0;
        assert_round_trips(&cam, 0.0, "ortho legacy height");
    }

    #[test]
    fn straight_down_reads_like_a_map() {
        for ortho in [false, true] {
            let cam = top_down(ortho);
            assert!(cam.view_matrix().iter().flatten().all(|v| v.is_finite()));
            let centre = cam.screen_to_ground(W / 2.0, H / 2.0, W, H, 0.0).unwrap();
            assert!((centre[0] - 10.0).abs() < 1e-4 && (centre[1] + 5.0).abs() < 1e-4);
            // North (-Z) at the top of the screen, east (+X) on the right.
            let top = cam.screen_to_ground(W / 2.0, 0.0, W, H, 0.0).unwrap();
            let right = cam.screen_to_ground(W, H / 2.0, W, H, 0.0).unwrap();
            assert!(top[1] < centre[1] - 1.0 && (top[0] - centre[0]).abs() < 1e-3);
            assert!(right[0] > centre[0] + 1.0 && (right[1] - centre[1]).abs() < 1e-3);
            assert_round_trips(&cam, 0.0, if ortho { "ortho down" } else { "persp down" });
        }
    }

    #[test]
    fn hidpi_logical_and_physical_agree() {
        for ortho in [false, true] {
            let cam = orbit_camera(40.0, 15.0, ortho);
            let scale = 1.5;
            for (x, y) in [(100.0, 600.0), (640.0, 400.0), (1200.0, 700.0)] {
                let logical = cam.screen_to_ground(x, y, W, H, 0.0).unwrap();
                let physical = cam
                    .screen_to_ground(x * scale, y * scale, W * scale, H * scale, 0.0)
                    .unwrap();
                assert!((logical[0] - physical[0]).abs() < 1e-3);
                assert!((logical[1] - physical[1]).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn ndc_offset_is_honoured() {
        let mut cam = orbit_camera(35.0, 0.0, false);
        cam.ndc_offset = [0.3, -0.2];
        let t = [cam.target.x, 0.0, cam.target.z];
        let [sx, sy] = project(&cam, t);
        assert!(
            (sx - W / 2.0).abs() > 50.0,
            "the offset should move the target"
        );
        let hit = cam.screen_to_ground(sx, sy, W, H, 0.0).unwrap();
        assert!((hit[0] - t[0]).abs() < 1e-3 && (hit[1] - t[2]).abs() < 1e-3);
    }

    #[test]
    fn misses_are_none() {
        // Perspective at a shallow pitch: the top of the screen is sky.
        let cam = orbit_camera(5.0, 0.0, false);
        assert!(cam.screen_to_ground(W / 2.0, 0.0, W, H, 0.0).is_none());
        // Orthographic looking level: every ray runs parallel to the ground.
        let level = orbit_camera(0.0, 0.0, true);
        assert!(level
            .screen_to_ground(W / 2.0, H / 2.0, W, H, 0.0)
            .is_none());
        // A plane above a camera that looks down is behind it.
        let down = top_down(false);
        assert!(down
            .screen_to_ground(W / 2.0, H / 2.0, W, H, 80.0)
            .is_none());
    }

    #[test]
    fn unproject_ray_starts_at_the_eye_for_perspective() {
        let cam = orbit_camera(30.0, 60.0, false);
        let (origin, dir) = cam.unproject_ray(0.0, 0.0);
        let f = cam.forward_vector();
        let eye = cam.position_array();
        let gap: f32 = (0..3)
            .map(|i| (origin[i] - eye[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!((gap - cam.near).abs() < 1e-3, "origin {gap} from the eye");
        assert!((0..3).all(|i| (dir[i] - f[i]).abs() < 1e-5));
    }
}
