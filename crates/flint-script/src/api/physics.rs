//! The `physics` and `physics_2d` modules: rays against colliders, the
//! sprite plane's rectangle query and 2D bodies' velocities (ADR 0116).
//! Rays and character moves take a layer mask (#729): a layer's name or a
//! list of names from the project's `[project.physics] layers`.

use super::builder::kind::{Any, Array, Entity, Map as MapKind, Num};
use super::builder::{Call, CallResult, ScriptApi};
use crate::context::ScriptCommand;
use flint_physics::{EntityRaycastHit, LayerMask, PhysicsSystem};
use rhai::{Dynamic, Map};

/// A mask argument: one layer's name or an array of names. An unknown
/// name, or anything else, is a script error at the call.
fn read_mask(call: &Call, physics: &PhysicsSystem, mask: &Dynamic) -> CallResult<LayerMask> {
    let names: Vec<String> = if let Some(name) = mask.read_lock::<rhai::ImmutableString>() {
        vec![name.to_string()]
    } else if let Some(list) = mask.read_lock::<rhai::Array>() {
        let mut names = Vec::with_capacity(list.len());
        for item in list.iter() {
            match item.read_lock::<rhai::ImmutableString>() {
                Some(name) => names.push(name.to_string()),
                None => {
                    return Err(call.error(format!(
                        "a mask is layer names; found a {} in it",
                        item.type_name()
                    )))
                }
            }
        }
        names
    } else {
        return Err(call.error(format!(
            "the mask is a layer's name or an array of names, not a {}",
            mask.type_name()
        )));
    };
    physics.layer_mask(&names).map_err(|e| call.error(e))
}

fn hit_3d(hit: EntityRaycastHit) -> Dynamic {
    let mut map = Map::new();
    map.insert("entity".into(), Dynamic::from(hit.entity_id.raw() as i64));
    map.insert("distance".into(), Dynamic::from(hit.distance as f64));
    map.insert("point_x".into(), Dynamic::from(hit.point[0] as f64));
    map.insert("point_y".into(), Dynamic::from(hit.point[1] as f64));
    map.insert("point_z".into(), Dynamic::from(hit.point[2] as f64));
    map.insert("normal_x".into(), Dynamic::from(hit.normal[0] as f64));
    map.insert("normal_y".into(), Dynamic::from(hit.normal[1] as f64));
    map.insert("normal_z".into(), Dynamic::from(hit.normal[2] as f64));
    Dynamic::from(map)
}

fn hit_2d(hit: EntityRaycastHit) -> Dynamic {
    let mut map = Map::new();
    map.insert("entity".into(), Dynamic::from(hit.entity_id.raw() as i64));
    map.insert("distance".into(), Dynamic::from(hit.distance as f64));
    map.insert("point_x".into(), Dynamic::from(hit.point[0] as f64));
    map.insert("point_y".into(), Dynamic::from(hit.point[1] as f64));
    map.insert("normal_x".into(), Dynamic::from(hit.normal[0] as f64));
    map.insert("normal_y".into(), Dynamic::from(hit.normal[1] as f64));
    Dynamic::from(map)
}

const HIT_3D: &str =
    "#{entity, distance, point_x, point_y, point_z, normal_x, normal_y, normal_z}, or () for no hit or no physics";
const HIT_2D: &str =
    "#{entity, distance, point_x, point_y, normal_x, normal_y}, or () for no hit or no physics";

/// A ray in 3D from a script: the calling entity's own collider left out.
#[allow(clippy::too_many_arguments)]
fn cast_3d(
    call: &Call,
    (ox, oy, oz): (f64, f64, f64),
    (dx, dy, dz): (f64, f64, f64),
    max_dist: f64,
    mask: Option<&Dynamic>,
) -> CallResult<Dynamic> {
    let exclude = Some(call.ctx_ref().current_entity);
    let Some(physics) = call.physics() else {
        return Ok(Dynamic::UNIT);
    };
    let mask = mask.map(|m| read_mask(call, physics, m)).transpose()?;
    Ok(physics
        .raycast_masked(
            [ox as f32, oy as f32, oz as f32],
            [dx as f32, dy as f32, dz as f32],
            max_dist as f32,
            exclude,
            mask,
        )
        .map_or(Dynamic::UNIT, hit_3d))
}

