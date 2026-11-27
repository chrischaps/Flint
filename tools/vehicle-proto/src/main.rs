//! The vehicle ADR's prototype (Flint #1627).
//!
//! FlintKart's Oval Plus (`splines/oval_plus.spline.toml`, a 12 m road with
//! walls at +-6.25 m) is built in Rapier 0.22, the engine's version, with the
//! engine's integration parameters. One autopilot drives four kart models
//! around it, three laps from a standing start, at three frame rates:
//!
//! - `arcade_frame`: FlintKart's `kart_controller.rhai` ported to Rust,
//!   stepped once a frame at the frame's dt (clamped to 0.05 s), as today.
//! - `arcade_fixed`: the same port, stepped at the fixed 1/60 s (option B).
//! - `rapier_raw`: Rapier's `DynamicRayCastVehicleController` on a dynamic
//!   body, tyre friction only (option A without assists).
//! - `rapier_assist`: the same with arcade assists (option A): yaw-rate
//!   assist toward the arcade turn rate, lateral grip, an upright torque.
//!
//! The port leaves out what stays FlintKart's (drift, hop, boost, items,
//! slipstream, kart bumps, respawn, body attitude). Output: one JSON line a
//! run on stdout, and `runs.json` with each run's lap-2 path.

use rapier3d::na::{Point3, Rotation3, Unit, UnitQuaternion, Vector3};
use rapier3d::control::{DynamicRayCastVehicleController, WheelTuning};
use rapier3d::prelude::*;
use std::f32::consts::PI;

type V3 = Vector3<f32>;

// ---------------------------------------------------------------- track

const CONTROL: [([f32; 3], f32); 15] = [
    ([0.0, 0.0, 0.0], 0.0),
    ([0.0, 0.0, -50.0], 0.0),
    ([0.0, 0.0, -100.0], 0.0),
    ([50.0, 0.0, -140.0], 5.0),
    ([100.0, 0.0, -100.0], 0.0),
    ([100.0, 0.0, -50.0], 0.0),
    ([100.0, 6.0, 0.0], 0.0),
    ([100.0, 8.0, 30.0], 0.0),
    ([100.0, 4.0, 60.0], 0.0),
    ([100.0, 0.0, 90.0], 0.0),
    ([80.0, 0.0, 120.0], 0.0),
    ([50.0, 0.0, 140.0], 15.0),
    ([20.0, 0.0, 120.0], 5.0),
    ([0.0, 0.0, 90.0], 0.0),
    ([0.0, 0.0, 50.0], 0.0),
];

#[derive(Clone, Copy)]
struct Sample {
    pos: V3,
    fwd: V3,
    right: V3,
    up: V3,
}

fn catmull_rom(p0: V3, p1: V3, p2: V3, p3: V3, t: f32) -> V3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn cr_scalar(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// `flint_geometry::spline::sample_closed_spline`, ported line for line.
fn sample_track(spacing: f32) -> Vec<Sample> {
    let pts: Vec<(V3, f32)> = CONTROL
        .iter()
        .map(|(p, tw)| (V3::new(p[0], p[1], p[2]), *tw))
        .collect();
    let n = pts.len();
    let mut total = 0.0;
    let mut prev = pts[0].0;
    for seg in 0..n {
        let (p0, p1, p2, p3) = (
            pts[(seg + n - 1) % n].0,
            pts[seg].0,
            pts[(seg + 1) % n].0,
            pts[(seg + 2) % n].0,
        );
        for j in 1..=20 {
            let pos = catmull_rom(p0, p1, p2, p3, j as f32 / 20.0);
            total += (pos - prev).norm();
            prev = pos;
        }
    }
    let count = ((total / spacing).ceil() as usize).max(20);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let gt = i as f32 / count as f32;
        let scaled = gt * n as f32;
        let seg = scaled.floor() as usize % n;
        let lt = scaled - scaled.floor();
        let idx = |k: usize| pts[k % n];
        let (a, b, c, d) = (idx(seg + n - 1), idx(seg), idx(seg + 1), idx(seg + 2));
        let pos = catmull_rom(a.0, b.0, c.0, d.0, lt);
        let nt = lt + 0.001;
        let next = if nt <= 1.0 {
            catmull_rom(a.0, b.0, c.0, d.0, nt)
        } else {
            let s = seg + 1;
            catmull_rom(idx(s + n - 1).0, idx(s).0, idx(s + 1).0, idx(s + 2).0, nt - 1.0)
        };
        let fwd = (next - pos).normalize();
        let twist = cr_scalar(a.1, b.1, c.1, d.1, lt).to_radians();
        let right_flat = fwd.cross(&V3::y()).normalize();
        let up_flat = right_flat.cross(&fwd).normalize();
        let axis = Unit::new_normalize(fwd);
        let rot = UnitQuaternion::from_axis_angle(&axis, twist);
        out.push(Sample {
            pos,
            fwd,
            right: rot * right_flat,
            up: rot * up_flat,
        });
    }
    out
}

// ---------------------------------------------------------------- world

struct World {
    gravity: V3,
    params: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad: DefaultBroadPhase,
    narrow: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    joints: ImpulseJointSet,
    multibody: MultibodyJointSet,
    ccd: CCDSolver,
    queries: QueryPipeline,
    walls: Vec<ColliderHandle>,
}

impl World {
    /// The engine's world (`crates/flint-physics/src/world.rs`): gravity
    /// -9.81, one solver iteration of four PGS iterations.
    fn new() -> Self {
        let mut params = IntegrationParameters::default();
        params.num_solver_iterations = std::num::NonZeroUsize::new(1).unwrap();
        params.num_internal_pgs_iterations = 4;
        Self {
            gravity: vector![0.0, -9.81, 0.0],
            params,
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad: DefaultBroadPhase::new(),
            narrow: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            joints: ImpulseJointSet::new(),
            multibody: MultibodyJointSet::new(),
            ccd: CCDSolver::new(),
            queries: QueryPipeline::new(),
            walls: Vec::new(),
        }
    }

