//! The chase camera (#1631): behind a vehicle and above it, looking a
//! little ahead, eased so a slide or a landing reads as motion rather than
//! a jolt. It places the kernel's eye (`PlayerView::eye`) at the frame's
//! `Stage::View`, so the camera, the listener and the scripts' player
//! follow the vehicle with no camera script (ADR 0138 decision 4).
//!
//! **It moves once a fixed step**, from the pose that step left: the
//! subsystem's `fixed_update` at the start of a frame's second and later
//! steps sees the pose the step before left, and `Stage::View` sees the
//! last one's. A vehicle's body is written back once a step, not between,
//! so a camera eased at the frame would slide past a vehicle that holds
//! still on frames with no step (most frames at 144 Hz) and the vehicle
//! would judder on the screen. Stepped with it, the shot is the same at 30
//! and 144 Hz, and the vehicle holds still in it.
//!
//! The defaults are FlintKart's `chase_camera.rhai` (its `follow_distance`,
//! `height_offset`, `look_ahead`, `smooth_speed`, speed pull-back and drop,
//! lead, FOV curve and steering lean), and its rates are its
//! `clamp(rate * dt)` eases at the fixed step, so at 1/60 s they are that
//! script's 60 fps numbers.

use crate::CHASE_CAMERA;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::{Eye, InputState};
use toml::Value;

/// A chase camera's tuning: the `chase_camera` fields that aren't
/// modifiers, each falling back to its schema default.
#[derive(Debug, Clone, PartialEq)]
pub struct ChaseTuning {
    pub distance: f32,
    pub height: f32,
    pub look_ahead: f32,
    pub aim_height: f32,
    pub speed_pullback: f32,
    pub speed_drop: f32,
    pub travel: f32,
    pub lag: f32,
    pub lead: f32,
    pub cut_distance: f32,
    pub fov: f32,
    pub fov_speed: f32,
    pub fov_attack: f32,
    pub fov_release: f32,
    pub steer_roll: f32,
    pub roll_rate: f32,
    pub look_back: String,
    pub look_back_lag: f32,
}

impl ChaseTuning {
    pub fn read(world: &FlintWorld, entity: EntityId) -> Self {
        let v = world.view(entity, CHASE_CAMERA);
        Self {
            distance: v.f32("distance", 8.0).max(0.0),
            height: v.f32("height", 4.0),
            look_ahead: v.f32("look_ahead", 4.0),
            aim_height: v.f32("aim_height", 1.0),
            speed_pullback: v.f32("speed_pullback", 1.2),
            speed_drop: v.f32("speed_drop", 0.5),
            travel: v.f32("travel", 1.0).clamp(0.0, 1.0),
            lag: v.f32("lag", 5.0).max(0.0),
            lead: v.f32("lead", 0.7).max(0.0),
            cut_distance: v.f32("cut_distance", 6.0).max(0.0),
            fov: v.f32("fov", 62.0).clamp(1.0, 179.0),
            fov_speed: v.f32("fov_speed", 16.0),
            fov_attack: v.f32("fov_attack", 6.0).max(0.0),
            fov_release: v.f32("fov_release", 3.0).max(0.0),
            steer_roll: v.f32("steer_roll", 0.012),
            roll_rate: v.f32("roll_rate", 5.0).max(0.0),
            look_back: v.str("look_back", "look_back").to_string(),
            look_back_lag: v.f32("look_back_lag", 8.0).max(0.0),
        }
    }
}

/// What a game wrote into `chase_camera`'s modifiers this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ChaseModifiers {
    pub offset: [f32; 3],
    pub roll: f32,
    pub fov_bonus: f32,
    pub fov_kick: f32,
    pub shake: [f32; 3],
    pub cut: bool,
    pub release: bool,
}

impl ChaseModifiers {
    pub fn read(world: &FlintWorld, entity: EntityId) -> Self {
        let v = world.view(entity, CHASE_CAMERA);
        Self {
            offset: v.vec3("offset", [0.0; 3]),
            roll: v.f32("roll", 0.0),
            fov_bonus: v.f32("fov_bonus", 0.0),
            fov_kick: v.f32("fov_kick", 0.0),
            shake: v.vec3("shake", [0.0; 3]),
            cut: v.bool("cut", false),
            release: v.bool("release", false),
        }
    }
}

