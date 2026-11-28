//! `<entity>.<component>.<field>` (#1699): a key of `flint render`'s `--set`
//! and `--sweep` that names one field of one entity's component, so a
//! terrain's grass, its tiling or the sun can be tuned or swept in one
//! process:
//!
//! ```text
//! --set island.terrain.grass.density=60
//! --set sun.light.direction=0.8,0.1,-0.3
//! --sweep island.terrain.texture_tile=8,18,32
//! --sweep sun.light.direction=0.8,0.7,0.2|0.8,0.3,0.2|0.8,0.1,0.2
//! ```
//!
//! A key is one by its spelling alone (three or more parts, the first not
//! `post`, `wooze` or `living`: [`EntityField::parse`]), so clap takes it
//! before any scene loads. What it names is checked once the scene has
//! loaded, against the world and the component's schema
//! ([`EntityField::resolve`]): an entity, a component or a field that isn't
//! there is an error naming what is, with the nearest name. The value is
//! read as the schema types the field (a number, a whole number, a bool, a
//! vector, one of an enum's values) and held to its `min` / `max`.
//!
//! The value is written into the loaded world ([`EntityField::lay`]),
//! after the scene's packages settle it and before the renderer binds it,
//! so what a package builds at bind (a terrain's chunks and grass) is
//! built from the value. A sweep of one draws a world a cell, through the
//! stage path (`render_stages.rs`).

use flint_core::diagnostic::closest;
use flint_ecs::FlintWorld;
use flint_schema::{FieldSchema, FieldType, SchemaRegistry};

/// One field of one entity's component, as a key spells it.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityField {
    pub entity: String,
    pub component: String,
    /// The rest of the key: `grass.density`, `direction`.
    pub field: String,
}

/// Most names an error lists before it counts the rest.
const LISTED: usize = 24;

/// Where in the component's table the field is.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    /// A key of the component's own table, as written: the schema's
    /// `"grass.density"` is one key, as the loader keeps it.
    Key(String),
    /// Down tables of the component, one key a step (`look.at`).
    Path(Vec<String>),
}

/// A key checked against a world: where the value goes, and the value.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    place: Place,
    pub value: toml::Value,
}

impl EntityField {
    /// `<entity>.<component>.<field>`, or `None` for a key of another
    /// family (`post.exposure`, `ascii.cell`, `wooze.glide`) or one with an
    /// empty part.
    pub fn parse(key: &str) -> Option<Self> {
        // The families' prefixes are read in one place, `Family::of`
        if super::sweep::Family::of(key) != super::sweep::Family::Entity {
            return None;
        }
        let mut parts = key.splitn(3, '.');
        let (entity, component, field) = (parts.next()?, parts.next()?, parts.next()?);
        if entity.is_empty() || component.is_empty() || field.split('.').any(str::is_empty) {
            return None;
        }
        Some(Self {
            entity: entity.to_string(),
            component: component.to_string(),
            field: field.to_string(),
        })
    }

    /// Whether a key has the shape of an entity's field: three or more
    /// parts. `Family::of` asks it once the other families' prefixes have
    /// passed, so `post.a.b` stays a post key.
    pub fn spelled(key: &str) -> bool {
        key.split('.').count() >= 3
    }

    /// The key as written.
    pub fn name(&self) -> String {
        format!("{}.{}.{}", self.entity, self.component, self.field)
    }

    /// A sheet's label for one value: the field's last part and the value
    /// (`density 30`), as a wooze knob's is. The sheet's subtitle names the
    /// whole key, and a grid's row labels have little room.
    pub fn label(&self, value: &str) -> String {
        let last = self.field.rsplit('.').next().unwrap_or(&self.field);
        format!("{last} {value}")
    }