    fn step(&mut self, dt: f32) {
        self.params.dt = dt;
        self.pipeline.step(
            &self.gravity,
            &self.params,
            &mut self.islands,
            &mut self.broad,
            &mut self.narrow,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.joints,
            &mut self.multibody,
            &mut self.ccd,
            Some(&mut self.queries),
            &(),
            &(),
        );
    }

    /// The road (`track_road`: width 12, top at the spline, friction 0.8,
    /// restitution 0.1) and its walls (0.5 x 1.5 at +-6.25, friction 0.3,
    /// restitution 0.5); or, for the skidpad, a plane.
    fn build_track(&mut self, track: &[Sample]) {
        let n = track.len();
        let mut verts = Vec::new();
        let mut idx = Vec::new();
        for s in track {
            let l = s.pos - s.right * 6.0;
            let r = s.pos + s.right * 6.0;
            verts.push(Point3::from(l));
            verts.push(Point3::from(r));
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b, c, d) = (2 * i as u32, 2 * i as u32 + 1, 2 * j as u32, 2 * j as u32 + 1);
            idx.push([a, c, b]);
            idx.push([b, c, d]);
        }
        let road = ColliderBuilder::trimesh(verts, idx)
            .friction(0.8)
            .restitution(0.1)
            .build();
        self.colliders.insert(road);
        for i in 0..n {
            let s0 = track[i];
            let s1 = track[(i + 1) % n];
            for side in [-1.0f32, 1.0] {
                let a = s0.pos + s0.right * 6.25 * side + s0.up * 0.75;
                let b = s1.pos + s1.right * 6.25 * side + s1.up * 0.75;
                let mid = (a + b) * 0.5;
                let along = b - a;
                let len = along.norm();
                let back = -along / len;
                let up = s0.up;
                let right = up.cross(&back).normalize();
                let up = back.cross(&right).normalize();
                let rot = Rotation3::from_basis_unchecked(&[right, up, back]);
                let iso = Isometry::from_parts(mid.into(), UnitQuaternion::from_rotation_matrix(&rot));
                let wall = ColliderBuilder::cuboid(0.25, 0.75, len * 0.5 + 0.05)
                    .position(iso)
                    .friction(0.3)
                    .restitution(0.5)
                    .build();
                let h = self.colliders.insert(wall);
                self.walls.push(h);
            }
        }
        self.queries.update(&self.colliders);
    }

    fn build_plane(&mut self) {
        let ground = ColliderBuilder::cuboid(2000.0, 0.5, 2000.0)
            .translation(vector![0.0, -0.5, 0.0])
            .friction(0.8)
            .build();
        self.colliders.insert(ground);
        self.queries.update(&self.colliders);
    }

    /// A ray against the static world only: (distance, normal).
    fn ray(&self, from: V3, dir: V3, len: f32) -> Option<(f32, V3)> {
        let ray = Ray::new(from.into(), dir);
        self.queries
            .cast_ray_and_get_normal(
                &self.bodies,
                &self.colliders,
                &ray,
                len,
                true,
                QueryFilter::only_fixed(),
            )
            .map(|(_, hit)| (hit.time_of_impact, hit.normal))
    }
}

// ---------------------------------------------------------------- tuning

/// `kart_physics` with FlintKart's defaults, `ai1`'s overrides
/// (`scenes/oval_plus.scene.toml`: max_speed 33, acceleration 16.5).
#[derive(Clone, Copy)]
struct Tune {
    max_speed: f32,
    accel: f32,
    brake_decel: f32,
    drag: f32,
    turn_low: f32,
    turn_high: f32,
    gravity: f32,
    ray_len: f32,
    steer_response: f32,
    min_turn_radius: f32,
    grip: f32,
    overspeed_decay: f32,
    wall_restitution: f32,
    wall_friction: f32,
    wall_align: f32,
}

const AI1: Tune = Tune {
    max_speed: 33.0,
    accel: 16.5,
    brake_decel: 25.0,
    drag: 0.5,
    turn_low: 180.0,
    turn_high: 60.0,
    gravity: -20.0,
    ray_len: 2.0,
    steer_response: 7.0,
    min_turn_radius: 2.6,
    grip: 9.0,
    overspeed_decay: 2.5,
    wall_restitution: 0.25,
    wall_friction: 0.3,
    wall_align: 5.0,
};

impl Tune {
    /// The arcade yaw rate at a speed, degrees a second: the turn-rate
    /// lerp, capped by the minimum turn radius (`kart_controller.rhai`).
    fn yaw_rate(&self, speed: f32) -> f32 {
        let a = speed.abs();
        let ratio = (a / self.max_speed).clamp(0.0, 1.0);
        let rate = self.turn_low + (self.turn_high - self.turn_low) * ratio;
        let cap = (a.max(2.5) / self.min_turn_radius).to_degrees();
        rate.min(cap)
    }
}

#[derive(Clone, Copy, Default)]
struct Input {
    throttle: f32,
    brake: f32,
    steer: f32,
    /// FlintKart's drift button: the game's logic, here only to show that
    /// each model can express it.
    drift: bool,
}

/// FlintKart's drift angle target, degrees: 15, up to 25 steering into
/// the drift, down to 8 counter-steering (`kart_controller.rhai`).
fn drift_target(steer: f32, dir: f32) -> f32 {
    let into = steer * dir;
    dir * if into > 0.0 { 15.0 + into * 10.0 } else { 15.0 + into * 7.0 }
}

/// What every model tells the autopilot and the measures.
#[derive(Clone, Copy)]
struct Pose {
    pos: V3,
    /// Horizontal body forward.
    fwd: V3,
    /// Signed speed along `fwd`.
    speed: f32,
    vel: V3,
    wall: bool,
}

trait Model {
    fn name(&self) -> &'static str;
    /// One frame of `frame_dt` seconds with this frame's input.
    fn frame(&mut self, w: &mut World, input: Input, frame_dt: f32);
    fn pose(&self, w: &World) -> Pose;
    /// The fastest speed the autopilot plans for a bend of curvature `k`.
    fn corner_speed(&self, k: f32) -> f32;
    fn brake_decel(&self) -> f32;
}

