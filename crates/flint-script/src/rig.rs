//! The camera rig's state and arithmetic (#1158), with no Rhai in it: the
//! `camera_rig` module's functions (`api/camera_rig.rs`) set it, and the
//! drain ([`crate::context::ScriptCallContext::take_commands`]) lays this
//! frame's jitter over the camera the scripts asked for.
//!
//! Everything is a function of game time (`total_time`) and the values a
//! script gave, never of a step's `dt`: a still at `t` and the frame at
//! `t` of a `--frame-step` run see the same shake, and two runs the same
//! frames.

use crate::context::CameraOverrides;
use flint_core::EntityId;
use std::collections::HashMap;

/// Below this many metres a shake has died out.
const SHAKE_FLOOR: f64 = 1e-4;

/// A view shake's rotation at its start, degrees, for each metre of its
/// amplitude when the script names none: a 0.1 m kick turns the view 1
/// degree.
pub const SHAKE_DEGREES_PER_METRE: f64 = 10.0;

/// The hand-held camera at strength 1: its drift (metres, and degrees of
/// yaw, pitch and roll) and its footfall bob and sway (metres, degrees).
const HAND_DRIFT_M: f64 = 0.02;
const HAND_DRIFT_DEG: f64 = 0.6;
const HAND_ROLL_DEG: f64 = 0.8;
const HAND_BOB_M: f64 = 0.014;
const HAND_SWAY_M: f64 = 0.008;
const HAND_SWAY_ROLL_DEG: f64 = 0.4;

/// SplitMix64's finaliser: a well-mixed 64 bits from any.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The gradient at lattice point `i` for `seed`, in [-1, 1].
fn gradient(i: i64, seed: i64) -> f64 {
    let h = mix((i as u64) ^ mix(seed as u64));
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// Smooth seeded noise of one variable, in [-1, 1]: 1D gradient noise
/// (Perlin's, with his quintic fade), 0 at every whole `x`, about one swing
/// either way per unit. The same `x` and `seed` give the same value on
/// every run and machine.
pub fn noise(x: f64, seed: i64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let i = x.floor();
    let f = x - i;
    let i = i as i64;
    let a = gradient(i, seed) * f;
    let b = gradient(i.wrapping_add(1), seed) * (f - 1.0);
    let fade = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    // 1D gradient noise peaks at half a gradient: doubled, it fills [-1, 1]
    ((a + (b - a) * fade) * 2.0).clamp(-1.0, 1.0)
}

/// A decaying shake in the camera's own frame (`shake_view`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ViewShake {
    /// Metres at `started`.
    pub amplitude: f64,
    /// Game time of the last kick.
    pub started: f64,
    /// Swings a second, about.
    pub frequency: f64,
    /// How fast it dies: the amplitude falls by e every `1 / decay` s.
    pub decay: f64,
    /// Radians of turn for each metre of amplitude.
    pub turn_per_metre: f64,
    pub seed: i64,
}

impl ViewShake {
    /// Metres of shake at `t`; 0 once it has died (or before it began).
    pub fn amplitude_at(&self, t: f64) -> f64 {
        let age = t - self.started;
        if self.amplitude <= 0.0 || age < 0.0 {
            return 0.0;
        }
        let a = self.amplitude * (-self.decay.max(0.0) * age).exp();
        if a < SHAKE_FLOOR {
            0.0
        } else {
            a
        }
    }

    /// A kick at `t`: `amplitude` metres on top of what is left of the last,
    /// shaking at `frequency`, dying at `decay`, turning `degrees` at its
    /// start (`None`: [`SHAKE_DEGREES_PER_METRE`]).
    pub fn kick(
        &mut self,
        t: f64,
        amplitude: f64,
        frequency: f64,
        decay: f64,
        degrees: Option<f64>,
        seed: i64,
    ) {
        let left = self.amplitude_at(t);
        let amplitude = amplitude.max(0.0);
        self.amplitude = left + amplitude;
        self.started = t;
        self.frequency = frequency.max(0.0);
        self.decay = decay.max(0.0);
        self.turn_per_metre = match degrees {
            Some(d) if amplitude > 0.0 => d.to_radians() / amplitude,
            Some(_) => 0.0,
            None => SHAKE_DEGREES_PER_METRE.to_radians(),
        };
        self.seed = seed;
    }
}

