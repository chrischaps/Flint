//! The vehicle on a plane, stepped as the player steps it: physics'
//! `fixed_update_in` over the run's resources it is in (#1616), the
//! subsystem's `fixed_update` inside it, and the
//! frame's stages around the steps.

use super::model::Vehicle;
use super::Vehicles;
use crate::{VEHICLE_INPUT, VEHICLE_STATE, VEHICLE_WHEEL};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::{FixedCtx, Resources, Subsystem};
use flint_physics::PhysicsSystem;
use flint_runtime::{EventBus, Eye, InputState, PlayerView, StateConfig};

const STEP: f64 = 1.0 / 60.0;

fn table(text: &str) -> toml::Value {
    toml::Value::Table(text.parse::<toml::Table>().expect("test TOML"))
}

fn put(world: &mut FlintWorld, id: EntityId, component: &str, text: &str) {
    world
        .set_component(id, component, table(text))
        .expect("component set");
}

/// A 1 km ground slab, its top at y = 0.
fn ground(world: &mut FlintWorld) -> EntityId {
    let g = world.spawn("ground").unwrap();
    put(world, g, "transform", "position = [0.0, -0.5, 0.0]");
    put(world, g, "rigidbody", "body_type = \"static\"");
    put(
        world,
        g,
        "collider",
        "shape = \"box\"\nsize = [1000.0, 1.0, 1000.0]\nfriction = 0.8",
    );
    g
}

/// The prototype's kart (ADR 0140): a 1.2 x 0.5 x 2 m chassis 0.7 m up,
/// front wheels steering, back wheels driving, driven by a script.
fn kart(world: &mut FlintWorld, name: &str, at: [f32; 3], extra: &str) -> EntityId {
    let k = world.spawn(name).unwrap();
    put(
        world,
        k,
        "transform",
        &format!("position = [{}, {}, {}]", at[0], at[1] + 0.7, at[2]),
    );
    put(
        world,
        k,
        "rigidbody",
        "body_type = \"dynamic\"\ngravity_scale = 2.04",
    );
    put(
        world,
        k,
        "collider",
        "shape = \"box\"\nsize = [1.2, 0.5, 2.0]\nfriction = 0.3",
    );
    put(
        world,
        k,
        "vehicle",
        &format!("driver = \"script\"\n{extra}"),
    );
    put(world, k, VEHICLE_INPUT, "");
    for (i, (x, z, front)) in [
        (-0.55, -0.75, true),
        (0.55, -0.75, true),
        (-0.55, 0.7, false),
        (0.55, 0.7, false),
    ]
    .into_iter()
    .enumerate()
    {
        let w = world.spawn(format!("{name}_wheel_{i}")).unwrap();
        put(
            world,
            w,
            "transform",
            &format!("position = [{x}, 0.0, {z}]"),
        );
        put(
            world,
            w,
            VEHICLE_WHEEL,
            &format!(
                "steer = {front}\ndrive = {}\nhandbrake = {}",
                !front, !front
            ),
        );
        world.set_parent(w, k).unwrap();
    }
    k
}

struct Run {
    world: FlintWorld,
    events: EventBus,
    input: InputState,
    policy: StateConfig,
    resources: Resources,
    vehicles: Vehicles,
}

impl Run {
    fn new(world: FlintWorld) -> Self {
        let vehicles = Vehicles::new(&world);
        Self {
            world,
            events: EventBus::new(),
            input: InputState::new(),
            policy: StateConfig::playing(),
            resources: {
                // Physics is the run's resource, as the player keeps it
                let mut resources = Resources::new();
                resources.insert(PhysicsSystem::new());
                resources
            },
            vehicles,
        }
    }

    /// One frame of `steps` fixed steps: state at `PreScripts`, the steps,
    /// then the wheel poses and neutral modifiers at `Physics`.
    fn frame(&mut self, steps: u32) {
        self.vehicles.write_state(&mut self.world);
        self.vehicles
            .write_player_input(&mut self.world, &self.input);
        flint_physics::of_mut(&mut self.resources)
            .expect("physics")
            .begin_frame_steps(steps as usize);
        for i in 0..steps {
            let Self {
                world,
                events,
                input,
                policy,
                resources,
                vehicles,
            } = self;
            PhysicsSystem::fixed_update_in(
                resources,
                world,
                STEP,
                events,
                |resources, world, events| {
                    vehicles.fixed_update(&mut FixedCtx {
                        world,
                        dt: STEP,
                        step: (i, steps),
                        input,
                        events,
                        policy,
                        resources,
                    });
                },
            )
            .unwrap();
        }
        // `Stage::View`: the kernel forgets last frame's eye, then the
        // chase camera places this one
        self.resources
            .get_or_insert_with(PlayerView::default)
            .new_frame();
        self.vehicles.place_eye(
            &mut self.world,
            &self.input,
            STEP as f32,
            &mut self.resources,
        );
        self.vehicles.after_steps(&mut self.world);
    }

