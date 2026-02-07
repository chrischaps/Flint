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
    register_input_api(engine, ctx.clone());
    register_time_api(engine, ctx.clone());
    register_audio_api(engine, ctx.clone());
    register_animation_api(engine, ctx.clone());
    register_physics_api(engine, ctx.clone());
    register_math_api(engine);
    register_event_api(engine, ctx.clone());
    register_ui_api(engine, ctx.clone());
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
