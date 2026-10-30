//! Two-bone IK over transform chains (ADR 0070).
//!
//! An `ik_two_bone` component sits on the second bone of a chain (forearm,
//! shin). Its transform parent is the first bone; a child named by `tip` marks
//! the chain end. Each frame, after scripts, physics and animation have posed
//! the world, [`IkPass::run`] rotates the two bones so the tip reaches the
//! `target` entity and the mid joint bends toward the `pole` entity. Only the
//! two bones' `rotation_quat` are written; positions and every other entity are
//! untouched, so a model that carries its targets as glTF empties (a hand
//! target parented to a steering yoke, say) animates its arms with no script
//! math at all.
//!
//! The solve starts from each bone's *rest* rotation, captured the first time
//! the pass sees the entity, not from last frame's result, so twist cannot
//! drift over many frames of a cyclic target.

use std::collections::{HashMap, HashSet};

use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::{
    mat4_to_rigid, quat_conjugate, quat_from_two_vectors, quat_mul, quat_nlerp, quat_normalize,
    quat_rotate_vec3, solve_two_bone, EntityId,
};
use flint_ecs::FlintWorld;

/// Per-scene state for the pass: rest rotations and one-shot warnings.
#[derive(Default)]
pub struct IkPass {
    /// Rest local rotations of (first bone, second bone), captured on first sight.
    rest: HashMap<EntityId, ([f32; 4], [f32; 4])>,
    warned: HashSet<EntityId>,
}

impl IkPass {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget captured rest poses and warnings (scene transition).
    pub fn clear(&mut self) {
        self.rest.clear();
        self.warned.clear();
    }

    /// Solve every enabled `ik_two_bone` chain in `world`.
    pub fn run(&mut self, world: &mut FlintWorld) {
        let ids: Vec<EntityId> = world
            .entities_with_component(comp::IK_TWO_BONE)
            .iter()
            .copied()
            .collect();
        for id in ids {
            if let Err(why) = self.solve_entity(world, id) {
                if self.warned.insert(id) {
                    let name = world.get_name(id).unwrap_or("?").to_string();
                    log::warn!("ik_two_bone on {name}: {why}");
                }
            }
        }
    }

