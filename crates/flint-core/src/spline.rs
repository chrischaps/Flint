//! Pure spline math — Catmull-Rom path sampling with twist (banking).
//!
//! Provides open and closed spline evaluation from control points,
//! producing evenly-spaced samples with position, forward, right, up
//! basis vectors and parametric t values.

use crate::Vec3;

/// A single control point on a Catmull-Rom spline.
#[derive(Debug, Clone)]
pub struct SplineControlPoint {
    pub position: Vec3,
    /// Twist (banking) in degrees around the forward axis.
    pub twist: f32,
}

/// A sampled point along a spline with computed basis vectors.
#[derive(Debug, Clone)]
pub struct SplineSample {
    pub position: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    /// Twist in degrees (before application to right/up).
    pub twist: f32,
    /// Parametric t in [0, 1) along the spline.
    pub t: f32,
}

/// Catmull-Rom spline interpolation between four points.
pub fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    Vec3::new(
        0.5 * ((2.0 * p1.x)
            + (-p0.x + p2.x) * t
            + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
            + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3),
        0.5 * ((2.0 * p1.y)
            + (-p0.y + p2.y) * t
            + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
            + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3),
        0.5 * ((2.0 * p1.z)
            + (-p0.z + p2.z) * t
            + (2.0 * p0.z - 5.0 * p1.z + 4.0 * p2.z - p3.z) * t2
            + (-p0.z + 3.0 * p1.z - 3.0 * p2.z + p3.z) * t3),
    )
}

/// Catmull-Rom interpolation for a single scalar value.
pub fn catmull_rom_scalar(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Rotate a vector around an axis by an angle in radians (Rodrigues' formula).
pub fn rotate_around_axis(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let dot = v.dot(&axis);
    let cross = axis.cross(&v);
    Vec3::new(
        v.x * cos_a + cross.x * sin_a + axis.x * dot * (1.0 - cos_a),
        v.y * cos_a + cross.y * sin_a + axis.y * dot * (1.0 - cos_a),
        v.z * cos_a + cross.z * sin_a + axis.z * dot * (1.0 - cos_a),
    )
}

/// Sample a closed-loop Catmull-Rom spline at approximately `spacing` meters apart.
pub fn sample_closed_spline(points: &[SplineControlPoint], spacing: f32) -> Vec<SplineSample> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }

    let world_up = Vec3::UP;

    // Estimate total spline length
    let segments_per_cp = 20;
    let mut total_length = 0.0_f32;
    let mut prev_pos = points[0].position;
    for seg in 0..n {
        let p0 = points[(seg + n - 1) % n].position;
        let p1 = points[seg].position;
        let p2 = points[(seg + 1) % n].position;
        let p3 = points[(seg + 2) % n].position;
        for j in 1..=segments_per_cp {
            let t = j as f32 / segments_per_cp as f32;
            let pos = catmull_rom(p0, p1, p2, p3, t);
            total_length += (pos - prev_pos).length();
            prev_pos = pos;
        }
    }

    let num_samples = (total_length / spacing).ceil() as usize;
    let num_samples = num_samples.max(20);
    let mut samples = Vec::with_capacity(num_samples);

    for i in 0..num_samples {
        let global_t = i as f32 / num_samples as f32;
        let scaled = global_t * n as f32;
        let seg = scaled.floor() as usize % n;
        let local_t = scaled - scaled.floor();

        let p0 = points[(seg + n - 1) % n].position;
        let p1 = points[seg].position;
        let p2 = points[(seg + 1) % n].position;
        let p3 = points[(seg + 2) % n].position;

        let position = catmull_rom(p0, p1, p2, p3, local_t);

        // Forward tangent via finite difference
        let dt = 0.001;
        let next_t = local_t + dt;
        let pos_next = if next_t <= 1.0 {
            catmull_rom(p0, p1, p2, p3, next_t)
        } else {
            let next_seg = (seg + 1) % n;
            let np0 = points[(next_seg + n - 1) % n].position;
            let np1 = points[next_seg].position;
            let np2 = points[(next_seg + 1) % n].position;
            let np3 = points[(next_seg + 2) % n].position;
            catmull_rom(np0, np1, np2, np3, next_t - 1.0)
        };

        let forward = (pos_next - position).normalized();

        // Interpolate twist using Catmull-Rom (C1 continuous, matches position)
        let tw0 = points[(seg + n - 1) % n].twist;
        let tw1 = points[seg].twist;
        let tw2 = points[(seg + 1) % n].twist;
        let tw3 = points[(seg + 2) % n].twist;
        let twist = catmull_rom_scalar(tw0, tw1, tw2, tw3, local_t);
        let twist_rad = twist.to_radians();

        // Basis vectors with twist
        let right_flat = forward.cross(&world_up).normalized();
        let up_flat = right_flat.cross(&forward).normalized();
        let right = rotate_around_axis(right_flat, forward, twist_rad);
        let up = rotate_around_axis(up_flat, forward, twist_rad);

        samples.push(SplineSample {
            position,
            forward,
            right,
            up,
            twist,
            t: global_t,
        });
    }

    samples
}

