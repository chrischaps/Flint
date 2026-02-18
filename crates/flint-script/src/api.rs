//! Rhai API function registration
//!
//! All functions accessible from Rhai scripts are registered here.
//! They access the world through the shared ScriptCallContext.

use crate::context::{DrawCommand, LogLevel, ScriptCallContext, ScriptCommand};
use flint_core::EntityId;
use rhai::{Dynamic, Engine, Map};
use std::sync::{Arc, Mutex};

/// Register all API functions on the Rhai engine
pub fn register_all(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    register_entity_api(engine, ctx.clone());
    register_spline_api(engine, ctx.clone());
    register_input_api(engine, ctx.clone());
    register_time_api(engine, ctx.clone());
    register_audio_api(engine, ctx.clone());
    register_animation_api(engine, ctx.clone());
    register_physics_api(engine, ctx.clone());
    register_math_api(engine);
    register_event_api(engine, ctx.clone());
    register_ui_api(engine, ctx.clone());
    register_data_ui_api(engine, ctx.clone());
    register_particle_api(engine, ctx.clone());
    register_state_api(engine, ctx.clone());
    register_scene_api(engine, ctx.clone());
    register_persistence_api(engine, ctx.clone());
    register_log_api(engine, ctx);
}

// ─── Entity API ──────────────────────────────────────────