    /// Check the key against `world` and the component's schema, and read
    /// `text` as the field's value. The error names what is there.
    pub fn resolve(
        &self,
        world: &FlintWorld,
        registry: &SchemaRegistry,
        text: &str,
    ) -> Result<Resolved, String> {
        let Some(id) = world.get_id(&self.entity) else {
            let mut names: Vec<&str> = world.entity_names().collect();
            names.sort_unstable();
            return Err(format!(
                "no entity '{}' in this scene{}; its entities: {}",
                self.entity,
                near(&self.entity, names.iter().copied()),
                listed(&names)
            ));
        };
        let components = world
            .get_components(id)
            .map(|c| {
                let mut names = c.component_names();
                names.sort_unstable();
                names
            })
            .unwrap_or_default();
        let Some(data) = world.get_component(id, &self.component) else {
            return Err(format!(
                "entity '{}' has no `{}` component{}; it has: {}",
                self.entity,
                self.component,
                near(&self.component, components.iter().copied()),
                listed(&components)
            ));
        };
        let schema = registry.get_component(&self.component);
        let declared = schema.and_then(|s| declared(&s.fields, &self.field));
        let place = match &declared {
            Some((place, _)) => place.clone(),
            None => match written(data, &self.field) {
                Some(place) => place,
                None => {
                    let mut names: Vec<String> = match schema.filter(|s| !s.is_open()) {
                        Some(schema) => schema.fields.keys().cloned().collect(),
                        None => keys(data),
                    };
                    names.sort_unstable();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    return Err(format!(
                        "`{}` has no field `{}`{}; its fields: {}{}",
                        self.component,
                        self.field,
                        near(&self.field, names.iter().copied()),
                        listed(&names),
                        if schema.is_some() {
                            format!(" (`flint schema {}` describes each)", self.component)
                        } else {
                            String::new()
                        }
                    ));
                }
            },
        };
        let value = match &declared {
            Some((_, field)) => typed(field, text)?,
            None => like(at(data, &place), text),
        };
        Ok(Resolved { place, value })
    }

    /// Write `text` into the entity's component in `world`, checked as
    /// [`EntityField::resolve`] checks it.
    pub fn lay(
        &self,
        world: &mut FlintWorld,
        registry: &SchemaRegistry,
        text: &str,
    ) -> Result<(), String> {
        let resolved = self.resolve(world, registry, text)?;
        let id = world.get_id(&self.entity).expect("resolve found it");
        let data = world
            .get_component_mut(id, &self.component)
            .expect("resolve found it");
        put(data, &resolved.place, resolved.value);
        Ok(())
    }
}

/// The field the schema declares under `field`: a key of its own
/// (`grass.density`, `direction`), or a key inside a field that declares
/// its own fields (`shear.xy`).
fn declared<'a>(
    fields: &'a std::collections::HashMap<String, FieldSchema>,
    field: &str,
) -> Option<(Place, &'a FieldSchema)> {
    if let Some(schema) = fields.get(field) {
        return Some((Place::Key(field.to_string()), schema));
    }
    let (head, rest) = field.split_once('.')?;
    let FieldType::Fields { fields: inner } = &fields.get(head)?.field_type else {
        return None;
    };
    match declared(inner, rest)? {
        (Place::Key(key), schema) => Some((Place::Path(vec![head.to_string(), key]), schema)),
        (Place::Path(mut path), schema) => {
            path.insert(0, head.to_string());
            Some((Place::Path(path), schema))
        }
    }
}

/// Where the component's table already writes `field`, for a field its
/// schema doesn't declare (a script's, or a component with no schema).
fn written(data: &toml::Value, field: &str) -> Option<Place> {
    let table = data.as_table()?;
    if table.contains_key(field) {
        return Some(Place::Key(field.to_string()));
    }
    let path: Vec<String> = field.split('.').map(str::to_string).collect();
    let mut node = data;
    for step in &path {
        node = node.as_table()?.get(step)?;
    }
    Some(Place::Path(path))
}

/// The component's keys, nested ones dotted: what a component with no
/// schema writes.
fn keys(data: &toml::Value) -> Vec<String> {
    let mut out = Vec::new();
    for (key, value) in data.as_table().into_iter().flatten() {
        match value.as_table() {
            Some(_) => out.extend(
                keys(value)
                    .into_iter()
                    .map(|inner| format!("{key}.{inner}")),
            ),
            None => out.push(key.clone()),
        }
    }
    out
}

fn at<'a>(data: &'a toml::Value, place: &Place) -> Option<&'a toml::Value> {
    match place {
        Place::Key(key) => data.get(key.as_str()),
        Place::Path(path) => path
            .iter()
            .try_fold(data, |node, step| node.get(step.as_str())),
    }
}

fn put(data: &mut toml::Value, place: &Place, value: toml::Value) {
    let path = match place {
        Place::Key(key) => std::slice::from_ref(key),
        Place::Path(path) => path.as_slice(),
    };
    let (last, steps) = path.split_last().expect("a place has a key");
    let mut node = data;
    for step in steps {
        let table = node.as_table_mut().expect("a component is a table");
        node = table
            .entry(step.clone())
            .or_insert_with(|| toml::Value::Table(Default::default()));
        if !node.is_table() {
            *node = toml::Value::Table(Default::default());
        }
    }
    if let Some(table) = node.as_table_mut() {
        table.insert(last.clone(), value);
    }
}

