//! The `ocean` module: the scene ocean's surface, sampled from the same
//! Gerstner spectrum the renderer displaces vertices with
//! (`flint_geometry::ocean` is the one source, and `total_time` is the clock
//! the player hands the renderer as its ocean time). ADR 0116.
//!
//! The spectrum is cached, and made again only when the `ocean`
//! component's parameters change. Without an ocean every sample is the
//! flat sea: height 0, velocity 0, normal straight up.

use super::builder::kind::{Map as MapKind, Num};
use super::builder::ScriptApi;
use flint_ecs::FlintWorld;
use flint_geometry::ocean::{OceanParams, WaveSpectrum};
use rhai::{Dynamic, Map};
use std::sync::{Arc, Mutex};

/// The spectrum the module samples, kept across calls.
type SpectrumCache = Arc<Mutex<Option<WaveSpectrum>>>;

/// The first `ocean` component's parameters, or `None` without one.
pub(crate) fn scene_ocean(world: &FlintWorld) -> Option<OceanParams> {
    let id = world
        .entities_with_component(flint_geometry::components::OCEAN)
        .iter()
        .next()
        .copied()?;
    let component = world
        .get_components(id)?
        .get(flint_geometry::components::OCEAN)?
        .clone();
    Some(OceanParams::from_component(&component))
}

/// `f` of the spectrum for `params`, made again only when they changed.
fn sample<R>(cache: &SpectrumCache, params: &OceanParams, f: impl FnOnce(&WaveSpectrum) -> R) -> R {
    let mut cached = crate::lock_or_recover(cache);
    if cached.as_ref().is_none_or(|s| &s.params != params) {
        *cached = Some(WaveSpectrum::generate(params));
    }
    f(cached.as_ref().expect("made just above"))
}

pub(crate) fn register(api: &mut ScriptApi) {
    let cache: SpectrumCache = Arc::new(Mutex::new(None));
    let mut m = api.module("ocean");

    let c = cache.clone();
    m.func(
        "ocean_height",
        "The ocean surface's height at world (x, z), as drawn this frame.",
    )
    .arg("x", Num)
    .arg("z", Num)
    .returns(Num, "metres; 0.0 without an ocean")
    .run(move |call, (x, z)| {
        let t = call.ctx_ref().total_time;
        let Some(params) = call.ctx_ref().try_world().and_then(scene_ocean) else {
            return 0.0;
        };
        sample(&c, &params, |s| {
            s.sample_height(x as f32, z as f32, t) as f64
        })
    });

    let c = cache.clone();
    m.func(
        "ocean_velocity_y",
        "The ocean surface's vertical velocity at world (x, z): the impact signal beside ocean_height.",
    )
    .arg("x", Num)
    .arg("z", Num)
    .returns(Num, "m/s; 0.0 without an ocean")
    .run(move |call, (x, z)| {
        let t = call.ctx_ref().total_time;
        let Some(params) = call.ctx_ref().try_world().and_then(scene_ocean) else {
            return 0.0;
        };
        sample(&c, &params, |s| {
            s.sample_velocity_y(x as f32, z as f32, t) as f64
        })
    });

    m.func(
        "ocean_normal",
        "The ocean surface's normal at world (x, z).",
    )
    .arg("x", Num)
    .arg("z", Num)
    .returns(MapKind, "#{x, y, z}; #{0, 1, 0} without an ocean")
    .run(move |call, (x, z)| {
        let t = call.ctx_ref().total_time;
        let n = match call.ctx_ref().try_world().and_then(scene_ocean) {
            Some(params) => sample(&cache, &params, |s| s.sample_normal(x as f32, z as f32, t)),
            None => [0.0, 1.0, 0.0],
        };
        let mut map = Map::new();
        map.insert("x".into(), Dynamic::from(n[0] as f64));
        map.insert("y".into(), Dynamic::from(n[1] as f64));
        map.insert("z".into(), Dynamic::from(n[2] as f64));
        map
    });
}

#[cfg(test)]
mod tests {
    use crate::context::WorldScope;
    use crate::engine::ScriptEngine;
    use flint_ecs::FlintWorld;

    #[test]
    fn without_an_ocean_the_sea_is_flat_and_with_one_it_moves() {
        let engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let eval = |world: &mut FlintWorld, src: &str| -> rhai::Map {
            let _scope = WorldScope::new(&engine.ctx, world);
            engine.rhai_engine().eval(src).unwrap()
        };
        let flat = eval(
            &mut world,
            "#{ h: ocean_height(3, 4), v: ocean_velocity_y(3.0, 4), n: ocean_normal(3, 4.0) }",
        );
        assert_eq!(flat["h"].as_float().unwrap(), 0.0);
        assert_eq!(flat["v"].as_float().unwrap(), 0.0);
        let n = flat["n"].clone().cast::<rhai::Map>();
        assert_eq!(n["y"].as_float().unwrap(), 1.0);

        let sea = world.spawn("sea").unwrap();
        world
            .set_field(sea, "ocean", "amplitude", toml::Value::Float(1.0))
            .unwrap();
        crate::lock_or_recover(&engine.ctx).total_time = 2.5;
        let heights: Vec<f64> = (0..8)
            .map(|i| {
                eval(&mut world, &format!("#{{ h: ocean_height({}, 0) }}", i * 7))["h"]
                    .as_float()
                    .unwrap()
            })
            .collect();
        assert!(
            heights.iter().any(|h| h.abs() > 1e-4),
            "a sea with waves: {heights:?}"
        );
        // The same point at the same time is the same height: the cached
        // spectrum is the one made from these parameters
        let again = eval(&mut world, "#{ h: ocean_height(14, 0) }")["h"]
            .as_float()
            .unwrap();
        assert_eq!(again, heights[2]);
    }
}