fn register_entity_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // self_entity() -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("self_entity", move || -> i64 {
            let c = ctx.lock().unwrap();
            c.current_entity.raw() as i64
        });
    }

    // this_entity() -> i64 (alias for self_entity)
    {
        let ctx = ctx.clone();
        engine.register_fn("this_entity", move || -> i64 {
            let c = ctx.lock().unwrap();
            c.current_entity.raw() as i64
        });
    }

    // get_entity(name: &str) -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("get_entity", move |name: &str| -> i64 {
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_id(name).map(|id| id.raw() as i64).unwrap_or(-1)
        });
    }

    // entity_exists(id: i64) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("entity_exists", move |id: i64| -> bool {
            if id < 0 { return false; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.contains(EntityId::from_raw(id as u64))
        });
    }

    // entity_name(id: i64) -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("entity_name", move |id: i64| -> String {
            if id < 0 { return String::new(); }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_name(EntityId::from_raw(id as u64))
                .unwrap_or("")
                .to_string()
        });
    }

    // has_component(id: i64, comp: &str) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("has_component", move |id: i64, comp: &str| -> bool {
            if id < 0 { return false; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_components(EntityId::from_raw(id as u64))
                .map(|comps| comps.has(comp))
                .unwrap_or(false)
        });
    }

    // get_component(id: i64, comp: &str) -> Dynamic (returns full component as Map, or () if missing)
    {
        let ctx = ctx.clone();
        engine.register_fn("get_component", move |id: i64, comp: &str| -> Dynamic {
            if id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            match world.get_components(EntityId::from_raw(id as u64)) {
                Some(comps) => {
                    match comps.get(comp) {
                        Some(val) => toml_to_dynamic(val.clone()),
                        None => Dynamic::UNIT,
                    }
                }
                None => Dynamic::UNIT,
            }
        });
    }

    // get_field(id: i64, comp: &str, field: &str) -> Dynamic
    {
        let ctx = ctx.clone();
        engine.register_fn("get_field", move |id: i64, comp: &str, field: &str| -> Dynamic {
            if id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_components(EntityId::from_raw(id as u64))
                .and_then(|comps| comps.get_field(comp, field).cloned())
                .map(toml_to_dynamic)
                .unwrap_or(Dynamic::UNIT)
        });
    }

    // set_field(id: i64, comp: &str, field: &str, val: Dynamic)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_field", move |id: i64, comp: &str, field: &str, val: Dynamic| {
            if id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            if let Some(tv) = dynamic_to_toml(&val) {
                if let Some(comps) = world.get_components_mut(EntityId::from_raw(id as u64)) {
                    comps.set_field(comp, field, tv);
                }
            }
        });
    }

    // get_position(id: i64) -> Map #{x, y, z}
    {
        let ctx = ctx.clone();
        engine.register_fn("get_position", move |id: i64| -> Map {
            let mut map = Map::new();
            if id < 0 { return map; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            if let Some(transform) = world.get_transform(EntityId::from_raw(id as u64)) {
                map.insert("x".into(), Dynamic::from(transform.position.x as f64));
                map.insert("y".into(), Dynamic::from(transform.position.y as f64));
                map.insert("z".into(), Dynamic::from(transform.position.z as f64));
            }
            map
        });
    }

    // set_position(id: i64, x: f64, y: f64, z: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_position", move |id: i64, x: f64, y: f64, z: f64| {
            if id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("transform", "position", toml::Value::Array(vec![
                    toml::Value::Float(x),
                    toml::Value::Float(y),
                    toml::Value::Float(z),
                ]));
            }
        });
    }

    // get_rotation(id: i64) -> Map #{x, y, z}
    {
        let ctx = ctx.clone();
        engine.register_fn("get_rotation", move |id: i64| -> Map {
            let mut map = Map::new();
            if id < 0 { return map; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            if let Some(transform) = world.get_transform(EntityId::from_raw(id as u64)) {
                map.insert("x".into(), Dynamic::from(transform.rotation.x as f64));
                map.insert("y".into(), Dynamic::from(transform.rotation.y as f64));
                map.insert("z".into(), Dynamic::from(transform.rotation.z as f64));
            }
            map
        });
    }

    // set_rotation(id: i64, x: f64, y: f64, z: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_rotation", move |id: i64, x: f64, y: f64, z: f64| {
            if id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("transform", "rotation", toml::Value::Array(vec![
                    toml::Value::Float(x),
                    toml::Value::Float(y),
                    toml::Value::Float(z),
                ]));
            }
        });
    }

    // distance(a: i64, b: i64) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("distance", move |a: i64, b: i64| -> f64 {
            if a < 0 || b < 0 { return f64::MAX; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            let ta = world.get_transform(EntityId::from_raw(a as u64));
            let tb = world.get_transform(EntityId::from_raw(b as u64));
            match (ta, tb) {
                (Some(a), Some(b)) => {
                    let dx = (a.position.x - b.position.x) as f64;
                    let dy = (a.position.y - b.position.y) as f64;
                    let dz = (a.position.z - b.position.z) as f64;
                    (dx * dx + dy * dy + dz * dz).sqrt()
                }
                _ => f64::MAX,
            }
        });
    }

    // spawn_entity(name: &str) -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("spawn_entity", move |name: &str| -> i64 {
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            world.spawn(name).map(|id| id.raw() as i64).unwrap_or(-1)
        });
    }

    // despawn_entity(id: i64)
    {
        let ctx = ctx.clone();
        engine.register_fn("despawn_entity", move |id: i64| {
            if id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let _ = world.despawn(EntityId::from_raw(id as u64));
        });
    }

    // set_parent(child_id: i64, parent_id: i64)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_parent", move |child_id: i64, parent_id: i64| {
            if child_id < 0 || parent_id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let _ = world.set_parent(
                EntityId::from_raw(child_id as u64),
                EntityId::from_raw(parent_id as u64),
            );
        });
    }

    // get_parent(id: i64) -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("get_parent", move |id: i64| -> i64 {
            if id < 0 { return -1; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_parent(EntityId::from_raw(id as u64))
                .map(|pid| pid.raw() as i64)
                .unwrap_or(-1)
        });
    }

    // get_children(id: i64) -> Array
    {
        let ctx = ctx.clone();
        engine.register_fn("get_children", move |id: i64| -> rhai::Array {
            if id < 0 { return vec![]; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.get_children(EntityId::from_raw(id as u64))
                .into_iter()
                .map(|cid| Dynamic::from(cid.raw() as i64))
                .collect()
        });
    }

    // get_world_position(id: i64) -> Map #{x,y,z}
    {
        let ctx = ctx.clone();
        engine.register_fn("get_world_position", move |id: i64| -> Map {
            let mut map = Map::new();
            if id < 0 { return map; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            if let Some(pos) = world.get_world_position(EntityId::from_raw(id as u64)) {
                map.insert("x".into(), Dynamic::from(pos.x as f64));
                map.insert("y".into(), Dynamic::from(pos.y as f64));
                map.insert("z".into(), Dynamic::from(pos.z as f64));
            }
            map
        });
    }

    // set_material_color(entity_id: i64, r: f64, g: f64, b: f64, a: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_material_color", move |id: i64, r: f64, g: f64, b: f64, a: f64| {
            if id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("material", "base_color_r", toml::Value::Float(r));
                comps.set_field("material", "base_color_g", toml::Value::Float(g));
                comps.set_field("material", "base_color_b", toml::Value::Float(b));
                comps.set_field("material", "base_color_a", toml::Value::Float(a));
            }
        });
    }

    // find_entities_with(component: &str) -> Array of entity IDs
    {
        let ctx = ctx.clone();
        engine.register_fn("find_entities_with", move |comp: &str| -> rhai::Array {
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.all_entities()
                .into_iter()
                .filter(|info| info.components.iter().any(|c| c == comp))
                .map(|info| Dynamic::from(info.id.raw() as i64))
                .collect()
        });
    }

    // entity_count_with(component: &str) -> i64
    {
        let ctx = ctx.clone();
        engine.register_fn("entity_count_with", move |comp: &str| -> i64 {
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            world.all_entities()
                .into_iter()
                .filter(|info| info.components.iter().any(|c| c == comp))
                .count() as i64
        });
    }
}

// ─── Spline API ──────────────────────────────────────────

