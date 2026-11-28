//! The controller over a physics world, stepped by hand (moved from
//! flint-physics with the controller, #1618).

use super::*;
use flint_runtime::EventBus;

pub(crate) fn spawn_from_toml(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

/// One fixed step of physics, as the player's Integrate does after the walk.
pub(crate) fn step(sys: &mut PhysicsSystem, world: &mut FlintWorld, dt: f64) {
    sys.fixed_update(world, dt, &mut EventBus::new()).unwrap();
}

/// A walker over `sys`: its controller, entered.
struct Walker {
    sys: PhysicsSystem,
    cc: CharacterController,
}

impl Walker {
    fn new(world: &mut FlintWorld) -> Self {
        let mut sys = PhysicsSystem::new();
        sys.initialize(world).unwrap();
        let mut cc = CharacterController::new();
        cc.enter(world, &mut sys);
        Self { sys, cc }
    }

    /// The frame's input, then a step of walking and of physics.
    fn frame(&mut self, input: &InputState, world: &mut FlintWorld, dt: f64) {
        self.cc.frame_input(input, world);
        self.cc.update(input, world, &mut self.sys, dt);
        step(&mut self.sys, world, dt);
    }
}

/// Metres walked forward in one second by a character whose
/// `character_controller` is `settings`.
fn metres_walked(settings: &str) -> (f32, usize) {
    let mut world = FlintWorld::new();
    spawn_from_toml(
        &mut world,
        "floor",
        "[transform]\nposition = [0, -0.5, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [200, 1, 200]",
    );
    let id = spawn_from_toml(
        &mut world,
        "player",
        &format!(
            "[transform]\nposition = [0, 1, 0]\n[rigidbody]\nbody_type = \"kinematic\"\n\
             [collider]\nshape = \"capsule\"\nsize = [0.6, 1.8, 0.6]\n\
             [character_controller]\n{settings}"
        ),
    );
    let mut w = Walker::new(&mut world);
    let mut input = InputState::new();
    let dt = 1.0 / 60.0;
    // Settle on the floor, then hold forward for a second
    for _ in 0..30 {
        w.cc.update(&input, &mut world, &mut w.sys, dt);
        step(&mut w.sys, &mut world, dt);
    }
    let start = world.get_transform(id).unwrap().position;
    input.process_key_down(flint_runtime::parse_key_code("KeyW").unwrap());
    for _ in 0..60 {
        w.cc.update(&input, &mut world, &mut w.sys, dt);
        step(&mut w.sys, &mut world, dt);
    }
    let end = world.get_transform(id).unwrap().position;
    let walked = ((end.x - start.x).powi(2) + (end.z - start.z).powi(2)).sqrt();
    (walked, world.read_reports().len())
}

#[test]
fn an_integer_move_speed_moves_at_that_speed() {
    let (as_float, _) = metres_walked("move_speed = 8.0");
    let (as_integer, reports) = metres_walked("move_speed = 8");
    let (unset, _) = metres_walked("");
    assert_eq!(reports, 0, "an integer is a number, not a mistake");
    assert_eq!(
        as_integer, as_float,
        "`move_speed = 8` walks as far as `move_speed = 8.0`"
    );
    // 60 steps of 1/60 s at 8 m/s, less the step the kinematic body lags
    assert!((7.5..=8.01).contains(&as_integer), "walked {as_integer}");
    assert!((4.6..=5.01).contains(&unset), "the default is 5: {unset}");

    // A string is a mistake: the default, and one report however long it walks
    let (wrong, reports) = metres_walked("move_speed = \"fast\"");
    assert_eq!(wrong, unset);
    assert_eq!(reports, 1);
}

/// A floor and a character standing on it after half a second, whose
/// `character_controller` is `settings`.
fn standing(settings: &str) -> (FlintWorld, Walker, EntityId) {
    let mut world = FlintWorld::new();
    spawn_from_toml(
        &mut world,
        "floor",
        "[transform]\nposition = [0, -0.5, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [200, 1, 200]",
    );
    let id = spawn_from_toml(
        &mut world,
        "player",
        &format!(
            "[transform]\nposition = [0, 1, 0]\n[rigidbody]\nbody_type = \"kinematic\"\n\
             [collider]\nshape = \"capsule\"\nsize = [0.6, 1.8, 0.6]\n\
             [character_controller]\n{settings}"
        ),
    );
    let mut w = Walker::new(&mut world);
    let input = InputState::new();
    for _ in 0..30 {
        w.frame(&input, &mut world, 1.0 / 60.0);
    }
    (world, w, id)
}

/// Highest the character's feet rise over `steps` steps of 1/60 s.
fn rise(w: &mut Walker, world: &mut FlintWorld, id: EntityId, steps: usize) -> f32 {
    let input = InputState::new();
    let floor = world.get_transform(id).unwrap().position.y;
    let mut top = floor;
    for _ in 0..steps {
        w.frame(&input, world, 1.0 / 60.0);
        top = top.max(world.get_transform(id).unwrap().position.y);
    }
    top - floor
}

#[test]
fn a_jump_pressed_on_a_frame_with_no_step_still_jumps() {
    let (mut world, mut w, id) = standing("");
    // A 144 Hz frame that runs no fixed step: Space goes down and is
    // cleared again at the frame's end, before any step reads it
    let mut input = InputState::new();
    input.process_key_down(flint_runtime::parse_key_code("Space").unwrap());
    w.cc.frame_input(&input, &world);
    input.end_frame();
    assert!(!input.is_action_just_pressed("jump"));

    let height = rise(&mut w, &mut world, id, 30);
    // jump_force 6 under 9.81: about 1.8 at the top
    assert!(
        height > 1.0,
        "the press waited for the next step: rose {height}"
    );
}

#[test]
fn a_frame_of_several_steps_turns_once() {
    let (world, mut w, _) = standing("");
    let mut input = InputState::new();
    input.process_mouse_raw_delta(100.0, 0.0);
    w.cc.frame_input(&input, &world);
    let turned = -w.cc.yaw();
    // 100 units at 0.172 degrees each, however many steps follow
    assert!((turned - 17.2f32.to_radians()).abs() < 1e-5, "{turned}");
}

#[test]
fn a_character_starts_facing_its_start_yaw() {
    let (world, w, _) = standing("start_yaw = 90");
    assert!((w.cc.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    let eye = w.cc.camera_position(&world);
    let ahead = w.cc.camera_target(eye) - eye;
    assert!(
        ahead.x > 0.999 && ahead.z.abs() < 1e-3,
        "faces +X: {ahead:?}"
    );

    // Unset: +Z, as before
    let (world, w, _) = standing("");
    let eye = w.cc.camera_position(&world);
    let ahead = w.cc.camera_target(eye) - eye;
    assert!(ahead.z > 0.999, "faces +Z: {ahead:?}");
}

#[test]
fn the_look_and_fall_settings_are_the_components() {
    // Pitch held at 30 degrees, however far the mouse goes
    let (world, mut w, _) = standing("pitch_limit = 30");
    let mut input = InputState::new();
    input.process_mouse_raw_delta(0.0, -10_000.0);
    w.cc.frame_input(&input, &world);
    assert!((w.cc.pitch() - 30f32.to_radians()).abs() < 1e-6);

    // Twice the sensitivity, twice the turn
    let (world, mut w, _) = standing("look_sensitivity = 0.344");
    let mut input = InputState::new();
    input.process_mouse_raw_delta(100.0, 0.0);
    w.cc.frame_input(&input, &world);
    assert!((-w.cc.yaw() - 34.4f32.to_radians()).abs() < 1e-5);

    // Half the gravity, a jump about twice as high
    let jump_height = |settings: &str| {
        let (mut world, mut w, id) = standing(settings);
        let mut input = InputState::new();
        input.process_key_down(flint_runtime::parse_key_code("Space").unwrap());
        w.cc.frame_input(&input, &world);
        rise(&mut w, &mut world, id, 120)
    };
    let full = jump_height("");
    let half = jump_height("gravity_scale = 0.5");
    assert!((half / full - 2.0).abs() < 0.1, "{full} then {half}");
}

/// The controller drives its own body: the step leaves the body where the
/// walk put it instead of moving it back to the transform, which it writes
/// a step behind (#1618: physics no longer knows the component by name).
#[test]
fn the_players_body_is_the_controllers_and_its_eye_is_the_views() {
    let (world, w, id) = standing("height = 2.0");
    let mut view = PlayerView::default();
    w.cc.enter_view(&mut view);
    assert_eq!(view.player, Some(id));
    assert!(view.mouse_look);
    assert_eq!(view.controls, &ACTIONS);
    assert_eq!(view.eye, None);

    w.cc.place_eye(&world, &mut view);
    let eye = view.eye.unwrap();
    let feet = world.get_transform(id).unwrap().position;
    assert_eq!(eye.position.y, feet.y + 1.7, "85% of the height");
    assert!(w.cc.grounded(), "it settled on the floor");
}

/// No `character_controller`: no player, and an empty view.
#[test]
fn a_scene_with_no_controller_has_no_player() {
    let mut world = FlintWorld::new();
    spawn_from_toml(
        &mut world,
        "crate",
        "[rigidbody]\nbody_type = \"dynamic\"\n[collider]\nshape = \"box\"",
    );
    let mut sys = PhysicsSystem::new();
    sys.initialize(&mut world).unwrap();
    let mut cc = CharacterController::new();
    assert_eq!(cc.enter(&world, &mut sys), None);
    let mut view = PlayerView::default();
    cc.enter_view(&mut view);
    cc.place_eye(&world, &mut view);
    assert_eq!(view, PlayerView::default());
}
