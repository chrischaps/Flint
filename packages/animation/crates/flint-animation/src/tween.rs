//! Tweens of any numeric component field (#734): a script's
//! `tween(entity, "component.field", target, seconds[, easing])`.
//!
//! A light's intensity, a material's colour, a camera's field of view, a
//! transform's position: anything a component holds as a number or a list of
//! numbers eases on the engine's one [`Tween`], stepped on game time by the
//! host (the player's game core, for the player and a headless capture alike),
//! and fires `tween_done` with `#{ entity, path }` when it arrives.
//!
//! The rules are the style tweens' (#370):
//!
//! - A tween starts where the field stands and begins to move the frame after
//!   it was asked for; it lands exactly on its target.
//! - A new tween of the same field replaces the old one from wherever it had
//!   got to, and no time to ease over sets the field at once.
//! - **A write by anything else takes over**: a field that no longer holds
//!   what the tween last wrote (a script's `set_field`, physics moving a body,
//!   an editor) ends the tween quietly, with no `tween_done`. So does the
//!   entity, or its component, going away.
//! - The field must already exist: a tween has nowhere to start from
//!   otherwise. An integer field moves as a float and arrives as an integer
//!   when its target is whole.

use flint_core::toml_util::toml_f64;
use flint_core::tween::{Easing, Tween};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;

/// The event a finished field tween fires to scripts' `on_event`, with
/// `#{ entity, path }`.
pub const TWEEN_DONE: &str = "tween_done";

/// A field a tween names: `component.field`, or deeper into a table the
/// component holds (`transform.shear.xz`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    component: String,
    fields: Vec<String>,
}

impl FieldPath {
    /// Read `path`, or say why it names no field.
    pub fn parse(path: &str) -> Result<Self, String> {
        let mut parts = path.split('.');
        let component = parts.next().unwrap_or_default();
        let fields: Vec<String> = parts.map(str::to_string).collect();
        if component.is_empty() || fields.is_empty() || fields.iter().any(String::is_empty) {
            return Err(format!(
                "`{path}` names no field; write it as \"component.field\" (\"light.intensity\")"
            ));
        }
        Ok(Self {
            component: component.to_string(),
            fields,
        })
    }

    /// The component's name.
    pub fn component(&self) -> &str {
        &self.component
    }

    /// As a script writes it.
    pub fn dotted(&self) -> String {
        format!("{}.{}", self.component, self.fields.join("."))
    }

    fn leaf<'w>(&self, world: &'w FlintWorld, id: EntityId) -> Option<&'w toml::Value> {
        let mut at = world.get_component(id, &self.component)?;
        for field in &self.fields {
            at = at.as_table()?.get(field)?;
        }
        Some(at)
    }

    fn leaf_mut<'w>(&self, world: &'w mut FlintWorld, id: EntityId) -> Option<&'w mut toml::Value> {
        let mut at = world.get_component_mut(id, &self.component)?;
        for field in &self.fields {
            at = at.as_table_mut()?.get_mut(field)?;
        }
        Some(at)
    }
}

/// What a field holds, as a tween eases it.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbers {
    values: Vec<f64>,
    /// One number, not a list of one.
    scalar: bool,
    /// Every one of them written as an integer.
    integers: bool,
}

impl Numbers {
    /// What `value` holds, if it is a number or a list of numbers.
    pub fn of(value: &toml::Value) -> Option<Self> {
        match value {
            toml::Value::Array(items) if !items.is_empty() => Some(Self {
                values: items.iter().map(toml_f64).collect::<Option<_>>()?,
                scalar: false,
                integers: items.iter().all(toml::Value::is_integer),
            }),
            toml::Value::Array(_) => None,
            other => Some(Self {
                values: vec![toml_f64(other)?],
                scalar: true,
                integers: other.is_integer(),
            }),
        }
    }