fn register_spline_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // spline_closest_point(spline_entity, x, y, z) -> Map #{t, x, y, z, dist_sq}
    {
        let ctx = ctx.clone();
        engine.register_fn("spline_closest_point", move |spline_id: i64, qx: f64, qy: f64, qz: f64| -> Dynamic {
            if spline_id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(spline_id as u64);
            let comps = match world.get_components(eid) {
                Some(c) => c,
                None => return Dynamic::UNIT,
            };
            let sd = match comps.get("spline_data") {
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
                let sx = px_arr[i].as_float().unwrap_or(0.0);
                let sy = py_arr[i].as_float().unwrap_or(0.0);
                let sz = pz_arr[i].as_float().unwrap_or(0.0);
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
            map.insert("t".into(), Dynamic::from(t_arr[best_idx].as_float().unwrap_or(0.0)));
            map.insert("x".into(), Dynamic::from(px_arr[best_idx].as_float().unwrap_or(0.0)));
            map.insert("y".into(), Dynamic::from(py_arr[best_idx].as_float().unwrap_or(0.0)));
            map.insert("z".into(), Dynamic::from(pz_arr[best_idx].as_float().unwrap_or(0.0)));
            map.insert("dist_sq".into(), Dynamic::from(best_dist_sq));
            Dynamic::from(map)
        });
    }

    // spline_sample_at(spline_entity, t) -> Map #{x, y, z, fwd_x, fwd_y, fwd_z, right_x, right_y, right_z}
    {
        let ctx = ctx.clone();
        engine.register_fn("spline_sample_at", move |spline_id: i64, t: f64| -> Dynamic {
            if spline_id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            let eid = EntityId::from_raw(spline_id as u64);
            let comps = match world.get_components(eid) {
                Some(c) => c,
                None => return Dynamic::UNIT,
            };
            let sd = match comps.get("spline_data") {
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

            let len = count.min(px_arr.len()).min(py_arr.len()).min(pz_arr.len()).min(t_arr.len());
            if len < 2 { return Dynamic::UNIT; }

            // Wrap t into [0, 1) for closed splines
            let t_wrapped = ((t % 1.0) + 1.0) % 1.0;

            // Binary search for the bracketing segment
            let mut lo = 0usize;
            let mut hi = len - 1;
            while lo + 1 < hi {
                let mid = (lo + hi) / 2;
                if t_arr[mid].as_float().unwrap_or(0.0) <= t_wrapped {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }

            let t0 = t_arr[lo].as_float().unwrap_or(0.0);
            let t1 = t_arr[hi].as_float().unwrap_or(0.0);
            let seg_len = t1 - t0;
            let frac = if seg_len.abs() > 1e-9 { (t_wrapped - t0) / seg_len } else { 0.0 };

            let x0 = px_arr[lo].as_float().unwrap_or(0.0);
            let y0 = py_arr[lo].as_float().unwrap_or(0.0);
            let z0 = pz_arr[lo].as_float().unwrap_or(0.0);
            let x1 = px_arr[hi].as_float().unwrap_or(0.0);
            let y1 = py_arr[hi].as_float().unwrap_or(0.0);
            let z1 = pz_arr[hi].as_float().unwrap_or(0.0);

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
        });
    }
}

// ─── Input API ───────────────────────────────────────────

fn register_input_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // is_action_pressed(action: &str) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_action_pressed", move |action: &str| -> bool {
            let c = ctx.lock().unwrap();
            c.input.actions_pressed.contains(action)
        });
    }

    // is_action_just_pressed(action: &str) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_action_just_pressed", move |action: &str| -> bool {
            let c = ctx.lock().unwrap();
            c.input.actions_just_pressed.contains(action)
        });
    }

    // is_action_just_released(action: &str) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_action_just_released", move |action: &str| -> bool {
            let c = ctx.lock().unwrap();
            c.input.actions_just_released.contains(action)
        });
    }

    // action_value(action: &str) -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("action_value", move |action: &str| -> f64 {
            let c = ctx.lock().unwrap();
            c.input.action_values.get(action).copied().unwrap_or(0.0)
        });
    }

    // mouse_delta_x() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("mouse_delta_x", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.input.mouse_delta.0
        });
    }

    // mouse_delta_y() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("mouse_delta_y", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.input.mouse_delta.1
        });
    }
}

// ─── Time API ────────────────────────────────────────────

fn register_time_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    {
        let ctx = ctx.clone();
        engine.register_fn("delta_time", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.delta_time
        });
    }
    {
        let ctx = ctx.clone();
        engine.register_fn("total_time", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.total_time
        });
    }
}

// ─── Audio API ───────────────────────────────────────────

fn register_audio_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // play_sound(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("play_sound", move |name: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::PlaySound {
                name: name.to_string(),
                volume: 1.0,
            });
        });
    }

    // play_sound(name: &str, volume: f64) — 2-arg overload with volume
    {
        let ctx = ctx.clone();
        engine.register_fn("play_sound", move |name: &str, volume: f64| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::PlaySound {
                name: name.to_string(),
                volume,
            });
        });
    }

    // play_sound_at(name: &str, x: f64, y: f64, z: f64, vol: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("play_sound_at", move |name: &str, x: f64, y: f64, z: f64, vol: f64| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::PlaySoundAt {
                name: name.to_string(),
                position: (x, y, z),
                volume: vol,
            });
        });
    }

    // stop_sound(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_sound", move |name: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::StopSound {
                name: name.to_string(),
            });
        });
    }
}