/// Sample an open Catmull-Rom spline at approximately `spacing` meters apart.
///
/// For open splines, phantom control points are created by reflecting
/// the first and last interior segments outward.
pub fn sample_open_spline(points: &[SplineControlPoint], spacing: f32) -> Vec<SplineSample> {
    let n = points.len();
    if n < 2 {
        return Vec::new();
    }

    // Build extended point list with phantom endpoints
    let phantom_start = SplineControlPoint {
        position: points[0].position * 2.0 - points[1].position,
        twist: points[0].twist,
    };
    let phantom_end = SplineControlPoint {
        position: points[n - 1].position * 2.0 - points[n - 2].position,
        twist: points[n - 1].twist,
    };

    let mut extended = Vec::with_capacity(n + 2);
    extended.push(phantom_start);
    extended.extend_from_slice(points);
    extended.push(phantom_end);

    // Now we have extended.len() = n + 2, with n + 1 segments between real points
    // but actually n - 1 segments between the original n points.
    let num_segs = n - 1;
    let world_up = Vec3::UP;

    // Estimate total length
    let segments_per_cp = 20;
    let mut total_length = 0.0_f32;
    let mut prev_pos = extended[1].position; // first real point
    for seg in 0..num_segs {
        let p0 = extended[seg].position;
        let p1 = extended[seg + 1].position;
        let p2 = extended[seg + 2].position;
        let p3 = extended[seg + 3].position;
        for j in 1..=segments_per_cp {
            let t = j as f32 / segments_per_cp as f32;
            let pos = catmull_rom(p0, p1, p2, p3, t);
            total_length += (pos - prev_pos).length();
            prev_pos = pos;
        }
    }

    let num_samples = (total_length / spacing).ceil() as usize;
    let num_samples = num_samples.max(10);
    // +1 so we include both endpoints
    let total_pts = num_samples + 1;
    let mut samples = Vec::with_capacity(total_pts);

    for i in 0..total_pts {
        let global_t = i as f32 / num_samples as f32; // [0, 1]
        let scaled = global_t * num_segs as f32;
        let seg = (scaled.floor() as usize).min(num_segs - 1);
        let local_t = scaled - seg as f32;

        let p0 = extended[seg].position;
        let p1 = extended[seg + 1].position;
        let p2 = extended[seg + 2].position;
        let p3 = extended[seg + 3].position;

        let position = catmull_rom(p0, p1, p2, p3, local_t);

        // Forward tangent
        let dt = 0.001;
        let next_t = local_t + dt;
        let pos_next = if next_t <= 1.0 {
            catmull_rom(p0, p1, p2, p3, next_t)
        } else if seg + 1 < num_segs {
            let np0 = extended[seg + 1].position;
            let np1 = extended[seg + 2].position;
            let np2 = extended[seg + 3].position;
            let np3 = extended[seg + 4].position;
            catmull_rom(np0, np1, np2, np3, next_t - 1.0)
        } else {
            catmull_rom(p0, p1, p2, p3, 1.0)
        };

        let forward = (pos_next - position).normalized();

        // Interpolate twist using Catmull-Rom (C1 continuous, matches position)
        let tw0 = extended[seg].twist;
        let tw1 = extended[seg + 1].twist;
        let tw2 = extended[seg + 2].twist;
        let tw3 = extended[seg + 3].twist;
        let twist = catmull_rom_scalar(tw0, tw1, tw2, tw3, local_t);
        let twist_rad = twist.to_radians();

        let right_flat = forward.cross(&world_up).normalized();
        let up_flat = right_flat.cross(&forward).normalized();
        let right = rotate_around_axis(right_flat, forward, twist_rad);
        let up = rotate_around_axis(up_flat, forward, twist_rad);

        samples.push(SplineSample {
            position,
            forward,
            right,
            up,
            twist,
            t: global_t,
        });
    }

    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_square_loop() -> Vec<SplineControlPoint> {
        vec![
            SplineControlPoint { position: Vec3::new(0.0, 0.0, 0.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(10.0, 0.0, 0.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(10.0, 0.0, 10.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(0.0, 0.0, 10.0), twist: 0.0 },
        ]
    }

    #[test]
    fn closed_spline_produces_samples() {
        let pts = make_square_loop();
        let samples = sample_closed_spline(&pts, 1.0);
        assert!(samples.len() > 10);
        // t values should span [0, 1)
        assert!(samples.first().unwrap().t < 0.01);
        assert!(samples.last().unwrap().t < 1.0);
    }

    #[test]
    fn open_spline_produces_samples() {
        let pts = vec![
            SplineControlPoint { position: Vec3::new(0.0, 0.0, 0.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(5.0, 0.0, 0.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(10.0, 0.0, 0.0), twist: 0.0 },
        ];
        let samples = sample_open_spline(&pts, 1.0);
        assert!(samples.len() >= 10);
        // Endpoints should be near the original endpoints
        let first = samples.first().unwrap();
        let last = samples.last().unwrap();
        assert!((first.position - pts[0].position).length() < 0.1);
        assert!((last.position - pts[2].position).length() < 0.5);
    }

    #[test]
    fn too_few_points_returns_empty() {
        let pts = vec![
            SplineControlPoint { position: Vec3::new(0.0, 0.0, 0.0), twist: 0.0 },
            SplineControlPoint { position: Vec3::new(1.0, 0.0, 0.0), twist: 0.0 },
        ];
        assert!(sample_closed_spline(&pts, 1.0).is_empty());
    }
}
