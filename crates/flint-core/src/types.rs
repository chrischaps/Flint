//! Spatial and common types

use serde::{Deserialize, Serialize};
use std::ops::{Add, Mul, Sub};

/// A 3D vector
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    pub const UP: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub const FORWARD: Self = Self {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    };
    pub const RIGHT: Self = Self {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn from_array(arr: [f32; 3]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
            z: arr[2],
        }
    }

    pub fn to_array(&self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalized(&self) -> Self {
        let len = self.length();
        if len > 0.0 {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            Self::ZERO
        }
    }

    pub fn dot(&self, other: &Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(&self, other: &Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
            z: self.z * scalar,
        }
    }
}

/// A 3D transform with position, rotation (Euler angles), and scale
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub position: Vec3,
    /// Rotation in degrees (Euler angles: pitch, yaw, roll)
    pub rotation: Vec3,
    pub scale: Vec3,
    /// Optional quaternion rotation [x, y, z, w]. When present, takes precedence
    /// over Euler angles in to_matrix() to avoid gimbal lock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_quat: Option<[f32; 4]>,
    /// Optional shear, applied between rotation and scale (#625):
    /// `[xy, xz, yx, yz, zx, zy]`, where `ab` is how far `a` moves per unit
    /// of `b`, so `x' = x + xy*y + xz*z` and so on. `None` (or all zeros) is
    /// the rigid-plus-scale matrix it always was. See [`Shear`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear: Option<[f32; 6]>,
}

/// Names of the six shear terms, in the order [`Transform::shear`] stores
/// them. `xz` is the one a ground strip curving sideways wants: x moves by
/// `xz` metres for every metre along z.
pub struct Shear;

