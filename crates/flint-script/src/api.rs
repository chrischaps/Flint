//! Rhai API function registration
//!
//! All functions accessible from Rhai scripts are registered here.
//! They access the world through the shared ScriptCallContext.
//!
//! ADR 0116 moves them, a module at a time, to `api/<module>.rs`, declared
//! through the builder (`api/builder.rs`) into the registry
//! (`api/registry.rs`) that `flint script api` lists. Done: `entity`,
//! `input`, `time`, `audio`, `animation`, `physics` (and `physics_2d`),
//! `camera`, `post_process`, `math`, `event` and `log`; the
//! `register_*_api` functions below are the rest (#697).

use crate::context::{DrawCommand, LogLevel, ScriptCallContext, ScriptCommand};
use crate::projector::{clip_to_screen, project_clip, View};
use flint_core::components as comp;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::toml_util::toml_f64;
use flint_core::vfs;
use flint_core::EntityId;
use rhai::{Array, Dynamic, Engine, ImmutableString, Map};
use std::sync::{Arc, Mutex};

mod animation;
mod audio;
pub mod builder;
mod camera;
#[cfg(test)]
mod declared_tests;
mod entity;
mod event;
mod input;
mod log;
mod math;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod overhead;
mod physics;
mod post_process;
pub mod registry;
mod time;

pub use builder::{Call, CallError, CallResult, Func, Module, ScriptApi};
pub use registry::ApiRegistry;

/// Register all API functions on the Rhai engine
///
/// The declared modules (ADR 0116) go through the builder and into the
/// registry; the rest still register on the engine directly, and are
/// listed as undeclared until step 2 ports them (#697).
pub fn register_all(api: &mut ScriptApi, random: &crate::random::ScriptRandom) {
    entity::register(api);
    input::register(api);
    time::register(api);
    audio::register(api);
    animation::register(api);
    physics::register(api);
    camera::register(api);
    post_process::register(api);
    math::register(api);
    random.declare(api);
    event::register(api);
    log::register(api);
    let ctx = api.ctx().clone();
    let engine = api.engine_mut();
    register_spline_api(engine, ctx.clone());
    register_ui_api(engine, ctx.clone());
    register_data_ui_api(engine, ctx.clone());
    register_particle_api(engine, ctx.clone());
    register_style_api(engine, ctx.clone());
    register_state_api(engine, ctx.clone());
    register_scene_api(engine, ctx.clone());
    register_wooze_api(engine, ctx.clone());
    register_persistence_api(engine, ctx.clone());
    register_data_api(engine, ctx.clone());
    register_terrain_api(engine, ctx.clone());
    register_sprite_api(engine, ctx.clone());
    register_sprite_animation_api(engine, ctx.clone());
    register_touch_api(engine, ctx.clone());
    register_camera_2d_api(engine, ctx.clone());
    register_chunk_api(engine, ctx.clone());
    register_screen_ui_api(engine, ctx.clone());
    register_conducted_api(engine, ctx.clone());
    register_ocean_api(engine, ctx.clone());
    register_quality_api(engine, ctx.clone());
    register_expression_api(engine, ctx);
}

// ─── Quality settings API (FlintKart#55) ──────────────────
//
// `set_quality(#{ msaa: 4, shadow_cascades: 2 })` changes the renderer's
// load levers from the next frame; `get_quality()` reads them back;
// `gpu_info()` says what the GPU is. Keys and ranges are
// `flint_core::quality::QualitySettings`'.

/// A script value as a quality value: bools, integers and floats.
fn quality_value(v: &Dynamic) -> Option<flint_core::quality::QualityValue> {
    use flint_core::quality::QualityValue;
    if let Ok(b) = v.as_bool() {
        Some(QualityValue::Bool(b))
    } else if let Ok(i) = v.as_int() {
        Some(QualityValue::Int(i))
    } else if let Ok(f) = v.as_float() {
        Some(QualityValue::Float(f))
    } else {
        None
    }
}

/// A quality value as a script value.
fn quality_dynamic(v: flint_core::quality::QualityValue) -> Dynamic {
    use flint_core::quality::QualityValue;
    match v {
        QualityValue::Bool(b) => b.into(),
        QualityValue::Int(i) => i.into(),
        QualityValue::Float(f) => f.into(),
    }
}

/// Merge `settings`' keys into `c.quality` and ask for it to be applied.
/// True when every key was taken; a bad key is warned about and skipped.
pub(crate) fn request_quality(c: &mut ScriptCallContext, settings: &Map) -> bool {
    let mut all_taken = true;
    let mut quality = c.quality;
    for (key, value) in settings {
        let result = match quality_value(value) {
            Some(v) => quality.set(key.as_str(), v),
            None => Err(flint_core::quality::QualityError::Malformed(format!(
                "{key}={value}"
            ))),
        };
        if let Err(e) = result {
            tracing::warn!("set_quality: {e}");
            all_taken = false;
        }
    }
    if quality != c.quality {
        c.quality = quality;
        c.quality_requested = true;
    }
    all_taken
}

fn register_quality_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // set_quality(map) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("set_quality", move |settings: Map| -> bool {
            let mut c = crate::lock_or_recover(&ctx);
            request_quality(&mut c, &settings)
        });
    }

    // set_quality(key, value) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("set_quality", move |key: &str, value: Dynamic| -> bool {
            let mut c = crate::lock_or_recover(&ctx);
            let mut settings = Map::new();
            settings.insert(key.into(), value);
            request_quality(&mut c, &settings)
        });
    }

    // get_quality() -> map of every setting
    {
        let ctx = ctx.clone();
        engine.register_fn("get_quality", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            c.quality
                .entries()
                .into_iter()
                .map(|(k, v)| (k.into(), quality_dynamic(v)))
                .collect()
        });
    }

    // get_quality(key) -> the value, or () for an unknown key
    {
        let ctx = ctx.clone();
        engine.register_fn("get_quality", move |key: &str| -> Dynamic {
            let c = crate::lock_or_recover(&ctx);
            c.quality.get(key).map_or(Dynamic::UNIT, quality_dynamic)
        });
    }

    // gpu_info() -> #{ name, vendor, device_type, backend, driver, class, platform }
    {
        let ctx = ctx.clone();
        engine.register_fn("gpu_info", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let g = &c.gpu_info;
            let mut map = Map::new();
            for (k, v) in [
                ("name", &g.name),
                ("vendor", &g.vendor),
                ("device_type", &g.device_type),
                ("backend", &g.backend),
                ("driver", &g.driver),
                ("class", &g.class),
                ("platform", &g.platform),
            ] {
                map.insert(k.into(), v.clone().into());
            }
            map
        });
    }
}

// ─── Conducted-parameters API (ADR 0020) ──────────────────
//
// The music session's per-frame state for scene bindings: lean, target,
// coherence, musical position, judged pulses, and the ladder/reintegration
// visual ramps. Neutral defaults when no session is active (a clean,
// settled world), so scripts never branch on session existence.

fn register_conducted_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // conducted_lean() / conducted_target() -> Map #{x, y}
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_lean", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.conducted.lean[0]));
            map.insert("y".into(), Dynamic::from(c.conducted.lean[1]));
            map
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_target", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.conducted.target[0]));
            map.insert("y".into(), Dynamic::from(c.conducted.target[1]));
            map
        });
    }

    // conducted_next_target() -> Map #{x, y, beats}: the next authored lean
    // key and suite beats until its anchor (ADR 0023). beats = 1e6 (and
    // x/y = the current target) when nothing is upcoming.
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_next_target", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.conducted.next_target[0]));
            map.insert("y".into(), Dynamic::from(c.conducted.next_target[1]));
            map.insert("beats".into(), Dynamic::from(c.conducted.next_target_beats));
            map
        });
    }

    // conducted_next_pulse() -> Map #{beats, open}: suite beats until the
    // next unconsumed judgment window's anchor (any kind) and whether that
    // window is open right now (P5 immediate-feedback pass). beats = 1e6
    // when nothing is upcoming (and in the neutral no-session state).
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_next_pulse", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("beats".into(), Dynamic::from(c.conducted.next_pulse_beats));
            map.insert("open".into(), Dynamic::from(c.conducted.pulse_window_open));
            map
        });
    }

    // Scalar getters, all f64 (Rhai has no implicit numeric coercion).
    macro_rules! scalar_getter {
        ($name:literal, $field:ident) => {{
            let ctx = ctx.clone();
            engine.register_fn($name, move || -> f64 {
                let c = crate::lock_or_recover(&ctx);
                c.conducted.$field
            });
        }};
    }
    scalar_getter!("conducted_coherence", coherence);
    scalar_getter!("conducted_beat_phase", beat_phase);
    scalar_getter!("conducted_bar_phase", bar_phase);
    scalar_getter!("conducted_beat", beat);
    scalar_getter!("conducted_desaturate", desaturate);
    scalar_getter!("conducted_blur", blur);
    scalar_getter!("conducted_chromatic", chromatic);
    scalar_getter!("conducted_reassembly", reassembly);
    scalar_getter!("conducted_rewind", rewind);

    // conducted_bar() -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_bar", move || -> i64 {
            let c = crate::lock_or_recover(&ctx);
            c.conducted.bar
        });
    }

    // conducted_section() -> String ("" when none)
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_section", move || -> String {
            let c = crate::lock_or_recover(&ctx);
            c.conducted.section.clone()
        });
    }

    // conducted_pulses() -> Array of Map #{age: f64, err_ms: f64, kind: string}
    // Pulses judged this frame — empty most frames.
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_pulses", move || -> rhai::Array {
            let c = crate::lock_or_recover(&ctx);
            c.conducted
                .pulses
                .iter()
                .map(|p| {
                    let mut map = Map::new();
                    map.insert("age".into(), Dynamic::from(p.age));
                    map.insert("err_ms".into(), Dynamic::from(p.err_ms));
                    map.insert("kind".into(), Dynamic::from(p.kind.clone()));
                    Dynamic::from_map(map)
                })
                .collect()
        });
    }

    // conducted_sway() -> Map #{x, y} (zeros under the prototype input map)
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_sway", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.conducted.sway[0]));
            map.insert("y".into(), Dynamic::from(c.conducted.sway[1]));
            map
        });
    }

    // conducted_pressure_l() / conducted_pressure_r() -> f64 in [0,1]
    scalar_getter!("conducted_pressure_l", pressure_l);
    scalar_getter!("conducted_pressure_r", pressure_r);

    // conducted_cues() -> Array of Map #{name: string, age: f64, params: Map}
    // Chart cues fired this frame (ADR 0033) — empty most frames. Params are
    // the chart's free-form table, flattened to floats/strings/bools.
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_cues", move || -> rhai::Array {
            let c = crate::lock_or_recover(&ctx);
            c.conducted
                .cues
                .iter()
                .map(|cue| {
                    let mut params = Map::new();
                    for (k, v) in &cue.params {
                        let d = match v {
                            crate::context::CueParam::Number(n) => Dynamic::from(*n),
                            crate::context::CueParam::Text(s) => Dynamic::from(s.clone()),
                            crate::context::CueParam::Flag(b) => Dynamic::from(*b),
                        };
                        params.insert(k.as_str().into(), d);
                    }
                    let mut map = Map::new();
                    map.insert("name".into(), Dynamic::from(cue.name.clone()));
                    map.insert("age".into(), Dynamic::from(cue.age));
                    map.insert("params".into(), Dynamic::from_map(params));
                    Dynamic::from_map(map)
                })
                .collect()
        });
    }

    // conducted_no_input() / conducted_preroll() -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_no_input", move || -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.conducted.no_input
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("conducted_preroll", move || -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.conducted.preroll
        });
    }
}

// ─── Ocean API ───────────────────────────────────────────
//
// Samples the same Gerstner spectrum the renderer displaces vertices with
// (flint_core::ocean is the single source of truth, and total_time is the
// same clock the player hands the renderer as ocean_time). The spectrum is
// cached and regenerated only when the `ocean` component's params change.

fn register_ocean_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    use flint_core::ocean::{OceanParams, WaveSpectrum};

    type SpectrumCache = Arc<Mutex<Option<WaveSpectrum>>>;
    let cache: SpectrumCache = Arc::new(Mutex::new(None));

    // Reads the first `ocean` component and returns (spectrum ready?, time).
    fn with_spectrum<R>(
        ctx: &Arc<Mutex<ScriptCallContext>>,
        cache: &Arc<Mutex<Option<WaveSpectrum>>>,
        f: impl FnOnce(&WaveSpectrum, f64) -> R,
    ) -> Option<R> {
        let c = ctx.lock().unwrap();
        let world = unsafe { c.world_ref() };
        let entity_id = world
            .entities_with_component(comp::OCEAN)
            .iter()
            .next()
            .copied()?;
        let ocean_comp = world
            .get_components(entity_id)
            .and_then(|comps| comps.get(comp::OCEAN).cloned())?;
        let params = OceanParams::from_component(&ocean_comp);

        let mut cached = cache.lock().unwrap();
        let needs_regen = cached.as_ref().map(|s| s.params != params).unwrap_or(true);
        if needs_regen {
            *cached = Some(WaveSpectrum::generate(&params));
        }
        Some(f(cached.as_ref().unwrap(), c.total_time))
    }

    // ocean_height(x, z) -> f64 — Eulerian surface height at world (x, z).
    {
        let ctx = ctx.clone();
        let cache = cache.clone();
        engine.register_fn("ocean_height", move |x: f64, z: f64| -> f64 {
            with_spectrum(&ctx, &cache, |spec, t| {
                spec.sample_height(x as f32, z as f32, t) as f64
            })
            .unwrap_or(0.0)
        });
    }

    // ocean_velocity_y(x, z) -> f64 — vertical surface velocity (m/s) at
    // world (x, z). Analytic ∂h/∂t; pairs with ocean_height for impact cues.
    {
        let ctx = ctx.clone();
        let cache = cache.clone();
        engine.register_fn("ocean_velocity_y", move |x: f64, z: f64| -> f64 {
            with_spectrum(&ctx, &cache, |spec, t| {
                spec.sample_velocity_y(x as f32, z as f32, t) as f64
            })
            .unwrap_or(0.0)
        });
    }

    // ocean_normal(x, z) -> #{x, y, z} — surface normal at world (x, z).
    {
        let ctx = ctx.clone();
        let cache = cache.clone();
        engine.register_fn("ocean_normal", move |x: f64, z: f64| -> Map {
            let n = with_spectrum(&ctx, &cache, |spec, t| {
                spec.sample_normal(x as f32, z as f32, t)
            })
            .unwrap_or([0.0, 1.0, 0.0]);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(n[0] as f64));
            map.insert("y".into(), Dynamic::from(n[1] as f64));
            map.insert("z".into(), Dynamic::from(n[2] as f64));
            map
        });
    }
}

// ─── Entity API: api/entity.rs, declared (ADR 0116) ──────

// ─── Spline API ──────────────────────────────────────────

