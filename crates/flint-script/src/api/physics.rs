//! The `physics` and `physics_2d` modules: rays against colliders, the
//! sprite plane's rectangle query and 2D bodies' velocities (ADR 0116).

use super::builder::kind::{Array, Entity, Map as MapKind, Num};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;
use rhai::{Dynamic, Map};

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
    .returns(
        MapKind,
        "#{entity, distance, point_x, point_y, point_z, normal_x, normal_y, normal_z}, or () for no hit or no physics",
    )
    .run(|call, (ox, oy, oz, dx, dy, dz, max_dist)| {
        let exclude = Some(call.ctx_ref().current_entity);
        let Some(physics) = call.physics() else {
            return Dynamic::UNIT;
        };
        match physics.raycast(
            [ox as f32, oy as f32, oz as f32],
            [dx as f32, dy as f32, dz as f32],
            max_dist as f32,
            exclude,
        ) {
            Some(hit) => {
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
            None => Dynamic::UNIT,
        }
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
    .returns(Array, "their ids; empty with no physics")
    .run(|call, (x, y, w, h)| match call.physics() {
        Some(p) => p
            .overlap_rect(x as f32, y as f32, w as f32, h as f32)
            .into_iter()
            .map(|eid| Dynamic::from(eid.raw() as i64))
            .collect(),
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
    .returns(
        MapKind,
        "#{entity, distance, point_x, point_y, normal_x, normal_y}, or () for no hit or no physics",
    )
    .run(|call, (ox, oy, dx, dy, max_dist)| {
        let exclude = Some(call.ctx_ref().current_entity);
        let Some(physics) = call.physics() else {
            return Dynamic::UNIT;
        };
        match physics.raycast_2d(
            ox as f32,
            oy as f32,
            dx as f32,
            dy as f32,
            max_dist as f32,
            exclude,
        ) {
            Some(hit) => {
                let mut map = Map::new();
                map.insert("entity".into(), Dynamic::from(hit.entity_id.raw() as i64));
                map.insert("distance".into(), Dynamic::from(hit.distance as f64));
                map.insert("point_x".into(), Dynamic::from(hit.point[0] as f64));
                map.insert("point_y".into(), Dynamic::from(hit.point[1] as f64));
                map.insert("normal_x".into(), Dynamic::from(hit.normal[0] as f64));
                map.insert("normal_y".into(), Dynamic::from(hit.normal[1] as f64));
                Dynamic::from(map)
            }
            None => Dynamic::UNIT,
        }
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

    m.func(
        "move_character",
        "Move a character's collider by a step, sliding along what it hits, from where the entity stands now.",
    )
    .arg("id", Entity)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .returns(
        MapKind,
        "#{x, y, z, grounded}: where it ended up; () with no physics, transform or character collider",
    )
    .run(|call, (id, dx, dy, dz)| {
        let dt = call.ctx_ref().delta_time as f32;
        let Some(physics) = call.physics() else {
            return Ok(Dynamic::UNIT);
        };
        // The entity's position in the world, which sees a set_position
        // made earlier this frame
        let Some(t) = call.world()?.get_transform(id) else {
            return Ok(Dynamic::UNIT);
        };
        let current = [t.position.x, t.position.y, t.position.z];
        Ok(
            match physics.move_character_shape(id, current, [dx as f32, dy as f32, dz as f32], dt) {
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
    });

    m.func("get_collider_extents", "The shape and size of an entity's collider.")
        .arg("id", Entity)
        .returns(
            MapKind,
            "#{shape: \"box\", half_x, half_y, half_z}, #{shape: \"sphere\", radius} or #{shape: \"capsule\"|\"cylinder\", radius, half_height}; () with no collider or no physics",
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
            };
            map.insert("shape".into(), Dynamic::from(shape.to_string()));
            Dynamic::from(map)
        });
}
