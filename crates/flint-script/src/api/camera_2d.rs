//! The `camera_2d` module: a side-on camera that follows an entity through
//! a deadzone with exponential smoothing, and a decaying screen shake
//! (ADR 0116). The state lives on the call context
//! (`CameraFollowState`, `ShakeState`) and the camera is set through the
//! overrides the host reads after the scripts run; the integrator is the
//! plain functions below.

use super::builder::kind::{Entity, Map as MapKind, Num};
use super::builder::ScriptApi;
use crate::context::{CameraFollowState, ScriptCallContext, ShakeState};
use rhai::{Dynamic, Map};

/// Below this a shake has died out.
const SHAKE_FLOOR: f32 = 0.001;

/// One step of the follow: move toward the nearest edge of a deadzone
/// rectangle around `target`, frame-rate independently. The first step
/// starts at the target.
pub(crate) fn follow_step(
    f: &mut CameraFollowState,
    target: [f32; 2],
    speed: f32,
    deadzone: [f32; 2],
    dt: f32,
) {
    if !f.initialized {
        f.current_x = target[0];
        f.current_y = target[1];
        f.initialized = true;
    }
    // Track only when the target is outside the deadzone, to its nearest edge
    let track = |current: f32, target: f32, half: f32| {
        let d = target - current;
        if d.abs() > half {
            target - d.signum() * half
        } else {
            current
        }
    };
    let track_x = track(f.current_x, target[0], deadzone[0] * 0.5);
    let track_y = track(f.current_y, target[1], deadzone[1] * 0.5);
    let t = 1.0 - (-speed * dt).exp();
    f.current_x += (track_x - f.current_x) * t;
    f.current_y += (track_y - f.current_y) * t;
}

/// Advance a live shake by `dt`: its offset this frame. It resets once it
/// has decayed below the floor.
pub(crate) fn advance_shake(s: &mut ShakeState, dt: f32) -> [f32; 2] {
    s.phase += dt;
    s.amplitude *= (-s.decay * dt).exp();
    let offset = [
        s.amplitude * (s.frequency * s.phase).sin(),
        s.amplitude * (s.frequency * 1.37 * s.phase).cos(),
    ];
    if s.amplitude < SHAKE_FLOOR {
        s.amplitude = 0.0;
        s.phase = 0.0;
    }
    offset
}

/// `camera_follow`'s frame: follow, shake, and set the camera override.
fn follow(c: &mut ScriptCallContext, target: [f32; 2], speed: f32, deadzone: [f32; 2], dt: f32) {
    follow_step(&mut c.camera_follow, target, speed, deadzone, dt);
    let shake = if c.shake.amplitude > SHAKE_FLOOR {
        advance_shake(&mut c.shake, dt)
    } else {
        [0.0, 0.0]
    };
    let x = c.camera_follow.current_x + shake[0];
    let y = c.camera_follow.current_y + shake[1];
    // z = 10 puts every sprite layer (z_offset = layer * 0.01) in front
    c.camera_position_override = Some([x, y, 10.0]);
    c.camera_target_override = Some([x, y, -1.0]);
}