fn fwd_from_yaw(yaw: f32) -> V3 {
    let r = yaw.to_radians();
    V3::new(-r.sin(), 0.0, -r.cos())
}
fn right_from_yaw(yaw: f32) -> V3 {
    let r = yaw.to_radians();
    V3::new(r.cos(), 0.0, -r.sin())
}
fn wrap(a: f32) -> f32 {
    a.rem_euclid(360.0)
}
fn delta(a: f32) -> f32 {
    let w = wrap(a);
    if w > 180.0 {
        w - 360.0
    } else {
        w
    }
}

/// The arcade corner speed: v * k <= 0.85 * yaw rate (ai_driver's grip).
fn arcade_corner_speed(t: &Tune, k: f32) -> f32 {
    let k = k.abs();
    if k < 1e-4 {
        return t.max_speed;
    }
    let (mut lo, mut hi) = (0.0f32, t.max_speed);
    for _ in 0..40 {
        let v = 0.5 * (lo + hi);
        if v * k <= 0.85 * t.yaw_rate(v).to_radians() {
            lo = v;
        } else {
            hi = v;
        }
    }
    lo
}

// ---------------------------------------------------------------- arcade port

/// `kart_controller.rhai`'s driving, without what stays FlintKart's.
struct Arcade {
    t: Tune,
    fixed: bool,
    acc: f32,
    pos: V3,
    yaw: f32,
    speed: f32,
    lat: f32,
    vy: f32,
    steering: f32,
    wall: bool,
    vel: V3,
    drift_dir: f32,
    drift_angle: f32,
}

impl Arcade {
    fn new(t: Tune, fixed: bool, pos: V3) -> Self {
        Self { t, fixed, acc: 0.0, pos, yaw: 0.0, speed: 0.0, lat: 0.0, vy: 0.0, steering: 0.0, wall: false, vel: V3::zeros(), drift_dir: 0.0, drift_angle: 0.0 }
    }