    fn solve_entity(&mut self, world: &mut FlintWorld, id: EntityId) -> Result<(), String> {
        let field = |world: &FlintWorld, name: &str| -> Option<toml::Value> {
            world
                .get_components(id)
                .and_then(|c| c.get_field(comp::IK_TWO_BONE, name))
                .cloned()
        };
        let enabled = field(world, "enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let weight = field(world, "weight")
            .as_ref()
            .and_then(toml_f32)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if !enabled || weight <= 0.0 {
            return Ok(());
        }
        let resolve = |world: &FlintWorld, key: &str| -> Result<EntityId, String> {
            let name = field(world, key)
                .and_then(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("`{key}` is not set"))?;
            world
                .get_id(&name)
                .ok_or_else(|| format!("`{key}` entity {name} not found"))
        };
        let target_id = resolve(world, "target")?;
        let pole_id = resolve(world, "pole")?;
        let tip_id = resolve(world, "tip")?;
        let upper_id = world
            .get_parent(id)
            .ok_or_else(|| "no transform parent to use as the first bone".to_string())?;
        if world.get_parent(tip_id) != Some(id) {
            return Err("`tip` must be a child of this entity".to_string());
        }

        // Rest geometry: the bones' local offsets are the segment vectors.
        let fore_t = world
            .get_transform(id)
            .ok_or_else(|| "no transform".to_string())?;
        let tip_t = world
            .get_transform(tip_id)
            .ok_or_else(|| "`tip` has no transform".to_string())?;
        let upper_t = world
            .get_transform(upper_id)
            .ok_or_else(|| "first bone has no transform".to_string())?;
        let upper_vec = fore_t.position; // first bone: its parent's origin to this entity
        let fore_vec = tip_t.position; // second bone: this entity to the tip
        let a = upper_vec.length();
        let b = fore_vec.length();
        if a < 1e-6 || b < 1e-6 {
            return Err("a bone has zero length (tip or this entity sits on its parent)".into());
        }

        // Capture the rest rotations once, before anything is written.
        let (upper_rest, fore_rest) = *self
            .rest
            .entry(id)
            .or_insert_with(|| (upper_t.effective_quat(), fore_t.effective_quat()));

        // World frame of the chain root (the first bone's parent).
        let root_q = match world.get_parent(upper_id) {
            Some(g) => {
                let m = world
                    .get_world_matrix(g)
                    .ok_or_else(|| "chain root has no transform".to_string())?;
                mat4_to_rigid(&m).1
            }
            None => [0.0, 0.0, 0.0, 1.0],
        };
        let shoulder = world
            .get_world_position(upper_id)
            .ok_or_else(|| "first bone has no world position".to_string())?;
        let target = world
            .get_world_position(target_id)
            .ok_or_else(|| "`target` has no world position".to_string())?;
        let pole = world
            .get_world_position(pole_id)
            .ok_or_else(|| "`pole` has no world position".to_string())?;

        let Some(sol) = solve_two_bone(shoulder, target, pole, a, b) else {
            // Pole colinear with the reach line: hold the previous pose this frame.
            return Ok(());
        };
        let e_dir = (sol.mid - shoulder).normalized();
        let f_dir = (sol.tip - sol.mid).normalized();

        // Rest world orientations, then the shortest rotation that aims each bone.
        let upper_rest_w = quat_mul(&root_q, &upper_rest);
        let cur_up = quat_rotate_vec3(&upper_rest_w, upper_vec.normalized().to_array());
        let upper_w = quat_normalize(&quat_mul(
            &quat_from_two_vectors(cur_up, e_dir.to_array()),
            &upper_rest_w,
        ));
        let fore_rest_w = quat_mul(&upper_w, &fore_rest);
        let cur_fore = quat_rotate_vec3(&fore_rest_w, fore_vec.normalized().to_array());
        let fore_w = quat_normalize(&quat_mul(
            &quat_from_two_vectors(cur_fore, f_dir.to_array()),
            &fore_rest_w,
        ));

        let upper_local = quat_mul(&quat_conjugate(&root_q), &upper_w);
        let fore_local = quat_mul(&quat_conjugate(&upper_w), &fore_w);
        let upper_out = quat_nlerp(&upper_rest, &upper_local, weight);
        let fore_out = quat_nlerp(&fore_rest, &fore_local, weight);

        write_rotation(world, upper_id, upper_out);
        write_rotation(world, id, fore_out);
        Ok(())
    }
}

/// Store a quaternion as the entity's rotation and zero the Euler triple, the
/// same shape the script API's `set_rotation_quat` writes.
fn write_rotation(world: &mut FlintWorld, id: EntityId, q: [f32; 4]) {
    let arr = toml::Value::Array(q.iter().map(|c| toml::Value::Float(*c as f64)).collect());
    let _ = world.set_field(id, comp::TRANSFORM, "rotation_quat", arr);
    let zero = toml::Value::Array(vec![toml::Value::Float(0.0); 3]);
    let _ = world.set_field(id, comp::TRANSFORM, "rotation", zero);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::Vec3;

    fn floats(v: &[f64]) -> toml::Value {
        toml::Value::Array(v.iter().map(|c| toml::Value::Float(*c)).collect())
    }

    fn table(pairs: Vec<(&str, toml::Value)>) -> toml::Value {
        let mut m = toml::map::Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), v);
        }
        toml::Value::Table(m)
    }

    fn node(
        world: &mut FlintWorld,
        name: &str,
        pos: [f64; 3],
        parent: Option<EntityId>,
    ) -> EntityId {
        let id = world.spawn(name).unwrap();
        world
            .set_component(id, "transform", table(vec![("position", floats(&pos))]))
            .unwrap();
        if let Some(p) = parent {
            world.set_parent(id, p).unwrap();
        }
        id
    }

    struct Rig {
        world: FlintWorld,
        torso: EntityId,
        upper: EntityId,
        fore: EntityId,
        tip: EntityId,
        target: EntityId,
    }

    /// Torso at the origin; shoulder 0.2 right; upper arm hangs 0.3 down;
    /// forearm 0.25 further down. Target and pole are free entities.
    fn rig(target: [f64; 3], pole: [f64; 3]) -> Rig {
        let mut world = FlintWorld::new();
        let torso = node(&mut world, "torso", [0.0, 0.0, 0.0], None);
        let upper = node(&mut world, "upper", [0.2, 0.0, 0.0], Some(torso));
        let fore = node(&mut world, "fore", [0.0, -0.3, 0.0], Some(upper));
        let tip = node(&mut world, "tip", [0.0, -0.25, 0.0], Some(fore));
        let target = node(&mut world, "target", target, None);
        node(&mut world, "pole", pole, None);
        world
            .set_component(
                fore,
                comp::IK_TWO_BONE,
                table(vec![
                    ("target", toml::Value::String("target".into())),
                    ("pole", toml::Value::String("pole".into())),
                    ("tip", toml::Value::String("tip".into())),
                ]),
            )
            .unwrap();
        Rig {
            world,
            torso,
            upper,
            fore,
            tip,
            target,
        }
    }

    fn dist(a: Vec3, b: Vec3) -> f32 {
        (a - b).length()
    }

    #[test]
    fn tip_reaches_target_and_elbow_bends_toward_pole() {
        let mut r = rig([0.3, -0.2, -0.3], [0.8, -0.3, 0.0]);
        let mut pass = IkPass::new();
        pass.run(&mut r.world);
        let tip = r.world.get_world_position(r.tip).unwrap();
        let target = r.world.get_world_position(r.target).unwrap();
        assert!(dist(tip, target) < 1e-4, "tip {tip:?} target {target:?}");
        let elbow = r.world.get_world_position(r.fore).unwrap();
        let shoulder = r.world.get_world_position(r.upper).unwrap();
        assert!(
            elbow.x > shoulder.x + 0.05,
            "elbow {elbow:?} should bend outward (+X)"
        );
        assert!((dist(shoulder, elbow) - 0.3).abs() < 1e-4);
        assert!((dist(elbow, tip) - 0.25).abs() < 1e-4);
        // Only rotations were written.
        let t = r.world.get_transform(r.fore).unwrap();
        assert!(t.rotation_quat.is_some());
        assert!((t.position.y + 0.3).abs() < 1e-6);
    }

    #[test]
    fn leaned_root_still_lands_the_tip() {
        let mut r = rig([0.3, -0.2, -0.3], [0.8, -0.3, 0.0]);
        r.world
            .set_field(r.torso, "transform", "rotation", floats(&[0.0, 0.0, 20.0]))
            .unwrap();
        let mut pass = IkPass::new();
        for _ in 0..3 {
            pass.run(&mut r.world); // repeat: solving from rest must be idempotent
        }
        let tip = r.world.get_world_position(r.tip).unwrap();
        let target = r.world.get_world_position(r.target).unwrap();
        assert!(dist(tip, target) < 1e-4, "tip {tip:?} target {target:?}");
    }

    #[test]
    fn moving_target_is_tracked_frame_to_frame() {
        let mut r = rig([0.3, -0.2, -0.3], [0.8, -0.3, 0.0]);
        let mut pass = IkPass::new();
        for step in 0..20 {
            let x = 0.3 - 0.02 * step as f64;
            r.world
                .set_field(r.target, "transform", "position", floats(&[x, -0.2, -0.3]))
                .unwrap();
            pass.run(&mut r.world);
            let tip = r.world.get_world_position(r.tip).unwrap();
            let target = r.world.get_world_position(r.target).unwrap();
            assert!(
                dist(tip, target) < 1e-4,
                "step {step}: tip {tip:?} target {target:?}"
            );
        }
    }

    #[test]
    fn weight_zero_and_disabled_leave_the_pose() {
        let mut r = rig([0.3, -0.2, -0.3], [0.8, -0.3, 0.0]);
        r.world
            .set_field(r.fore, comp::IK_TWO_BONE, "weight", toml::Value::Float(0.0))
            .unwrap();
        let before = r.world.get_world_position(r.tip).unwrap();
        IkPass::new().run(&mut r.world);
        assert!(dist(before, r.world.get_world_position(r.tip).unwrap()) < 1e-6);
        r.world
            .set_field(r.fore, comp::IK_TWO_BONE, "weight", toml::Value::Float(1.0))
            .unwrap();
        r.world
            .set_field(
                r.fore,
                comp::IK_TWO_BONE,
                "enabled",
                toml::Value::Boolean(false),
            )
            .unwrap();
        IkPass::new().run(&mut r.world);
        assert!(dist(before, r.world.get_world_position(r.tip).unwrap()) < 1e-6);
    }

    #[test]
    fn overreach_points_the_straight_arm_at_the_target() {
        let mut r = rig([3.0, -0.5, 0.0], [0.8, -0.3, 0.0]);
        IkPass::new().run(&mut r.world);
        let shoulder = r.world.get_world_position(r.upper).unwrap();
        let tip = r.world.get_world_position(r.tip).unwrap();
        let target = r.world.get_world_position(r.target).unwrap();
        let reach = dist(shoulder, tip);
        assert!(reach > 0.548 && reach <= 0.55 + 1e-4, "reach {reach}");
        let d = (tip - shoulder).normalized();
        let t = (target - shoulder).normalized();
        assert!(d.dot(&t) > 0.9999, "{d:?} vs {t:?}");
    }

    #[test]
    fn missing_target_warns_once_and_changes_nothing() {
        let mut r = rig([0.3, -0.2, -0.3], [0.8, -0.3, 0.0]);
        r.world
            .set_field(
                r.fore,
                comp::IK_TWO_BONE,
                "target",
                toml::Value::String("nope".into()),
            )
            .unwrap();
        let before = r.world.get_world_position(r.tip).unwrap();
        let mut pass = IkPass::new();
        pass.run(&mut r.world);
        pass.run(&mut r.world);
        assert_eq!(pass.warned.len(), 1);
        assert!(dist(before, r.world.get_world_position(r.tip).unwrap()) < 1e-6);
    }
}