fn register_spline_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // spline_closest_point(spline_entity, x, y, z) -> Map #{t, x, y, z, dist_sq}
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "spline_closest_point",
            move |spline_id: i64, qx: f64, qy: f64, qz: f64| -> Dynamic {
                if spline_id < 0 {
                    return Dynamic::UNIT;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_ref() };
                let eid = EntityId::from_raw(spline_id as u64);
                let comps = match world.get_components(eid) {
                    Some(c) => c,
                    None => return Dynamic::UNIT,
                };
                let sd = match comps.get(comp::SPLINE_DATA) {
                    Some(v) => v,
                    None => return Dynamic::UNIT,
                };
                let table = match sd.as_table() {
                    Some(t) => t,
                    None => return Dynamic::UNIT,
                };

                let count = match table.get("sample_count").and_then(|v| v.as_integer()) {
                    Some(n) if n > 0 => n as usize,
                    _ => return Dynamic::UNIT,
                };

                let px_arr = match table.get("positions_x").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let py_arr = match table.get("positions_y").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let pz_arr = match table.get("positions_z").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let t_arr = match table.get("t_values").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };

                let mut best_dist_sq = f64::MAX;
                let mut best_idx = 0usize;

                for i in 0..count.min(px_arr.len()).min(pz_arr.len()).min(t_arr.len()) {
                    let sx = toml_f64(&px_arr[i]).unwrap_or(0.0);
                    let sy = toml_f64(&py_arr[i]).unwrap_or(0.0);
                    let sz = toml_f64(&pz_arr[i]).unwrap_or(0.0);
                    let dx = qx - sx;
                    let dy = qy - sy;
                    let dz = qz - sz;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best_idx = i;
                    }
                }

                let mut map = Map::new();
                map.insert(
                    "t".into(),
                    Dynamic::from(toml_f64(&t_arr[best_idx]).unwrap_or(0.0)),
                );
                map.insert(
                    "x".into(),
                    Dynamic::from(toml_f64(&px_arr[best_idx]).unwrap_or(0.0)),
                );
                map.insert(
                    "y".into(),
                    Dynamic::from(toml_f64(&py_arr[best_idx]).unwrap_or(0.0)),
                );
                map.insert(
                    "z".into(),
                    Dynamic::from(toml_f64(&pz_arr[best_idx]).unwrap_or(0.0)),
                );
                map.insert("dist_sq".into(), Dynamic::from(best_dist_sq));
                Dynamic::from(map)
            },
        );
    }

    // spline_is_gap(spline_entity, t) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("spline_is_gap", move |spline_id: i64, t: f64| -> bool {
            if spline_id < 0 {
                return false;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(spline_id as u64);
            let comps = match world.get_components(eid) {
                Some(c) => c,
                None => return false,
            };
            let sd = match comps.get(comp::SPLINE_DATA) {
                Some(v) => v,
                None => return false,
            };
            let table = match sd.as_table() {
                Some(t) => t,
                None => return false,
            };

            let gap_count = table
                .get("gap_count")
                .and_then(|v| v.as_integer())
                .unwrap_or(0);
            if gap_count == 0 {
                return false;
            }

            let gap_starts = match table.get("gap_starts").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => return false,
            };
            let gap_ends = match table.get("gap_ends").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => return false,
            };

            let t_wrapped = ((t % 1.0) + 1.0) % 1.0;
            for i in 0..(gap_count as usize)
                .min(gap_starts.len())
                .min(gap_ends.len())
            {
                let start = toml_f64(&gap_starts[i]).unwrap_or(0.0);
                let end = toml_f64(&gap_ends[i]).unwrap_or(0.0);
                if start <= end {
                    if t_wrapped >= start && t_wrapped < end {
                        return true;
                    }
                } else {
                    // Wrap-around range
                    if t_wrapped >= start || t_wrapped < end {
                        return true;
                    }
                }
            }
            false
        });
    }

    // spline_gap_at(spline_entity, t) -> Map #{start_t, end_t} or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("spline_gap_at", move |spline_id: i64, t: f64| -> Dynamic {
            if spline_id < 0 {
                return Dynamic::UNIT;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(spline_id as u64);
            let comps = match world.get_components(eid) {
                Some(c) => c,
                None => return Dynamic::UNIT,
            };
            let sd = match comps.get(comp::SPLINE_DATA) {
                Some(v) => v,
                None => return Dynamic::UNIT,
            };
            let table = match sd.as_table() {
                Some(t) => t,
                None => return Dynamic::UNIT,
            };

            let gap_count = table
                .get("gap_count")
                .and_then(|v| v.as_integer())
                .unwrap_or(0);
            if gap_count == 0 {
                return Dynamic::UNIT;
            }

            let gap_starts = match table.get("gap_starts").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => return Dynamic::UNIT,
            };
            let gap_ends = match table.get("gap_ends").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => return Dynamic::UNIT,
            };

            let t_wrapped = ((t % 1.0) + 1.0) % 1.0;
            for i in 0..(gap_count as usize)
                .min(gap_starts.len())
                .min(gap_ends.len())
            {
                let start = toml_f64(&gap_starts[i]).unwrap_or(0.0);
                let end = toml_f64(&gap_ends[i]).unwrap_or(0.0);
                let in_gap = if start <= end {
                    t_wrapped >= start && t_wrapped < end
                } else {
                    t_wrapped >= start || t_wrapped < end
                };
                if in_gap {
                    let mut map = Map::new();
                    map.insert("start_t".into(), Dynamic::from(start));
                    map.insert("end_t".into(), Dynamic::from(end));
                    return Dynamic::from(map);
                }
            }
            Dynamic::UNIT
        });
    }

    // spline_sample_at(spline_entity, t) -> Map #{x, y, z, fwd_x, fwd_y, fwd_z, right_x, right_y, right_z}
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "spline_sample_at",
            move |spline_id: i64, t: f64| -> Dynamic {
                if spline_id < 0 {
                    return Dynamic::UNIT;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_ref() };
                let eid = EntityId::from_raw(spline_id as u64);
                let comps = match world.get_components(eid) {
                    Some(c) => c,
                    None => return Dynamic::UNIT,
                };
                let sd = match comps.get(comp::SPLINE_DATA) {
                    Some(v) => v,
                    None => return Dynamic::UNIT,
                };
                let table = match sd.as_table() {
                    Some(t) => t,
                    None => return Dynamic::UNIT,
                };

                let count = match table.get("sample_count").and_then(|v| v.as_integer()) {
                    Some(n) if n > 1 => n as usize,
                    _ => return Dynamic::UNIT,
                };

                let px_arr = match table.get("positions_x").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let py_arr = match table.get("positions_y").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let pz_arr = match table.get("positions_z").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };
                let t_arr = match table.get("t_values").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => return Dynamic::UNIT,
                };

                let len = count
                    .min(px_arr.len())
                    .min(py_arr.len())
                    .min(pz_arr.len())
                    .min(t_arr.len());
                if len < 2 {
                    return Dynamic::UNIT;
                }

                // Wrap t into [0, 1) for closed splines
                let t_wrapped = ((t % 1.0) + 1.0) % 1.0;

                // Binary search for the bracketing segment
                let mut lo = 0usize;
                let mut hi = len - 1;
                while lo + 1 < hi {
                    let mid = (lo + hi) / 2;
                    if toml_f64(&t_arr[mid]).unwrap_or(0.0) <= t_wrapped {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }

                let t0 = toml_f64(&t_arr[lo]).unwrap_or(0.0);
                let t1 = toml_f64(&t_arr[hi]).unwrap_or(0.0);
                let seg_len = t1 - t0;
                let frac = if seg_len.abs() > 1e-9 {
                    (t_wrapped - t0) / seg_len
                } else {
                    0.0
                };

                let x0 = toml_f64(&px_arr[lo]).unwrap_or(0.0);
                let y0 = toml_f64(&py_arr[lo]).unwrap_or(0.0);
                let z0 = toml_f64(&pz_arr[lo]).unwrap_or(0.0);
                let x1 = toml_f64(&px_arr[hi]).unwrap_or(0.0);
                let y1 = toml_f64(&py_arr[hi]).unwrap_or(0.0);
                let z1 = toml_f64(&pz_arr[hi]).unwrap_or(0.0);

                let px = x0 + (x1 - x0) * frac;
                let py = y0 + (y1 - y0) * frac;
                let pz = z0 + (z1 - z0) * frac;

                // Forward direction: tangent along the spline
                let mut fx = x1 - x0;
                let mut fy = y1 - y0;
                let mut fz = z1 - z0;
                let flen = (fx * fx + fy * fy + fz * fz).sqrt();
                if flen > 1e-9 {
                    fx /= flen;
                    fy /= flen;
                    fz /= flen;
                }

                // Right vector: cross(forward, up) where up = (0,1,0)
                let mut rx = fz;
                let ry = 0.0_f64;
                let mut rz = -fx;
                let rlen = (rx * rx + rz * rz).sqrt();
                if rlen > 1e-9 {
                    rx /= rlen;
                    rz /= rlen;
                }

                let mut map = Map::new();
                map.insert("x".into(), Dynamic::from(px));
                map.insert("y".into(), Dynamic::from(py));
                map.insert("z".into(), Dynamic::from(pz));
                map.insert("fwd_x".into(), Dynamic::from(fx));
                map.insert("fwd_y".into(), Dynamic::from(fy));
                map.insert("fwd_z".into(), Dynamic::from(fz));
                map.insert("right_x".into(), Dynamic::from(rx));
                map.insert("right_y".into(), Dynamic::from(ry));
                map.insert("right_z".into(), Dynamic::from(rz));
                Dynamic::from(map)
            },
        );
    }
}

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

// ─── Particle API ─────────────────────────────────────────

/// Monotonic handle source for `play_effect` (starts at 1 so 0 means "none").
static NEXT_EFFECT_HANDLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn register_particle_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // emit_burst(entity_id, count) — fire N particles immediately
    {
        let ctx = ctx.clone();
        engine.register_fn("emit_burst", move |entity_id: i64, count: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands
                .push(ScriptCommand::EmitBurst { entity_id, count });
        });
    }

    // start_emitter(entity_id) — set playing = true in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("start_emitter", move |entity_id: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field(
                    comp::PARTICLE_EMITTER,
                    "playing",
                    toml::Value::Boolean(true),
                );
            }
        });
    }

    // stop_emitter(entity_id) — set playing = false in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_emitter", move |entity_id: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field(
                    comp::PARTICLE_EMITTER,
                    "playing",
                    toml::Value::Boolean(false),
                );
            }
        });
    }

    // play_effect(name, x, y, z) -> handle — spawn a detached one-shot
    // instance of a `particles/<name>.particles.toml` effect (ADR 0068).
    // Returns 0 when the command could not be queued.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "play_effect",
            move |name: &str, x: f64, y: f64, z: f64| -> i64 {
                let handle = NEXT_EFFECT_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut c = crate::lock_or_recover(&ctx);
                c.commands.push(ScriptCommand::PlayEffect {
                    handle,
                    name: name.to_string(),
                    position: (x, y, z),
                });
                handle as i64
            },
        );
    }

    // stop_effect(handle) — stop emission; particles already alive finish
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_effect", move |handle: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::StopEffect {
                handle: handle.max(0) as u64,
            });
        });
    }

    // set_effect_param(handle, param, value) — emission_scale | scale |
    // playing (> 0.5) | x | y | z
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_effect_param",
            move |handle: i64, param: &str, value: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.commands.push(ScriptCommand::SetEffectParam {
                    handle: handle.max(0) as u64,
                    param: param.to_string(),
                    value,
                });
            },
        );
    }

    // set_emission_rate(entity_id, rate) — update emission rate in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("set_emission_rate", move |entity_id: i64, rate: f64| {
            let mut c = crate::lock_or_recover(&ctx);
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field(
                    comp::PARTICLE_EMITTER,
                    "emission_rate",
                    toml::Value::Float(rate),
                );
            }
        });
    }
}

// ─── Per-entity styles (#269) ─────────────────────────────
//
// Queue changes to an entity's stylize style (`ascii`, #266). The player
// applies them after the frame's scripts, before the renderer reads the
// world, and fires `style_changed` to `on_event`. A param can change every
// frame: that is a uniform write, never a GPU allocation.

