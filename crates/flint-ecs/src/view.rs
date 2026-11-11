//! Typed reads of a component that say when they fall back (ADR 0115,
//! decision 5).
//!
//! Components are `toml::Value` tables read by string key, so a reader used
//! to be a chain of `.get("friction").and_then(..).unwrap_or(0.5)`, and a
//! value of the wrong type, or a misspelled enum string, took the default
//! and said nothing. A [`ComponentView`] is that chain with a name on it:
//!
//! ```
//! use flint_core::components as comp;
//! use flint_ecs::FlintWorld;
//!
//! let mut world = FlintWorld::new();
//! let id = world.spawn("crate").unwrap();
//! let data: toml::Value = toml::from_str("body_type = \"dynamc\"\nmass = 2").unwrap();
//! world.set_component(id, comp::RIGIDBODY, data).unwrap();
//!
//! let rb = world.view(id, comp::RIGIDBODY);
//! assert_eq!(rb.f32("mass", 1.0), 2.0); // an integer is a number
//! let kind = rb.choice("body_type", &["static", "dynamic", "kinematic"], "static");
//! assert_eq!(kind, "static"); // the default, and it was reported:
//!
//! let said = world.read_reports().snapshot();
//! assert_eq!(
//!     said[0].to_string(),
//!     "error[bad-enum-value] entity 'crate' rigidbody.body_type: \
//!      \"dynamc\" is not a value of `body_type`"
//! );
//! ```
//!
//! What a view promises:
//!
//! - **Numbers coerce.** `f32`/`f64` take a TOML integer or float; `i64`
//!   takes an integer, or a float with no fraction (`3.0`).
//! - **A missing field, or a missing component, is the default** and is not
//!   reported: [`ComponentView::exists`] says whether the component is there.
//! - **A wrong type or an unknown choice is the default too, and is
//!   reported once** per (entity name, component, field) to the world's
//!   [`ReadReports`]: a [`Diagnostic`] with the same codes schema validation
//!   uses (`bad-type`, `bad-enum-value`), logged through `tracing::warn!` and
//!   kept for whoever wants the list.
//! - **A read that finds nothing wrong allocates nothing and takes no lock**,
//!   so a system may read every tick. A read that keeps finding the same
//!   wrong value hashes three strings and takes one uncontended lock.
//!
//! The `opt_*` readers are for fields with no default (a reader that does
//! something else when the field is absent): `None` when missing, and `None`
//! and a report when wrong.

use crate::FlintWorld;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::toml_util::{toml_f32, toml_f64};
use flint_core::EntityId;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// How many diagnostics a [`ReadReports`] keeps. Each is still logged past
/// this; a game that spawns wrongly-typed entities under fresh names all
/// day should not grow the list without bound.
pub const KEPT_REPORTS: usize = 512;

/// What the typed readers have reported, each (entity name, component,
/// field) once. A [`FlintWorld`] owns one ([`FlintWorld::read_reports`]) and
/// empties it on [`FlintWorld::clear`], so a reloaded scene speaks again.
///
/// No clock and no file: it works on wasm as it does natively. Keys are a
/// 64-bit hash of the three names, so asking "was this said?" never
/// allocates.
#[derive(Default)]
pub struct ReadReports {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    seen: HashSet<u64>,
    kept: Vec<Diagnostic>,
}

