//! Camera-cut detection (#343, #474): notice when the frame's camera no
//! longer follows on from the last one, so styles that remember earlier
//! frames (ASCII's steady glyphs, #277) start over instead of carrying the
//! old shot's glyphs into the new one.
//!
//! A cut is a step that is both large and sudden. Large: in one frame the
//! camera moves more than [`CUT_DISTANCE`] metres, turns more than
//! [`CUT_ANGLE_DEG`] degrees, or zooms by more than [`CUT_ZOOM`] times
//! either way (a punch-in that changes only the FOV or the orthographic
//! height). Sudden: that step's rate is more than [`CUT_RATIO`] times the
//! rate of the frame before. The second test keeps a fast chase camera, a
//! quick dolly zoom, or a hitch (which stretches one frame's step but not
//! its rate) from resetting every frame and losing its stability for good.
//! A switch between perspective and orthographic is always a cut.
//!
//! A missed cut costs a few frames of the old shot fading out; a false one
//! costs one frame drawn without history. Both are mild, so the thresholds
//! lean towards catching cuts.

use flint_render::Camera;

/// Metres in one frame past which a move may be a cut.
pub const CUT_DISTANCE: f32 = 2.0;
/// Degrees in one frame past which a turn may be a cut.
pub const CUT_ANGLE_DEG: f32 = 30.0;
/// Times bigger or smaller the view may get in one frame before a zoom may
/// be a cut. The view's size is `tan(fov / 2)` for a perspective camera and
/// the half-height for an orthographic one, so 1.25 is a 50 to 40 degree
/// punch-in (or an ortho height from 10 to 8). Every zoom carries some of
/// the old frame over (#474, `examples/ascii_cuts.scene.toml`: 1,288 of
/// 14,400 cells at 1.1x, 2,974 at 2x) with no knee, and a reset moves the
/// flips onto the cut frame rather than saving any, so the line is intent:
/// under it a step is more likely an effect (a sprint's FOV kick), over it
/// a new framing. The book's Stylize Passes, Camera cuts, has the table.
pub const CUT_ZOOM: f32 = 1.25;
/// How many times faster than the frame before a large step must be.
pub const CUT_RATIO: f32 = 4.0;

/// The shortest frame a rate is taken over, so a paused clock (dt = 0)
/// gives a huge rate instead of a division by zero.
const MIN_DT: f32 = 1.0e-4;

/// What the detector compares frame to frame: where the camera is, where
/// it looks, and how much of the world its view holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    /// The view's half-height: at unit distance (`tan(fov / 2)`) for a
    /// perspective camera, in world units for an orthographic one.
    pub size: f32,
    pub orthographic: bool,
}

impl Shot {
    /// The shot a camera frames, sized as its projection sizes it.
    pub fn of(camera: &Camera) -> Self {
        let tan_half = (camera.fov.to_radians() / 2.0).tan();
        let size = if !camera.orthographic {
            tan_half
        } else if camera.ortho_height > 0.0 {
            camera.ortho_height
        } else {
            camera.distance * tan_half
        };
        Self {
            position: camera.position_array(),
            forward: camera.forward_vector(),
            size,
            orthographic: camera.orthographic,
        }
    }
}

/// Watches the camera frame to frame; [`observe`](Self::observe) says when
/// a frame is a cut.
#[derive(Debug, Clone)]
pub struct CameraCutDetector {
    /// Off: `observe` never reports a cut (a script turns it off with
    /// `set_camera_cut_detection(false)` when it resets at its own cuts).
    pub enabled: bool,
    last: Option<Shot>,
    /// The last frame's move, turn and zoom rates (m/s, degrees/s, ln of
    /// the size per second) that was not a cut: a cut's jump is not motion
    /// the next frame compares against.
    rates: [f32; 3],
}

impl Default for CameraCutDetector {
    fn default() -> Self {
        Self {
            enabled: true,
            last: None,
            rates: [0.0; 3],
        }
    }
}

impl CameraCutDetector {
    /// Forget the last shot: the next frame starts the watch (a new scene,
    /// whose history is reset anyway).
    pub fn forget(&mut self) {
        self.last = None;
        self.rates = [0.0; 3];
    }

