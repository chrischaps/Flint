//! Physics world wrapping Rapier 3D

use rapier3d::prelude::*;

/// Wraps Rapier's physics pipeline and body/collider sets
pub struct PhysicsWorld {
    pub rigid_body_set: RigidBodySet,
    pub collider_set: ColliderSet,
    pub gravity: Vector<Real>,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: DefaultBroadPhase,
    pub narrow_phase: NarrowPhase,
    pub impulse_joint_set: ImpulseJointSet,
    pub multibody_joint_set: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
    pub query_pipeline: QueryPipeline,

    /// Collision events from the last step
    collision_recv: crossbeam::channel::Receiver<CollisionEvent>,
    contact_force_recv: crossbeam::channel::Receiver<ContactForceEvent>,
    event_handler: ChannelEventCollector,
}

impl PhysicsWorld {
    /// Create a new physics world with standard gravity
    pub fn new() -> Self {
        let (collision_send, collision_recv) = crossbeam::channel::unbounded();
        let (contact_force_send, contact_force_recv) = crossbeam::channel::unbounded();
        let event_handler = ChannelEventCollector::new(collision_send, contact_force_send);

        Self {
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            gravity: vector![0.0, -9.81, 0.0],
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            collision_recv,
            contact_force_recv,
            event_handler,
        }
    }

    /// Step the physics simulation by dt seconds
    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt;

        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &(),
            &self.event_handler,
        );
    }

    /// Drain collision events from the last step
    pub fn drain_collision_events(&self) -> Vec<CollisionEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.collision_recv.try_recv() {
            events.push(event);
        }
        events
    }

    /// Drain contact force events from the last step
    pub fn drain_contact_force_events(&self) -> Vec<ContactForceEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.contact_force_recv.try_recv() {
            events.push(event);
        }
        events
    }

    /// Insert a rigid body and return its handle
    pub fn insert_rigid_body(&mut self, body: RigidBody) -> RigidBodyHandle {
        self.rigid_body_set.insert(body)
    }

    /// Insert a collider attached to a rigid body
    pub fn insert_collider_with_parent(
        &mut self,
        collider: Collider,
        parent: RigidBodyHandle,
    ) -> ColliderHandle {
        self.collider_set
            .insert_with_parent(collider, parent, &mut self.rigid_body_set)
    }

    /// Remove a rigid body and its attached colliders
    pub fn remove_rigid_body(&mut self, handle: RigidBodyHandle) {
        self.rigid_body_set.remove(
            handle,
            &mut self.island_manager,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            true,
        );
    }

    /// Get a rigid body by handle
    pub fn get_rigid_body(&self, handle: RigidBodyHandle) -> Option<&RigidBody> {
        self.rigid_body_set.get(handle)
    }

    /// Get a mutable rigid body by handle
    pub fn get_rigid_body_mut(&mut self, handle: RigidBodyHandle) -> Option<&mut RigidBody> {
        self.rigid_body_set.get_mut(handle)
    }
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_physics_world() {
        let world = PhysicsWorld::new();
        assert_eq!(world.rigid_body_set.len(), 0);
        assert_eq!(world.collider_set.len(), 0);
    }

    #[test]
    fn test_insert_body_and_collider() {
        let mut world = PhysicsWorld::new();

        let body = RigidBodyBuilder::dynamic()
            .translation(vector![0.0, 5.0, 0.0])
            .build();
        let handle = world.insert_rigid_body(body);

        let collider = ColliderBuilder::ball(0.5).build();
        world.insert_collider_with_parent(collider, handle);

        assert_eq!(world.rigid_body_set.len(), 1);
        assert_eq!(world.collider_set.len(), 1);
    }

    #[test]
    fn test_gravity_simulation() {
        let mut world = PhysicsWorld::new();

        let body = RigidBodyBuilder::dynamic()
            .translation(vector![0.0, 10.0, 0.0])
            .build();
        let handle = world.insert_rigid_body(body);

        let collider = ColliderBuilder::ball(0.5).build();
        world.insert_collider_with_parent(collider, handle);

        let initial_y = world
            .get_rigid_body(handle)
            .unwrap()
            .translation()
            .y;

        // Step several times
        for _ in 0..60 {
            world.step(1.0 / 60.0);
        }

        let final_y = world
            .get_rigid_body(handle)
            .unwrap()
            .translation()
            .y;

        // Object should have fallen due to gravity
        assert!(final_y < initial_y);
    }
}
