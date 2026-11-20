//! Fields a scene declares (#1014, ADR 0123 decision 4): `[field.<name>]`
//! tables, each a few named numbers by place with zones over them
//! (`flint_core::field::SceneFields`), read and changed by scripts through
//! the `field` module: `field_sample("weather", "wind", pos)`, `field_at`,
//! `field_set`, `field_set_zone` and `field_remove_zone`.
//!
//! Nothing but scripts reads them yet, so the fields live in the script
//! system's module state, where the module's functions read and change them
//! in place: a change is seen by the next call, in the same frame, and every
//! ramp runs on the game clock (`total_time`), which is what a script's
//! `on_update` steps on. The subsystem only puts them there when the scene
//! comes up, enables the module and takes them away with the scene.
//!
//! The `field_` names are one family because `set_field` and `get_field`
//! are a component's fields (`set_field(id, "health", "hp", 3)`).

use super::{Lasts, SceneCtx, Subsystem};
use flint_core::field::{SceneField, SceneFields};
use flint_script::api::builder::kind::{Bool, Map, Num, Str};
use flint_script::api::{map_to_toml, Call, CallResult, Module};
use flint_script::ScriptModule;

/// No game state pauses it: a pause stops the scripts that change it, and
/// its clock is the game's.
pub(crate) const POLICY: Option<&str> = None;

/// Its script module.
pub(crate) const SCRIPT_MODULES: &[&dyn ScriptModule] = &[&FIELD];

/// It has no debug keys.
pub(crate) const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "field",
    when: "a scene with a [field.<name>] table",
    keys: &[],
};

/// The subsystem for a scene with a `[field]` table: what it declares goes
/// into the script system for the `field` module, and what's wrong with it
/// is said once, here, with the rest still used.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let fields = read(ctx.tables, ctx.scene_path)?;
    ctx.script.set_module_state(ScriptFields { fields });
    Some(Box::new(FieldSubsystem))
}

/// The fields `tables` declares, its mistakes warned naming `scene`; `None`
/// with no `[field]` table.
pub(crate) fn read(tables: &toml::Table, scene: &str) -> Option<SceneFields> {
    let (fields, issues) = SceneFields::of_tables(tables)?;
    for issue in issues {
        tracing::warn!(target: "load", "{scene}: {issue}");
    }
    Some(fields)
}

/// A capture's fields (#1014): `flint render --scripts` reads the scene's
/// `[field]` table from its file as the player does, so a script samples the
/// same values there. The subsystem is the player's, for the capture's list,
/// which is what enables the module.
pub(crate) fn for_capture(
    script: &mut flint_script::ScriptSystem,
    scene_path: &str,
) -> Option<Box<dyn Subsystem>> {
    let text = flint_core::vfs::read_to_string(scene_path).ok()?;
    let tables = toml::from_str::<toml::Table>(&text).ok()?;
    let fields = read(&tables, scene_path)?;
    script.set_module_state(ScriptFields { fields });
    Some(Box::new(FieldSubsystem))
}

/// The scene's fields, as the `field` module's functions see them.
#[derive(Debug, Default)]
pub(crate) struct ScriptFields {
    pub(crate) fields: SceneFields,
}

struct FieldSubsystem;

impl Subsystem for FieldSubsystem {
    fn name(&self) -> &'static str {
        "field"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["field"]
    }

    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        ctx.script.set_module_state(ScriptFields::default());
    }
}

/// The `field` module.
pub(crate) struct FieldModule;

pub(crate) static FIELD: FieldModule = FieldModule;

/// The field called `name`, or the script error naming the ones there are.
fn field<'a>(call: &'a mut Call<'_>, name: &str) -> CallResult<&'a mut SceneField> {
    let error = |names: Vec<&str>| {
        let there = if names.is_empty() {
            "the scene declares none".to_string()
        } else {
            format!("the scene declares {}", names.join(", "))
        };
        format!("there is no field '{name}'; {there}")
    };
    let func = call.name();
    let position = call.position();
    let fields = &mut call.ctx().module_state.get_mut::<ScriptFields>().fields;
    if fields.get(name).is_none() {
        let message = error(fields.names());
        return Err(Box::new(rhai::EvalAltResult::ErrorRuntime(
            format!("{func}: {message}").into(),
            position,
        )));
    }
    Ok(fields.get_mut(name).expect("found above"))
}

/// A place from `#{x, y, z}` (what `get_position` returns), each a number.
fn place(call: &Call<'_>, pos: &rhai::Map) -> CallResult<[f32; 3]> {
    let axis = |name: &str| {
        let value = pos.get(name)?;
        value
            .as_float()
            .ok()
            .or_else(|| value.as_int().ok().map(|i| i as f64))
    };
    match (axis("x"), axis("y"), axis("z")) {
        (Some(x), Some(y), Some(z)) => Ok([x as f32, y as f32, z as f32]),
        _ => Err(call.error("the place must be #{x, y, z}, each a number")),
    }
}

/// An `f32` as the shortest decimal that is the same `f32`, so a value
/// written 0.4 reads 0.4, not 0.4000000059604645 (as `wooze_at` does).
fn num(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}