    fn eye(&self) -> Option<Eye> {
        self.resources.get::<PlayerView>().and_then(|v| v.eye)
    }

    fn seconds(&mut self, seconds: f64) {
        for _ in 0..(seconds / STEP).round() as u32 {
            self.frame(1);
        }
    }

    fn set_input(&mut self, id: EntityId, field: &str, value: toml::Value) {
        self.world
            .set_field(id, VEHICLE_INPUT, field, value)
            .unwrap();
    }

    fn state(&self, id: EntityId) -> flint_ecs::ComponentView<'_> {
        self.world.view(id, VEHICLE_STATE)
    }

    fn position(&self, id: EntityId) -> [f32; 3] {
        let t = self.world.get_transform(id).unwrap().position;
        [t.x, t.y, t.z]
    }

    fn vehicle(&self, id: EntityId) -> &Vehicle {
        self.vehicles
            .vehicles()
            .iter()
            .find(|v| v.entity == id)
            .unwrap()
    }
}

fn f(v: f64) -> toml::Value {
    toml::Value::Float(v)
}

#[test]
fn a_vehicle_settles_on_its_own_wheels_not_inside_its_chassis() {
    // Each wheel's ray starts inside the chassis box: without the body left
    // out of the cast, every wheel would hit its own collider at once
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(2.0);
    let v = run.vehicle(k);
    assert_eq!(v.wheel_seen.len(), 4);
    for (i, w) in v.wheel_seen.iter().enumerate() {
        assert!(w.contact, "wheel {i} on the ground");
        assert!(
            w.compression > 0.05 && w.compression < 0.95,
            "wheel {i}'s spring is between its stops: {}",
            w.compression
        );
        assert_eq!(
            w.ground,
            run.world.get_id("ground"),
            "wheel {i} stands on the ground"
        );
    }
    let y = run.position(k)[1];
    // Hung 0.35 + 0.275 m of ray under the body's origin, the spring pressed
    assert!(y > 0.3 && y < 0.65, "rides on its springs: y = {y}");
    assert!(run.state(k).bool("grounded", false));
    assert_eq!(run.state(k).i64("wheels_grounded", 0), 4);
    assert!(
        !run.state(k).bool("contact", true),
        "the ground under the wheels is no contact"
    );
    assert!(run.state(k).f32("speed", 9.0).abs() < 0.2, "and stays put");
}

#[test]
fn full_throttle_reaches_the_speed_cap_and_steering_right_turns_right() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(
        &mut world,
        "kart",
        [0.0, 0.0, 0.0],
        "top_speed = 20.0\naccel = 16.5",
    );
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(6.0);
    let speed = run.state(k).f32("speed", 0.0);
    assert!(
        (speed - 20.0).abs() < 1.0,
        "held near the 20 m/s cap: {speed}"
    );
    let z = run.position(k)[2];
    assert!(z < -60.0, "drove forward along -Z: z = {z}");
    let yaw0 = run.state(k).f32("yaw", 0.0);
    assert!(yaw0.abs() < 3.0, "straight: yaw {yaw0}");
    run.set_input(k, "steer", f(1.0));
    run.seconds(0.5);
    let yaw_rate = run.state(k).f32("yaw_rate", 0.0);
    assert!(
        yaw_rate < -20.0,
        "steer right is a right turn (negative yaw rate): {yaw_rate}"
    );
    assert!(run.position(k)[0] > 0.5, "and moves right (+X)");
    let target = run.state(k).f32("yaw_target", 0.0);
    assert!(
        target < -20.0 && (target - yaw_rate).abs() < 15.0,
        "the assist pulls toward a right turn the body follows: target {target}, rate {yaw_rate}"
    );
}

