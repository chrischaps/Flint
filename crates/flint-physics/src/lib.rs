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
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::{EventBus, GameEvent, InputState, RuntimeSystem};
use sync::PhysicsSync;
use world::PhysicsWorld;

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
