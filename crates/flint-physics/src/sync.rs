//! Synchronization between FlintWorld (TOML components) and Rapier physics

use crate::world::PhysicsWorld;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use rapier3d::na;
use rapier3d::prelude::*;
use std::collections::HashMap;

/// Bridges Flint's dynamic components with Rapier's rigid body and collider sets
pub struct PhysicsSync {
    /// EntityId -> RigidBodyHandle mapping
    pub body_map: HashMap<EntityId, RigidBodyHandle>,
    /// EntityId -> ColliderHandle mapping
    pub collider_map: HashMap<EntityId, ColliderHandle>,
    /// Track which entities we've already synced
    synced_entities: std::collections::HashSet<EntityId>,
}

impl Default for PhysicsSync {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicsSync {
    pub fn new() -> Self {
        Self {
            body_map: HashMap::new(),
            collider_map: HashMap::new(),
            synced_entities: std::collections::HashSet::new(),
        }
    }

    /// Push Flint entities with rigidbody/collider components into Rapier
    pub fn sync_to_rapier(&mut self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        // Find entities with rigidbody and/or collider components
        for entity in world.all_entities() {
            if self.synced_entities.contains(&entity.id) {
                continue;
            }

            let components = match world.get_components(entity.id) {
                Some(c) => c,
                None => continue,
            };

            // Need at least a rigidbody component to create a physics body
            let rb_data = match components.get("rigidbody") {
                Some(v) => v,
                None => continue,
            };

            // Read transform
            let transform = world.get_transform(entity.id).unwrap_or_default();

            // Build rigid body
            let body_type = rb_data
                .get("body_type")
                .and_then(|v| v.as_str())
                .unwrap_or("static");

            let builder = match body_type {
                "dynamic" => RigidBodyBuilder::dynamic(),
                "kinematic" | "kinematic_position" => RigidBodyBuilder::kinematic_position_based(),
                "kinematic_velocity" => RigidBodyBuilder::kinematic_velocity_based(),
                _ => RigidBodyBuilder::fixed(),
            };

            let mass = rb_data
                .get("mass")
                .and_then(|v| v.as_float())
                .unwrap_or(1.0) as f32;

            let linear_damping = rb_data
                .get("linear_damping")
                .and_then(|v| v.as_float())
                .unwrap_or(0.0) as f32;

            let angular_damping = rb_data
                .get("angular_damping")
                .and_then(|v| v.as_float())
                .unwrap_or(0.0) as f32;

            let gravity_scale = rb_data
                .get("gravity_scale")
                .and_then(|v| v.as_float())
                .unwrap_or(1.0) as f32;

            let body = builder
                .translation(vector![
                    transform.position.x,
                    transform.position.y,
                    transform.position.z
                ])
                .additional_mass(mass)
                .linear_damping(linear_damping)
                .angular_damping(angular_damping)
                .gravity_scale(gravity_scale)
                .build();

            let body_handle = physics.insert_rigid_body(body);
            self.body_map.insert(entity.id, body_handle);

            // Build collider if present
            if let Some(col_data) = components.get("collider") {
                let shape_str = col_data
                    .get("shape")
                    .and_then(|v| v.as_str())
                    .unwrap_or("box");

                let size = read_vec3_from_value(
                    col_data.get("size"),
                    Vec3::new(1.0, 1.0, 1.0),
                );

                let collider_shape: SharedShape = match shape_str {
                    "sphere" => SharedShape::ball(size.x * 0.5),
                    "capsule" => {
                        let radius = size.x * 0.5;
                        let half_height = size.y * 0.5 - radius;
                        SharedShape::capsule_y(half_height.max(0.01), radius)
                    }
                    _ => {
                        // "box" — half-extents
                        SharedShape::cuboid(size.x * 0.5, size.y * 0.5, size.z * 0.5)
                    }
                };

                let is_sensor = col_data
                    .get("is_sensor")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let friction = col_data
                    .get("friction")
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.5) as f32;

                let restitution = col_data
                    .get("restitution")
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.0) as f32;

                // Offset collider to match bounds center (for asymmetric bounds like doors)
                let bounds_offset = components
                    .get("bounds")
                    .and_then(|b| compute_bounds_center(b))
                    .unwrap_or([0.0, 0.0, 0.0]);

                let mut builder = ColliderBuilder::new(collider_shape)
                    .sensor(is_sensor)
                    .friction(friction)
                    .restitution(restitution);

                if bounds_offset[0].abs() > f32::EPSILON
                    || bounds_offset[1].abs() > f32::EPSILON
                    || bounds_offset[2].abs() > f32::EPSILON
                {
                    builder = builder.position(Isometry::translation(
                        bounds_offset[0],
                        bounds_offset[1],
                        bounds_offset[2],
                    ));
                }

                let collider = builder.build();

                let col_handle = physics.insert_collider_with_parent(collider, body_handle);
                self.collider_map.insert(entity.id, col_handle);
            }

