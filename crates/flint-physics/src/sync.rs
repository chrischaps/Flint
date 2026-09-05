//! Synchronization between FlintWorld (TOML components) and Rapier physics
//!
//! Body frames are **world** rigid poses (ADR 0069): a rigidbody on a parented
//! entity is placed from `get_world_matrix`, and a dynamic body's simulated
//! pose is written back as a local transform against the parent's world
//! matrix. Scale is not part of the physics chain: every entity that carries a
//! rigidbody or joint, and each of its ancestors, is assumed to have unit scale
//! (a warning is logged once per entity when that is not the case).

use crate::world::PhysicsWorld;
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_core::{
    euler_deg_to_quat, mat4_scale, mat4_to_rigid, quat_rotate_vec3, rigid_inverse_apply, EntityId,
    Vec3,
};
use flint_ecs::{DynamicComponents, FlintWorld};
use rapier3d::na;
use rapier3d::prelude::*;
use std::collections::{HashMap, HashSet};

/// Bridges Flint's dynamic components with Rapier's rigid body and collider sets
#[derive(Default)]
pub struct PhysicsSync {
    /// EntityId -> RigidBodyHandle mapping
    pub(crate) body_map: HashMap<EntityId, RigidBodyHandle>,
    /// EntityId -> ColliderHandle mapping
    pub(crate) collider_map: HashMap<EntityId, ColliderHandle>,
    /// Track which entities we've already synced
    pub(crate) synced_entities: HashSet<EntityId>,
    /// Jointed entity -> its impulse joint (the entity is always `body2`)
    pub(crate) joint_map: HashMap<EntityId, ImpulseJointHandle>,
    /// Last motor/limit parameters pushed into Rapier, per jointed entity
    pub(crate) joint_cache: HashMap<EntityId, JointParams>,
    /// Entities already warned about (non-unit scale, unresolved joint parent)
    warned: HashSet<EntityId>,
}