#[test]
fn a_brake_at_a_standstill_reverses_and_hold_stops_everything() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "brake", f(1.0));
    run.seconds(3.0);
    let speed = run.state(k).f32("speed", 0.0);
    assert!(speed < -3.0, "reversing: {speed}");
    assert!(
        speed > -9.5,
        "at most the 0.3 reverse fraction of 30 m/s: {speed}"
    );
    run.set_input(k, "brake", f(0.0));
    run.set_input(k, "throttle", f(1.0));
    run.set_input(k, "hold", toml::Value::Boolean(true));
    run.seconds(3.0);
    let speed = run.state(k).f32("speed", 9.0);
    assert!(
        speed.abs() < 0.5,
        "hold brakes and ignores the throttle: {speed}"
    );
}

#[test]
fn every_assist_at_zero_is_still_a_car() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(
        &mut world,
        "raw",
        [0.0, 0.0, 0.0],
        "grip = 0.0\nyaw_assist = 0.0\nupright = 0.0",
    );
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(3.0);
    assert!(
        run.state(k).f32("speed", 0.0) > 15.0,
        "the wheels alone drive it"
    );
    run.set_input(k, "steer", f(1.0));
    run.seconds(1.0);
    assert!(
        run.state(k).f32("yaw_rate", 0.0) < -10.0,
        "and its front wheels steer it"
    );
}

#[test]
fn push_is_applied_once_and_modifiers_return_to_neutral() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(1.0);
    run.set_input(k, "push", toml::Value::Array(vec![f(0.0), f(6.0), f(0.0)]));
    run.set_input(k, "speed_bonus", f(5.0));
    run.frame(1);
    let v = run.world.view(k, VEHICLE_INPUT);
    assert_eq!(
        v.vec3("push", [9.0; 3]),
        [0.0; 3],
        "the push is zeroed once applied"
    );
    assert_eq!(
        v.f32("speed_bonus", 9.0),
        0.0,
        "a modifier is neutral after the frame's steps"
    );
    run.seconds(0.2);
    assert!(run.position(k)[1] > 0.9, "a 6 m/s hop left the ground");
    assert!(!run.state(k).bool("grounded", true), "and is airborne");
    assert!(run.state(k).f32("airborne_time", 0.0) > 0.1);
    run.seconds(2.0);
    assert!(run.state(k).bool("grounded", false), "it lands");
}

/// A hop at speed with the wheel turned right, `air_control` off and on:
/// only with it does the kart turn while its wheels are off the ground,
/// and its travel turns with its nose. The heading's turn, the travel's
/// slip off the nose at the end (degrees), and whether it's still up.
fn heading_turned_in_a_hop(air_control: f64) -> (f32, f32, bool) {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(
        &mut world,
        "kart",
        [0.0, 0.0, 0.0],
        &format!(
            "top_speed = 20.0
accel = 16.5
air_control = {air_control}"
        ),
    );
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(3.0);
    let before = run.state(k).f32("yaw", 0.0);
    run.set_input(k, "push", toml::Value::Array(vec![f(0.0), f(6.0), f(0.0)]));
    run.frame(1);
    run.set_input(k, "steer", f(1.0));
    run.seconds(0.3);
    let airborne = !run.state(k).bool("grounded", true);
    let turned = run.state(k).f32("yaw", 0.0) - before;
    (turned, run.state(k).f32("slip_angle", 0.0), airborne)
}

#[test]
fn air_control_steers_a_hop_and_off_leaves_it_to_its_spin() {
    let (off, _, airborne_off) = heading_turned_in_a_hop(0.0);
    let (on, slip, airborne_on) = heading_turned_in_a_hop(1.0);
    assert!(airborne_off && airborne_on, "both are still in the air");
    assert!(
        slip.abs() < 1.5,
        "the travel turned with the nose, so it lands going where it points: slip {slip} degrees"
    );
    assert!(
        off.abs() < 1.0,
        "with air_control 0 nothing steers off the ground: {off} degrees"
    );
    assert!(
        on < -3.0,
        "air_control 1 turns the hop right (negative yaw): {on} degrees, off {off}"
    );
}

/// The speed over the ground a hop at 20 m/s loses in 0.4 s of flight, at
/// `air_drag`.
fn speed_lost_in_a_hop(air_drag: f64) -> f32 {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(
        &mut world,
        "kart",
        [0.0, 0.0, 0.0],
        &format!(
            "top_speed = 20.0
accel = 16.5
air_drag = {air_drag}"
        ),
    );
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(4.0);
    run.set_input(k, "push", toml::Value::Array(vec![f(0.0), f(6.0), f(0.0)]));
    run.frame(1);
    run.seconds(0.1);
    // Over the ground: the body pitches in flight, so not along its nose
    let flat = |run: &Run| {
        let [x, _, z] = run.state(k).vec3("velocity", [0.0; 3]);
        x.hypot(z)
    };
    let before = flat(&run);
    run.seconds(0.4);
    assert!(!run.state(k).bool("grounded", true), "still in the air");
    before - flat(&run)
}

