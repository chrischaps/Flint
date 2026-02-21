//! Mouse picking via ray-AABB intersection
//!
//! Unprojects screen coordinates through the camera's inverse view-projection
//! matrix and tests against entity bounding boxes for viewport click selection.

use flint_core::EntityId;
use flint_render::Camera;

/// A ray in 3D space
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
}

/// Axis-Aligned Bounding Box in world space
#[derive(Debug, Clone, Copy)]
pub struct AABB {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Entity with its world-space AABB for picking
#[derive(Debug, Clone)]
pub struct PickTarget {
    pub entity_id: EntityId,
    pub aabb: AABB,
}

impl Ray {
    /// Create a ray from screen coordinates using the camera's inverse VP matrix.
    /// `screen_x`, `screen_y` are in physical pixels.
    /// `viewport_width`, `viewport_height` are the viewport dimensions in physical pixels.
    pub fn from_screen(
        screen_x: f32,
        screen_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        camera: &Camera,
    ) -> Self {
        let inv_vp = camera.inverse_view_projection_matrix();

        // Convert to NDC [-1, 1]
        let ndc_x = 2.0 * screen_x / viewport_width - 1.0;
        let ndc_y = 1.0 - 2.0 * screen_y / viewport_height; // Y flipped

        // Unproject near point (z = -1 in clip space for OpenGL-style)
        let near_clip = [ndc_x, ndc_y, -1.0, 1.0];
        let far_clip = [ndc_x, ndc_y, 1.0, 1.0];

        let near_world = mul_mat4_vec4(&inv_vp, near_clip);
        let far_world = mul_mat4_vec4(&inv_vp, far_clip);

        let near_w = near_world[3];
        let far_w = far_world[3];

        let origin = [
            near_world[0] / near_w,
            near_world[1] / near_w,
            near_world[2] / near_w,
        ];
        let far_pt = [
            far_world[0] / far_w,
            far_world[1] / far_w,
            far_world[2] / far_w,
        ];

        let dir = [
            far_pt[0] - origin[0],
            far_pt[1] - origin[1],
            far_pt[2] - origin[2],
        ];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let direction = if len > 1e-8 {
            [dir[0] / len, dir[1] / len, dir[2] / len]
        } else {
            [0.0, 0.0, -1.0]
        };

        Self { origin, direction }
    }
}

impl AABB {
    /// Create from center position and half-extents
    pub fn from_center_half(center: [f32; 3], half: [f32; 3]) -> Self {
        Self {
            min: [center[0] - half[0], center[1] - half[1], center[2] - half[2]],
            max: [center[0] + half[0], center[1] + half[1], center[2] + half[2]],
        }
    }

    /// Create from min and max corners
    pub fn from_min_max(min: [f32; 3], max: [f32; 3]) -> Self {
        Self { min, max }
    }

    /// Transform an AABB by a 4x4 world matrix (column-major).
    /// Uses the standard AABB-from-transformed-AABB technique.
    pub fn transformed(&self, mat: &[[f32; 4]; 4]) -> Self {
        // Translation from column 3
        let mut new_min = [mat[3][0], mat[3][1], mat[3][2]];
        let mut new_max = [mat[3][0], mat[3][1], mat[3][2]];

        // For each axis of the original AABB, project through the rotation/scale
        for i in 0..3 {
            for j in 0..3 {
                let a = mat[i][j] * self.min[i];
                let b = mat[i][j] * self.max[i];
                new_min[j] += a.min(b);
                new_max[j] += a.max(b);
            }
        }

        Self { min: new_min, max: new_max }
    }
}

/// Ray-AABB intersection using the slab method (Kay/Kajiya).
/// Returns the distance along the ray to the nearest hit, or None if no intersection.
pub fn ray_intersect(ray: &Ray, aabb: &AABB) -> Option<f32> {
    let mut tmin = f32::NEG_INFINITY;
    let mut tmax = f32::INFINITY;

    for i in 0..3 {
        if ray.direction[i].abs() < 1e-8 {
            // Ray is parallel to this slab
            if ray.origin[i] < aabb.min[i] || ray.origin[i] > aabb.max[i] {
                return None;
            }
        } else {
            let inv_d = 1.0 / ray.direction[i];
            let mut t1 = (aabb.min[i] - ray.origin[i]) * inv_d;
            let mut t2 = (aabb.max[i] - ray.origin[i]) * inv_d;

            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            tmin = tmin.max(t1);
            tmax = tmax.min(t2);

            if tmin > tmax {
                return None;
            }
        }
    }

    // Return nearest positive intersection
    if tmax < 0.0 {
        None // AABB is behind the ray
    } else {
        Some(tmin.max(0.0))
    }
}

/// Pick the nearest entity at the given screen coordinates.
pub fn pick_entity(
    screen_x: f32,
    screen_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    camera: &Camera,
    targets: &[PickTarget],
) -> Option<(EntityId, f32)> {
    let ray = Ray::from_screen(screen_x, screen_y, viewport_width, viewport_height, camera);

    let mut best: Option<(EntityId, f32)> = None;

    for target in targets {
        if let Some(dist) = ray_intersect(&ray, &target.aabb) {
            if best.map_or(true, |(_, d)| dist < d) {
                best = Some((target.entity_id, dist));
            }
        }
    }

    best
}

/// Multiply a 4x4 column-major matrix by a 4D vector
fn mul_mat4_vec4(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0] * v[3],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1] * v[3],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2] * v[3],
        m[0][3] * v[0] + m[1][3] * v[1] + m[2][3] * v[2] + m[3][3] * v[3],
    ]
}

/// Build pick targets from a FlintWorld by reading entity transforms and bounds.
pub fn build_pick_targets(world: &flint_ecs::FlintWorld) -> Vec<PickTarget> {
    let mut targets = Vec::new();

    for entity in world.all_entities() {
        let transform = world.get_transform(entity.id).unwrap_or_default();
        let world_mat = world.get_world_matrix(entity.id)
            .unwrap_or_else(|| transform.to_matrix());

        // Try to get bounds component for size, else use a 1x1x1 default
        let (local_min, local_max) = if let Some(components) = world.get_components(entity.id) {
            if let Some(bounds) = components.get("bounds") {
                extract_bounds_min_max(bounds).unwrap_or(([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
            } else {
                // Check for model component — use a default size
                if components.has("model") {
                    ([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5])
                } else {
                    ([-0.25, -0.25, -0.25], [0.25, 0.25, 0.25])
                }
            }
        } else {
            ([-0.25, -0.25, -0.25], [0.25, 0.25, 0.25])
        };

        let local_aabb = AABB::from_min_max(local_min, local_max);
        let world_aabb = local_aabb.transformed(&world_mat);

        targets.push(PickTarget {
            entity_id: entity.id,
            aabb: world_aabb,
        });
    }

    targets
}

fn extract_bounds_min_max(bounds: &toml::Value) -> Option<([f32; 3], [f32; 3])> {
    let min = bounds.get("min")?;
    let max = bounds.get("max")?;
    let min_arr = extract_vec3(min)?;
    let max_arr = extract_vec3(max)?;
    Some((min_arr, max_arr))
}

fn extract_vec3(value: &toml::Value) -> Option<[f32; 3]> {
    if let Some(arr) = value.as_array() {
        if arr.len() >= 3 {
            let x = arr[0].as_float().or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
            let y = arr[1].as_float().or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
            let z = arr[2].as_float().or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
            return Some([x, y, z]);
        }
    }
    None
}
