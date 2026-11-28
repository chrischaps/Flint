//! The rig as clip kinds in an `AnimationSystem` (#1621): what the player's
//! frame and a capture's step ask of it, through the system alone.
//!
//! - its kinds run at their slots whatever order they were added in, so the
//!   frame is today's (property, sequences, skeletal, node, flipbook);
//! - the skeletal kind's pose (`AnimationSystem::pose`, the player's
//!   `Stage::Pose`) solves two-bone IK over the world, as `IkPass` alone did;
//! - registering a clip through `Rig` adds the kind it needs.

use flint_animation::{AnimationSystem, SLOT_FLIPBOOK, SLOT_NODE, SLOT_SKELETAL};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_rig::skeletal_clip::SkeletalClip;
use flint_rig::{IkPass, NodeKind, Rig, SkeletalKind};

fn floats(v: &[f64]) -> toml::Value {
    toml::Value::Array(v.iter().map(|c| toml::Value::Float(*c)).collect())
}

fn node(world: &mut FlintWorld, name: &str, pos: [f64; 3], parent: Option<EntityId>) -> EntityId {
    let id = world.spawn(name).unwrap();
    let mut transform = toml::map::Map::new();
    transform.insert("position".into(), floats(&pos));
    world
        .set_component(id, comp::TRANSFORM, toml::Value::Table(transform))
        .unwrap();
    if let Some(p) = parent {
        world.set_parent(id, p).unwrap();
    }
    id
}

/// An arm hanging down from a shoulder, its hand asked to reach up and to
/// the side: the chain the book's IK page draws.
fn arm() -> (FlintWorld, EntityId, EntityId) {
    let mut world = FlintWorld::new();
    let torso = node(&mut world, "torso", [0.0, 0.0, 0.0], None);
    let upper = node(&mut world, "upper", [0.2, 0.0, 0.0], Some(torso));
    let fore = node(&mut world, "fore", [0.0, -0.3, 0.0], Some(upper));
    node(&mut world, "tip", [0.0, -0.25, 0.0], Some(fore));
    node(&mut world, "target", [0.45, 0.1, 0.1], None);
    node(&mut world, "pole", [0.2, -0.2, -1.0], None);
    let mut ik = toml::map::Map::new();
    for (k, v) in [("target", "target"), ("pole", "pole"), ("tip", "tip")] {
        ik.insert(k.into(), toml::Value::String(v.into()));
    }
    world
        .set_component(fore, comp::IK_TWO_BONE, toml::Value::Table(ik))
        .unwrap();
    (world, upper, fore)
}

fn rotations(world: &FlintWorld, ids: &[EntityId]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            world
                .get_components(*id)
                .and_then(|c| c.get(comp::TRANSFORM).cloned())
                .map(|t| t.to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn the_kinds_run_at_their_slots_whatever_order_they_were_added_in() {
    let mut system = AnimationSystem::new();
    system.add_kind(Box::new(NodeKind::new()));
    system.add_kind(Box::new(SkeletalKind::new()));
    let order: Vec<(&str, u32)> = system.kinds().map(|k| (k.name(), k.slot())).collect();
    assert_eq!(order, [("skeletal", SLOT_SKELETAL), ("node", SLOT_NODE)]);
    assert!(SLOT_NODE < SLOT_FLIPBOOK, "the flipbook runs after the rig");
}

#[test]
fn the_skeletal_kinds_pose_solves_ik_as_the_pass_alone_does() {
    let (mut alone, upper, fore) = arm();
    IkPass::new().run(&mut alone);

    let (mut world, ..) = arm();
    let before = rotations(&world, &[upper, fore]);
    let mut system = AnimationSystem::new();
    system.add_kind(Box::new(SkeletalKind::new()));
    system.pose(&mut world);

    let after = rotations(&world, &[upper, fore]);
    assert_ne!(after, before, "the chain bent");
    assert_eq!(after, rotations(&alone, &[upper, fore]));
}

#[test]
fn without_the_rigs_kinds_the_pose_leaves_the_chain_as_authored() {
    let (mut world, upper, fore) = arm();
    let before = rotations(&world, &[upper, fore]);
    AnimationSystem::new().pose(&mut world);
    assert_eq!(rotations(&world, &[upper, fore]), before);
}

#[test]
fn a_skeletal_clip_registered_through_rig_adds_its_kind_once() {
    let mut system = AnimationSystem::new();
    assert!(system.skeletal().is_none());
    for name in ["idle", "wave"] {
        system.add_skeletal_clip(SkeletalClip {
            name: name.into(),
            duration: 1.0,
            joint_tracks: vec![],
        });
    }
    assert_eq!(system.kinds().count(), 1);
    assert_eq!(system.skeletal_clip_names(), ["idle", "wave"]);
    assert!(system.kind("skeletal").unwrap().has_clip("wave"));
    assert!(system.nodes().is_none());
}
