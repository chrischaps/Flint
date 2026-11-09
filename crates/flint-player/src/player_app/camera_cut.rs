//! Camera-cut detection (#343): notice when the frame's camera no longer
//! follows on from the last one, so styles that remember earlier frames
//! (ASCII's steady glyphs, #277) start over instead of carrying the old
//! shot's glyphs into the new one.
//!
//! A cut is a step that is both large and sudden. Large: the camera moves
//! more than [`CUT_DISTANCE`] metres, or turns more than [`CUT_ANGLE_DEG`]
//! degrees, in one frame. Sudden: that step's rate is more than
//! [`CUT_RATIO`] times the rate of the frame before. The second test keeps a
//! fast chase camera (or a hitch, which stretches one frame's step but not
//! its rate) from resetting every frame and losing its stability for good.
//!
//! A missed cut costs a few frames of the old shot fading out; a false one
//! costs one frame drawn without history. Both are mild, so the thresholds
//! lean towards catching cuts. FOV and orthographic zoom are not watched.

/// Metres in one frame past which a move may be a cut.
pub const CUT_DISTANCE: f32 = 2.0;
/// Degrees in one frame past which a turn may be a cut.
pub const CUT_ANGLE_DEG: f32 = 30.0;
/// How many times faster than the frame before a large step must be.
pub const CUT_RATIO: f32 = 4.0;

/// The shortest frame a rate is taken over, so a paused clock (dt = 0)
/// gives a huge rate instead of a division by zero.
const MIN_DT: f32 = 1.0e-4;

#[derive(Debug, Clone, Copy)]
struct Pose {
    position: [f32; 3],
    forward: [f32; 3],
}

/// Watches the camera frame to frame; [`observe`](Self::observe) says when
/// a frame is a cut.
#[derive(Debug, Clone)]
pub struct CameraCutDetector {
    /// Off: `observe` never reports a cut (a script turns it off with
    /// `set_camera_cut_detection(false)` when it resets at its own cuts).
    pub enabled: bool,
    last: Option<Pose>,
    /// The last frame's move and turn rates (m/s, degrees/s) that was not a
    /// cut: a cut's jump is not motion the next frame compares against.
    rates: [f32; 2],
}

impl Default for CameraCutDetector {
    fn default() -> Self {
        Self {
            enabled: true,
            last: None,
            rates: [0.0, 0.0],
        }
    }
}

impl CameraCutDetector {
    /// Forget the last pose: the next frame starts the watch (a new scene,
    /// whose history is reset anyway).
    pub fn forget(&mut self) {
        self.last = None;
        self.rates = [0.0, 0.0];
    }

    /// This frame's camera, `dt` seconds after the last. True when the
    /// frame is a cut. The first frame after `forget` never is.
    pub fn observe(&mut self, position: [f32; 3], forward: [f32; 3], dt: f32) -> bool {
        let pose = Pose { position, forward };
        let Some(last) = self.last.replace(pose) else {
            return false;
        };
        let distance = length(sub(position, last.position));
        let angle = angle_deg(last.forward, forward);
        let dt = dt.max(MIN_DT);
        let rates = [distance / dt, angle / dt];
        let jumped = distance > CUT_DISTANCE && rates[0] > CUT_RATIO * self.rates[0];
        let turned = angle > CUT_ANGLE_DEG && rates[1] > CUT_RATIO * self.rates[1];
        let cut = jumped || turned;
        if !cut {
            self.rates = rates;
        }
        cut && self.enabled
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Angle between two directions in degrees (0 when either is degenerate).
fn angle_deg(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (length(a), length(b));
    if la < 1.0e-6 || lb < 1.0e-6 || !la.is_finite() || !lb.is_finite() {
        return 0.0;
    }
    let cos = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (la * lb);
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    const FWD: [f32; 3] = [0.0, 0.0, -1.0];

    fn yaw(deg: f32) -> [f32; 3] {
        let r = deg.to_radians();
        [-r.sin(), 0.0, -r.cos()]
    }

    #[test]
    fn first_frame_is_never_a_cut() {
        let mut d = CameraCutDetector::default();
        assert!(!d.observe([100.0, 0.0, 0.0], FWD, DT));
    }

    #[test]
    fn a_teleport_is_a_cut() {
        let mut d = CameraCutDetector::default();
        d.observe([0.0, 1.0, 0.0], FWD, DT);
        d.observe([0.01, 1.0, 0.0], FWD, DT);
        assert!(d.observe([30.0, 1.0, 0.0], FWD, DT));
        // The shot after the cut follows on again
        assert!(!d.observe([30.01, 1.0, 0.0], FWD, DT));
    }

    #[test]
    fn a_sharp_turn_is_a_cut_and_a_steady_orbit_is_not() {
        let mut d = CameraCutDetector::default();
        for i in 0..60 {
            assert!(!d.observe([0.0; 3], yaw(i as f32 * 0.5), DT), "frame {i}");
        }
        assert!(d.observe([0.0; 3], yaw(29.5 + 90.0), DT));
    }

    #[test]
    fn a_fast_camera_moving_steadily_is_not_cut() {
        // 200 m/s: over 3 m a frame, every frame
        let mut d = CameraCutDetector::default();
        let mut x = 0.0;
        assert!(!d.observe([x, 0.0, 0.0], FWD, DT));
        x += 0.1; // ramping up from rest over a few frames
        assert!(!d.observe([x, 0.0, 0.0], FWD, DT));
        for step in [0.3, 1.0, 2.5, 3.33, 3.33, 3.33, 3.33] {
            x += step;
            assert!(!d.observe([x, 0.0, 0.0], FWD, DT), "step {step}");
        }
        // ...but a respawn from it still is
        assert!(d.observe([x + 400.0, 0.0, 0.0], FWD, DT));
    }

    #[test]
    fn a_hitch_stretches_the_step_not_the_rate() {
        let mut d = CameraCutDetector::default();
        let speed = 12.0; // m/s
        let mut x = 0.0;
        d.observe([x, 0.0, 0.0], FWD, DT);
        x += speed * DT;
        d.observe([x, 0.0, 0.0], FWD, DT);
        // A 250 ms frame: 3 m in one step, at the same speed
        x += speed * 0.25;
        assert!(!d.observe([x, 0.0, 0.0], FWD, 0.25));
    }

    #[test]
    fn small_moves_never_cut_however_sudden() {
        let mut d = CameraCutDetector::default();
        d.observe([0.0; 3], FWD, DT);
        assert!(!d.observe([1.5, 0.0, 0.0], yaw(25.0), DT));
    }

    #[test]
    fn disabled_reports_nothing_and_forget_restarts() {
        let mut d = CameraCutDetector {
            enabled: false,
            ..Default::default()
        };
        d.observe([0.0; 3], FWD, DT);
        assert!(!d.observe([50.0, 0.0, 0.0], FWD, DT));
        d.enabled = true;
        d.forget();
        assert!(!d.observe([-50.0, 0.0, 0.0], FWD, DT));
        assert!(d.observe([50.0, 0.0, 0.0], FWD, DT));
    }

    #[test]
    fn a_paused_clock_does_not_divide_by_zero() {
        let mut d = CameraCutDetector::default();
        d.observe([0.0; 3], FWD, DT);
        assert!(!d.observe([0.0; 3], FWD, 0.0));
        assert!(d.observe([10.0, 0.0, 0.0], FWD, 0.0));
    }
}
