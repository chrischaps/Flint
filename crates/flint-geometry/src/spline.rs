//! Pure spline math — Catmull-Rom path sampling with twist (banking).
//!
//! Provides open and closed spline evaluation from control points,
//! producing evenly-spaced samples with position, forward, right, up
//! basis vectors and parametric t values.

use flint_core::Vec3;

/// A single control point on a Catmull-Rom spline.
#[derive(Debug, Clone)]
pub struct SplineControlPoint {
    pub position: Vec3,
    /// Twist (banking) in degrees around the forward axis.
    pub twist: f32,
    /// When true, swept geometry (and its collider) is omitted from this CP
    /// to the next. Consecutive `gap_after` CPs merge into longer gaps.
    pub gap_after: bool,
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
        gap_after: false,
    };
    let phantom_end = SplineControlPoint {
        position: points[n - 1].position * 2.0 - points[n - 2].position,
        twist: points[n - 1].twist,
        gap_after: false,
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

        // Forward tangent: a step on, or past the path's end a step back,
        // since a step on from the last sample is the sample itself (#1016)
        let dt = 0.001;
        let next_t = local_t + dt;
        let forward = if next_t <= 1.0 {
            catmull_rom(p0, p1, p2, p3, next_t) - position
        } else if seg + 1 < num_segs {
            let np0 = extended[seg + 1].position;
            let np1 = extended[seg + 2].position;
            let np2 = extended[seg + 3].position;
            let np3 = extended[seg + 4].position;
            catmull_rom(np0, np1, np2, np3, next_t - 1.0) - position
        } else {
            position - catmull_rom(p0, p1, p2, p3, local_t - dt)
        }
        .normalized();

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

// ─── Spans: what holds from one control point to the next ─────────

/// A stretch of a spline over which one value holds: `start` inclusive,
/// `end` exclusive, in parametric `t`. A span whose `start` is after its
/// `end` wraps through 1 (a closed spline's run over its first point).
#[derive(Debug, Clone, PartialEq)]
pub struct Span<V> {
    pub start: f32,
    pub end: f32,
    pub value: V,
}

/// The runs of `values`, one per control point: what holds from that point
/// to the next (`None`: nothing does). Neighbours with equal values merge
/// into one span.
///
/// For a closed spline with N control points, point k's stretch is
/// t = k/N to (k+1)/N, and a run through the last point carries on round
/// into the first. For an open spline, k/(N-1) to (k+1)/(N-1): its last
/// point begins no stretch, so its value holds nowhere.
pub fn segment_spans<V: PartialEq + Clone>(values: &[Option<V>], closed: bool) -> Vec<Span<V>> {
    let n = values.len();
    if n < 2 {
        return Vec::new();
    }
    let num_segs = if closed { n } else { n - 1 };
    let mut spans: Vec<Span<V>> = Vec::new();

    let mut i = 0;
    while i < num_segs {
        let Some(value) = &values[i] else {
            i += 1;
            continue;
        };
        let mut end = i + 1;
        while end < num_segs && values[end].as_ref() == Some(value) {
            end += 1;
        }
        spans.push(Span {
            start: i as f32 / num_segs as f32,
            end: (end as f32 / num_segs as f32).min(1.0),
            value: value.clone(),
        });
        i = end;
    }

    // A closed spline's run that ends at 1 and one that starts at 0, of the
    // same value, are one run through the first point
    if closed && spans.len() >= 2 {
        let last = spans.len() - 1;
        if (spans[last].end - 1.0).abs() < 1e-6
            && spans[0].start.abs() < 1e-6
            && spans[last].value == spans[0].value
        {
            spans[0].start = spans[last].start;
            spans.pop();
        }
    }

    spans
}

/// Compute parametric t-ranges where `gap_after` is true: the
/// [`segment_spans`] of the points' `gap_after`.
pub fn compute_gap_ranges(points: &[SplineControlPoint], closed: bool) -> Vec<(f32, f32)> {
    let gaps: Vec<Option<()>> = points.iter().map(|p| p.gap_after.then_some(())).collect();
    segment_spans(&gaps, closed)
        .into_iter()
        .map(|s| (s.start, s.end))
        .collect()
}

/// Check if parametric t falls within any gap range.
///
/// Start is inclusive, end is exclusive.
pub fn is_in_gap(t: f32, gaps: &[(f32, f32)]) -> bool {
    gap_at(t, gaps).is_some()
}

/// Return the gap range containing t, or None.
///
/// Handles wrap-around ranges where start > end (for closed splines).
pub fn gap_at(t: f32, gaps: &[(f32, f32)]) -> Option<(f32, f32)> {
    let t_wrapped = ((t % 1.0) + 1.0) % 1.0;
    for &(start, end) in gaps {
        if start <= end {
            // Normal range
            if t_wrapped >= start && t_wrapped < end {
                return Some((start, end));
            }
        } else {
            // Wrap-around range (e.g. 0.9..0.1)
            if t_wrapped >= start || t_wrapped < end {
                return Some((start, end));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_points(positions: &[[f32; 3]], twist: f32) -> Vec<SplineControlPoint> {
        positions
            .iter()
            .map(|&p| SplineControlPoint {
                position: Vec3::from_array(p),
                twist,
                gap_after: false,
            })
            .collect()
    }

    fn is_unit(v: Vec3) -> bool {
        (v.length() - 1.0).abs() < 1e-4
    }

    #[test]
    fn every_open_sample_has_a_unit_frame() {
        // #1016: the last sample's forward was a step on from itself, zero
        for (positions, twist) in [
            (
                &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [10.0, 0.0, 0.0]][..],
                0.0,
            ),
            (
                &[[0.0, 0.0, 0.0], [3.0, 0.5, 0.0], [6.0, 0.0, 2.0]][..],
                15.0,
            ),
            (&[[0.0, 0.0, 0.0], [4.0, 1.0, 3.0]][..], -30.0),
        ] {
            let samples = sample_open_spline(&open_points(positions, twist), 0.5);
            for (i, s) in samples.iter().enumerate() {
                assert!(
                    is_unit(s.forward) && is_unit(s.right) && is_unit(s.up),
                    "sample {i} of {}: {s:?}",
                    samples.len()
                );
            }
        }
    }

    #[test]
    fn an_open_paths_last_forward_points_on_along_it() {
        let pts = open_points(&[[0.0, 0.0, 0.0], [3.0, 0.5, 0.0], [6.0, 0.0, 2.0]], 0.0);
        let samples = sample_open_spline(&pts, 0.5);
        let [.., before, last] = &samples[..] else {
            panic!("too few samples");
        };
        let chord = (last.position - before.position).normalized();
        assert!(last.forward.dot(&chord) > 0.99, "{last:?} vs {chord:?}");
        // and it carries on from the sample before's, not a turn
        assert!(last.forward.dot(&before.forward) > 0.99);
    }

    fn make_square_loop() -> Vec<SplineControlPoint> {
        vec![
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
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
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(5.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
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
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(1.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
        ];
        assert!(sample_closed_spline(&pts, 1.0).is_empty());
    }

    #[test]
    fn gap_ranges_single_gap_closed() {
        let pts = vec![
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: true,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
        ];
        let ranges = compute_gap_ranges(&pts, true);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0 - 0.25).abs() < 0.01); // CP1 → t=1/4
        assert!((ranges[0].1 - 0.50).abs() < 0.01); // CP2 → t=2/4
    }

    #[test]
    fn gap_ranges_consecutive_merge() {
        let pts = vec![
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: true,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: true,
            },
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
        ];
        let ranges = compute_gap_ranges(&pts, true);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0 - 0.25).abs() < 0.01); // CP1
        assert!((ranges[0].1 - 0.75).abs() < 0.01); // CP3
    }

    #[test]
    fn gap_ranges_open_spline() {
        let pts = vec![
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: true,
            },
            SplineControlPoint {
                position: Vec3::new(5.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
        ];
        let ranges = compute_gap_ranges(&pts, false);
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].0.abs() < 0.01); // CP0 → t=0
        assert!((ranges[0].1 - 0.50).abs() < 0.01); // CP1 → t=1/2
    }

    #[test]
    fn is_in_gap_basic() {
        let gaps = vec![(0.25, 0.50)];
        assert!(!is_in_gap(0.1, &gaps));
        assert!(is_in_gap(0.3, &gaps));
        assert!(!is_in_gap(0.5, &gaps)); // end exclusive
        assert!(!is_in_gap(0.7, &gaps));
    }

    #[test]
    fn gap_at_returns_range() {
        let gaps = vec![(0.25, 0.50)];
        assert!(gap_at(0.1, &gaps).is_none());
        let g = gap_at(0.3, &gaps).unwrap();
        assert!((g.0 - 0.25).abs() < 0.01);
        assert!((g.1 - 0.50).abs() < 0.01);
    }

    #[test]
    fn gap_wraparound_closed() {
        // Gaps at CP3 (last) that wraps to CP0
        let pts = vec![
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 0.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(10.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: false,
            },
            SplineControlPoint {
                position: Vec3::new(0.0, 0.0, 10.0),
                twist: 0.0,
                gap_after: true,
            },
        ];
        let ranges = compute_gap_ranges(&pts, true);
        assert_eq!(ranges.len(), 1);
        // Last CP gap: t=0.75 to 1.0 — no merge since CP0 doesn't have gap_after
        assert!((ranges[0].0 - 0.75).abs() < 0.01);
        assert!((ranges[0].1 - 1.0).abs() < 0.01);
        assert!(is_in_gap(0.8, &ranges));
        assert!(!is_in_gap(0.1, &ranges));
    }

    #[test]
    fn spans_merge_equal_neighbours_and_wrap_only_when_equal() {
        let v = |s: &[Option<i32>]| s.to_vec();
        // Closed, five points: 1 1 2 _ 1 -> the last 1 runs round into the first two
        let spans = segment_spans(&v(&[Some(1), Some(1), Some(2), None, Some(1)]), true);
        let got: Vec<_> = spans.iter().map(|s| (s.start, s.end, s.value)).collect();
        assert_eq!(got, vec![(0.8, 0.4, 1), (0.4, 0.6, 2)]);
        // A different value at the end doesn't merge round
        let spans = segment_spans(&v(&[Some(1), None, None, Some(3)]), true);
        let got: Vec<_> = spans.iter().map(|s| (s.start, s.end, s.value)).collect();
        assert_eq!(got, vec![(0.0, 0.25, 1), (0.75, 1.0, 3)]);
        // Open: the last point begins no stretch
        let spans = segment_spans(&v(&[Some(1), Some(2), Some(2)]), false);
        let got: Vec<_> = spans.iter().map(|s| (s.start, s.end, s.value)).collect();
        assert_eq!(got, vec![(0.0, 0.5, 1), (0.5, 1.0, 2)]);
        assert!(segment_spans(&v(&[None, None, Some(9)]), false).is_empty());
        assert!(segment_spans(&v(&[Some(1)]), true).is_empty());
    }

    /// `compute_gap_ranges` as it was before #855 rewrote it over
    /// [`segment_spans`], verbatim: the oracle the new one must agree with.
    fn gap_ranges_before_855(points: &[SplineControlPoint], closed: bool) -> Vec<(f32, f32)> {
        let n = points.len();
        if n < 2 {
            return Vec::new();
        }
        let num_segs = if closed { n } else { n - 1 };
        let mut ranges: Vec<(f32, f32)> = Vec::new();
        let mut i = 0;
        while i < n {
            if points[i].gap_after {
                let start_t = i as f32 / num_segs as f32;
                let mut end_idx = i + 1;
                while end_idx < n && points[end_idx].gap_after {
                    end_idx += 1;
                }
                let end_t = (end_idx as f32 / num_segs as f32).min(1.0);
                ranges.push((start_t, end_t));
                i = end_idx;
            } else {
                i += 1;
            }
        }
        if closed && ranges.len() >= 2 {
            let last = ranges.len() - 1;
            if (ranges[last].1 - 1.0).abs() < 1e-6 && ranges[0].0.abs() < 1e-6 {
                let merged_start = ranges[last].0;
                let merged_end = ranges[0].1;
                ranges[0] = (merged_start, merged_end);
                ranges.pop();
            }
        }
        ranges
    }

    #[test]
    fn every_gap_pattern_gives_the_ranges_it_gave_before() {
        for n in 2..=9usize {
            for closed in [true, false] {
                for mask in 0u32..(1 << n) {
                    let pts: Vec<_> = (0..n)
                        .map(|i| SplineControlPoint {
                            position: Vec3::new(i as f32, 0.0, 0.0),
                            twist: 0.0,
                            gap_after: mask & (1 << i) != 0,
                        })
                        .collect();
                    let old = gap_ranges_before_855(&pts, closed);
                    // The old one also gave an open spline's last point an
                    // empty range (1, 1), which no t is in
                    let old: Vec<_> = old.into_iter().filter(|r| r.0 != r.1).collect();
                    assert_eq!(
                        compute_gap_ranges(&pts, closed),
                        old,
                        "n {n}, closed {closed}, gaps {mask:b}"
                    );
                }
            }
        }
    }
}
