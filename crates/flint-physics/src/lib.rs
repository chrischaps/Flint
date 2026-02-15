//! Flint Physics - Rapier 3D integration
//!
//! Provides physics simulation for the Flint engine:
//! - `PhysicsWorld` — wraps Rapier pipeline, body/collider sets
//! - `PhysicsSync` — bridges Flint's dynamic TOML components with Rapier bodies
//! - `CharacterController` — first-person kinematic character with gravity and collision
//! - `PhysicsSystem` — implements `RuntimeSystem` for integration into the game loop

pub mod character;
pub mod sync;
pub mod world;

use character::CharacterController;
use flint_core::{EntityId, Result};
use flint_ecs::FlintWorld;
use flint_runtime::{EventBus, GameEvent, InputState, RuntimeSystem};
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
}

/// Physics system implementing RuntimeSystem for the game loop
pub struct PhysicsSystem {
    pub physics_world: PhysicsWorld,
    pub sync: PhysicsSync,
    pub character: CharacterController,
    pub event_bus: EventBus,
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
            event_bus: EventBus::new(),
        }
    }

    /// Cast a ray and resolve the hit collider to an EntityId
    pub fn raycast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
        exclude_entity: Option<EntityId>,
    ) -> Option<EntityRaycastHit> {
        let exclude_collider = exclude_entity
            .and_then(|eid| self.sync.collider_map.get(&eid).copied());

        let hit = self.physics_world.cast_ray(origin, direction, max_distance, exclude_collider)?;

        // Resolve collider handle → EntityId
        let entity_id = self.sync.collider_map
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
            QueryFilter::default().exclude_rigid_body(body_handle),
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
        } else {
            None
        }
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

impl RuntimeSystem for PhysicsSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        // Initial sync of all entities with physics components
        self.sync.sync_to_rapier(world, &mut self.physics_world);

        // Find the player entity (entity with character_controller component)
        for entity in world.all_entities() {
            if let Some(components) = world.get_components(entity.id) {
                if components.has("character_controller") {
                    self.character.set_player_entity(entity.id);
                    break;
                }
            }
        }

        Ok(())
    }

    fn fixed_update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        // Sync any new entities to Rapier
        self.sync.sync_to_rapier(world, &mut self.physics_world);

        // Update kinematic bodies from ECS transforms (e.g., animated doors)
        self.sync.update_kinematic_bodies(world, &mut self.physics_world);

        // Update sensor flags (e.g., dead enemies become non-solid)
        self.sync.update_sensor_flags(world, &mut self.physics_world);

        // Step physics
        self.physics_world.step(dt as f32);

        // Drain collision events and push to event bus
        for event in self.physics_world.drain_collision_events() {
            match event {
                rapier3d::prelude::CollisionEvent::Started(h1, h2, _) => {
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
                        self.event_bus
                            .push(GameEvent::CollisionStarted {
                                entity_a: a,
                                entity_b: b,
                            });
                    }
                }
                rapier3d::prelude::CollisionEvent::Stopped(h1, h2, _) => {
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
                        self.event_bus
                            .push(GameEvent::CollisionEnded {
                                entity_a: a,
                                entity_b: b,
                            });
                    }
                }
            }
        }

        // Sync transforms back from Rapier for dynamic bodies
        self.sync.sync_from_rapier(world, &self.physics_world);

        Ok(())
    }

    fn update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Variable-rate updates (none needed for physics)
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "physics"
    }
}
