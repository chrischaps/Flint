//! FlintWorld - ECS world with stable IDs and dynamic components

use crate::component::DynamicComponents;
use crate::entity::EntityInfo;
use bimap::BiMap;
use flint_core::{mat4_mul, EntityId, FlintError, Result, Transform, Vec3};
use flint_schema::SchemaRegistry;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Static empty set returned by `entities_with_component` for missing components.
static EMPTY_ENTITY_SET: LazyLock<HashSet<EntityId>> = LazyLock::new(HashSet::new);

/// The main ECS world for Flint
///
/// Wraps hecs::World with:
/// - Stable EntityId mapping
/// - Dynamic component storage
/// - Named entity lookup
pub struct FlintWorld {
    /// The underlying hecs world
    world: hecs::World,
    /// Bidirectional mapping: EntityId <-> hecs::Entity
    id_map: BiMap<EntityId, hecs::Entity>,
    /// Entity name -> EntityId mapping
    name_map: HashMap<String, EntityId>,
    /// Dynamic components for each entity
    components: HashMap<EntityId, DynamicComponents>,
    /// Parent relationships: child -> parent
    parents: HashMap<EntityId, EntityId>,
    /// Reverse index: component name -> set of entities that have it
    component_index: HashMap<String, HashSet<EntityId>>,
}

