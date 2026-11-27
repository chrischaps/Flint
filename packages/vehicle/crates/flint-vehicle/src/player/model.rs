//! The handling model (ADR 0140 decision 1): Rapier's
//! `DynamicRayCastVehicleController` carries and drives the body, and the
//! arcade assists pull it toward a kart's handling. Every assist is a
//! `vehicle` field whose value 0 turns it off; with all of them at 0 it is a
//! plain raycast car.
//!
//! The numbers and their shapes come from the prototype the ADR measured
//! (`proto/vehicle-1627`, `tools/vehicle-proto/`): a kart faces -Z, its
//! wheels' axles are +X and their rays -Y, Rapier's forward axis is index 2,
//! and the steering angle is `-steer x max_angle`.
//!
//! One [`Vehicle`] is one entity's: its wheels, its controller once its
//! body exists, and what the last step saw, which the frame's stages hand
//! to scripts ([`Snapshot`], [`WheelSeen`]).

use crate::{VEHICLE, VEHICLE_INPUT, VEHICLE_WHEEL};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::physics::rapier::control::{DynamicRayCastVehicleController, WheelTuning};
use flint_package::physics::rapier::parry::query as parry_query;
use flint_package::physics::rapier::prelude::nalgebra::{Unit, UnitQuaternion};
use flint_package::physics::rapier::prelude::*;
use flint_package::physics::PhysicsSystem;

type V3 = Vector<Real>;

/// Below this speed a brake reverses, as FlintKart's does.
const REVERSE_BELOW: f32 = 1.0;
/// The slowest speed the turn-rate curve's radius is taken at, m/s.
const MIN_TURN_SPEED: f32 = 2.5;
/// How hard the yaw assist turns the nose toward a slip target, per second
/// per radian off it (the prototype's drift).
const SLIP_HOLD: f32 = 4.0;
/// The chassis' reach for a contact, m: a touch within this of a wall is one.
const CONTACT_REACH: f32 = 0.02;
/// The speed under the cap over which the engine eases off, m/s.
const CAP_EASE: f32 = 0.5;
/// The side friction of a handbrake wheel, against 1 for a rolling one.
const HANDBRAKE_SIDE_FRICTION: f32 = 0.3;

/// A `vehicle`'s tuning, read each step (so it is live).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub top_speed: f32,
    pub accel: f32,
    pub brake: f32,
    pub reverse: f32,
    pub drag: f32,
    pub overspeed_decay: f32,
    pub mass: f32,
    pub center_of_mass: [f32; 3],
    pub turn_rate_low: f32,
    pub turn_rate_high: f32,
    pub min_turn_radius: f32,
    pub steer_response: f32,
    pub steer_curve: f32,
    pub grip: f32,
    pub yaw_assist: f32,
    pub upright: f32,
    pub scrub: f32,
    pub tyre_grip: f32,
    pub suspension_stiffness: f32,
    pub suspension_damping: f32,
    pub suspension_travel: f32,
}

impl Tuning {
    /// The entity's `vehicle`, each field falling back to its schema's
    /// default.
    pub fn read(world: &FlintWorld, entity: EntityId) -> Self {
        let v = world.view(entity, VEHICLE);
        Self {
            top_speed: v.f32("top_speed", 30.0),
            accel: v.f32("accel", 15.0),
            brake: v.f32("brake", 25.0),
            reverse: v.f32("reverse", 0.3),
            drag: v.f32("drag", 0.5),
            overspeed_decay: v.f32("overspeed_decay", 2.5),
            mass: v.f32("mass", 150.0).max(0.1),
            center_of_mass: v.vec3("center_of_mass", [0.0, -0.25, 0.0]),
            turn_rate_low: v.f32("turn_rate_low", 180.0),
            turn_rate_high: v.f32("turn_rate_high", 60.0),
            min_turn_radius: v.f32("min_turn_radius", 2.6).max(0.1),
            steer_response: v.f32("steer_response", 7.0),
            steer_curve: v.f32("steer_curve", 1.0).max(0.1),
            grip: v.f32("grip", 9.0),
            yaw_assist: v.f32("yaw_assist", 10.0),
            upright: v.f32("upright", 60.0),
            scrub: v.f32("scrub", 0.0),
            tyre_grip: v.f32("tyre_grip", 1.8),
            suspension_stiffness: v.f32("suspension_stiffness", 40.0),
            suspension_damping: v.f32("suspension_damping", 3.5),
            suspension_travel: v.f32("suspension_travel", 0.3),
        }
    }