impl PhysicsSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all sync state for a scene transition.
    pub fn clear(&mut self) {
        self.body_map.clear();
        self.collider_map.clear();
        self.synced_entities.clear();
        self.joint_map.clear();
        self.joint_cache.clear();
        self.warned.clear();
    }

    /// Push Flint entities with rigidbody/collider components into Rapier
    pub fn sync_to_rapier(&mut self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        // Find entities with rigidbody components
        for &entity_id in world.entities_with_component(comp::RIGIDBODY) {
            if self.synced_entities.contains(&entity_id) {
                continue;
            }

            let components = match world.get_components(entity_id) {
                Some(c) => c,
                None => continue,
            };

            // Need at least a rigidbody component to create a physics body
            let rb_data = match components.get(comp::RIGIDBODY) {
                Some(v) => v,
                None => continue,
            };

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

            let mass = rb_data.get("mass").and_then(toml_f32).unwrap_or(1.0);
            let linear_damping = rb_data
                .get("linear_damping")
                .and_then(toml_f32)
                .unwrap_or(0.0);
            let angular_damping = rb_data
                .get("angular_damping")
                .and_then(toml_f32)
                .unwrap_or(0.0);
            let gravity_scale = rb_data
                .get("gravity_scale")
                .and_then(toml_f32)
                .unwrap_or(1.0);
            let mode_2d = read_mode_2d(components);

            // World rigid pose (parent chain applied), sprite lift, 2D clamp
            let iso = self.body_world_pose(world, entity_id, components, mode_2d);

            // `additional_mass` sets mass only: a body with no collider would
            // keep zero angular inertia and could never be rotated, not by a
            // joint, not by a motor. Give collider-less bodies the inertia of
            // a 0.2 m solid sphere of that mass (I = 2/5 m r^2).
            let builder = if components.get(comp::COLLIDER).is_none() {
                let inertia = 0.4 * mass * 0.2 * 0.2;
                builder.additional_mass_properties(MassProperties::new(
                    point![0.0, 0.0, 0.0],
                    mass,
                    vector![inertia, inertia, inertia],
                ))
            } else {
                builder.additional_mass(mass)
            };
            let mut builder = builder
                .position(iso)
                .linear_damping(linear_damping)
                .angular_damping(angular_damping)
                .gravity_scale(gravity_scale);

            if mode_2d && body_type == "dynamic" {
                builder = builder.enabled_rotations(false, false, true);
            }

            let body = builder.build();

            let body_handle = physics.insert_rigid_body(body);
            self.body_map.insert(entity_id, body_handle);

            // Build collider if present
            if let Some(col_data) = components.get(comp::COLLIDER) {
                let shape_str = col_data
                    .get("shape")
                    .and_then(|v| v.as_str())
                    .unwrap_or("box");

                let size = read_vec3_from_value(col_data.get("size"), Vec3::new(1.0, 1.0, 1.0));

                let collider_shape: SharedShape = match shape_str {
                    "sphere" => SharedShape::ball(size.x * 0.5),
                    "capsule" => {
                        let radius = size.x * 0.5;
                        let half_height = size.y * 0.5 - radius;
                        SharedShape::capsule_y(half_height.max(0.01), radius)
                    }
                    // Y-axis cylinder: size = [diameter, height, _]. Use
                    // `collider.rotation` to lay it on its side for a wheel.
                    "cylinder" => SharedShape::cylinder((size.y * 0.5).max(0.01), size.x * 0.5),
                    "sprite" => {
                        // Auto-size from sprite component dimensions
                        let (sw, sh) = components
                            .get(comp::SPRITE)
                            .map(|s| {
                                let w = s.get("width").and_then(toml_f32).unwrap_or(1.0);
                                let h = s.get("height").and_then(toml_f32).unwrap_or(1.0);
                                (w, h)
                            })
                            .unwrap_or((1.0, 1.0));
                        SharedShape::cuboid(sw * 0.5, sh * 0.5, 0.1)
                    }
                    _ => {
                        // "box" — half-extents
                        let hz = if mode_2d { 0.1 } else { size.z * 0.5 };
                        SharedShape::cuboid(size.x * 0.5, size.y * 0.5, hz)
                    }
                };

                let is_sensor = col_data
                    .get("is_sensor")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let friction = col_data.get("friction").and_then(toml_f32).unwrap_or(0.5);
                let restitution = col_data
                    .get("restitution")
                    .and_then(toml_f32)
                    .unwrap_or(0.0);

                let mut builder = ColliderBuilder::new(collider_shape)
                    .sensor(is_sensor)
                    .friction(friction)
                    .restitution(restitution)
                    .active_events(ActiveEvents::COLLISION_EVENTS);

                if let Some(local) = collider_local_isometry(components, col_data) {
                    builder = builder.position(local);
                }

                let collider = builder.build();

                let col_handle = physics.insert_collider_with_parent(collider, body_handle);
                self.collider_map.insert(entity_id, col_handle);
            }

            self.synced_entities.insert(entity_id);
        }
    }

    /// Rigid world pose of an entity as Rapier sees it: the world matrix's
    /// translation and rotation, plus the sprite anchor lift and the 2D z clamp.
    fn body_world_pose(
        &mut self,
        world: &FlintWorld,
        entity_id: EntityId,
        components: &DynamicComponents,
        mode_2d: bool,
    ) -> Isometry<f32> {
        let mat = world
            .get_world_matrix(entity_id)
            .unwrap_or_else(|| flint_core::Transform::IDENTITY.to_matrix());
        let (pos, quat) = mat4_to_rigid(&mat);

        let scale = mat4_scale(&mat);
        if scale.iter().any(|s| (s - 1.0).abs() > 1e-3) && self.warned.insert(entity_id) {
            tracing::warn!(
                "physics: entity {:?} has non-unit world scale {:?}; bodies, colliders and joints assume unit scale on the physics chain",
                world.get_name(entity_id).unwrap_or("?"),
                scale
            );
        }

        let z = if mode_2d { 0.0 } else { pos.z };
        let translation = na::Vector3::new(pos.x, pos.y + sprite_anchor_offset_y(components), z);
        Isometry::from_parts(translation.into(), quat_to_na(quat))
    }

    /// Write Rapier poses back to entity transforms (dynamic bodies only).
    /// A parented body is written as a local pose against the parent's world matrix.
    pub fn sync_from_rapier(&self, world: &mut FlintWorld, physics: &PhysicsWorld) {
        // Read everything from the immutable world first, then write.
        let mut writes: Vec<(EntityId, Vec3, [f32; 4])> = Vec::new();

        for (entity_id, body_handle) in &self.body_map {
            let body = match physics.get_rigid_body(*body_handle) {
                Some(b) => b,
                None => continue,
            };

            // Only sync dynamic bodies back (static/kinematic are controlled by game logic)
            if !body.is_dynamic() {
                continue;
            }

            let components = match world.get_components(*entity_id) {
                Some(c) => c,
                None => continue,
            };

            let iso = body.position();
            let t = iso.translation.vector;
            // Reverse the sprite anchor lift applied when the body was placed
            let world_pos = Vec3::new(t.x, t.y - sprite_anchor_offset_y(components), t.z);
            let world_quat = na_to_quat(&iso.rotation);

            let (mut local_pos, local_quat) = match world
                .get_parent(*entity_id)
                .and_then(|p| world.get_world_matrix(p))
            {
                Some(parent_world) => rigid_inverse_apply(&parent_world, world_pos, world_quat),
                None => (world_pos, world_quat),
            };

            if read_mode_2d(components) {
                local_pos.z = 0.0;
            }

            writes.push((*entity_id, local_pos, local_quat));
        }

        for (entity_id, pos, quat) in writes {
            let Some(components) = world.get_components_mut(entity_id) else {
                continue;
            };
            components.set_field(
                comp::TRANSFORM,
                "position",
                toml::Value::Array(vec![
                    toml::Value::Float(pos.x as f64),
                    toml::Value::Float(pos.y as f64),
                    toml::Value::Float(pos.z as f64),
                ]),
            );
            components.set_field(
                comp::TRANSFORM,
                "rotation_quat",
                toml::Value::Array(quat.iter().map(|c| toml::Value::Float(*c as f64)).collect()),
            );
        }
    }

    /// Update kinematic bodies from ECS transforms each fixed step.
    /// This lets animated or scripted entities (doors, a vehicle root) move
    /// their colliders and drag any jointed dynamic children along.
    pub fn update_kinematic_bodies(&mut self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        let handles: Vec<(EntityId, RigidBodyHandle)> =
            self.body_map.iter().map(|(e, h)| (*e, *h)).collect();

        for (entity_id, body_handle) in handles {
            let body = match physics.get_rigid_body(body_handle) {
                Some(b) => b,
                None => continue,
            };

            // Only update kinematic bodies (not static or dynamic)
            if !body.is_kinematic() {
                continue;
            }

            let Some(components) = world.get_components(entity_id) else {
                continue;
            };

            // Skip entities driven by the character controller (player)
            if components.has(comp::CHARACTER_CONTROLLER) {
                continue;
            }

            let mode_2d = read_mode_2d(components);
            let iso = self.body_world_pose(world, entity_id, components, mode_2d);

            if let Some(body_mut) = physics.get_rigid_body_mut(body_handle) {
                body_mut.set_next_kinematic_position(iso);
            }
        }
    }

    /// Update sensor flags on already-synced colliders to match ECS state.
    /// This allows scripts to make colliders non-solid at runtime (e.g., dead enemies).
    pub fn update_sensor_flags(&self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        for (entity_id, col_handle) in &self.collider_map {
            let components = match world.get_components(*entity_id) {
                Some(c) => c,
                None => continue,
            };

            let col_data = match components.get(comp::COLLIDER) {
                Some(v) => v,
                None => continue,
            };

            let wants_sensor = col_data
                .get("is_sensor")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(collider) = physics.collider_set.get_mut(*col_handle) {
                if collider.is_sensor() != wants_sensor {
                    collider.set_sensor(wants_sensor);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Joints (ADR 0069)
    // ------------------------------------------------------------------

    /// Create Rapier impulse joints for entities carrying a `joint` component.
    ///
    /// The jointed entity is `body2`; `joint.parent` (an entity name) or, when
    /// empty, the transform parent is `body1`. Both must already have bodies;
    /// an entity whose bodies are not ready yet is retried on the next step.
    /// Anchors and axes are authored in the child's local frame and converted
    /// into each body's frame from the current world matrices.
    pub fn sync_joints(&mut self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        for &entity_id in world.entities_with_component(comp::JOINT) {
            if self.joint_map.contains_key(&entity_id) {
                continue;
            }
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(jd) = components.get(comp::JOINT) else {
                continue;
            };

            let kind = JointKind::parse(jd.get("type").and_then(|v| v.as_str()).unwrap_or("hinge"));

            let parent_name = jd.get("parent").and_then(|v| v.as_str()).unwrap_or("");
            let parent_id = if parent_name.is_empty() {
                world.get_parent(entity_id)
            } else {
                world.get_id(parent_name)
            };
            let Some(parent_id) = parent_id else {
                if self.warned.insert(entity_id) {
                    tracing::warn!(
                        "physics: joint on {:?} has no parent (set joint.parent or a transform parent)",
                        world.get_name(entity_id).unwrap_or("?")
                    );
                }
                continue;
            };

            let (Some(&parent_handle), Some(&child_handle)) =
                (self.body_map.get(&parent_id), self.body_map.get(&entity_id))
            else {
                // Bodies not created yet (e.g. a rigidbody added later); retry next step.
                continue;
            };

            let child_world = world
                .get_world_matrix(entity_id)
                .unwrap_or_else(|| flint_core::Transform::IDENTITY.to_matrix());
            let parent_world = world
                .get_world_matrix(parent_id)
                .unwrap_or_else(|| flint_core::Transform::IDENTITY.to_matrix());
            let (child_pos, child_quat) = mat4_to_rigid(&child_world);

            let anchor = jd.get("anchor").and_then(toml_vec3).unwrap_or([0.0; 3]);
            let axis = jd
                .get("axis")
                .and_then(toml_vec3)
                .unwrap_or([1.0, 0.0, 0.0]);

            // Child frame == child body frame (unit scale). Parent anchor/axis
            // come from the world-space anchor expressed in the parent's frame.
            let a_world = quat_rotate_vec3(&child_quat, anchor);
            let anchor_world = Vec3::new(
                child_pos.x + a_world[0],
                child_pos.y + a_world[1],
                child_pos.z + a_world[2],
            );
            let (anchor_parent, rel_quat) =
                rigid_inverse_apply(&parent_world, anchor_world, child_quat);
            let axis_child = normalize_or_x(axis);
            let axis_parent = normalize_or_x(quat_rotate_vec3(&rel_quat, axis_child));

            let a_c = point![anchor[0], anchor[1], anchor[2]];
            let a_p = point![anchor_parent.x, anchor_parent.y, anchor_parent.z];
            let ax_c = na::UnitVector3::new_normalize(na::Vector3::from(axis_child));
            let ax_p = na::UnitVector3::new_normalize(na::Vector3::from(axis_parent));

            let mut joint: GenericJoint = match kind {
                JointKind::Hinge => {
                    let mut g: GenericJoint = RevoluteJointBuilder::new(ax_c)
                        .local_anchor1(a_p)
                        .local_anchor2(a_c)
                        .into();
                    // `new(axis)` seeds both frames with the child axis; the
                    // parent's copy must be expressed in the parent's frame.
                    g.set_local_axis1(ax_p);
                    g
                }
                JointKind::Prismatic => PrismaticJointBuilder::new(ax_c)
                    .local_anchor1(a_p)
                    .local_anchor2(a_c)
                    .local_axis1(ax_p)
                    .local_axis2(ax_c)
                    .into(),
                JointKind::Spherical => SphericalJointBuilder::new()
                    .local_anchor1(a_p)
                    .local_anchor2(a_c)
                    .into(),
                JointKind::Fixed => FixedJointBuilder::new()
                    .local_frame1(Isometry::from_parts(
                        a_p.coords.into(),
                        quat_to_na(rel_quat),
                    ))
                    .local_frame2(Isometry::translation(a_c.x, a_c.y, a_c.z))
                    .into(),
            };

            let contacts = jd
                .get("contacts_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            joint.set_contacts_enabled(contacts);

            let params = read_joint_params(jd, kind);
            if kind != JointKind::Fixed {
                apply_joint_params(&mut joint, &params);
            }

            let handle = physics.insert_impulse_joint(parent_handle, child_handle, joint);
            // A jointed body must never sleep: Rapier does not wake a sleeping
            // dynamic body when the kinematic body it is jointed to moves, so a
            // scripted parent would drive off and leave the child behind.
            if let Some(body) = physics.get_rigid_body_mut(child_handle) {
                *body.activation_mut() = rapier3d::dynamics::RigidBodyActivation::cannot_sleep();
                body.wake_up(true);
            }
            self.joint_map.insert(entity_id, handle);
            self.joint_cache.insert(entity_id, params);
        }
    }

    /// Push changed motor targets / gains / limits from the `joint` component
    /// into the live Rapier joint. Cheap: one component read per jointed entity,
    /// and Rapier is only touched when something differs from the last push.
    pub fn update_joint_targets(&mut self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        for (entity_id, handle) in &self.joint_map {
            let Some(jd) = world
                .get_components(*entity_id)
                .and_then(|c| c.get(comp::JOINT))
            else {
                continue;
            };
            let kind = JointKind::parse(jd.get("type").and_then(|v| v.as_str()).unwrap_or("hinge"));
            if kind == JointKind::Fixed {
                continue;
            }
            let params = read_joint_params(jd, kind);
            if self.joint_cache.get(entity_id) == Some(&params) {
                continue;
            }
            if let Some(joint) = physics.get_impulse_joint_mut(*handle) {
                apply_joint_params(&mut joint.data, &params);
            }
            if let Some(body) = self
                .body_map
                .get(entity_id)
                .and_then(|h| physics.get_rigid_body_mut(*h))
            {
                body.wake_up(true);
            }
            self.joint_cache.insert(*entity_id, params);
        }
    }

    /// Forget every joint attached to `body_handle` (Rapier drops them itself
    /// when the body is removed; this keeps the maps from dangling).
    pub(crate) fn forget_joints_of_body(
        &mut self,
        body_handle: RigidBodyHandle,
        physics: &PhysicsWorld,
    ) {
        let stale: Vec<EntityId> = self
            .joint_map
            .iter()
            .filter(|(_, h)| {
                physics
                    .impulse_joint_set
                    .get(**h)
                    .map(|j| j.body1 == body_handle || j.body2 == body_handle)
                    .unwrap_or(true)
            })
            .map(|(e, _)| *e)
            .collect();
        for e in stale {
            self.joint_map.remove(&e);
            self.joint_cache.remove(&e);
        }
    }

    /// Re-seat jointed dynamic bodies on their kinematic parents after a step.
    ///
    /// Rapier solves a joint against the kinematic body's start-of-step pose
    /// and matches velocities along the tangent, so when a scripted parent
    /// moves on a curve (or turns under the child) the anchors separate a few
    /// millimetres a step and the locked rotations drift, and the joint never
    /// measures the error because the parent has already moved on. Over a turn
    /// that grows into a visible slip or a part lying on its side. After the
    /// step, rebuild the child's pose from the parent's joint frame: locked
    /// linear and angular axes are made exact, free axes keep the simulated
    /// coordinate (clamped to the joint's limits). Velocities are left alone
    /// so springs still overshoot and settle.
    pub fn project_kinematic_joints(&self, physics: &mut PhysicsWorld) {
        let mut updates: Vec<(RigidBodyHandle, Isometry<f32>)> = Vec::new();
        for &handle in self.joint_map.values() {
            let Some(joint) = physics.impulse_joint_set.get(handle) else {
                continue;
            };
            let (Some(b1), Some(b2)) = (
                physics.get_rigid_body(joint.body1),
                physics.get_rigid_body(joint.body2),
            ) else {
                continue;
            };
            if b1.is_dynamic() || !b2.is_dynamic() {
                continue;
            }
            let data = &joint.data;
            let locked = data.locked_axes;
            let f1 = b1.position() * data.local_frame1; // joint frame from the parent
            let f2 = b2.position() * data.local_frame2; // joint frame from the child

            // Linear: child anchor offset in the parent's joint frame.
            let mut o = f1.rotation.inverse() * (f2.translation.vector - f1.translation.vector);
            let lin = [JointAxis::LinX, JointAxis::LinY, JointAxis::LinZ];
            let lin_mask = [JointAxesMask::LIN_X, JointAxesMask::LIN_Y, JointAxesMask::LIN_Z];
            for i in 0..3 {
                if locked.contains(lin_mask[i]) {
                    o[i] = 0.0;
                } else if data.limit_axes.contains(lin_mask[i]) {
                    let l = data.limits[lin[i] as usize];
                    o[i] = o[i].clamp(l.min, l.max);
                }
            }

            // Angular: relative rotation of the child frame in the parent frame,
            // reduced to the twist about a single free axis, or identity when
            // all three are locked. Two or three free axes are left alone.
            let rel = f1.rotation.inverse() * f2.rotation;
            let ang_free = [
                !locked.contains(JointAxesMask::ANG_X),
                !locked.contains(JointAxesMask::ANG_Y),
                !locked.contains(JointAxesMask::ANG_Z),
            ];
            let new_rel = match ang_free.iter().filter(|f| **f).count() {
                0 => na::UnitQuaternion::identity(),
                1 => {
                    let q = rel.quaternion();
                    let (i, j, k) = if ang_free[0] {
                        (q.i, 0.0, 0.0)
                    } else if ang_free[1] {
                        (0.0, q.j, 0.0)
                    } else {
                        (0.0, 0.0, q.k)
                    };
                    let twist = na::Quaternion::new(q.w, i, j, k);
                    if twist.norm_squared() < 1e-12 {
                        na::UnitQuaternion::identity()
                    } else {
                        na::UnitQuaternion::from_quaternion(twist)
                    }
                }
                _ => rel,
            };

            let new_f2 = Isometry::from_parts(
                (f1.translation.vector + f1.rotation * o).into(),
                f1.rotation * new_rel,
            );
            let new_pose = new_f2 * data.local_frame2.inverse();
            let cur = b2.position();
            let moved = (new_pose.translation.vector - cur.translation.vector).norm_squared()
                > 1e-12
                || new_pose.rotation.angle_to(&cur.rotation) > 1e-5;
            if moved {
                updates.push((joint.body2, new_pose));
            }
        }
        for (handle, pose) in updates {
            if let Some(body) = physics.get_rigid_body_mut(handle) {
                body.set_position(pose, true);
            }
        }
    }

    /// Current joint coordinate of an entity's joint: hinge angle in degrees,
    /// prismatic displacement in metres, `None` for other kinds or no joint.
    pub fn joint_position(&self, entity_id: EntityId, physics: &PhysicsWorld) -> Option<f32> {
        let handle = *self.joint_map.get(&entity_id)?;
        let joint = physics.impulse_joint_set.get(handle)?;
        let b1 = physics.get_rigid_body(joint.body1)?;
        let b2 = physics.get_rigid_body(joint.body2)?;
        if let Some(rev) = joint.data.as_revolute() {
            return Some(rev.angle(b1.rotation(), b2.rotation()).to_degrees());
        }
        if joint.data.as_prismatic().is_some() {
            let anchor1 = b1.position() * joint.data.local_anchor1();
            let anchor2 = b2.position() * joint.data.local_anchor2();
            let axis1 = b1.position() * joint.data.local_axis1();
            return Some((anchor2 - anchor1).dot(&axis1));
        }
        None
    }

    /// Get the rigid body handle for an entity
    pub fn get_body_handle(&self, entity_id: EntityId) -> Option<RigidBodyHandle> {
        self.body_map.get(&entity_id).copied()
    }

    /// Get the impulse joint handle for a jointed entity
    pub fn get_joint_handle(&self, entity_id: EntityId) -> Option<ImpulseJointHandle> {
        self.joint_map.get(&entity_id).copied()
    }

    /// Check if an entity has been synced to physics
    pub fn is_synced(&self, entity_id: EntityId) -> bool {
        self.synced_entities.contains(&entity_id)
    }

    /// Register a trimesh collider from raw geometry and attach it to an existing rigid body.
    /// Used for procedural geometry like track surfaces that can't be approximated by primitives.
    pub fn register_trimesh(
        &mut self,
        entity_id: EntityId,
        physics: &mut PhysicsWorld,
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
        body_handle: RigidBodyHandle,
        friction: f32,
        restitution: f32,
    ) {
        let rapier_vertices: Vec<rapier3d::na::Point3<f32>> = vertices
            .into_iter()
            .map(|v| rapier3d::na::Point3::new(v[0], v[1], v[2]))
            .collect();

        let shape = SharedShape::trimesh(rapier_vertices, indices);
        let collider = ColliderBuilder::new(shape)
            .friction(friction)
            .restitution(restitution)
            .build();

        let col_handle = physics.insert_collider_with_parent(collider, body_handle);
        self.collider_map.insert(entity_id, col_handle);
        self.synced_entities.insert(entity_id);
    }

    /// Create a static rigid body for a trimesh entity and return its handle.
    /// Convenience method for registering static track geometry.
    pub fn register_static_trimesh(
        &mut self,
        entity_id: EntityId,
        physics: &mut PhysicsWorld,
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
        friction: f32,
        restitution: f32,
    ) {
        let body = RigidBodyBuilder::fixed().build();
        let body_handle = physics.insert_rigid_body(body);
        self.body_map.insert(entity_id, body_handle);

        self.register_trimesh(
            entity_id,
            physics,
            vertices,
            indices,
            body_handle,
            friction,
            restitution,
        );
    }
}

// ----------------------------------------------------------------------
// Joint parameter plumbing
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JointKind {
    Hinge,
    Prismatic,
    Spherical,
    Fixed,
}

impl JointKind {
    fn parse(s: &str) -> Self {
        match s {
            "prismatic" | "slider" => JointKind::Prismatic,
            "spherical" | "ball" => JointKind::Spherical,
            "fixed" => JointKind::Fixed,
            _ => JointKind::Hinge,
        }
    }

    /// Whether the joint coordinate is an angle (authored in degrees).
    fn angular(self) -> bool {
        matches!(self, JointKind::Hinge | JointKind::Spherical)
    }
}

/// Motor and limit settings as last pushed into Rapier (radians / metres).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct JointParams {
    axis: JointAxis,
    target: f32,
    stiffness: f32,
    damping: f32,
    max_force: f32,
    model: MotorModel,
    limits: Option<[f32; 2]>,
}

fn read_joint_params(jd: &toml::Value, kind: JointKind) -> JointParams {
    let axis = match kind {
        JointKind::Prismatic => JointAxis::LinX,
        JointKind::Spherical => match jd.get("motor_axis").and_then(|v| v.as_str()) {
            Some("y") => JointAxis::AngY,
            Some("z") => JointAxis::AngZ,
            _ => JointAxis::AngX,
        },
        _ => JointAxis::AngX,
    };
    let unit = if kind.angular() {
        std::f32::consts::PI / 180.0
    } else {
        1.0
    };
    let target = jd.get("motor_target").and_then(toml_f32).unwrap_or(0.0) * unit;
    let stiffness = jd
        .get("motor_stiffness")
        .and_then(toml_f32)
        .unwrap_or(0.0)
        .max(0.0);
    let damping = jd
        .get("motor_damping")
        .and_then(toml_f32)
        .unwrap_or(0.0)
        .max(0.0);
    let max_force = jd
        .get("motor_max_force")
        .and_then(toml_f32)
        .unwrap_or(0.0)
        .max(0.0);
    let model = match jd.get("motor_model").and_then(|v| v.as_str()) {
        Some("force") => MotorModel::ForceBased,
        _ => MotorModel::AccelerationBased,
    };
    let limits = jd
        .get("limits")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() >= 2)
        .and_then(|a| Some([toml_f32(&a[0])? * unit, toml_f32(&a[1])? * unit]))
        .filter(|l| l[0] < l[1]);
    JointParams {
        axis,
        target,
        stiffness,
        damping,
        max_force,
        model,
        limits,
    }
}

fn apply_joint_params(joint: &mut GenericJoint, p: &JointParams) {
    if p.stiffness > 0.0 || p.damping > 0.0 {
        joint.set_motor_position(p.axis, p.target, p.stiffness, p.damping);
        joint.set_motor_model(p.axis, p.model);
        joint.set_motor_max_force(
            p.axis,
            if p.max_force > 0.0 {
                p.max_force
            } else {
                f32::MAX
            },
        );
    } else {
        joint.motor_axes.remove(p.axis.into());
    }
    match p.limits {
        Some(l) => {
            joint.set_limits(p.axis, l);
        }
        None => joint.limit_axes.remove(p.axis.into()),
    }
}

// ----------------------------------------------------------------------
// Small conversions
// ----------------------------------------------------------------------

fn quat_to_na(q: [f32; 4]) -> na::UnitQuaternion<f32> {
    na::UnitQuaternion::from_quaternion(na::Quaternion::new(q[3], q[0], q[1], q[2]))
}

fn na_to_quat(q: &na::UnitQuaternion<f32>) -> [f32; 4] {
    [q.i, q.j, q.k, q.w]
}

fn normalize_or_x(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        [1.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn read_mode_2d(components: &DynamicComponents) -> bool {
    components
        .get(comp::RIGIDBODY)
        .and_then(|rb| rb.get("mode_2d"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Lift so the body centre sits at the sprite's visual centre: with
/// `anchor_y = 0` (bottom-anchored) the centre is `height / 2` above the
/// entity position. `sync_from_rapier` reverses it.
fn sprite_anchor_offset_y(components: &DynamicComponents) -> f32 {
    components
        .get(comp::SPRITE)
        .map(|s| {
            let anchor_y = s.get("anchor_y").and_then(toml_f32).unwrap_or(0.0);
            let height = s.get("height").and_then(toml_f32).unwrap_or(1.0);
            (0.5 - anchor_y) * height
        })
        .unwrap_or(0.0)
}

/// Collider pose relative to the body: bounds centre + `collider.offset`,
/// rotated by `collider.rotation` (Euler degrees). `None` when identity.
fn collider_local_isometry(
    components: &DynamicComponents,
    col_data: &toml::Value,
) -> Option<Isometry<f32>> {
    let bounds_center = components
        .get(comp::BOUNDS)
        .and_then(compute_bounds_center)
        .unwrap_or([0.0, 0.0, 0.0]);
    let offset = col_data
        .get("offset")
        .and_then(toml_vec3)
        .unwrap_or([0.0; 3]);
    let rotation = col_data
        .get("rotation")
        .and_then(toml_vec3)
        .unwrap_or([0.0; 3]);

    let t = [
        bounds_center[0] + offset[0],
        bounds_center[1] + offset[1],
        bounds_center[2] + offset[2],
    ];
    let identity = t.iter().all(|c| c.abs() <= f32::EPSILON)
        && rotation.iter().all(|c| c.abs() <= f32::EPSILON);
    if identity {
        return None;
    }
    let q = quat_to_na(euler_deg_to_quat(rotation[0], rotation[1], rotation[2]));
    Some(Isometry::from_parts(
        na::Vector3::new(t[0], t[1], t[2]).into(),
        q,
    ))
}

/// Compute the center offset of a bounds component
fn compute_bounds_center(bounds: &toml::Value) -> Option<[f32; 3]> {
    let min = toml_vec3(bounds.get("min")?)?;
    let max = toml_vec3(bounds.get("max")?)?;
    Some([
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ])
}

/// Helper to read a Vec3 from a TOML value (array or table)
fn read_vec3_from_value(value: Option<&toml::Value>, default: Vec3) -> Vec3 {
    let value = match value {
        Some(v) => v,
        None => return default,
    };

    if let Some(arr) = toml_vec3(value) {
        return Vec3::new(arr[0], arr[1], arr[2]);
    }

    if let Some(table) = value.as_table() {
        let x = table.get("x").and_then(toml_f32).unwrap_or(default.x);
        let y = table.get("y").and_then(toml_f32).unwrap_or(default.y);
        let z = table.get("z").and_then(toml_f32).unwrap_or(default.z);
        return Vec3::new(x, y, z);
    }

    default
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec3_value(v: [f64; 3]) -> toml::Value {
        toml::Value::Array(v.iter().map(|c| toml::Value::Float(*c)).collect())
    }

    fn set_transform(world: &mut FlintWorld, id: EntityId, pos: [f64; 3]) {
        let mut t = toml::map::Map::new();
        t.insert("position".into(), vec3_value(pos));
        world
            .set_component(id, "transform", toml::Value::Table(t))
            .unwrap();
    }

    fn set_body(world: &mut FlintWorld, id: EntityId, body_type: &str) {
        let mut t = toml::map::Map::new();
        t.insert("body_type".into(), toml::Value::String(body_type.into()));
        world
            .set_component(id, "rigidbody", toml::Value::Table(t))
            .unwrap();
    }

    fn set_collider(world: &mut FlintWorld, id: EntityId, shape: &str, size: [f64; 3]) {
        let mut t = toml::map::Map::new();
        t.insert("shape".into(), toml::Value::String(shape.into()));
        t.insert("size".into(), vec3_value(size));
        world
            .set_component(id, "collider", toml::Value::Table(t))
            .unwrap();
    }

    #[test]
    fn test_sync_static_body() {
        let mut flint_world = FlintWorld::new();
        let id = flint_world.spawn("floor").unwrap();
        set_transform(&mut flint_world, id, [0.0, 0.0, 0.0]);
        set_body(&mut flint_world, id, "static");
        set_collider(&mut flint_world, id, "box", [20.0, 0.2, 20.0]);

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
        set_transform(&mut flint_world, id, [0.0, 10.0, 0.0]);
        set_body(&mut flint_world, id, "dynamic");
        set_collider(&mut flint_world, id, "sphere", [1.0, 1.0, 1.0]);

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
        assert!(
            transform.rotation_quat.is_some(),
            "rotation is written back"
        );
    }

    #[test]
    fn child_of_parent_body_simulates_in_world_space() {
        let mut world = FlintWorld::new();
        let parent = world.spawn("rig").unwrap();
        set_transform(&mut world, parent, [10.0, 0.0, 0.0]);
        let child = world.spawn("bob").unwrap();
        set_transform(&mut world, child, [0.0, 5.0, 0.0]);
        set_body(&mut world, child, "dynamic");
        set_collider(&mut world, child, "sphere", [1.0, 1.0, 1.0]);
        world.set_parent(child, parent).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();
        sync.sync_to_rapier(&world, &mut physics);

        let body = physics.get_rigid_body(sync.body_map[&child]).unwrap();
        let t = body.translation();
        assert!(
            (t.x - 10.0).abs() < 1e-5 && (t.y - 5.0).abs() < 1e-5,
            "{t:?}"
        );

        for _ in 0..30 {
            physics.step(1.0 / 60.0);
        }
        sync.sync_from_rapier(&mut world, &physics);

        let local = world.get_transform(child).unwrap();
        assert!(local.position.x.abs() < 1e-4, "local x stays 0: {local:?}");
        assert!(local.position.y < 5.0, "local y falls: {local:?}");
        let wp = world.get_world_position(child).unwrap();
        assert!(
            (wp.x - 10.0).abs() < 1e-4,
            "world x keeps parent offset: {wp:?}"
        );
    }

    #[test]
    fn kinematic_body_honours_rotation_quat_and_parent() {
        let mut world = FlintWorld::new();
        let parent = world.spawn("root").unwrap();
        let mut t = toml::map::Map::new();
        t.insert("position".into(), vec3_value([0.0, 0.0, 0.0]));
        t.insert("rotation".into(), vec3_value([0.0, 90.0, 0.0]));
        world
            .set_component(parent, "transform", toml::Value::Table(t))
            .unwrap();

        let child = world.spawn("arm").unwrap();
        let q = flint_core::euler_deg_to_quat(0.0, 0.0, 30.0);
        let mut t = toml::map::Map::new();
        t.insert("position".into(), vec3_value([2.0, 0.0, 0.0]));
        t.insert(
            "rotation_quat".into(),
            toml::Value::Array(q.iter().map(|c| toml::Value::Float(*c as f64)).collect()),
        );
        world
            .set_component(child, "transform", toml::Value::Table(t))
            .unwrap();
        set_body(&mut world, child, "kinematic");
        set_collider(&mut world, child, "box", [1.0, 1.0, 1.0]);
        world.set_parent(child, parent).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();
        sync.sync_to_rapier(&world, &mut physics);
        sync.update_kinematic_bodies(&world, &mut physics);
        physics.step(1.0 / 60.0);

        let body = physics.get_rigid_body(sync.body_map[&child]).unwrap();
        let t = body.translation();
        // Parent yaw 90° sends local +X to world -Z
        assert!(t.x.abs() < 1e-4 && (t.z + 2.0).abs() < 1e-4, "{t:?}");
        let expected = flint_core::quat_mul(&flint_core::euler_deg_to_quat(0.0, 90.0, 0.0), &q);
        let got = na_to_quat(body.rotation());
        let dot: f32 = (0..4).map(|i| expected[i] * got[i]).sum();
        assert!(dot.abs() > 0.9999, "rotation {got:?} vs {expected:?}");
    }

    #[test]
    fn collider_rotation_offset_lays_cylinder_on_its_side() {
        let mut world = FlintWorld::new();
        let id = world.spawn("wheel").unwrap();
        set_transform(&mut world, id, [0.0, 1.0, 0.0]);
        set_body(&mut world, id, "static");
        let mut t = toml::map::Map::new();
        t.insert("shape".into(), toml::Value::String("cylinder".into()));
        t.insert("size".into(), vec3_value([1.0, 0.4, 0.0]));
        t.insert("rotation".into(), vec3_value([0.0, 0.0, 90.0]));
        t.insert("offset".into(), vec3_value([0.0, 0.5, 0.0]));
        world
            .set_component(id, "collider", toml::Value::Table(t))
            .unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();
        sync.sync_to_rapier(&world, &mut physics);
        let col = physics.collider_set.get(sync.collider_map[&id]).unwrap();
        assert!(col.shape().as_cylinder().is_some());
        let pos = col.position();
        assert!((pos.translation.y - 1.5).abs() < 1e-5);
        // The cylinder's Y axis now points along world X
        let axis = pos.rotation * na::Vector3::y();
        assert!(axis.x.abs() > 0.999, "{axis:?}");
    }
}