// ─── Animation API (direct ECS writes) ──────────────────

fn register_animation_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // play_clip(entity_id: i64, clip_name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("play_clip", move |entity_id: i64, clip_name: &str| {
            if entity_id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("animator", "clip", toml::Value::String(clip_name.to_string()));
                comps.set_field("animator", "playing", toml::Value::Boolean(true));
            }
        });
    }

    // stop_clip(entity_id: i64)
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_clip", move |entity_id: i64| {
            if entity_id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("animator", "playing", toml::Value::Boolean(false));
            }
        });
    }

    // blend_to(entity_id: i64, clip: &str, duration: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("blend_to", move |entity_id: i64, clip: &str, duration: f64| {
            if entity_id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("animator", "blend_target", toml::Value::String(clip.to_string()));
                comps.set_field("animator", "blend_duration", toml::Value::Float(duration));
            }
        });
    }

    // set_anim_speed(entity_id: i64, speed: f64)
    {
        let ctx = ctx.clone();
        engine.register_fn("set_anim_speed", move |entity_id: i64, speed: f64| {
            if entity_id < 0 { return; }
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_mut() };
            let eid = EntityId::from_raw(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("animator", "speed", toml::Value::Float(speed));
            }
        });
    }
}

// ─── Physics / Raycast API ───────────────────────────────

fn register_physics_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // raycast(ox, oy, oz, dx, dy, dz, max_dist) -> Map or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("raycast", move |ox: f64, oy: f64, oz: f64, dx: f64, dy: f64, dz: f64, max_dist: f64| -> Dynamic {
            let c = ctx.lock().unwrap();
            let physics = unsafe { c.physics_ref() };
            let physics = match physics {
                Some(p) => p,
                None => return Dynamic::UNIT,
            };

            // Exclude the current entity from the raycast
            let exclude = Some(c.current_entity);

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
    }

    // get_camera_direction() -> Map #{x, y, z}
    {
        let ctx = ctx.clone();
        engine.register_fn("get_camera_direction", move || -> Map {
            let c = ctx.lock().unwrap();
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.camera_direction[0] as f64));
            map.insert("y".into(), Dynamic::from(c.camera_direction[1] as f64));
            map.insert("z".into(), Dynamic::from(c.camera_direction[2] as f64));
            map
        });
    }

    // get_camera_position() -> Map #{x, y, z}
    {
        let ctx = ctx.clone();
        engine.register_fn("get_camera_position", move || -> Map {
            let c = ctx.lock().unwrap();
            let mut map = Map::new();
            map.insert("x".into(), Dynamic::from(c.camera_position[0] as f64));
            map.insert("y".into(), Dynamic::from(c.camera_position[1] as f64));
            map.insert("z".into(), Dynamic::from(c.camera_position[2] as f64));
            map
        });
    }

    // set_camera_position(x, y, z) — override camera position from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_camera_position", move |x: f64, y: f64, z: f64| {
            let mut c = ctx.lock().unwrap();
            c.camera_position_override = Some([x as f32, y as f32, z as f32]);
        });
    }

    // set_camera_target(x, y, z) — override camera look-at target from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_camera_target", move |x: f64, y: f64, z: f64| {
            let mut c = ctx.lock().unwrap();
            c.camera_target_override = Some([x as f32, y as f32, z as f32]);
        });
    }

    // set_camera_fov(fov) — override camera field of view from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_camera_fov", move |fov: f64| {
            let mut c = ctx.lock().unwrap();
            c.camera_fov_override = Some(fov as f32);
        });
    }

    // set_vignette(intensity) — override vignette intensity from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_vignette", move |intensity: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_vignette_override = Some(intensity as f32);
        });
    }

    // set_bloom_intensity(intensity) — override bloom intensity from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_bloom_intensity", move |intensity: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_bloom_override = Some(intensity as f32);
        });
    }

    // set_exposure(value) — override exposure from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_exposure", move |value: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_exposure_override = Some(value as f32);
        });
    }

    // set_chromatic_aberration(intensity) — override chromatic aberration from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_chromatic_aberration", move |intensity: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_chromatic_aberration_override = Some(intensity as f32);
        });
    }

    // set_radial_blur(intensity) — override radial blur from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_radial_blur", move |intensity: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_radial_blur_override = Some(intensity as f32);
        });
    }

    // set_ssao_intensity(intensity) — override SSAO intensity from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_ssao_intensity", move |intensity: f64| {
            let mut c = ctx.lock().unwrap();
            c.postprocess_ssao_intensity_override = Some(intensity as f32);
        });
    }

    // set_audio_lowpass(cutoff_hz) — override audio low-pass filter cutoff from scripts
    {
        let ctx = ctx.clone();
        engine.register_fn("set_audio_lowpass", move |cutoff_hz: f64| {
            let mut c = ctx.lock().unwrap();
            c.audio_lowpass_cutoff_override = Some(cutoff_hz as f32);
        });
    }
}