            self.synced_entities.insert(entity.id);
        }
    }

    /// Write Rapier positions/rotations back to entity transforms
    pub fn sync_from_rapier(&self, world: &mut FlintWorld, physics: &PhysicsWorld) {
        for (entity_id, body_handle) in &self.body_map {
            let body = match physics.get_rigid_body(*body_handle) {
                Some(b) => b,
                None => continue,
            };

            // Only sync dynamic bodies back (static/kinematic are controlled by game logic)
            if !body.is_dynamic() {
                continue;
            }

            let pos = body.translation();
            let components = match world.get_components_mut(*entity_id) {
                Some(c) => c,
                None => continue,
            };

            // Update the transform component position
            let position_value = toml::Value::Array(vec![
                toml::Value::Float(pos.x as f64),
                toml::Value::Float(pos.y as f64),
                toml::Value::Float(pos.z as f64),
            ]);

            components.set_field("transform", "position", position_value);
        }
    }

    /// Update kinematic bodies from ECS transforms each frame.
    /// This lets animated entities (like doors) move their physics colliders.
    pub fn update_kinematic_bodies(&self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        for (entity_id, body_handle) in &self.body_map {
            let body = match physics.get_rigid_body(*body_handle) {
                Some(b) => b,
                None => continue,
            };

            // Only update kinematic bodies (not static or dynamic)
            if !body.is_kinematic() {
                continue;
            }

            // Skip entities driven by the character controller (player)
            if let Some(components) = world.get_components(*entity_id) {
                if components.has("character_controller") {
                    continue;
                }
            }

            let transform = world.get_transform(*entity_id).unwrap_or_default();

            // Convert Euler degrees to quaternion (ZYX order, matching renderer)
            let rotation_f32 = euler_to_quat(
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
            );

            // Compute bounds center offset (for asymmetric bounds like doors)
            let bounds_center = world
                .get_components(*entity_id)
                .and_then(|c| c.get("bounds"))
                .and_then(|b| compute_bounds_center(b))
                .unwrap_or([0.0, 0.0, 0.0]);

            // Rotate the bounds center by the entity's rotation
            let offset = rotation_f32 * na::Vector3::new(
                bounds_center[0],
                bounds_center[1],
                bounds_center[2],
            );

            let translation = na::Vector3::new(
                transform.position.x + offset.x,
                transform.position.y + offset.y,
                transform.position.z + offset.z,
            );

            let iso = Isometry::from_parts(
                translation.into(),
                rotation_f32,
            );

            // Get mutable body and set next kinematic position
            if let Some(body_mut) = physics.get_rigid_body_mut(*body_handle) {
                body_mut.set_next_kinematic_position(iso);
            }
        }
    }

    /// Get the rigid body handle for an entity
    pub fn get_body_handle(&self, entity_id: EntityId) -> Option<RigidBodyHandle> {
        self.body_map.get(&entity_id).copied()
    }

    /// Check if an entity has been synced to physics
    pub fn is_synced(&self, entity_id: EntityId) -> bool {
        self.synced_entities.contains(&entity_id)
    }
}

/// Compute the center offset of a bounds component
fn compute_bounds_center(bounds: &toml::Value) -> Option<[f32; 3]> {
    let min = bounds.get("min")?;
    let max = bounds.get("max")?;

    let min_arr = extract_toml_vec3(min)?;
    let max_arr = extract_toml_vec3(max)?;

    Some([
        (min_arr[0] + max_arr[0]) / 2.0,
        (min_arr[1] + max_arr[1]) / 2.0,
        (min_arr[2] + max_arr[2]) / 2.0,
    ])
}

/// Extract a [f32; 3] from a TOML array value
fn extract_toml_vec3(value: &toml::Value) -> Option<[f32; 3]> {
    let arr = value.as_array()?;
    if arr.len() >= 3 {
        let x = arr[0].as_float().or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
        let y = arr[1].as_float().or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
        let z = arr[2].as_float().or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
        Some([x, y, z])
    } else {
        None
    }
}

