//! Character controller using Rapier's kinematic character controller

use crate::world::PhysicsWorld;
use flint_core::components as comp;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::InputState;
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude::*;
use std::collections::HashMap;

/// How long a jump press is remembered, in seconds
const JUMP_BUFFER_WINDOW: f32 = 0.15;

/// The input actions the controller reads. Fixed names, the controller's
/// interface (ADR 0134): a game binds its own keys, pads and touch controls
/// to them in its input config. The player's controls line lists these.
pub const MOVE_FORWARD: &str = "move_forward";
pub const MOVE_BACKWARD: &str = "move_backward";
pub const MOVE_LEFT: &str = "move_left";
pub const MOVE_RIGHT: &str = "move_right";
pub const JUMP: &str = "jump";
pub const SPRINT: &str = "sprint";
/// All six, in the controls line's order.
pub const ACTIONS: [&str; 6] = [
    MOVE_FORWARD,
    MOVE_BACKWARD,
    MOVE_LEFT,
    MOVE_RIGHT,
    JUMP,
    SPRINT,
];

/// The `character_controller` fields the controller reads, with the
/// fallbacks the schema's defaults repeat
/// (`schemas/components/character_controller.toml`). Read again every
/// step, so a script can change them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    /// Walk speed, units a second
    pub move_speed: f32,
    /// Upward speed a jump starts with
    pub jump_force: f32,
    /// Speed times this while `sprint` is held
    pub sprint_multiplier: f32,
    /// Radians turned per unit of raw mouse motion (the field is degrees)
    pub look_sensitivity: f32,
    /// Pitch is held within +/- this, radians (the field is degrees)
    pub pitch_limit: f32,
    /// The physics world's gravity times this pulls the character down
    pub gravity_scale: f32,
    /// Tallest ledge walked up without a jump; 0 turns stepping off
    pub step_height: f32,
    /// Furthest the character is pulled down onto the ground; 0 turns it off
    pub snap_to_ground: f32,
}

impl Settings {
    pub(crate) fn of(world: &FlintWorld, id: EntityId) -> Self {
        let cc = world.view(id, comp::CHARACTER_CONTROLLER);
        Self {
            move_speed: cc.f32("move_speed", 5.0),
            jump_force: cc.f32("jump_force", 6.0),
            sprint_multiplier: cc.f32("sprint_multiplier", 1.6),
            look_sensitivity: cc.f32("look_sensitivity", 0.172).to_radians(),
            pitch_limit: cc.f32("pitch_limit", 80.0).to_radians(),
            gravity_scale: cc.f32("gravity_scale", 1.0),
            step_height: cc.f32("step_height", 0.5),
            snap_to_ground: cc.f32("snap_to_ground", 0.2),
        }
    }
}

/// First-person character controller wrapping Rapier's KinematicCharacterController
pub struct CharacterController {
    /// The Rapier character controller
    controller: KinematicCharacterController,
    /// Track which entity is the player character
    player_entity: Option<EntityId>,
    /// Current vertical velocity for gravity/jumping
    vertical_velocity: f32,
    /// Whether the character is on the ground
    pub(crate) grounded: bool,
    /// Remaining time in the jump input buffer (seconds). A press, taken
    /// once a frame by `frame_input`, sets it to `JUMP_BUFFER_WINDOW`; each
    /// physics step ticks it down. A jump executes whenever the character
    /// is grounded and this timer is still positive, so a press on a frame
    /// that ran no step waits for the next one.
    jump_buffer: f32,
    /// Camera yaw (horizontal look angle in radians)
    pub(crate) yaw: f32,
    /// Camera pitch (vertical look angle in radians)
    pub(crate) pitch: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self::new()
    }
}