fn register_style_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // set_entity_style(entity_id, style) — draw the entity and its
    // descendants in `style`; settings it had before are kept
    {
        let ctx = ctx.clone();
        engine.register_fn("set_entity_style", move |entity_id: i64, style: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetEntityStyle {
                entity_id,
                style: style.to_string(),
            });
        });
    }

    // set_style_param(entity_id, param, value) — a number (`cell`, `glow`,
    // ...), a string (`ramp`, `color`, ...), `enabled` true/false, `blend`
    // 0..1 (how much of the style shows, #278), or a colour (`tint`: a
    // three-number array or a "#rrggbb" string, #315)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_style_param",
            move |entity_id: i64, param: &str, value: Dynamic| {
                let mut c = crate::lock_or_recover(&ctx);
                let command = match dynamic_to_toml(&value) {
                    Some(value) => ScriptCommand::SetStyleParam {
                        entity_id,
                        param: param.to_string(),
                        value,
                    },
                    None => ScriptCommand::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "set_style_param: `{param}` can't take a {}",
                            value.type_name()
                        ),
                    },
                };
                c.commands.push(command);
            },
        );
    }

    // tween_style_param(entity_id, param, target, seconds[, easing]) — let
    // the engine ease a number, `blend` or a colour to `target` over
    // `seconds` of game time (#370), instead of a set_style_param every
    // frame. Easing: "linear", "smooth" (default), "ease_in", "ease_out",
    // "ease_in_out". Fires `style_tween_done` #{ entity, param } at the end;
    // a set_style_param of the same setting, or clear_entity_style, stops it.
    for with_easing in [false, true] {
        let ctx = ctx.clone();
        let tween =
            move |entity_id: i64, param: &str, target: Dynamic, seconds: Dynamic, easing: &str| {
                let mut c = crate::lock_or_recover(&ctx);
                // Seconds as a float or an integer (a web script has i64 only)
                let secs = seconds
                    .as_float()
                    .ok()
                    .or_else(|| seconds.as_int().ok().map(|i| i as f64));
                let command = match (dynamic_to_toml(&target), secs) {
                    (Some(target), Some(seconds)) => ScriptCommand::TweenStyleParam {
                        entity_id,
                        param: param.to_string(),
                        target,
                        seconds,
                        easing: easing.to_string(),
                    },
                    (None, _) => ScriptCommand::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "tween_style_param: `{param}` can't ease to a {}",
                            target.type_name()
                        ),
                    },
                    (_, None) => ScriptCommand::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "tween_style_param: seconds must be a number, not a {}",
                            seconds.type_name()
                        ),
                    },
                };
                c.commands.push(command);
            };
        if with_easing {
            engine.register_fn("tween_style_param", tween);
        } else {
            engine.register_fn(
                "tween_style_param",
                move |entity_id: i64, param: &str, target: Dynamic, seconds: Dynamic| {
                    tween(entity_id, param, target, seconds, "smooth")
                },
            );
        }
    }

    // clear_entity_style(entity_id) — take the entity's own style off
    {
        let ctx = ctx.clone();
        engine.register_fn("clear_entity_style", move |entity_id: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands
                .push(ScriptCommand::ClearEntityStyle { entity_id });
        });
    }

    // Reading a style back (#372). These see the world as the script finds
    // it: a style set this frame is still a queued command, and reads back
    // from the next frame on. The host's StyleReader answers; without one
    // every entity is unstyled.
    fn read_style<T>(
        ctx: &Arc<Mutex<ScriptCallContext>>,
        entity_id: i64,
        f: impl FnOnce(
            &dyn crate::style_reader::StyleReader,
            &flint_ecs::FlintWorld,
            EntityId,
        ) -> Option<T>,
    ) -> Option<T> {
        if entity_id < 0 {
            return None;
        }
        let c = crate::lock_or_recover(ctx);
        let reader = c.style_reader.clone()?;
        let world = unsafe { c.world_ref() };
        f(reader.as_ref(), world, EntityId::from_raw(entity_id as u64))
    }

    // get_entity_style(entity_id) -> String — the style the entity is drawn
    // in: its own, one it inherits from an ancestor, or the scene's
    // whole-frame style; "" for none
    {
        let ctx = ctx.clone();
        engine.register_fn("get_entity_style", move |entity_id: i64| -> String {
            read_style(&ctx, entity_id, |r, w, id| r.entity_style(w, id))
                .map(|s| s.style)
                .unwrap_or_default()
        });
    }

    // get_entity_style_variant(entity_id) -> String — the same style's
    // variant, named for the settings that set it apart
    // ("ascii{ramp=blocks}", "ascii@barrel"); "" for none
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "get_entity_style_variant",
            move |entity_id: i64| -> String {
                read_style(&ctx, entity_id, |r, w, id| r.entity_style(w, id))
                    .map(|s| s.variant)
                    .unwrap_or_default()
            },
        );
    }

    // get_style_param(entity_id, param) — the value in effect: the entity's
    // (or the ancestor's it inherits), else the scene's, else the style's
    // default. A number (`cell`), a string (`ramp`), a colour as
    // [r, g, b] (`tint`), `blend` 0..1, `enabled` true. () when the entity
    // is drawn in no style or the style has no such setting
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "get_style_param",
            move |entity_id: i64, param: &str| -> Dynamic {
                read_style(&ctx, entity_id, |r, w, id| r.style_param(w, id, param))
                    .map(toml_to_dynamic)
                    .unwrap_or(Dynamic::UNIT)
            },
        );
    }

    // ─── Whole-frame styles (#369) ───
    //
    // The same for the scene's [post_process.styles.<name>] tables: any
    // registered style (kuwahara, invert, watercolor, full-screen ascii, a
    // user style). Every style takes `blend` 0..1, how much of it shows:
    // ramp it to cross-fade into a look. Fires `post_style_changed`
    // #{ style, enabled } on set and reset.

    // set_post_style(style, enabled) — switch a style on or off for the
    // whole frame; settings it had before are kept
    {
        let ctx = ctx.clone();
        engine.register_fn("set_post_style", move |style: &str, enabled: bool| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetPostStyle {
                style: style.to_string(),
                enabled,
            });
        });
    }

    // set_post_style_param(style, param, value) — a number (`radius`,
    // `cell`, `blend`...), a string (`ramp`, `palette`...), `enabled`, or a
    // colour (`tint`: [r, g, b] or "#rrggbb"). Cheap every frame.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_post_style_param",
            move |style: &str, param: &str, value: Dynamic| {
                let mut c = crate::lock_or_recover(&ctx);
                let command = match dynamic_to_toml(&value) {
                    Some(value) => ScriptCommand::SetPostStyleParam {
                        style: style.to_string(),
                        param: param.to_string(),
                        value,
                    },
                    None => ScriptCommand::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "set_post_style_param: `{param}` can't take a {}",
                            value.type_name()
                        ),
                    },
                };
                c.commands.push(command);
            },
        );
    }

    // reset_post_style(style) — back to what the scene authored for it
    {
        let ctx = ctx.clone();
        engine.register_fn("reset_post_style", move |style: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ResetPostStyle {
                style: style.to_string(),
            });
        });
    }

    // reset_style_history() — the next frame doesn't follow on from this
    // one (a cutscene's new shot, a teleport, a respawn; #343): styles
    // that remember earlier frames, like ASCII's steady glyphs, start over
    {
        let ctx = ctx.clone();
        engine.register_fn("reset_style_history", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ResetStyleHistory);
        });
    }

    // set_camera_cut_detection(enabled) — the player resets style history
    // by itself when the camera jumps or turns far past its recent motion
    // (on by default); a script that calls reset_style_history() at its
    // own cuts can turn that off
    {
        let ctx = ctx.clone();
        engine.register_fn("set_camera_cut_detection", move |enabled: bool| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands
                .push(ScriptCommand::SetCameraCutDetection { enabled });
        });
    }

    // set_look(name) — a whole post-processing preset (#356): a
    // styles/<name>.look.toml beside the scene or a built-in ("graphite",
    // "matrix", ...), laid under the scene's own [post_process] keys in
    // place of the look it had. "" is no look. Applied after on_update.
    {
        let ctx = ctx.clone();
        engine.register_fn("set_look", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            let name = name.trim();
            c.commands.push(ScriptCommand::SetLook {
                name: (!name.is_empty()).then(|| name.to_string()),
            });
        });
    }

    // clear_look() — back to the scene's own keys, without a look
    {
        let ctx = ctx.clone();
        engine.register_fn("clear_look", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetLook { name: None });
        });
    }
}

// ─── UI Draw API ─────────────────────────────────────────