    fn step(&mut self, w: &World, input: Input, dt: f32) {
        let t = self.t;
        let (mut px, mut py, mut pz) = (self.pos.x, self.pos.y, self.pos.z);
        let (start_x, start_z) = (px, pz);
        let hit = w.ray(V3::new(px, py + 0.5, pz), -V3::y(), t.ray_len);
        let grounded = hit.is_some();
        let ground_y = hit.map(|(d, _)| py + 0.5 - d).unwrap_or(py);

        let mut throttle = input.throttle;
        let mut brake = 0.0;
        if input.brake > 0.0 {
            if self.speed > 1.0 {
                brake = input.brake;
            } else {
                throttle = -0.3 * input.brake;
            }
        }
        let steer_in = input.steer;
        let mut rate = t.steer_response;
        if steer_in.abs() < self.steering.abs() {
            rate *= 1.5;
        }
        self.steering += (steer_in - self.steering).clamp(-rate * dt, rate * dt);

        let prev_travel = self.yaw + self.drift_angle;
        let abs_speed = self.speed.abs();
        // The drift, simplified: no hop, no charge, no boost
        if input.drift && self.drift_dir == 0.0 && grounded && abs_speed > 8.0 && steer_in.abs() > 0.3 {
            self.drift_dir = steer_in.signum();
        }
        if !input.drift || abs_speed < 3.0 {
            self.drift_dir = 0.0;
        }
        let drifting = self.drift_dir != 0.0;
        if drifting {
            let target = drift_target(self.steering, self.drift_dir);
            self.drift_angle += (target - self.drift_angle) * (3.5 * dt).clamp(0.0, 1.0);
            self.speed -= 0.6 * dt;
        } else {
            self.drift_angle *= 1.0 - (8.0 * dt).clamp(0.0, 1.0);
        }
        if grounded && (abs_speed > 0.5 || throttle.abs() > 0.1) {
            let mut turn = t.yaw_rate(abs_speed);
            if drifting {
                turn *= 1.2;
            }
            let mut sign = if self.speed < 0.0 { -1.0 } else { 1.0 };
            if abs_speed <= 0.5 {
                sign = if throttle < 0.0 { -1.0 } else { 1.0 };
            }
            self.yaw = wrap(self.yaw - self.steering * sign * turn * dt);
        }
        // Slip: last step's velocity held while the heading turns under it
        let (of, or) = (fwd_from_yaw(prev_travel), right_from_yaw(prev_travel));
        let wv = of * self.speed + or * self.lat;
        let travel = self.yaw + self.drift_angle;
        let (nf, nr) = (fwd_from_yaw(travel), right_from_yaw(travel));
        self.speed = wv.dot(&nf);
        self.lat = wv.dot(&nr);

        let eff_max = t.max_speed;
        if grounded {
            if brake > 0.0 {
                let f = t.brake_decel * brake;
                if self.speed > 0.0 {
                    self.speed = (self.speed - f * dt).max(0.0);
                } else {
                    self.speed = (self.speed + f * dt).min(0.0);
                }
            } else {
                self.speed += throttle * t.accel * dt;
            }
            self.speed -= self.speed * t.drag * dt;
            if self.speed > eff_max {
                self.speed -= (self.speed - eff_max) * (t.overspeed_decay * dt).clamp(0.0, 1.0);
            }
            self.speed = self.speed.clamp(-t.max_speed * 0.3, eff_max + 15.0);
            let grip = if drifting { 3.5 } else { t.grip };
            self.lat *= 1.0 - (grip * dt).clamp(0.0, 1.0);
            if self.vy > 0.0 || (self.vy < 0.0 && py > ground_y) {
                self.vy += t.gravity * dt;
                if py + self.vy * dt <= ground_y {
                    py = ground_y;
                    self.vy = 0.0;
                }
            } else {
                let gap = (ground_y - py).abs();
                if gap > 1.0 {
                    py = ground_y;
                } else {
                    py += (ground_y - py) * (25.0 * dt).clamp(0.0, 1.0);
                }
                self.vy = 0.0;
            }
        } else {
            self.vy += t.gravity * dt;
        }
        let tf = fwd_from_yaw(travel);
        let tr = right_from_yaw(travel);
        let mut v = tf * self.speed + tr * self.lat;

        // Swept wall check
        let ray_y = py + 0.5;
        let mut step = V3::new(v.x * dt, 0.0, v.z * dt);
        let sl = step.norm();
        if sl > 0.05 {
            let sd = step / sl;
            if let Some((d, n)) = w.ray(V3::new(start_x, ray_y, start_z), sd, sl + 0.3) {
                if n.y.abs() < 0.6 {
                    let allowed = (d - 0.3).max(0.0);
                    if allowed < sl {
                        step = sd * allowed;
                    }
                }
            }
        }
        px += step.x;
        py += self.vy * dt;
        pz += step.z;

        // Radial wall rays
        let (radius, ray_len) = (0.9f32, 1.3f32);
        let dirs = [(1.0, 0.0), (0.707, 0.707), (0.0, 1.0), (-0.707, 0.707), (-1.0, 0.0), (-0.707, -0.707), (0.0, -1.0), (0.707, -0.707)];
        let (mut push_x, mut push_z, mut wnx, mut wnz) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut contact = false;
        for (dx, dz) in dirs {
            if let Some((d, n)) = w.ray(V3::new(px, py + 0.5, pz), V3::new(dx, 0.0, dz), ray_len) {
                if n.y.abs() < 0.6 && d < radius {
                    contact = true;
                    let nl = (n.x * n.x + n.z * n.z).sqrt().max(1e-3);
                    let (nx, nz) = (n.x / nl, n.z / nl);
                    let pen = radius - d;
                    if push_x * nx + push_z * nz < pen {
                        push_x += nx * pen;
                        push_z += nz * pen;
                    }
                    wnx += nx;
                    wnz += nz;
                    let impact = -(v.x * nx + v.z * nz);
                    if impact > 0.0 {
                        v.x += nx * impact * (1.0 + t.wall_restitution);
                        v.z += nz * impact * (1.0 + t.wall_restitution);
                        let dot = v.x * nx + v.z * nz;
                        let (tvx, tvz) = (v.x - nx * dot, v.z - nz * dot);
                        let tl = (tvx * tvx + tvz * tvz).sqrt();
                        if tl > 0.01 {
                            let loss = t.wall_friction * impact * (1.0 + t.wall_restitution);
                            let scrub = (loss / tl).clamp(0.0, 1.0);
                            v.x -= tvx * scrub;
                            v.z -= tvz * scrub;
                        }
                    }
                }
            }
        }
        px += push_x;
        pz += push_z;
        if contact && self.speed.abs() > 2.0 {
            let wl = (wnx * wnx + wnz * wnz).sqrt();
            if wl > 1e-3 {
                let (wx, wz) = (wnx / wl, wnz / wl);
                let bf = fwd_from_yaw(self.yaw);
                let into = bf.x * wx + bf.z * wz;
                if into < 0.0 && into > -0.7 {
                    let (tx, tz) = (bf.x - wx * into, bf.z - wz * into);
                    if (tx * tx + tz * tz).sqrt() > 0.2 {
                        let along = (-tx).atan2(-tz).to_degrees();
                        let dy = delta(along - self.yaw);
                        self.yaw = wrap(self.yaw + dy * (t.wall_align * dt).clamp(0.0, 1.0));
                    }
                }
            }
        }
        let tf = fwd_from_yaw(self.yaw + self.drift_angle);
        let tr = right_from_yaw(self.yaw + self.drift_angle);
        self.speed = v.x * tf.x + v.z * tf.z;
        self.lat = v.x * tr.x + v.z * tr.z;
        self.speed = self.speed.max(-t.max_speed * 0.3);
        self.vel = V3::new(v.x, self.vy, v.z);
        self.pos = V3::new(px, py, pz);
        self.wall = contact;
    }
}

impl Model for Arcade {
    fn name(&self) -> &'static str {
        if self.fixed { "arcade_fixed" } else { "arcade_frame" }
    }
    fn frame(&mut self, w: &mut World, input: Input, frame_dt: f32) {
        if self.fixed {
            self.acc += frame_dt;
            let mut wall = false;
            while self.acc >= FIXED - 1e-6 {
                self.step(w, input, FIXED);
                wall |= self.wall;
                self.acc -= FIXED;
            }
            self.wall = wall;
        } else {
            self.step(w, input, frame_dt.min(0.05));
        }
    }
    fn pose(&self, _w: &World) -> Pose {
        Pose { pos: self.pos, fwd: fwd_from_yaw(self.yaw), speed: self.speed, vel: self.vel, wall: self.wall }
    }
    fn corner_speed(&self, k: f32) -> f32 {
        arcade_corner_speed(&self.t, k)
    }
    fn brake_decel(&self) -> f32 {
        self.t.brake_decel * 0.7
    }
}

const FIXED: f32 = 1.0 / 60.0;

// ---------------------------------------------------------------- Rapier vehicle

/// Assists, each 0 for off. The yaw assist pulls the yaw rate toward the
/// arcade turn rate (per second); grip scrubs sideways speed (per second);
/// upright pulls the body's up toward the ground's (per second squared).
#[derive(Clone, Copy)]
struct Assist {
    yaw: f32,
    grip: f32,
    upright: f32,
}

struct Dynamic {
    name: &'static str,
    t: Tune,
    assist: Assist,
    mass: f32,
    mu: f32,
    g: f32,
    acc: f32,
    body: RigidBodyHandle,
    chassis: ColliderHandle,
    vehicle: DynamicRayCastVehicleController,
    wall: bool,
    inertia_y: f32,
    drift_dir: f32,
}

const WHEELBASE: f32 = 1.45;