#[test]
fn air_drag_zero_keeps_a_hops_speed_and_the_default_drags_it() {
    let arcade = speed_lost_in_a_hop(0.0);
    let car = speed_lost_in_a_hop(1.0);
    assert!(
        arcade.abs() < 0.3,
        "air_drag 0: no drag off the ground, lost {arcade} m/s"
    );
    assert!(
        car > 2.0,
        "air_drag 1 (the default) drags in the air too: lost {car} m/s"
    );
}

#[test]
fn vehicle_place_moves_it_there_at_rest() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(2.0);
    run.set_input(k, "throttle", f(0.0));
    assert!(run.vehicles.place(k, [10.0, 0.7, 5.0], 90.0));
    run.seconds(1.0);
    let [x, _, z] = run.position(k);
    assert!(
        (x - 10.0).abs() < 0.3 && (z - 5.0).abs() < 0.3,
        "placed: {x}, {z}"
    );
    let yaw = run.state(k).f32("yaw", 0.0);
    assert!((yaw - 90.0).abs() < 2.0, "facing yaw 90 (-X): {yaw}");
    assert!(run.state(k).f32("speed", 9.0).abs() < 0.5, "at rest");
    assert!(!run
        .vehicles
        .place(run.world.get_id("ground").unwrap(), [0.0; 3], 0.0));
}

#[test]
fn a_wall_is_a_contact_with_its_normal_entity_and_impact() {
    wall_contact_with_chassis(None);
}

/// A round chassis (FlintKart's puck) meets walls as a box does
#[test]
fn a_cylinder_chassis_sees_the_wall_too() {
    wall_contact_with_chassis(Some(
        "shape = \"cylinder\"\nsize = [2.0, 0.4, 2.0]\nfriction = 0.3",
    ));
}

/// A swept road's wall is a host trimesh (a spline's): a round chassis
/// coasting square into it bounces off within one step, and the contact is
/// still seen (the reach grows with the step's travel)
#[test]
fn a_cylinder_chassis_sees_a_trimesh_wall_it_bounces_off() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let wall = world.spawn("wall").unwrap();
    put(&mut world, wall, "transform", "position = [0.0, 0.0, 0.0]");
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    put(
        &mut world,
        k,
        "collider",
        "shape = \"cylinder\"\nsize = [2.0, 0.4, 2.0]\nfriction = 0.3\nrestitution = 0.5",
    );
    let mut run = Run::new(world);
    // A 20 m face across the road at z = -30, facing the kart (+Z)
    flint_physics::of_mut(&mut run.resources)
        .expect("physics")
        .register_static_trimesh(
            wall,
            vec![
                [-10.0, 0.0, -30.0],
                [10.0, 0.0, -30.0],
                [10.0, 2.0, -30.0],
                [-10.0, 2.0, -30.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
            0.3,
            0.5,
        );
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    run.seconds(2.0);
    // Off the throttle: it meets the wall coasting and nothing presses it on
    run.set_input(k, "throttle", f(0.0));
    let mut hit = None;
    for _ in 0..300 {
        run.frame(1);
        if run.state(k).bool("contact", false) {
            hit = Some(run.state(k).f32("impact", 0.0));
            break;
        }
    }
    let impact = hit.expect("drove into the trimesh wall and saw it");
    assert!(impact > 5.0, "hit at speed: {impact} m/s");
    assert_eq!(run.state(k).i64("contact_entity", -1), wall.raw() as i64);
}

fn wall_contact_with_chassis(collider: Option<&str>) {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let wall = world.spawn("wall").unwrap();
    put(
        &mut world,
        wall,
        "transform",
        "position = [0.0, 1.0, -20.0]",
    );
    put(&mut world, wall, "rigidbody", "body_type = \"static\"");
    put(
        &mut world,
        wall,
        "collider",
        "shape = \"box\"\nsize = [20.0, 2.0, 1.0]",
    );
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    if let Some(c) = collider {
        put(&mut world, k, "collider", c);
    }
    let mut run = Run::new(world);
    run.seconds(0.5);
    run.set_input(k, "throttle", f(1.0));
    let mut hit = None;
    for _ in 0..300 {
        run.frame(1);
        if run.state(k).bool("contact", false) {
            hit = Some(run.state(k).f32("impact", 0.0));
            break;
        }
    }
    let impact = hit.expect("drove into the wall");
    assert!(impact > 5.0, "hit at speed: {impact} m/s");
    let s = run.state(k);
    assert_eq!(s.i64("contact_entity", -1), wall.raw() as i64);
    let n = s.vec3("contact_normal", [0.0; 3]);
    assert!(
        n[2] > 0.9,
        "the wall's normal points back at the kart (+Z): {n:?}"
    );
}

#[test]
fn a_vehicle_spawned_mid_run_is_driven_from_its_first_step() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    kart(&mut world, "first", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(0.5);
    let second = kart(&mut run.world, "second", [5.0, 0.0, 0.0], "");
    run.set_input(second, "throttle", f(1.0));
    run.frame(1);
    assert_eq!(
        run.vehicles.vehicles().len(),
        2,
        "picked up at the next step"
    );
    assert!(
        run.vehicle(second).has_controller(),
        "its body was made by that step's sync-in"
    );
    run.seconds(2.0);
    let v = run.vehicle(second);
    assert!(
        v.wheel_seen.iter().all(|w| w.contact),
        "on its wheels, not on itself"
    );
    assert!(run.state(second).f32("speed", 0.0) > 5.0, "and driving");
}

#[test]
fn the_wheel_children_are_posed_on_their_springs() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    let mut run = Run::new(world);
    run.seconds(1.0);
    run.set_input(k, "steer", f(1.0));
    run.seconds(0.5);
    let front = run.world.get_id("kart_wheel_0").unwrap();
    let t = run.world.get_transform(front).unwrap();
    assert!((t.position.x + 0.55).abs() < 1e-4 && (t.position.z + 0.75).abs() < 1e-4);
    assert!(
        t.position.y < -0.1 && t.position.y > -0.7,
        "hung on its spring below the attach point: {}",
        t.position.y
    );
    let q = t.rotation_quat.expect("posed by quaternion");
    assert!(
        q[1] < -0.05,
        "the front wheel is steered right (about -Y): {q:?}"
    );
    let back = run.world.get_id("kart_wheel_2").unwrap();
    let qb = run
        .world
        .get_transform(back)
        .unwrap()
        .rotation_quat
        .unwrap();
    assert!(qb[1].abs() < 1e-4, "a back wheel doesn't steer: {qb:?}");
}