// ─── Math API ────────────────────────────────────────────

fn register_math_api(engine: &mut Engine) {
    engine.register_fn("clamp", |val: f64, min: f64, max: f64| -> f64 {
        val.clamp(min, max)
    });

    engine.register_fn("lerp", |a: f64, b: f64, t: f64| -> f64 {
        a + (b - a) * t
    });

    engine.register_fn("random", || -> f64 {
        // Simple pseudo-random based on time — no external crate needed
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        // Simple hash to get a 0..1 value
        let hash = t.wrapping_mul(2654435761);
        (hash as f64) / (u32::MAX as f64)
    });

    engine.register_fn("random_range", |min: f64, max: f64| -> f64 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let hash = t.wrapping_mul(2654435761);
        let r = (hash as f64) / (u32::MAX as f64);
        min + (max - min) * r
    });

    engine.register_fn("sin", |x: f64| -> f64 { x.sin() });
    engine.register_fn("cos", |x: f64| -> f64 { x.cos() });
    engine.register_fn("abs", |x: f64| -> f64 { x.abs() });
    engine.register_fn("sqrt", |x: f64| -> f64 { x.sqrt() });
    engine.register_fn("floor", |x: f64| -> f64 { x.floor() });
    engine.register_fn("ceil", |x: f64| -> f64 { x.ceil() });
    engine.register_fn("min", |a: f64, b: f64| -> f64 { a.min(b) });
    engine.register_fn("max", |a: f64, b: f64| -> f64 { a.max(b) });
    engine.register_fn("atan2", |y: f64, x: f64| -> f64 { y.atan2(x) });

    // Constants
    engine.register_fn("PI", || -> f64 { std::f64::consts::PI });
    engine.register_fn("TAU", || -> f64 { std::f64::consts::TAU });

    // Angle conversion
    engine.register_fn("deg_to_rad", |degrees: f64| -> f64 {
        degrees * std::f64::consts::PI / 180.0
    });
    engine.register_fn("rad_to_deg", |radians: f64| -> f64 {
        radians * 180.0 / std::f64::consts::PI
    });

    // Direction helpers — encode the Y-up, right-handed coordinate system
    // FORWARD = (0, 0, -1), RIGHT = (1, 0, 0), rotated by yaw around Y
    engine.register_fn("forward_from_yaw", |yaw_degrees: f64| -> Map {
        let yaw_rad = yaw_degrees * std::f64::consts::PI / 180.0;
        let mut map = Map::new();
        map.insert("x".into(), Dynamic::from(-yaw_rad.sin()));
        map.insert("y".into(), Dynamic::from(0.0_f64));
        map.insert("z".into(), Dynamic::from(-yaw_rad.cos()));
        map
    });
    engine.register_fn("right_from_yaw", |yaw_degrees: f64| -> Map {
        let yaw_rad = yaw_degrees * std::f64::consts::PI / 180.0;
        let mut map = Map::new();
        map.insert("x".into(), Dynamic::from(yaw_rad.cos()));
        map.insert("y".into(), Dynamic::from(0.0_f64));
        map.insert("z".into(), Dynamic::from(-yaw_rad.sin()));
        map
    });

    // Angle wrapping — normalizes degrees to [0, 360)
    engine.register_fn("wrap_angle", |degrees: f64| -> f64 {
        ((degrees % 360.0) + 360.0) % 360.0
    });
}

// ─── Event + Log API ─────────────────────────────────────

fn register_event_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // fire_event(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("fire_event", move |name: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::FireEvent {
                name: name.to_string(),
                data: toml::Value::Table(toml::map::Map::new()),
            });
        });
    }

    // fire_event_data(name: &str, data: Map)
    {
        let ctx = ctx.clone();
        engine.register_fn("fire_event_data", move |name: &str, data: Map| {
            let mut c = ctx.lock().unwrap();
            let toml_data = map_to_toml(&data);
            c.commands.push(ScriptCommand::FireEvent {
                name: name.to_string(),
                data: toml_data,
            });
        });
    }
}

// ─── Particle API ─────────────────────────────────────────

fn register_particle_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // emit_burst(entity_id, count) — fire N particles immediately
    {
        let ctx = ctx.clone();
        engine.register_fn("emit_burst", move |entity_id: i64, count: i64| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::EmitBurst {
                entity_id,
                count,
            });
        });
    }

    // start_emitter(entity_id) — set playing = true in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("start_emitter", move |entity_id: i64| {
            let mut c = ctx.lock().unwrap();
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("particle_emitter", "playing", toml::Value::Boolean(true));
            }
        });
    }

    // stop_emitter(entity_id) — set playing = false in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("stop_emitter", move |entity_id: i64| {
            let mut c = ctx.lock().unwrap();
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field("particle_emitter", "playing", toml::Value::Boolean(false));
            }
        });
    }

    // set_emission_rate(entity_id, rate) — update emission rate in ECS
    {
        let ctx = ctx.clone();
        engine.register_fn("set_emission_rate", move |entity_id: i64, rate: f64| {
            let mut c = ctx.lock().unwrap();
            let world = unsafe { &mut *c.world };
            let eid = EntityId(entity_id as u64);
            if let Some(comps) = world.get_components_mut(eid) {
                comps.set_field(
                    "particle_emitter",
                    "emission_rate",
                    toml::Value::Float(rate),
                );
            }
        });
    }
}