impl ReadReports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Report `make()` unless this (entity, component, field) has been
    /// reported before. `make` runs only the first time. Returns whether it
    /// was the first.
    pub fn once(
        &self,
        entity: &str,
        component: &str,
        field: &str,
        make: impl FnOnce() -> Diagnostic,
    ) -> bool {
        let mut hasher = DefaultHasher::new();
        entity.hash(&mut hasher);
        component.hash(&mut hasher);
        field.hash(&mut hasher);
        let key = hasher.finish();

        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !inner.seen.insert(key) {
            return false;
        }
        let diagnostic = make();
        tracing::warn!("{}", diagnostic.one_line());
        if inner.kept.len() < KEPT_REPORTS {
            inner.kept.push(diagnostic);
        }
        true
    }

    /// Everything reported so far, in the order it was found.
    pub fn snapshot(&self) -> Vec<Diagnostic> {
        self.lock().kept.clone()
    }

    /// Everything reported so far, removed from the list. What was reported
    /// stays reported: taking the list does not make a reader repeat itself.
    pub fn take(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.lock().kept)
    }

    /// How many diagnostics are held.
    pub fn len(&self) -> usize {
        self.lock().kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget everything, so each problem is reported again when next read.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.seen.clear();
        inner.kept.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Whose component a view reads: looked up only when there is something to
/// report (`FlintWorld::get_name` walks the name map).
#[derive(Clone, Copy)]
enum Owner<'a> {
    Entity(&'a FlintWorld, EntityId),
    Named(&'a str),
}

/// One component of one entity, read by field with a default: see the
/// module docs. Made by [`FlintWorld::view`], or [`ComponentView::of`] for a
/// table that is already in hand.
#[derive(Clone, Copy)]
pub struct ComponentView<'a> {
    owner: Owner<'a>,
    component: &'a str,
    data: Option<&'a toml::Value>,
    reports: &'a ReadReports,
}

impl FlintWorld {
    /// A typed, reporting view of `component` on entity `id` (ADR 0115).
    /// A missing entity or component reads as all defaults.
    pub fn view<'a>(&'a self, id: EntityId, component: &'a str) -> ComponentView<'a> {
        let data = self.get_component(id, component);
        let view = ComponentView {
            owner: Owner::Entity(self, id),
            component,
            data,
            reports: self.read_reports(),
        };
        view.check_is_table();
        view
    }
}

impl<'a> ComponentView<'a> {
    /// A view of a component table that is already in hand, for an entity
    /// known by name. `data` is the component's value (`None`: all defaults).
    pub fn of(
        entity: &'a str,
        component: &'a str,
        data: Option<&'a toml::Value>,
        reports: &'a ReadReports,
    ) -> Self {
        let view = ComponentView {
            owner: Owner::Named(entity),
            component,
            data,
            reports,
        };
        view.check_is_table();
        view
    }

    /// Whether the entity has the component at all.
    pub fn exists(&self) -> bool {
        self.data.is_some()
    }

    /// Whether the field is written, whatever its type.
    pub fn has(&self, field: &str) -> bool {
        self.raw(field).is_some()
    }

    /// The field's value as written, unchecked: for a field whose shape no
    /// typed reader covers. Nothing is reported.
    pub fn raw(&self, field: &str) -> Option<&'a toml::Value> {
        self.data?.as_table()?.get(field)
    }

    /// The component's name.
    pub fn component(&self) -> &'a str {
        self.component
    }

    // -- numbers ---------------------------------------------------------

    /// A number, integer or float.
    pub fn f32(&self, field: &str, default: f32) -> f32 {
        self.read(field, "a number", toml_f32, Some(&default))
            .unwrap_or(default)
    }

    /// A number, integer or float.
    pub fn f64(&self, field: &str, default: f64) -> f64 {
        self.read(field, "a number", toml_f64, Some(&default))
            .unwrap_or(default)
    }

    /// An integer; a float with no fraction (`3.0`) is one too.
    pub fn i64(&self, field: &str, default: i64) -> i64 {
        self.read(field, "an integer", whole, Some(&default))
            .unwrap_or(default)
    }

    pub fn opt_f32(&self, field: &str) -> Option<f32> {
        self.read(field, "a number", toml_f32, None::<&f32>)
    }

    pub fn opt_f64(&self, field: &str) -> Option<f64> {
        self.read(field, "a number", toml_f64, None::<&f64>)
    }

    pub fn opt_i64(&self, field: &str) -> Option<i64> {
        self.read(field, "an integer", whole, None::<&i64>)
    }

    // -- flags and text --------------------------------------------------

    pub fn bool(&self, field: &str, default: bool) -> bool {
        self.read(field, "true or false", toml::Value::as_bool, Some(&default))
            .unwrap_or(default)
    }

    pub fn opt_bool(&self, field: &str) -> Option<bool> {
        self.read(field, "true or false", toml::Value::as_bool, None::<&bool>)
    }

    pub fn str(&self, field: &str, default: &'a str) -> &'a str {
        self.read_str(field, Some(default)).unwrap_or(default)
    }

    pub fn opt_str(&self, field: &str) -> Option<&'a str> {
        self.read_str(field, None)
    }

    /// One of `allowed`, as the `&str` from that list (so a `match` on the
    /// result can be exhaustive over it). Anything else is reported with the
    /// allowed values and the nearest one, and reads as `default`.
    pub fn choice<'c>(&self, field: &str, allowed: &[&'c str], default: &'c str) -> &'c str {
        self.read_choice(field, allowed, Some(default))
            .unwrap_or(default)
    }

    /// [`choice`](Self::choice) for a field with no default.
    pub fn opt_choice<'c>(&self, field: &str, allowed: &[&'c str]) -> Option<&'c str> {
        self.read_choice(field, allowed, None)
    }

    // -- vectors ---------------------------------------------------------

    /// Three numbers: an array `[x, y, z]` (more are ignored) or a table
    /// with any of `x`, `y`, `z`, the rest taken from `default`.
    pub fn vec3(&self, field: &str, default: [f32; 3]) -> [f32; 3] {
        self.read_vec3(field, Some(default)).unwrap_or(default)
    }

    /// [`vec3`](Self::vec3) for a field with no default; a table's missing
    /// members are zero.
    pub fn opt_vec3(&self, field: &str) -> Option<[f32; 3]> {
        self.read_vec3(field, None)
    }

    /// Four numbers in an array.
    pub fn vec4(&self, field: &str, default: [f32; 4]) -> [f32; 4] {
        self.read(
            field,
            "an array of 4 numbers",
            flint_core::toml_util::toml_vec4,
            Some(&Listed(&default)),
        )
        .unwrap_or(default)
    }

    pub fn opt_vec4(&self, field: &str) -> Option<[f32; 4]> {
        self.read(
            field,
            "an array of 4 numbers",
            flint_core::toml_util::toml_vec4,
            None::<&f32>,
        )
    }

    /// A colour: `[r, g, b]` (alpha 1) or `[r, g, b, a]`.
    pub fn color(&self, field: &str, default: [f32; 4]) -> [f32; 4] {
        self.read(
            field,
            "a colour, [r, g, b] or [r, g, b, a]",
            flint_core::toml_util::toml_color,
            Some(&Listed(&default)),
        )
        .unwrap_or(default)
    }

    pub fn opt_color(&self, field: &str) -> Option<[f32; 4]> {
        self.read(
            field,
            "a colour, [r, g, b] or [r, g, b, a]",
            flint_core::toml_util::toml_color,
            None::<&f32>,
        )
    }

    // -- reporting -------------------------------------------------------

    /// Report something a reader found that no typed read covers (a value
    /// out of the range it can use, a field that needs another), once for
    /// this entity, component and `field`. `make` builds the diagnostic; the
    /// entity, component and field are filled in.
    pub fn report(&self, field: &str, make: impl FnOnce() -> Diagnostic) {
        self.with_entity(|entity| {
            self.reports.once(entity, self.component, field, || {
                let d = make().entity(entity).component(self.component);
                if field.is_empty() {
                    d
                } else {
                    d.field(field)
                }
            });
        });
    }

    // -- the machinery ---------------------------------------------------

    fn with_entity(&self, f: impl FnOnce(&str)) {
        match self.owner {
            Owner::Named(name) => f(name),
            Owner::Entity(world, id) => f(world.get_name(id).unwrap_or("?")),
        }
    }

    /// A component that is not a table has no fields: say so once.
    fn check_is_table(&self) {
        if let Some(data) = self.data {
            if !data.is_table() {
                self.report("", || {
                    Diagnostic::error(
                        codes::BAD_TYPE,
                        format!("a component is a table, got {}", shown(data)),
                    )
                    .hint("every field reads as its default")
                });
            }
        }
    }

    /// The one read: missing is `None` in silence, a value `get` accepts is
    /// `Some`, anything else is `None` and a report.
    fn read<T, D: std::fmt::Display + ?Sized>(
        &self,
        field: &str,
        expected: &str,
        get: impl FnOnce(&toml::Value) -> Option<T>,
        default: Option<&D>,
    ) -> Option<T> {
        let value = self.raw(field)?;
        let read = get(value);
        if read.is_none() {
            self.wrong_type(field, expected, value, default.map(|d| d.to_string()));
        }
        read
    }

    fn read_str(&self, field: &str, default: Option<&str>) -> Option<&'a str> {
        let value = self.raw(field)?;
        let read = value.as_str();
        if read.is_none() {
            self.wrong_type(
                field,
                "a string",
                value,
                default.map(|d| format!("\"{d}\"")),
            );
        }
        read
    }

    fn read_choice<'c>(
        &self,
        field: &str,
        allowed: &[&'c str],
        default: Option<&'c str>,
    ) -> Option<&'c str> {
        let value = self.raw(field)?;
        let Some(written) = value.as_str() else {
            self.wrong_type(
                field,
                &format!("one of {}", allowed.join(", ")),
                value,
                default.map(|d| format!("\"{d}\"")),
            );
            return None;
        };
        if let Some(found) = allowed.iter().find(|a| **a == written) {
            return Some(found);
        }
        self.report(field, || {
            let mut hint = String::new();
            if let Some(near) = diagnostic::did_you_mean(written, allowed.iter().copied()) {
                hint.push_str(&near);
                hint.push(' ');
            }
            hint.push_str("allowed values: ");
            hint.push_str(&allowed.join(", "));
            hint.push_str(&fallback(default.map(|d| format!("\"{d}\""))));
            Diagnostic::error(
                codes::BAD_ENUM_VALUE,
                format!("\"{written}\" is not a value of `{field}`"),
            )
            .hint(hint)
        });
        None
    }

    fn read_vec3(&self, field: &str, default: Option<[f32; 3]>) -> Option<[f32; 3]> {
        let value = self.raw(field)?;
        let read = match value {
            toml::Value::Array(_) => flint_core::toml_util::toml_vec3(value),
            toml::Value::Table(table) => {
                let base = default.unwrap_or([0.0; 3]);
                let member = |key: &str, base: f32| match table.get(key) {
                    None => Some(base),
                    Some(v) => toml_f32(v),
                };
                (|| {
                    Some([
                        member("x", base[0])?,
                        member("y", base[1])?,
                        member("z", base[2])?,
                    ])
                })()
            }
            _ => None,
        };
        if read.is_none() {
            self.wrong_type(
                field,
                "3 numbers, [x, y, z]",
                value,
                default.map(|d| Listed(&d).to_string()),
            );
        }
        read
    }

    fn wrong_type(&self, field: &str, expected: &str, got: &toml::Value, default: Option<String>) {
        self.report(field, || {
            Diagnostic::error(
                codes::BAD_TYPE,
                format!("expected {expected}, got {}", shown(got)),
            )
            .hint(fallback(default).trim_start_matches("; ").to_string())
        });
    }
}

