//! Dynamic component storage

use flint_core::EntityId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Dynamic components stored as TOML values
///
/// This allows archetypes to be defined at runtime in schema files
/// rather than requiring Rust types for each component.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicComponents {
    /// The archetype name for this entity (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Component data: component_name -> field data. Ordered by name, so
    /// any walk of it is the same in every process (#1115): a `HashMap`
    /// here gave the viewer's inspector a new component order each launch.
    #[serde(flatten)]
    pub data: BTreeMap<String, toml::Value>,
}

impl DynamicComponents {
    /// Create empty components
    pub fn new() -> Self {
        Self::default()
    }

    /// Create components with an archetype
    pub fn with_archetype(archetype: impl Into<String>) -> Self {
        Self {
            archetype: Some(archetype.into()),
            data: BTreeMap::new(),
        }
    }

    /// Get component data by name
    pub fn get(&self, component: &str) -> Option<&toml::Value> {
        self.data.get(component)
    }

    /// Get mutable component data by name
    pub fn get_mut(&mut self, component: &str) -> Option<&mut toml::Value> {
        self.data.get_mut(component)
    }

    /// Set component data
    pub fn set(&mut self, component: impl Into<String>, data: toml::Value) {
        self.data.insert(component.into(), data);
    }

    /// Remove a component
    pub fn remove(&mut self, component: &str) -> Option<toml::Value> {
        self.data.remove(component)
    }

    /// Check if a component exists
    pub fn has(&self, component: &str) -> bool {
        self.data.contains_key(component)
    }

    /// The names of the components held, in name order (#688)
    pub fn component_names(&self) -> Vec<&str> {
        self.data.keys().map(|s| s.as_str()).collect()
    }

    /// Get a field value from a component
    pub fn get_field(&self, component: &str, field: &str) -> Option<&toml::Value> {
        self.data.get(component).and_then(|v| v.get(field))
    }

    /// Merge fields into an existing component (archetype defaults + entity overrides)
    ///
    /// If the component already exists as a table, merges individual fields
    /// from `data` into it (entity-level fields win). If it doesn't exist
    /// or isn't a table, sets the component outright.
    pub fn merge_component(&mut self, component: impl Into<String>, data: toml::Value) {
        let key = component.into();
        if let Some(existing) = self.data.get_mut(&key) {
            if let (Some(existing_table), Some(override_table)) =
                (existing.as_table_mut(), data.as_table())
            {
                for (k, v) in override_table {
                    existing_table.insert(k.clone(), v.clone());
                }
                return;
            }
        }
        self.data.insert(key, data);
    }

    /// Set a field value in a component
    pub fn set_field(&mut self, component: &str, field: &str, value: toml::Value) {
        let comp = self
            .data
            .entry(component.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

        if let Some(table) = comp.as_table_mut() {
            table.insert(field.to_string(), value);
        }
    }

    /// Merge data from another DynamicComponents
    pub fn merge(&mut self, other: &DynamicComponents) {
        for (name, value) in &other.data {
            if let Some(existing) = self.data.get_mut(name) {
                // Merge tables, overwrite scalars
                if let (Some(existing_table), Some(other_table)) =
                    (existing.as_table_mut(), value.as_table())
                {
                    for (k, v) in other_table {
                        existing_table.insert(k.clone(), v.clone());
                    }
                } else {
                    self.data.insert(name.clone(), value.clone());
                }
            } else {
                self.data.insert(name.clone(), value.clone());
            }
        }
    }
}

/// The component index a world keeps: component name -> the entities that
/// have it, in id order.
pub(crate) type ComponentIndex = HashMap<String, BTreeSet<EntityId>>;

/// Record that `id` has `component` (no allocation when the component is
/// already indexed, which is every per-frame write).
pub(crate) fn index_add(index: &mut ComponentIndex, component: &str, id: EntityId) {
    match index.get_mut(component) {
        Some(set) => {
            set.insert(id);
        }
        None => {
            index.insert(component.to_string(), BTreeSet::from([id]));
        }
    }
}

/// Record that `id` no longer has `component`.
pub(crate) fn index_remove(index: &mut ComponentIndex, component: &str, id: EntityId) {
    if let Some(set) = index.get_mut(component) {
        set.remove(&id);
    }
}

/// One entity's components, borrowed for writing from
/// [`FlintWorld::get_components_mut`](crate::FlintWorld::get_components_mut)
/// (#691).
///
/// Every way it has of adding or removing a component ([`set`](Self::set),
/// [`set_field`](Self::set_field), [`merge_component`](Self::merge_component),
/// [`merge`](Self::merge), [`remove`](Self::remove)) keeps the world's
/// component index, and [`get_mut`](Self::get_mut) hands out one component's
/// value, which can change but can't add or take away a component. Reading
/// goes through `Deref` to [`DynamicComponents`]; there is no `DerefMut`, so
/// its map can't be reached for writing, and the index can't fall out of
/// step with what an entity holds.
pub struct ComponentsMut<'a> {
    id: EntityId,
    components: &'a mut DynamicComponents,
    index: &'a mut ComponentIndex,
}

impl<'a> ComponentsMut<'a> {
    pub(crate) fn new(
        id: EntityId,
        components: &'a mut DynamicComponents,
        index: &'a mut ComponentIndex,
    ) -> Self {
        Self {
            id,
            components,
            index,
        }
    }

    /// The entity these components belong to.
    pub fn id(&self) -> EntityId {
        self.id
    }

