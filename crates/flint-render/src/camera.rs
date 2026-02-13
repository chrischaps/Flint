//! 3D Camera with orbit and first-person controls

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

        self.position = Vec3::new(
            self.target.x + x,
            self.target.y + y,
            self.target.z + z,
        );
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
        self.distance = (self.distance - delta).max(1.0).min(100.0);
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
        let forward = (self.target - self.position).normalized();
        let right = forward.cross(&self.up).normalized();
        let up = right.cross(&forward);

        self.target = self.target + right * dx + up * dy;
        self.update_orbit();
    }

    /// Get the view matrix (4x4, column-major)
    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        let f = (self.target - self.position).normalized();
        let s = f.cross(&self.up).normalized();
        let u = s.cross(&f);

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
        if self.orthographic {
            self.orthographic_matrix()
        } else {
            self.perspective_matrix()
        }
    }

    fn perspective_matrix(&self) -> [[f32; 4]; 4] {
        let fov_rad = self.fov.to_radians();
        let f = 1.0 / (fov_rad / 2.0).tan();

        let depth = self.far - self.near;

        [
            [f / self.aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, -(self.far + self.near) / depth, -1.0],
            [0.0, 0.0, -(2.0 * self.far * self.near) / depth, 0.0],
        ]
    }

    fn orthographic_matrix(&self) -> [[f32; 4]; 4] {
        // Size the ortho volume so objects at `distance` appear the same size as in perspective
        let half_h = self.distance * (self.fov.to_radians() / 2.0).tan();
        let half_w = half_h * self.aspect;
        let depth = self.far - self.near;

        // Column-major: m[col][row]
        // Maps depth to [0, 1] (wgpu convention): z_view=-near → 0, z_view=-far → 1
        // (The perspective matrix uses [-1,1] OpenGL convention which works due to
        // hyperbolic 1/z mapping, but orthographic has w=1 so we need [0,1] directly)
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
        let f = (self.target - self.position).normalized();
        let s = f.cross(&self.up).normalized();
        [s.x, s.y, s.z]
    }

    /// Get camera up vector (world space, perpendicular to both forward and right)
    pub fn up_vector(&self) -> [f32; 3] {
        let f = (self.target - self.position).normalized();
        let s = f.cross(&self.up).normalized();
        let u = s.cross(&f);
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
}

fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                result[i][j] += a[k][j] * b[i][k];
            }
        }
    }
    result
}

/// Compute the inverse of a 4x4 column-major matrix using cofactor expansion
fn mat4_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Flatten column-major to indexable
    let s = |col: usize, row: usize| -> f32 { m[col][row] };

    let c00 = s(2,2)*s(3,3) - s(3,2)*s(2,3);
    let c02 = s(1,2)*s(3,3) - s(3,2)*s(1,3);
    let c03 = s(1,2)*s(2,3) - s(2,2)*s(1,3);

    let c04 = s(2,1)*s(3,3) - s(3,1)*s(2,3);
    let c06 = s(1,1)*s(3,3) - s(3,1)*s(1,3);
    let c07 = s(1,1)*s(2,3) - s(2,1)*s(1,3);

    let c08 = s(2,1)*s(3,2) - s(3,1)*s(2,2);
    let c10 = s(1,1)*s(3,2) - s(3,1)*s(1,2);
    let c11 = s(1,1)*s(2,2) - s(2,1)*s(1,2);

    let c12 = s(2,0)*s(3,3) - s(3,0)*s(2,3);
    let c14 = s(1,0)*s(3,3) - s(3,0)*s(1,3);
    let c15 = s(1,0)*s(2,3) - s(2,0)*s(1,3);

    let c16 = s(2,0)*s(3,2) - s(3,0)*s(2,2);
    let c18 = s(1,0)*s(3,2) - s(3,0)*s(1,2);
    let c19 = s(1,0)*s(2,2) - s(2,0)*s(1,2);

    let c20 = s(2,0)*s(3,1) - s(3,0)*s(2,1);
    let c22 = s(1,0)*s(3,1) - s(3,0)*s(1,1);
    let c23 = s(1,0)*s(2,1) - s(2,0)*s(1,1);

    let f0 = [c00, c00, c02, c03];
    let f1 = [c04, c04, c06, c07];
    let f2 = [c08, c08, c10, c11];
    let f3 = [c12, c12, c14, c15];
    let f4 = [c16, c16, c18, c19];
    let f5 = [c20, c20, c22, c23];

    let v0 = [s(1,0), s(0,0), s(0,0), s(0,0)];
    let v1 = [s(1,1), s(0,1), s(0,1), s(0,1)];
    let v2 = [s(1,2), s(0,2), s(0,2), s(0,2)];
    let v3 = [s(1,3), s(0,3), s(0,3), s(0,3)];

    let mut inv = [[0.0f32; 4]; 4];
    let sign_a = [1.0, -1.0, 1.0, -1.0];
    let sign_b = [-1.0, 1.0, -1.0, 1.0];

    for i in 0..4 {
        inv[0][i] = sign_a[i] * (v1[i]*f0[i] - v2[i]*f1[i] + v3[i]*f2[i]);
        inv[1][i] = sign_b[i] * (v0[i]*f0[i] - v2[i]*f3[i] + v3[i]*f4[i]);
        inv[2][i] = sign_a[i] * (v0[i]*f1[i] - v1[i]*f3[i] + v3[i]*f5[i]);
        inv[3][i] = sign_b[i] * (v0[i]*f2[i] - v1[i]*f4[i] + v2[i]*f5[i]);
    }

    let det = s(0,0)*inv[0][0] + s(1,0)*inv[0][1] + s(2,0)*inv[0][2] + s(3,0)*inv[0][3];

    if det.abs() < 1e-10 {
        return [[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]];
    }

    let inv_det = 1.0 / det;
    for col in &mut inv {
        for val in col.iter_mut() {
            *val *= inv_det;
        }
    }
    inv
}