impl Dynamic {
    fn new(w: &mut World, name: &'static str, t: Tune, assist: Assist, pos: V3) -> Self {
        let mass = 150.0;
        let g_scale = -t.gravity / 9.81;
        let inertia = vector![
            mass / 12.0 * (0.5f32.powi(2) + 2.0f32.powi(2)),
            mass / 12.0 * (1.2f32.powi(2) + 2.0f32.powi(2)),
            mass / 12.0 * (1.2f32.powi(2) + 0.5f32.powi(2))
        ];
        let rb = RigidBodyBuilder::dynamic()
            .translation(pos + V3::new(0.0, 0.7, 0.0))
            .gravity_scale(g_scale)
            .additional_mass_properties(MassProperties::new(point![0.0, -0.25, 0.0], mass, inertia))
            .can_sleep(false)
            .build();
        let body = w.bodies.insert(rb);
        let col = ColliderBuilder::cuboid(0.6, 0.25, 1.0)
            .density(0.0)
            .friction(0.3)
            .restitution(0.2)
            .build();
        let chassis = w.colliders.insert_with_parent(col, body, &mut w.bodies);
        let mu = 1.8;
        let tuning = WheelTuning {
            suspension_stiffness: 40.0,
            suspension_compression: 2.5,
            suspension_damping: 3.5,
            max_suspension_travel: 0.3,
            side_friction_stiffness: 1.0,
            friction_slip: mu,
            max_suspension_force: 100_000.0,
        };
        let mut vehicle = DynamicRayCastVehicleController::new(body);
        vehicle.index_forward_axis = 2;
        for (x, z) in [(-0.55, -0.75), (0.55, -0.75), (-0.55, 0.7), (0.55, 0.7)] {
            vehicle.add_wheel(point![x, 0.0, z], vector![0.0, -1.0, 0.0], vector![1.0, 0.0, 0.0], 0.35, 0.275, &tuning);
        }
        Self { name, t, assist, mass, mu, g: -t.gravity, acc: 0.0, body, chassis, vehicle, wall: false, inertia_y: inertia.y, drift_dir: 0.0 }
    }

    fn step(&mut self, w: &mut World, input: Input, dt: f32) {
        let t = self.t;
        let (fwd, up, vel, angvel) = {
            let b = &w.bodies[self.body];
            let rot = b.rotation();
            (rot * -V3::z(), rot * V3::y(), *b.linvel(), *b.angvel())
        };
        let speed = vel.dot(&fwd);
        // The steering geometry asks the path the arcade model would drive
        let r = speed.abs().max(2.5) / t.yaw_rate(speed).to_radians();
        let max_angle = (WHEELBASE / r).atan();
        let mut throttle = input.throttle;
        let mut brake = 0.0;
        if input.brake > 0.0 {
            if speed > 1.0 {
                brake = input.brake;
            } else {
                throttle = -0.3 * input.brake;
            }
        }
        // The drift as the game would ask it of the package: lower grip,
        // the turn rate x1.2, and a slip angle the yaw assist holds
        if input.drift && self.drift_dir == 0.0 && speed.abs() > 8.0 && input.steer.abs() > 0.3 {
            self.drift_dir = input.steer.signum();
        }
        if !input.drift || speed.abs() < 3.0 {
            self.drift_dir = 0.0;
        }
        let drifting = self.drift_dir != 0.0;
        let drive = throttle * self.mass * t.accel / 2.0;
        for (i, wheel) in self.vehicle.wheels_mut().iter_mut().enumerate() {
            if i < 2 {
                wheel.steering = -input.steer * max_angle;
            }
            wheel.side_friction_stiffness = if drifting && i >= 2 { 0.3 } else { 1.0 };
            wheel.engine_force = if i >= 2 { drive } else { 0.0 };
            wheel.brake = brake * self.mass * t.brake_decel * dt / 4.0;
        }
        self.vehicle.update_vehicle(
            dt,
            &mut w.bodies,
            &w.colliders,
            &w.queries,
            QueryFilter::only_fixed(),
        );
        let contacts: Vec<V3> = self
            .vehicle
            .wheels()
            .iter()
            .filter(|wh| wh.raycast_info().is_in_contact)
            .map(|wh| wh.raycast_info().contact_normal_ws)
            .collect();
        let b = &mut w.bodies[self.body];
        // Drag, as the arcade model's (it is what sets the top speed)
        let fwd_flat = V3::new(fwd.x, 0.0, fwd.z).normalize();
        b.apply_impulse(-fwd_flat * (speed * t.drag * self.mass * dt), true);
        if contacts.len() >= 2 {
            let n = contacts.iter().fold(V3::zeros(), |a, c| a + c).normalize();
            if self.assist.yaw > 0.0 {
                let sign = if speed < -0.5 { -1.0 } else { 1.0 };
                let mut target = -input.steer * sign * t.yaw_rate(speed).to_radians();
                if drifting {
                    // Nose to the right of the velocity, + (as drift_dir)
                    let vh = V3::new(vel.x, 0.0, vel.z).normalize();
                    let slip = (vh.x * fwd.z - vh.z * fwd.x).atan2(vh.dot(&fwd));
                    let want = drift_target(input.steer, self.drift_dir).to_radians();
                    target = target * 1.2 - 4.0 * (want - slip);
                }
                let wy = angvel.dot(&n);
                let dw = (target - wy) * (self.assist.yaw * dt).clamp(0.0, 1.0);
                b.apply_torque_impulse(n * (dw * self.inertia_y), true);
            }
            if self.assist.grip > 0.0 {
                // Scrub sideways of the travel heading, which a drift holds
                // off the nose, as the arcade model's lat is
                let travel = if drifting {
                    let want = drift_target(input.steer, self.drift_dir).to_radians();
                    UnitQuaternion::from_axis_angle(&Unit::new_normalize(n), want) * fwd
                } else {
                    fwd
                };
                let right = travel.cross(&n).normalize();
                let vlat = b.linvel().dot(&right);
                let rate = if drifting { 3.5 } else { self.assist.grip };
                let dv = -vlat * (rate * dt).clamp(0.0, 1.0);
                b.apply_impulse(right * (dv * self.mass), true);
            }
            if self.assist.upright > 0.0 {
                let axis = up.cross(&n);
                let perp = angvel - n * angvel.dot(&n);
                let k = self.assist.upright;
                let c = 2.0 * k.sqrt();
                let torque = (axis * k - perp * c) * (self.inertia_y * dt);
                b.apply_torque_impulse(torque, true);
            }
        }
        w.step(dt);
        self.wall = w
            .narrow
            .contact_pairs_with(self.chassis)
            .any(|p| p.has_any_active_contact && (w.walls.contains(&p.collider1) || w.walls.contains(&p.collider2)));
    }
}