impl Default for FlintWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl FlintWorld {
    /// Create a new empty world
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            id_map: BiMap::new(),
            name_map: HashMap::new(),
            components: HashMap::new(),
            parents: HashMap::new(),
            component_index: HashMap::new(),
        }
    }

    /// Spawn a new entity with a name
    pub fn spawn(&mut self, name: impl Into<String>) -> Result<EntityId> {
        let name = name.into();

        if self.name_map.contains_key(&name) {
            return Err(FlintError::DuplicateEntityName(name));
        }

        let entity_id = EntityId::new();
        let hecs_entity = self.world.spawn(());

        self.id_map.insert(entity_id, hecs_entity);
        self.name_map.insert(name, entity_id);
        self.components.insert(entity_id, DynamicComponents::new());

        Ok(entity_id)
    }

    /// Spawn an entity with a specific ID (for loading scenes)
    pub fn spawn_with_id(&mut self, id: EntityId, name: impl Into<String>) -> Result<()> {
        let name = name.into();

        if self.name_map.contains_key(&name) {
            return Err(FlintError::DuplicateEntityName(name));
        }

        // Ensure the ID counter stays ahead
        EntityId::ensure_counter_above(id.raw());

        let hecs_entity = self.world.spawn(());

        self.id_map.insert(id, hecs_entity);
        self.name_map.insert(name, id);
        self.components.insert(id, DynamicComponents::new());

        Ok(())
    }

    /// Spawn an entity from an archetype
    pub fn spawn_archetype(
        &mut self,
        name: impl Into<String>,
        archetype: &str,
        registry: &SchemaRegistry,
    ) -> Result<EntityId> {
        let name = name.into();
        let id = self.spawn(name)?;

        // Get archetype schema
        let arch_schema = registry
            .get_archetype(archetype)
            .ok_or_else(|| FlintError::ArchetypeNotFound(archetype.to_string()))?;

        // Set archetype and apply defaults
        let components = self.components.get_mut(&id).unwrap();
        components.archetype = Some(archetype.to_string());

        // Apply archetype defaults
        for (comp_name, defaults) in &arch_schema.defaults {
            components.set(comp_name.clone(), defaults.clone());
            self.component_index
                .entry(comp_name.clone())
                .or_default()
                .insert(id);
        }

        Ok(id)
    }

    /// Despawn an entity
    pub fn despawn(&mut self, id: EntityId) -> Result<()> {
        let hecs_entity = self
            .id_map
            .get_by_left(&id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))?;

        // Remove from name map
        self.name_map.retain(|_, v| *v != id);

        // Remove from world
        self.world
            .despawn(*hecs_entity)
            .map_err(|_| FlintError::EntityNotFound(id.to_string()))?;

        self.id_map.remove_by_left(&id);
        // Remove entity from component index before dropping components
        if let Some(comps) = self.components.get(&id) {
            for comp_name in comps.component_names() {
                if let Some(set) = self.component_index.get_mut(comp_name) {
                    set.remove(&id);
                }
            }
        }
        self.components.remove(&id);
        self.parents.remove(&id);

        // Remove as parent from any children
        self.parents.retain(|_, parent| *parent != id);

        Ok(())
    }

    /// Despawn an entity by name
    pub fn despawn_by_name(&mut self, name: &str) -> Result<()> {
        let id = self
            .name_map
            .get(name)
            .copied()
            .ok_or_else(|| FlintError::EntityNotFound(name.to_string()))?;

        self.despawn(id)
    }

    /// Get entity ID by name
    pub fn get_id(&self, name: &str) -> Option<EntityId> {
        self.name_map.get(name).copied()
    }

    /// Get entity name by ID
    pub fn get_name(&self, id: EntityId) -> Option<&str> {
        self.name_map
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.as_str())
    }

    /// Get components for an entity
    pub fn get_components(&self, id: EntityId) -> Option<&DynamicComponents> {
        self.components.get(&id)
    }

    /// Get mutable components for an entity.
    ///
    /// **Note:** Callers must not add or remove component *keys* through this
    /// handle — doing so bypasses the component index. Use `set_component()`,
    /// `set_field()`, or `remove_component()` instead.
    pub fn get_components_mut(&mut self, id: EntityId) -> Option<&mut DynamicComponents> {
        self.components.get_mut(&id)
    }

    /// Set a component on an entity
    pub fn set_component(
        &mut self,
        id: EntityId,
        component: &str,
        data: toml::Value,
    ) -> Result<()> {
        let components = self
            .components
            .get_mut(&id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))?;

        components.set(component, data);
        self.component_index
            .entry(component.to_string())
            .or_default()
            .insert(id);
        Ok(())
    }

    /// Merge fields into an existing component on an entity
    ///
    /// If the component already exists (e.g. from archetype defaults),
    /// individual fields from `data` are merged in rather than replacing
    /// the entire component. Entity-level fields win over defaults.
    pub fn merge_component(
        &mut self,
        id: EntityId,
        component: &str,
        data: toml::Value,
    ) -> Result<()> {
        let components = self
            .components
            .get_mut(&id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))?;

        components.merge_component(component, data);
        self.component_index
            .entry(component.to_string())
            .or_default()
            .insert(id);
        Ok(())
    }

    /// Get a component from an entity
    pub fn get_component(&self, id: EntityId, component: &str) -> Option<&toml::Value> {
        self.components.get(&id).and_then(|c| c.get(component))
    }

    /// Set parent relationship
    pub fn set_parent(&mut self, child: EntityId, parent: EntityId) -> Result<()> {
        if !self.id_map.contains_left(&child) {
            return Err(FlintError::EntityNotFound(child.to_string()));
        }
        if !self.id_map.contains_left(&parent) {
            return Err(FlintError::EntityNotFound(parent.to_string()));
        }

        self.parents.insert(child, parent);
        Ok(())
    }

    /// Set parent by name
    pub fn set_parent_by_name(&mut self, child: &str, parent: &str) -> Result<()> {
        let child_id = self
            .get_id(child)
            .ok_or_else(|| FlintError::EntityNotFound(child.to_string()))?;
        let parent_id = self
            .get_id(parent)
            .ok_or_else(|| FlintError::EntityNotFound(parent.to_string()))?;

        self.set_parent(child_id, parent_id)
    }

    /// Get parent of an entity
    pub fn get_parent(&self, child: EntityId) -> Option<EntityId> {
        self.parents.get(&child).copied()
    }

    /// Get children of an entity
    pub fn get_children(&self, parent: EntityId) -> Vec<EntityId> {
        self.parents
            .iter()
            .filter(|(_, p)| **p == parent)
            .map(|(c, _)| *c)
            .collect()
    }

    /// Get info about all entities
    pub fn all_entities(&self) -> Vec<EntityInfo> {
        self.name_map
            .iter()
            .map(|(name, id)| {
                let components = self.components.get(id);
                let archetype = components.and_then(|c| c.archetype.clone());
                let comp_names = components
                    .map(|c| c.component_names().into_iter().map(String::from).collect())
                    .unwrap_or_default();

                let parent = self
                    .parents
                    .get(id)
                    .and_then(|pid| self.get_name(*pid).map(String::from));

                EntityInfo {
                    id: *id,
                    name: name.clone(),
                    archetype,
                    parent,
                    components: comp_names,
                }
            })
            .collect()
    }

    /// Get number of entities
    pub fn entity_count(&self) -> usize {
        self.name_map.len()
    }

    /// Check if an entity exists
    pub fn contains(&self, id: EntityId) -> bool {
        self.id_map.contains_left(&id)
    }

    /// Check if an entity with name exists
    pub fn contains_name(&self, name: &str) -> bool {
        self.name_map.contains_key(name)
    }

    /// Clear the world
    pub fn clear(&mut self) {
        self.world.clear();
        self.id_map.clear();
        self.name_map.clear();
        self.components.clear();
        self.parents.clear();
        self.component_index.clear();
    }

    /// Get transform from an entity's components
    pub fn get_transform(&self, id: EntityId) -> Option<Transform> {
        let components = self.components.get(&id)?;
        let transform_data = components.get("transform")?;

        // Parse transform from TOML value
        let pos = transform_data
            .get("position")
            .and_then(|v| parse_vec3(v))
            .unwrap_or(Vec3::ZERO);
        let rot = transform_data
            .get("rotation")
            .and_then(|v| parse_vec3(v))
            .unwrap_or(Vec3::ZERO);
        let scale = transform_data
            .get("scale")
            .and_then(|v| parse_vec3(v))
            .unwrap_or(Vec3::ONE);
        let rotation_quat = transform_data
            .get("rotation_quat")
            .and_then(|v| parse_quat(v));

        Some(Transform {
            position: pos,
            rotation: rot,
            scale,
            rotation_quat,
        })
    }

    /// Get the world-space transform matrix for an entity, walking the parent chain
    pub fn get_world_matrix(&self, id: EntityId) -> Option<[[f32; 4]; 4]> {
        let local = self.get_transform(id)?;
        match self.parents.get(&id) {
            Some(parent_id) => {
                let parent_mat = self.get_world_matrix(*parent_id)?;
                Some(mat4_mul(&parent_mat, &local.to_matrix()))
            }
            None => Some(local.to_matrix()),
        }
    }

    /// Get the world-space position for an entity (extracts translation from world matrix)
    pub fn get_world_position(&self, id: EntityId) -> Option<Vec3> {
        let mat = self.get_world_matrix(id)?;
        Some(Vec3::new(mat[3][0], mat[3][1], mat[3][2]))
    }

    /// Get all entity IDs that have a given component. O(1) lookup.
    ///
    /// Returns a reference to the internal `HashSet`; returns an empty set
    /// for components that no entity currently has.
    pub fn entities_with_component(&self, component: &str) -> &HashSet<EntityId> {
        self.component_index
            .get(component)
            .unwrap_or(&EMPTY_ENTITY_SET)
    }

    /// Get entity IDs that have *all* of the given components (intersection).
    pub fn entities_with_components(&self, names: &[&str]) -> Vec<EntityId> {
        if names.is_empty() {
            return Vec::new();
        }
        // Start from the smallest set for efficiency
        let sets: Vec<&HashSet<EntityId>> = names
            .iter()
            .map(|n| self.entities_with_component(n))
            .collect();
        let smallest = sets.iter().min_by_key(|s| s.len()).unwrap();
        smallest
            .iter()
            .copied()
            .filter(|id| sets.iter().all(|set| set.contains(id)))
            .collect()
    }

    /// Rebuild the component index from scratch. Safety net after bulk
    /// operations that may have bypassed the index (e.g. scene loading).
    pub fn rebuild_component_index(&mut self) {
        self.component_index.clear();
        for (&id, comps) in &self.components {
            for comp_name in comps.component_names() {
                self.component_index
                    .entry(comp_name.to_string())
                    .or_default()
                    .insert(id);
            }
        }
    }

    /// Set a single field within a component, creating the component if needed.
    /// Maintains the component index.
    pub fn set_field(
        &mut self,
        id: EntityId,
        component: &str,
        field: &str,
        value: toml::Value,
    ) -> Result<()> {
        let components = self
            .components
            .get_mut(&id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))?;

        components.set_field(component, field, value);
        self.component_index
            .entry(component.to_string())
            .or_default()
            .insert(id);
        Ok(())
    }

    /// Remove a component from an entity. Maintains the component index.
    /// Returns the removed component data, or `None` if it didn't exist.
    pub fn remove_component(
        &mut self,
        id: EntityId,
        component: &str,
    ) -> Result<Option<toml::Value>> {
        let components = self
            .components
            .get_mut(&id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))?;

        let removed = components.remove(component);
        if removed.is_some() {
            if let Some(set) = self.component_index.get_mut(component) {
                set.remove(&id);
            }
        }
        Ok(removed)
    }

    /// Iterate over entity names
    pub fn entity_names(&self) -> impl Iterator<Item = &str> {
        self.name_map.keys().map(|s| s.as_str())
    }
}