fn register_ui_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // draw_text(x, y, text, size, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_text",
            move |x: f64, y: f64, text: &str, size: f64, r: f64, g: f64, b: f64, a: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.to_string(),
                    size: size as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: 0,
                    align: 0,
                    stroke: None,
                    font: None,
                    letter_spacing: 0.0,
                    shadow: None,
                    glow: None,
                });
            },
        );
    }

    // draw_text_ex(x, y, text, size, r, g, b, a, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_text_ex",
            move |x: f64,
                  y: f64,
                  text: &str,
                  size: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.to_string(),
                    size: size as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                    align: 0,
                    stroke: None,
                    font: None,
                    letter_spacing: 0.0,
                    shadow: None,
                    glow: None,
                });
            },
        );
    }

    // draw_text_stroked(x, y, text, size, r, g, b, a, stroke_r, stroke_g, stroke_b, stroke_a, stroke_width)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_text_stroked",
            move |x: f64,
                  y: f64,
                  text: &str,
                  size: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  sr: f64,
                  sg: f64,
                  sb: f64,
                  sa: f64,
                  sw: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.to_string(),
                    size: size as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: 0,
                    align: 0,
                    stroke: Some(([sr as f32, sg as f32, sb as f32, sa as f32], sw as f32)),
                    font: None,
                    letter_spacing: 0.0,
                    shadow: None,
                    glow: None,
                });
            },
        );
    }

    // draw_text_stroked_ex(x, y, text, size, r, g, b, a, sr, sg, sb, sa, stroke_w, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_text_stroked_ex",
            move |x: f64,
                  y: f64,
                  text: &str,
                  size: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  sr: f64,
                  sg: f64,
                  sb: f64,
                  sa: f64,
                  sw: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.to_string(),
                    size: size as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                    align: 0,
                    stroke: Some(([sr as f32, sg as f32, sb as f32, sa as f32], sw as f32)),
                    font: None,
                    letter_spacing: 0.0,
                    shadow: None,
                    glow: None,
                });
            },
        );
    }

    // draw_text_opts(x, y, text, size, opts) — opts is a map with optional keys:
    //   font (string), color ([r,g,b,a]), align ("left"|"center"|"right"),
    //   stroke ([r,g,b,a,width]), spacing (px), layer (int), shadow ([dx,dy,r,g,b,a]),
    //   glow ([r,g,b,a,radius]: a soft halo under the stroke, #626)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_text_opts",
            move |x: f64, y: f64, text: &str, size: f64, opts: Map| {
                let o = TextOpts::from_map(&opts);
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.to_string(),
                    size: size as f32,
                    color: o.color,
                    layer: o.layer,
                    align: o.align,
                    stroke: o.stroke,
                    font: o.font,
                    letter_spacing: o.spacing,
                    shadow: o.shadow,
                    glow: o.glow,
                });
            },
        );
    }

    // draw_rect(x, y, w, h, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect",
            move |x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64, a: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectFilled {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    rounding: 0.0,
                    layer: 0,
                });
            },
        );
    }

    // draw_rect_ex(x, y, w, h, r, g, b, a, rounding, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect_ex",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  rounding: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectFilled {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    rounding: rounding as f32,
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_rect_outline(x, y, w, h, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect_outline",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectOutline {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    rounding: 0.0,
                    layer: 0,
                });
            },
        );
    }

    // draw_circle(x, y, radius, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_circle",
            move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::CircleFilled {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: 0,
                });
            },
        );
    }

    // draw_circle_ex(x, y, radius, r, g, b, a, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_circle_ex",
            move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64, layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::CircleFilled {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_circle_outline(x, y, radius, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_circle_outline",
            move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64, thickness: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::CircleOutline {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    layer: 0,
                });
            },
        );
    }

    // draw_circle_outline_ex(x, y, radius, r, g, b, a, thickness, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_circle_outline_ex",
            move |x: f64,
                  y: f64,
                  radius: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::CircleOutline {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_line(x1, y1, x2, y2, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_line",
            move |x1: f64,
                  y1: f64,
                  x2: f64,
                  y2: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Line {
                    x1: x1 as f32,
                    y1: y1 as f32,
                    x2: x2 as f32,
                    y2: y2 as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    layer: 0,
                });
            },
        );
    }

    // world_to_screen(x, y, z) -> #{x, y, depth, visible}
    // Projects a world point with the camera the last frame rendered with.
    // `visible` is false behind the near plane (x/y are then meaningless).
    {
        let ctx = ctx.clone();
        engine.register_fn("world_to_screen", move |x: f64, y: f64, z: f64| -> Map {
            // The same answer `Projector` gives native code (ADR 0087).
            let view = View::of(&crate::lock_or_recover(&ctx));
            let mut map = Map::new();
            let (sx, sy, depth, visible) =
                match view.world_to_screen([x as f32, y as f32, z as f32]) {
                    Some(p) => (p.x, p.y, p.depth, true),
                    None => {
                        let clip = project_clip(&view.view_proj, [x as f32, y as f32, z as f32]);
                        (0.0, 0.0, clip[3], false)
                    }
                };
            map.insert("x".into(), Dynamic::from(sx as f64));
            map.insert("y".into(), Dynamic::from(sy as f64));
            map.insert("depth".into(), Dynamic::from(depth as f64));
            map.insert("visible".into(), Dynamic::from(visible));
            map
        });
    }

    // screen_to_ground(x, y, plane_y) -> [x, z] or ()
    // The ground under a screen point: where the ray through (x, y) meets the
    // plane y = plane_y. (x, y) is in logical points, the mouse_x()/mouse_y()
    // space, and the camera is the one world_to_screen uses, so
    // world_to_screen of the hit lands back on (x, y). () where the view
    // doesn't reach the plane: sky, a level orthographic camera, or a plane
    // behind the camera.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "screen_to_ground",
            move |x: f64, y: f64, plane_y: f64| -> Dynamic {
                let c = crate::lock_or_recover(&ctx);
                match flint_core::projection::screen_to_ground(
                    &c.camera_view_proj,
                    x as f32,
                    y as f32,
                    c.screen_width,
                    c.screen_height,
                    plane_y as f32,
                ) {
                    Some([gx, gz]) => Dynamic::from_array(vec![
                        Dynamic::from(gx as f64),
                        Dynamic::from(gz as f64),
                    ]),
                    None => Dynamic::UNIT,
                }
            },
        );
    }

    // screen_to_heightfield(x, y) -> [x, y, z] or ()
    // The game's ground under a screen point (ADR 0095): where the ray
    // through (x, y) first comes down onto the ground field the host
    // registered with Projector::set_ground. () with no ground registered,
    // or where the ray misses it (the sky, or past its edge); fall back to
    // screen_to_ground there.
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_to_heightfield", move |x: f64, y: f64| -> Dynamic {
            let (view, ground) = {
                let c = crate::lock_or_recover(&ctx);
                (crate::projector::View::of(&c), c.ground.clone())
            };
            match ground.and_then(|g| view.screen_to_heightfield(x as f32, y as f32, &g)) {
                Some([gx, gy, gz]) => Dynamic::from_array(vec![
                    Dynamic::from(gx as f64),
                    Dynamic::from(gy as f64),
                    Dynamic::from(gz as f64),
                ]),
                None => Dynamic::UNIT,
            }
        });
    }

    // ground_height(x, z) -> f64 or ()
    // World y of the registered ground field at (x, z). () with none
    // registered, or off its edge. (terrain_height reads a scene's terrain.)
    {
        let ctx = ctx.clone();
        engine.register_fn("ground_height", move |x: f64, z: f64| -> Dynamic {
            let ground = crate::lock_or_recover(&ctx).ground.clone();
            match ground.and_then(|g| g.height_at(x as f32, z as f32)) {
                Some(h) => Dynamic::from(h as f64),
                None => Dynamic::UNIT,
            }
        });
    }

    // raycast_meshes(ox, oy, oz, dx, dy, dz, max_dist) -> Map or ()
    // The nearest mesh drawn as a ray target (ADR 0105) that the ray from
    // (ox, oy, oz) along (dx, dy, dz) meets within max_dist world units:
    // #{ entity, distance, point_x/y/z, normal_x/y/z }, as `raycast` answers,
    // with the normal facing back along the ray. () when it meets none. The
    // meshes stand where the last frame drew them.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "raycast_meshes",
            move |ox: f64, oy: f64, oz: f64, dx: f64, dy: f64, dz: f64, max_dist: f64| -> Dynamic {
                let scene = crate::lock_or_recover(&ctx).ray_scene.clone();
                let o = [ox as f32, oy as f32, oz as f32];
                let d = [dx as f32, dy as f32, dz as f32];
                ray_hit_dynamic(scene.cast(o, d, max_dist as f32))
            },
        );
    }

    // screen_to_mesh(x, y) -> Map or ()
    // The nearest mesh drawn as a ray target under a screen point (ADR 0105),
    // as raycast_meshes answers: picking what's drawn, a wall or a roof,
    // rather than the ground. () where the ray meets none.
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_to_mesh", move |x: f64, y: f64| -> Dynamic {
            let (view, scene) = {
                let c = crate::lock_or_recover(&ctx);
                (crate::projector::View::of(&c), c.ray_scene.clone())
            };
            ray_hit_dynamic(
                view.screen_ray(x as f32, y as f32)
                    .and_then(|(o, d)| scene.cast(o, d, f32::INFINITY)),
            )
        });
    }

    // draw_line_3d(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)
    // World-space debug line, projected onto the HUD layer. Clipped against
    // the near plane so a segment that passes the camera still draws.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_line_3d",
            move |x1: f64,
                  y1: f64,
                  z1: f64,
                  x2: f64,
                  y2: f64,
                  z2: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                push_line_3d(
                    &mut c,
                    [x1, y1, z1],
                    [x2, y2, z2],
                    [r as f32, g as f32, b as f32, a as f32],
                    thickness as f32,
                    0,
                );
            },
        );
    }

    // draw_world_line(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)
    // World-space line drawn in the 3D frame with a depth test (ADR 0083):
    // geometry in front hides it. Projected with the camera the frame
    // renders with; thickness in logical points, colour as the HUD's.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_world_line",
            move |x1: f64,
                  y1: f64,
                  z1: f64,
                  x2: f64,
                  y2: f64,
                  z2: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.world_lines.push(flint_core::world_line::WorldLine {
                    a: [x1 as f32, y1 as f32, z1 as f32],
                    b: [x2 as f32, y2 as f32, z2 as f32],
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                });
            },
        );
    }

    // draw_world_text(x, y, z, text, size, opts)
    // Text at a world point, drawn in the 3D frame with a depth test
    // (ADR 0104): geometry in front hides it. opts are draw_text_opts's
    // (font, color, align, stroke, spacing, shadow, glow; no layer), centred on
    // the point by default, plus valign ("top"|"middle"|"bottom"), and
    // facing ([nx, ny, nz]) with up ([x, y, z], world up by default) to lay
    // it in a plane. A billboard's size is in points; on a plane, every
    // length is in world units.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_world_text",
            move |x: f64, y: f64, z: f64, text: &str, size: f64, opts: Map| {
                let t = world_text_from([x as f32, y as f32, z as f32], text, size as f32, &opts);
                let mut c = crate::lock_or_recover(&ctx);
                c.world_texts.push(t);
            },
        );
    }

    // draw_line_3d_ex(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_line_3d_ex",
            move |x1: f64,
                  y1: f64,
                  z1: f64,
                  x2: f64,
                  y2: f64,
                  z2: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                push_line_3d(
                    &mut c,
                    [x1, y1, z1],
                    [x2, y2, z2],
                    [r as f32, g as f32, b as f32, a as f32],
                    thickness as f32,
                    layer as i32,
                );
            },
        );
    }

    // draw_line_ex(x1, y1, x2, y2, r, g, b, a, thickness, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_line_ex",
            move |x1: f64,
                  y1: f64,
                  x2: f64,
                  y2: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Line {
                    x1: x1 as f32,
                    y1: y1 as f32,
                    x2: x2 as f32,
                    y2: y2 as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_rect_outline_ex(x, y, w, h, r, g, b, a, thickness, rounding, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect_outline_ex",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  rounding: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectOutline {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    rounding: rounding as f32,
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_rect_ex4(x, y, w, h, r, g, b, a, tl, tr, br, bl, layer)
    // Filled rectangle with per-corner rounding (top-left, top-right,
    // bottom-right, bottom-left).
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect_ex4",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  tl: f64,
                  tr: f64,
                  br: f64,
                  bl: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectRounded4 {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    rounding: [tl as f32, tr as f32, br as f32, bl as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_rect_gradient(x, y, w, h, r1, g1, b1, a1, r2, g2, b2, a2, vertical, layer)
    // Linear two-colour fill: colour 1 -> colour 2, top->bottom when
    // `vertical`, left->right otherwise.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_rect_gradient",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  r1: f64,
                  g1: f64,
                  b1: f64,
                  a1: f64,
                  r2: f64,
                  g2: f64,
                  b2: f64,
                  a2: f64,
                  vertical: bool,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::RectGradient {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color_a: [r1 as f32, g1 as f32, b1 as f32, a1 as f32],
                    color_b: [r2 as f32, g2 as f32, b2 as f32, a2 as f32],
                    vertical,
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_ring(cx, cy, r_inner, r_outer, start_deg, end_deg, r, g, b, a, layer)
    // Annular sector. Degrees from 12 o'clock, clockwise (0 = up, 90 =
    // right); `end_deg > start_deg`, sweep clamped to 360; r_inner 0 = pie.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_ring",
            move |cx: f64,
                  cy: f64,
                  r_inner: f64,
                  r_outer: f64,
                  start_deg: f64,
                  end_deg: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Ring {
                    cx: cx as f32,
                    cy: cy as f32,
                    r_inner: r_inner as f32,
                    r_outer: r_outer as f32,
                    start_deg: start_deg as f32,
                    end_deg: end_deg as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_arc(cx, cy, radius, start_deg, end_deg, r, g, b, a, thickness, layer)
    // Sugar for a ring centred on `radius` with the given stroke thickness.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_arc",
            move |cx: f64,
                  cy: f64,
                  radius: f64,
                  start_deg: f64,
                  end_deg: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  layer: i64| {
                let half = (thickness * 0.5) as f32;
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Ring {
                    cx: cx as f32,
                    cy: cy as f32,
                    r_inner: (radius as f32 - half).max(0.0),
                    r_outer: radius as f32 + half,
                    start_deg: start_deg as f32,
                    end_deg: end_deg as f32,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_polygon(points, r, g, b, a, layer) - convex fill.
    // `points` is `[[x, y], ...]` or flat `[x0, y0, x1, y1, ...]` (ints or floats).
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_polygon",
            move |points: Array, r: f64, g: f64, b: f64, a: f64, layer: i64| {
                let Some(points) = crate::draw2d::parse_points(&points) else {
                    tracing::warn!(
                        "draw_polygon: points must be [[x,y],...] or a flat even-length [x0,y0,...] list of numbers"
                    );
                    return;
                };
                if points.len() < 3 {
                    return;
                }
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Polygon {
                    points,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // draw_polygon_outline(points, r, g, b, a, thickness, closed, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_polygon_outline",
            move |points: Array,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  thickness: f64,
                  closed: bool,
                  layer: i64| {
                let Some(points) = crate::draw2d::parse_points(&points) else {
                    tracing::warn!(
                        "draw_polygon_outline: points must be [[x,y],...] or a flat even-length [x0,y0,...] list of numbers"
                    );
                    return;
                };
                if points.len() < 2 {
                    return;
                }
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::PolygonOutline {
                    points,
                    color: [r as f32, g as f32, b as f32, a as f32],
                    thickness: thickness as f32,
                    closed,
                    layer: layer as i32,
                });
            },
        );
    }

    // push_clip(x, y, w, h) / pop_clip()
    // Every draw call issued while a clip is pushed is confined to that rect
    // (nested pushes intersect). Unbalanced pops are ignored and the stack
    // resets each frame.
    {
        let ctx = ctx.clone();
        engine.register_fn("push_clip", move |x: f64, y: f64, w: f64, h: f64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.push_clip([x as f32, y as f32, w as f32, h as f32]);
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("pop_clip", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.pop_clip();
        });
    }

    // push_over_ui() / pop_over_ui()
    // Every draw call issued between them draws above the game's egui panels
    // and windows instead of under them (ADR 0085). Pushes nest, unbalanced
    // pops are ignored, and the depth resets each frame.
    {
        let ctx = ctx.clone();
        engine.register_fn("push_over_ui", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.push_over_ui();
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("pop_over_ui", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.pop_over_ui();
        });
    }

    // draw_sprite(x, y, w, h, name)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_sprite",
            move |x: f64, y: f64, w: f64, h: f64, name: &str| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Sprite {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    name: name.to_string(),
                    uv: [0.0, 0.0, 1.0, 1.0],
                    tint: [1.0, 1.0, 1.0, 1.0],
                    layer: 0,
                });
            },
        );
    }

    // draw_sprite_ex(x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "draw_sprite_ex",
            move |x: f64,
                  y: f64,
                  w: f64,
                  h: f64,
                  name: &str,
                  u0: f64,
                  v0: f64,
                  u1: f64,
                  v1: f64,
                  r: f64,
                  g: f64,
                  b: f64,
                  a: f64,
                  layer: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.push_draw(DrawCommand::Sprite {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    name: name.to_string(),
                    uv: [u0 as f32, v0 as f32, u1 as f32, v1 as f32],
                    tint: [r as f32, g as f32, b as f32, a as f32],
                    layer: layer as i32,
                });
            },
        );
    }

    // screen_width() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_width", move || -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.screen_width as f64
        });
    }

    // screen_height() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_height", move || -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.screen_height as f64
        });
    }

    // find_nearest_interactable() -> Map or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("find_nearest_interactable", move || -> Dynamic {
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_ref() };
            match crate::engine::find_nearest_interactable(world) {
                Some(nearest) => {
                    let mut map = Map::new();
                    map.insert(
                        "entity".into(),
                        Dynamic::from(nearest.entity_id.raw() as i64),
                    );
                    map.insert("prompt_text".into(), Dynamic::from(nearest.prompt_text));
                    map.insert(
                        "interaction_type".into(),
                        Dynamic::from(nearest.interaction_type),
                    );
                    map.insert("distance".into(), Dynamic::from(nearest.distance));
                    Dynamic::from(map)
                }
                None => Dynamic::UNIT,
            }
        });
    }

    // measure_text(text, size) -> Map #{width, height}
    {
        let ctx = ctx.clone();
        engine.register_fn("measure_text", move |text: &str, size: f64| -> Map {
            let c = crate::lock_or_recover(&ctx);
            let (w, h) = measure_text_with(&c, text, size as f32, None, 0.0);
            drop(c);
            let mut map = Map::new();
            map.insert("width".into(), Dynamic::from(w as f64));
            map.insert("height".into(), Dynamic::from(h as f64));
            map
        });
    }

    // measure_text_ex(text, size, font, spacing) -> Map #{width, height}
    // `font` is a family name ("" = default font); `spacing` is extra letter spacing in px.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "measure_text_ex",
            move |text: &str, size: f64, font: &str, spacing: f64| -> Map {
                let c = crate::lock_or_recover(&ctx);
                let family = (!font.is_empty()).then_some(font);
                let (w, h) = measure_text_with(&c, text, size as f32, family, spacing as f32);
                drop(c);
                let mut map = Map::new();
                map.insert("width".into(), Dynamic::from(w as f64));
                map.insert("height".into(), Dynamic::from(h as f64));
                map
            },
        );
    }

    // move_character(entity_id, dx, dy, dz) -> Map #{x, y, z, grounded} or ()
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "move_character",
            move |entity_id: i64, dx: f64, dy: f64, dz: f64| -> Dynamic {
                if entity_id < 0 {
                    return Dynamic::UNIT;
                }
                let c = crate::lock_or_recover(&ctx);
                let physics = unsafe { c.physics_ref() };
                let physics = match physics {
                    Some(p) => p,
                    None => return Dynamic::UNIT,
                };
                let world = unsafe { c.world_ref() };
                let eid = EntityId::from_raw(entity_id as u64);

                // Read entity's current ECS position (freshest data, sees set_position from same frame)
                let current_pos = match world.get_transform(eid) {
                    Some(t) => [t.position.x, t.position.y, t.position.z],
                    None => return Dynamic::UNIT,
                };

                let dt = c.delta_time as f32;
                match physics.move_character_shape(
                    eid,
                    current_pos,
                    [dx as f32, dy as f32, dz as f32],
                    dt,
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
                }
            },
        );
    }

    // get_collider_extents(entity_id) -> Map or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("get_collider_extents", move |entity_id: i64| -> Dynamic {
            if entity_id < 0 {
                return Dynamic::UNIT;
            }
            let c = crate::lock_or_recover(&ctx);
            let physics = unsafe { c.physics_ref() };
            let physics = match physics {
                Some(p) => p,
                None => return Dynamic::UNIT,
            };
            let eid = EntityId::from_raw(entity_id as u64);

            match physics.get_entity_collider_extents(eid) {
                Some(flint_physics::ColliderExtents::Box { half_extents }) => {
                    let mut map = Map::new();
                    map.insert("shape".into(), Dynamic::from("box".to_string()));
                    map.insert("half_x".into(), Dynamic::from(half_extents[0] as f64));
                    map.insert("half_y".into(), Dynamic::from(half_extents[1] as f64));
                    map.insert("half_z".into(), Dynamic::from(half_extents[2] as f64));
                    Dynamic::from(map)
                }
                Some(flint_physics::ColliderExtents::Sphere { radius }) => {
                    let mut map = Map::new();
                    map.insert("shape".into(), Dynamic::from("sphere".to_string()));
                    map.insert("radius".into(), Dynamic::from(radius as f64));
                    Dynamic::from(map)
                }
                Some(flint_physics::ColliderExtents::Capsule {
                    radius,
                    half_height,
                }) => {
                    let mut map = Map::new();
                    map.insert("shape".into(), Dynamic::from("capsule".to_string()));
                    map.insert("radius".into(), Dynamic::from(radius as f64));
                    map.insert("half_height".into(), Dynamic::from(half_height as f64));
                    Dynamic::from(map)
                }
                Some(flint_physics::ColliderExtents::Cylinder {
                    radius,
                    half_height,
                }) => {
                    let mut map = Map::new();
                    map.insert("shape".into(), Dynamic::from("cylinder".to_string()));
                    map.insert("radius".into(), Dynamic::from(radius as f64));
                    map.insert("half_height".into(), Dynamic::from(half_height as f64));
                    Dynamic::from(map)
                }
                None => Dynamic::UNIT,
            }
        });
    }
}

// ─── Data-Driven UI API ───────────────────────────────────