impl Model for Dynamic {
    fn name(&self) -> &'static str {
        self.name
    }
    fn frame(&mut self, w: &mut World, input: Input, frame_dt: f32) {
        self.acc += frame_dt;
        let mut wall = false;
        while self.acc >= FIXED - 1e-6 {
            self.step(w, input, FIXED);
            wall |= self.wall;
            self.acc -= FIXED;
        }
        self.wall = wall;
    }
    fn pose(&self, w: &World) -> Pose {
        let b = &w.bodies[self.body];
        let f = b.rotation() * -V3::z();
        let fwd = V3::new(f.x, 0.0, f.z).normalize();
        let vel = *b.linvel();
        Pose { pos: *b.translation() - V3::new(0.0, 0.7, 0.0), fwd, speed: vel.dot(&fwd), vel, wall: self.wall }
    }
    fn corner_speed(&self, k: f32) -> f32 {
        let arcade = arcade_corner_speed(&self.t, k);
        if self.assist.grip > 0.0 {
            return arcade;
        }
        let tyre = (0.75 * self.mu * self.g / k.abs().max(1e-4)).sqrt();
        arcade.min(tyre)
    }
    fn brake_decel(&self) -> f32 {
        self.t.brake_decel * 0.7
    }
}

// ---------------------------------------------------------------- autopilot

/// ai_driver.rhai's shape: steer at a point a lookahead ahead, plan the
/// fastest speed that still slows for every bend in sight. The centreline is
/// the line (no racing line, no pack, no drift).
struct Autopilot {
    curv: Vec<f32>,
    ds: f32,
    idx: usize,
    smoothed: f32,
}

impl Autopilot {
    fn new(track: &[Sample]) -> Self {
        let n = track.len();
        let mut ds = 0.0;
        for i in 0..n {
            ds += (track[(i + 1) % n].pos - track[i].pos).norm();
        }
        ds /= n as f32;
        let curv = (0..n)
            .map(|i| {
                let a = track[(i + n - 2) % n].fwd;
                let b = track[(i + 2) % n].fwd;
                let a = V3::new(a.x, 0.0, a.z).normalize();
                let b = V3::new(b.x, 0.0, b.z).normalize();
                a.dot(&b).clamp(-1.0, 1.0).acos() / (4.0 * ds)
            })
            .collect();
        Self { curv, ds, idx: 0, smoothed: 0.0 }
    }

    fn nearest(&mut self, track: &[Sample], p: V3) -> usize {
        let n = track.len();
        let mut best = (f32::MAX, self.idx);
        for o in 0..60 {
            let i = (self.idx + n + o - 20) % n;
            let d = (track[i].pos - p).norm_squared();
            if d < best.0 {
                best = (d, i);
            }
        }
        self.idx = best.1;
        self.idx
    }

    fn drive(&mut self, track: &[Sample], m: &dyn Model, pose: &Pose, dt: f32) -> Input {
        let n = track.len();
        let i = self.nearest(track, pose.pos);
        let speed = pose.speed;
        let look = (6.0 + speed.abs() * 0.42).clamp(7.0, 24.0);
        let target = track[(i + (look / self.ds).round() as usize) % n].pos;
        let to = V3::new(target.x - pose.pos.x, 0.0, target.z - pose.pos.z).normalize();
        let cross = pose.fwd.x * to.z - pose.fwd.z * to.x; // + = target to the right
        let err = cross.atan2(pose.fwd.dot(&to)).to_degrees();
        let ratio = (speed.abs() / 33.0).clamp(0.0, 1.0);
        let gain = 0.035 + ratio * 0.025;
        let raw = (err * gain).clamp(-1.0, 1.0);
        self.smoothed += (raw - self.smoothed) * (12.0 * dt).clamp(0.0, 1.0);
        let a_brake = m.brake_decel();
        let mut v_target = f32::MAX;
        let mut d = 0.0;
        let mut j = i;
        while d < 90.0 {
            let vc = m.corner_speed(self.curv[j % n]);
            v_target = v_target.min((vc * vc + 2.0 * a_brake * d).sqrt());
            d += self.ds;
            j += 1;
        }
        let mut inp = Input { throttle: 1.0, brake: 0.0, steer: self.smoothed, drift: false };
        if speed > v_target + 0.5 {
            inp.throttle = 0.0;
            inp.brake = ((speed - v_target) / 5.0).clamp(0.15, 1.0);
        } else if speed > v_target - 1.0 {
            inp.throttle = 0.6;
        }
        inp
    }
}

// ---------------------------------------------------------------- runs

struct Run {
    laps: Vec<f32>,
    top_speed: f32,
    wall_time: f32,
    wall_hits: u32,
    tight_radius: f32,
    tight_speed: f32,
    path: Vec<[f32; 3]>,
    finished: bool,
}

