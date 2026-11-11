//! The `view` module: between the screen and the world, with the camera
//! the last frame rendered with (ADR 0087): projecting a world point,
//! finding the ground or a mesh under a screen point, and casting rays at
//! the meshes drawn as ray targets (ADR 0105). Screen points are logical
//! points, the mouse_x()/mouse_y() and draw space.

use super::builder::kind::{Any, Map as MapKind, Num};
use super::builder::ScriptApi;
use crate::projector::{project_clip, View};
use rhai::{Dynamic, Map};

/// A mesh ray's hit as scripts see it: the keys `raycast` answers with.
fn ray_hit_dynamic(hit: Option<flint_core::ray_mesh::RayHit>) -> Dynamic {
    let Some(hit) = hit else {
        return Dynamic::UNIT;
    };
    let mut map = Map::new();
    map.insert("entity".into(), Dynamic::from(hit.entity.raw() as i64));
    map.insert("distance".into(), Dynamic::from(hit.distance as f64));
    for (k, axis) in ["x", "y", "z"].iter().enumerate() {
        map.insert(
            format!("point_{axis}").into(),
            Dynamic::from(hit.point[k] as f64),
        );
        map.insert(
            format!("normal_{axis}").into(),
            Dynamic::from(hit.normal[k] as f64),
        );
    }
    Dynamic::from(map)
}

fn floats(v: &[f32]) -> Dynamic {
    Dynamic::from_array(v.iter().map(|f| Dynamic::from(*f as f64)).collect())
}

const HIT_KEYS: &str =
    "#{entity, distance, point_x, point_y, point_z, normal_x, normal_y, normal_z}, the normal facing back along the ray; () when it meets none";

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("view");

    m.func(
        "world_to_screen",
        "Where a world point lands on the screen, with the camera the last frame rendered with.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .returns(
        MapKind,
        "#{x, y, depth, visible}; visible is false behind the near plane, where x and y mean nothing",
    )
    .run(|call, (x, y, z)| {
        // The same answer `Projector` gives native code (ADR 0087)
        let view = View::of(call.ctx_ref());
        let p = [x as f32, y as f32, z as f32];
        let (sx, sy, depth, visible) = match view.world_to_screen(p) {
            Some(p) => (p.x, p.y, p.depth, true),
            None => (0.0, 0.0, project_clip(&view.view_proj, p)[3], false),
        };
        let mut map = Map::new();
        map.insert("x".into(), Dynamic::from(sx as f64));
        map.insert("y".into(), Dynamic::from(sy as f64));
        map.insert("depth".into(), Dynamic::from(depth as f64));
        map.insert("visible".into(), Dynamic::from(visible));
        map
    });

    m.func(
        "screen_to_ground",
        "Where the ray through a screen point meets the plane y = plane_y.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("plane_y", Num)
    .returns(
        Any,
        "[x, z], or () where the view doesn't reach the plane (sky, a level orthographic camera, a plane behind)",
    )
    .run(|call, (x, y, plane_y)| {
        let c = call.ctx_ref();
        match flint_core::projection::screen_to_ground(
            &c.camera_view_proj,
            x as f32,
            y as f32,
            c.screen_width,
            c.screen_height,
            plane_y as f32,
        ) {
            Some(xz) => floats(&xz),
            None => Dynamic::UNIT,
        }
    });

    m.func(
        "screen_to_heightfield",
        "Where the ray through a screen point first comes down onto the game's registered ground field (ADR 0095).",
    )
    .arg("x", Num)
    .arg("y", Num)
    .returns(
        Any,
        "[x, y, z], or () with no ground registered or where the ray misses it; fall back to screen_to_ground",
    )
    .run(|call, (x, y)| {
        let c = call.ctx_ref();
        let view = View::of(c);
        match c
            .ground
            .as_ref()
            .and_then(|g| view.screen_to_heightfield(x as f32, y as f32, g))
        {
            Some(xyz) => floats(&xyz),
            None => Dynamic::UNIT,
        }
    });

    m.func(
        "ground_height",
        "The registered ground field's world y at (x, z); terrain_height reads a scene's terrain.",
    )
    .arg("x", Num)
    .arg("z", Num)
    .returns(
        Any,
        "a float, or () with no ground registered or off its edge",
    )
    .run(|call, (x, z)| {
        match call
            .ctx_ref()
            .ground
            .as_ref()
            .and_then(|g| g.height_at(x as f32, z as f32))
        {
            Some(h) => Dynamic::from(h as f64),
            None => Dynamic::UNIT,
        }
    });

    m.func(
        "raycast_meshes",
        "The nearest mesh drawn as a ray target that a ray meets within max_dist, where the last frame drew it.",
    )
    .arg("ox", Num)
    .arg("oy", Num)
    .arg("oz", Num)
    .arg("dx", Num)
    .arg("dy", Num)
    .arg("dz", Num)
    .arg("max_dist", Num)
    .returns(Any, HIT_KEYS)
    .run(|call, (ox, oy, oz, dx, dy, dz, max_dist)| {
        let o = [ox as f32, oy as f32, oz as f32];
        let d = [dx as f32, dy as f32, dz as f32];
        ray_hit_dynamic(call.ctx_ref().ray_scene.cast(o, d, max_dist as f32))
    });

    m.func(
        "screen_to_mesh",
        "The nearest mesh drawn as a ray target under a screen point: picking a wall or a roof, not the ground.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .returns(Any, HIT_KEYS)
    .run(|call, (x, y)| {
        let c = call.ctx_ref();
        let view = View::of(c);
        ray_hit_dynamic(
            view.screen_ray(x as f32, y as f32)
                .and_then(|(o, d)| c.ray_scene.cast(o, d, f32::INFINITY)),
        )
    });
}