/// `; the default 5 is used`, or `; read as if it were not set`.
fn fallback(default: Option<String>) -> String {
    match default {
        Some(default) => format!("; the default {default} is used"),
        None => "; read as if it were not set".to_string(),
    }
}

/// An integer, or a float that is one.
fn whole(value: &toml::Value) -> Option<i64> {
    match value {
        toml::Value::Integer(i) => Some(*i),
        toml::Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
        _ => None,
    }
}

/// A value for a message: its type and what was written, cut short.
fn shown(value: &toml::Value) -> String {
    let mut text = value.to_string();
    if text.chars().count() > 40 {
        text = text.chars().take(37).collect::<String>() + "...";
    }
    format!("{} {}", value.type_str(), text)
}

/// `[1, 0.5, 0]` for a default in a hint.
struct Listed<'a>(&'a [f32]);

impl std::fmt::Display for Listed<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[")?;
        for (i, n) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{n}")?;
        }
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::components as comp;
    use flint_core::diagnostic::Severity;

    fn world_with(entity: &str, component: &str, toml_text: &str) -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn(entity).unwrap();
        let data: toml::Value = toml::from_str(toml_text).unwrap();
        world.set_component(id, component, data).unwrap();
        (world, id)
    }

    #[test]
    fn integers_and_floats_are_both_numbers() {
        let (world, id) = world_with(
            "player",
            comp::CHARACTER_CONTROLLER,
            "move_speed = 8\njump_force = 6.5\nsteps = 3.0\nlives = 2",
        );
        let c = world.view(id, comp::CHARACTER_CONTROLLER);
        assert_eq!(c.f32("move_speed", 5.0), 8.0);
        assert_eq!(c.f64("move_speed", 5.0), 8.0);
        assert_eq!(c.f32("jump_force", 6.0), 6.5);
        assert_eq!(c.i64("steps", 1), 3, "a float with no fraction is whole");
        assert_eq!(c.i64("lives", 1), 2);
        assert_eq!(c.opt_f32("move_speed"), Some(8.0));
        assert!(world.read_reports().is_empty());
    }

    #[test]
    fn a_missing_field_or_component_is_the_default_in_silence() {
        let (world, id) = world_with("crate", comp::COLLIDER, "friction = 0.2");
        let c = world.view(id, comp::COLLIDER);
        assert!(c.exists());
        assert!(c.has("friction") && !c.has("restitution"));
        assert_eq!(c.f32("restitution", 0.25), 0.25);
        assert_eq!(c.str("shape", "box"), "box");
        assert_eq!(c.choice("shape", &["box", "sphere"], "box"), "box");
        assert_eq!(c.vec3("size", [1.0; 3]), [1.0; 3]);
        assert_eq!(c.opt_f32("restitution"), None);
        assert_eq!(c.opt_str("shape"), None);

        let none = world.view(id, comp::RIGIDBODY);
        assert!(!none.exists());
        assert_eq!(none.f32("mass", 1.0), 1.0);
        assert!(!none.bool("mode_2d", false));

        let nobody = world.view(EntityId::new(), comp::RIGIDBODY);
        assert_eq!(nobody.f32("mass", 1.0), 1.0);
        assert!(world.read_reports().is_empty());
    }

    #[test]
    fn a_wrong_type_is_the_default_and_is_reported_once() {
        let (world, id) = world_with(
            "player",
            comp::CHARACTER_CONTROLLER,
            "move_speed = \"fast\"\ngrounded = 1\nlives = 2.5\nname = 7",
        );
        let c = world.view(id, comp::CHARACTER_CONTROLLER);
        for _ in 0..100 {
            assert_eq!(c.f32("move_speed", 5.0), 5.0);
        }
        // A second view of the same component is the same reader
        assert_eq!(
            world
                .view(id, comp::CHARACTER_CONTROLLER)
                .f32("move_speed", 5.0),
            5.0
        );
        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 1, "{said:?}");
        let d = &said[0];
        assert_eq!(d.code, codes::BAD_TYPE);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.entity.as_deref(), Some("player"));
        assert_eq!(d.component.as_deref(), Some(comp::CHARACTER_CONTROLLER));
        assert_eq!(d.field.as_deref(), Some("move_speed"));
        assert_eq!(d.message, "expected a number, got string \"fast\"");
        assert_eq!(d.hint.as_deref(), Some("the default 5 is used"));

        assert!(c.bool("grounded", true), "1 is not a bool: the default");
        assert_eq!(c.i64("lives", 3), 3, "2.5 is not whole");
        assert_eq!(c.str("name", "anon"), "anon");
        assert_eq!(c.opt_f32("name"), Some(7.0));
        let said = world.read_reports().snapshot();
        let fields: Vec<_> = said.iter().filter_map(|d| d.field.as_deref()).collect();
        assert_eq!(fields, ["move_speed", "grounded", "lives", "name"]);
        assert_eq!(said[2].message, "expected an integer, got float 2.5");
    }

    #[test]
    fn an_optional_read_reports_and_says_it_was_dropped() {
        let (world, id) = world_with("player", comp::CHARACTER_CONTROLLER, "height = \"tall\"");
        let c = world.view(id, comp::CHARACTER_CONTROLLER);
        assert_eq!(c.opt_f32("height"), None);
        let said = world.read_reports().take();
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].hint.as_deref(), Some("read as if it were not set"));
        // Taking the list does not make the reader say it again
        assert_eq!(c.opt_f32("height"), None);
        assert!(world.read_reports().is_empty());
    }

    #[test]
    fn an_unknown_choice_names_the_entity_the_allowed_values_and_the_nearest() {
        let (world, id) = world_with("crate", comp::RIGIDBODY, "body_type = \"dynamc\"");
        let rb = world.view(id, comp::RIGIDBODY);
        let allowed = ["static", "dynamic", "kinematic"];
        assert_eq!(rb.choice("body_type", &allowed, "static"), "static");
        assert_eq!(rb.choice("body_type", &allowed, "static"), "static");
        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 1);
        assert_eq!(
            said[0].one_line(),
            "error[bad-enum-value] entity 'crate' rigidbody.body_type: \"dynamc\" is not a \
             value of `body_type` (did you mean `dynamic`? allowed values: static, dynamic, \
             kinematic; the default \"static\" is used)"
        );

        // Nothing near: the allowed values alone. Not a string: a type error
        let (world, id) = world_with("orb", comp::COLLIDER, "shape = \"blob\"\nkind = 3");
        let c = world.view(id, comp::COLLIDER);
        assert_eq!(c.choice("shape", &["box", "sphere"], "box"), "box");
        assert_eq!(c.opt_choice("kind", &["a", "b"]), None);
        let said = world.read_reports().snapshot();
        assert_eq!(
            said[0].hint.as_deref(),
            Some("allowed values: box, sphere; the default \"box\" is used")
        );
        assert_eq!(said[1].code, codes::BAD_TYPE);
        assert_eq!(said[1].message, "expected one of a, b, got integer 3");
    }

    #[test]
    fn a_known_choice_comes_back_as_the_listed_str() {
        let (world, id) = world_with("orb", comp::COLLIDER, "shape = \"sphere\"");
        let allowed = ["box", "sphere"];
        let shape = world
            .view(id, comp::COLLIDER)
            .choice("shape", &allowed, "box");
        assert!(std::ptr::eq(shape, allowed[1]));
        assert!(world.read_reports().is_empty());
    }

    #[test]
    fn vectors_take_arrays_and_tables() {
        let (world, id) = world_with(
            "crate",
            comp::COLLIDER,
            "size = [2, 0.5, 1]\noffset = { y = 3 }\nrotation = [0, 90]\ntint = [1, 0, 0]\n\
             quat = [0, 0, 0, 1]\nbad = { x = \"left\" }",
        );
        let c = world.view(id, comp::COLLIDER);
        assert_eq!(c.vec3("size", [1.0; 3]), [2.0, 0.5, 1.0]);
        assert_eq!(c.vec3("offset", [1.0; 3]), [1.0, 3.0, 1.0]);
        assert_eq!(c.opt_vec3("offset"), Some([0.0, 3.0, 0.0]));
        assert_eq!(c.color("tint", [0.0; 4]), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(c.vec4("quat", [0.0; 4]), [0.0, 0.0, 0.0, 1.0]);
        assert!(world.read_reports().is_empty());

        assert_eq!(c.vec3("rotation", [0.0; 3]), [0.0; 3]);
        assert_eq!(c.vec3("bad", [0.0; 3]), [0.0; 3]);
        assert_eq!(c.vec4("size", [9.0; 4]), [9.0; 4]);
        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 3);
        assert_eq!(
            said[0].message,
            "expected 3 numbers, [x, y, z], got array [0, 90]"
        );
        assert_eq!(
            said[0].hint.as_deref(),
            Some("the default [0, 0, 0] is used")
        );
    }

    #[test]
    fn a_component_that_is_not_a_table_is_reported_and_reads_as_defaults() {
        let mut world = FlintWorld::new();
        let id = world.spawn("crate").unwrap();
        world
            .set_component(id, comp::COLLIDER, toml::Value::Integer(5))
            .unwrap();
        let c = world.view(id, comp::COLLIDER);
        assert!(c.exists());
        assert_eq!(c.f32("friction", 0.5), 0.5);
        let _ = world.view(id, comp::COLLIDER);
        let said = world.read_reports().snapshot();
        assert_eq!(said.len(), 1);
        assert_eq!(
            said[0].to_string(),
            "error[bad-type] entity 'crate' component 'collider': a component is a table, got \
             integer 5"
        );
    }

    #[test]
    fn each_entity_and_field_is_its_own_report_and_clear_starts_over() {
        let mut world = FlintWorld::new();
        for name in ["a", "b"] {
            let id = world.spawn(name).unwrap();
            let data: toml::Value = toml::from_str("mass = \"heavy\"").unwrap();
            world.set_component(id, comp::RIGIDBODY, data).unwrap();
        }
        let read_all = |world: &FlintWorld| {
            for name in ["a", "b"] {
                let id = world.get_id(name).unwrap();
                world.view(id, comp::RIGIDBODY).f32("mass", 1.0);
            }
        };
        read_all(&world);
        read_all(&world);
        assert_eq!(world.read_reports().len(), 2);
        world.read_reports().clear();
        read_all(&world);
        assert_eq!(world.read_reports().len(), 2);

        // A cleared world forgets, so a reloaded scene is told again
        world.clear();
        assert!(world.read_reports().is_empty());
    }

    #[test]
    fn a_view_of_a_table_in_hand_and_a_readers_own_report() {
        let reports = ReadReports::new();
        let data: toml::Value = toml::from_str("bus = \"musik\"").unwrap();
        let view = ComponentView::of("radio", comp::AUDIO_SOURCE, Some(&data), &reports);
        assert_eq!(view.choice("bus", &["sfx", "music"], "sfx"), "sfx");
        view.report("file", || {
            Diagnostic::warning(codes::BAD_TYPE, "no file, so nothing plays")
        });
        view.report("file", || unreachable!("said once"));
        let said = reports.snapshot();
        assert_eq!(said.len(), 2);
        assert_eq!(said[0].entity.as_deref(), Some("radio"));
        assert!(said[0].hint.as_deref().unwrap().contains("`music`"));
        assert_eq!(said[1].field.as_deref(), Some("file"));
        assert_eq!(view.component(), comp::AUDIO_SOURCE);
    }

    #[test]
    fn the_kept_list_is_bounded() {
        let reports = ReadReports::new();
        for i in 0..KEPT_REPORTS + 10 {
            let name = format!("e{i}");
            let first = reports.once(&name, "c", "f", || {
                Diagnostic::error(codes::BAD_TYPE, "wrong")
            });
            assert!(first);
        }
        assert_eq!(reports.len(), KEPT_REPORTS);
        assert!(!reports.once("e0", "c", "f", || unreachable!()));
    }
}