#[test]
fn a_player_vehicle_reads_its_actions() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = kart(&mut world, "kart", [0.0, 0.0, 0.0], "");
    world
        .set_field(
            k,
            crate::VEHICLE,
            "driver",
            toml::Value::String("player".into()),
        )
        .unwrap();
    world
        .set_field(k, crate::VEHICLE, "actions", table("throttle = \"go\""))
        .unwrap();
    let mut run = Run::new(world);
    let w = flint_runtime::parse_key_code("KeyW").unwrap();
    run.input.bind_action("go", vec![w]);
    run.input.process_key_down(w);
    run.seconds(2.0);
    assert_eq!(run.world.view(k, VEHICLE_INPUT).f32("throttle", 0.0), 1.0);
    assert!(
        run.state(k).f32("speed", 0.0) > 5.0,
        "the player's key drives it"
    );
}

// --- The chase camera (#1631) ---------------------------------------------

/// A kart with a chase camera, driven by its script input.
fn chased_kart(world: &mut FlintWorld, camera: &str) -> EntityId {
    let k = kart(world, "kart", [0.0, 0.0, 0.0], "top_speed = 24.0");
    put(world, k, crate::CHASE_CAMERA, camera);
    k
}

fn table_vec(v: [f64; 3]) -> toml::Value {
    toml::Value::Array(v.iter().map(|c| f(*c)).collect())
}

#[test]
fn a_chase_camera_sits_behind_and_above_and_looks_ahead() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = chased_kart(&mut world, "");
    let mut run = Run::new(world);
    run.seconds(1.0);
    let eye = run.eye().expect("the chase camera placed the eye");
    let [x, y, z] = run.position(k);
    // The kart faces -Z, so behind it is +Z: FlintKart's 8 m back, 4 m up
    assert!((eye.position.x - x).abs() < 0.01, "{eye:?}");
    assert!(
        (eye.position.z - (z + 8.0)).abs() < 0.05,
        "{eye:?} kart {z}"
    );
    assert!(
        (eye.position.y - (y + 4.0)).abs() < 0.05,
        "{eye:?} kart {y}"
    );
    assert!(
        (eye.target.z - (z - 4.0)).abs() < 0.05,
        "4 m ahead: {eye:?}"
    );
    assert!(
        (eye.fov.unwrap() - 62.0).abs() < 0.01,
        "at rest, 62 degrees"
    );
    let view = run.resources.get::<PlayerView>().unwrap();
    assert_eq!(view.player, Some(k), "the game is played as the kart");
}

