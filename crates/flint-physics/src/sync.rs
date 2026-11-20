//! Synchronization between FlintWorld (TOML components) and Rapier physics
//!
//! Body frames are **world** rigid poses (ADR 0069): a rigidbody on a parented
//! entity is placed from `get_world_matrix`, and a dynamic body's simulated
//! pose is written back as a local transform against the parent's world
//! matrix. A body is rigid, so scale is carried by its collider (#729): the
//! entity's world scale multiplies the collider's size, offset and the
//! bounds centre (a joint's anchor stays metres, unscaled, with a warning
//! for a scaled jointed body). A uniform scale is exact for every
//! shape; a non-uniform one is exact for an unrotated box, a Y-axis capsule
//! or cylinder scaled alike in x and z, and a `mesh` or `convex`, and
//! approximated (with a warning, once) for the rest.

use crate::layers::{CollisionLayers, LayerMask, DEFAULT_LAYER};
use crate::world::PhysicsWorld;
use crate::ColliderMesh;
use flint_core::components as comp;
use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::toml_util::toml_f32;
use flint_core::{
    euler_deg_to_quat, mat4_scale, mat4_to_rigid, quat_rotate_vec3, rigid_inverse_apply, EntityId,
    Transform, Vec3,
};
use flint_ecs::{ComponentView, FlintWorld};
use rapier3d::na;
use rapier3d::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What `rigidbody.body_type` may be. `kinematic` is position-based.
pub const BODY_TYPES: &[&str] = &[
    "static",
    "dynamic",
    "kinematic",
    "kinematic_position",
    "kinematic_velocity",
];

/// What `collider.shape` may be.
pub const COLLIDER_SHAPES: &[&str] = &[
    "box", "sphere", "capsule", "cylinder", "sprite", "mesh", "convex",
];

/// What a scaled mesh shape is cached under: the triangles it was made
/// from (by address), whether it is a hull, and the bits of the scale,
/// offset and rotation baked into its points.
type MeshShapeKey = (usize, bool, [u32; 9]);

/// What `joint.type` may be (`slider` is prismatic, `ball` spherical).
pub const JOINT_TYPES: &[&str] = &["hinge", "prismatic", "slider", "spherical", "ball", "fixed"];

/// Bridges Flint's dynamic components with Rapier's rigid body and collider sets
#[derive(Default)]
pub struct PhysicsSync {
    /// EntityId -> RigidBodyHandle mapping
    pub(crate) body_map: HashMap<EntityId, RigidBodyHandle>,
    /// EntityId -> ColliderHandle mapping
    pub(crate) collider_map: HashMap<EntityId, ColliderHandle>,
    /// ColliderHandle -> EntityId, the reverse of `collider_map`, so a
    /// contact or a ray hit finds its entity without a scan (#729)
    pub(crate) collider_entity: HashMap<ColliderHandle, EntityId>,
    /// The layer names `collider.layer` and `collider.mask` are read
    /// against (the project's, `[project.physics] layers`). Kept across
    /// scene transitions.
    pub(crate) layers: CollisionLayers,
    /// Triangles a host handed over for `mesh` and `convex` colliders, by
    /// entity (the player reads them from each entity's model)
    pub(crate) meshes: HashMap<EntityId, Arc<ColliderMesh>>,
    /// Shapes made from those triangles, so a hundred crates of one model
    /// and one scale share one hull
    mesh_shapes: HashMap<MeshShapeKey, Option<SharedShape>>,
    /// Entities already warned about a shape a scale or a body type changed
    shape_warned: HashSet<EntityId>,
    /// Jointed entities already warned that their anchor ignores their scale
    joint_scale_warned: HashSet<EntityId>,
    /// Trimeshes registered by a host (terrain, splines) whose entity's
    /// `collider` layer is read at the next sync, when the world is at hand
    trimesh_pending: Vec<EntityId>,
    /// Track which entities we've already synced
    pub(crate) synced_entities: HashSet<EntityId>,
    /// Jointed entity -> its impulse joint (the entity is always `body2`)
    pub(crate) joint_map: HashMap<EntityId, ImpulseJointHandle>,
    /// Last motor/limit parameters pushed into Rapier, per jointed entity
    pub(crate) joint_cache: HashMap<EntityId, JointParams>,
    /// Entities already warned about (non-unit scale, unresolved joint parent)
    warned: HashSet<EntityId>,
    /// Jointed bodies already reported for a non-finite pose (one warning each).
    nan_warned: HashSet<RigidBodyHandle>,
    /// Entities seen with a `collider` and no `rigidbody` on the last pass.
    /// One still alone on the next pass is reported (`lone_reported`): a
    /// script that adds the two a step apart is not told off in between.
    lone_colliders: HashSet<EntityId>,
    lone_reported: HashSet<EntityId>,
    /// Start-of-step pose of every kinematic body, recorded when its next
    /// pose is set; gives the parent's exact step velocity for the projection
    kin_start: HashMap<RigidBodyHandle, Isometry<f32>>,
    /// Pose of every kinematic body at the first fixed step of the current
    /// frame; the scripted target is reached by interpolating from here across
    /// the frame's steps (see `update_kinematic_bodies`)
    kin_frame_start: HashMap<RigidBodyHandle, Isometry<f32>>,
}

