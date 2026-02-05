//! Pure keyframe evaluation — binary search + interpolation

use crate::clip::{AnimationTrack, Interpolation};

/// Sample a track at a given time, returning the interpolated value.
///
/// Uses binary search to find the surrounding keyframes, then interpolates
/// according to the track's interpolation mode.
pub fn sample_track(track: &AnimationTrack, time: f64) -> [f32; 3] {
    let keyframes = &track.keyframes;

    if keyframes.is_empty() {
        return [0.0; 3];
    }

    // Before first keyframe — clamp to first value
    if time <= keyframes[0].time {
        return keyframes[0].value;
    }

    // After last keyframe — clamp to last value
    let last = &keyframes[keyframes.len() - 1];
    if time >= last.time {
        return last.value;
    }

    // Binary search for the interval containing `time`
    let idx = match keyframes.binary_search_by(|kf| kf.time.partial_cmp(&time).unwrap()) {
        Ok(i) => return keyframes[i].value, // exact match
        Err(i) => i, // insertion point — time is between [i-1] and [i]
    };

    let prev = &keyframes[idx - 1];
    let next = &keyframes[idx];

    // Normalized interpolation factor
    let span = next.time - prev.time;
    if span <= 0.0 {
        return prev.value;
    }
    let t = ((time - prev.time) / span) as f32;

    match track.interpolation {
        Interpolation::Step => prev.value,
        Interpolation::Linear => lerp_array(prev.value, next.value, t),
        Interpolation::CubicSpline => {
            let out_tan = prev.out_tangent.unwrap_or([0.0; 3]);
            let in_tan = next.in_tangent.unwrap_or([0.0; 3]);
            let dt = span as f32;
            cubic_hermite(prev.value, out_tan, next.value, in_tan, dt, t)
        }
    }
}

/// Component-wise linear interpolation between two [f32; 3] arrays.
pub fn lerp_array(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Cubic Hermite spline interpolation.
///
/// `p0`, `m0`: start value and outgoing tangent (scaled by `dt`)
/// `p1`, `m1`: end value and incoming tangent (scaled by `dt`)
/// `dt`: time span of the interval (for tangent scaling)
/// `t`: normalized [0..1] parameter
pub fn cubic_hermite(
    p0: [f32; 3],
    m0: [f32; 3],
    p1: [f32; 3],
    m1: [f32; 3],
    dt: f32,
    t: f32,
) -> [f32; 3] {
    let t2 = t * t;
    let t3 = t2 * t;

    // Hermite basis functions
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;

    let mut result = [0.0f32; 3];
    for i in 0..3 {
        result[i] = h00 * p0[i] + h10 * (m0[i] * dt) + h01 * p1[i] + h11 * (m1[i] * dt);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{AnimationTrack, Interpolation, Keyframe, TrackTarget};

    fn make_track(interp: Interpolation, keyframes: Vec<Keyframe>) -> AnimationTrack {
        AnimationTrack {
            target: TrackTarget::Position,
            interpolation: interp,
            keyframes,
        }
    }

    #[test]
    fn sample_empty_track_returns_zero() {
        let track = make_track(Interpolation::Linear, vec![]);
        assert_eq!(sample_track(&track, 0.5), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn sample_before_first_keyframe_clamps() {
        let track = make_track(
            Interpolation::Linear,
            vec![Keyframe {
                time: 1.0,
                value: [5.0, 10.0, 15.0],
                in_tangent: None,
                out_tangent: None,
            }],
        );
        assert_eq!(sample_track(&track, 0.0), [5.0, 10.0, 15.0]);
    }

    #[test]
    fn sample_after_last_keyframe_clamps() {
        let track = make_track(
            Interpolation::Linear,
            vec![
                Keyframe { time: 0.0, value: [0.0, 0.0, 0.0], in_tangent: None, out_tangent: None },
                Keyframe { time: 1.0, value: [10.0, 20.0, 30.0], in_tangent: None, out_tangent: None },
            ],
        );
        assert_eq!(sample_track(&track, 5.0), [10.0, 20.0, 30.0]);
    }

    #[test]
    fn sample_linear_midpoint() {
        let track = make_track(
            Interpolation::Linear,
            vec![
                Keyframe { time: 0.0, value: [0.0, 0.0, 0.0], in_tangent: None, out_tangent: None },
                Keyframe { time: 2.0, value: [10.0, 20.0, 30.0], in_tangent: None, out_tangent: None },
            ],
        );
        let v = sample_track(&track, 1.0);
        assert!((v[0] - 5.0).abs() < 1e-5);
        assert!((v[1] - 10.0).abs() < 1e-5);
        assert!((v[2] - 15.0).abs() < 1e-5);
    }

    #[test]
    fn sample_step_holds_previous() {
        let track = make_track(
            Interpolation::Step,
            vec![
                Keyframe { time: 0.0, value: [1.0, 2.0, 3.0], in_tangent: None, out_tangent: None },
                Keyframe { time: 1.0, value: [4.0, 5.0, 6.0], in_tangent: None, out_tangent: None },
            ],
        );
        // At t=0.5, Step should still hold the first keyframe value
        assert_eq!(sample_track(&track, 0.5), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn sample_exact_keyframe_time() {
        let track = make_track(
            Interpolation::Linear,
            vec![
                Keyframe { time: 0.0, value: [0.0, 0.0, 0.0], in_tangent: None, out_tangent: None },
                Keyframe { time: 1.0, value: [10.0, 10.0, 10.0], in_tangent: None, out_tangent: None },
                Keyframe { time: 2.0, value: [20.0, 20.0, 20.0], in_tangent: None, out_tangent: None },
            ],
        );
        assert_eq!(sample_track(&track, 1.0), [10.0, 10.0, 10.0]);
    }

    #[test]
    fn sample_cubic_hermite_endpoints() {
        // With zero tangents, cubic hermite at t=0 and t=1 should give exact endpoints
        let track = make_track(
            Interpolation::CubicSpline,
            vec![
                Keyframe {
                    time: 0.0,
                    value: [0.0, 0.0, 0.0],
                    in_tangent: Some([0.0; 3]),
                    out_tangent: Some([0.0; 3]),
                },
                Keyframe {
                    time: 1.0,
                    value: [10.0, 10.0, 10.0],
                    in_tangent: Some([0.0; 3]),
                    out_tangent: Some([0.0; 3]),
                },
            ],
        );
        // Should clamp to first at t=0
        assert_eq!(sample_track(&track, 0.0), [0.0, 0.0, 0.0]);
        // Should clamp to last at t=1
        assert_eq!(sample_track(&track, 1.0), [10.0, 10.0, 10.0]);
    }
}