/// Key `key` of field `name` at `at`, now.
fn sample(call: &mut Call<'_>, name: &str, key: &str, at: [f32; 3]) -> CallResult<f64> {
    let time = call.ctx().total_time;
    let field = field(call, name)?;
    let Some(i) = field.index(key) else {
        let keys: Vec<&str> = field.keys().iter().map(|k| k.name.as_str()).collect();
        let message = format!(
            "'{key}' isn't a {} key; it takes {}",
            field.name(),
            keys.join(", ")
        );
        return Err(call.error(message));
    };
    Ok(num(field.sample_index(i, at, time)))
}

/// A table from a script's map, or the error saying it must be one.
fn table_of(call: &Call<'_>, map: &rhai::Map) -> CallResult<toml::Table> {
    match map_to_toml(map) {
        toml::Value::Table(table) => Ok(table),
        _ => Err(call.error("expected a map")),
    }
}

fn set(call: &mut Call<'_>, name: &str, values: &rhai::Map, seconds: f64) -> CallResult<()> {
    let table = table_of(call, values)?;
    let time = call.ctx().total_time;
    let result = field(call, name)?.set_global(&table, time, seconds.max(0.0));
    result.map_err(|why| call.error(why))
}

fn set_zone(
    call: &mut Call<'_>,
    name: &str,
    zone: &str,
    fields: &rhai::Map,
    seconds: f64,
) -> CallResult<()> {
    let table = table_of(call, fields)?;
    let time = call.ctx().total_time;
    let result = field(call, name)?.set_zone(zone, &table, time, seconds.max(0.0));
    result.map_err(|why| call.error(format!("zone '{zone}': {why}")))
}

fn remove_zone(call: &mut Call<'_>, name: &str, zone: &str, seconds: f64) -> CallResult<bool> {
    let time = call.ctx().total_time;
    Ok(field(call, name)?.remove_zone(zone, time, seconds.max(0.0)))
}

impl ScriptModule for FieldModule {
    fn name(&self) -> &'static str {
        "field"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene declares a field: a [field.<name>] table with its keys"
    }

    /// The writes take an optional last argument of seconds to get there
    /// over (an int or a float); without it the change is immediate. A
    /// write is seen by the next read, in the same frame.
    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "field_sample",
            "One key of a scene's field at a place, now: its zones and scripts' changes included.",
        )
        .arg("field", Str)
        .arg("key", Str)
        .arg("pos", Map)
        .returns(Num, "the key's value there")
        .run(|call, (name, key, pos)| {
            let at = place(call, &pos)?;
            sample(call, &name, &key, at)
        });

        m.func(
            "field_sample",
            "One key of a scene's field at x, y, z, now.",
        )
        .arg("field", Str)
        .arg("key", Str)
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num)
        .returns(Num, "the key's value there")
        .run(|call, (name, key, x, y, z)| {
            sample(call, &name, &key, [x as f32, y as f32, z as f32])
        });

        m.func(
            "field_at",
            "Every key of a scene's field at a place, now, and the zones that reach it.",
        )
        .arg("field", Str)
        .arg("pos", Map)
        .returns(Map, "#{<key>: value, ..., zones: #{name: share}}")
        .run(|call, (name, pos)| {
            let at = place(call, &pos)?;
            let time = call.ctx().total_time;
            let field = field(call, &name)?;
            let mut map = rhai::Map::new();
            for (i, key) in field.keys().iter().enumerate() {
                let value = num(field.sample_index(i, at, time));
                map.insert(key.name.as_str().into(), value.into());
            }
            let mut zones = rhai::Map::new();
            for (zone, share) in field.zones_in(at, time) {
                zones.insert(zone.into(), num(share).into());
            }
            map.insert("zones".into(), zones.into());
            Ok(map)
        });

        m.func(
            "field_set",
            "Change a field's global values at once: any of its keys, each held to its range.",
        )
        .arg("field", Str)
        .arg("values", Map)
        .run(|call, (name, values)| set(call, &name, &values, 0.0));

        m.func(
            "field_set",
            "Change a field's global values over some seconds.",
        )
        .arg("field", Str)
        .arg("values", Map)
        .arg("seconds", Num)
        .run(|call, (name, values, seconds)| set(call, &name, &values, seconds));

        m.func(
            "field_set_zone",
            "Change a field's named zone at once, keeping what fields leaves out, or add it (shape, center, radius or size, falloff, a key).",
        )
        .arg("field", Str)
        .arg("zone", Str)
        .arg("fields", Map)
        .run(|call, (name, zone, fields)| set_zone(call, &name, &zone, &fields, 0.0));

        m.func(
            "field_set_zone",
            "Change or add a field's named zone, its values easing and a new zone fading in over some seconds.",
        )
        .arg("field", Str)
        .arg("zone", Str)
        .arg("fields", Map)
        .arg("seconds", Num)
        .run(|call, (name, zone, fields, seconds)| {
            set_zone(call, &name, &zone, &fields, seconds)
        });

        m.func("field_remove_zone", "Drop a field's named zone at once.")
            .arg("field", Str)
            .arg("zone", Str)
            .returns(Bool, "whether there was one")
            .run(|call, (name, zone)| remove_zone(call, &name, &zone, 0.0));

        m.func(
            "field_remove_zone",
            "Fade a field's named zone out over some seconds, then drop it.",
        )
        .arg("field", Str)
        .arg("zone", Str)
        .arg("seconds", Num)
        .returns(Bool, "whether there was one")
        .run(|call, (name, zone, seconds)| remove_zone(call, &name, &zone, seconds));
    }
}

#[cfg(test)]
#[path = "../field_tests.rs"]
mod tests;