fn register_data_ui_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    use crate::ui::element::StyleValue;

    // load_ui(layout_path) -> i64 handle
    {
        let ctx = ctx.clone();
        engine.register_fn("load_ui", move |layout_path: &str| -> i64 {
            let mut c = crate::lock_or_recover(&ctx);
            // A path from the project root (ADR 0114)
            let root = c.project_root_holding("load_ui", layout_path);
            c.ui_system.load(layout_path, &root)
        });
    }

    // unload_ui(handle)
    {
        let ctx = ctx.clone();
        engine.register_fn("unload_ui", move |handle: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.unload(handle);
        });
    }

    // ui_set_text(element_id, text)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_text", move |element_id: &str, text: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.set_text(element_id, text);
        });
    }

    // ui_show(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_show", move |element_id: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.show(element_id);
        });
    }

    // ui_hide(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_hide", move |element_id: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.hide(element_id);
        });
    }

    // ui_set_visible(element_id, visible)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_visible", move |element_id: &str, visible: bool| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.set_visible(element_id, visible);
        });
    }

    // ui_set_color(element_id, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "ui_set_color",
            move |element_id: &str, r: f64, g: f64, b: f64, a: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.ui_system
                    .set_color(element_id, r as f32, g as f32, b as f32, a as f32);
            },
        );
    }

    // ui_set_bg_color(element_id, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "ui_set_bg_color",
            move |element_id: &str, r: f64, g: f64, b: f64, a: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.ui_system
                    .set_bg_color(element_id, r as f32, g as f32, b as f32, a as f32);
            },
        );
    }

    // ui_set_style(element_id, prop, val) — every property the style parser
    // accepts: numbers, strings (text_align / layout / font), bools
    // (height_auto), arrays (color / padding / shadow), and "$token" strings
    // for any of them.
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "ui_set_style",
            move |element_id: &str, prop: &str, val: Dynamic| {
                let style_val = if val.is_float() {
                    StyleValue::Float(val.as_float().unwrap_or(0.0) as f32)
                } else if val.is_int() {
                    StyleValue::Float(val.as_int().unwrap_or(0) as f32)
                } else if val.is_string() {
                    StyleValue::String(val.into_string().unwrap_or_default())
                } else if val.is_bool() {
                    StyleValue::Bool(val.as_bool().unwrap_or(false))
                } else if val.is_array() {
                    let arr = val.cast::<Array>();
                    let values = array_to_f32s(&arr);
                    match StyleValue::from_numbers(&values) {
                        Some(sv) => sv,
                        None => {
                            tracing::warn!(
                                "ui_set_style({element_id}, {prop}): expected 3, 4 or 6 numbers, got {}",
                                arr.len()
                            );
                            return;
                        }
                    }
                } else {
                    tracing::warn!(
                        "ui_set_style({element_id}, {prop}): unsupported value type {}",
                        val.type_name()
                    );
                    return;
                };
                let mut c = crate::lock_or_recover(&ctx);
                c.ui_system.set_style(element_id, prop, style_val);
            },
        );
    }

    // ui_set_style_array(element_id, prop, array) — for array-valued props:
    // color / bg_color / stroke_color / padding / uv ([a,b,c,d]) and shadow ([dx,dy,r,g,b,a])
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "ui_set_style_array",
            move |element_id: &str, prop: &str, val: Array| {
                let values = array_to_f32s(&val);
                let Some(style_val) = StyleValue::from_numbers(&values) else {
                    tracing::warn!(
                        "ui_set_style_array({element_id}, {prop}): expected 4 or 6 numbers, got {}",
                        val.len()
                    );
                    return;
                };
                let mut c = crate::lock_or_recover(&ctx);
                c.ui_system.set_style(element_id, prop, style_val);
            },
        );
    }

    // ui_reset_style(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_reset_style", move |element_id: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.reset_style(element_id);
        });
    }

    // ui_set_class(element_id, class)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_class", move |element_id: &str, class: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.ui_system.set_class(element_id, class);
        });
    }

    // ui_exists(element_id) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_exists", move |element_id: &str| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.ui_system.exists(element_id)
        });
    }

    // ui_hit(element_id, x, y) -> bool — point inside the element's resolved rect
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_hit", move |element_id: &str, x: f64, y: f64| -> bool {
            let mut c = crate::lock_or_recover(&ctx);
            let sw = c.screen_width;
            let sh = c.screen_height;
            c.ui_system.hit(element_id, x as f32, y as f32, sw, sh)
        });
    }

    // ui_token(name) -> array | float | string | bool | ()
    // Reads a style token ("accent", "$accent", "color.accent") from the
    // loaded documents' [tokens] tables and shared token file.
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_token", move |name: &str| -> Dynamic {
            let c = crate::lock_or_recover(&ctx);
            match c.ui_system.token(name) {
                Some(v) => toml_token_to_dynamic(&v),
                None => Dynamic::UNIT,
            }
        });
    }

    // ui_get_rect(element_id) -> Map #{x, y, w, h} or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_get_rect", move |element_id: &str| -> Dynamic {
            let mut c = crate::lock_or_recover(&ctx);
            let sw = c.screen_width;
            let sh = c.screen_height;
            match c.ui_system.get_rect(element_id, sw, sh) {
                Some((x, y, w, h)) => {
                    let mut map = Map::new();
                    map.insert("x".into(), Dynamic::from(x as f64));
                    map.insert("y".into(), Dynamic::from(y as f64));
                    map.insert("w".into(), Dynamic::from(w as f64));
                    map.insert("h".into(), Dynamic::from(h as f64));
                    Dynamic::from(map)
                }
                None => Dynamic::UNIT,
            }
        });
    }
}

/// Convert a token value to what a script expects: numbers → FLOAT,
/// numeric arrays → Array of FLOAT, strings/bools as-is, anything else `()`.
fn toml_token_to_dynamic(v: &toml::Value) -> Dynamic {
    match v {
        toml::Value::Float(f) => Dynamic::from(*f),
        toml::Value::Integer(i) => Dynamic::from(*i as f64),
        toml::Value::String(s) => Dynamic::from(s.clone()),
        toml::Value::Boolean(b) => Dynamic::from(*b),
        toml::Value::Array(arr) => {
            let out: Array = arr
                .iter()
                .map(|x| match x {
                    toml::Value::Float(f) => Dynamic::from(*f),
                    toml::Value::Integer(i) => Dynamic::from(*i as f64),
                    toml::Value::String(s) => Dynamic::from(s.clone()),
                    toml::Value::Boolean(b) => Dynamic::from(*b),
                    _ => Dynamic::UNIT,
                })
                .collect();
            Dynamic::from(out)
        }
        _ => Dynamic::UNIT,
    }
}

// ─── Expression API (Flint#518) ───────────────────────────
//
// State from outside the process: a timeline played on the game clock
// (`--expression-timeline`) or newline-delimited JSON on a localhost socket
// (`--expression-listen`). Flint doesn't know the fields; any JSON object
// arrives as a map, numbers as floats. The host delivers each new state before
// on_update, calling `on_expression(state)` on every script that defines it.

fn register_expression_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // expression() -> Map: the latest state, #{} before the first arrives
    engine.register_fn("expression", move || -> Map {
        crate::lock_or_recover(&ctx).expression.clone()
    });
}

// ─── State Machine API ────────────────────────────────────

fn register_state_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // push_state(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("push_state", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::PushState {
                name: name.to_string(),
            });
        });
    }

    // pop_state()
    {
        let ctx = ctx.clone();
        engine.register_fn("pop_state", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::PopState);
        });
    }

    // replace_state(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("replace_state", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ReplaceState {
                name: name.to_string(),
            });
        });
    }

    // current_state() -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("current_state", move || -> String {
            let c = crate::lock_or_recover(&ctx);
            if c.state_machine.is_null() {
                return "playing".to_string();
            }
            let sm = unsafe { &*c.state_machine };
            sm.current_state().to_string()
        });
    }

    // state_stack() -> Array
    {
        let ctx = ctx.clone();
        engine.register_fn("state_stack", move || -> rhai::Array {
            let c = crate::lock_or_recover(&ctx);
            if c.state_machine.is_null() {
                return vec![Dynamic::from("playing".to_string())];
            }
            let sm = unsafe { &*c.state_machine };
            sm.stack_names()
                .into_iter()
                .map(|s| Dynamic::from(s.to_string()))
                .collect()
        });
    }

    // register_state(name: &str, config: Map)
    {
        let ctx = ctx.clone();
        engine.register_fn("register_state", move |name: &str, config: Map| {
            let mut c = crate::lock_or_recover(&ctx);
            if c.state_machine.is_null() {
                return;
            }
            let sm = unsafe { &mut *c.state_machine };

            let policy = |key: &str| -> flint_runtime::SystemPolicy {
                match config
                    .get(key)
                    .and_then(|v| v.clone().into_string().ok())
                    .as_deref()
                {
                    Some("pause") | Some("Pause") => flint_runtime::SystemPolicy::Pause,
                    Some("hidden") | Some("Hidden") => flint_runtime::SystemPolicy::Hidden,
                    _ => flint_runtime::SystemPolicy::Run,
                }
            };

            let transparent = config
                .get("transparent")
                .and_then(|v| v.as_bool().ok())
                .unwrap_or(false);

            sm.register_state(
                name,
                flint_runtime::StateConfig {
                    physics: policy("physics"),
                    scripts: policy("scripts"),
                    animation: policy("animation"),
                    particles: policy("particles"),
                    audio: policy("audio"),
                    rendering: policy("rendering"),
                    transparent,
                },
            );
        });
    }
}

// ─── Scene API ────────────────────────────────────────────

fn register_scene_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // load_scene(path: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("load_scene", move |path: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::LoadScene {
                path: path.to_string(),
            });
        });
    }

    // quit_game(): end the run, as closing the window does (#659)
    {
        let ctx = ctx.clone();
        engine.register_fn("quit_game", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::QuitGame);
        });
    }

    // reload_scene()
    {
        let ctx = ctx.clone();
        engine.register_fn("reload_scene", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ReloadScene);
        });
    }

    // current_scene() -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("current_scene", move || -> String {
            let c = crate::lock_or_recover(&ctx);
            c.current_scene_path.clone()
        });
    }

    // complete_transition()
    {
        let ctx = ctx.clone();
        engine.register_fn("complete_transition", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::FireEvent {
                name: TRANSITION_COMPLETE.to_string(),
                data: toml::Value::Table(toml::map::Map::new()),
            });
        });
    }

    // transition_progress() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("transition_progress", move || -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.transition_progress
        });
    }

    // is_transitioning() -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_transitioning", move || -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.transition_progress >= 0.0
        });
    }

    // transition_phase() -> String ("idle", "exiting", "entering")
    {
        let ctx = ctx.clone();
        engine.register_fn("transition_phase", move || -> String {
            let c = crate::lock_or_recover(&ctx);
            c.transition_phase.clone()
        });
    }
}

// ─── Wooze API (#213) ─────────────────────────────────────
//
// Takes effect when the frame's commands run, and reaches a living prop on the
// next cycle of each of its parts, or at once for step, which bends the clock.
// What's wrong with a table is logged by the player, naming the zone. A scene
// with no [wooze] table has no field to change, and says so.

fn register_wooze_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // Living Props' knobs (#218), the F3 panel's by another door. tune_living takes
    // what a .wooze.toml holds: #{ wooze: #{ step: 0.25 }, living: #{ echoes:
    // #{ count: 3 } }, boil: #{ on: 2 } }. What it leaves out stays as it is.
    // With a species (#246) it takes one species' own values, which beat the
    // global ones: #{ species: "grove_thorn", step: 0.25, echoes: #{ count: 1 } }.
    {
        let ctx = ctx.clone();
        engine.register_fn("tune_living", move |knobs: Map| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::TuneLiving {
                knobs: map_to_toml(&knobs),
            });
        });
    }
    // clear_living(species), clear_living(species, ["echoes", "step"]): give a
    // species' values back to the layers below, all of them or the keys named.
    {
        let ctx = ctx.clone();
        engine.register_fn("clear_living", move |species: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ClearLiving {
                species: species.to_string(),
                keys: Vec::new(),
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("clear_living", move |species: &str, keys: Array| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::ClearLiving {
                species: species.to_string(),
                keys: keys.iter().map(|key| key.to_string()).collect(),
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("freeze_living", move |frozen: bool| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::FreezeLiving { frozen });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("step_living", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::StepLiving);
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("save_living_preset", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SaveLivingPreset {
                name: name.to_string(),
            });
        });
    }
    // commit_living(): write every value the scene file doesn't give into it
    // (#249), species tables included, each on its own line. Logged; an error on
    // the web, which has no file to write.
    {
        let ctx = ctx.clone();
        engine.register_fn("commit_living", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::CommitLiving);
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("load_living_preset", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::LoadLivingPreset {
                name: name.to_string(),
            });
        });
    }
    // A/B (#238): compare_living("", "linklater") puts the knobs as they stand
    // beside a preset; flip_living() swaps which is shown, split_living(true)
    // shows both, A left; stop_comparing_living() keeps what's shown.
    {
        let ctx = ctx.clone();
        engine.register_fn("compare_living", move |a: &str, b: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::CompareLiving {
                a: a.to_string(),
                b: b.to_string(),
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("flip_living", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::FlipLiving);
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("split_living", move |split: bool| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SplitLiving { split });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_comparing_living", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::StopComparingLiving);
        });
    }

    // set_wooze(knobs: Map), set_wooze(knobs: Map, seconds)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_wooze", move |knobs: Map| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetWooze {
                knobs: map_to_toml(&knobs),
                seconds: 0.0,
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("set_wooze", move |knobs: Map, seconds: f64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetWooze {
                knobs: map_to_toml(&knobs),
                seconds,
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("set_wooze", move |knobs: Map, seconds: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetWooze {
                knobs: map_to_toml(&knobs),
                seconds: seconds as f64,
            });
        });
    }

    // set_wooze_zone(name, fields: Map), set_wooze_zone(name, fields: Map, seconds)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_wooze_zone", move |name: &str, fields: Map| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::SetWoozeZone {
                name: name.to_string(),
                fields: map_to_toml(&fields),
                seconds: 0.0,
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_wooze_zone",
            move |name: &str, fields: Map, seconds: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.commands.push(ScriptCommand::SetWoozeZone {
                    name: name.to_string(),
                    fields: map_to_toml(&fields),
                    seconds,
                });
            },
        );
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_wooze_zone",
            move |name: &str, fields: Map, seconds: i64| {
                let mut c = crate::lock_or_recover(&ctx);
                c.commands.push(ScriptCommand::SetWoozeZone {
                    name: name.to_string(),
                    fields: map_to_toml(&fields),
                    seconds: seconds as f64,
                });
            },
        );
    }

    // remove_wooze_zone(name), remove_wooze_zone(name, seconds)
    {
        let ctx = ctx.clone();
        engine.register_fn("remove_wooze_zone", move |name: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::RemoveWoozeZone {
                name: name.to_string(),
                seconds: 0.0,
            });
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("remove_wooze_zone", move |name: &str, seconds: f64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::RemoveWoozeZone {
                name: name.to_string(),
                seconds,
            });
        });
    }
    {
        engine.register_fn("remove_wooze_zone", move |name: &str, seconds: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::RemoveWoozeZone {
                name: name.to_string(),
                seconds: seconds as f64,
            });
        });
    }
}

// ─── Persistence API ──────────────────────────────────────