impl Shear {
    pub const KEYS: [&'static str; 6] = ["xy", "xz", "yx", "yz", "zx", "zy"];

    /// The shear as a 3x3 matrix, row-major: `h[row][col]`.
    pub fn matrix(s: &[f32; 6]) -> [[f32; 3]; 3] {
        let [xy, xz, yx, yz, zx, zy] = *s;
        [[1.0, xy, xz], [yx, 1.0, yz], [zx, zy, 1.0]]
    }

    /// Read `transform.shear` from a scene or a script: a table of any of
    /// the six named terms (`{ xz = 0.25 }`; a missing term is 0). `None`
    /// for anything else, a term that isn't a number, or a shear that is
    /// all zeros, so an unsheared entity stays `None`.
    pub fn from_toml(value: &toml::Value) -> Option<[f32; 6]> {
        let num = crate::toml_util::toml_f64;
        let mut out = [0.0f32; 6];
        match value {
            toml::Value::Table(t) => {
                for (i, key) in Self::KEYS.iter().enumerate() {
                    if let Some(v) = t.get(*key) {
                        out[i] = num(v)? as f32;
                    }
                }
            }
            _ => return None,
        }
        out.iter().any(|v| *v != 0.0).then_some(out)
    }

    /// Unknown keys in a `shear` table, for warnings: `xzz` is a typo that
    /// would otherwise shear nothing, silently.
    pub fn unknown_keys(value: &toml::Value) -> Vec<String> {
        match value {
            toml::Value::Table(t) => t
                .keys()
                .filter(|k| !Self::KEYS.contains(&k.as_str()))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The table form a writer stores: only the terms that aren't zero.
    pub fn to_toml(s: &[f32; 6]) -> toml::Value {
        let mut t = toml::map::Map::new();
        for (key, v) in Self::KEYS.iter().zip(s) {
            if *v != 0.0 {
                t.insert((*key).to_string(), toml::Value::Float(*v as f64));
            }
        }
        toml::Value::Table(t)
    }

    /// Determinant of [`Self::matrix`]. Near zero the shear flattens space
    /// (`xy = yx = 1` folds x onto y) and normals have no inverse to use.
    pub fn determinant(s: &[f32; 6]) -> f32 {
        let h = Self::matrix(s);
        h[0][0] * (h[1][1] * h[2][2] - h[1][2] * h[2][1])
            - h[0][1] * (h[1][0] * h[2][2] - h[1][2] * h[2][0])
            + h[0][2] * (h[1][0] * h[2][1] - h[1][1] * h[2][0])
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            rotation_quat: None,
            shear: None,
        }
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        rotation_quat: None,
        shear: None,
    };

    pub fn from_position(position: Vec3) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    pub fn with_position(mut self, position: Vec3) -> Self {
        self.position = position;
        self
    }

    pub fn with_rotation(mut self, rotation: Vec3) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_rotation_quat(mut self, q: [f32; 4]) -> Self {
        self.rotation_quat = Some(q);
        self
    }

    /// Shear terms `[xy, xz, yx, yz, zx, zy]` (see [`Shear`]).
    pub fn with_shear(mut self, shear: [f32; 6]) -> Self {
        self.shear = Some(shear);
        self
    }

    /// Convert to a 4x4 transformation matrix (column-major)
    pub fn to_matrix(&self) -> [[f32; 4]; 4] {
        let (r00, r01, r02, r10, r11, r12, r20, r21, r22) =
            if let Some([x, y, z, w]) = self.rotation_quat {
                // Build rotation matrix directly from quaternion (no gimbal lock)
                (
                    1.0 - 2.0 * (y * y + z * z),
                    2.0 * (x * y - w * z),
                    2.0 * (x * z + w * y),
                    2.0 * (x * y + w * z),
                    1.0 - 2.0 * (x * x + z * z),
                    2.0 * (y * z - w * x),
                    2.0 * (x * z - w * y),
                    2.0 * (y * z + w * x),
                    1.0 - 2.0 * (x * x + y * y),
                )
            } else {
                // Euler angles path (ZYX order)
                let (px, py, pz) = (
                    self.rotation.x.to_radians(),
                    self.rotation.y.to_radians(),
                    self.rotation.z.to_radians(),
                );

                let (sx, cx) = (px.sin(), px.cos());
                let (sy, cy) = (py.sin(), py.cos());
                let (sz, cz) = (pz.sin(), pz.cos());

                (
                    cy * cz,
                    sx * sy * cz - cx * sz,
                    cx * sy * cz + sx * sz,
                    cy * sz,
                    sx * sy * sz + cx * cz,
                    cx * sy * sz - sx * cz,
                    -sy,
                    sx * cy,
                    cx * cy,
                )
            };

        // Shear sits between rotation and scale: M = T * R * H * S, so a
        // point is scaled, then sheared, then rotated. Column j of R*H*S is
        // R times column j of H, times scale j. With no shear H is the
        // identity and this is exactly the old R*S, bit for bit.
        let r = [[r00, r01, r02], [r10, r11, r12], [r20, r21, r22]];
        let scale = [self.scale.x, self.scale.y, self.scale.z];
        let mut cols = [[0.0f32; 3]; 3];
        match self.shear {
            Some(sh) if sh.iter().any(|v| *v != 0.0) => {
                let h = Shear::matrix(&sh);
                for (j, col) in cols.iter_mut().enumerate() {
                    for (i, out) in col.iter_mut().enumerate() {
                        let rh = r[i][0] * h[0][j] + r[i][1] * h[1][j] + r[i][2] * h[2][j];
                        *out = rh * scale[j];
                    }
                }
            }
            _ => {
                for (j, col) in cols.iter_mut().enumerate() {
                    for (i, out) in col.iter_mut().enumerate() {
                        *out = r[i][j] * scale[j];
                    }
                }
            }
        }

        [
            [cols[0][0], cols[0][1], cols[0][2], 0.0],
            [cols[1][0], cols[1][1], cols[1][2], 0.0],
            [cols[2][0], cols[2][1], cols[2][2], 0.0],
            [self.position.x, self.position.y, self.position.z, 1.0],
        ]
    }
}

/// RGBA color
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const RED: Self = Self {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const GREEN: Self = Self {
        r: 0.0,
        g: 1.0,
        b: 0.0,
        a: 1.0,
    };
    pub const BLUE: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    };

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as f32 / 255.0,
            g: ((hex >> 8) & 0xFF) as f32 / 255.0,
            b: (hex & 0xFF) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub fn to_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

/// Multiply two 4x4 column-major matrices
pub fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_operations() {
        let v1 = Vec3::new(1.0, 2.0, 3.0);
        let v2 = Vec3::new(4.0, 5.0, 6.0);

        let sum = v1 + v2;
        assert_eq!(sum, Vec3::new(5.0, 7.0, 9.0));

        let diff = v2 - v1;
        assert_eq!(diff, Vec3::new(3.0, 3.0, 3.0));

        let scaled = v1 * 2.0;
        assert_eq!(scaled, Vec3::new(2.0, 4.0, 6.0));
    }

    #[test]
    fn test_transform_default() {
        let t = Transform::default();
        assert_eq!(t.position, Vec3::ZERO);
        assert_eq!(t.rotation, Vec3::ZERO);
        assert_eq!(t.scale, Vec3::ONE);
    }

    fn apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
        let mut out = [m[3][0], m[3][1], m[3][2]];
        for (row, o) in out.iter_mut().enumerate() {
            for (col, v) in p.iter().enumerate() {
                *o += m[col][row] * v;
            }
        }
        out
    }