fn run_lap(make: &dyn Fn(&mut World) -> Box<dyn Model>, frame_dt: f32) -> Run {
    let track = sample_track(2.0);
    let mut w = World::new();
    w.build_track(&track);
    let mut m = make(&mut w);
    // Settle on the grid (the countdown), then GO
    for _ in 0..(1.0 / FIXED) as usize {
        m.frame(&mut w, Input::default(), FIXED);
    }
    let mut ap = Autopilot::new(&track);
    let n = track.len() as f32;
    let mut time = 0.0f32;
    let mut lap_start = 0.0;
    let mut prev_t = ap.nearest(&track, m.pose(&w).pos) as f32 / n;
    let mut passed = false;
    let mut run = Run { laps: vec![], top_speed: 0.0, wall_time: 0.0, wall_hits: 0, tight_radius: f32::MAX, tight_speed: 0.0, path: vec![], finished: false };
    let mut was_wall = false;
    let mut window: Vec<(f32, f32, f32, bool)> = vec![]; // (time, speed, heading, wall)
    let mut last_path = -1.0f32;
    while time < 200.0 && run.laps.len() < 3 {
        let pose = m.pose(&w);
        let input = ap.drive(&track, m.as_ref(), &pose, frame_dt);
        m.frame(&mut w, input, frame_dt);
        time += frame_dt;
        let pose = m.pose(&w);
        if pose.pos.y < -10.0 {
            break;
        }
        let hs = (pose.vel.x * pose.vel.x + pose.vel.z * pose.vel.z).sqrt();
        run.top_speed = run.top_speed.max(hs);
        if pose.wall {
            run.wall_time += frame_dt;
            if !was_wall {
                run.wall_hits += 1;
            }
        }
        was_wall = pose.wall;
        let heading = pose.fwd.z.atan2(pose.fwd.x);
        window.push((time, pose.speed, heading, pose.wall));
        window.retain(|s| time - s.0 <= 0.5 + 1e-4);
        if run.laps.len() >= 1 && window.len() > 2 && window.iter().all(|s| !s.3 && s.1 > 10.0) {
            let (a, b) = (window.first().unwrap(), window.last().unwrap());
            let mut dh = b.2 - a.2;
            while dh > PI { dh -= 2.0 * PI; }
            while dh < -PI { dh += 2.0 * PI; }
            let rate = dh.abs() / (b.0 - a.0);
            let v = window.iter().map(|s| s.1).sum::<f32>() / window.len() as f32;
            if rate > 1e-3 && v / rate < run.tight_radius {
                run.tight_radius = v / rate;
                run.tight_speed = v;
            }
        }
        if run.laps.len() == 1 && time - last_path >= 0.1 {
            run.path.push([pose.pos.x, pose.pos.z, hs]);
            last_path = time;
        }
        let t = ap.idx as f32 / n;
        if !passed && t > 0.3 && t < 0.7 {
            passed = true;
        }
        if passed && prev_t > 0.85 && t < 0.15 {
            run.laps.push(time - lap_start);
            lap_start = time;
        }
        prev_t = t;
    }
    run.finished = run.laps.len() == 3;
    run
}

/// Steer from a rolling start, the throttle holding the speed: the circle
/// the kart settles on, and how fast its yaw rate answered the wheel.
struct Pad {
    /// Of the path (the velocity's heading), m.
    radius: f32,
    lat_accel: f32,
    /// Nose right of the velocity, degrees.
    slip: f32,
    speed: f32,
    /// To 63% of the settled yaw rate, ms.
    rise_ms: f32,
}

fn skidpad(make: &dyn Fn(&mut World) -> Box<dyn Model>, v0: f32, steer: f32, drift: bool) -> Pad {
    let mut w = World::new();
    w.build_plane();
    let mut m = make(&mut w);
    for _ in 0..60 {
        m.frame(&mut w, Input::default(), FIXED);
    }
    let mut time = 0.0;
    let mut late = vec![];
    let mut rates = vec![];
    let mut prev_h: Option<f32> = None;
    while time < 12.0 {
        let p = m.pose(&w);
        let on = time > 5.0;
        let mut inp = Input { throttle: 0.0, brake: 0.0, steer: if on { steer } else { 0.0 }, drift: on && drift };
        if p.speed < v0 - 0.3 {
            inp.throttle = 1.0;
        } else if p.speed < v0 {
            inp.throttle = 0.5;
        }
        m.frame(&mut w, inp, FIXED);
        time += FIXED;
        let p = m.pose(&w);
        let h = p.fwd.z.atan2(p.fwd.x);
        if let Some(ph) = prev_h {
            let mut d = h - ph;
            while d > PI { d -= 2.0 * PI; }
            while d < -PI { d += 2.0 * PI; }
            if time > 5.0 { rates.push((time, d / FIXED)); }
        }
        prev_h = Some(h);
        if time > 9.0 {
            late.push((time, p.vel, p.fwd));
        }
    }
    let settled: Vec<f32> = rates.iter().filter(|r| r.0 > 9.0).map(|r| r.1).collect();
    let settled = settled.iter().sum::<f32>() / settled.len() as f32;
    let rise = rates.iter().find(|r| (r.1 / settled) >= 0.632).map(|r| (r.0 - 5.0) * 1000.0).unwrap_or(-1.0);
    let mut dh = 0.0;
    for k in 1..late.len() {
        let (a, b) = (late[k - 1].1, late[k].1);
        let mut d = b.z.atan2(b.x) - a.z.atan2(a.x);
        while d > PI { d -= 2.0 * PI; }
        while d < -PI { d += 2.0 * PI; }
        dh += d;
    }
    let span = late.last().unwrap().0 - late[0].0;
    let rate = dh.abs() / span;
    let speed = late.iter().map(|s| (s.1.x * s.1.x + s.1.z * s.1.z).sqrt()).sum::<f32>() / late.len() as f32;
    let slip = late.iter().map(|s| {
        let vh = V3::new(s.1.x, 0.0, s.1.z).normalize();
        (vh.x * s.2.z - vh.z * s.2.x).atan2(vh.dot(&s.2)).to_degrees()
    }).sum::<f32>() / late.len() as f32;
    Pad { radius: speed / rate.max(1e-4), lat_accel: speed * rate, slip, speed, rise_ms: rise }
}