fn register_persistence_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // persist_set(key, value)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_set", move |key: &str, val: Dynamic| {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            if let Some(tv) = crate::api::dynamic_to_toml(&val) {
                store.set(key, tv);
            }
        });
    }

    // persist_get(key) -> Dynamic
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_get", move |key: &str| -> Dynamic {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return Dynamic::UNIT;
            }
            let store = unsafe { &*c.persistent_store };
            match store.get(key) {
                Some(val) => crate::api::toml_to_dynamic(val.clone()),
                None => Dynamic::UNIT,
            }
        });
    }

    // persist_has(key) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_has", move |key: &str| -> bool {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return false;
            }
            let store = unsafe { &*c.persistent_store };
            store.has(key)
        });
    }

    // persist_remove(key)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_remove", move |key: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            store.remove(key);
        });
    }

    // persist_clear()
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_clear", move || {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            store.clear();
        });
    }

    // persist_keys() -> Array
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_keys", move || -> rhai::Array {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return vec![];
            }
            let store = unsafe { &*c.persistent_store };
            store
                .keys()
                .into_iter()
                .map(|k| Dynamic::from(k.to_string()))
                .collect()
        });
    }

    // persist_save() -- flush the engine-managed save file now (the player
    // otherwise writes it 1 s after the last change, on scene transitions
    // and on exit).
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_save", move || {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            store.request_flush();
        });
    }

    // persist_save(path) -- explicit path, written immediately
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_save", move |path: &str| {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &*c.persistent_store };
            if let Err(e) = store.save_to_file(std::path::Path::new(path)) {
                tracing::warn!("save error: {}", e);
            }
        });
    }

    // persist_slot() -> String: the slot held ("default" to begin with)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_slot", move || -> String {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return flint_runtime::DEFAULT_SLOT.to_string();
            }
            let store = unsafe { &*c.persistent_store };
            store.slot().to_string()
        });
    }

    // persist_slot(name) -> bool: hold another slot. The one held is saved
    // first if it changed; `name` is then loaded, or begun empty. False
    // (and a warning) for a bad name, a save that failed, or a run with no
    // save location; the slot held is then unchanged.
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_slot", move |name: &str| -> bool {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return false;
            }
            let store = unsafe { &mut *c.persistent_store };
            match store.open_slot(name) {
                Ok(loaded) if loaded.is_error() => {
                    tracing::error!("persist: {loaded}");
                    true
                }
                Ok(loaded) => {
                    tracing::info!("persist: slot '{name}': {loaded}");
                    true
                }
                Err(e) => {
                    tracing::warn!("persist_slot: {e}");
                    false
                }
            }
        });
    }

    // persist_slots() -> Array of the slots that have a save, in name order
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_slots", move || -> rhai::Array {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return vec![];
            }
            let store = unsafe { &*c.persistent_store };
            store.slots().into_iter().map(Dynamic::from).collect()
        });
    }

    // persist_version() -> int: the version of the data held (0 for a file
    // from before versions)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_version", move || -> i64 {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return 0;
            }
            let store = unsafe { &*c.persistent_store };
            store.version()
        });
    }

    // persist_set_version(n): stamp the data held, for a game with no
    // `[project] save_version` that migrates by hand
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_set_version", move |version: i64| {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            if version < 0 {
                tracing::warn!("persist_set_version({version}): a version is 0 or more");
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            store.set_version(version);
        });
    }

    // persist_corrupt() -> () when the slot held loaded cleanly (or had no
    // file), else #{ slot, file, backup, error }
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_corrupt", move || -> Dynamic {
            let c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return Dynamic::UNIT;
            }
            let store = unsafe { &*c.persistent_store };
            match store.corrupt() {
                None => Dynamic::UNIT,
                Some(corrupt) => {
                    let mut map = rhai::Map::new();
                    map.insert("slot".into(), corrupt.slot.clone().into());
                    map.insert("file".into(), corrupt.file.clone().into());
                    map.insert("backup".into(), corrupt.backup.clone().into());
                    map.insert("error".into(), corrupt.error.clone().into());
                    Dynamic::from(map)
                }
            }
        });
    }

    // persist_load(path)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_load", move |path: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            if let Err(e) = store.load_from_file(std::path::Path::new(path)) {
                tracing::warn!("load error: {}", e);
            }
        });
    }
}

// ─── Data files API ──────────────────────────────────────

/// Read and cache a TOML file relative to the project root, returning the
/// whole document as a Rhai map (`()` on failure, which is also logged).
fn load_data_impl(c: &mut ScriptCallContext, path: &str, reload: bool) -> Dynamic {
    let full = c.project_root_holding("load_data", path).join(path);
    let key = full.to_string_lossy().to_string();
    if reload {
        c.data_cache.remove(&key);
    }
    if let Some(v) = c.data_cache.get(&key) {
        return toml_to_dynamic(v.clone());
    }
    let parsed = vfs::read_to_string(&full)
        .map_err(|e| e.to_string())
        .and_then(|text| text.parse::<toml::Value>().map_err(|e| e.to_string()));
    match parsed {
        Ok(v) => {
            let d = toml_to_dynamic(v.clone());
            c.data_cache.insert(key, v);
            d
        }
        Err(e) => {
            let message = format!("load_data('{}') failed (resolved to {}): {}", path, key, e);
            tracing::warn!("{}", message);
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Warn,
                message,
            });
            Dynamic::UNIT
        }
    }
}

fn register_data_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // load_data(path) -> Map | ()  — parsed once per path, cached for the session.
    // Types are preserved exactly (TOML integer → INT, float → FLOAT), unlike ui_token.
    {
        let ctx = ctx.clone();
        engine.register_fn("load_data", move |path: &str| -> Dynamic {
            let mut c = crate::lock_or_recover(&ctx);
            load_data_impl(&mut c, path, false)
        });
    }

    // load_data(path, reload) — reload = true drops the cached parse first.
    {
        let ctx = ctx.clone();
        engine.register_fn("load_data", move |path: &str, reload: bool| -> Dynamic {
            let mut c = crate::lock_or_recover(&ctx);
            load_data_impl(&mut c, path, reload)
        });
    }
}

// ─── Conversion helpers ──────────────────────────────────

/// Convert a toml::Value to rhai::Dynamic
pub fn toml_to_dynamic(val: toml::Value) -> Dynamic {
    match val {
        toml::Value::Boolean(b) => Dynamic::from(b),
        toml::Value::Integer(i) => Dynamic::from(i),
        toml::Value::Float(f) => Dynamic::from(f),
        toml::Value::String(s) => Dynamic::from(s),
        toml::Value::Array(arr) => {
            let items: Vec<Dynamic> = arr.into_iter().map(toml_to_dynamic).collect();
            Dynamic::from(items)
        }
        toml::Value::Table(table) => {
            let mut map = Map::new();
            for (k, v) in table {
                map.insert(k.into(), toml_to_dynamic(v));
            }
            Dynamic::from(map)
        }
        toml::Value::Datetime(dt) => Dynamic::from(dt.to_string()),
    }
}

/// Convert a rhai::Dynamic to toml::Value
pub fn dynamic_to_toml(val: &Dynamic) -> Option<toml::Value> {
    if val.is_bool() {
        return val.as_bool().ok().map(toml::Value::Boolean);
    }
    if val.is_int() {
        return val.as_int().ok().map(toml::Value::Integer);
    }
    if val.is_float() {
        return val.as_float().ok().map(toml::Value::Float);
    }
    if val.is_string() {
        return val.clone().into_string().ok().map(toml::Value::String);
    }
    if val.is_array() {
        let arr = val.clone().into_array().ok()?;
        let items: Vec<toml::Value> = arr.iter().filter_map(dynamic_to_toml).collect();
        return Some(toml::Value::Array(items));
    }
    if val.is_map() {
        return Some(map_to_toml_from_dynamic(val));
    }
    None
}

fn map_to_toml(map: &Map) -> toml::Value {
    let mut table = toml::map::Map::new();
    for (k, v) in map {
        if let Some(tv) = dynamic_to_toml(v) {
            table.insert(k.to_string(), tv);
        }
    }
    toml::Value::Table(table)
}

fn map_to_toml_from_dynamic(val: &Dynamic) -> toml::Value {
    if let Some(map) = val.clone().try_cast::<Map>() {
        return map_to_toml(&map);
    }
    toml::Value::String(val.to_string())
}

// ─── Terrain API ──────────────────────────────────────────

fn register_terrain_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // terrain_height(x: f64, z: f64) -> f64
    // Returns the world-space Y height of the terrain at (x, z), or 0.0 if no terrain is loaded.
    {
        let ctx = ctx.clone();
        engine.register_fn("terrain_height", move |x: f64, z: f64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            if let Some(ref height_fn) = c.terrain_height_fn {
                height_fn(x as f32, z as f32) as f64
            } else {
                0.0
            }
        });
    }
}

// ─── Sprite API ───────────────────────────────────────────

fn register_sprite_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // set_sprite_source_rect(entity_id, x, y, w, h) — set source rectangle in pixels
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_sprite_source_rect",
            move |id: i64, x: f64, y: f64, w: f64, h: f64| {
                if id < 0 {
                    return;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_mut() };
                let eid = EntityId::from_raw(id as u64);
                if let Some(components) = world.get_components_mut(eid) {
                    if let Some(sprite) = components.get_mut(comp::SPRITE) {
                        if let Some(table) = sprite.as_table_mut() {
                            table.insert(
                                "source_rect".to_string(),
                                toml::Value::Array(vec![
                                    toml::Value::Float(x),
                                    toml::Value::Float(y),
                                    toml::Value::Float(w),
                                    toml::Value::Float(h),
                                ]),
                            );
                        }
                    }
                }
            },
        );
    }

    // set_sprite_flip(entity_id, flip_x, flip_y)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_sprite_flip",
            move |id: i64, flip_x: bool, flip_y: bool| {
                if id < 0 {
                    return;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_mut() };
                let eid = EntityId::from_raw(id as u64);
                if let Some(components) = world.get_components_mut(eid) {
                    if let Some(sprite) = components.get_mut(comp::SPRITE) {
                        if let Some(table) = sprite.as_table_mut() {
                            table.insert("flip_x".to_string(), toml::Value::Boolean(flip_x));
                            table.insert("flip_y".to_string(), toml::Value::Boolean(flip_y));
                        }
                    }
                }
            },
        );
    }

    // set_sprite_tint(entity_id, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_sprite_tint",
            move |id: i64, r: f64, g: f64, b: f64, a: f64| {
                if id < 0 {
                    return;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_mut() };
                let eid = EntityId::from_raw(id as u64);
                if let Some(components) = world.get_components_mut(eid) {
                    if let Some(sprite) = components.get_mut(comp::SPRITE) {
                        if let Some(table) = sprite.as_table_mut() {
                            table.insert(
                                "tint".to_string(),
                                toml::Value::Array(vec![
                                    toml::Value::Float(r),
                                    toml::Value::Float(g),
                                    toml::Value::Float(b),
                                    toml::Value::Float(a),
                                ]),
                            );
                        }
                    }
                }
            },
        );
    }

    // set_sprite_layer(entity_id, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_sprite_layer", move |id: i64, layer: i64| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components_mut(eid) {
                if let Some(sprite) = components.get_mut(comp::SPRITE) {
                    if let Some(table) = sprite.as_table_mut() {
                        table.insert("layer".to_string(), toml::Value::Integer(layer));
                    }
                }
            }
        });
    }

    // set_sprite_visible(entity_id, visible)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_sprite_visible", move |id: i64, visible: bool| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components_mut(eid) {
                if let Some(sprite) = components.get_mut(comp::SPRITE) {
                    if let Some(table) = sprite.as_table_mut() {
                        table.insert("visible".to_string(), toml::Value::Boolean(visible));
                    }
                }
            }
        });
    }

    // get_sprite_layer(entity_id) -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("get_sprite_layer", move |id: i64| -> i64 {
            if id < 0 {
                return 0;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components(eid) {
                if let Some(sprite) = components.get(comp::SPRITE) {
                    return sprite
                        .get("layer")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(0);
                }
            }
            0
        });
    }
}

// ─── Sprite Animation API ──────────────────────────────────────────

fn register_sprite_animation_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // sprite_play(entity_id, clip_name) — start playing a sprite animation clip
    {
        let ctx = ctx.clone();
        engine.register_fn("sprite_play", move |id: i64, clip_name: String| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components_mut(eid) {
                components.set_field(
                    comp::SPRITE_ANIMATOR,
                    "clip",
                    toml::Value::String(clip_name),
                );
                components.set_field(comp::SPRITE_ANIMATOR, "playing", toml::Value::Boolean(true));
            }
        });
    }

    // sprite_stop(entity_id) — stop sprite animation
    {
        let ctx = ctx.clone();
        engine.register_fn("sprite_stop", move |id: i64| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components_mut(eid) {
                components.set_field(
                    comp::SPRITE_ANIMATOR,
                    "playing",
                    toml::Value::Boolean(false),
                );
            }
        });
    }

    // sprite_set_speed(entity_id, speed) — set animation playback speed
    {
        let ctx = ctx.clone();
        engine.register_fn("sprite_set_speed", move |id: i64, speed: f64| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components_mut(eid) {
                components.set_field(comp::SPRITE_ANIMATOR, "speed", toml::Value::Float(speed));
            }
        });
    }

    // sprite_is_playing(entity_id) -> bool — check if sprite animation is playing
    {
        let ctx = ctx.clone();
        engine.register_fn("sprite_is_playing", move |id: i64| -> bool {
            if id < 0 {
                return false;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(components) = world.get_components(eid) {
                if let Some(sa) = components.get(comp::SPRITE_ANIMATOR) {
                    return sa.get("playing").and_then(|v| v.as_bool()).unwrap_or(false);
                }
            }
            false
        });
    }
}

// ─── Touch API ────────────────────────────────────────────

fn register_touch_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // touch_count() -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("touch_count", move || -> i64 {
            let c = crate::lock_or_recover(&ctx);
            c.input.touches.len() as i64
        });
    }

    // touch_x(id: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("touch_x", move |id: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touches
                .iter()
                .find(|(tid, _, _)| *tid == id)
                .map(|(_, x, _)| *x)
                .unwrap_or(-1.0)
        });
    }

    // touch_y(id: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("touch_y", move |id: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touches
                .iter()
                .find(|(tid, _, _)| *tid == id)
                .map(|(_, _, y)| *y)
                .unwrap_or(-1.0)
        });
    }

    // is_touching(id: i64) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_touching", move |id: i64| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.input.touches.iter().any(|(tid, _, _)| *tid == id)
        });
    }

    // touch_just_started(id: i64) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("touch_just_started", move |id: i64| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_just_started
                .iter()
                .any(|(tid, _, _)| *tid == id)
        });
    }

    // touch_just_ended(id: i64) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("touch_just_ended", move |id: i64| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.input.touch_just_ended.iter().any(|tid| *tid == id)
        });
    }

    // tap_count() -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("tap_count", move || -> i64 {
            let c = crate::lock_or_recover(&ctx);
            c.input.touch_taps.len() as i64
        });
    }

    // tap_x(index: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("tap_x", move |index: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_taps
                .get(index as usize)
                .map(|(x, _)| *x)
                .unwrap_or(-1.0)
        });
    }

    // tap_y(index: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("tap_y", move |index: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_taps
                .get(index as usize)
                .map(|(_, y)| *y)
                .unwrap_or(-1.0)
        });
    }

    // swipe_count() -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("swipe_count", move || -> i64 {
            let c = crate::lock_or_recover(&ctx);
            c.input.touch_swipes.len() as i64
        });
    }

    // swipe_direction(index: i64) -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("swipe_direction", move |index: i64| -> String {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_swipes
                .get(index as usize)
                .map(|(dir, _, _)| dir.clone())
                .unwrap_or_default()
        });
    }

    // swipe_x(index: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("swipe_x", move |index: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_swipes
                .get(index as usize)
                .map(|(_, x, _)| *x)
                .unwrap_or(-1.0)
        });
    }

    // swipe_y(index: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("swipe_y", move |index: i64| -> f64 {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_swipes
                .get(index as usize)
                .map(|(_, _, y)| *y)
                .unwrap_or(-1.0)
        });
    }

    // is_swipe(direction: &str) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_swipe", move |direction: &str| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.input
                .touch_swipes
                .iter()
                .any(|(dir, _, _)| dir == direction)
        });
    }
}

// ─── Camera 2D API (follow, shake) ──────────────────────