/// Helper to read a Vec3 from a TOML value (array or table)
fn read_vec3_from_value(value: Option<&toml::Value>, default: Vec3) -> Vec3 {
    let value = match value {
        Some(v) => v,
        None => return default,
    };

    if let Some(arr) = value.as_array() {
        if arr.len() >= 3 {
            let x = arr[0]
                .as_float()
                .or_else(|| arr[0].as_integer().map(|i| i as f64))
                .unwrap_or(default.x as f64) as f32;
            let y = arr[1]
                .as_float()
                .or_else(|| arr[1].as_integer().map(|i| i as f64))
                .unwrap_or(default.y as f64) as f32;
            let z = arr[2]
                .as_float()
                .or_else(|| arr[2].as_integer().map(|i| i as f64))
                .unwrap_or(default.z as f64) as f32;
            return Vec3::new(x, y, z);
        }
    }

    if let Some(table) = value.as_table() {
        let x = table
            .get("x")
            .and_then(|v| v.as_float())
            .unwrap_or(default.x as f64) as f32;
        let y = table
            .get("y")
            .and_then(|v| v.as_float())
            .unwrap_or(default.y as f64) as f32;
        let z = table
            .get("z")
            .and_then(|v| v.as_float())
            .unwrap_or(default.z as f64) as f32;
        return Vec3::new(x, y, z);
    }

    default
}

/// Convert Euler angles (degrees, ZYX order) to a unit quaternion matching the renderer convention
fn euler_to_quat(rx_deg: f32, ry_deg: f32, rz_deg: f32) -> na::UnitQuaternion<f32> {
    let qx = na::UnitQuaternion::from_axis_angle(&na::Vector3::x_axis(), rx_deg.to_radians());
    let qy = na::UnitQuaternion::from_axis_angle(&na::Vector3::y_axis(), ry_deg.to_radians());
    let qz = na::UnitQuaternion::from_axis_angle(&na::Vector3::z_axis(), rz_deg.to_radians());
    qx * qy * qz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_static_body() {
        let mut flint_world = FlintWorld::new();
        let id = flint_world.spawn("floor").unwrap();

        // Set transform
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
        flint_world.set_component(id, "transform", transform_data).unwrap();

        // Set rigidbody
        let rb_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("body_type".into(), toml::Value::String("static".into()));
            t
        });
        flint_world.set_component(id, "rigidbody", rb_data).unwrap();

        // Set collider
        let col_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("shape".into(), toml::Value::String("box".into()));
            t.insert(
                "size".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(20.0),
                    toml::Value::Float(0.2),
                    toml::Value::Float(20.0),
                ]),
            );
            t
        });
        flint_world.set_component(id, "collider", col_data).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();

        sync.sync_to_rapier(&flint_world, &mut physics);

        assert!(sync.body_map.contains_key(&id));
        assert!(sync.collider_map.contains_key(&id));
        assert_eq!(physics.rigid_body_set.len(), 1);
        assert_eq!(physics.collider_set.len(), 1);
    }

    #[test]
    fn test_sync_dynamic_body_falls() {
        let mut flint_world = FlintWorld::new();
        let id = flint_world.spawn("ball").unwrap();

        let transform_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(0.0),
                    toml::Value::Float(10.0),
                    toml::Value::Float(0.0),
                ]),
            );
            t
        });
        flint_world.set_component(id, "transform", transform_data).unwrap();

        let rb_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("body_type".into(), toml::Value::String("dynamic".into()));
            t
        });
        flint_world.set_component(id, "rigidbody", rb_data).unwrap();

        let col_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("shape".into(), toml::Value::String("sphere".into()));
            t.insert(
                "size".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(1.0),
                    toml::Value::Float(1.0),
                    toml::Value::Float(1.0),
                ]),
            );
            t
        });
        flint_world.set_component(id, "collider", col_data).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();

        sync.sync_to_rapier(&flint_world, &mut physics);

        // Step physics
        for _ in 0..60 {
            physics.step(1.0 / 60.0);
        }

        // Sync back
        sync.sync_from_rapier(&mut flint_world, &physics);

        // Check the transform was updated (ball should have fallen)
        let transform = flint_world.get_transform(id).unwrap();
        assert!(transform.position.y < 10.0);
    }
}
