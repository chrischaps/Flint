//! The `terrain` module: the scene terrain's height, by the sampler the
//! host installs (`ScriptSystem::set_terrain_height_fn`). ADR 0116.

use super::builder::kind::Num;
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("terrain");

    m.func(
        "terrain_height",
        "The world-space height of the terrain at world (x, z).",
    )
    .arg("x", Num)
    .arg("z", Num)
    .returns(Num, "metres; 0.0 when no terrain is loaded")
    .run(|call, (x, z)| match call.ctx_ref().terrain_height_fn {
        Some(ref height) => height(x as f32, z as f32) as f64,
        None => 0.0,
    });
}