fn register_camera_2d_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // camera_follow(entity_id, offset_x, offset_y, speed, deadzone_w, deadzone_h)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "camera_follow",
            move |entity_id: i64,
                  offset_x: f64,
                  offset_y: f64,
                  speed: f64,
                  deadzone_w: f64,
                  deadzone_h: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                let dt = c.delta_time as f32;
                if dt <= 0.0 {
                    return;
                }

                // Read target entity position
                let (target_x, target_y) = {
                    let world = unsafe { c.world_ref() };
                    match world.get_transform(flint_core::EntityId::from_raw(entity_id as u64)) {
                        Some(t) => (
                            t.position.x + offset_x as f32,
                            t.position.y + offset_y as f32,
                        ),
                        None => return,
                    }
                };

                // Initialize follow position on first call
                if !c.camera_follow.initialized {
                    c.camera_follow.current_x = target_x;
                    c.camera_follow.current_y = target_y;
                    c.camera_follow.initialized = true;
                }

                // Deadzone: only track when target is outside deadzone rectangle
                let dx = target_x - c.camera_follow.current_x;
                let dy = target_y - c.camera_follow.current_y;
                let half_w = deadzone_w as f32 * 0.5;
                let half_h = deadzone_h as f32 * 0.5;

                // Compute tracking target — nearest edge of deadzone
                let track_x = if dx.abs() > half_w {
                    target_x - dx.signum() * half_w
                } else {
                    c.camera_follow.current_x
                };
                let track_y = if dy.abs() > half_h {
                    target_y - dy.signum() * half_h
                } else {
                    c.camera_follow.current_y
                };

                // Frame-rate-independent exponential smoothing
                let t = 1.0 - (-(speed as f32) * dt).exp();
                c.camera_follow.current_x += (track_x - c.camera_follow.current_x) * t;
                c.camera_follow.current_y += (track_y - c.camera_follow.current_y) * t;

                // Advance shake
                let mut shake_offset_x = 0.0f32;
                let mut shake_offset_y = 0.0f32;
                if c.shake.amplitude > 0.001 {
                    c.shake.phase += dt;
                    c.shake.amplitude *= (-(c.shake.decay) * dt).exp();
                    let freq = c.shake.frequency;
                    shake_offset_x = c.shake.amplitude * (freq * c.shake.phase).sin();
                    shake_offset_y = c.shake.amplitude * (freq * 1.37 * c.shake.phase).cos();
                    if c.shake.amplitude < 0.001 {
                        c.shake.amplitude = 0.0;
                        c.shake.phase = 0.0;
                    }
                }

                let final_x = c.camera_follow.current_x + shake_offset_x;
                let final_y = c.camera_follow.current_y + shake_offset_y;

                // Z=10 ensures all sprite layers (z_offset = layer*0.01) are in front of the camera
                c.camera_position_override = Some([final_x, final_y, 10.0]);
                c.camera_target_override = Some([final_x, final_y, -1.0]);
            },
        );
    }

    // camera_follow_position() -> #{x, y}
    {
        let ctx = ctx.clone();
        engine.register_fn("camera_follow_position", move || -> Map {
            let c = crate::lock_or_recover(&ctx);
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.camera_follow.current_x as f64));
            map.insert("y".into(), Dynamic::from(c.camera_follow.current_y as f64));
            map
        });
    }

    // camera_follow_set(x, y) — teleport follow position
    {
        let ctx = ctx.clone();
        engine.register_fn("camera_follow_set", move |x: f64, y: f64| {
            let mut c = crate::lock_or_recover(&ctx);
            c.camera_follow.current_x = x as f32;
            c.camera_follow.current_y = y as f32;
            c.camera_follow.initialized = true;
        });
    }

    // camera_shake(amplitude, frequency, decay)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "camera_shake",
            move |amplitude: f64, frequency: f64, decay: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                // Stack: take max of current + new amplitude
                let new_amp = (c.shake.amplitude + amplitude as f32).max(amplitude as f32);
                c.shake.amplitude = new_amp;
                c.shake.frequency = frequency as f32;
                c.shake.decay = decay as f32;
                // Don't reset phase — let it continue naturally
            },
        );
    }

    // camera_shake_stop()
    {
        let ctx = ctx.clone();
        engine.register_fn("camera_shake_stop", move || {
            let mut c = crate::lock_or_recover(&ctx);
            c.shake.amplitude = 0.0;
            c.shake.phase = 0.0;
        });
    }

    // camera_apply_shake() — for manual camera control without camera_follow
    {
        let ctx = ctx.clone();
        engine.register_fn("camera_apply_shake", move || {
            let mut c = crate::lock_or_recover(&ctx);
            let dt = c.delta_time as f32;
            if dt <= 0.0 || c.shake.amplitude < 0.001 {
                return;
            }

            c.shake.phase += dt;
            c.shake.amplitude *= (-(c.shake.decay) * dt).exp();
            let freq = c.shake.frequency;
            let shake_x = c.shake.amplitude * (freq * c.shake.phase).sin();
            let shake_y = c.shake.amplitude * (freq * 1.37 * c.shake.phase).cos();

            if c.shake.amplitude < 0.001 {
                c.shake.amplitude = 0.0;
                c.shake.phase = 0.0;
            }

            // Apply shake offset to existing camera override (or current camera position)
            if let Some(pos) = c.camera_position_override.as_mut() {
                pos[0] += shake_x;
                pos[1] += shake_y;
            } else {
                let base = c.camera_position;
                c.camera_position_override = Some([base[0] + shake_x, base[1] + shake_y, base[2]]);
            }
            if let Some(target) = c.camera_target_override.as_mut() {
                target[0] += shake_x;
                target[1] += shake_y;
            }
        });
    }
}

// ─── Chunk Streaming API ────────────────────────────────

fn register_chunk_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // load_chunk(path, offset_x, offset_y, chunk_id)
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "load_chunk",
            move |path: &str, offset_x: f64, offset_y: f64, chunk_id: &str| {
                let mut c = crate::lock_or_recover(&ctx);
                c.commands.push(ScriptCommand::LoadChunk {
                    path: path.to_string(),
                    offset_x,
                    offset_y,
                    chunk_id: chunk_id.to_string(),
                });
            },
        );
    }

    // unload_chunk(chunk_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("unload_chunk", move |chunk_id: &str| {
            let mut c = crate::lock_or_recover(&ctx);
            c.commands.push(ScriptCommand::UnloadChunk {
                chunk_id: chunk_id.to_string(),
            });
        });
    }

    // is_chunk_loaded(chunk_id) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_chunk_loaded", move |chunk_id: &str| -> bool {
            let c = crate::lock_or_recover(&ctx);
            c.loaded_chunk_ids.contains(chunk_id)
        });
    }
}

// ─── Screen UI API ──────────────────────────────────────

fn register_screen_ui_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // ui_set_value(entity_id, value) — set ui_fill.value
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_value", move |id: i64, value: f64| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                comps.set_field(comp::UI_FILL, "value", toml::Value::Float(value));
            }
        });
    }

    // set_text(entity_id, text) — set ui_text.text
    {
        let ctx = ctx.clone();
        engine.register_fn("set_text", move |id: i64, text: &str| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                comps.set_field(comp::UI_TEXT, "text", toml::Value::String(text.to_string()));
            }
        });
    }

    // set_text_color(entity_id, r, g, b, a) — set ui_text.color
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_text_color",
            move |id: i64, r: f64, g: f64, b: f64, a: f64| {
                if id < 0 {
                    return;
                }
                let c = crate::lock_or_recover(&ctx);
                let world = unsafe { c.world_mut() };
                if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                    let color = toml::Value::Array(vec![
                        toml::Value::Float(r),
                        toml::Value::Float(g),
                        toml::Value::Float(b),
                        toml::Value::Float(a),
                    ]);
                    comps.set_field(comp::UI_TEXT, "color", color);
                }
            },
        );
    }

    // set_anchor(entity_id, anchor_name) — set screen_anchor.anchor
    {
        let ctx = ctx.clone();
        engine.register_fn("set_anchor", move |id: i64, anchor: &str| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                comps.set_field(
                    comp::SCREEN_ANCHOR,
                    "anchor",
                    toml::Value::String(anchor.to_string()),
                );
            }
        });
    }

    // set_anchor_offset(entity_id, x, y) — set screen_anchor.offset_x/y
    {
        let ctx = ctx.clone();
        engine.register_fn("set_anchor_offset", move |id: i64, x: f64, y: f64| {
            if id < 0 {
                return;
            }
            let c = crate::lock_or_recover(&ctx);
            let world = unsafe { c.world_mut() };
            if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                comps.set_field(comp::SCREEN_ANCHOR, "offset_x", toml::Value::Float(x));
                comps.set_field(comp::SCREEN_ANCHOR, "offset_y", toml::Value::Float(y));
            }
        });
    }
}

// ─── Text option helpers ─────────────────────────────────

/// Read a Rhai number (INT or FLOAT) as f32; anything else is `None`.
fn dyn_to_f32(d: &Dynamic) -> Option<f32> {
    if let Some(f) = d.clone().try_cast::<f64>() {
        Some(f as f32)
    } else {
        d.clone().try_cast::<i64>().map(|i| i as f32)
    }
}

/// Convert a Rhai array of numbers (INT or FLOAT mixed) to f32s, skipping non-numbers.
pub(crate) fn array_to_f32s(arr: &Array) -> Vec<f32> {
    arr.iter().filter_map(dyn_to_f32).collect()
}

/// Parsed `draw_text_opts` option map. Every key is optional; defaults are
/// white, left-aligned, layer 0, no stroke/shadow, default font.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextOpts {
    pub color: [f32; 4],
    pub align: u8,
    pub layer: i32,
    pub stroke: Option<([f32; 4], f32)>,
    pub font: Option<String>,
    pub spacing: f32,
    pub shadow: Option<([f32; 4], f32, f32)>,
    /// A blurred halo under the stroke: colour and reach in points (#626).
    pub glow: Option<([f32; 4], f32)>,
}

impl Default for TextOpts {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            align: 0,
            layer: 0,
            stroke: None,
            font: None,
            spacing: 0.0,
            shadow: None,
            glow: None,
        }
    }
}

impl TextOpts {
    pub(crate) fn from_map(opts: &Map) -> Self {
        let mut o = Self::default();
        if let Some(v) = opts.get("font") {
            if let Some(s) = v.clone().try_cast::<ImmutableString>() {
                if !s.is_empty() {
                    o.font = Some(s.to_string());
                }
            }
        }
        if let Some(v) = opts.get("color") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a] = array_to_f32s(&arr)[..] {
                    o.color = [r, g, b, a];
                }
            }
        }
        if let Some(v) = opts.get("align") {
            if let Some(s) = v.clone().try_cast::<ImmutableString>() {
                o.align = match s.as_str() {
                    "center" => 1,
                    "right" => 2,
                    _ => 0,
                };
            } else if let Some(i) = dyn_to_f32(v) {
                o.align = (i as i64).clamp(0, 2) as u8;
            }
        }
        if let Some(v) = opts.get("stroke") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a, w] = array_to_f32s(&arr)[..] {
                    o.stroke = Some(([r, g, b, a], w));
                }
            }
        }
        if let Some(v) = opts.get("spacing") {
            if let Some(f) = dyn_to_f32(v) {
                o.spacing = f;
            }
        }
        if let Some(v) = opts.get("layer") {
            if let Some(f) = dyn_to_f32(v) {
                o.layer = f as i32;
            }
        }
        if let Some(v) = opts.get("shadow") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [dx, dy, r, g, b, a] = array_to_f32s(&arr)[..] {
                    o.shadow = Some(([r, g, b, a], dx, dy));
                }
            }
        }
        if let Some(v) = opts.get("glow") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a, radius] = array_to_f32s(&arr)[..] {
                    o.glow = Some(([r, g, b, a], radius));
                }
            }
        }
        o
    }
}

/// A `draw_world_text` call as the host draws it: `draw_text_opts`'s
/// options, centred and middled on the point unless `align` or `valign`
/// say otherwise, and on a plane when `facing` is a usable normal.
pub(crate) fn world_text_from(
    at: [f32; 3],
    text: &str,
    size: f32,
    opts: &Map,
) -> flint_core::world_text::WorldText {
    let o = TextOpts::from_map(opts);
    let align = if opts.contains_key("align") {
        o.align
    } else {
        1
    };
    let valign = match opts.get("valign") {
        Some(v) => match v.clone().try_cast::<ImmutableString>() {
            Some(s) => match s.as_str() {
                "top" => 0,
                "bottom" => 2,
                _ => 1,
            },
            None => dyn_to_f32(v).map_or(1, |i| (i as i64).clamp(0, 2) as u8),
        },
        None => 1,
    };
    let vec3 = |key: &str| -> Option<[f32; 3]> {
        let arr = opts.get(key)?.clone().try_cast::<Array>()?;
        match array_to_f32s(&arr)[..] {
            [x, y, z] => Some([x, y, z]),
            _ => None,
        }
    };
    let plane = vec3("facing")
        .filter(|n| n.iter().any(|c| *c != 0.0))
        .map(|normal| flint_core::world_text::TextPlane {
            normal,
            up: vec3("up").unwrap_or([0.0, 1.0, 0.0]),
        });
    flint_core::world_text::WorldText {
        at,
        text: text.to_string(),
        size,
        color: o.color,
        align,
        valign,
        stroke: o.stroke,
        font: o.font,
        spacing: o.spacing,
        shadow: o.shadow,
        glow: o.glow,
        plane,
    }
}

/// Measure text with the host's installed measurer, falling back to the
/// glyph-count estimate (0.6 em per char) when none is installed.
pub(crate) fn measure_text_with(
    c: &ScriptCallContext,
    text: &str,
    size: f32,
    font: Option<&str>,
    spacing: f32,
) -> (f32, f32) {
    match &c.text_measurer {
        Some(m) => m(text, size, font, spacing),
        None => {
            let chars = text.chars().count() as f32;
            (chars * size * 0.6 + spacing * (chars - 1.0).max(0.0), size)
        }
    }
}

#[cfg(test)]
mod text_opts_tests {
    use super::*;

    fn arr(vals: &[Dynamic]) -> Dynamic {
        Dynamic::from(vals.to_vec())
    }