fn cast_2d(
    call: &Call,
    (ox, oy): (f64, f64),
    (dx, dy): (f64, f64),
    max_dist: f64,
    mask: Option<&Dynamic>,
) -> CallResult<Dynamic> {
    let exclude = Some(call.ctx_ref().current_entity);
    let Some(physics) = call.physics() else {
        return Ok(Dynamic::UNIT);
    };
    let mask = mask.map(|m| read_mask(call, physics, m)).transpose()?;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-8 {
        return Ok(Dynamic::UNIT);
    }
    Ok(physics
        .raycast_masked(
            [ox as f32, oy as f32, 0.0],
            [(dx / len) as f32, (dy / len) as f32, 0.0],
            max_dist as f32,
            exclude,
            mask,
        )
        .map_or(Dynamic::UNIT, hit_2d))
}

/// A character's move from a script, from where the entity stands now.
fn move_by(
    call: &Call,
    id: flint_core::EntityId,
    (dx, dy, dz): (f64, f64, f64),
    mask: Option<&Dynamic>,
) -> CallResult<Dynamic> {
    let dt = call.ctx_ref().delta_time as f32;
    let Some(physics) = call.physics() else {
        return Ok(Dynamic::UNIT);
    };
    let mask = mask.map(|m| read_mask(call, physics, m)).transpose()?;
    // The entity's position in the world, which sees a set_position made
    // earlier this frame
    let Some(t) = call.world()?.get_transform(id) else {
        return Ok(Dynamic::UNIT);
    };
    let current = [t.position.x, t.position.y, t.position.z];
    Ok(
        match physics.move_character_masked(
            id,
            current,
            [dx as f32, dy as f32, dz as f32],
            dt,
            mask,
        ) {
            Some(result) => {
                let mut map = Map::new();
                map.insert("x".into(), Dynamic::from(result.position[0] as f64));
                map.insert("y".into(), Dynamic::from(result.position[1] as f64));
                map.insert("z".into(), Dynamic::from(result.position[2] as f64));
                map.insert("grounded".into(), Dynamic::from(result.grounded));
                Dynamic::from(map)
            }
            None => Dynamic::UNIT,
        },
    )
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("physics");

    m.func(
        "raycast",
        "The nearest collider a ray meets, the calling entity's own excluded.",
    )
    .arg("ox", Num)
    .arg("oy", Num)
    .arg("oz", Num)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .arg("max_dist", Num)
    .returns(MapKind, HIT_3D)
    .run(|call, (ox, oy, oz, dx, dy, dz, max_dist)| {
        cast_3d(call, (ox, oy, oz), (dx, dy, dz), max_dist, None)
    });

    m.func(
        "raycast",
        "The nearest collider on a layer of `mask` (a layer's name or an array of names) a ray meets, the calling entity's own excluded.",
    )
    .arg("ox", Num)
    .arg("oy", Num)
    .arg("oz", Num)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .arg("max_dist", Num)
    .arg("mask", Any)
    .returns(MapKind, HIT_3D)
    .run(|call, (ox, oy, oz, dx, dy, dz, max_dist, mask)| {
        cast_3d(call, (ox, oy, oz), (dx, dy, dz), max_dist, Some(&mask))
    });

    m.func(
        "collision_layers",
        "The collision layers colliders and masks may name: `default`, then the project's.",
    )
    .returns(Array, "their names, `default` first; empty with no physics")
    .run(|call, ()| match call.physics() {
        Some(p) => p
            .layers()
            .names()
            .map(|n| Dynamic::from(n.to_string()))
            .collect(),
        None => rhai::Array::new(),
    });

    let mut m = api.module("physics_2d");

    m.func(
        "overlap_rect",
        "Every entity whose 2D collider overlaps a rectangle on the sprite plane.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .returns(Array, "their ids, in name order; empty with no physics")
    .run(|call, (x, y, w, h)| match call.physics() {
        Some(p) => {
            let hits = p.overlap_rect(x as f32, y as f32, w as f32, h as f32);
            // Rapier answers in its own order, which follows body insertion
            // (hash order, #688): the same contract as every entity list.
            let hits = match call.world() {
                Ok(world) => world.in_name_order(hits),
                Err(_) => hits,
            };
            hits.into_iter()
                .map(|eid| Dynamic::from(eid.raw() as i64))
                .collect()
        }
        None => rhai::Array::new(),
    });

    m.func(
        "raycast_2d",
        "The nearest 2D collider a ray on the sprite plane meets, the calling entity's own excluded.",
    )
    .arg("ox", Num)
    .arg("oy", Num)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("max_dist", Num)
    .returns(MapKind, HIT_2D)
    .run(|call, (ox, oy, dx, dy, max_dist)| cast_2d(call, (ox, oy), (dx, dy), max_dist, None));

    m.func(
        "raycast_2d",
        "The nearest 2D collider on a layer of `mask` (a layer's name or an array of names) a ray on the sprite plane meets, the calling entity's own excluded.",
    )
    .arg("ox", Num)
    .arg("oy", Num)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("max_dist", Num)
    .arg("mask", Any)
    .returns(MapKind, HIT_2D)
    .run(|call, (ox, oy, dx, dy, max_dist, mask)| {
        cast_2d(call, (ox, oy), (dx, dy), max_dist, Some(&mask))
    });

    m.func(
        "set_velocity_2d",
        "Set a 2D body's velocity, applied after the script batch.",
    )
    .arg("id", Entity)
    .arg("vx", Num)
    .arg("vy", Num)
    .run(|call, (entity, vx, vy)| {
        call.ctx()
            .commands
            .push(ScriptCommand::SetVelocity2D { entity, vx, vy });
    });

    m.func("get_velocity_2d", "A 2D body's velocity.")
        .arg("id", Entity)
        .returns(
            MapKind,
            "#{vx, vy}, or () when it has no 2D body or there is no physics",
        )
        .run(
            |call, (id,)| match call.physics().and_then(|p| p.get_velocity_2d(id)) {
                Some(vel) => {
                    let mut map = Map::new();
                    map.insert("vx".into(), Dynamic::from(vel[0] as f64));
                    map.insert("vy".into(), Dynamic::from(vel[1] as f64));
                    Dynamic::from(map)
                }
                None => Dynamic::UNIT,
            },
        );

    // Back in `physics`: the character controller's move and collider
    // shapes, declared after the 2D functions only because they were in
    // the draw group before ADR 0116
    let mut m = api.module("physics");

    const MOVED: &str =
        "#{x, y, z, grounded}: where it ended up; () with no physics, transform or character collider";

    m.func(
        "move_character",
        "Move a character's collider by a step, sliding along what its collider's mask meets, from where the entity stands now.",
    )
    .arg("id", Entity)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .returns(MapKind, MOVED)
    .run(|call, (id, dx, dy, dz)| move_by(call, id, (dx, dy, dz), None));

    m.func(
        "move_character",
        "Move a character's collider by a step, sliding along what is on a layer of `mask` (a layer's name or an array of names) in place of its collider's mask.",
    )
    .arg("id", Entity)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .arg("mask", Any)
    .returns(MapKind, MOVED)
    .run(|call, (id, dx, dy, dz, mask)| move_by(call, id, (dx, dy, dz), Some(&mask)));

    m.func("get_collider_extents", "The shape and size of an entity's collider.")
        .arg("id", Entity)
        .returns(
            MapKind,
            "#{shape: \"box\", half_x, half_y, half_z}, #{shape: \"sphere\", radius}, #{shape: \"capsule\"|\"cylinder\", radius, half_height} or #{shape: \"mesh\"|\"convex\", half_x, half_y, half_z, centre_x, centre_y, centre_z} (its box); () with no collider or no physics",
        )
        .run(|call, (id,)| {
            use flint_physics::ColliderExtents as E;
            let Some(extents) = call.physics().and_then(|p| p.get_entity_collider_extents(id)) else {
                return Dynamic::UNIT;
            };
            let mut map = Map::new();
            let mut put = |k: &str, v: f32| {
                map.insert(k.into(), Dynamic::from(v as f64));
            };
            let shape = match extents {
                E::Box { half_extents } => {
                    put("half_x", half_extents[0]);
                    put("half_y", half_extents[1]);
                    put("half_z", half_extents[2]);
                    "box"
                }
                E::Sphere { radius } => {
                    put("radius", radius);
                    "sphere"
                }
                E::Capsule {
                    radius,
                    half_height,
                } => {
                    put("radius", radius);
                    put("half_height", half_height);
                    "capsule"
                }
                E::Cylinder {
                    radius,
                    half_height,
                } => {
                    put("radius", radius);
                    put("half_height", half_height);
                    "cylinder"
                }
                E::Mesh {
                    half_extents,
                    centre,
                }
                | E::Convex {
                    half_extents,
                    centre,
                } => {
                    put("half_x", half_extents[0]);
                    put("half_y", half_extents[1]);
                    put("half_z", half_extents[2]);
                    put("centre_x", centre[0]);
                    put("centre_y", centre[1]);
                    put("centre_z", centre[2]);
                    if matches!(extents, E::Mesh { .. }) {
                        "mesh"
                    } else {
                        "convex"
                    }
                }
            };
            map.insert("shape".into(), Dynamic::from(shape.to_string()));
            Dynamic::from(map)
        });
}