/// The modifiers' neutral values, written back after each frame's eye.
pub(super) fn neutral() -> [(&'static str, Value); 7] {
    let zero = || Value::Array(vec![Value::Float(0.0); 3]);
    [
        ("offset", zero()),
        ("roll", Value::Float(0.0)),
        ("fov_bonus", Value::Float(0.0)),
        ("fov_kick", Value::Float(0.0)),
        ("shake", zero()),
        ("cut", Value::Boolean(false)),
        ("release", Value::Boolean(false)),
    ]
}

/// Where the vehicle is, as one step left it: what a camera step reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Followed {
    pub position: Vec3,
    /// The body's forward (a vehicle faces -Z), in the world
    pub forward: Vec3,
    /// `vehicle_input.steer`, -1 to 1, positive right
    pub steer: f32,
    /// `vehicle.top_speed`, for the speed's share of the curves
    pub top_speed: f32,
}

impl Followed {
    /// The entity's world pose, its steering and its speed cap.
    pub fn read(world: &FlintWorld, entity: EntityId) -> Option<Self> {
        let m = world.get_world_matrix(entity)?;
        Some(Self {
            position: Vec3::new(m[3][0], m[3][1], m[3][2]),
            forward: Vec3::new(-m[2][0], -m[2][1], -m[2][2]),
            steer: world
                .view(entity, crate::VEHICLE_INPUT)
                .f32("steer", 0.0)
                .clamp(-1.0, 1.0),
            top_speed: world.view(entity, crate::VEHICLE).f32("top_speed", 30.0),
        })
    }
}

/// The camera's eased state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub position: Vec3,
    pub target: Vec3,
    /// Degrees, eased toward its target
    pub fov: f32,
    /// Radians, eased toward its target
    pub roll: f32,
    /// Where the vehicle was at the last step, for its velocity and a cut
    pub was_at: Vec3,
    /// Its speed along its heading over the last step, m/s
    pub speed: f32,
}

/// One vehicle's chase camera.
#[derive(Debug, Clone, PartialEq)]
pub struct Chase {
    pub entity: EntityId,
    /// `None` until its first step, which places it where it wants to be
    pub shot: Option<Shot>,
    /// The steps it has taken
    pub steps: u64,
    /// The cuts it made (the first placement among them)
    pub cuts: u64,
}

fn flat(v: Vec3) -> Option<Vec3> {
    let len = (v.x * v.x + v.z * v.z).sqrt();
    (len > 1e-4).then(|| Vec3::new(v.x / len, 0.0, v.z / len))
}

fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.z + (b.z - a.z) * t,
    )
}

/// `clamp(rate * dt, 0, 1)`: FlintKart's ease, which at a fixed step is
/// the same whatever the frame rate.
fn ease(rate: f32, dt: f32) -> f32 {
    (rate * dt).clamp(0.0, 1.0)
}

impl Chase {
    pub fn new(entity: EntityId) -> Self {
        Self {
            entity,
            shot: None,
            steps: 0,
            cuts: 0,
        }
    }

