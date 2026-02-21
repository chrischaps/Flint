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
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            rotation_quat: None,
        }
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        rotation_quat: None,
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

        // Apply scale and translation
        let mat = [
            [r00 * self.scale.x, r10 * self.scale.x, r20 * self.scale.x, 0.0],
            [r01 * self.scale.y, r11 * self.scale.y, r21 * self.scale.y, 0.0],
            [r02 * self.scale.z, r12 * self.scale.z, r22 * self.scale.z, 0.0],
            [self.position.x, self.position.y, self.position.z, 1.0],
        ];
        mat
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

    #[test]
    fn test_color_from_hex() {
        let c = Color::from_hex(0xFF8844);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.533).abs() < 0.01);
        assert!((c.b - 0.267).abs() < 0.01);
    }
}