    /// How a script would write it: `5.0`, `[1.0, 0.5, 0.2]`.
    fn shape(&self) -> String {
        match self.scalar {
            true => "a number".into(),
            false => format!("a list of {} numbers", self.values.len()),
        }
    }

    /// Its value with `values` in place: floats while moving, and integers
    /// where the field was one and the number is whole once `arrived`.
    fn value(&self, values: &[f64], arrived: bool) -> toml::Value {
        let number = |v: f64| {
            if arrived && self.integers && v.fract() == 0.0 && v.abs() < 9.0e15 {
                toml::Value::Integer(v as i64)
            } else {
                toml::Value::Float(v)
            }
        };
        match self.scalar {
            true => number(values[0]),
            false => toml::Value::Array(values.iter().copied().map(number).collect()),
        }
    }
}

/// Where a tween of `path` on `id` would start, and the numbers it would head
/// for: the field as it stands and `target` checked against its shape. The
/// script call asks this to report a mistake at its line; the host asks
/// again when it starts the tween.
pub fn endpoints(
    world: &FlintWorld,
    id: EntityId,
    path: &FieldPath,
    target: &toml::Value,
) -> Result<(Numbers, Vec<f64>), String> {
    let dotted = path.dotted();
    if world.get_component(id, &path.component).is_none() {
        return Err(format!(
            "`{dotted}`: the entity has no `{}` component",
            path.component
        ));
    }
    let leaf = path.leaf(world, id).ok_or_else(|| {
        format!("`{dotted}`: the component has no such field to start from; set it first")
    })?;
    let from = Numbers::of(leaf).ok_or_else(|| {
        format!("`{dotted}` holds {leaf}, which isn't a number or a list of numbers")
    })?;
    let to = Numbers::of(target)
        .filter(|to| to.scalar == from.scalar && to.values.len() == from.values.len())
        .ok_or_else(|| {
            format!(
                "`{dotted}` holds {}, so it can't ease to {target}",
                from.shape()
            )
        })?;
    if let Some(bad) = to.values.iter().find(|v| !v.is_finite()) {
        return Err(format!("`{dotted}` can't ease to {bad}"));
    }
    Ok((from, to.values))
}

#[derive(Debug)]
struct FieldTween {
    entity: EntityId,
    path: FieldPath,
    /// The field's shape at the start, which every write keeps.
    shape: Numbers,
    tween: Tween<Vec<f64>>,
    /// What the field held after the tween's last write (or at its start):
    /// anything else there means something else wrote it.
    written: toml::Value,
}

/// Every field tween running. The host owns one, starts tweens from the
/// scripts' `tween` commands and steps it once a frame on game time while
/// scripts run.
#[derive(Debug, Default)]
pub struct FieldTweens {
    tweens: Vec<FieldTween>,
}

impl FieldTweens {
    /// How many are running.
    pub fn len(&self) -> usize {
        self.tweens.len()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.tweens.is_empty()
    }

    /// Whether `entity`'s field at `path` is easing.
    pub fn is_tweening(&self, entity: EntityId, path: &str) -> bool {
        FieldPath::parse(path).is_ok_and(|path| {
            self.tweens
                .iter()
                .any(|t| t.entity == entity && t.path == path)
        })
    }

    /// Drop every tween (a new scene).
    pub fn clear(&mut self) {
        self.tweens.clear();
    }

    /// `tween`: ease `entity`'s field at `path` to `target` over `seconds` of
    /// game time, moving as `easing` says, from where it stands. A tween of
    /// the same field already running is replaced; no time to ease over sets
    /// the field now and returns its `tween_done`.
    pub fn start(
        &mut self,
        world: &mut FlintWorld,
        entity: EntityId,
        path: &str,
        target: &toml::Value,
        seconds: f64,
        easing: Easing,
    ) -> Result<Option<GameEvent>, String> {
        let path = FieldPath::parse(path).map_err(|e| format!("tween: {e}"))?;
        let (from, to) =
            endpoints(world, entity, &path, target).map_err(|e| format!("tween: {e}"))?;
        self.tweens
            .retain(|t| !(t.entity == entity && t.path == path));
        let tween = Tween::new(from.values.clone(), to, seconds, easing);
        if tween.done() {
            let value = from.value(&tween.value(), true);
            if let Some(leaf) = path.leaf_mut(world, entity) {
                *leaf = value;
            }
            return Ok(Some(done(entity, &path)));
        }
        // As it stands (`endpoints` found it there)
        let written = match path.leaf(world, entity) {
            Some(now) => now.clone(),
            None => from.value(&from.values, false),
        };
        self.tweens.push(FieldTween {
            entity,
            path,
            shape: from,
            tween,
            written,
        });
        Ok(None)
    }