    #[test]
    fn shear_moves_x_with_z() {
        // The scanline road's tile: x' = x + s*z.
        let t = Transform::default().with_shear([0.0, 0.25, 0.0, 0.0, 0.0, 0.0]);
        let m = t.to_matrix();
        assert_eq!(apply(&m, [0.0, 0.0, 4.0]), [1.0, 0.0, 4.0]);
        assert_eq!(apply(&m, [2.0, 3.0, 0.0]), [2.0, 3.0, 0.0]);
    }

    #[test]
    fn shear_sits_between_rotation_and_scale() {
        // Scale first: a 2x z stretch doubles how far the shear carries x.
        let t = Transform::default()
            .with_scale(Vec3::new(1.0, 1.0, 2.0))
            .with_shear([0.0, 0.5, 0.0, 0.0, 0.0, 0.0])
            .with_rotation(Vec3::new(0.0, 90.0, 0.0));
        let p = apply(&t.to_matrix(), [0.0, 0.0, 1.0]);
        // scaled (0,0,2) -> sheared (1,0,2) -> yawed 90 deg: (x,z) -> (z,-x)
        for (got, want) in p.iter().zip([2.0, 0.0, -1.0]) {
            assert!((got - want).abs() < 1e-5, "{p:?}");
        }
    }

    #[test]
    fn no_shear_is_the_old_matrix_bit_for_bit() {
        let t = Transform::default()
            .with_position(Vec3::new(1.0, 2.0, 3.0))
            .with_rotation(Vec3::new(10.0, 20.0, 30.0))
            .with_scale(Vec3::new(2.0, 0.5, 3.0));
        let plain = t.to_matrix();
        assert_eq!(t.with_shear([0.0; 6]).to_matrix(), plain);
        // The old composition, written out: column j is R's column j times scale j.
        let r = Transform {
            scale: Vec3::ONE,
            position: Vec3::ZERO,
            ..t
        }
        .to_matrix();
        let s = [2.0, 0.5, 3.0];
        for col in 0..3 {
            for row in 0..3 {
                assert_eq!(plain[col][row], r[col][row] * s[col]);
            }
        }
    }

    #[test]
    fn shear_reads_named_terms_from_toml() {
        let v: toml::Value = toml::from_str("xz = 0.25\nzy = -1").unwrap();
        assert_eq!(Shear::from_toml(&v), Some([0.0, 0.25, 0.0, 0.0, 0.0, -1.0]));
        let zero: toml::Value = toml::from_str("xz = 0.0").unwrap();
        assert_eq!(Shear::from_toml(&zero), None);
        let typo: toml::Value = toml::from_str("xzz = 0.25").unwrap();
        assert_eq!(Shear::unknown_keys(&typo), vec!["xzz".to_string()]);
        let round = Shear::to_toml(&[0.0, 0.25, 0.0, 0.0, 0.0, -1.0]);
        assert_eq!(Shear::from_toml(&round), Shear::from_toml(&v));
        assert_eq!(round.as_table().unwrap().len(), 2);
    }

    #[test]
    fn a_folding_shear_has_no_determinant() {
        assert!(Shear::determinant(&[1.0, 0.0, 1.0, 0.0, 0.0, 0.0]).abs() < 1e-6);
        assert_eq!(Shear::determinant(&[0.0, 0.7, 0.0, 0.3, 0.0, 0.0]), 1.0);
    }

    #[test]
    fn test_color_from_hex() {
        let c = Color::from_hex(0xFF8844);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.533).abs() < 0.01);
        assert!((c.b - 0.267).abs() < 0.01);
    }
}