/// `camera_apply_shake`: shake whatever camera the script has set.
fn apply_shake(c: &mut ScriptCallContext, dt: f32) {
    if dt <= 0.0 || c.shake.amplitude < SHAKE_FLOOR {
        return;
    }
    let [sx, sy] = advance_shake(&mut c.shake, dt);
    match c.camera_position_override.as_mut() {
        Some(pos) => {
            pos[0] += sx;
            pos[1] += sy;
        }
        None => {
            let base = c.camera_position;
            c.camera_position_override = Some([base[0] + sx, base[1] + sy, base[2]]);
        }
    }
    if let Some(target) = c.camera_target_override.as_mut() {
        target[0] += sx;
        target[1] += sy;
    }
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("camera_2d");

    m.func(
        "camera_follow",
        "Follow an entity from an offset, at a speed, moving only once it leaves a deadzone; call every frame.",
    )
    .arg("id", Entity)
    .arg("offset_x", Num)
    .arg("offset_y", Num)
    .arg("speed", Num)
    .arg("deadzone_w", Num)
    .arg("deadzone_h", Num)
    .run(|call, (id, offset_x, offset_y, speed, deadzone_w, deadzone_h)| {
        let dt = call.ctx_ref().delta_time as f32;
        if dt <= 0.0 {
            return Ok(());
        }
        let Some(t) = call.world()?.get_transform(id) else {
            return Ok(());
        };
        let target = [
            t.position.x + offset_x as f32,
            t.position.y + offset_y as f32,
        ];
        let deadzone = [deadzone_w as f32, deadzone_h as f32];
        follow(call.ctx(), target, speed as f32, deadzone, dt);
        Ok(())
    });

    m.func(
        "camera_follow_position",
        "Where camera_follow has the camera.",
    )
    .returns(MapKind, "#{x, y}")
    .run(|call, ()| {
        let f = &call.ctx_ref().camera_follow;
        let mut map = Map::new();
        map.insert("x".into(), Dynamic::from(f.current_x as f64));
        map.insert("y".into(), Dynamic::from(f.current_y as f64));
        map
    });

    m.func(
        "camera_follow_set",
        "Put camera_follow's camera at a point at once, without smoothing.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .run(|call, (x, y)| {
        let f = &mut call.ctx().camera_follow;
        f.current_x = x as f32;
        f.current_y = y as f32;
        f.initialized = true;
    });

    m.func(
        "camera_shake",
        "Shake the camera: an amplitude (stacking on a shake under way), a frequency and a decay rate.",
    )
    .arg("amplitude", Num)
    .arg("frequency", Num)
    .arg("decay", Num)
    .run(|call, (amplitude, frequency, decay)| {
        let s = &mut call.ctx().shake;
        let amplitude = amplitude as f32;
        s.amplitude = (s.amplitude + amplitude).max(amplitude);
        s.frequency = frequency as f32;
        s.decay = decay as f32;
        // The phase carries on
    });

    m.func("camera_shake_stop", "Stop the camera shake at once.")
        .run(|call, ()| {
            let s = &mut call.ctx().shake;
            s.amplitude = 0.0;
            s.phase = 0.0;
        });

    m.func(
        "camera_apply_shake",
        "Add this frame's shake to a camera the script sets itself, without camera_follow.",
    )
    .run(|call, ()| {
        let c = call.ctx();
        let dt = c.delta_time as f32;
        apply_shake(c, dt);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_follow_starts_on_the_target_and_holds_inside_the_deadzone() {
        let mut f = CameraFollowState::default();
        follow_step(&mut f, [5.0, 2.0], 4.0, [2.0, 2.0], 1.0 / 60.0);
        assert_eq!((f.current_x, f.current_y), (5.0, 2.0));
        // Inside the deadzone: no move
        follow_step(&mut f, [5.9, 2.5], 4.0, [2.0, 2.0], 1.0 / 60.0);
        assert_eq!((f.current_x, f.current_y), (5.0, 2.0));
        // Out past it: toward the deadzone's near edge, not the target
        for _ in 0..600 {
            follow_step(&mut f, [10.0, 2.0], 4.0, [2.0, 2.0], 1.0 / 60.0);
        }
        assert!((f.current_x - 9.0).abs() < 1e-3, "{}", f.current_x);
    }

    #[test]
    fn a_shake_decays_to_nothing_and_resets() {
        let mut s = ShakeState {
            amplitude: 1.0,
            ..ShakeState::default()
        };
        let first = advance_shake(&mut s, 1.0 / 60.0);
        assert!(first[0].abs() <= 1.0 && first[1].abs() <= 1.0);
        for _ in 0..600 {
            advance_shake(&mut s, 1.0 / 60.0);
        }
        assert_eq!((s.amplitude, s.phase), (0.0, 0.0));
    }
}
