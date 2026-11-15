//! Flint Physics - Rapier 3D integration
//!
//! Provides physics simulation for the Flint engine:
//! - `PhysicsWorld` — wraps Rapier pipeline, body/collider sets
//! - `PhysicsSync` — bridges Flint's dynamic TOML components with Rapier bodies
//! - `CharacterController` — first-person kinematic character with gravity and collision
//! - `PhysicsSystem` — the fixed-step simulation the player steps; its contacts go
//!   into the caller's `EventBus`

pub mod character;
pub mod sync;
pub mod world;

#[cfg(test)]
mod joint_tests;

use character::CharacterController;
use flint_core::components as comp;
use flint_core::{EntityId, Result, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::{EventBus, GameEvent, InputState};
use rapier3d::control::{CharacterLength, KinematicCharacterController};
use rapier3d::prelude::*;
use sync::PhysicsSync;
use world::PhysicsWorld;

/// Entity-level raycast result
#[derive(Debug, Clone)]
pub struct EntityRaycastHit {
    pub entity_id: EntityId,
    pub distance: f32,
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

/// Result of a collision-corrected character movement
#[derive(Debug, Clone)]
pub struct MoveCharacterResult {
    /// Corrected absolute position after collision resolution
    pub position: [f32; 3],
    /// Whether the character is touching ground after movement
    pub grounded: bool,
}

/// Collider shape dimensions for an entity
#[derive(Debug, Clone)]
pub enum ColliderExtents {
    Box { half_extents: [f32; 3] },
    Sphere { radius: f32 },
    Capsule { radius: f32, half_height: f32 },
    Cylinder { radius: f32, half_height: f32 },
}

/// The physics simulation: initialized from the world when a scene is
/// entered, then stepped at the fixed rate ([`fixed_update`](Self::fixed_update)).
/// It keeps no events: each step pushes its collisions and triggers into the
/// bus the caller hands it, which the player owns (#727).
pub struct PhysicsSystem {
    pub(crate) physics_world: PhysicsWorld,
    pub(crate) sync: PhysicsSync,
    pub(crate) character: CharacterController,
    /// Fixed steps this frame will run (set by the frame loop) and the index
    /// of the step in progress, for even kinematic motion across the frame
    frame_steps: usize,
    step_in_frame: usize,
    /// Wall time since the last fixed step, in steps: `frame_steps` plus the
    /// accumulator remainder. Kinematic targets are interpolated over this
    /// span so each step moves an equal share of the scripted motion
    frame_span: f32,
}

impl Default for PhysicsSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicsSystem {
    pub fn new() -> Self {
        Self {
            physics_world: PhysicsWorld::new(),
            sync: PhysicsSync::new(),
            character: CharacterController::new(),
            frame_steps: 1,
            step_in_frame: 0,
            frame_span: 1.0,
        }
    }

    /// Announce how many fixed steps the coming frame will run, so scripted
    /// kinematic motion is spread evenly across them instead of landing on
    /// the first step (see `PhysicsSync::update_kinematic_bodies`). Callers
    /// that never announce get the old one-step-per-frame behaviour.
    pub fn begin_frame_steps(&mut self, steps: usize) {
        self.frame_steps = steps.max(1);
        self.frame_span = self.frame_steps as f32;
        self.step_in_frame = 0;
    }

    /// Like `begin_frame_steps`, from the clock's pending fixed time in steps
    /// (`GameClock::pending_fixed_ratio`): the whole number of steps runs this
    /// frame and the fraction is carried over. Kinematic bodies move
    /// `1 / ratio` of the way to the scripted pose per step, so their step
    /// velocity is the scripted wall-clock velocity whether a frame holds
    /// several steps or a step spans several frames. The body trails the
    /// scripted pose by the carried fraction (under one step); jointed
    /// children are written back against the body's pose, so the trail is
    /// invisible to the scene.
    pub fn begin_frame(&mut self, pending_ratio: f64) {
        let ratio = pending_ratio.max(0.0) as f32;
        self.frame_steps = (ratio.floor() as usize).max(1);
        self.frame_span = ratio.max(1.0);
        self.step_in_frame = 0;
    }

    /// Cast a ray and resolve the hit collider to an EntityId
    pub fn raycast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
        exclude_entity: Option<EntityId>,
    ) -> Option<EntityRaycastHit> {
        let exclude_collider =
            exclude_entity.and_then(|eid| self.sync.collider_map.get(&eid).copied());

        let hit = self
            .physics_world
            .cast_ray(origin, direction, max_distance, exclude_collider)?;

        // Resolve collider handle → EntityId
        let entity_id = self
            .sync
            .collider_map
            .iter()
            .find(|(_, ch)| **ch == hit.collider_handle)
            .map(|(eid, _)| *eid)?;

        Some(EntityRaycastHit {
            entity_id,
            distance: hit.distance,
            point: hit.point,
            normal: hit.normal,
        })
    }

    /// Collision-corrected kinematic character movement using Rapier's shape-sweep.
    /// Scripts pass the desired movement delta; the engine resolves collisions and
    /// returns the corrected absolute position plus grounded status.
    pub fn move_character_shape(
        &self,
        entity_id: EntityId,
        current_pos: [f32; 3],
        desired_delta: [f32; 3],
        dt: f32,
    ) -> Option<MoveCharacterResult> {
        let collider_handle = *self.sync.collider_map.get(&entity_id)?;
        let body_handle = *self.sync.body_map.get(&entity_id)?;

        let collider = self.physics_world.collider_set.get(collider_handle)?;
        let shape = collider.shape();

        // Build isometry from the ECS position (freshest data from scripts)
        let position = Isometry::translation(current_pos[0], current_pos[1], current_pos[2]);

        // Lightweight config struct — no snap/autostep so scripts own their physics feel
        let controller = KinematicCharacterController {
            offset: CharacterLength::Absolute(0.01),
            snap_to_ground: None,
            autostep: None,
            ..KinematicCharacterController::default()
        };

        let desired = vector![desired_delta[0], desired_delta[1], desired_delta[2]];

        let corrected = controller.move_shape(
            dt,
            &self.physics_world.rigid_body_set,
            &self.physics_world.collider_set,
            &self.physics_world.query_pipeline,
            shape,
            &position,
            desired,
            QueryFilter::default()
                .exclude_rigid_body(body_handle)
                .exclude_sensors(),
            |_| {},
        );

        let resolved = [
            current_pos[0] + corrected.translation.x,
            current_pos[1] + corrected.translation.y,
            current_pos[2] + corrected.translation.z,
        ];

        Some(MoveCharacterResult {
            position: resolved,
            grounded: corrected.grounded,
        })
    }

    /// Current joint coordinate of an entity's joint (hinge degrees or
    /// prismatic metres), or `None` when it has no hinge/prismatic joint.
    pub fn joint_position(&self, entity_id: EntityId) -> Option<f32> {
        self.sync.joint_position(entity_id, &self.physics_world)
    }

    /// Number of live impulse joints (diagnostics)
    pub fn joint_count(&self) -> usize {
        self.physics_world.impulse_joint_count()
    }

    /// Query an entity's collider shape dimensions.
    pub fn get_entity_collider_extents(&self, entity_id: EntityId) -> Option<ColliderExtents> {
        let collider_handle = *self.sync.collider_map.get(&entity_id)?;
        let collider = self.physics_world.collider_set.get(collider_handle)?;
        let shape = collider.shape();

        if let Some(cuboid) = shape.as_cuboid() {
            let he = cuboid.half_extents;
            Some(ColliderExtents::Box {
                half_extents: [he.x, he.y, he.z],
            })
        } else if let Some(ball) = shape.as_ball() {
            Some(ColliderExtents::Sphere {
                radius: ball.radius,
            })
        } else if let Some(capsule) = shape.as_capsule() {
            Some(ColliderExtents::Capsule {
                radius: capsule.radius,
                half_height: capsule.half_height(),
            })
        } else if let Some(cyl) = shape.as_cylinder() {
            Some(ColliderExtents::Cylinder {
                radius: cyl.radius,
                half_height: cyl.half_height,
            })
        } else {
            None
        }
    }

    /// Query all entities whose colliders overlap an axis-aligned rectangle in the XY plane.
    pub fn overlap_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Vec<EntityId> {
        let shape = rapier3d::prelude::Cuboid::new(vector![w * 0.5, h * 0.5, 1000.0]);
        let shape_pos = Isometry::translation(x + w * 0.5, y + h * 0.5, 0.0);

        let mut result = Vec::new();
        self.physics_world.query_pipeline.intersections_with_shape(
            &self.physics_world.rigid_body_set,
            &self.physics_world.collider_set,
            &shape_pos,
            &shape,
            QueryFilter::default(),
            |handle| {
                if let Some(eid) = self
                    .sync
                    .collider_map
                    .iter()
                    .find(|(_, ch)| **ch == handle)
                    .map(|(eid, _)| *eid)
                {
                    result.push(eid);
                }
                true // continue searching
            },
        );
        result
    }

    /// Cast a 2D ray in the XY plane (Z=0) and return the first hit.
    pub fn raycast_2d(
        &self,
        ox: f32,
        oy: f32,
        dx: f32,
        dy: f32,
        max_distance: f32,
        exclude_entity: Option<EntityId>,
    ) -> Option<EntityRaycastHit> {
        // Normalize direction for the 3D ray
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-8 {
            return None;
        }
        self.raycast(
            [ox, oy, 0.0],
            [dx / len, dy / len, 0.0],
            max_distance,
            exclude_entity,
        )
    }

    /// Set linear velocity on a 2D body (Z velocity = 0).
    pub fn set_velocity_2d(&mut self, entity_id: EntityId, vx: f32, vy: f32) {
        if let Some(body_handle) = self.sync.body_map.get(&entity_id) {
            if let Some(body) = self.physics_world.get_rigid_body_mut(*body_handle) {
                body.set_linvel(vector![vx, vy, 0.0], true);
            }
        }
    }

    /// Get linear velocity of a 2D body.
    pub fn get_velocity_2d(&self, entity_id: EntityId) -> Option<[f32; 2]> {
        let body_handle = self.sync.body_map.get(&entity_id)?;
        let body = self.physics_world.get_rigid_body(*body_handle)?;
        let vel = body.linvel();
        Some([vel.x, vel.y])
    }

    /// Sync specific new entities to Rapier (for chunk loading).
    pub fn sync_new_entities(&mut self, world: &mut FlintWorld, _entity_ids: &[EntityId]) {
        // sync_to_rapier already skips already-synced entities, so we can just re-run it
        self.sync.sync_to_rapier(world, &mut self.physics_world);
    }

    /// Remove a single entity from the physics world (for chunk unloading).
    pub fn remove_entity(&mut self, entity_id: EntityId) {
        // Remove rigid body (which also removes attached colliders and joints in Rapier)
        if let Some(body_handle) = self.sync.body_map.remove(&entity_id) {
            self.sync
                .forget_joints_of_body(body_handle, &self.physics_world);
            self.physics_world.remove_rigid_body(body_handle);
        } else if let Some(joint_handle) = self.sync.joint_map.remove(&entity_id) {
            self.physics_world.remove_impulse_joint(joint_handle);
            self.sync.joint_cache.remove(&entity_id);
        }
        // Remove standalone collider if any
        if let Some(collider_handle) = self.sync.collider_map.remove(&entity_id) {
            self.physics_world.remove_collider(collider_handle);
        }
        self.sync.synced_entities.remove(&entity_id);
    }

    /// Reset all physics state for a scene transition.
    /// Replaces the world, clears sync maps, resets the character. Events are
    /// the caller's bus's, and stay there.
    pub fn clear(&mut self) {
        self.physics_world = PhysicsWorld::new();
        self.sync.clear();
        self.character = CharacterController::new();
    }

    /// Whether a player entity has been assigned to the character controller
    pub fn has_player_entity(&self) -> bool {
        self.character.player_entity().is_some()
    }

    /// Camera yaw angle in radians (horizontal look)
    pub fn camera_yaw(&self) -> f32 {
        self.character.yaw
    }

    /// Camera pitch angle in radians (vertical look)
    pub fn camera_pitch(&self) -> f32 {
        self.character.pitch
    }

    /// Camera position derived from the player entity's transform + eye height
    pub fn camera_position(&self, world: &FlintWorld) -> Vec3 {
        self.character.camera_position(world)
    }

    /// Camera look-at target from yaw/pitch
    pub fn camera_target(&self, camera_pos: Vec3) -> Vec3 {
        self.character.camera_target(camera_pos)
    }

    /// Register a static trimesh collider from raw geometry
    pub fn register_static_trimesh(
        &mut self,
        entity_id: EntityId,
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
        friction: f32,
        restitution: f32,
    ) {
        self.sync.register_static_trimesh(
            entity_id,
            &mut self.physics_world,
            vertices,
            indices,
            friction,
            restitution,
        );
    }

    /// Run the character controller update (called from player app with input access)
    pub fn update_character(&mut self, input: &InputState, world: &mut FlintWorld, dt: f64) {
        self.character.update(
            input,
            world,
            &mut self.physics_world,
            &self.sync.body_map,
            &self.sync.collider_map,
            dt,
        );
    }
}

impl PhysicsSystem {
    /// Read the scene's bodies, colliders and joints, and find the player
    /// entity, when a scene is entered.
    pub fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        // Initial sync of all entities with physics components, then joints
        self.sync.sync_to_rapier(world, &mut self.physics_world);
        self.sync.sync_joints(world, &mut self.physics_world);

        // Find the player entity (entity with character_controller component)
        for entity in world.all_entities() {
            if let Some(components) = world.get_components(entity.id) {
                if components.has(comp::CHARACTER_CONTROLLER) {
                    self.character.set_player_entity(entity.id);
                    break;
                }
            }
        }

        Ok(())
    }

    /// One fixed step of `dt` seconds: new bodies and joints in, kinematic
    /// poses and joint targets from the world, the step, then this step's
    /// collisions and triggers pushed onto `events` and dynamic bodies'
    /// poses written back to the world.
    pub fn fixed_update(
        &mut self,
        world: &mut FlintWorld,
        dt: f64,
        events: &mut EventBus,
    ) -> Result<()> {
        // Sync any new entities to Rapier, then any joints whose bodies now exist
        self.sync.sync_to_rapier(world, &mut self.physics_world);
        self.sync.sync_joints(world, &mut self.physics_world);

        // Update kinematic bodies from ECS transforms (e.g., animated doors),
        // moving them an even share of the frame's motion per step
        self.sync.update_kinematic_bodies(
            world,
            &mut self.physics_world,
            self.step_in_frame,
            self.frame_span,
        );
        self.step_in_frame += 1;

        // Update sensor flags (e.g., dead enemies become non-solid)
        self.sync
            .update_sensor_flags(world, &mut self.physics_world);

        // Push script-written joint targets / limits into Rapier
        self.sync
            .update_joint_targets(world, &mut self.physics_world);

        // Step physics
        self.physics_world.step(dt as f32);

        // This step's collisions and triggers, onto the caller's bus
        for event in self.physics_world.drain_collision_events() {
            match event {
                rapier3d::prelude::CollisionEvent::Started(h1, h2, flags) => {
                    // Map collider handles back to entity IDs
                    let e1 = self
                        .sync
                        .collider_map
                        .iter()
                        .find(|(_, ch)| **ch == h1)
                        .map(|(eid, _)| *eid);
                    let e2 = self
                        .sync
                        .collider_map
                        .iter()
                        .find(|(_, ch)| **ch == h2)
                        .map(|(eid, _)| *eid);
                    if let (Some(a), Some(b)) = (e1, e2) {
                        if flags.contains(CollisionEventFlags::SENSOR) {
                            // Determine which is the sensor (trigger) and which is the entity
                            let a_is_sensor = self
                                .physics_world
                                .collider_set
                                .get(h1)
                                .map(|c| c.is_sensor())
                                .unwrap_or(false);
                            let (trigger, entity) = if a_is_sensor { (a, b) } else { (b, a) };
                            events.push(GameEvent::TriggerEntered { entity, trigger });
                        } else {
                            events.push(GameEvent::CollisionStarted {
                                entity_a: a,
                                entity_b: b,
                            });
                        }
                    }
                }
                rapier3d::prelude::CollisionEvent::Stopped(h1, h2, flags) => {
                    let e1 = self
                        .sync
                        .collider_map
                        .iter()
                        .find(|(_, ch)| **ch == h1)
                        .map(|(eid, _)| *eid);
                    let e2 = self
                        .sync
                        .collider_map
                        .iter()
                        .find(|(_, ch)| **ch == h2)
                        .map(|(eid, _)| *eid);
                    if let (Some(a), Some(b)) = (e1, e2) {
                        if flags.contains(CollisionEventFlags::SENSOR) {
                            let a_is_sensor = self
                                .physics_world
                                .collider_set
                                .get(h1)
                                .map(|c| c.is_sensor())
                                .unwrap_or(false);
                            let (trigger, entity) = if a_is_sensor { (a, b) } else { (b, a) };
                            events.push(GameEvent::TriggerExited { entity, trigger });
                        } else {
                            events.push(GameEvent::CollisionEnded {
                                entity_a: a,
                                entity_b: b,
                            });
                        }
                    }
                }
            }
        }

        // Sync transforms back from Rapier for dynamic bodies
        self.sync
            .project_kinematic_joints(&mut self.physics_world, dt as f32);
        self.sync.sync_from_rapier(world, &self.physics_world);

        Ok(())
    }

    /// A fixed step for a test that reads no contacts.
    #[cfg(test)]
    pub(crate) fn step(&mut self, world: &mut FlintWorld, dt: f64) {
        self.fixed_update(world, dt, &mut EventBus::new()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a FlintWorld entity with transform, rigidbody, and collider
    fn spawn_physics_entity(
        world: &mut FlintWorld,
        name: &str,
        pos: [f64; 3],
        body_type: &str,
        shape: &str,
        size: [f64; 3],
        is_sensor: bool,
        mode_2d: bool,
    ) -> EntityId {
        let id = world.spawn(name).unwrap();

        let transform_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(pos[0]),
                    toml::Value::Float(pos[1]),
                    toml::Value::Float(pos[2]),
                ]),
            );
            t
        });
        world
            .set_component(id, "transform", transform_data)
            .unwrap();

        let rb_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("body_type".into(), toml::Value::String(body_type.into()));
            t.insert("mode_2d".into(), toml::Value::Boolean(mode_2d));
            t
        });
        world.set_component(id, "rigidbody", rb_data).unwrap();

        let col_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("shape".into(), toml::Value::String(shape.into()));
            t.insert(
                "size".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(size[0]),
                    toml::Value::Float(size[1]),
                    toml::Value::Float(size[2]),
                ]),
            );
            t.insert("is_sensor".into(), toml::Value::Boolean(is_sensor));
            t
        });
        world.set_component(id, "collider", col_data).unwrap();

        id
    }

    #[test]
    fn test_sensor_emits_trigger_not_collision() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        // Dynamic ball that will fall into a sensor
        let _ball = spawn_physics_entity(
            &mut world,
            "ball",
            [0.0, 3.0, 0.0],
            "dynamic",
            "sphere",
            [1.0, 1.0, 1.0],
            false,
            false,
        );

        // Static sensor volume at origin
        let _sensor = spawn_physics_entity(
            &mut world,
            "sensor",
            [0.0, 0.0, 0.0],
            "static",
            "box",
            [4.0, 4.0, 4.0],
            true,
            false,
        );

        sys.initialize(&mut world).unwrap();

        // Step physics until ball falls into sensor
        let mut bus = EventBus::new();
        for _ in 0..120 {
            sys.fixed_update(&mut world, 1.0 / 60.0, &mut bus).unwrap();
        }

        let events = bus.drain();
        let has_trigger = events
            .iter()
            .any(|e| matches!(e, GameEvent::TriggerEntered { .. }));
        let has_collision = events
            .iter()
            .any(|e| matches!(e, GameEvent::CollisionStarted { .. }));

        assert!(has_trigger, "Sensor collision should emit TriggerEntered");
        assert!(
            !has_collision,
            "Sensor collision should NOT emit CollisionStarted"
        );
    }

    #[test]
    fn test_solid_emits_collision_not_trigger() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        // Dynamic ball
        spawn_physics_entity(
            &mut world,
            "ball",
            [0.0, 3.0, 0.0],
            "dynamic",
            "sphere",
            [1.0, 1.0, 1.0],
            false,
            false,
        );

        // Static solid floor
        spawn_physics_entity(
            &mut world,
            "floor",
            [0.0, 0.0, 0.0],
            "static",
            "box",
            [10.0, 0.5, 10.0],
            false,
            false,
        );

        sys.initialize(&mut world).unwrap();

        let mut bus = EventBus::new();
        for _ in 0..120 {
            sys.fixed_update(&mut world, 1.0 / 60.0, &mut bus).unwrap();
        }

        let events = bus.drain();
        let has_collision = events
            .iter()
            .any(|e| matches!(e, GameEvent::CollisionStarted { .. }));
        let has_trigger = events
            .iter()
            .any(|e| matches!(e, GameEvent::TriggerEntered { .. }));

        assert!(
            has_collision,
            "Solid collision should emit CollisionStarted"
        );
        assert!(
            !has_trigger,
            "Solid collision should NOT emit TriggerEntered"
        );
    }

    #[test]
    fn test_mode_2d_locks_z_to_zero() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        // Dynamic body with mode_2d at Z=5 (should be forced to 0)
        let id = spawn_physics_entity(
            &mut world,
            "player",
            [0.0, 5.0, 5.0],
            "dynamic",
            "box",
            [1.0, 1.0, 1.0],
            false,
            true,
        );

        sys.initialize(&mut world).unwrap();

        // Step and sync back
        for _ in 0..10 {
            sys.step(&mut world, 1.0 / 60.0);
        }

        let transform = world.get_transform(id).unwrap();
        assert!(
            transform.position.z.abs() < 0.001,
            "mode_2d should zero out Z, got {}",
            transform.position.z
        );
    }

    #[test]
    fn test_sprite_shape_reads_dimensions() {
        let mut world = FlintWorld::new();

        let id = world.spawn("sprite_entity").unwrap();

        let transform_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                ]),
            );
            t
        });
        world
            .set_component(id, "transform", transform_data)
            .unwrap();

        // Sprite with specific dimensions
        let sprite_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("width".into(), toml::Value::Float(2.0));
            t.insert("height".into(), toml::Value::Float(3.0));
            t
        });
        world.set_component(id, "sprite", sprite_data).unwrap();

        let rb_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("body_type".into(), toml::Value::String("static".into()));
            t
        });
        world.set_component(id, "rigidbody", rb_data).unwrap();

        let col_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("shape".into(), toml::Value::String("sprite".into()));
            t
        });
        world.set_component(id, "collider", col_data).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();
        sync.sync_to_rapier(&world, &mut physics);

        // Check collider extents match sprite dimensions
        let col_handle = *sync.collider_map.get(&id).unwrap();
        let collider = physics.collider_set.get(col_handle).unwrap();
        let cuboid = collider.shape().as_cuboid().expect("Should be a cuboid");
        let he = cuboid.half_extents;

        assert!(
            (he.x - 1.0).abs() < 0.01,
            "Half-width should be 1.0, got {}",
            he.x
        );
        assert!(
            (he.y - 1.5).abs() < 0.01,
            "Half-height should be 1.5, got {}",
            he.y
        );
        assert!(
            (he.z - 0.1).abs() < 0.01,
            "Half-depth should be 0.1, got {}",
            he.z
        );
    }

    #[test]
    fn test_overlap_rect() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        // Entity inside the query rect
        let inside = spawn_physics_entity(
            &mut world,
            "inside",
            [1.0, 1.0, 0.0],
            "static",
            "box",
            [0.5, 0.5, 0.5],
            false,
            true,
        );

        // Entity outside the query rect
        let outside = spawn_physics_entity(
            &mut world,
            "outside",
            [10.0, 10.0, 0.0],
            "static",
            "box",
            [0.5, 0.5, 0.5],
            false,
            true,
        );

        sys.initialize(&mut world).unwrap();
        // Need to step once so query pipeline is updated
        sys.step(&mut world, 1.0 / 60.0);

        let results = sys.overlap_rect(0.0, 0.0, 3.0, 3.0);
        assert!(
            results.contains(&inside),
            "Should find entity at (1,1) in rect (0,0)-(3,3)"
        );
        assert!(
            !results.contains(&outside),
            "Should NOT find entity at (10,10) in rect (0,0)-(3,3)"
        );
    }

    #[test]
    fn test_raycast_2d() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        // Target at x=5, y=0
        let target = spawn_physics_entity(
            &mut world,
            "target",
            [5.0, 0.0, 0.0],
            "static",
            "box",
            [1.0, 1.0, 1.0],
            false,
            true,
        );

        sys.initialize(&mut world).unwrap();
        sys.step(&mut world, 1.0 / 60.0);

        // Cast ray from origin rightward
        let hit = sys.raycast_2d(0.0, 0.0, 1.0, 0.0, 20.0, None);
        assert!(hit.is_some(), "Raycast should hit the target");
        let hit = hit.unwrap();
        assert_eq!(hit.entity_id, target);
        assert!(hit.distance > 0.0 && hit.distance < 6.0);
    }

    #[test]
    fn test_set_get_velocity_2d() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        let id = spawn_physics_entity(
            &mut world,
            "mover",
            [0.0, 0.0, 0.0],
            "dynamic",
            "box",
            [1.0, 1.0, 1.0],
            false,
            true,
        );

        sys.initialize(&mut world).unwrap();

        // Set velocity
        sys.set_velocity_2d(id, 3.0, 7.0);

        // Get it back
        let vel = sys.get_velocity_2d(id);
        assert!(vel.is_some(), "Should be able to read velocity back");
        let vel = vel.unwrap();
        assert!(
            (vel[0] - 3.0).abs() < 0.01,
            "vx should be 3.0, got {}",
            vel[0]
        );
        assert!(
            (vel[1] - 7.0).abs() < 0.01,
            "vy should be 7.0, got {}",
            vel[1]
        );
    }

    // ------------------------------------------------------------------
    // Typed reads (ADR 0115 decision 5, #685): each of the ticket's cases
    // ------------------------------------------------------------------

    fn spawn_from_toml(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
        let id = world.spawn(name).unwrap();
        let table: toml::Table = toml::from_str(text).unwrap();
        for (component, data) in table {
            world.set_component(id, &component, data).unwrap();
        }
        id
    }

    #[test]
    fn an_unknown_body_type_warns_with_the_entity_and_the_allowed_values() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        let id = spawn_from_toml(
            &mut world,
            "crate",
            "[transform]\nposition = [0, 5, 0]\n[rigidbody]\nbody_type = \"dynamc\"\n\
             [collider]\nshape = \"box\"",
        );
        sys.initialize(&mut world).unwrap();
        for _ in 0..30 {
            sys.step(&mut world, 1.0 / 60.0);
        }

        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 1, "once, however many steps run: {said:?}");
        assert_eq!(
            said[0].one_line(),
            "error[bad-enum-value] entity 'crate' rigidbody.body_type: \"dynamc\" is not a \
             value of `body_type` (did you mean `dynamic`? allowed values: static, dynamic, \
             kinematic, kinematic_position, kinematic_velocity; the default \"static\" is used)"
        );
        // It still falls back to a static body, as it always did
        let y = world.get_transform(id).unwrap().position.y;
        assert_eq!(y, 5.0, "a static body does not fall");
    }

    #[test]
    fn an_unknown_shape_and_joint_type_warn() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        spawn_from_toml(
            &mut world,
            "anchor",
            "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"",
        );
        spawn_from_toml(
            &mut world,
            "arm",
            "[transform]\nposition = [0, 1, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
             [collider]\nshape = \"spere\"\nfriction = \"slippy\"\n\
             [joint]\ntype = \"hing\"\nparent = \"anchor\"\nlimits = [10]",
        );
        sys.initialize(&mut world).unwrap();
        sys.step(&mut world, 1.0 / 60.0);
        sys.step(&mut world, 1.0 / 60.0);

        let said = world.read_reports().snapshot();
        let lines: Vec<String> = said.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            lines,
            [
                "error[bad-enum-value] entity 'arm' collider.shape: \"spere\" is not a value of \
                 `shape`",
                "error[bad-type] entity 'arm' collider.friction: expected a number, got string \
                 \"slippy\"",
                "error[bad-enum-value] entity 'arm' joint.type: \"hing\" is not a value of `type`",
                "error[bad-type] entity 'arm' joint.limits: expected 2 numbers, [min, max], got \
                 [10]",
            ]
        );
        assert_eq!(
            said[0].hint.as_deref(),
            Some(
                "did you mean `sphere`? allowed values: box, sphere, capsule, cylinder, sprite; \
                 the default \"box\" is used"
            )
        );
    }

    #[test]
    fn a_collider_without_a_rigidbody_warns_once() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        spawn_from_toml(
            &mut world,
            "wall",
            "[transform]\nposition = [0, 0, 0]\n[collider]\nshape = \"box\"",
        );
        spawn_from_toml(
            &mut world,
            "floor",
            "[rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"box\"",
        );
        sys.initialize(&mut world).unwrap();
        assert!(
            world.read_reports().is_empty(),
            "the first pass waits: a script may add the rigidbody a step later"
        );
        for _ in 0..10 {
            sys.step(&mut world, 1.0 / 60.0);
        }
        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(
            said[0].one_line(),
            "warning[needs-component] entity 'wall' component 'collider': a collider with no \
             rigidbody collides with nothing (add a `rigidbody` to the entity, `body_type = \
             \"static\"` for scenery, or remove the collider)"
        );
    }

    #[test]
    fn a_rigidbody_added_a_step_after_its_collider_is_not_reported() {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        let id = spawn_from_toml(&mut world, "late", "[collider]\nshape = \"box\"");
        sys.initialize(&mut world).unwrap();
        let body: toml::Value = toml::from_str("body_type = \"static\"").unwrap();
        world.set_component(id, "rigidbody", body).unwrap();
        for _ in 0..5 {
            sys.step(&mut world, 1.0 / 60.0);
        }
        assert!(world.read_reports().is_empty());
        assert!(sys.sync.get_body_handle(id).is_some());
    }

    /// Metres walked forward in one second by a character whose
    /// `character_controller` is `settings`.
    fn metres_walked(settings: &str) -> (f32, usize) {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
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
        sys.initialize(&mut world).unwrap();
        let mut input = InputState::new();
        let dt = 1.0 / 60.0;
        // Settle on the floor, then hold forward for a second
        for _ in 0..30 {
            sys.update_character(&input, &mut world, dt);
            sys.step(&mut world, dt);
        }
        let start = world.get_transform(id).unwrap().position;
        input.process_key_down(flint_runtime::parse_key_code("KeyW").unwrap());
        for _ in 0..60 {
            sys.update_character(&input, &mut world, dt);
            sys.step(&mut world, dt);
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
}
