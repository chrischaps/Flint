//! Rhai API function registration
//!
//! All functions accessible from Rhai scripts are registered here.
//! They access the world through the shared ScriptCallContext.
//!
//! ADR 0116 moves them, a module at a time, to `api/<module>.rs`, declared
//! through the builder (`api/builder.rs`) into the registry
//! (`api/registry.rs`) that `flint script api` lists. Done: `entity`,
//! `input`, `time`, `audio`, `animation`, `physics` (and `physics_2d`),
//! `camera`, `post_process`, `math`, `event`, `log`, `draw`,
//! `draw_world`, `view`, `ui`, `screen_ui`, `sprite`, `camera_2d` and
//! `touch`; the `register_*_api` functions below are the rest (#697).

use crate::context::{LogLevel, ScriptCallContext, ScriptCommand};
use flint_core::components as comp;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::toml_util::toml_f64;
use flint_core::vfs;
use flint_core::EntityId;
use rhai::{Array, Dynamic, Engine, Map};
use std::sync::{Arc, Mutex};

mod animation;
mod audio;
pub mod builder;
mod camera;
mod camera_2d;
#[cfg(test)]
mod declared_tests;
mod draw;
mod draw_world;
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
mod screen_ui;
mod sprite;
mod text_opts;
mod time;
mod touch;
mod ui;
mod view;

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
    draw::register(api);
    draw_world::register(api);
    view::register(api);
    ui::register(api);
    screen_ui::register(api);
    sprite::register(api);
    camera_2d::register(api);
    touch::register(api);
    let ctx = api.ctx().clone();
    let engine = api.engine_mut();
    register_spline_api(engine, ctx.clone());
    register_particle_api(engine, ctx.clone());
    register_style_api(engine, ctx.clone());
    register_state_api(engine, ctx.clone());
    register_scene_api(engine, ctx.clone());
    register_wooze_api(engine, ctx.clone());
    register_persistence_api(engine, ctx.clone());
    register_data_api(engine, ctx.clone());
    register_terrain_api(engine, ctx.clone());
    register_chunk_api(engine, ctx.clone());
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