    /// One component's value, to change in place. A component the entity
    /// doesn't have is `None`: adding one is [`set`](Self::set) or
    /// [`set_field`](Self::set_field).
    pub fn get_mut(&mut self, component: &str) -> Option<&mut toml::Value> {
        self.components.get_mut(component)
    }

    /// Set (or replace) a whole component.
    pub fn set(&mut self, component: impl Into<String>, data: toml::Value) {
        let component = component.into();
        index_add(self.index, &component, self.id);
        self.components.set(component, data);
    }

    /// Set one field of a component, making the component if it isn't there.
    pub fn set_field(&mut self, component: &str, field: &str, value: toml::Value) {
        self.components.set_field(component, field, value);
        index_add(self.index, component, self.id);
    }

    /// Merge fields into a component: see [`DynamicComponents::merge_component`].
    pub fn merge_component(&mut self, component: impl Into<String>, data: toml::Value) {
        let component = component.into();
        index_add(self.index, &component, self.id);
        self.components.merge_component(component, data);
    }

    /// Merge every component of `other` in: see [`DynamicComponents::merge`].
    pub fn merge(&mut self, other: &DynamicComponents) {
        for name in other.data.keys() {
            index_add(self.index, name, self.id);
        }
        self.components.merge(other);
    }

    /// Remove a component, returning what it held.
    pub fn remove(&mut self, component: &str) -> Option<toml::Value> {
        let removed = self.components.remove(component);
        if removed.is_some() {
            index_remove(self.index, component, self.id);
        }
        removed
    }

    /// Set or clear the entity's archetype name. It names where the
    /// defaults came from; it adds no component.
    pub fn set_archetype(&mut self, archetype: Option<String>) {
        self.components.archetype = archetype;
    }
}

impl std::ops::Deref for ComponentsMut<'_> {
    type Target = DynamicComponents;

    fn deref(&self) -> &DynamicComponents {
        self.components
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A walk of an entity's components is in name order, whatever order
    /// they were set in (#1115): the viewer's inspector walks `data` itself.
    #[test]
    fn components_walk_in_name_order() {
        let mut comps = DynamicComponents::new();
        for name in [
            "transform",
            "script",
            "light",
            "audio_source",
            "model",
            "bounds",
        ] {
            comps.set(name, toml::Value::Table(toml::map::Map::new()));
        }
        let walked: Vec<&str> = comps.data.keys().map(String::as_str).collect();
        let expected = [
            "audio_source",
            "bounds",
            "light",
            "model",
            "script",
            "transform",
        ];
        assert_eq!(walked, expected);
        assert_eq!(comps.component_names(), expected);
    }

    #[test]
    fn test_merge_component_table_into_table() {
        let mut comps = DynamicComponents::new();
        comps.set(
            "material",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("color".into(), toml::Value::String("red".into()));
                m.insert("size".into(), toml::Value::Integer(5));
                m
            }),
        );

        // Merge overrides size and adds visible
        comps.merge_component(
            "material",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("size".into(), toml::Value::Integer(10));
                m.insert("visible".into(), toml::Value::Boolean(true));
                m
            }),
        );

        let mat = comps.get("material").unwrap();
        assert_eq!(mat.get("color").and_then(|v| v.as_str()), Some("red"));
        assert_eq!(mat.get("size").and_then(|v| v.as_integer()), Some(10));
        assert_eq!(mat.get("visible").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_merge_component_into_empty() {
        let mut comps = DynamicComponents::new();
        assert!(comps.get("health").is_none());

        comps.merge_component(
            "health",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("current".into(), toml::Value::Integer(100));
                m
            }),
        );

        assert!(comps.has("health"));
        assert_eq!(
            comps
                .get("health")
                .unwrap()
                .get("current")
                .and_then(|v| v.as_integer()),
            Some(100)
        );
    }

    #[test]
    fn test_merge_non_table_replaces() {
        let mut comps = DynamicComponents::new();
        comps.set("tag", toml::Value::String("old".into()));

        // Merge a table onto a string — should replace
        comps.merge_component(
            "tag",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("name".into(), toml::Value::String("new".into()));
                m
            }),
        );

        let tag = comps.get("tag").unwrap();
        assert!(tag.is_table(), "non-table should be replaced by table");
        assert_eq!(tag.get("name").and_then(|v| v.as_str()), Some("new"));
    }

    #[test]
    fn test_merge_bulk() {
        let mut base = DynamicComponents::new();
        base.set(
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(2.0),
                        toml::Value::Float(3.0),
                    ]),
                );
                m.insert(
                    "scale".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                    ]),
                );
                m
            }),
        );

        let mut overrides = DynamicComponents::new();
        overrides.set(
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(10.0),
                        toml::Value::Float(20.0),
                        toml::Value::Float(30.0),
                    ]),
                );
                m
            }),
        );
        overrides.set(
            "health",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("current".into(), toml::Value::Integer(50));
                m
            }),
        );

        base.merge(&overrides);

        // position overridden
        let t = base.get("transform").unwrap();
        let pos = t.get("position").unwrap().as_array().unwrap();
        assert!((pos[0].as_float().unwrap() - 10.0).abs() < 0.001);

        // scale preserved
        assert!(t.get("scale").is_some());

        // new component added
        assert!(base.has("health"));
    }

    #[test]
    fn test_set_field_creates_component() {
        let mut comps = DynamicComponents::new();
        assert!(!comps.has("stats"));

        comps.set_field("stats", "strength", toml::Value::Integer(18));

        assert!(comps.has("stats"));
        assert_eq!(
            comps
                .get_field("stats", "strength")
                .and_then(|v| v.as_integer()),
            Some(18)
        );
    }
}