fn register_log_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // log(msg: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("log", move |msg: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Info,
                message: msg.to_string(),
            });
        });
    }

    // log_info(msg: &str) — alias for log()
    {
        let ctx = ctx.clone();
        engine.register_fn("log_info", move |msg: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Info,
                message: msg.to_string(),
            });
        });
    }

    // log_warn(msg: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("log_warn", move |msg: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Warn,
                message: msg.to_string(),
            });
        });
    }

    // log_error(msg: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("log_error", move |msg: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Error,
                message: msg.to_string(),
            });
        });
    }
}

// ─── UI Draw API ─────────────────────────────────────────

fn register_ui_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // draw_text(x, y, text, size, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_text", move |x: f64, y: f64, text: &str, size: f64, r: f64, g: f64, b: f64, a: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Text {
                x: x as f32, y: y as f32,
                text: text.to_string(),
                size: size as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                layer: 0,
                align: 0,
                stroke: None,
            });
        });
    }

    // draw_text_ex(x, y, text, size, r, g, b, a, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_text_ex", move |x: f64, y: f64, text: &str, size: f64, r: f64, g: f64, b: f64, a: f64, layer: i64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Text {
                x: x as f32, y: y as f32,
                text: text.to_string(),
                size: size as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                layer: layer as i32,
                align: 0,
                stroke: None,
            });
        });
    }

    // draw_text_stroked(x, y, text, size, r, g, b, a, stroke_r, stroke_g, stroke_b, stroke_a, stroke_width)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_text_stroked", move |x: f64, y: f64, text: &str, size: f64,
            r: f64, g: f64, b: f64, a: f64,
            sr: f64, sg: f64, sb: f64, sa: f64, sw: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Text {
                x: x as f32, y: y as f32,
                text: text.to_string(),
                size: size as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                layer: 0,
                align: 0,
                stroke: Some(([sr as f32, sg as f32, sb as f32, sa as f32], sw as f32)),
            });
        });
    }

    // draw_rect(x, y, w, h, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_rect", move |x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64, a: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::RectFilled {
                x: x as f32, y: y as f32, w: w as f32, h: h as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                rounding: 0.0,
                layer: 0,
            });
        });
    }

    // draw_rect_ex(x, y, w, h, r, g, b, a, rounding, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_rect_ex", move |x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64, a: f64, rounding: f64, layer: i64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::RectFilled {
                x: x as f32, y: y as f32, w: w as f32, h: h as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                rounding: rounding as f32,
                layer: layer as i32,
            });
        });
    }

    // draw_rect_outline(x, y, w, h, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_rect_outline", move |x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64, a: f64, thickness: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::RectOutline {
                x: x as f32, y: y as f32, w: w as f32, h: h as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                thickness: thickness as f32,
                layer: 0,
            });
        });
    }

    // draw_circle(x, y, radius, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_circle", move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::CircleFilled {
                x: x as f32, y: y as f32,
                radius: radius as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                layer: 0,
            });
        });
    }

    // draw_circle_outline(x, y, radius, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_circle_outline", move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64, thickness: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::CircleOutline {
                x: x as f32, y: y as f32,
                radius: radius as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                thickness: thickness as f32,
                layer: 0,
            });
        });
    }

    // draw_line(x1, y1, x2, y2, r, g, b, a, thickness)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_line", move |x1: f64, y1: f64, x2: f64, y2: f64, r: f64, g: f64, b: f64, a: f64, thickness: f64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Line {
                x1: x1 as f32, y1: y1 as f32, x2: x2 as f32, y2: y2 as f32,
                color: [r as f32, g as f32, b as f32, a as f32],
                thickness: thickness as f32,
                layer: 0,
            });
        });
    }

    // draw_sprite(x, y, w, h, name)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_sprite", move |x: f64, y: f64, w: f64, h: f64, name: &str| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Sprite {
                x: x as f32, y: y as f32, w: w as f32, h: h as f32,
                name: name.to_string(),
                uv: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
                layer: 0,
            });
        });
    }

    // draw_sprite_ex(x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)
    {
        let ctx = ctx.clone();
        engine.register_fn("draw_sprite_ex", move |x: f64, y: f64, w: f64, h: f64, name: &str, u0: f64, v0: f64, u1: f64, v1: f64, r: f64, g: f64, b: f64, a: f64, layer: i64| {
            let mut c = ctx.lock().unwrap();
            c.draw_commands.push(DrawCommand::Sprite {
                x: x as f32, y: y as f32, w: w as f32, h: h as f32,
                name: name.to_string(),
                uv: [u0 as f32, v0 as f32, u1 as f32, v1 as f32],
                tint: [r as f32, g as f32, b as f32, a as f32],
                layer: layer as i32,
            });
        });
    }

    // screen_width() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_width", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.screen_width as f64
        });
    }

    // screen_height() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("screen_height", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.screen_height as f64
        });
    }

    // find_nearest_interactable() -> Map or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("find_nearest_interactable", move || -> Dynamic {
            let c = ctx.lock().unwrap();
            let world = unsafe { c.world_ref() };
            match crate::engine::find_nearest_interactable(world) {
                Some(nearest) => {
                    let mut map = Map::new();
                    map.insert("entity".into(), Dynamic::from(nearest.entity_id.raw() as i64));
                    map.insert("prompt_text".into(), Dynamic::from(nearest.prompt_text));
                    map.insert("interaction_type".into(), Dynamic::from(nearest.interaction_type));
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
            drop(ctx.lock().unwrap());
            // Approximate text width: average character width ~0.6 * font_size
            let char_width = size * 0.6;
            let width = text.len() as f64 * char_width;
            let mut map = Map::new();
            map.insert("width".into(), Dynamic::from(width));
            map.insert("height".into(), Dynamic::from(size));
            map
        });
    }

    // move_character(entity_id, dx, dy, dz) -> Map #{x, y, z, grounded} or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("move_character", move |entity_id: i64, dx: f64, dy: f64, dz: f64| -> Dynamic {
            if entity_id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
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
            match physics.move_character_shape(eid, current_pos, [dx as f32, dy as f32, dz as f32], dt) {
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
        });
    }

    // get_collider_extents(entity_id) -> Map or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("get_collider_extents", move |entity_id: i64| -> Dynamic {
            if entity_id < 0 { return Dynamic::UNIT; }
            let c = ctx.lock().unwrap();
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
                Some(flint_physics::ColliderExtents::Capsule { radius, half_height }) => {
                    let mut map = Map::new();
                    map.insert("shape".into(), Dynamic::from("capsule".to_string()));
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
            let mut c = ctx.lock().unwrap();
            // Resolve relative to project root (scene's parent's parent)
            let scene_dir = if c.current_scene_path.is_empty() {
                std::path::PathBuf::from(".")
            } else {
                let p = std::path::Path::new(&c.current_scene_path);
                // scenes/oval_plus.scene.toml → parent "scenes" → parent "" → "."
                let root = p.parent()
                    .and_then(|dir| dir.parent())
                    .unwrap_or(std::path::Path::new("."));
                if root.as_os_str().is_empty() {
                    std::path::PathBuf::from(".")
                } else {
                    root.to_path_buf()
                }
            };
            c.ui_system.load(layout_path, &scene_dir)
        });
    }

    // unload_ui(handle)
    {
        let ctx = ctx.clone();
        engine.register_fn("unload_ui", move |handle: i64| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.unload(handle);
        });
    }

    // ui_set_text(element_id, text)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_text", move |element_id: &str, text: &str| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.set_text(element_id, text);
        });
    }

    // ui_show(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_show", move |element_id: &str| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.show(element_id);
        });
    }

    // ui_hide(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_hide", move |element_id: &str| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.hide(element_id);
        });
    }

    // ui_set_visible(element_id, visible)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_visible", move |element_id: &str, visible: bool| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.set_visible(element_id, visible);
        });
    }

    // ui_set_color(element_id, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_color", move |element_id: &str, r: f64, g: f64, b: f64, a: f64| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.set_color(element_id, r as f32, g as f32, b as f32, a as f32);
        });
    }

    // ui_set_bg_color(element_id, r, g, b, a)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_bg_color", move |element_id: &str, r: f64, g: f64, b: f64, a: f64| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.set_bg_color(element_id, r as f32, g as f32, b as f32, a as f32);
        });
    }

    // ui_set_style(element_id, prop, val)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_style", move |element_id: &str, prop: &str, val: Dynamic| {
            let mut c = ctx.lock().unwrap();
            let style_val = if val.is_float() {
                StyleValue::Float(val.as_float().unwrap_or(0.0) as f32)
            } else if val.is_int() {
                StyleValue::Float(val.as_int().unwrap_or(0) as f32)
            } else if val.is_string() {
                StyleValue::String(val.into_string().unwrap_or_default())
            } else if val.is_bool() {
                StyleValue::Bool(val.as_bool().unwrap_or(false))
            } else {
                return;
            };
            c.ui_system.set_style(element_id, prop, style_val);
        });
    }

    // ui_reset_style(element_id)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_reset_style", move |element_id: &str| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.reset_style(element_id);
        });
    }

    // ui_set_class(element_id, class)
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_set_class", move |element_id: &str, class: &str| {
            let mut c = ctx.lock().unwrap();
            c.ui_system.set_class(element_id, class);
        });
    }

    // ui_exists(element_id) -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_exists", move |element_id: &str| -> bool {
            let c = ctx.lock().unwrap();
            c.ui_system.exists(element_id)
        });
    }

    // ui_get_rect(element_id) -> Map #{x, y, w, h} or ()
    {
        let ctx = ctx.clone();
        engine.register_fn("ui_get_rect", move |element_id: &str| -> Dynamic {
            let mut c = ctx.lock().unwrap();
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

// ─── State Machine API ────────────────────────────────────

fn register_state_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // push_state(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("push_state", move |name: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::PushState {
                name: name.to_string(),
            });
        });
    }

    // pop_state()
    {
        let ctx = ctx.clone();
        engine.register_fn("pop_state", move || {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::PopState);
        });
    }

    // replace_state(name: &str)
    {
        let ctx = ctx.clone();
        engine.register_fn("replace_state", move |name: &str| {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::ReplaceState {
                name: name.to_string(),
            });
        });
    }

    // current_state() -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("current_state", move || -> String {
            let c = ctx.lock().unwrap();
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
            let c = ctx.lock().unwrap();
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
            let mut c = ctx.lock().unwrap();
            if c.state_machine.is_null() {
                return;
            }
            let sm = unsafe { &mut *c.state_machine };

            let policy = |key: &str| -> flint_runtime::SystemPolicy {
                match config.get(key).and_then(|v| v.clone().into_string().ok()).as_deref() {
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
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::LoadScene {
                path: path.to_string(),
            });
        });
    }

    // reload_scene()
    {
        let ctx = ctx.clone();
        engine.register_fn("reload_scene", move || {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::ReloadScene);
        });
    }

    // current_scene() -> String
    {
        let ctx = ctx.clone();
        engine.register_fn("current_scene", move || -> String {
            let c = ctx.lock().unwrap();
            c.current_scene_path.clone()
        });
    }

    // complete_transition()
    {
        let ctx = ctx.clone();
        engine.register_fn("complete_transition", move || {
            let mut c = ctx.lock().unwrap();
            c.commands.push(ScriptCommand::FireEvent {
                name: "__transition_complete".to_string(),
                data: toml::Value::Table(toml::map::Map::new()),
            });
        });
    }

    // transition_progress() -> f64
    {
        let ctx = ctx.clone();
        engine.register_fn("transition_progress", move || -> f64 {
            let c = ctx.lock().unwrap();
            c.transition_progress
        });
    }

    // is_transitioning() -> bool
    {
        let ctx = ctx.clone();
        engine.register_fn("is_transitioning", move || -> bool {
            let c = ctx.lock().unwrap();
            c.transition_progress >= 0.0
        });
    }

    // transition_phase() -> String ("idle", "exiting", "entering")
    {
        let ctx = ctx.clone();
        engine.register_fn("transition_phase", move || -> String {
            let c = ctx.lock().unwrap();
            c.transition_phase.clone()
        });
    }
}

