//! The vehicles' subsystem: a scene's vehicles, driven each fixed step and
//! handed to scripts and the screen at the frame's stages.

use super::model::{wheel_pose, Input, Tuning, Vehicle, WheelSpec};
use super::module::Place;
use crate::{VEHICLE, VEHICLE_INPUT, VEHICLE_STATE, VEHICLE_WHEEL};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::{FixedCtx, SceneCtx, Stage, StageCtx, Subsystem};
use flint_runtime::InputState;
use flint_script::context::ScriptCommand;
use toml::Value;

/// The subsystem's name: the game-state policy name it would pause by, and
/// what logs and panels call it.
pub const NAME: &str = "vehicle";

/// A scene's vehicles.
pub struct Vehicles {
    vehicles: Vec<Vehicle>,
    /// The world's next id when the vehicles were last looked for: a
    /// vehicle spawned since is picked up at the next step.
    looked_at: EntityId,
    /// The fixed steps it has heard.
    steps: u64,
}

/// The subsystem for a scene with any `vehicle`.
pub fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if ctx.world.entities_with_component(VEHICLE).is_empty() {
        return None;
    }
    Some(Box::new(Vehicles::new(ctx.world)))
}

/// The neutral modifiers, written back after each frame's steps.
const NEUTRAL: [(&str, f64); 7] = [
    ("grip_scale", 1.0),
    ("turn_scale", 1.0),
    ("speed_scale", 1.0),
    ("speed_bonus", 0.0),
    ("accel_bonus", 0.0),
    ("slip_target", 0.0),
    ("spin", 0.0),
];

fn float(v: f32) -> Value {
    Value::Float(v as f64)
}

fn vec3(v: [f32; 3]) -> Value {
    Value::Array(v.iter().map(|c| float(*c)).collect())
}

fn id_value(id: Option<EntityId>) -> Value {
    Value::Integer(id.map_or(-1, |e| e.raw() as i64))
}

/// The box a vehicle's inertia is taken from: its collider's `size` times
/// its scale, or a kart's 1.2 x 0.5 x 2 m with no collider.
fn chassis_size(world: &FlintWorld, entity: EntityId) -> [f32; 3] {
    let size = world
        .view(entity, flint_core::components::COLLIDER)
        .vec3("size", [1.2, 0.5, 2.0]);
    let scale = world.get_transform(entity).map_or([1.0; 3], |t| {
        [t.scale.x.abs(), t.scale.y.abs(), t.scale.z.abs()]
    });
    [size[0] * scale[0], size[1] * scale[1], size[2] * scale[2]]
}

impl Vehicles {
    /// The world's vehicles, each with its wheels.
    pub fn new(world: &FlintWorld) -> Self {
        let mut vehicles = Self {
            vehicles: Vec::new(),
            looked_at: EntityId::from_raw(0),
            steps: 0,
        };
        vehicles.find(world);
        vehicles
    }

    /// Picks up the vehicles the world has that it doesn't, and drops the
    /// ones it no longer has.
    fn find(&mut self, world: &FlintWorld) {
        let ids = world.entities_with_component(VEHICLE);
        self.vehicles.retain(|v| ids.contains(&v.entity));
        for &entity in ids {
            if self.vehicles.iter().any(|v| v.entity == entity) {
                continue;
            }
            let wheels = world
                .children(entity)
                .iter()
                .copied()
                .filter(|c| world.get_component(*c, VEHICLE_WHEEL).is_some())
                .map(|c| {
                    let at = world
                        .get_transform(c)
                        .map_or([0.0; 3], |t| [t.position.x, t.position.y, t.position.z]);
                    WheelSpec::read(world, c, at)
                })
                .collect::<Vec<_>>();
            if wheels.is_empty() {
                tracing::warn!(
                    target: "vehicle",
                    "vehicle {:?} has no child with vehicle_wheel: it has no wheels to drive on",
                    world.get_name(entity).unwrap_or("?")
                );
            }
            self.vehicles.push(Vehicle::new(entity, wheels));
        }
        self.looked_at = world.next_id();
    }

    /// The vehicles, for a test or a panel.
    pub fn vehicles(&self) -> &[Vehicle] {
        &self.vehicles
    }

    /// The fixed steps it has heard.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// A `vehicle_place` for the next step: `false` when `entity` isn't one
    /// of the vehicles.
    pub fn place(&mut self, entity: EntityId, position: [f32; 3], yaw: f32) -> bool {
        match self.vehicles.iter_mut().find(|v| v.entity == entity) {
            Some(v) => {
                v.place = Some((position, yaw));
                true
            }
            None => false,
        }
    }