    /// The turn rate at `speed`, degrees a second: `turn_rate_low` eased to
    /// `turn_rate_high` over the speed range, capped by the minimum turn
    /// radius, then `turn_scale`d (FlintKart's `kart_controller.rhai`).
    pub fn yaw_rate(&self, speed: f32, turn_scale: f32) -> f32 {
        let a = speed.abs();
        let ratio = if self.top_speed > 0.0 {
            (a / self.top_speed).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let rate = self.turn_rate_low + (self.turn_rate_high - self.turn_rate_low) * ratio;
        let cap = (a.max(MIN_TURN_SPEED) / self.min_turn_radius).to_degrees();
        rate.min(cap) * turn_scale
    }
}

/// A `vehicle_input` as one step reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Input {
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
    pub handbrake: bool,
    pub hold: bool,
    pub grip_scale: f32,
    pub turn_scale: f32,
    pub speed_scale: f32,
    pub speed_bonus: f32,
    pub accel_bonus: f32,
    pub slip_target: f32,
    pub spin: f32,
    pub push: [f32; 3],
}

impl Default for Input {
    fn default() -> Self {
        Self {
            throttle: 0.0,
            brake: 0.0,
            steer: 0.0,
            handbrake: false,
            hold: false,
            grip_scale: 1.0,
            turn_scale: 1.0,
            speed_scale: 1.0,
            speed_bonus: 0.0,
            accel_bonus: 0.0,
            slip_target: 0.0,
            spin: 0.0,
            push: [0.0; 3],
        }
    }
}

impl Input {
    pub fn read(world: &FlintWorld, entity: EntityId) -> Self {
        let v = world.view(entity, VEHICLE_INPUT);
        let n = Self::default();
        Self {
            throttle: v.f32("throttle", 0.0).clamp(-1.0, 1.0),
            brake: v.f32("brake", 0.0).clamp(0.0, 1.0),
            steer: v.f32("steer", 0.0).clamp(-1.0, 1.0),
            handbrake: v.bool("handbrake", false),
            hold: v.bool("hold", false),
            grip_scale: v.f32("grip_scale", n.grip_scale),
            turn_scale: v.f32("turn_scale", n.turn_scale),
            speed_scale: v.f32("speed_scale", n.speed_scale),
            speed_bonus: v.f32("speed_bonus", 0.0),
            accel_bonus: v.f32("accel_bonus", 0.0),
            slip_target: v.f32("slip_target", 0.0),
            spin: v.f32("spin", 0.0),
            push: v.vec3("push", [0.0; 3]),
        }
    }
}

/// A wheel as its `vehicle_wheel` and its entity's authored place say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelSpec {
    pub entity: EntityId,
    /// Where its suspension hangs, in the vehicle's frame: the child's
    /// position as the scene wrote it (the package moves the child after).
    pub attach: [f32; 3],
    pub radius: f32,
    pub rest_length: f32,
    pub steer: bool,
    pub drive: bool,
    pub brake: bool,
    pub handbrake: bool,
    pub pose: bool,
}

impl WheelSpec {
    pub fn read(world: &FlintWorld, entity: EntityId, attach: [f32; 3]) -> Self {
        let v = world.view(entity, VEHICLE_WHEEL);
        Self {
            entity,
            attach,
            radius: v.f32("radius", 0.275).max(0.01),
            rest_length: v.f32("rest_length", 0.35).max(0.0),
            steer: v.bool("steer", false),
            drive: v.bool("drive", false),
            brake: v.bool("brake", true),
            handbrake: v.bool("handbrake", false),
            pose: v.bool("pose", true),
        }
    }
}