/// The camera moves once a step from the pose the step left, so a drive
/// seen at 30, 60 and 144 frames a second ends on one eye, to the bit, and
/// at 144 a frame with no step leaves it where it was.
#[test]
fn a_chase_camera_frames_one_drive_alike_at_30_60_and_144_hz() {
    fn drive(frame_rate: u32) -> (Eye, u64) {
        let mut world = FlintWorld::new();
        ground(&mut world);
        let k = chased_kart(&mut world, "");
        let mut run = Run::new(world);
        run.set_input(k, "throttle", f(1.0));
        run.set_input(k, "steer", f(0.4));
        let mut done = 0u32;
        let mut frame = 0u32;
        while done < 180 {
            frame += 1;
            let due = frame * 60 / frame_rate;
            let steps = due - done;
            let before = run.eye();
            run.frame(steps);
            if steps == 0 && before.is_some() {
                assert_eq!(run.eye(), before, "a frame with no step moves nothing");
            }
            done = due;
        }
        (run.eye().unwrap(), run.vehicles.chase().unwrap().steps)
    }
    let (at30, steps30) = drive(30);
    let (at60, steps60) = drive(60);
    let (at144, steps144) = drive(144);
    assert_eq!((steps30, steps60, steps144), (180, 180, 180));
    assert_eq!(at30, at60);
    assert_eq!(at60, at144);
    // It drove: the camera is well away from the start, and wider with the
    // speed
    assert!(at60.position.z < -10.0, "{at60:?}");
    assert!(at60.fov.unwrap() > 66.0, "{at60:?}");
}

#[test]
fn a_respawn_cuts_and_release_hands_the_shot_back() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    let k = chased_kart(&mut world, "");
    let mut run = Run::new(world);
    run.seconds(0.5);
    let cuts = run.vehicles.chase().unwrap().cuts;
    assert!(run.vehicles.place(k, [40.0, 0.7, 0.0], 0.0));
    run.seconds(0.1);
    assert_eq!(run.vehicles.chase().unwrap().cuts, cuts + 1, "one cut");
    let eye = run.eye().unwrap();
    assert!(
        (eye.position.x - 40.0).abs() < 0.1,
        "cut, not flown: {eye:?}"
    );

    // A finish camera: the script has the shot, and the modifiers last a frame
    run.world
        .set_field(
            k,
            crate::CHASE_CAMERA,
            "release",
            toml::Value::Boolean(true),
        )
        .unwrap();
    run.world
        .set_field(k, crate::CHASE_CAMERA, "fov_kick", f(9.0))
        .unwrap();
    run.frame(1);
    assert_eq!(run.eye(), None, "released: no eye");
    let cam = run.world.view(k, crate::CHASE_CAMERA);
    assert!(!cam.bool("release", true) && cam.f32("fov_kick", 1.0) == 0.0);
    run.frame(1);
    assert!(run.eye().is_some(), "and back the next frame");
}

#[test]
fn the_modifiers_shape_the_shot_for_a_frame() {
    let mut world = FlintWorld::new();
    ground(&mut world);
    // No lag: the shot is where it wants to be, so the frames compare
    let k = chased_kart(&mut world, "lag = 1000.0");
    let mut run = Run::new(world);
    run.seconds(1.0);
    let plain = run.eye().unwrap();
    run.world
        .set_field(k, crate::CHASE_CAMERA, "shake", table_vec([0.5, 0.25, 0.1]))
        .unwrap();
    run.world
        .set_field(k, crate::CHASE_CAMERA, "fov_kick", f(10.0))
        .unwrap();
    run.frame(0);
    let shaken = run.eye().unwrap();
    // Looking down -Z, the camera's right is +X
    assert!(
        (shaken.position.x - plain.position.x - 0.5).abs() < 1e-4,
        "{shaken:?}"
    );
    assert!((shaken.position.y - plain.position.y - 0.25).abs() < 1e-4);
    assert!((shaken.roll - plain.roll - 0.1).abs() < 1e-6);
    assert!((shaken.fov.unwrap() - plain.fov.unwrap() - 10.0).abs() < 1e-4);
    run.frame(0);
    assert_eq!(run.eye(), Some(plain), "neutral again: the plain shot");
}