// ─── Persistence API ──────────────────────────────────────

fn register_persistence_api(engine: &mut Engine, ctx: Arc<Mutex<ScriptCallContext>>) {
    // persist_set(key, value)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_set", move |key: &str, val: Dynamic| {
            let mut c = ctx.lock().unwrap();
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
            let c = ctx.lock().unwrap();
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
            let c = ctx.lock().unwrap();
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
            let mut c = ctx.lock().unwrap();
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
            let mut c = ctx.lock().unwrap();
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
            let c = ctx.lock().unwrap();
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

    // persist_save(path)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_save", move |path: &str| {
            let c = ctx.lock().unwrap();
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &*c.persistent_store };
            if let Err(e) = store.save_to_file(std::path::Path::new(path)) {
                eprintln!("[persist] save error: {}", e);
            }
        });
    }

    // persist_load(path)
    {
        let ctx = ctx.clone();
        engine.register_fn("persist_load", move |path: &str| {
            let mut c = ctx.lock().unwrap();
            if c.persistent_store.is_null() {
                return;
            }
            let store = unsafe { &mut *c.persistent_store };
            if let Err(e) = store.load_from_file(std::path::Path::new(path)) {
                eprintln!("[persist] load error: {}", e);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let d = toml_to_dynamic(toml::Value::Float(3.14));
        assert!((d.as_float().unwrap() - 3.14).abs() < 1e-10);
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
        let original = toml::Value::Float(2.718);
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

    #[test]
    fn test_math_clamp() {
        let mut engine = Engine::new();
        register_math_api(&mut engine);
        let result: f64 = engine.eval("clamp(5.0, 0.0, 3.0)").unwrap();
        assert!((result - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_math_lerp() {
        let mut engine = Engine::new();
        register_math_api(&mut engine);
        let result: f64 = engine.eval("lerp(0.0, 10.0, 0.5)").unwrap();
        assert!((result - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_math_trig() {
        let mut engine = Engine::new();
        register_math_api(&mut engine);
        let result: f64 = engine.eval("sin(0.0)").unwrap();
        assert!(result.abs() < 1e-10);
        let result: f64 = engine.eval("cos(0.0)").unwrap();
        assert!((result - 1.0).abs() < 1e-10);
    }
}