/// What a step saw of the body as it began: the state scripts read.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Snapshot {
    pub speed: f32,
    pub velocity: [f32; 3],
    pub forward: [f32; 3],
    pub lateral_speed: f32,
    pub slip_angle: f32,
    pub yaw: f32,
    pub yaw_rate: f32,
    pub accel_long: f32,
    pub accel_lat: f32,
    pub wheels_grounded: u32,
    pub airborne_time: f32,
    pub contact: bool,
    pub contact_normal: [f32; 3],
    pub contact_entity: Option<EntityId>,
    pub impact: f32,
    /// The yaw rate the assist pulled toward, degrees a second (the panel's).
    pub yaw_target: f32,
    /// The wheel angle at full lock, degrees (the panel's).
    pub max_steer: f32,
}

/// What a step saw of one wheel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelSeen {
    pub contact: bool,
    pub ground: Option<EntityId>,
    pub normal: [f32; 3],
    pub compression: f32,
    pub slip: f32,
    /// The spring's length, the steering angle and the spin, for the pose.
    pub length: f32,
    pub steering: f32,
    pub rotation: f32,
}

impl Default for WheelSeen {
    fn default() -> Self {
        Self {
            contact: false,
            ground: None,
            normal: [0.0, 1.0, 0.0],
            compression: 0.0,
            slip: 0.0,
            length: 0.0,
            steering: 0.0,
            rotation: 0.0,
        }
    }
}

/// One vehicle, from its scene's load to its end.
pub struct Vehicle {
    pub entity: EntityId,
    pub wheels: Vec<WheelSpec>,
    controller: Option<DynamicRayCastVehicleController>,
    /// The mass properties last set on the body (mass, centre, box).
    mass_set: Option<(f32, [f32; 3], [f32; 3])>,
    /// The steering as it follows the input at `steer_response`.
    steer: f32,
    prev_velocity: Option<V3>,
    pub snapshot: Snapshot,
    pub wheel_seen: Vec<WheelSeen>,
    /// A `vehicle_place` the next step carries out: position and yaw.
    pub place: Option<([f32; 3], f32)>,
    /// The steps this vehicle has been driven.
    pub steps: u64,
}