    /// Stop `entity`'s tweens: of the field at `path`, or all of them.
    pub fn stop(&mut self, entity: EntityId, path: Option<&str>) {
        let path = path.and_then(|p| FieldPath::parse(p).ok());
        self.tweens
            .retain(|t| !(t.entity == entity && path.as_ref().is_none_or(|p| t.path == *p)));
    }

    /// Move every tween on by `dt` seconds of game time and write its value;
    /// `tween_done` for each that arrived. One whose field something else
    /// wrote since, or that is gone, ends without one.
    pub fn step(&mut self, world: &mut FlintWorld, dt: f64) -> Vec<GameEvent> {
        let mut events = Vec::new();
        self.tweens.retain_mut(|t| {
            let Some(leaf) = t.path.leaf_mut(world, t.entity) else {
                return false;
            };
            if *leaf != t.written {
                // Something else wrote it: it has taken over
                return false;
            }
            let values = t.tween.step(dt);
            let arrived = t.tween.done();
            *leaf = t.shape.value(&values, arrived);
            t.written = leaf.clone();
            if arrived {
                events.push(done(t.entity, &t.path));
            }
            !arrived
        });
        events
    }
}

fn done(entity: EntityId, path: &FieldPath) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("entity".into(), toml::Value::Integer(entity.raw() as i64));
    data.insert("path".into(), path.dotted().into());
    GameEvent::Custom {
        name: TWEEN_DONE.into(),
        data: toml::Value::Table(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn("lamp").unwrap();
        let light: toml::Value = toml::toml! {
            intensity = 2
            color = [1.0, 0.5, 0.25]
            kind = "point"
        }
        .into();
        world.set_component(id, "light", light).unwrap();
        (world, id)
    }

    fn intensity(world: &FlintWorld, id: EntityId) -> toml::Value {
        world.get_component(id, "light").unwrap()["intensity"].clone()
    }

    #[test]
    fn a_path_is_a_component_and_a_field() {
        assert!(FieldPath::parse("light.intensity").is_ok());
        assert_eq!(
            FieldPath::parse("transform.shear.xz").unwrap().dotted(),
            "transform.shear.xz"
        );
        for bad in ["light", "", ".intensity", "light.", "light..x"] {
            let e = FieldPath::parse(bad).unwrap_err();
            assert!(e.contains("component.field"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_number_eases_on_game_time_and_lands_on_its_target() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let target = toml::Value::Integer(10);
        let started = tweens.start(
            &mut world,
            id,
            "light.intensity",
            &target,
            1.0,
            Easing::Linear,
        );
        assert!(matches!(started, Ok(None)), "{started:?}");
        assert_eq!(
            intensity(&world, id),
            toml::Value::Integer(2),
            "moves next frame"
        );
        assert!(tweens.is_tweening(id, "light.intensity"));
        assert!(tweens.step(&mut world, 0.5).is_empty());
        assert_eq!(intensity(&world, id), toml::Value::Float(6.0));
        let events = tweens.step(&mut world, 0.5);
        assert_eq!(
            intensity(&world, id),
            toml::Value::Integer(10),
            "arrives whole"
        );
        let [GameEvent::Custom { name, data }] = &events[..] else {
            panic!("{events:?}");
        };
        assert_eq!(name, TWEEN_DONE);
        assert_eq!(data["path"].as_str(), Some("light.intensity"));
        assert!(tweens.is_empty());
    }

    #[test]
    fn a_colour_eases_by_the_easing_asked_for() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let target: toml::Value = vec![0.0, 0.5, 1.25].into();
        tweens
            .start(&mut world, id, "light.color", &target, 2.0, Easing::EaseIn)
            .unwrap();
        tweens.step(&mut world, 1.0);
        let k = Easing::EaseIn.apply(0.5);
        let color = &world.get_component(id, "light").unwrap()["color"];
        let expected: toml::Value = vec![1.0 - k, 0.5, 0.25 + k].into();
        assert_eq!(*color, expected);
    }

    #[test]
    fn a_write_by_anything_else_takes_over() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let target = toml::Value::Float(10.0);
        tweens
            .start(
                &mut world,
                id,
                "light.intensity",
                &target,
                1.0,
                Easing::Smooth,
            )
            .unwrap();
        tweens.step(&mut world, 0.25);
        world
            .set_field(id, "light", "intensity", toml::Value::Float(3.0))
            .unwrap();
        assert!(tweens.step(&mut world, 0.25).is_empty(), "no tween_done");
        assert!(tweens.is_empty());
        assert_eq!(intensity(&world, id), toml::Value::Float(3.0));
    }

    #[test]
    fn a_new_tween_of_the_field_starts_where_the_old_one_stood() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let up = toml::Value::Float(10.0);
        let linear = Easing::Linear;
        tweens
            .start(&mut world, id, "light.intensity", &up, 1.0, linear)
            .unwrap();
        tweens.step(&mut world, 0.5);
        let down = toml::Value::Float(0.0);
        tweens
            .start(&mut world, id, "light.intensity", &down, 1.0, linear)
            .unwrap();
        assert_eq!(tweens.len(), 1);
        tweens.step(&mut world, 0.5);
        assert_eq!(intensity(&world, id), toml::Value::Float(3.0));
    }

    #[test]
    fn no_time_sets_it_now_and_is_done_now() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let target = toml::Value::Float(4.5);
        let done = tweens.start(
            &mut world,
            id,
            "light.intensity",
            &target,
            0.0,
            Easing::Smooth,
        );
        assert!(matches!(done, Ok(Some(GameEvent::Custom { .. }))));
        assert_eq!(intensity(&world, id), toml::Value::Float(4.5));
        assert!(tweens.is_empty());
    }

    #[test]
    fn what_cant_ease_says_why() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let mut refused = |path: &str, target: toml::Value| {
            tweens
                .start(&mut world, id, path, &target, 1.0, Easing::Smooth)
                .unwrap_err()
        };
        let e = refused("camera.fov", 1.0.into());
        assert!(e.contains("no `camera` component"), "{e}");
        let e = refused("light.range", 1.0.into());
        assert!(e.contains("no such field"), "{e}");
        let e = refused("light.kind", 1.0.into());
        assert!(e.contains("isn't a number"), "{e}");
        let e = refused("light.color", 1.0.into());
        assert!(e.contains("a list of 3 numbers"), "{e}");
        let e = refused("light.intensity", vec![1.0, 2.0].into());
        assert!(e.contains("holds a number"), "{e}");
        let e = refused("light.intensity", f64::INFINITY.into());
        assert!(e.contains("can't ease to inf"), "{e}");
    }

    #[test]
    fn a_gone_entity_ends_its_tween_quietly() {
        let (mut world, id) = world();
        let mut tweens = FieldTweens::default();
        let target = toml::Value::Float(10.0);
        tweens
            .start(
                &mut world,
                id,
                "light.intensity",
                &target,
                1.0,
                Easing::Smooth,
            )
            .unwrap();
        world.despawn(id).unwrap();
        assert!(tweens.step(&mut world, 0.5).is_empty());
        assert!(tweens.is_empty());
    }
}