    #[test]
    fn empty_opts_are_white_left_layer0() {
        let o = TextOpts::from_map(&Map::new());
        assert_eq!(o, TextOpts::default());
        assert_eq!(o.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn parses_every_key_with_mixed_int_and_float() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from("display"));
        m.insert(
            "color".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0.5_f64),
                Dynamic::from(0_i64),
                Dynamic::from(1.0_f64),
            ]),
        );
        m.insert("align".into(), Dynamic::from("center"));
        m.insert(
            "stroke".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(1_i64),
                Dynamic::from(2.0_f64),
            ]),
        );
        m.insert("spacing".into(), Dynamic::from(1.5_f64));
        m.insert("layer".into(), Dynamic::from(3_i64));
        m.insert(
            "shadow".into(),
            arr(&[
                Dynamic::from(2_i64),
                Dynamic::from(3.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.6_f64),
            ]),
        );
        m.insert(
            "glow".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0.2_f64),
                Dynamic::from(0.6_f64),
                Dynamic::from(0.9_f64),
                Dynamic::from(18_i64),
            ]),
        );
        let o = TextOpts::from_map(&m);
        assert_eq!(o.glow, Some(([1.0, 0.2, 0.6, 0.9], 18.0)));
        assert_eq!(world_text_from([0.0; 3], "x", 1.0, &m).glow, o.glow);
        assert_eq!(o.font.as_deref(), Some("display"));
        assert_eq!(o.color, [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(o.align, 1);
        assert_eq!(o.stroke, Some(([0.0, 0.0, 0.0, 1.0], 2.0)));
        assert_eq!(o.spacing, 1.5);
        assert_eq!(o.layer, 3);
        assert_eq!(o.shadow, Some(([0.0, 0.0, 0.0, 0.6], 2.0, 3.0)));
    }

    #[test]
    fn malformed_arrays_and_empty_font_are_ignored() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from(""));
        m.insert("color".into(), arr(&[Dynamic::from(1.0_f64)]));
        m.insert("align".into(), Dynamic::from("right"));
        m.insert("stroke".into(), Dynamic::from("nope"));
        m.insert("glow".into(), arr(&vec![Dynamic::from(1.0_f64); 4]));
        let o = TextOpts::from_map(&m);
        assert_eq!(o.glow, None, "a glow needs five numbers");
        assert_eq!(o.font, None);
        assert_eq!(o.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(o.align, 2);
        assert_eq!(o.stroke, None);
    }

    #[test]
    fn world_text_is_a_centred_billboard_by_default() {
        let t = world_text_from([1.0, 2.0, 3.0], "Ada's", 14.0, &Map::new());
        assert_eq!(t.at, [1.0, 2.0, 3.0]);
        assert_eq!(t.text, "Ada's");
        assert_eq!((t.align, t.valign), (1, 1));
        assert_eq!(t.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(t.plane, None);
    }

    #[test]
    fn world_text_takes_the_text_opts_and_a_plane() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from("display"));
        m.insert("align".into(), Dynamic::from("left"));
        m.insert("valign".into(), Dynamic::from("bottom"));
        m.insert(
            "facing".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(-1.0_f64),
            ]),
        );
        let t = world_text_from([0.0; 3], "x", 0.2, &m);
        assert_eq!(t.font.as_deref(), Some("display"));
        assert_eq!((t.align, t.valign), (0, 2));
        let plane = t.plane.unwrap();
        assert_eq!(plane.normal, [0.0, 0.0, -1.0]);
        assert_eq!(plane.up, [0.0, 1.0, 0.0]);

        m.insert(
            "up".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
            ]),
        );
        assert_eq!(
            world_text_from([0.0; 3], "x", 0.2, &m).plane.unwrap().up,
            [1.0, 0.0, 0.0]
        );
    }

    #[test]
    fn world_text_without_a_usable_normal_is_a_billboard() {
        let mut m = Map::new();
        m.insert(
            "facing".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
            ]),
        );
        assert_eq!(world_text_from([0.0; 3], "x", 9.0, &m).plane, None);
        m.insert("facing".into(), arr(&[Dynamic::from(1_i64)]));
        assert_eq!(world_text_from([0.0; 3], "x", 9.0, &m).plane, None);
    }

    #[test]
    fn measure_uses_installed_measurer() {
        let mut c = ScriptCallContext::new();
        assert_eq!(measure_text_with(&c, "abcd", 10.0, None, 0.0), (24.0, 10.0));
        c.text_measurer = Some(Arc::new(
            |text: &str, size: f32, font: Option<&str>, sp: f32| {
                let base = if font == Some("display") { 2.0 } else { 1.0 };
                (text.len() as f32 * size * base + sp, size * 1.5)
            },
        ));
        assert_eq!(measure_text_with(&c, "abcd", 10.0, None, 0.0), (40.0, 15.0));
        assert_eq!(
            measure_text_with(&c, "ab", 10.0, Some("display"), 1.0),
            (41.0, 15.0)
        );
    }

    #[test]
    fn style_array_maps_by_length() {
        use crate::ui::element::StyleValue;
        assert_eq!(
            StyleValue::from_numbers(&array_to_f32s(&vec![
                Dynamic::from(1_i64),
                Dynamic::from(2.0_f64),
                Dynamic::from(3_i64),
                Dynamic::from(4_i64)
            ])),
            Some(StyleValue::Color([1.0, 2.0, 3.0, 4.0]))
        );
        assert_eq!(
            StyleValue::from_numbers(&[1.0, 2.0, 0.0, 0.0, 0.0, 1.0]),
            Some(StyleValue::Shadow(1.0, 2.0, [0.0, 0.0, 0.0, 1.0]))
        );
        assert_eq!(StyleValue::from_numbers(&[1.0, 2.0]), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quality_engine() -> (Engine, Arc<Mutex<ScriptCallContext>>) {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut engine = Engine::new();
        register_quality_api(&mut engine, ctx.clone());
        (engine, ctx)
    }

    #[test]
    fn a_script_sets_and_reads_quality() {
        let (engine, ctx) = quality_engine();
        let ok: bool = engine
            .eval(
                "set_quality(#{ msaa: 4, shadow_cascades: 2, ssao: false, render_scale: 0.75, \
                 shadow_distance: 90 })",
            )
            .unwrap();
        assert!(ok);
        {
            let c = ctx.lock().unwrap();
            assert!(c.quality_requested);
            assert_eq!(c.quality.msaa, 4);
            assert_eq!(c.quality.shadow_cascades, 2);
            assert!(!c.quality.ssao);
            assert_eq!(c.quality.render_scale, 0.75);
            // An integer is fine where a float is wanted.
            assert_eq!(c.quality.shadow_distance, 90.0);
        }
        // Read back at once, clamped.
        let msaa: i64 = engine.eval("get_quality().msaa").unwrap();
        assert_eq!(msaa, 4);
        let scale: f64 = engine.eval("get_quality(\"render_scale\")").unwrap();
        assert_eq!(scale, 0.75);
        let keys: i64 = engine.eval("get_quality().len()").unwrap();
        assert_eq!(
            keys as usize,
            flint_core::quality::QualitySettings::KEYS.len()
        );
        let unknown: bool = engine.eval("get_quality(\"sharpness\") == ()").unwrap();
        assert!(unknown);
    }

    #[test]
    fn a_bad_key_is_skipped_and_the_rest_applied() {
        let (engine, ctx) = quality_engine();
        let ok: bool = engine
            .eval("set_quality(#{ bloom: false, sharpness: 3, msaa: \"lots\" })")
            .unwrap();
        assert!(!ok);
        let c = ctx.lock().unwrap();
        assert!(!c.quality.bloom);
        assert_eq!(c.quality.msaa, 1);
    }

    #[test]
    fn setting_what_is_already_set_asks_for_nothing() {
        let (engine, ctx) = quality_engine();
        let ok: bool = engine.eval("set_quality(\"msaa\", 1)").unwrap();
        assert!(ok);
        assert!(!ctx.lock().unwrap().quality_requested);
        let _: bool = engine.eval("set_quality(\"shadows\", false)").unwrap();
        assert!(ctx.lock().unwrap().quality_requested);
    }

    #[test]
    fn a_script_reads_the_gpu() {
        let (engine, ctx) = quality_engine();
        ctx.lock().unwrap().gpu_info = flint_core::quality::GpuInfo {
            name: "Mali-G715".into(),
            class: "mobile".into(),
            platform: "android".into(),
            ..Default::default()
        };
        let class: String = engine.eval("gpu_info().class").unwrap();
        assert_eq!(class, "mobile");
        let name: String = engine.eval("gpu_info().name").unwrap();
        assert_eq!(name, "Mali-G715");
    }

    /// `load_data` and `load_ui` paths are from the project root (ADR
    /// 0114): the same file from a scene in `scenes/` and from one two
    /// folders deep, which used to look under `scenes/`.
    #[test]
    fn test_root_paths_are_from_the_project_root_at_any_depth() {
        use flint_core::project::Project;
        use flint_core::vfs::{self, MemorySource};
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "[project]\nid = \"g\"\n");
        files.insert("game/data/x.toml", "n = 7\n");
        files.insert("game/scenes/a.scene.toml", "");
        files.insert("game/scenes/act1/deep/b.scene.toml", "");
        // TEMPORARY (one release): a file only where the root used to be
        // guessed for the deep scene is still read
        files.insert("game/scenes/act1/data/old.toml", "n = 3\n");
        // No marker: the scene folder's parent, as before
        files.insert("loose/levels/c.scene.toml", "");
        files.insert("loose/data/x.toml", "n = 9\n");
        vfs::scoped(Arc::new(files), || {
            let n = |scene: &str, handed: bool, file: &str| -> Option<i64> {
                let mut c = ScriptCallContext::new();
                c.current_scene_path = scene.to_string();
                // With the project handed in by the host, and found from
                // the scene path when it wasn't
                if handed {
                    c.project = Some(Project::for_scene(std::path::Path::new(scene)));
                }
                let d = load_data_impl(&mut c, file, false);
                d.is_map().then(|| d.cast::<Map>()["n"].as_int().unwrap())
            };
            for handed in [true, false] {
                assert_eq!(
                    n("game/scenes/a.scene.toml", handed, "data/x.toml"),
                    Some(7)
                );
                assert_eq!(
                    n("game/scenes/act1/deep/b.scene.toml", handed, "data/x.toml"),
                    Some(7)
                );
                assert_eq!(
                    n(
                        "game/scenes/act1/deep/b.scene.toml",
                        handed,
                        "data/old.toml"
                    ),
                    Some(3)
                );
                assert_eq!(n("game/scenes/a.scene.toml", handed, "data/old.toml"), None);
                assert_eq!(
                    n("loose/levels/c.scene.toml", handed, "data/x.toml"),
                    Some(9)
                );
            }
        });
    }

    #[test]
    fn test_load_data_types_cache_and_missing() {
        let root = std::env::temp_dir().join(format!(
            "flint_data_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(root.join("data/x.toml"), "n = 7\nf = 1.5\nb = true\ns = \"hi\"\n[nested]\nk = 2\n[[list]]\nid = \"a\"\n[[list]]\nid = \"b\"\n").unwrap();

        let mut c = ScriptCallContext::new();
        c.current_scene_path = root
            .join("scenes/x.scene.toml")
            .to_string_lossy()
            .to_string();

        let d = load_data_impl(&mut c, "data/x.toml", false);
        assert!(d.is_map());
        let m = d.cast::<Map>();
        assert!(m["n"].is_int());
        assert_eq!(m["n"].as_int().unwrap(), 7);
        assert!(m["f"].is_float());
        assert_eq!(m["b"].as_bool().unwrap(), true);
        assert_eq!(m["s"].clone().into_string().unwrap(), "hi");
        assert_eq!(m["nested"].clone().cast::<Map>()["k"].as_int().unwrap(), 2);
        assert_eq!(m["list"].clone().cast::<Array>().len(), 2);
        assert_eq!(c.data_cache.len(), 1);

        // Cached: the file can change on disk and the same parse comes back
        std::fs::write(root.join("data/x.toml"), "n = 8\n").unwrap();
        let d2 = load_data_impl(&mut c, "data/x.toml", false);
        assert_eq!(d2.cast::<Map>()["n"].as_int().unwrap(), 7);
        // ...until a reload is asked for
        let d3 = load_data_impl(&mut c, "data/x.toml", true);
        assert_eq!(d3.cast::<Map>()["n"].as_int().unwrap(), 8);

        // Missing file: unit, and a warning queued for the game log
        let d4 = load_data_impl(&mut c, "data/missing.toml", false);
        assert!(d4.is_unit());
        assert!(c.commands.iter().any(|cmd| matches!(
            cmd,
            ScriptCommand::Log {
                level: LogLevel::Warn,
                ..
            }
        )));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_toml_to_dynamic_bool() {
        let d = toml_to_dynamic(toml::Value::Boolean(true));
        assert_eq!(d.as_bool().unwrap(), true);
    }

    #[test]
    fn test_toml_to_dynamic_integer() {
        let d = toml_to_dynamic(toml::Value::Integer(42));
        assert_eq!(d.as_int().unwrap(), 42);
    }

    #[test]
    fn test_toml_to_dynamic_float() {
        let d = toml_to_dynamic(toml::Value::Float(3.25));
        assert!((d.as_float().unwrap() - 3.25).abs() < 1e-10);
    }

    #[test]
    fn test_toml_to_dynamic_string() {
        let d = toml_to_dynamic(toml::Value::String("hello".into()));
        assert_eq!(d.into_string().unwrap(), "hello");
    }

    #[test]
    fn test_toml_to_dynamic_array() {
        let arr = toml::Value::Array(vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::Integer(3),
        ]);
        let d = toml_to_dynamic(arr);
        let items = d.into_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_int().unwrap(), 1);
    }

    #[test]
    fn test_dynamic_to_toml_roundtrip() {
        let original = toml::Value::Float(2.75);
        let dynamic = toml_to_dynamic(original.clone());
        let back = dynamic_to_toml(&dynamic).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn test_dynamic_to_toml_string() {
        let d = Dynamic::from("test".to_string());
        let t = dynamic_to_toml(&d).unwrap();
        assert_eq!(t.as_str().unwrap(), "test");
    }

    #[test]
    fn test_dynamic_to_toml_bool() {
        let d = Dynamic::from(true);
        let t = dynamic_to_toml(&d).unwrap();
        assert_eq!(t.as_bool().unwrap(), true);
    }
}

/// Project a world-space segment with the frame's camera and record it as
/// a HUD line. Clipped against the near plane so a segment that passes the
/// camera still draws; fully-behind segments are dropped.
fn push_line_3d(
    c: &mut ScriptCallContext,
    a: [f64; 3],
    b: [f64; 3],
    color: [f32; 4],
    thickness: f32,
    layer: i32,
) {
    let mut p = project_clip(&c.camera_view_proj, [a[0] as f32, a[1] as f32, a[2] as f32]);
    let mut q = project_clip(&c.camera_view_proj, [b[0] as f32, b[1] as f32, b[2] as f32]);
    const NEAR_W: f32 = 1e-3;
    if p[3] <= NEAR_W && q[3] <= NEAR_W {
        return;
    }
    if p[3] <= NEAR_W || q[3] <= NEAR_W {
        // Move the behind-camera end onto the near plane (linear in clip space).
        let t = (NEAR_W - p[3]) / (q[3] - p[3]);
        let m = [
            p[0] + (q[0] - p[0]) * t,
            p[1] + (q[1] - p[1]) * t,
            p[2] + (q[2] - p[2]) * t,
            NEAR_W,
        ];
        if p[3] <= NEAR_W {
            p = m;
        } else {
            q = m;
        }
    }
    let (sx1, sy1) = clip_to_screen(p, c.screen_width, c.screen_height);
    let (sx2, sy2) = clip_to_screen(q, c.screen_width, c.screen_height);
    c.push_draw(DrawCommand::Line {
        x1: sx1,
        y1: sy1,
        x2: sx2,
        y2: sy2,
        color,
        thickness,
        layer,
    });
}