fn arr(v: &V3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// The heading of a forward vector, FlintKart's way: 0 faces -Z, positive
/// turns left.
pub fn yaw_of(forward: [f32; 3]) -> f32 {
    (-forward[0]).atan2(-forward[2]).to_degrees()
}

/// The rotation about +Y that faces a vehicle at `yaw` degrees.
pub fn rotation_of_yaw(yaw: f32) -> UnitQuaternion<Real> {
    UnitQuaternion::from_axis_angle(&Vector::y_axis(), yaw.to_radians())
}

impl Vehicle {
    pub fn new(entity: EntityId, wheels: Vec<WheelSpec>) -> Self {
        let n = wheels.len();
        Self {
            entity,
            wheels,
            controller: None,
            mass_set: None,
            steer: 0.0,
            prev_velocity: None,
            snapshot: Snapshot {
                forward: [0.0, 0.0, -1.0],
                ..Snapshot::default()
            },
            wheel_seen: vec![WheelSeen::default(); n],
            place: None,
            steps: 0,
        }
    }

    /// Whether its controller has been made (its body exists).
    pub fn has_controller(&self) -> bool {
        self.controller.is_some()
    }

    /// The front-to-back spread of the wheels, the steering geometry's
    /// wheelbase (1.45 m for the prototype's kart).
    fn wheelbase(&self) -> f32 {
        let zs = self.wheels.iter().map(|w| w.attach[2]);
        let (lo, hi) = zs.fold((f32::MAX, f32::MIN), |(lo, hi), z| (lo.min(z), hi.max(z)));
        if hi > lo {
            hi - lo
        } else {
            1.0
        }
    }

    fn make_controller(&mut self, body: RigidBodyHandle, tuning: &Tuning) {
        let mut controller = DynamicRayCastVehicleController::new(body);
        // A vehicle faces -Z: Rapier's forward is the body's Z axis, and a
        // wheel's +X axle with its -Y ray drives along -Z
        controller.index_forward_axis = 2;
        let wheel_tuning = wheel_tuning(tuning);
        for w in &self.wheels {
            let [x, y, z] = w.attach;
            controller.add_wheel(
                point![x, y, z],
                vector![0.0, -1.0, 0.0],
                vector![1.0, 0.0, 0.0],
                w.rest_length,
                w.radius,
                &wheel_tuning,
            );
        }
        self.controller = Some(controller);
    }

    /// One fixed step of `dt` seconds: the wheels set from `input`, Rapier's
    /// raycast vehicle updated against the last step's colliders (its own
    /// body's left out), then the assists, the modifiers, the drag and the
    /// push applied as impulses that the step about to run integrates.
    /// `false` when the body doesn't exist yet.
    pub fn step(
        &mut self,
        physics: &mut PhysicsSystem,
        tuning: &Tuning,
        input: &Input,
        chassis_size: [f32; 3],
        dt: f32,
    ) -> bool {
        let Some(handle) = physics.body_of(self.entity) else {
            return false;
        };
        if physics.bodies().get(handle).is_none_or(|b| !b.is_dynamic()) {
            return false;
        }
        if self.controller.is_none() {
            self.make_controller(handle, tuning);
        }
        self.steps += 1;

        // The mass, its centre and the box's inertia (`rigidbody` has none
        // of them), set again when the tuning moves
        let mass_key = (tuning.mass, tuning.center_of_mass, chassis_size);
        if self.mass_set != Some(mass_key) {
            let body = &mut physics.bodies_mut()[handle];
            body.set_additional_mass_properties(mass_props(tuning, chassis_size), true);
            body.enable_ccd(true);
            self.mass_set = Some(mass_key);
        }

        // A `vehicle_place`: there, at rest
        if let Some(([x, y, z], yaw)) = self.place.take() {
            let body = &mut physics.bodies_mut()[handle];
            body.set_position(
                Isometry::from_parts(vector![x, y, z].into(), rotation_of_yaw(yaw)),
                true,
            );
            body.set_linvel(V3::zeros(), true);
            body.set_angvel(V3::zeros(), true);
            self.prev_velocity = None;
            self.steer = 0.0;
        }

        // What the body is as this step begins: the state scripts read
        let (rot, vel, angvel, mass) = {
            let b = &physics.bodies()[handle];
            (*b.rotation(), *b.linvel(), *b.angvel(), b.mass())
        };
        let fwd = rot * -V3::z();
        let right = rot * V3::x();
        let up = rot * V3::y();
        let speed = vel.dot(&fwd);
        let flat = V3::new(vel.x, 0.0, vel.z);
        let slip = if flat.norm() > 0.5 {
            let vh = flat.normalize();
            // Positive: the nose is right of the travel heading
            (vh.x * fwd.z - vh.z * fwd.x).atan2(vh.dot(&fwd))
        } else {
            0.0
        };
        let accel = self
            .prev_velocity
            .map(|p| (vel - p) / dt)
            .unwrap_or_else(V3::zeros);
        self.prev_velocity = Some(vel);

        // The input as it drives: a brake near standstill reverses
        let mut throttle = input.throttle;
        let mut brake = input.brake;
        let mut steer_in = input.steer;
        if input.hold {
            throttle = 0.0;
            brake = 1.0;
            steer_in = 0.0;
        } else if brake > 0.0 && speed <= REVERSE_BELOW && throttle <= 0.0 {
            throttle = -brake;
            brake = 0.0;
        } else if throttle < 0.0 && speed > REVERSE_BELOW {
            brake = brake.max(-throttle);
            throttle = 0.0;
        }
        self.steer = if tuning.steer_response > 0.0 {
            let most = tuning.steer_response * dt;
            self.steer + (steer_in - self.steer).clamp(-most, most)
        } else {
            steer_in
        };
        let steer = self.steer;

        // The cap: top speed (reversing: its fraction), scaled and bonused
        let cap = if throttle < 0.0 || speed < -REVERSE_BELOW {
            tuning.top_speed * tuning.reverse * input.speed_scale
        } else {
            (tuning.top_speed * input.speed_scale + input.speed_bonus).max(0.0)
        };

        // Steering geometry asks the path the turn-rate curve would drive
        let rate = tuning.yaw_rate(speed, input.turn_scale).to_radians();
        let radius = speed.abs().max(MIN_TURN_SPEED) / rate.max(1e-4);
        let max_angle = (self.wheelbase() / radius).atan();
        let drive_wheels = self.wheels.iter().filter(|w| w.drive).count().max(1) as f32;
        let brake_wheels = self.wheels.iter().filter(|w| w.brake).count().max(1) as f32;
        let hand_wheels = self.wheels.iter().filter(|w| w.handbrake).count().max(1) as f32;
        let accel_now = if throttle < 0.0 {
            tuning.accel * tuning.reverse
        } else {
            tuning.accel
        };
        // The engine eases off over the last half metre a second under the
        // cap, so drag and drive meet just under it rather than chattering
        let under = if throttle >= 0.0 {
            cap - speed
        } else {
            cap + speed
        };
        let ease = (under / CAP_EASE).clamp(0.0, 1.0);
        let engine = throttle * mass * accel_now * ease / drive_wheels;
        let wheel_tuning = wheel_tuning(tuning);
        let controller = self.controller.as_mut().expect("made above");
        for (spec, wheel) in self.wheels.iter().zip(controller.wheels_mut()) {
            wheel.radius = spec.radius;
            wheel.suspension_rest_length = spec.rest_length;
            wheel.suspension_stiffness = wheel_tuning.suspension_stiffness;
            wheel.damping_compression = wheel_tuning.suspension_compression;
            wheel.damping_relaxation = wheel_tuning.suspension_damping;
            wheel.max_suspension_travel = wheel_tuning.max_suspension_travel;
            wheel.friction_slip = wheel_tuning.friction_slip;
            wheel.steering = if spec.steer { -steer * max_angle } else { 0.0 };
            wheel.engine_force = if spec.drive { engine } else { 0.0 };
            let mut b = if spec.brake {
                brake * mass * tuning.brake * dt / brake_wheels
            } else {
                0.0
            };
            let locked = input.handbrake && spec.handbrake;
            if locked {
                b = b.max(mass * tuning.brake * dt / hand_wheels);
            }
            wheel.brake = b;
            wheel.side_friction_stiffness = if locked { HANDBRAKE_SIDE_FRICTION } else { 1.0 };
        }

        // Rapier's raycast wheels, against the last step's colliders, the
        // vehicle's own body and every sensor left out
        let filter = QueryFilter::default()
            .exclude_rigid_body(handle)
            .exclude_sensors();
        {
            let sets = physics.rapier_mut();
            controller.update_vehicle(dt, sets.bodies, sets.colliders, sets.queries, filter);
        }

        // What each wheel saw
        let mut ground_colliders = Vec::new();
        let mut contacts = Vec::new();
        for (i, (spec, wheel)) in self.wheels.iter().zip(controller.wheels()).enumerate() {
            let info = wheel.raycast_info();
            let travel = wheel.max_suspension_travel.max(1e-4);
            let mut seen = WheelSeen {
                contact: info.is_in_contact,
                ground: None,
                normal: arr(&info.contact_normal_ws),
                compression: 0.0,
                slip: 0.0,
                length: info.suspension_length,
                steering: wheel.steering,
                rotation: wheel.rotation,
            };
            if info.is_in_contact {
                contacts.push(info.contact_normal_ws);
                if let Some(c) = info.ground_object {
                    ground_colliders.push(c);
                    seen.ground = physics.entity_of_collider(c);
                }
                let longest = spec.rest_length + travel;
                seen.compression =
                    ((longest - info.suspension_length) / (2.0 * travel)).clamp(0.0, 1.0);
                // The wheel's heading over the ground against its travel there
                let heading =
                    UnitQuaternion::from_axis_angle(&Vector::y_axis(), wheel.steering) * -V3::z();
                let heading = rot * heading;
                let at = physics.bodies()[handle].velocity_at_point(&info.contact_point_ws);
                let n = info.contact_normal_ws;
                let along = at - n * at.dot(&n);
                if along.norm() > 0.5 {
                    let h = heading - n * heading.dot(&n);
                    let side = h.cross(&n);
                    seen.slip = (-along.dot(&side)).atan2(along.dot(&h)).to_degrees();
                }
            }
            self.wheel_seen[i] = seen;
        }
        let grounded = contacts.len();

        // The assists, the modifiers, the drag and the push
        let inertia = {
            let b = &physics.bodies()[handle];
            b.mass_properties().local_mprops.principal_inertia()
        };
        let mut yaw_target = 0.0;
        let body = &mut physics.bodies_mut()[handle];
        body.wake_up(true);
        let fwd_flat = V3::new(fwd.x, 0.0, fwd.z);
        let fwd_flat = if fwd_flat.norm() > 1e-4 {
            fwd_flat.normalize()
        } else {
            fwd
        };
        if tuning.drag > 0.0 {
            body.apply_impulse(-fwd_flat * (speed * tuning.drag * mass * dt), true);
        }
        let over = speed.abs() - cap;
        if over > 0.0 && tuning.overspeed_decay > 0.0 {
            let dv = over * (tuning.overspeed_decay * dt).min(1.0);
            body.apply_impulse(-fwd_flat * (speed.signum() * dv * mass), true);
        }
        if grounded >= 2 {
            let n = contacts.iter().fold(V3::zeros(), |a, c| a + c).normalize();
            let want = input.slip_target.to_radians();
            if input.accel_bonus != 0.0 {
                let tangent = (fwd - n * fwd.dot(&n)).normalize();
                if input.accel_bonus > 0.0 && speed < cap {
                    body.apply_impulse(tangent * (input.accel_bonus * mass * dt), true);
                } else if input.accel_bonus < 0.0 && speed > 0.0 {
                    let dv = (-input.accel_bonus * dt).min(speed);
                    body.apply_impulse(-tangent * (dv * mass), true);
                }
            }
            if tuning.yaw_assist > 0.0 {
                let sign = if speed < -0.5 { -1.0 } else { 1.0 };
                let mut target = -steer * sign * rate;
                if want != 0.0 {
                    target -= SLIP_HOLD * (want - slip);
                }
                if input.spin != 0.0 {
                    target = input.spin.to_radians();
                }
                yaw_target = target.to_degrees();
                let wy = angvel.dot(&n);
                let dw = (target - wy) * (tuning.yaw_assist * dt).clamp(0.0, 1.0);
                body.apply_torque_impulse(n * (dw * inertia.y), true);
            }
            let grip = tuning.grip * input.grip_scale;
            if grip > 0.0 {
                // Scrub sideways of the travel heading, which a slip target
                // holds off the nose
                let travel = if want != 0.0 {
                    UnitQuaternion::from_axis_angle(&Unit::new_normalize(n), want) * fwd
                } else {
                    fwd
                };
                let side = travel.cross(&n).normalize();
                let vlat = body.linvel().dot(&side);
                let dv = -vlat * (grip * dt).clamp(0.0, 1.0);
                body.apply_impulse(side * (dv * mass), true);
            }
            if tuning.scrub > 0.0 && speed.abs() > 0.5 {
                let dv = (speed.abs() * tuning.scrub * slip.abs() * dt).min(speed.abs());
                body.apply_impulse(-fwd_flat * (speed.signum() * dv * mass), true);
            }
            if tuning.upright > 0.0 {
                let axis = up.cross(&n);
                let perp = angvel - n * angvel.dot(&n);
                let k = tuning.upright;
                let c = 2.0 * k.sqrt();
                let torque = (axis * k - perp * c) * (inertia.y * dt);
                body.apply_torque_impulse(torque, true);
            }
        }
        let [px, py, pz] = input.push;
        if px != 0.0 || py != 0.0 || pz != 0.0 {
            body.apply_impulse((right * px + up * py + fwd * pz) * mass, true);
        }

        // The chassis against anything but the ground under its wheels
        let contact = chassis_contact(physics, handle, &ground_colliders);
        let was = self.snapshot.contact;
        let impact = match (&contact, was) {
            (Some((n, other)), false) => {
                let before = accel * -dt + vel;
                let theirs = other_velocity(physics, *other);
                (-(before - theirs).dot(n)).max(0.0)
            }
            _ => self.snapshot.impact,
        };
        let airborne = if grounded > 0 {
            0.0
        } else {
            self.snapshot.airborne_time + dt
        };
        self.snapshot = Snapshot {
            speed,
            velocity: arr(&vel),
            forward: arr(&fwd),
            lateral_speed: vel.dot(&right),
            slip_angle: slip.to_degrees(),
            yaw: yaw_of(arr(&fwd)),
            yaw_rate: angvel.y.to_degrees(),
            accel_long: accel.dot(&fwd),
            accel_lat: accel.dot(&right),
            wheels_grounded: grounded as u32,
            airborne_time: airborne,
            contact: contact.is_some(),
            contact_normal: contact.as_ref().map_or([0.0; 3], |(n, _)| arr(n)),
            contact_entity: contact.and_then(|(_, c)| physics.entity_of_collider(c)),
            impact,
            yaw_target,
            max_steer: max_angle.to_degrees(),
        };
        true
    }
}

/// Rapier's wheel tuning from the vehicle's.
fn wheel_tuning(t: &Tuning) -> WheelTuning {
    WheelTuning {
        suspension_stiffness: t.suspension_stiffness,
        suspension_compression: t.suspension_damping * 0.7,
        suspension_damping: t.suspension_damping,
        max_suspension_travel: t.suspension_travel,
        side_friction_stiffness: 1.0,
        friction_slip: t.tyre_grip,
        max_suspension_force: 100_000.0,
    }
}

/// The chassis' mass at its centre, with a solid box's inertia.
fn mass_props(t: &Tuning, [w, h, l]: [f32; 3]) -> MassProperties {
    let m = t.mass;
    let inertia = vector![
        m / 12.0 * (h * h + l * l),
        m / 12.0 * (w * w + l * l),
        m / 12.0 * (w * w + h * h)
    ];
    let [cx, cy, cz] = t.center_of_mass;
    MassProperties::new(point![cx, cy, cz], m, inertia)
}

/// The velocity of the body a collider is on (zero for a fixed one).
fn other_velocity(physics: &PhysicsSystem, collider: ColliderHandle) -> V3 {
    physics
        .colliders()
        .get(collider)
        .and_then(|c| c.parent())
        .and_then(|b| physics.bodies().get(b))
        .map_or_else(V3::zeros, |b| *b.linvel())
}

/// The nearest touch between the vehicle's colliders and any other solid
/// collider that isn't the ground under its wheels: its normal (pointing
/// at the chassis) and the collider touched. From the last step's
/// colliders, as the wheels' rays are: Rapier's narrow phase isn't lent to
/// a package (#1649).
fn chassis_contact(
    physics: &PhysicsSystem,
    handle: RigidBodyHandle,
    ground: &[ColliderHandle],
) -> Option<(V3, ColliderHandle)> {
    let body = physics.bodies().get(handle)?;
    let colliders = physics.colliders();
    let mut best: Option<(f32, V3, ColliderHandle)> = None;
    for own in body.colliders() {
        let Some(mine) = colliders.get(*own) else {
            continue;
        };
        if mine.is_sensor() {
            continue;
        }
        let aabb = mine.compute_aabb().loosened(CONTACT_REACH);
        physics
            .queries()
            .colliders_with_aabb_intersecting_aabb(&aabb, |other| {
                let Some(theirs) = colliders.get(*other) else {
                    return true;
                };
                if theirs.parent() == Some(handle) || theirs.is_sensor() || ground.contains(other) {
                    return true;
                }
                if let Ok(Some(c)) = parry_query::contact(
                    mine.position(),
                    mine.shape(),
                    theirs.position(),
                    theirs.shape(),
                    CONTACT_REACH,
                ) {
                    let normal = -c.normal1.into_inner();
                    if best.is_none_or(|(d, _, _)| c.dist < d) {
                        best = Some((c.dist, normal, *other));
                    }
                }
                true
            });
    }
    best.map(|(_, n, c)| (n, c))
}

/// A wheel child's local pose for its spring, steering and spin, in the
/// vehicle's frame: position, and rotation as `[x, y, z, w]`.
pub fn wheel_pose(spec: &WheelSpec, seen: &WheelSeen) -> ([f32; 3], [f32; 4]) {
    let [x, y, z] = spec.attach;
    let length = if seen.length > 0.0 {
        seen.length
    } else {
        spec.rest_length
    };
    let steer = UnitQuaternion::from_axis_angle(&Vector::y_axis(), seen.steering);
    let spin = UnitQuaternion::from_axis_angle(&Vector::x_axis(), seen.rotation);
    let q = steer * spin;
    ([x, y - length, z], [q.i, q.j, q.k, q.w])
}