/// At 25 m/s, wheel straight, into a wall `angle` degrees off running
/// alongside it: the speed and heading 1.5 s after first touching it.
/// (exit speed m/s, heading change deg, touched)
fn wall_test(make: &dyn Fn(&mut World) -> Box<dyn Model>, angle: f32) -> (f32, f32, bool) {
    let mut w = World::new();
    w.build_plane();
    // The kart runs down -Z from z = 5; the wall's line crosses x = 1.5 at
    // z = -150, so it has 155 m to reach 25 m/s
    let rot = UnitQuaternion::from_axis_angle(&V3::y_axis(), angle.to_radians());
    let wall = ColliderBuilder::cuboid(0.25, 0.75, 400.0)
        .position(Isometry::from_parts(Translation::new(1.5, 0.75, -150.0), rot))
        .friction(0.3)
        .restitution(0.5)
        .build();
    let h = w.colliders.insert(wall);
    w.walls.push(h);
    w.queries.update(&w.colliders);
    let mut m = make(&mut w);
    for _ in 0..60 {
        m.frame(&mut w, Input::default(), FIXED);
    }
    let h0 = { let p = m.pose(&w); p.fwd.z.atan2(p.fwd.x) };
    let mut time = 0.0;
    let mut touched_at: Option<f32> = None;
    while time < 12.0 {
        let p = m.pose(&w);
        let mut inp = Input::default();
        if touched_at.is_none() && p.speed < 24.7 {
            inp.throttle = 1.0;
        } else if touched_at.is_none() && p.speed < 25.0 {
            inp.throttle = 0.5;
        }
        m.frame(&mut w, inp, FIXED);
        time += FIXED;
        let p = m.pose(&w);
        if touched_at.is_none() && p.wall {
            touched_at = Some(time);
        }
        if let Some(t0) = touched_at {
            if time - t0 >= 1.5 {
                let hs = (p.vel.x * p.vel.x + p.vel.z * p.vel.z).sqrt();
                let mut d = p.fwd.z.atan2(p.fwd.x) - h0;
                while d > PI { d -= 2.0 * PI; }
                while d < -PI { d += 2.0 * PI; }
                return (hs, d.to_degrees().abs(), true);
            }
        }
    }
    (0.0, 0.0, false)
}

fn main() {
    let spawn = V3::new(1.5, 0.0, 5.0); // ai1's grid slot
    let models: Vec<(&str, Box<dyn Fn(&mut World) -> Box<dyn Model>>)> = vec![
        ("arcade_frame", Box::new(move |_| Box::new(Arcade::new(AI1, false, spawn)) as Box<dyn Model>)),
        ("arcade_fixed", Box::new(move |_| Box::new(Arcade::new(AI1, true, spawn)) as Box<dyn Model>)),
        ("rapier_raw", Box::new(move |w| Box::new(Dynamic::new(w, "rapier_raw", AI1, Assist { yaw: 0.0, grip: 0.0, upright: 0.0 }, spawn)) as Box<dyn Model>)),
        ("rapier_assist", Box::new(move |w| Box::new(Dynamic::new(w, "rapier_assist", AI1, Assist { yaw: 10.0, grip: 9.0, upright: 60.0 }, spawn)) as Box<dyn Model>)),
    ];
    let mut json = String::from("[\n");
    for (name, make) in &models {
        for (label, dt) in [("1/30", 1.0 / 30.0f32), ("1/60", 1.0 / 60.0), ("1/144", 1.0 / 144.0)] {
            let clock = std::time::Instant::now();
            let r = run_lap(make.as_ref(), dt);
            // Wall time per simulated 1/60 s, autopilot and track build included
            let sim = r.laps.iter().sum::<f32>() + 1.0;
            let us = clock.elapsed().as_secs_f64() * 1e6 / (sim as f64 * 60.0);
            let line = format!(
                "{{\"model\":\"{name}\",\"frame\":\"{label}\",\"finished\":{},\"laps\":{:?},\"top_speed\":{:.2},\"wall_time\":{:.2},\"wall_hits\":{},\"tight_radius\":{:.2},\"tight_speed\":{:.2},\"us_per_step\":{:.1}}}",
                r.finished, r.laps.iter().map(|l| (l * 1000.0).round() / 1000.0).collect::<Vec<_>>(), r.top_speed, r.wall_time, r.wall_hits, r.tight_radius, r.tight_speed, us
            );
            println!("{line}");
            let path: Vec<String> = r.path.iter().map(|p| format!("[{:.2},{:.2},{:.1}]", p[0], p[1], p[2])).collect();
            json.push_str(&format!("{},\n", line.trim_end_matches('}').to_string() + &format!(",\"path\":[{}]}}", path.join(","))));
        }
        for (v0, steer, drift, label) in [(10.0f32, 1.0f32, false, "lock10"), (20.0, 1.0, false, "lock20"), (30.0, 1.0, false, "lock30"), (20.0, 0.5, false, "half20"), (20.0, 0.6, true, "drift20")] {
            let p = skidpad(make.as_ref(), v0, steer, drift);
            let line = format!("{{\"model\":\"{name}\",\"pad\":\"{label}\",\"radius\":{:.2},\"lat_accel\":{:.2},\"slip_deg\":{:.1},\"speed\":{:.2},\"rise_ms\":{:.0}}}", p.radius, p.lat_accel, p.slip, p.speed, p.rise_ms);
            println!("{line}");
            json.push_str(&line);
            json.push_str(",\n");
        }
        for angle in [10.0f32, 30.0] {
            let (v, dh, touched) = wall_test(make.as_ref(), angle);
            let line = format!("{{\"model\":\"{name}\",\"wall\":{angle},\"touched\":{touched},\"exit_speed\":{v:.2},\"heading_change\":{dh:.1}}}");
            println!("{line}");
            json.push_str(&line);
            json.push_str(",\n");
        }
    }
    let track = sample_track(2.0);
    let outline: Vec<String> = track.iter().map(|s| format!("[{:.2},{:.2}]", s.pos.x, s.pos.z)).collect();
    json.push_str(&format!("{{\"track\":[{}]}}\n]\n", outline.join(",")));
    std::fs::write("runs.json", json).unwrap();
}