    /// One step of `dt` seconds toward where it wants to be, behind `at`.
    pub fn step(
        &mut self,
        at: &Followed,
        tuning: &ChaseTuning,
        mods: &ChaseModifiers,
        looking_back: bool,
        dt: f32,
    ) {
        self.steps += 1;
        let p = at.position;
        let dt = dt.max(1e-6);
        let body = flat(at.forward).unwrap_or(Vec3::new(0.0, 0.0, -1.0));
        // The velocity over the step, and whether the move was a respawn
        let (mut velocity, mut cut) = match &self.shot {
            None => (Vec3::ZERO, true),
            Some(shot) => {
                let d = Vec3::new(
                    p.x - shot.was_at.x,
                    p.y - shot.was_at.y,
                    p.z - shot.was_at.z,
                );
                let v = Vec3::new(d.x / dt, d.y / dt, d.z / dt);
                let moved = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
                // Against the speed it had: a respawn's own move explains
                // nothing
                (v, moved > tuning.cut_distance + shot.speed.abs() * dt * 3.0)
            }
        };
        cut |= mods.cut;
        if cut {
            velocity = Vec3::ZERO;
            self.cuts += 1;
        }
        // The heading: the travel's once moving forward, the body's at rest
        // and in reverse
        let speed = velocity.x * body.x + velocity.z * body.z;
        let travel_share = (speed / 3.0).clamp(0.0, 1.0) * tuning.travel;
        let fwd = match flat(velocity) {
            Some(travel) if travel_share > 0.0 => flat(Vec3::new(
                body.x + (travel.x - body.x) * travel_share,
                0.0,
                body.z + (travel.z - body.z) * travel_share,
            ))
            .unwrap_or(body),
            _ => body,
        };
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        let ratio = if at.top_speed > 0.0 {
            (speed.abs() / at.top_speed).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dir = if looking_back { -1.0 } else { 1.0 };
        let rate = if looking_back {
            tuning.look_back_lag
        } else {
            tuning.lag
        };

        let back = tuning.distance + ratio * tuning.speed_pullback + mods.offset[2];
        let ideal = Vec3::new(
            p.x - fwd.x * back * dir + right.x * mods.offset[0],
            p.y + tuning.height - ratio * tuning.speed_drop + mods.offset[1],
            p.z - fwd.z * back * dir + right.z * mods.offset[0],
        );
        // Chasing a point moving at v with rate k trails it by v / k: aim
        // ahead by most of that, so turns stay smooth and the straight-line
        // lag mostly goes
        let lead = if rate > 0.0 { tuning.lead / rate } else { 0.0 };
        let ahead = Vec3::new(velocity.x * lead, 0.0, velocity.z * lead);
        let aim = Vec3::new(
            p.x + fwd.x * tuning.look_ahead * dir + ahead.x,
            p.y + tuning.aim_height,
            p.z + fwd.z * tuning.look_ahead * dir + ahead.z,
        );
        let fov_target = tuning.fov + ratio * tuning.fov_speed + mods.fov_bonus;
        let roll_target = if looking_back {
            0.0
        } else {
            mods.roll - at.steer * ratio * tuning.steer_roll
        };
        let wanted = Vec3::new(ideal.x + ahead.x, ideal.y, ideal.z + ahead.z);

        let mut shot = self.shot.unwrap_or(Shot {
            position: wanted,
            target: aim,
            fov: fov_target,
            roll: roll_target,
            was_at: p,
            speed,
        });
        let t = if cut { 1.0 } else { ease(rate, dt) };
        shot.position = lerp(shot.position, wanted, t);
        shot.target = lerp(shot.target, aim, t);
        let fov_rate = if fov_target > shot.fov {
            tuning.fov_attack
        } else {
            tuning.fov_release
        };
        shot.fov += (fov_target - shot.fov) * ease(fov_rate, dt);
        shot.roll += (roll_target - shot.roll) * ease(tuning.roll_rate, dt);
        shot.was_at = p;
        shot.speed = speed;
        self.shot = Some(shot);
    }

    /// The eye for this frame, with the frame's shake and kick on top;
    /// `None` before its first step.
    pub fn eye(&self, mods: &ChaseModifiers) -> Option<Eye> {
        let shot = self.shot?;
        let view = Vec3::new(
            shot.target.x - shot.position.x,
            0.0,
            shot.target.z - shot.position.z,
        );
        let right = flat(view).map_or(Vec3::ZERO, |v| Vec3::new(-v.z, 0.0, v.x));
        let [side, up, roll] = mods.shake;
        let position = Vec3::new(
            shot.position.x + right.x * side,
            shot.position.y + up,
            shot.position.z + right.z * side,
        );
        let mut eye = Eye::looking(position, shot.target);
        eye.fov = Some((shot.fov + mods.fov_kick).clamp(1.0, 179.0));
        eye.roll = shot.roll + roll;
        Some(eye)
    }
}

/// Whether the look-back action is held.
pub fn looking_back(input: &InputState, tuning: &ChaseTuning) -> bool {
    !tuning.look_back.is_empty() && input.is_action_pressed(&tuning.look_back)
}

/// The vehicle the game is played as: the first with a `chase_camera`, by
/// name (one player, the first by name: ADR 0134 decision 1).
pub fn chased(world: &FlintWorld, vehicles: &[EntityId]) -> Option<EntityId> {
    vehicles
        .iter()
        .copied()
        .filter(|id| world.get_component(*id, CHASE_CAMERA).is_some())
        .min_by(|a, b| {
            world
                .get_name(*a)
                .unwrap_or("")
                .cmp(world.get_name(*b).unwrap_or(""))
        })
}
