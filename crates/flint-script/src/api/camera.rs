//! The `camera` module: where the 3D camera is and looks, and the
//! overrides a script puts on it, applied by the host after the script
//! batch (ADR 0116). The sprite plane's follow camera is `camera_2d`.

use super::builder::kind::{Bool, Map, Num};
use super::builder::ScriptApi;
use rhai::Dynamic;

fn xyz(v: [f32; 3]) -> rhai::Map {
    let mut map = rhai::Map::new();
    map.insert("x".into(), Dynamic::from(v[0] as f64));
    map.insert("y".into(), Dynamic::from(v[1] as f64));
    map.insert("z".into(), Dynamic::from(v[2] as f64));
    map
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("camera");

    m.func("get_camera_position", "The camera's position in the world.")
        .returns(Map, "#{x, y, z}")
        .run(|call, ()| xyz(call.ctx_ref().camera_position));

    m.func("get_camera_direction", "The camera's forward vector.")
        .returns(Map, "#{x, y, z}, unit length")
        .run(|call, ()| xyz(call.ctx_ref().camera_direction));

    m.func(
        "set_camera_position",
        "Put the camera at a point, overriding its rig.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .run(|call, (x, y, z)| {
        call.ctx().camera_position_override = Some([x as f32, y as f32, z as f32]);
    });

    m.func(
        "set_camera_target",
        "Point the camera at a point, overriding its rig.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .run(|call, (x, y, z)| {
        call.ctx().camera_target_override = Some([x as f32, y as f32, z as f32]);
    });

    m.func(
        "set_camera_fov",
        "The camera's vertical field of view, degrees.",
    )
    .arg("fov", Num)
    .run(|call, (fov,)| {
        call.ctx().camera_fov_override = Some(fov as f32);
    });

    m.func(
        "set_camera_orthographic",
        "Switch the camera between orthographic (true) and perspective (false).",
    )
    .arg("enabled", Bool)
    .run(|call, (enabled,)| {
        call.ctx().camera_orthographic_override = Some(enabled);
    });

    m.func(
        "set_camera_ortho_height",
        "The orthographic camera's half-height in world units.",
    )
    .arg("height", Num)
    .run(|call, (height,)| {
        call.ctx().camera_ortho_height_override = Some(height as f32);
    });

    m.func(
        "set_camera_roll",
        "Roll the camera about its view axis, radians.",
    )
    .arg("radians", Num)
    .run(|call, (radians,)| {
        call.ctx().camera_roll_override = Some(radians as f32);
    });
}