fn parse_quat(value: &toml::Value) -> Option<[f32; 4]> {
    if let Some(arr) = value.as_array() {
        if arr.len() >= 4 {
            let x = arr[0]
                .as_float()
                .or_else(|| arr[0].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            let y = arr[1]
                .as_float()
                .or_else(|| arr[1].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            let z = arr[2]
                .as_float()
                .or_else(|| arr[2].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            let w = arr[3]
                .as_float()
                .or_else(|| arr[3].as_integer().map(|i| i as f64))
                .unwrap_or(1.0) as f32;
            return Some([x, y, z, w]);
        }
    }
    None
}

fn parse_vec3(value: &toml::Value) -> Option<Vec3> {
    if let Some(table) = value.as_table() {
        let x = table.get("x").and_then(|v| v.as_float()).unwrap_or(0.0) as f32;
        let y = table.get("y").and_then(|v| v.as_float()).unwrap_or(0.0) as f32;
        let z = table.get("z").and_then(|v| v.as_float()).unwrap_or(0.0) as f32;
        return Some(Vec3::new(x, y, z));
    }

    if let Some(arr) = value.as_array() {
        if arr.len() >= 3 {
            let x = arr[0]
                .as_float()
                .or_else(|| arr[0].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            let y = arr[1]
                .as_float()
                .or_else(|| arr[1].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            let z = arr[2]
                .as_float()
                .or_else(|| arr[2].as_integer().map(|i| i as f64))
                .unwrap_or(0.0) as f32;
            return Some(Vec3::new(x, y, z));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_and_get() {
        let mut world = FlintWorld::new();
        let id = world.spawn("test_entity").unwrap();

        assert!(world.contains(id));
        assert!(world.contains_name("test_entity"));
        assert_eq!(world.get_id("test_entity"), Some(id));
        assert_eq!(world.get_name(id), Some("test_entity"));
    }

    #[test]
    fn test_despawn() {
        let mut world = FlintWorld::new();
        let id = world.spawn("test_entity").unwrap();

        world.despawn(id).unwrap();

        assert!(!world.contains(id));
        assert!(!world.contains_name("test_entity"));
    }

    #[test]
    fn test_duplicate_name() {
        let mut world = FlintWorld::new();
        world.spawn("test").unwrap();

        assert!(matches!(
            world.spawn("test"),
            Err(FlintError::DuplicateEntityName(_))
        ));
    }

    #[test]
    fn test_parent_child() {
        let mut world = FlintWorld::new();
        let parent = world.spawn("parent").unwrap();
        let child = world.spawn("child").unwrap();

        world.set_parent(child, parent).unwrap();

        assert_eq!(world.get_parent(child), Some(parent));
        assert!(world.get_children(parent).contains(&child));
    }

    #[test]
    fn test_components() {
        let mut world = FlintWorld::new();
        let id = world.spawn("entity").unwrap();

        let data = toml::toml! {
            locked = false
            style = "hinged"
        };

        world
            .set_component(id, "door", toml::Value::Table(data))
            .unwrap();

        let comp = world.get_component(id, "door").unwrap();
        assert_eq!(comp.get("locked").and_then(|v| v.as_bool()), Some(false));
    }

    // ── Component index tests ───────────────────────────────

    #[test]
    fn test_index_on_set_component() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        assert!(world.entities_with_component("door").is_empty());

        world
            .set_component(id, "door", toml::Value::Table(Default::default()))
            .unwrap();

        assert!(world.entities_with_component("door").contains(&id));
    }

    #[test]
    fn test_index_on_merge_component() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        world
            .merge_component(id, "material", toml::Value::Table(Default::default()))
            .unwrap();

        assert!(world.entities_with_component("material").contains(&id));
    }

    #[test]
    fn test_index_on_despawn() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();
        world
            .set_component(id, "light", toml::Value::Table(Default::default()))
            .unwrap();
        assert_eq!(world.entities_with_component("light").len(), 1);

        world.despawn(id).unwrap();
        assert!(world.entities_with_component("light").is_empty());
    }

    #[test]
    fn test_index_on_clear() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();
        world
            .set_component(id, "script", toml::Value::Table(Default::default()))
            .unwrap();
        assert!(!world.entities_with_component("script").is_empty());

        world.clear();
        assert!(world.entities_with_component("script").is_empty());
    }

    #[test]
    fn test_entities_with_components_intersection() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();

        world
            .set_component(a, "animator", toml::Value::Table(Default::default()))
            .unwrap();
        world
            .set_component(a, "skeleton", toml::Value::Table(Default::default()))
            .unwrap();
        world
            .set_component(b, "animator", toml::Value::Table(Default::default()))
            .unwrap();

        let both = world.entities_with_components(&["animator", "skeleton"]);
        assert_eq!(both.len(), 1);
        assert!(both.contains(&a));
        assert!(!both.contains(&b));
    }

    #[test]
    fn test_rebuild_component_index() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();
        world
            .set_component(id, "rigidbody", toml::Value::Table(Default::default()))
            .unwrap();

        // Corrupt the index
        world.component_index.clear();
        assert!(world.entities_with_component("rigidbody").is_empty());

        world.rebuild_component_index();
        assert!(world.entities_with_component("rigidbody").contains(&id));
    }

    #[test]
    fn test_set_field_creates_component_in_index() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        world
            .set_field(id, "health", "current", toml::Value::Integer(100))
            .unwrap();

        assert!(world.entities_with_component("health").contains(&id));
    }

    #[test]
    fn test_remove_component_updates_index() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();
        world
            .set_component(id, "collider", toml::Value::Table(Default::default()))
            .unwrap();
        assert!(world.entities_with_component("collider").contains(&id));

        let removed = world.remove_component(id, "collider").unwrap();
        assert!(removed.is_some());
        assert!(!world.entities_with_component("collider").contains(&id));
    }
}