/// A hand-held camera (`set_hand_held`): drift, and a walk's bob and sway.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HandHeld {
    /// 0 is off, 1 a person holding a camera.
    pub strength: f64,
    /// Footfalls a second; 0 stands still.
    pub steps: f64,
    pub seed: i64,
}

/// What the rig adds to a camera this frame: metres along its right and
/// up, and radians of yaw (to the right), pitch (up) and roll.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Jitter {
    pub right: f64,
    pub up: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

impl Jitter {
    fn is_none(&self) -> bool {
        *self == Jitter::default()
    }
}

/// The camera rig's state on the call context: the shake, the hand-held
/// camera, and which shot each script's list was in.
#[derive(Debug, Default)]
pub struct CameraRig {
    pub shake: ViewShake,
    pub hand: HandHeld,
    /// (entity, the list's hash) to (shot index, game time it began).
    shots: HashMap<(EntityId, u64), (i64, f64)>,
}

impl CameraRig {
    /// This frame's jitter at game time `t`.
    pub fn jitter(&self, t: f64) -> Jitter {
        let mut j = Jitter::default();
        let a = self.shake.amplitude_at(t);
        if a > 0.0 {
            let s = &self.shake;
            let x = t * s.frequency;
            let turn = a * s.turn_per_metre;
            j.right += a * noise(x, s.seed);
            j.up += a * noise(x, s.seed.wrapping_add(1));
            j.yaw += turn * noise(x, s.seed.wrapping_add(2));
            j.pitch += turn * noise(x, s.seed.wrapping_add(3));
            j.roll += turn * noise(x * 0.8, s.seed.wrapping_add(4));
        }
        let h = &self.hand;
        if h.strength > 0.0 {
            let k = h.strength;
            let seed = h.seed.wrapping_add(16);
            // A slow wander: never quite still, never in a rhythm
            j.right += k * HAND_DRIFT_M * noise(t * 0.35, seed);
            j.up += k * HAND_DRIFT_M * noise(t * 0.3, seed + 1);
            j.yaw += k * HAND_DRIFT_DEG.to_radians() * noise(t * 0.25, seed + 2);
            j.pitch += k * HAND_DRIFT_DEG.to_radians() * noise(t * 0.22, seed + 3);
            j.roll += k * HAND_ROLL_DEG.to_radians() * noise(t * 0.18, seed + 4);
            if h.steps > 0.0 {
                // Each footfall a dip, the body swaying over one foot then
                // the other: |sin| has a period of one step, sin of two
                let phase = std::f64::consts::PI * h.steps * t;
                let bob = phase.sin().abs() - std::f64::consts::FRAC_2_PI;
                j.up += k * HAND_BOB_M * bob;
                j.right += k * HAND_SWAY_M * phase.sin();
                j.roll += k * HAND_SWAY_ROLL_DEG.to_radians() * phase.sin();
            }
        }
        j
    }

    /// Lay the jitter at `t` over the camera the scripts asked for: only
    /// what they set this frame moves (a position, a target), so a camera
    /// the host drives itself is left alone and no shake feeds into the
    /// next frame's. `base_position` / `base_direction` are last frame's
    /// camera, for the half a script didn't set.
    pub fn shake_requested(
        &self,
        t: f64,
        camera: &mut CameraOverrides,
        base_position: [f32; 3],
        base_direction: [f32; 3],
    ) {
        if camera.position.is_none() && camera.target.is_none() {
            return;
        }
        let j = self.jitter(t);
        if j.is_none() {
            return;
        }
        let v = |a: [f32; 3]| [a[0] as f64, a[1] as f64, a[2] as f64];
        let pos = v(camera.position.unwrap_or(base_position));
        let target = match camera.target {
            Some(t) => v(t),
            None => add(pos, v(base_direction)),
        };
        let to = sub(target, pos);
        let dist = len(to);
        if dist < 1e-6 {
            return;
        }
        let forward = scale(to, 1.0 / dist);
        let r = cross(forward, [0.0, 1.0, 0.0]);
        // Looking straight up or down there is no right: use world X
        let right = if len(r) < 1e-4 {
            [1.0, 0.0, 0.0]
        } else {
            scale(r, 1.0 / len(r))
        };
        let up = cross(right, forward);
        let offset = add(scale(right, j.right), scale(up, j.up));
        let shaken = add(pos, offset);
        let turned = add(
            forward,
            add(scale(right, j.yaw.tan()), scale(up, j.pitch.tan())),
        );
        let turned = scale(turned, dist / len(turned));
        let f = |a: [f64; 3]| [a[0] as f32, a[1] as f32, a[2] as f32];
        if camera.position.is_some() {
            camera.position = Some(f(shaken));
        }
        if camera.target.is_some() {
            camera.target = Some(f(add(shaken, turned)));
        }
        camera.roll = Some(camera.roll.unwrap_or(0.0) + j.roll as f32);
    }

