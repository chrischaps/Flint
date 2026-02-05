//! Character controller using Rapier's kinematic character controller

use crate::world::PhysicsWorld;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::InputState;
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude::*;
use std::collections::HashMap;

/// First-person character controller wrapping Rapier's KinematicCharacterController
pub struct CharacterController {
    /// The Rapier character controller
    controller: KinematicCharacterController,
    /// Track which entity is the player character
    player_entity: Option<EntityId>,
    /// Current vertical velocity for gravity/jumping
    vertical_velocity: f32,
    /// Whether the character is on the ground
    pub grounded: bool,
    /// Camera yaw (horizontal look angle in radians)
    pub yaw: f32,
    /// Camera pitch (vertical look angle in radians)
    pub pitch: f32,
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

    /// Update the character controller based on input
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

        // Read character controller settings from component
        let (move_speed, jump_force) = {
            let components = match world.get_components(entity_id) {
                Some(c) => c,
                None => return,
            };

            let cc_data = components.get("character_controller");
            let speed = cc_data
                .and_then(|v| v.get("move_speed"))
                .and_then(|v| v.as_float())
                .unwrap_or(5.0) as f32;
            let jump = cc_data
                .and_then(|v| v.get("jump_force"))
                .and_then(|v| v.as_float())
                .unwrap_or(6.0) as f32;
            (speed, jump)
        };

        // Process mouse look
        let (raw_dx, raw_dy) = input.raw_mouse_delta();
        let mouse_sensitivity = 0.003;
        self.yaw -= raw_dx as f32 * mouse_sensitivity;
        self.pitch -= raw_dy as f32 * mouse_sensitivity;
        self.pitch = self.pitch.clamp(-1.4, 1.4);

        // Compute movement direction from input (relative to yaw)
        let mut move_dir = Vec3::ZERO;
        if input.is_action_pressed("move_forward") {
            move_dir.z -= 1.0;
        }
        if input.is_action_pressed("move_backward") {
            move_dir.z += 1.0;
        }
        if input.is_action_pressed("move_left") {
            move_dir.x -= 1.0;
        }
        if input.is_action_pressed("move_right") {
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
        let horizontal_len =
            (world_move.x * world_move.x + world_move.z * world_move.z).sqrt();
        let horizontal = if horizontal_len > 0.001 {
            let speed_mult = if input.is_action_pressed("sprint") {
                1.6
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

        // Gravity and jumping
        if self.grounded {
            self.vertical_velocity = -0.1; // Small downward force to stay grounded
            if input.is_action_just_pressed("jump") {
                self.vertical_velocity = jump_force;
            }
        } else {
            self.vertical_velocity -= 9.81 * dt as f32;
        }

        // Build desired translation
        let desired = vector![
            horizontal.x * dt as f32,
            self.vertical_velocity * dt as f32,
            horizontal.z * dt as f32
        ];

        // Use Rapier's character controller to compute corrected movement
        let corrected = self.controller.move_shape(
            dt as f32,
            &physics.rigid_body_set,
            &physics.collider_set,
            &physics.query_pipeline,
            physics
                .collider_set
                .get(collider_handle)
                .unwrap()
                .shape(),
            physics
                .rigid_body_set
                .get(body_handle)
                .unwrap()
                .position(),
            desired,
            QueryFilter::default().exclude_rigid_body(body_handle),
            |_| {},
        );

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
            if let Some(components) = world.get_components_mut(entity_id) {
                let position_value = toml::Value::Array(vec![
                    toml::Value::Float(pos.x as f64),
                    toml::Value::Float(pos.y as f64),
                    toml::Value::Float(pos.z as f64),
                ]);
                components.set_field("transform", "position", position_value);
            }
        }
    }

    /// Get the camera position (player position + eye height)
    pub fn camera_position(&self, world: &FlintWorld) -> Vec3 {
        let entity_id = match self.player_entity {
            Some(id) => id,
            None => return Vec3::new(0.0, 2.0, 0.0),
        };

        let transform = world
            .get_transform(entity_id)
            .unwrap_or_default();

        // Read eye height from character_controller component (default: height * 0.85)
        let eye_offset = world
            .get_components(entity_id)
            .and_then(|c| c.get("character_controller"))
            .and_then(|v| v.get("height"))
            .and_then(|v| v.as_float())
            .map(|h| h as f32 * 0.85)
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