    /// `vehicle_state` and each wheel's state, from the last step.
    pub fn write_state(&self, world: &mut FlintWorld) {
        for v in &self.vehicles {
            let s = &v.snapshot;
            if let Some(mut c) = world.get_components_mut(v.entity) {
                let fields = [
                    ("speed", float(s.speed)),
                    ("velocity", vec3(s.velocity)),
                    ("forward", vec3(s.forward)),
                    ("lateral_speed", float(s.lateral_speed)),
                    ("slip_angle", float(s.slip_angle)),
                    ("yaw", float(s.yaw)),
                    ("yaw_rate", float(s.yaw_rate)),
                    ("yaw_target", float(s.yaw_target)),
                    ("accel_long", float(s.accel_long)),
                    ("accel_lat", float(s.accel_lat)),
                    ("grounded", Value::Boolean(s.wheels_grounded > 0)),
                    ("wheels_grounded", Value::Integer(s.wheels_grounded as i64)),
                    ("airborne_time", float(s.airborne_time)),
                    ("contact", Value::Boolean(s.contact)),
                    ("contact_normal", vec3(s.contact_normal)),
                    ("contact_entity", id_value(s.contact_entity)),
                    ("impact", float(s.impact)),
                ];
                for (field, value) in fields {
                    c.set_field(VEHICLE_STATE, field, value);
                }
            }
            for (spec, seen) in v.wheels.iter().zip(&v.wheel_seen) {
                if let Some(mut c) = world.get_components_mut(spec.entity) {
                    c.set_field(VEHICLE_WHEEL, "contact", Value::Boolean(seen.contact));
                    c.set_field(VEHICLE_WHEEL, "ground", id_value(seen.ground));
                    c.set_field(VEHICLE_WHEEL, "normal", vec3(seen.normal));
                    c.set_field(VEHICLE_WHEEL, "compression", float(seen.compression));
                    c.set_field(VEHICLE_WHEEL, "slip", float(seen.slip));
                }
            }
        }
    }

    /// A player-driven vehicle's driving input, from the frame's actions.
    pub fn write_player_input(&self, world: &mut FlintWorld, input: &InputState) {
        for v in &self.vehicles {
            let control = world.view(v.entity, VEHICLE);
            if control.choice("driver", &["player", "script"], "player") != "player" {
                continue;
            }
            let curve = control.f32("steer_curve", 1.0).max(0.1);
            let actions = control.raw("actions").and_then(Value::as_table);
            let action = |role: &str, default: &'static str| -> String {
                actions
                    .and_then(|t| t.get(role))
                    .and_then(Value::as_str)
                    .unwrap_or(default)
                    .to_string()
            };
            let throttle = input.action_value(&action("throttle", "accelerate"));
            let brake = input.action_value(&action("brake", "brake"));
            let raw = input
                .action_value(&action("steer", "steering"))
                .clamp(-1.0, 1.0);
            let steer = raw.signum() * raw.abs().powf(curve);
            let handbrake = input.is_action_pressed(&action("handbrake", "drift"));
            if let Some(mut c) = world.get_components_mut(v.entity) {
                c.set_field(VEHICLE_INPUT, "throttle", float(throttle.clamp(-1.0, 1.0)));
                c.set_field(VEHICLE_INPUT, "brake", float(brake.clamp(0.0, 1.0)));
                c.set_field(VEHICLE_INPUT, "steer", float(steer));
                c.set_field(VEHICLE_INPUT, "handbrake", Value::Boolean(handbrake));
            }
        }
    }

    /// The wheel children's poses (`pose = true`), and the modifiers back
    /// to neutral for the next frame.
    pub fn after_steps(&self, world: &mut FlintWorld) {
        for v in &self.vehicles {
            for (spec, seen) in v.wheels.iter().zip(&v.wheel_seen) {
                if !spec.pose || !v.has_controller() {
                    continue;
                }
                let (position, quat) = wheel_pose(spec, seen);
                if let Some(mut c) = world.get_components_mut(spec.entity) {
                    c.set_field(
                        flint_core::components::TRANSFORM,
                        "position",
                        vec3(position),
                    );
                    c.set_field(
                        flint_core::components::TRANSFORM,
                        "rotation_quat",
                        Value::Array(quat.iter().map(|q| float(*q)).collect()),
                    );
                }
            }
            if world.get_component(v.entity, VEHICLE_INPUT).is_none() {
                continue;
            }
            if let Some(mut c) = world.get_components_mut(v.entity) {
                for (field, neutral) in NEUTRAL {
                    c.set_field(VEHICLE_INPUT, field, Value::Float(neutral));
                }
            }
        }
    }
}

impl Subsystem for Vehicles {
    fn name(&self) -> &'static str {
        NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[super::module::MODULE]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            Stage::PreScripts => {
                self.write_state(ctx.world);
                self.write_player_input(ctx.world, ctx.input);
            }
            Stage::Physics => self.after_steps(ctx.world),
            _ => {}
        }
    }

    /// `vehicle_place`, carried out at the next step.
    fn script_command(&mut self, command: &ScriptCommand, _ctx: &mut StageCtx) -> bool {
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(place) = command.payload::<Place>() else {
            return false;
        };
        if !self.place(place.entity, place.position, place.yaw) {
            tracing::warn!(target: "vehicle", "vehicle_place: entity {} is not a vehicle", place.entity.raw());
        }
        true
    }

    /// One fixed step: each vehicle's tuning and input read, its wheels and
    /// assists applied for Rapier's step to integrate (ADR 0137 section 3a).
    fn fixed_update(&mut self, ctx: &mut FixedCtx) {
        self.steps += 1;
        if ctx.world.next_id() != self.looked_at {
            self.find(ctx.world);
        }
        let dt = ctx.dt as f32;
        for v in &mut self.vehicles {
            if !ctx.world.contains(v.entity) {
                continue;
            }
            let tuning = Tuning::read(ctx.world, v.entity);
            let input = Input::read(ctx.world, v.entity);
            // The wheels' own fields are live too
            for spec in &mut v.wheels {
                *spec = WheelSpec::read(ctx.world, spec.entity, spec.attach);
            }
            let size = chassis_size(ctx.world, v.entity);
            if !v.step(ctx.physics, &tuning, &input, size, dt) {
                continue;
            }
            if input.push != [0.0; 3] {
                if let Some(mut c) = ctx.world.get_components_mut(v.entity) {
                    c.set_field(VEHICLE_INPUT, "push", vec3([0.0; 3]));
                }
            }
        }
    }
}
