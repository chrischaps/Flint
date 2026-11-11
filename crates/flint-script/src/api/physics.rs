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
}