impl CharacterController {
    pub fn new() -> Self {
        let controller = KinematicCharacterController {
            offset: CharacterLength::Absolute(0.01),
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(0.5),
                min_width: CharacterLength::Absolute(0.2),
                include_dynamic_bodies: false,
            }),
            snap_to_ground: Some(CharacterLength::Absolute(0.2)),
            ..KinematicCharacterController::default()
        };

        Self {
            controller,
            player_entity: None,
            vertical_velocity: 0.0,
            grounded: false,
            jump_buffer: 0.0,
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    /// Set which entity is the player character
    pub fn set_player_entity(&mut self, entity_id: EntityId) {
        self.player_entity = Some(entity_id);
    }

    /// Get the player entity
    pub fn player_entity(&self) -> Option<EntityId> {
        self.player_entity
    }

    /// Face the scene's `start_yaw` (degrees about +Y; 0 faces +Z, 90 faces
    /// +X), level, when the player entity is found.
    pub(crate) fn face_start(&mut self, world: &FlintWorld) {
        let Some(id) = self.player_entity else {
            return;
        };
        self.yaw = world
            .view(id, comp::CHARACTER_CONTROLLER)
            .f32("start_yaw", 0.0)
            .to_radians();
        self.pitch = 0.0;
    }

    /// The frame's share of input, taken once a frame before the fixed
    /// steps (#728): mouse look turns the view, and a jump press is
    /// buffered for the steps to come. Read inside each step, a frame of
    /// two steps turned twice as far, and a frame of none (most frames
    /// above 60 Hz) lost its motion and its press.
    pub fn frame_input(&mut self, input: &InputState, world: &FlintWorld) {
        let Some(id) = self.player_entity else {
            return;
        };
        if world.get_components(id).is_none() {
            return;
        }
        let settings = Settings::of(world, id);
        let (raw_dx, raw_dy) = input.raw_mouse_delta();
        self.yaw -= raw_dx as f32 * settings.look_sensitivity;
        self.pitch -= raw_dy as f32 * settings.look_sensitivity;
        let limit = settings.pitch_limit.abs();
        self.pitch = self.pitch.clamp(-limit, limit);

        if input.is_action_just_pressed(JUMP) {
            self.jump_buffer = JUMP_BUFFER_WINDOW;
        }
    }

    /// One fixed step of walking, falling and jumping. Look and the jump
    /// press come from `frame_input`; a step reads only what is held.
    pub fn update(
        &mut self,
        input: &InputState,
        world: &mut FlintWorld,
        physics: &mut PhysicsWorld,
        body_map: &HashMap<EntityId, RigidBodyHandle>,
        collider_map: &HashMap<EntityId, ColliderHandle>,
        dt: f64,
    ) {
        let entity_id = match self.player_entity {
            Some(id) => id,
            None => return,
        };

        let body_handle = match body_map.get(&entity_id) {
            Some(h) => *h,
            None => return,
        };

        let collider_handle = match collider_map.get(&entity_id) {
            Some(h) => *h,
            None => return,
        };

        // Read every step, so a script can change them; the view takes an
        // integer (`move_speed = 8` was 5.0, the default, before #685).
        if world.get_components(entity_id).is_none() {
            return;
        }
        let settings = Settings::of(world, entity_id);
        let (move_speed, jump_force) = (settings.move_speed, settings.jump_force);
        self.controller.autostep = (settings.step_height > 0.0).then_some(CharacterAutostep {
            max_height: CharacterLength::Absolute(settings.step_height),
            min_width: CharacterLength::Absolute(0.2),
            include_dynamic_bodies: false,
        });
        self.controller.snap_to_ground = (settings.snap_to_ground > 0.0)
            .then_some(CharacterLength::Absolute(settings.snap_to_ground));

        // Compute movement direction from input (relative to yaw)
        let mut move_dir = Vec3::ZERO;
        if input.is_action_pressed(MOVE_FORWARD) {
            move_dir.z -= 1.0;
        }
        if input.is_action_pressed(MOVE_BACKWARD) {
            move_dir.z += 1.0;
        }
        if input.is_action_pressed(MOVE_LEFT) {
            move_dir.x -= 1.0;
        }
        if input.is_action_pressed(MOVE_RIGHT) {
            move_dir.x += 1.0;
        }

        // Rotate movement by yaw to align with camera direction.
        // Camera forward is (sin(yaw), 0, cos(yaw)) and screen-right is
        // forward×up = (-cos(yaw), 0, sin(yaw)), so local -Z maps to
        // forward and local +X maps to screen-right.
        let cos_yaw = self.yaw.cos();
        let sin_yaw = self.yaw.sin();
        let world_move = Vec3::new(
            -move_dir.x * cos_yaw - move_dir.z * sin_yaw,
            0.0,
            move_dir.x * sin_yaw - move_dir.z * cos_yaw,
        );

        // Normalize horizontal movement
        let horizontal_len = (world_move.x * world_move.x + world_move.z * world_move.z).sqrt();
        let horizontal = if horizontal_len > 0.001 {
            let speed_mult = if input.is_action_pressed(SPRINT) {
                settings.sprint_multiplier
            } else {
                1.0
            };
            Vec3::new(
                world_move.x / horizontal_len * move_speed * speed_mult,
                0.0,
                world_move.z / horizontal_len * move_speed * speed_mult,
            )
        } else {
            Vec3::ZERO
        };

        // A press `frame_input` buffered is remembered for JUMP_BUFFER_WINDOW seconds
        self.jump_buffer = (self.jump_buffer - dt as f32).max(0.0);

        // Gravity and jumping
        if self.grounded {
            self.vertical_velocity = -0.1; // Small downward force to stay grounded
            if self.jump_buffer > 0.0 {
                self.vertical_velocity = jump_force;
                self.jump_buffer = 0.0;
            }
        } else {
            self.vertical_velocity += physics.gravity.y * settings.gravity_scale * dt as f32;
        }

        // Build desired translation
        let desired = vector![
            horizontal.x * dt as f32,
            self.vertical_velocity * dt as f32,
            horizontal.z * dt as f32
        ];

        // Use Rapier's character controller to compute corrected movement.
        // It meets what its collider meets: `collider.layer` and `mask`
        // (#729), so a player whose mask leaves out `pickup` walks through
        // pickups.
        let collider = physics.collider_set.get(collider_handle).unwrap();
        let corrected = self.controller.move_shape(
            dt as f32,
            &physics.rigid_body_set,
            &physics.collider_set,
            &physics.query_pipeline,
            collider.shape(),
            physics.rigid_body_set.get(body_handle).unwrap().position(),
            desired,
            QueryFilter::default()
                .exclude_rigid_body(body_handle)
                .exclude_sensors()
                .groups(collider.collision_groups()),
            |_| {},
        );

        // If we just landed and have a buffered jump, apply it immediately
        if corrected.grounded && !self.grounded && self.jump_buffer > 0.0 {
            self.vertical_velocity = jump_force;
            self.jump_buffer = 0.0;
        }
        self.grounded = corrected.grounded;

        // Apply the corrected movement to the kinematic body
        if let Some(body) = physics.get_rigid_body_mut(body_handle) {
            let current = *body.position();
            let new_pos = Isometry::new(
                current.translation.vector + corrected.translation,
                current.rotation.scaled_axis(),
            );
            body.set_next_kinematic_position(new_pos);
        }

        // Write back to Flint transform
        if let Some(body) = physics.get_rigid_body(body_handle) {
            let pos = body.translation();
            if let Some(mut components) = world.get_components_mut(entity_id) {
                let position_value = toml::Value::Array(vec![
                    toml::Value::Float(pos.x as f64),
                    toml::Value::Float(pos.y as f64),
                    toml::Value::Float(pos.z as f64),
                ]);
                components.set_field(comp::TRANSFORM, "position", position_value);
            }
        }
    }

    /// Get the camera position (player position + eye height)
    pub fn camera_position(&self, world: &FlintWorld) -> Vec3 {
        let entity_id = match self.player_entity {
            Some(id) => id,
            None => return Vec3::new(0.0, 2.0, 0.0),
        };

        let transform = world.get_transform(entity_id).unwrap_or_default();

        // Eye height: 85% of character_controller.height, 1.5 when unset
        let eye_offset = world
            .view(entity_id, comp::CHARACTER_CONTROLLER)
            .opt_f32("height")
            .map(|h| h * 0.85)
            .unwrap_or(1.5);

        Vec3::new(
            transform.position.x,
            transform.position.y + eye_offset,
            transform.position.z,
        )
    }

    /// Get the camera target point (position + forward direction from yaw/pitch)
    pub fn camera_target(&self, camera_pos: Vec3) -> Vec3 {
        let forward_x = self.pitch.cos() * self.yaw.sin();
        let forward_y = self.pitch.sin();
        let forward_z = self.pitch.cos() * self.yaw.cos();

        Vec3::new(
            camera_pos.x + forward_x,
            camera_pos.y + forward_y,
            camera_pos.z + forward_z,
        )
    }
}