    /// This frame's shot, `dt` seconds after the last. True when the frame
    /// is a cut. The first frame after `forget` never is.
    pub fn observe(&mut self, shot: Shot, dt: f32) -> bool {
        let Some(last) = self.last.replace(shot) else {
            return false;
        };
        let distance = length(sub(shot.position, last.position));
        let angle = angle_deg(last.forward, shot.forward);
        let zoom = zoom_step(last.size, shot.size);
        let dt = dt.max(MIN_DT);
        let rates = [distance / dt, angle / dt, zoom / dt];
        let jumped = distance > CUT_DISTANCE && rates[0] > CUT_RATIO * self.rates[0];
        let turned = angle > CUT_ANGLE_DEG && rates[1] > CUT_RATIO * self.rates[1];
        let zoomed = zoom > CUT_ZOOM.ln() && rates[2] > CUT_RATIO * self.rates[2];
        let switched = shot.orthographic != last.orthographic;
        let cut = jumped || turned || zoomed || switched;
        if !cut {
            self.rates = rates;
        }
        cut && self.enabled
    }
}

/// How far the view's size moved, as |ln(b / a)|: a zoom in and the same
/// zoom out are the same step. 0 when either size is unusable.
fn zoom_step(a: f32, b: f32) -> f32 {
    if !(a > 0.0 && b > 0.0 && a.is_finite() && b.is_finite()) {
        return 0.0;
    }
    (b / a).ln().abs()
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

    /// A perspective shot at 50 degrees
    fn at(position: [f32; 3], forward: [f32; 3]) -> Shot {
        Shot {
            position,
            forward,
            size: fov(50.0),
            orthographic: false,
        }
    }

    fn fov(deg: f32) -> f32 {
        (deg.to_radians() / 2.0).tan()
    }

    fn zoomed(size: f32) -> Shot {
        Shot {
            size,
            ..at([0.0; 3], FWD)
        }
    }

    #[test]
    fn first_frame_is_never_a_cut() {
        let mut d = CameraCutDetector::default();
        assert!(!d.observe(at([100.0, 0.0, 0.0], FWD), DT));
    }

    #[test]
    fn a_teleport_is_a_cut() {
        let mut d = CameraCutDetector::default();
        d.observe(at([0.0, 1.0, 0.0], FWD), DT);
        d.observe(at([0.01, 1.0, 0.0], FWD), DT);
        assert!(d.observe(at([30.0, 1.0, 0.0], FWD), DT));
        // The shot after the cut follows on again
        assert!(!d.observe(at([30.01, 1.0, 0.0], FWD), DT));
    }

    #[test]
    fn a_sharp_turn_is_a_cut_and_a_steady_orbit_is_not() {
        let mut d = CameraCutDetector::default();
        for i in 0..60 {
            assert!(
                !d.observe(at([0.0; 3], yaw(i as f32 * 0.5)), DT),
                "frame {i}"
            );
        }
        assert!(d.observe(at([0.0; 3], yaw(29.5 + 90.0)), DT));
    }

    #[test]
    fn a_fast_camera_moving_steadily_is_not_cut() {
        // 200 m/s: over 3 m a frame, every frame
        let mut d = CameraCutDetector::default();
        let mut x = 0.0;
        assert!(!d.observe(at([x, 0.0, 0.0], FWD), DT));
        x += 0.1; // ramping up from rest over a few frames
        assert!(!d.observe(at([x, 0.0, 0.0], FWD), DT));
        for step in [0.3, 1.0, 2.5, 3.33, 3.33, 3.33, 3.33] {
            x += step;
            assert!(!d.observe(at([x, 0.0, 0.0], FWD), DT), "step {step}");
        }
        // ...but a respawn from it still is
        assert!(d.observe(at([x + 400.0, 0.0, 0.0], FWD), DT));
    }

    #[test]
    fn a_hitch_stretches_the_step_not_the_rate() {
        let mut d = CameraCutDetector::default();
        let speed = 12.0; // m/s
        let mut x = 0.0;
        d.observe(at([x, 0.0, 0.0], FWD), DT);
        x += speed * DT;
        d.observe(at([x, 0.0, 0.0], FWD), DT);
        // A 250 ms frame: 3 m in one step, at the same speed
        x += speed * 0.25;
        assert!(!d.observe(at([x, 0.0, 0.0], FWD), 0.25));
    }

    #[test]
    fn small_moves_never_cut_however_sudden() {
        let mut d = CameraCutDetector::default();
        d.observe(at([0.0; 3], FWD), DT);
        assert!(!d.observe(at([1.5, 0.0, 0.0], yaw(25.0)), DT));
        // nor a small zoom: 50 to 45 degrees is 1.13x
        assert!(!d.observe(
            Shot {
                size: fov(45.0),
                ..at([1.5, 0.0, 0.0], yaw(25.0))
            },
            DT
        ));
    }

    #[test]
    fn a_punch_in_is_a_cut_and_so_is_the_zoom_back_out() {
        let mut d = CameraCutDetector::default();
        d.observe(zoomed(fov(50.0)), DT);
        d.observe(zoomed(fov(50.0)), DT);
        // 50 to 30 degrees: the view 1.74x smaller, nothing else moved
        assert!(d.observe(zoomed(fov(30.0)), DT));
        assert!(!d.observe(zoomed(fov(30.0)), DT));
        assert!(d.observe(zoomed(fov(50.0)), DT));
    }

    #[test]
    fn an_orthographic_zoom_is_watched_on_its_height() {
        let ortho = |size| Shot {
            orthographic: true,
            ..zoomed(size)
        };
        let mut d = CameraCutDetector::default();
        d.observe(ortho(10.0), DT);
        d.observe(ortho(10.0), DT);
        assert!(!d.observe(ortho(9.0), DT), "1.11x is a nudge");
        assert!(d.observe(ortho(4.0), DT), "2.25x is a cut");
    }

    #[test]
    fn a_dolly_zoom_or_a_scroll_wheel_is_not_cut() {
        // FOV 60 to 20 degrees over a second: about 1.9% a frame
        let mut d = CameraCutDetector::default();
        for i in 0..=60 {
            let deg = 60.0 - 40.0 * i as f32 / 60.0;
            assert!(!d.observe(zoomed(fov(deg)), DT), "frame {i}");
        }
        // A fast steady zoom, 1.3x a frame after a short ramp, stays motion
        let mut d = CameraCutDetector::default();
        let mut size = 1.0;
        for factor in [1.0, 1.05, 1.2, 1.3, 1.3, 1.3, 1.3] {
            size /= factor;
            assert!(!d.observe(zoomed(size), DT), "factor {factor}");
        }
    }

    #[test]
    fn switching_projection_is_always_a_cut() {
        let mut d = CameraCutDetector::default();
        let shot = zoomed(fov(50.0));
        d.observe(shot, DT);
        assert!(d.observe(
            Shot {
                orthographic: true,
                ..shot
            },
            DT
        ));
        assert!(d.observe(shot, DT));
    }

    #[test]
    fn a_shot_is_sized_as_its_projection_sizes_it() {
        let mut camera = Camera {
            fov: 60.0,
            distance: 20.0,
            ..Default::default()
        };
        let tan30 = 30.0f32.to_radians().tan();
        assert!((Shot::of(&camera).size - tan30).abs() < 1e-6);
        camera.orthographic = true;
        assert!((Shot::of(&camera).size - 20.0 * tan30).abs() < 1e-5);
        camera.ortho_height = 7.0;
        assert_eq!(Shot::of(&camera).size, 7.0);
        assert!(Shot::of(&camera).orthographic);
    }

    #[test]
    fn disabled_reports_nothing_and_forget_restarts() {
        let mut d = CameraCutDetector {
            enabled: false,
            ..Default::default()
        };
        d.observe(at([0.0; 3], FWD), DT);
        assert!(!d.observe(at([50.0, 0.0, 0.0], FWD), DT));
        d.enabled = true;
        d.forget();
        assert!(!d.observe(at([-50.0, 0.0, 0.0], FWD), DT));
        assert!(d.observe(at([50.0, 0.0, 0.0], FWD), DT));
    }

    #[test]
    fn a_paused_clock_does_not_divide_by_zero() {
        let mut d = CameraCutDetector::default();
        d.observe(at([0.0; 3], FWD), DT);
        assert!(!d.observe(at([0.0; 3], FWD), 0.0));
        assert!(d.observe(at([10.0, 0.0, 0.0], FWD), 0.0));
        // a degenerate size is no zoom
        assert!(!d.observe(
            Shot {
                size: 0.0,
                ..at([10.0, 0.0, 0.0], FWD)
            },
            0.0
        ));
    }
}