impl PhysicsSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all sync state for a scene transition. The layers are the
    /// project's and stay.
    pub fn clear(&mut self) {
        self.body_map.clear();
        self.collider_map.clear();
        self.collider_entity.clear();
        self.meshes.clear();
        self.mesh_shapes.clear();
        self.shape_warned.clear();
        self.joint_scale_warned.clear();
        self.trimesh_pending.clear();
        self.synced_entities.clear();
        self.joint_map.clear();
        self.joint_cache.clear();
        self.warned.clear();
        self.nan_warned.clear();
        self.lone_colliders.clear();
        self.lone_reported.clear();
        self.kin_start.clear();
        self.kin_frame_start.clear();
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
            let rb = world.view(entity_id, comp::RIGIDBODY);
            if !rb.exists() {
                continue;
            }

            // Build rigid body. An unknown `body_type` is reported and the
            // body is static, the default: it used to be static in silence.
            let body_type = rb.choice("body_type", BODY_TYPES, "static");

            let builder = match body_type {
                "dynamic" => RigidBodyBuilder::dynamic(),
                "kinematic" | "kinematic_position" => RigidBodyBuilder::kinematic_position_based(),
                "kinematic_velocity" => RigidBodyBuilder::kinematic_velocity_based(),
                _ => RigidBodyBuilder::fixed(),
            };

            let mass = rb.f32("mass", 1.0);
            let linear_damping = rb.f32("linear_damping", 0.0);
            let angular_damping = rb.f32("angular_damping", 0.0);
            let gravity_scale = rb.f32("gravity_scale", 1.0);
            let mode_2d = rb.bool("mode_2d", false);

            // World rigid pose (parent chain applied), sprite lift, 2D clamp
            let iso = self.body_world_pose(world, entity_id, mode_2d);

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
            let col = world.view(entity_id, comp::COLLIDER);
            if col.exists() {
                let scale = world_scale(world, entity_id);
                let (collider_shape, local) =
                    self.collider_shape(world, entity_id, &col, scale, mode_2d, body_type);

                let is_sensor = col.bool("is_sensor", false);
                let friction = col.f32("friction", 0.5);
                let restitution = col.f32("restitution", 0.0);

                let mut builder = ColliderBuilder::new(collider_shape)
                    .sensor(is_sensor)
                    .friction(friction)
                    .restitution(restitution)
                    .collision_groups(self.groups_of(&col))
                    .active_events(ActiveEvents::COLLISION_EVENTS);

                if let Some(local) = local {
                    builder = builder.position(local);
                }

                let collider = builder.build();

                let col_handle = physics.insert_collider_with_parent(collider, body_handle);
                self.insert_collider(entity_id, col_handle);
            }

            self.synced_entities.insert(entity_id);
        }

        // A host's trimesh (terrain, a spline) takes its entity's
        // `collider.layer` and `mask` too, when it has a collider table
        for entity_id in std::mem::take(&mut self.trimesh_pending) {
            let col = world.view(entity_id, comp::COLLIDER);
            if !col.exists() {
                continue;
            }
            let groups = self.groups_of(&col);
            if let Some(c) = self
                .collider_map
                .get(&entity_id)
                .and_then(|h| physics.collider_set.get_mut(*h))
            {
                c.set_collision_groups(groups);
            }
        }

        self.report_lone_colliders(world);
    }

    /// Remember an entity's collider both ways.
    pub(crate) fn insert_collider(&mut self, entity_id: EntityId, handle: ColliderHandle) {
        if let Some(old) = self.collider_map.insert(entity_id, handle) {
            self.collider_entity.remove(&old);
        }
        self.collider_entity.insert(handle, entity_id);
    }

    /// Forget an entity's collider both ways, returning its handle.
    pub(crate) fn remove_collider(&mut self, entity_id: EntityId) -> Option<ColliderHandle> {
        let handle = self.collider_map.remove(&entity_id)?;
        self.collider_entity.remove(&handle);
        Some(handle)
    }

    /// The entity a collider belongs to.
    pub(crate) fn entity_of(&self, handle: ColliderHandle) -> Option<EntityId> {
        self.collider_entity.get(&handle).copied()
    }

    /// The layer a collider is on and the layers it meets, from
    /// `collider.layer` (one name, `default` when unsaid) and
    /// `collider.mask` (a list of names, every layer when unsaid). A name
    /// that is no layer is reported once and left out (a mask) or read as
    /// `default` (the layer).
    pub(crate) fn groups_of(&self, col: &ComponentView) -> InteractionGroups {
        let names: Vec<&str> = self.layers.names().collect();
        let layer = col.choice("layer", &names, DEFAULT_LAYER);
        let membership = self.layers.bit(layer).unwrap_or(LayerMask::from_bits(1));
        InteractionGroups::new(membership.group(), self.mask_of(col).group())
    }

    fn mask_of(&self, col: &ComponentView) -> LayerMask {
        let Some(value) = col.raw("mask") else {
            return LayerMask::ALL;
        };
        let Some(items) = value.as_array() else {
            col.report("mask", || {
                Diagnostic::error(
                    codes::BAD_TYPE,
                    format!("expected a list of layer names, got {}", value.type_str()),
                )
                .hint("mask = [\"default\", \"player\"]; it meets every layer")
            });
            return LayerMask::ALL;
        };
        let mut bits = 0;
        for (i, item) in items.iter().enumerate() {
            let field = format!("mask[{i}]");
            match item.as_str() {
                Some(name) => match self.layers.bit(name) {
                    Some(bit) => bits |= bit.bits(),
                    None => col.report(&field, || {
                        Diagnostic::error(
                            codes::BAD_ENUM_VALUE,
                            format!("\"{name}\" is not a collision layer"),
                        )
                        .hint(format!("{}; it is left out", self.layers.unknown(name)))
                    }),
                },
                None => col.report(&field, || {
                    Diagnostic::error(
                        codes::BAD_TYPE,
                        format!("expected a layer name, got {}", item.type_str()),
                    )
                }),
            }
        }
        LayerMask::from_bits(bits)
    }

    /// A collider's shape at the entity's world `scale`, and its pose on
    /// the body (bounds centre plus `collider.offset`, scaled, turned by
    /// `collider.rotation`; `None` when identity). `mesh` and `convex` bake
    /// the pose and the scale into their points, so they have none.
    fn collider_shape(
        &mut self,
        world: &FlintWorld,
        entity_id: EntityId,
        col: &ComponentView,
        scale: [f32; 3],
        mode_2d: bool,
        body_type: &str,
    ) -> (SharedShape, Option<Isometry<f32>>) {
        let shape_str = col.choice("shape", COLLIDER_SHAPES, "box");
        if matches!(shape_str, "mesh" | "convex") {
            if let Some(shape) = self.mesh_shape(entity_id, col, shape_str, scale, body_type) {
                return (shape, None);
            }
            // No triangles (reported): a box of `size`, as before #729
        }

        let [sx, sy, sz] = col.vec3("size", [1.0, 1.0, 1.0]);
        let [kx, ky, kz] = scale;
        let rotation = col.vec3("rotation", [0.0; 3]);
        let unrotated = rotation.iter().all(|c| c.abs() <= f32::EPSILON);
        let uniform = is_uniform(scale);
        let round = kx.max(kz);
        // Whether the shape takes the scale exactly; an approximation is
        // warned about once below.
        let (shape, exact, took): (SharedShape, bool, &str) = match shape_str {
            "sphere" => {
                let r = sx * 0.5 * kx.max(ky).max(kz);
                (SharedShape::ball(r), uniform, "the largest axis")
            }
            "capsule" => {
                let radius = sx * 0.5 * round;
                let half_height = sy * 0.5 * ky - radius;
                (
                    SharedShape::capsule_y(half_height.max(0.01), radius),
                    uniform || (unrotated && close(kx, kz)),
                    "the larger of x and z for its radius",
                )
            }
            // Y-axis cylinder: size = [diameter, height, _]. Use
            // `collider.rotation` to lay it on its side for a wheel.
            "cylinder" => (
                SharedShape::cylinder((sy * 0.5 * ky).max(0.01), sx * 0.5 * round),
                uniform || (unrotated && close(kx, kz)),
                "the larger of x and z for its radius",
            ),
            "sprite" => {
                // Auto-size from sprite component dimensions
                let sprite = world.view(entity_id, comp::SPRITE);
                let sw = sprite.f32("width", 1.0);
                let sh = sprite.f32("height", 1.0);
                (
                    SharedShape::cuboid(sw * 0.5 * kx, sh * 0.5 * ky, 0.1),
                    uniform || unrotated,
                    "its axes as they lie before `collider.rotation`",
                )
            }
            _ => {
                // "box" — half-extents
                let hz = if mode_2d { 0.1 } else { sz * 0.5 * kz };
                (
                    SharedShape::cuboid(sx * 0.5 * kx, sy * 0.5 * ky, hz),
                    uniform || unrotated,
                    "its axes as they lie before `collider.rotation`",
                )
            }
        };
        if !exact && self.shape_warned.insert(entity_id) {
            tracing::warn!(
                "physics: entity {:?} has non-uniform world scale [{:.3}, {:.3}, {:.3}]; its {shape_str} collider can't stretch that way and is scaled by {took}. An unrotated box, or a `mesh` or `convex`, stretches exactly",
                world.get_name(entity_id).unwrap_or("?"),
                kx,
                ky,
                kz
            );
        }

        let local = collider_local_isometry(&world.view(entity_id, comp::BOUNDS), col, scale);
        (shape, local)
    }

    /// A `mesh` or `convex` shape from the triangles handed over for the
    /// entity, its points turned by `collider.rotation`, moved by
    /// `collider.offset` and stretched by the world scale. `None` (and a
    /// warning, once) when there are none or they make no shape. A
    /// dynamic body's `mesh` is made `convex`: Rapier makes no contacts
    /// between two triangle meshes, so it would fall through a mesh floor.
    fn mesh_shape(
        &mut self,
        entity_id: EntityId,
        col: &ComponentView,
        kind: &str,
        scale: [f32; 3],
        body_type: &str,
    ) -> Option<SharedShape> {
        let Some(mesh) = self.meshes.get(&entity_id).cloned() else {
            col.report("shape", || {
                Diagnostic::warning(
                    codes::NEEDS_COMPONENT,
                    format!("a `{kind}` collider has no triangles, so it is a box of `size`"),
                )
                .hint(
                    "it takes them from `collider.model`, else the entity's `model.asset`: \
                     name a model whose file is found (a model made at runtime has none)",
                )
            });
            return None;
        };
        let convex = kind == "convex" || body_type == "dynamic";
        if kind == "mesh" && convex {
            col.report("shape", || {
                Diagnostic::warning(
                    codes::NEEDS_COMPONENT,
                    "a dynamic body's `mesh` collider is made `convex`",
                )
                .hint(
                    "two triangle meshes never touch in Rapier, so a dynamic mesh would fall \
                     through a mesh floor; say `convex`, or build it from boxes",
                )
            });
        }
        let offset = col.vec3("offset", [0.0; 3]);
        let rotation = col.vec3("rotation", [0.0; 3]);
        let mut bits = [0u32; 9];
        for (i, v) in scale.iter().chain(&offset).chain(&rotation).enumerate() {
            bits[i] = v.to_bits();
        }
        let key = (Arc::as_ptr(&mesh) as usize, convex, bits);
        if let Some(shape) = self.mesh_shapes.get(&key) {
            return shape.clone();
        }

        let q = euler_deg_to_quat(rotation[0], rotation[1], rotation[2]);
        let points: Vec<Point<f32>> = mesh
            .positions
            .iter()
            .map(|p| {
                let r = quat_rotate_vec3(&q, *p);
                point![
                    (r[0] + offset[0]) * scale[0],
                    (r[1] + offset[1]) * scale[1],
                    (r[2] + offset[2]) * scale[2]
                ]
            })
            .collect();
        let shape = if convex {
            if points.len() < 4 {
                None
            } else {
                SharedShape::convex_hull(&points)
            }
        } else {
            let n = points.len() as u32;
            let triangles: Vec<[u32; 3]> = mesh
                .triangles
                .iter()
                .copied()
                .filter(|t| {
                    t.iter().all(|i| *i < n) && t[0] != t[1] && t[1] != t[2] && t[0] != t[2]
                })
                .collect();
            (!triangles.is_empty()).then(|| SharedShape::trimesh(points, triangles))
        };
        if shape.is_none() {
            col.report("shape", || {
                Diagnostic::warning(
                    codes::NEEDS_COMPONENT,
                    format!(
                        "the model's {} points and {} triangles make no `{}` shape, so it is a box of `size`",
                        mesh.positions.len(),
                        mesh.triangles.len(),
                        if convex { "convex" } else { "mesh" }
                    ),
                )
                .hint("a hull needs four points that are not on one plane")
            });
        }
        self.mesh_shapes.insert(key, shape.clone());
        shape
    }

    /// Entities whose `mesh` or `convex` collider wants triangles, and the
    /// model each names: `collider.model`, else `model.asset`. A host
    /// reads these before its models load (a model of several nodes gives
    /// its `model` to the children it is expanded into), then hands the
    /// triangles over with [`set_mesh`](Self::set_mesh).
    pub fn mesh_requests(world: &FlintWorld) -> Vec<(EntityId, String)> {
        let mut wanted = Vec::new();
        for &entity_id in world.entities_with_component(comp::COLLIDER) {
            let col = world.view(entity_id, comp::COLLIDER);
            if !matches!(col.opt_str("shape"), Some("mesh" | "convex")) {
                continue;
            }
            let asset = col.opt_str("model").or_else(|| {
                let model = world.view(entity_id, comp::MODEL);
                (!model.bool("runtime", false))
                    .then(|| model.opt_str("asset"))
                    .flatten()
            });
            if let Some(asset) = asset.filter(|a| !a.is_empty()) {
                wanted.push((entity_id, asset.to_string()));
            }
        }
        wanted
    }

    /// Hand over the triangles of an entity's `mesh` or `convex` collider,
    /// in the entity's own space (as its model is drawn), before its body
    /// is made.
    pub fn set_mesh(&mut self, entity_id: EntityId, mesh: Arc<ColliderMesh>) {
        self.meshes.insert(entity_id, mesh);
    }

    /// A `collider` is only ever given to Rapier as part of its entity's
    /// `rigidbody` (the loop above), so one without a rigidbody collides
    /// with nothing. Say so, once per entity, the second pass it is seen
    /// that way. Terrain and spline meshes register their own trimesh and
    /// are in `synced_entities`, so they are not this.
    fn report_lone_colliders(&mut self, world: &FlintWorld) {
        for &entity_id in world.entities_with_component(comp::COLLIDER) {
            if self.synced_entities.contains(&entity_id) || self.lone_reported.contains(&entity_id)
            {
                continue;
            }
            if self.lone_colliders.insert(entity_id) {
                continue;
            }
            self.lone_reported.insert(entity_id);
            world.view(entity_id, comp::COLLIDER).report("", || {
                Diagnostic::warning(
                    codes::NEEDS_COMPONENT,
                    "a collider with no rigidbody collides with nothing",
                )
                .hint(
                    "add a `rigidbody` to the entity, `body_type = \"static\"` for scenery, \
                     or remove the collider",
                )
            });
        }
    }

    /// Rigid world pose of an entity as Rapier sees it: the world matrix's
    /// translation and rotation, plus the sprite anchor lift and the 2D z clamp.
    fn body_world_pose(
        &mut self,
        world: &FlintWorld,
        entity_id: EntityId,
        mode_2d: bool,
    ) -> Isometry<f32> {
        let mat = world
            .get_world_matrix(entity_id)
            .unwrap_or_else(|| flint_core::Transform::IDENTITY.to_matrix());
        let (pos, quat) = mat4_to_rigid(&mat);

        // Scale is the collider's (`collider_shape`). A shear (#625, or a
        // non-uniform scale above a rotation) has no rigid body: the
        // collider is placed by the matrix's rotation alone and keeps its
        // square shape.
        let skew = flint_core::mat4_skew(&mat);
        if skew > 1e-3 && self.warned.insert(entity_id) {
            tracing::warn!(
                "physics: entity {:?} is sheared (basis {:.1} degrees off square); bodies and colliders ignore shear",
                world.get_name(entity_id).unwrap_or("?"),
                90.0 - skew.clamp(0.0, 1.0).acos().to_degrees()
            );
        }

        let z = if mode_2d { 0.0 } else { pos.z };
        let translation =
            na::Vector3::new(pos.x, pos.y + sprite_anchor_offset_y(world, entity_id), z);
        Isometry::from_parts(translation.into(), quat_to_na(quat))
    }

    /// World matrix of `parent` for writing a child body back as a local pose.
    /// The chain is composed from the nearest ancestor that has a Rapier body,
    /// using that body's simulated pose rather than its ECS transform: a
    /// kinematic ancestor trails its scripted pose by the accumulator
    /// remainder (see `update_kinematic_bodies`), and the child's simulated
    /// pose is relative to where the body is, not where the script put it.
    /// Falls back to the ECS world matrix when no ancestor has a body.
    fn parent_world_matrix(
        &self,
        world: &FlintWorld,
        physics: &PhysicsWorld,
        parent: EntityId,
    ) -> Option<[[f32; 4]; 4]> {
        let mut locals: Vec<[[f32; 4]; 4]> = Vec::new();
        let mut cur = parent;
        loop {
            if let Some(body) = self
                .body_map
                .get(&cur)
                .and_then(|h| physics.get_rigid_body(*h))
            {
                let lift = sprite_anchor_offset_y(world, cur);
                let mut iso = *body.position();
                iso.translation.vector.y -= lift;
                // The body is rigid; the entity's own scale goes back on,
                // so its children's local poses are in its scaled frame
                let mut mat = iso_to_mat4(&iso);
                let [kx, ky, kz] = world_scale(world, cur);
                for (col, k) in mat.iter_mut().take(3).zip([kx, ky, kz]) {
                    for c in col.iter_mut().take(3) {
                        *c *= k;
                    }
                }
                for local in locals.iter().rev() {
                    mat = flint_core::mat4_mul(&mat, local);
                }
                return Some(mat);
            }
            // A bare ancestor (no transform) is a grouping node: identity.
            locals.push(
                world
                    .get_transform(cur)
                    .unwrap_or(Transform::IDENTITY)
                    .to_matrix(),
            );
            match world.get_parent(cur) {
                Some(p) => cur = p,
                None => return Some(world.get_frame_matrix(parent)),
            }
        }
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

            if world.get_components(*entity_id).is_none() {
                continue;
            }

            let iso = body.position();
            let t = iso.translation.vector;
            // Reverse the sprite anchor lift applied when the body was placed
            let world_pos = Vec3::new(t.x, t.y - sprite_anchor_offset_y(world, *entity_id), t.z);
            let world_quat = na_to_quat(&iso.rotation);

            let (mut local_pos, local_quat) = match world
                .get_parent(*entity_id)
                .and_then(|p| self.parent_world_matrix(world, physics, p))
            {
                Some(parent_world) => {
                    // Rigid inverse, then out of the parent's scale: a
                    // child of a parent scaled 2 sits at half the distance
                    let (p, q) = rigid_inverse_apply(&parent_world, world_pos, world_quat);
                    // (unit scale divides by nothing, so it writes what it did)
                    let [kx, ky, kz] = unit_snapped(mat4_scale(&parent_world));
                    let unscale = |v: f32, k: f32| if k > 1e-10 { v / k } else { v };
                    (
                        Vec3::new(unscale(p.x, kx), unscale(p.y, ky), unscale(p.z, kz)),
                        q,
                    )
                }
                None => (world_pos, world_quat),
            };

            if world
                .view(*entity_id, comp::RIGIDBODY)
                .bool("mode_2d", false)
            {
                local_pos.z = 0.0;
            }

            writes.push((*entity_id, local_pos, local_quat));
        }

        for (entity_id, pos, quat) in writes {
            let Some(mut components) = world.get_components_mut(entity_id) else {
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
    ///
    /// Scripts pose kinematic bodies once per frame, but physics runs on a
    /// fixed accumulator: a frame may run zero, one or several steps. Handing
    /// the whole frame's motion to whichever step comes next makes the body's
    /// step velocity swing between double and zero, and the joint solver
    /// passes that swing straight into jointed children (a prismatic wheel
    /// rings along its free axis in proportion to speed). This is step
    /// `step_idx` within a frame spanning `frame_span` steps: the target is interpolated
    /// from the pose at the frame's first step so every step carries an even
    /// share of the motion. `(0, 1)` is a lone step and reaches the target.
    pub fn update_kinematic_bodies(
        &mut self,
        world: &FlintWorld,
        physics: &mut PhysicsWorld,
        step_idx: usize,
        frame_span: f32,
    ) {
        // `frame_span` is the wall time since the last step in steps: the
        // frame's step count plus the accumulator remainder. Step `i` moves
        // the body to `(i + 1) / span` of the way from its frame-start pose to
        // the scripted pose, so every step covers the same share of the
        // scripted motion; the remainder (under one step) is left for the
        // next frame, where it is part of that frame's span.
        let span = if frame_span.is_finite() {
            frame_span.max(1.0)
        } else {
            1.0
        };
        let first_step = step_idx == 0;
        let frac = ((step_idx + 1) as f32 / span).min(1.0);
        let handles: Vec<(EntityId, RigidBodyHandle)> =
            self.body_map.iter().map(|(e, h)| (*e, *h)).collect();
        if first_step {
            self.kin_frame_start.clear();
        }

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

            let mode_2d = world
                .view(entity_id, comp::RIGIDBODY)
                .bool("mode_2d", false);
            let iso = self.body_world_pose(world, entity_id, mode_2d);

            if let Some(body_mut) = physics.rigid_body_set.get_mut(body_handle) {
                // Rapier refreshes a body's world centre of mass only for
                // active dynamic bodies, so a kinematic body's stays at its
                // creation pose and the joint solver's lever arm (angvel x
                // (anchor - com)) grows with every metre driven: a turning
                // parent then flings its jointed children. Recompute from the
                // current pose before each step.
                body_mut.recompute_mass_properties_from_colliders(&physics.collider_set);
                let cur = *body_mut.position();
                self.kin_start.insert(body_handle, cur);
                let start = *self.kin_frame_start.entry(body_handle).or_insert(cur);
                let next = if frac >= 1.0 {
                    iso
                } else {
                    start.lerp_slerp(&iso, frac)
                };
                body_mut.set_next_kinematic_position(next);
            }
        }
    }

    /// Update sensor flags on already-synced colliders to match ECS state.
    /// This allows scripts to make colliders non-solid at runtime (e.g., dead enemies).
    pub fn update_sensor_flags(&self, world: &FlintWorld, physics: &mut PhysicsWorld) {
        for (entity_id, col_handle) in &self.collider_map {
            let col = world.view(*entity_id, comp::COLLIDER);
            if !col.exists() {
                continue;
            }
            let wants_sensor = col.bool("is_sensor", false);

            let fixed = physics.is_fixed_collider(*col_handle);
            if let Some(collider) = physics.collider_set.get_mut(*col_handle) {
                if collider.is_sensor() != wants_sensor {
                    collider.set_sensor(wants_sensor);
                    if fixed {
                        physics.touch_static();
                    }
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
            let jd = world.view(entity_id, comp::JOINT);
            if !jd.exists() {
                continue;
            }

            let kind = JointKind::read(&jd);

            let parent_name = jd.str("parent", "");
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

            // The anchor is metres along the child's axes, its scale not
            // applied: scenes are written that way (a bob drawn at 0.56
            // names its pivot 1.6 m off), so a scaled jointed body is told
            // once rather than having its pivot moved (#729)
            let anchor = jd.vec3("anchor", [0.0; 3]);
            let axis = jd.vec3("axis", [1.0, 0.0, 0.0]);
            let scale = unit_snapped(mat4_scale(&child_world));
            if scale != [1.0; 3] && self.joint_scale_warned.insert(entity_id) {
                tracing::warn!(
                    "physics: jointed entity {:?} has world scale [{:.3}, {:.3}, {:.3}]; its collider takes the scale, but `joint.anchor` is metres along its axes, unscaled",
                    world.get_name(entity_id).unwrap_or("?"),
                    scale[0],
                    scale[1],
                    scale[2]
                );
            }

            // Parent anchor/axis come from the world-space anchor expressed
            // in the parent's frame.
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

            joint.set_contacts_enabled(jd.bool("contacts_enabled", false));

            let params = read_joint_params(&jd, kind);
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
            let jd = world.view(*entity_id, comp::JOINT);
            if !jd.exists() {
                continue;
            }
            let kind = JointKind::read(&jd);
            if kind == JointKind::Fixed {
                continue;
            }
            let params = read_joint_params(&jd, kind);
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
    ///
    /// Reaching a limit also cancels the relative velocity into it, so a stop
    /// is inelastic instead of trading impulses with Rapier's own limit
    /// constraint step after step.
    pub fn project_kinematic_joints(&mut self, physics: &mut PhysicsWorld, dt: f32) {
        let mut updates: Vec<(RigidBodyHandle, Isometry<f32>, Option<na::Vector3<f32>>)> =
            Vec::new();
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

            // Rapier's angular motor measures the hinge angle as
            // asin(rel_quat.imag[axis]) with no clamp: a twist near 180 deg
            // whose quaternion norm has drifted a few 1e-6 over 1.0 gives NaN,
            // and the solver writes it into the child's pose and velocity for
            // good (the pod vanished from the trick bench). Rebuild such a
            // child from the parent frame at the motor's target angle, at rest.
            let pose_finite = b2
                .position()
                .translation
                .vector
                .iter()
                .all(|c| c.is_finite())
                && b2.position().rotation.coords.iter().all(|c| c.is_finite());
            let vel_finite = b2.linvel().iter().all(|c| c.is_finite())
                && b2.angvel().iter().all(|c| c.is_finite());
            if !pose_finite || !vel_finite {
                let axis_i = [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ]
                    .into_iter()
                    .find(|a| {
                        !locked.contains((*a).into()) && data.motor_axes.contains((*a).into())
                    });
                let rel = match axis_i {
                    Some(a) => {
                        let mut v = na::Vector3::zeros();
                        v[a as usize - 3] = 1.0;
                        na::UnitQuaternion::from_axis_angle(
                            &na::UnitVector3::new_normalize(v),
                            data.motors[a as usize].target_pos,
                        )
                    }
                    None => na::UnitQuaternion::identity(),
                };
                let f2 = Isometry::from_parts(f1.translation, f1.rotation * rel);
                let pose = f2 * data.local_frame2.inverse();
                if self.nan_warned.insert(joint.body2) {
                    tracing::warn!(
                        "physics: jointed body {:?} went non-finite; re-seated on its parent at the motor target",
                        joint.body2
                    );
                }
                updates.push((joint.body2, pose, Some(na::Vector3::zeros())));
                continue;
            }
            let f2 = b2.position() * data.local_frame2; // joint frame from the child

            // Linear: child anchor offset in the parent's joint frame. Locked
            // axes go to zero; a free axis keeps its geometric coordinate,
            // clamped to the limits.
            let geom = f1.rotation.inverse() * (f2.translation.vector - f1.translation.vector);
            // Parent step velocity from its own start and end poses, for the
            // relative velocity at the anchor when a limit is hit.
            let (v1, w1) = match self.kin_start.get(&joint.body1) {
                Some(start) if dt > 0.0 => {
                    let end = b1.position();
                    let v = (end.translation.vector - start.translation.vector) / dt;
                    let dq = end.rotation * start.rotation.inverse();
                    (v, dq.scaled_axis() / dt)
                }
                _ => (na::Vector3::zeros(), na::Vector3::zeros()),
            };
            let rel_v = b2.linvel() - (v1 + w1.cross(&(f2.translation.vector - b1.translation())));
            let rel_v_local = f1.rotation.inverse() * rel_v;
            let mut o = na::Vector3::zeros();
            let mut vel_fix = na::Vector3::zeros();
            let mut fix_vel = false;
            let lin = [JointAxis::LinX, JointAxis::LinY, JointAxis::LinZ];
            let lin_mask = [
                JointAxesMask::LIN_X,
                JointAxesMask::LIN_Y,
                JointAxesMask::LIN_Z,
            ];
            for i in 0..3 {
                if locked.contains(lin_mask[i]) {
                    continue;
                }
                let mut c = geom[i];
                if data.limit_axes.contains(lin_mask[i]) {
                    let l = data.limits[lin[i] as usize];
                    let clamped = c.clamp(l.min, l.max);
                    if clamped != c {
                        vel_fix[i] = -rel_v_local[i];
                        fix_vel = true;
                    }
                    c = clamped;
                }
                o[i] = c;
            }
            let new_vel = if fix_vel {
                Some(b2.linvel() + f1.rotation * vel_fix)
            } else {
                None
            };

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
            let mut new_pose = new_f2 * data.local_frame2.inverse();
            // Products of f32 unit quaternions drift off unit norm; Rapier
            // never renormalises, and the motor's asin needs |imag| <= 1.
            new_pose.rotation = na::UnitQuaternion::new_normalize(new_pose.rotation.into_inner());
            let cur = b2.position();
            let moved = (new_pose.translation.vector - cur.translation.vector).norm_squared()
                > 1e-12
                || new_pose.rotation.angle_to(&cur.rotation) > 1e-5;
            if moved || new_vel.is_some() {
                updates.push((joint.body2, new_pose, new_vel));
            }
        }
        for (handle, pose, vel) in updates {
            if let Some(body) = physics.get_rigid_body_mut(handle) {
                body.set_position(pose, true);
                if let Some(v) = vel {
                    body.set_linvel(v, true);
                    if v == na::Vector3::zeros() {
                        body.set_angvel(na::Vector3::zeros(), true);
                    }
                }
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
        // On `default` until the next sync reads the entity's `collider`
        // layer, if it has one (`trimesh_pending`)
        let collider = ColliderBuilder::new(shape)
            .friction(friction)
            .restitution(restitution)
            .collision_groups(InteractionGroups::new(Group::GROUP_1, Group::ALL))
            .build();

        let col_handle = physics.insert_collider_with_parent(collider, body_handle);
        self.insert_collider(entity_id, col_handle);
        self.trimesh_pending.push(entity_id);
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
    /// `joint.type`; an unknown one is reported and is a hinge, the default.
    fn read(jd: &ComponentView) -> Self {
        match jd.choice("type", JOINT_TYPES, "hinge") {
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

fn read_joint_params(jd: &ComponentView, kind: JointKind) -> JointParams {
    let axis = match kind {
        JointKind::Prismatic => JointAxis::LinX,
        JointKind::Spherical => match jd.choice("motor_axis", &["x", "y", "z"], "x") {
            "y" => JointAxis::AngY,
            "z" => JointAxis::AngZ,
            _ => JointAxis::AngX,
        },
        _ => JointAxis::AngX,
    };
    let unit = if kind.angular() {
        std::f32::consts::PI / 180.0
    } else {
        1.0
    };
    let target = jd.f32("motor_target", 0.0) * unit;
    let stiffness = jd.f32("motor_stiffness", 0.0).max(0.0);
    let damping = jd.f32("motor_damping", 0.0).max(0.0);
    let max_force = jd.f32("motor_max_force", 0.0).max(0.0);
    let model = match jd.choice("motor_model", &["acceleration", "force"], "acceleration") {
        "force" => MotorModel::ForceBased,
        _ => MotorModel::AccelerationBased,
    };
    // `[min, max]`; min >= max means free, so only the shape is reported
    let limits = jd
        .raw("limits")
        .and_then(|v| {
            let pair = v
                .as_array()
                .filter(|a| a.len() >= 2)
                .and_then(|a| Some([toml_f32(&a[0])?, toml_f32(&a[1])?]));
            if pair.is_none() {
                jd.report("limits", || {
                    Diagnostic::error(
                        codes::BAD_TYPE,
                        format!("expected 2 numbers, [min, max], got {v}"),
                    )
                    .hint("the joint is left free")
                });
            }
            pair
        })
        .map(|l| [l[0] * unit, l[1] * unit])
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

/// Column-major `[col][row]` matrix of a rigid pose, matching `Transform::to_matrix`.
fn iso_to_mat4(iso: &Isometry<f32>) -> [[f32; 4]; 4] {
    let m = iso.to_homogeneous();
    let mut out = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            out[c][r] = m[(r, c)];
        }
    }
    out
}

fn quat_to_na(q: [f32; 4]) -> na::UnitQuaternion<f32> {
    // Renormalise: matrix-extracted quaternions are off unit by a few ulps,
    // and Rapier's motors assume exact unit rotations.
    na::UnitQuaternion::new_normalize(na::Quaternion::new(q[3], q[0], q[1], q[2]))
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

/// Lift so the body centre sits at the sprite's visual centre: with
/// `anchor_y = 0` (bottom-anchored) the centre is `height / 2` above the
/// entity position. `sync_from_rapier` reverses it.
fn sprite_anchor_offset_y(world: &FlintWorld, id: EntityId) -> f32 {
    let sprite = world.view(id, comp::SPRITE);
    if !sprite.exists() {
        return 0.0;
    }
    (0.5 - sprite.f32("anchor_y", 0.0)) * sprite.f32("height", 1.0)
}

/// Collider pose relative to the body: bounds centre + `collider.offset`,
/// both points in the entity's frame and so stretched by its world
/// `scale`, rotated by `collider.rotation` (Euler degrees). `None` when
/// identity.
fn collider_local_isometry(
    bounds: &ComponentView,
    col: &ComponentView,
    scale: [f32; 3],
) -> Option<Isometry<f32>> {
    let bounds_center = compute_bounds_center(bounds).unwrap_or([0.0, 0.0, 0.0]);
    let offset = col.vec3("offset", [0.0; 3]);
    let rotation = col.vec3("rotation", [0.0; 3]);

    let t = [
        (bounds_center[0] + offset[0]) * scale[0],
        (bounds_center[1] + offset[1]) * scale[1],
        (bounds_center[2] + offset[2]) * scale[2],
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

/// The lengths of an entity's world matrix's axes: what its collider and
/// joint anchors are stretched by. `[1, 1, 1]` for an entity with no
/// transform.
fn world_scale(world: &FlintWorld, id: EntityId) -> [f32; 3] {
    world
        .get_world_matrix(id)
        .map(|m| unit_snapped(mat4_scale(&m)))
        .unwrap_or([1.0; 3])
}

/// A scale whose axes are 1 to rounding (a rotated unscaled matrix's
/// columns are a few ulps off) read as exactly 1, so an unscaled entity's
/// shapes and anchors are the bits they were before scale was applied.
fn unit_snapped(s: [f32; 3]) -> [f32; 3] {
    s.map(|k| if (k - 1.0).abs() <= 1e-6 { 1.0 } else { k })
}

/// Two scale factors the same, to a part in a thousand.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0)
}

fn is_uniform(s: [f32; 3]) -> bool {
    close(s[0], s[1]) && close(s[1], s[2])
}

/// Compute the center offset of a bounds component
fn compute_bounds_center(bounds: &ComponentView) -> Option<[f32; 3]> {
    let min = bounds.opt_vec3("min")?;
    let max = bounds.opt_vec3("max")?;
    Some([
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ])
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
    fn child_under_a_bare_group_keeps_the_rig_offset() {
        let mut world = FlintWorld::new();
        let rig = world.spawn("rig").unwrap();
        set_transform(&mut world, rig, [10.0, 0.0, 0.0]);
        let group = world.spawn("group").unwrap();
        world.set_parent(group, rig).unwrap();
        let child = world.spawn("bob").unwrap();
        set_transform(&mut world, child, [0.0, 5.0, 0.0]);
        set_body(&mut world, child, "dynamic");
        set_collider(&mut world, child, "sphere", [1.0, 1.0, 1.0]);
        world.set_parent(child, group).unwrap();

        let mut physics = PhysicsWorld::new();
        let mut sync = PhysicsSync::new();
        sync.sync_to_rapier(&world, &mut physics);
        let t = *physics
            .get_rigid_body(sync.body_map[&child])
            .unwrap()
            .translation();
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
        sync.update_kinematic_bodies(&world, &mut physics, 0, 1.0);
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