/// `text` as the field's type says, held to its range.
fn typed(field: &FieldSchema, text: &str) -> Result<toml::Value, String> {
    let numbers = |count: &[usize], what: &str| -> Result<toml::Value, String> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        let values: Option<Vec<f64>> = parts.iter().map(|p| p.parse().ok()).collect();
        match values.filter(|v| count.contains(&v.len())) {
            Some(values) => Ok(toml::Value::Array(
                values.into_iter().map(toml::Value::Float).collect(),
            )),
            None => Err(format!("'{text}' is not {what}")),
        }
    };
    let number = || -> Result<f64, String> {
        text.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("'{text}' is not a number"))
    };
    let value = match &field.field_type {
        FieldType::F32 | FieldType::F64 => toml::Value::Float(number()?),
        FieldType::I32 | FieldType::I64 => toml::Value::Integer(
            text.parse()
                .map_err(|_| format!("'{text}' is not a whole number"))?,
        ),
        FieldType::Bool => toml::Value::Boolean(
            text.parse()
                .map_err(|_| format!("'{text}' is not true or false"))?,
        ),
        FieldType::Vec2 => numbers(&[2], "two numbers, x,y")?,
        FieldType::Vec3 => numbers(&[3], "three numbers, x,y,z")?,
        FieldType::Vec4 => numbers(&[4], "four numbers, x,y,z,w")?,
        FieldType::Color => numbers(&[3, 4], "a colour, r,g,b or r,g,b,a")?,
        FieldType::Enum { values } => {
            if !values.iter().any(|v| v == text) {
                return Err(format!(
                    "'{text}' is not one of {}{}",
                    values.join(", "),
                    near(text, values.iter().map(String::as_str))
                ));
            }
            toml::Value::String(text.to_string())
        }
        FieldType::String | FieldType::Entity | FieldType::Asset { .. } => {
            toml::Value::String(text.to_string())
        }
        _ => super::sweep::parse_value(text),
    };
    if let Some(v) = flint_core::toml_util::toml_f64(&value) {
        let low = field.min.filter(|min| v < *min);
        let high = field.max.filter(|max| v > *max);
        if low.is_some() || high.is_some() {
            let range = match (field.min, field.max) {
                (Some(min), Some(max)) => format!("from {min} to {max}"),
                (Some(min), None) => format!("at least {min}"),
                (None, Some(max)) => format!("at most {max}"),
                (None, None) => unreachable!(),
            };
            return Err(format!("{text} is out of range: the field takes {range}"));
        }
    }
    Ok(value)
}

/// `text` in the kind of what the field holds now, for a field no schema
/// types: a number stays a number (a float unless both are whole), a list
/// of numbers a list.
fn like(now: Option<&toml::Value>, text: &str) -> toml::Value {
    let value = super::sweep::parse_value(text);
    match (now, &value) {
        (Some(toml::Value::Float(_)), toml::Value::Integer(i)) => toml::Value::Float(*i as f64),
        (Some(toml::Value::Array(_)), toml::Value::Integer(_) | toml::Value::Float(_)) => {
            toml::Value::Array(vec![value])
        }
        _ => value,
    }
}

fn near<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    closest(name, candidates)
        .map(|c| format!(" (did you mean `{c}`?)"))
        .unwrap_or_default()
}