    /// Whether the shot `index` of the list `key` began on this frame
    /// (game time `t`) for `entity`: true the first time a script sees a
    /// shot, and for every call it makes during that frame.
    pub fn shot_began(&mut self, entity: EntityId, key: u64, index: i64, t: f64) -> bool {
        let seen = self.shots.entry((entity, key)).or_insert((index, t));
        if seen.0 != index {
            *seen = (index, t);
        }
        seen.1 == t
    }

    /// A new scene: no shake, no hand, no shots seen.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Where game time `t` falls in a shot list: `starts` is each shot's
/// start, then the end of the last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    /// 0 for the first shot; -1 before it; the last shot's after the end.
    pub index: i64,
    /// Seconds into the shot (negative before the first).
    pub time: f64,
    /// The shot's length, seconds.
    pub length: f64,
    /// `time / length`, held to 0..1.
    pub u: f64,
    /// `t` is at or past the list's end.
    pub done: bool,
}

/// The shot `t` is in, or what is wrong with the list.
pub fn shot_at(t: f64, starts: &[f64]) -> Result<Shot, String> {
    if starts.len() < 2 {
        return Err(format!(
            "a shot list is each shot's start then the end, at least two numbers; this has {}",
            starts.len()
        ));
    }
    if let Some(i) = starts.windows(2).position(|w| w[1] <= w[0]) {
        return Err(format!(
            "a shot list's times must rise; {} at {} is not after {}",
            starts[i + 1],
            i + 1,
            starts[i]
        ));
    }
    let shots = starts.len() - 1;
    let end = starts[shots];
    // The shots begun by t, of all but the end
    let begun = starts[..shots].partition_point(|&s| s <= t);
    let index = begun as i64 - 1;
    let i = begun.saturating_sub(1);
    let length = starts[i + 1] - starts[i];
    let time = t - starts[i];
    Ok(Shot {
        index,
        time,
        length,
        u: if index < 0 {
            0.0
        } else {
            (time / length).clamp(0.0, 1.0)
        },
        done: t >= end,
    })
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_seeded_and_the_same_every_time() {
        let a: Vec<f64> = (0..2000).map(|i| noise(i as f64 * 0.01, 7)).collect();
        let b: Vec<f64> = (0..2000).map(|i| noise(i as f64 * 0.01, 7)).collect();
        assert_eq!(a, b);
        assert!(a.iter().all(|v| (-1.0..=1.0).contains(v)));
        // Smooth: no jump between neighbours a hundredth apart
        let worst = a
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.05, "{worst}");
        // It moves, both ways
        assert!(a.iter().any(|&v| v > 0.3) && a.iter().any(|&v| v < -0.3));
        // Another seed is another curve; whole x is 0
        let c: Vec<f64> = (0..2000).map(|i| noise(i as f64 * 0.01, 8)).collect();
        assert_ne!(a, c);
        assert_eq!(noise(3.0, 7), 0.0);
    }

    fn looking_down_x() -> CameraOverrides {
        CameraOverrides {
            position: Some([0.0, 2.0, 0.0]),
            target: Some([10.0, 2.0, 0.0]),
            ..CameraOverrides::default()
        }
    }

    #[test]
    fn a_shake_moves_the_camera_across_its_view_never_along_it() {
        let mut rig = CameraRig::default();
        rig.shake.kick(0.0, 0.2, 12.0, 0.0, Some(0.0), 3);
        for i in 1..200 {
            let t = i as f64 / 60.0;
            let mut cam = looking_down_x();
            rig.shake_requested(t, &mut cam, [0.0; 3], [0.0, 0.0, -1.0]);
            let p = cam.position.unwrap();
            // Looking along +X: right is +Z, up is +Y; X never moves
            assert_eq!(p[0], 0.0, "t {t}: moved along its line of sight");
            assert!(p[2].abs() <= 0.2 + 1e-6 && (p[1] - 2.0).abs() <= 0.2 + 1e-6);
            // No turn asked for: the target moves with it
            let tg = cam.target.unwrap();
            assert!((tg[2] - p[2]).abs() < 1e-5 && (tg[1] - p[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn a_shake_decays_stacks_and_dies() {
        let mut s = ViewShake::default();
        s.kick(1.0, 0.1, 10.0, 2.0, None, 0);
        assert_eq!(s.amplitude_at(0.5), 0.0, "not before it starts");
        assert!((s.amplitude_at(1.5) - 0.1 * (-1.0f64).exp()).abs() < 1e-9);
        // A second kick adds to what is left
        s.kick(1.5, 0.1, 10.0, 2.0, None, 0);
        assert!((s.amplitude_at(1.5) - (0.1 + 0.1 * (-1.0f64).exp())).abs() < 1e-9);
        assert_eq!(s.amplitude_at(20.0), 0.0, "died out");
        // Its turn is ten degrees a metre unless asked
        assert!((s.turn_per_metre - 10f64.to_radians()).abs() < 1e-12);
    }

    #[test]
    fn only_what_the_script_set_is_shaken() {
        let mut rig = CameraRig::default();
        rig.shake.kick(0.0, 0.3, 9.0, 0.0, None, 1);
        // Nothing asked for: the host's own camera is left alone
        let mut none = CameraOverrides::default();
        rig.shake_requested(0.3, &mut none, [1.0; 3], [0.0, 0.0, -1.0]);
        assert_eq!(none, CameraOverrides::default());
        // A target alone moves, the position stays the host's
        let mut target = CameraOverrides {
            target: Some([0.0, 0.0, -5.0]),
            ..CameraOverrides::default()
        };
        rig.shake_requested(0.3, &mut target, [0.0; 3], [0.0, 0.0, -1.0]);
        assert!(target.position.is_none());
        assert_ne!(target.target, Some([0.0, 0.0, -5.0]));
        assert!(target.roll.is_some());
    }

    #[test]
    fn the_hand_held_camera_bobs_with_each_step() {
        let mut rig = CameraRig::default();
        rig.hand = HandHeld {
            strength: 1.0,
            steps: 2.0,
            seed: 0,
        };
        let j = |t| rig.jitter(t);
        // Footfalls at every half second: the bob's lowest points
        let at_step = j(0.5).up;
        let mid_step = j(0.75).up;
        assert!(mid_step > at_step, "{mid_step} vs {at_step}");
        // Small: centimetres and a degree or so
        for i in 0..600 {
            let v = j(i as f64 / 60.0);
            assert!(v.right.abs() < 0.04 && v.up.abs() < 0.04, "{v:?}");
            assert!(v.roll.abs() < 2f64.to_radians(), "{v:?}");
        }
        rig.hand.strength = 0.0;
        assert_eq!(rig.jitter(1.3), Jitter::default());
    }

    #[test]
    fn a_shot_list_names_the_shot_its_time_and_its_end() {
        let list = [0.0, 4.0, 9.5, 14.0];
        let s = shot_at(5.0, &list).unwrap();
        assert_eq!((s.index, s.time, s.length, s.done), (1, 1.0, 5.5, false));
        assert!((s.u - 1.0 / 5.5).abs() < 1e-12);
        assert_eq!(
            shot_at(4.0, &list).unwrap().index,
            1,
            "a start is its shot's"
        );
        let before = shot_at(-1.0, &[0.0, 2.0]).unwrap();
        assert_eq!((before.index, before.u), (-1, 0.0));
        let after = shot_at(20.0, &list).unwrap();
        assert_eq!((after.index, after.u, after.done), (2, 1.0, true));
        assert!(shot_at(1.0, &[0.0]).is_err());
        assert!(shot_at(1.0, &[0.0, 3.0, 2.0]).is_err());
    }

    #[test]
    fn a_shots_first_frame_is_flagged_once_per_frame_it_began() {
        let mut rig = CameraRig::default();
        let e = EntityId::from_raw(1);
        assert!(rig.shot_began(e, 9, 0, 0.0), "the first frame seen");
        assert!(rig.shot_began(e, 9, 0, 0.0), "every call that frame");
        assert!(!rig.shot_began(e, 9, 0, 1.0 / 60.0));
        assert!(rig.shot_began(e, 9, 1, 4.0));
        assert!(!rig.shot_began(e, 9, 1, 4.0 + 1.0 / 60.0));
        // Another entity's list is its own
        assert!(rig.shot_began(EntityId::from_raw(2), 9, 1, 4.1));
    }
}