fn listed(names: &[&str]) -> String {
    if names.is_empty() {
        return "none".into();
    }
    let shown = names[..names.len().min(LISTED)].join(", ");
    match names.len().saturating_sub(LISTED) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"
[component.terrain]
[component.terrain.fields]
texture_tile = { type = "f32", default = 16.0, min = 0.5 }
chunk_resolution = { type = "i32", default = 64 }
"grass.enabled" = { type = "bool", default = false }
"grass.density" = { type = "f32", default = 8.0 }

[component.light]
[component.light.fields]
type = { type = "enum", values = ["directional", "point", "spot"] }
direction = { type = "vec3" }
color = { type = "vec3", default = [1.0, 1.0, 1.0] }
"#;

    fn world() -> (FlintWorld, SchemaRegistry) {
        let mut registry = SchemaRegistry::new();
        registry.load_component_string(SCHEMA).unwrap();
        let mut world = FlintWorld::new();
        for (name, component, text) in [
            (
                "island",
                "terrain",
                "texture_tile = 16.0\n\"grass.density\" = 8.0\n\"grass.enabled\" = true",
            ),
            (
                "sun",
                "light",
                "type = \"directional\"\ndirection = [0, 1, 0]",
            ),
            ("crate", "spin", "speed = 2.0\nlook = { at = [0, 0, 0] }"),
        ] {
            let id = world.spawn(name).unwrap();
            world
                .set_component(id, component, toml::from_str(text).unwrap())
                .unwrap();
        }
        (world, registry)
    }

    fn field(key: &str) -> EntityField {
        EntityField::parse(key).unwrap_or_else(|| panic!("{key} is an entity's field"))
    }

    #[test]
    fn a_key_is_an_entitys_by_its_spelling() {
        let f = field("island.terrain.grass.density");
        assert_eq!(
            (f.entity.as_str(), f.component.as_str(), f.field.as_str()),
            ("island", "terrain", "grass.density")
        );
        assert_eq!(f.name(), "island.terrain.grass.density");
        assert_eq!(f.label("30"), "density 30");
        for other in [
            "post.exposure",
            "ascii.cell",
            "wooze.glide",
            "living.echoes.count",
            "post.a.b",
            "a..b",
            "a.b.",
            "look",
        ] {
            assert_eq!(EntityField::parse(other), None, "{other}");
        }
        assert!(EntityField::spelled("sun.light.direction"));
        assert!(!EntityField::spelled("ascii.cell"));
    }

    #[test]
    fn a_field_is_written_where_its_reader_looks() {
        let (mut world, registry) = world();
        field("island.terrain.grass.density")
            .lay(&mut world, &registry, "60")
            .unwrap();
        field("sun.light.direction")
            .lay(&mut world, &registry, "0.8,0.1,-0.3")
            .unwrap();
        // A declared field the scene left out is written too
        field("island.terrain.chunk_resolution")
            .lay(&mut world, &registry, "32")
            .unwrap();
        let island = world.get_id("island").unwrap();
        let terrain = world.view(island, "terrain");
        assert_eq!(terrain.f32("grass.density", 0.0), 60.0);
        assert_eq!(terrain.f32("texture_tile", 0.0), 16.0, "the rest is kept");
        assert_eq!(
            world.get_component(island, "terrain").unwrap()["chunk_resolution"],
            toml::Value::Integer(32)
        );
        let sun = world.get_id("sun").unwrap();
        assert_eq!(
            world.view(sun, "light").vec3("direction", [0.0; 3]),
            [0.8, 0.1, -0.3]
        );
        // A schema-less component's nested field, in the kind it holds
        field("crate.spin.look.at")
            .lay(&mut world, &registry, "1,2,3")
            .unwrap();
        field("crate.spin.speed")
            .lay(&mut world, &registry, "4")
            .unwrap();
        let spin = world
            .get_component(world.get_id("crate").unwrap(), "spin")
            .unwrap();
        assert_eq!(spin["look"]["at"].as_array().unwrap().len(), 3);
        assert_eq!(spin["speed"], toml::Value::Float(4.0));
    }

    #[test]
    fn what_isnt_there_is_named_with_what_is() {
        let (world, registry) = world();
        let err =
            |key: &str, value: &str| field(key).resolve(&world, &registry, value).unwrap_err();
        let e = err("iland.terrain.grass.density", "60");
        assert!(e.contains("no entity 'iland'"), "{e}");
        assert!(e.contains("did you mean `island`?"), "{e}");
        assert!(e.contains("crate, island, sun"), "{e}");
        let e = err("island.terran.grass.density", "60");
        assert!(e.contains("no `terran` component"), "{e}");
        assert!(e.contains("did you mean `terrain`?"), "{e}");
        let e = err("island.terrain.grass.densty", "60");
        assert!(e.contains("no field `grass.densty`"), "{e}");
        assert!(e.contains("did you mean `grass.density`?"), "{e}");
        assert!(e.contains("texture_tile"), "{e}");
        assert!(e.contains("flint schema terrain"), "{e}");
        let e = err("crate.spin.sped", "1");
        assert!(e.contains("did you mean `speed`?"), "{e}");
        assert!(e.contains("look.at, speed"), "{e}");
    }

    #[test]
    fn a_value_is_read_as_its_field_is_typed() {
        let (world, registry) = world();
        let err =
            |key: &str, value: &str| field(key).resolve(&world, &registry, value).unwrap_err();
        assert!(err("island.terrain.texture_tile", "wide").contains("not a number"));
        assert!(err("island.terrain.texture_tile", "0.1").contains("at least 0.5"));
        assert!(err("island.terrain.chunk_resolution", "1.5").contains("whole number"));
        assert!(err("island.terrain.grass.enabled", "yes").contains("true or false"));
        assert!(err("sun.light.direction", "1,2").contains("three numbers"));
        let e = err("sun.light.type", "pointt");
        assert!(e.contains("did you mean `point`?"), "{e}");
        let ok =
            |key: &str, value: &str| field(key).resolve(&world, &registry, value).unwrap().value;
        assert_eq!(
            ok("island.terrain.texture_tile", "8"),
            toml::Value::Float(8.0)
        );
        assert_eq!(
            ok("sun.light.type", "spot"),
            toml::Value::String("spot".into())
        );
    }
}
