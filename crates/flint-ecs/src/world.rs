//! FlintWorld - ECS world with stable IDs and dynamic components

use crate::component::{index_remove, ComponentIndex, ComponentsMut, DynamicComponents};
use crate::entity::EntityInfo;
use crate::view::ReadReports;
use flint_core::toml_util::{toml_to_vec3, toml_vec4};
use flint_core::{mat4_mul, EntityId, FlintError, Result, Transform, Vec3};
use flint_schema::SchemaRegistry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

/// Where every [`FlintWorld::write_stamp`] comes from: one counter for the
/// process, so no two writes anywhere share a stamp, and an entity of a
/// world made again (a reload) never reads as one a system already saw.
static NEXT_WRITE_STAMP: AtomicU64 = AtomicU64::new(1);

fn next_write_stamp() -> u64 {
    NEXT_WRITE_STAMP.fetch_add(1, Ordering::Relaxed)
}

/// Static empty set returned by `entities_with_component` for missing components.
static EMPTY_ENTITY_SET: BTreeSet<EntityId> = BTreeSet::new();

/// One entity: its name and what it holds.
#[derive(Debug, Clone)]
struct EntityRecord {
    name: String,
    components: DynamicComponents,
    /// Bumped each time `components` is lent out to write, or the parent
    /// link changes: [`FlintWorld::write_stamp`] (#1221)
    writes: u64,
}

/// The main ECS world for Flint
///
/// Its own storage, keyed by stable [`EntityId`] (#691; it once wrapped a
/// `hecs::World` that stored nothing): one record per entity (its name and
/// its dynamic components), a name -> id map, the parent links both ways,
/// and an index from each component name to the entities that hold it.
/// Every lookup is a map access: [`get_name`](Self::get_name) and
/// [`get_id`](Self::get_id) are O(1) and O(log n),
/// [`children`](Self::children) is O(1) and hands out the list it keeps.
///
/// **The index can't be bypassed** (#691): every write goes through the
/// world or the [`ComponentsMut`] guard [`get_components_mut`](Self::get_components_mut)
/// returns, each of which keeps it.
///
/// **Despawn takes the subtree** (#691): [`despawn`](Self::despawn) removes
/// the entity and every descendant. A child's transform is in its parent's
/// frame, so a child left behind as a root would jump to wherever its local
/// numbers put it in the world; to keep one, reparent it first.
///
/// **Order** (#688): a world hands out its own ids, counting up from 1 (or
/// from past the world it follows, [`FlintWorld::after`]), and every walk it
/// offers is ordered: [`all_entities`](Self::all_entities),
/// [`entity_names`](Self::entity_names) and
/// [`get_children`](Self::get_children) (#689) by name,
/// [`entities_with_component`](Self::entities_with_component) by id. A scene spawns its entities
/// in name order, so loading the same scene gives the same ids every time,
/// in this process or the next, and what iterates a component (physics
/// creating bodies, scripts, the renderer) meets entities in the same order
/// every run.
pub struct FlintWorld {
    /// Every entity: its name and components, by id. An id is live while
    /// it is a key here
    entities: HashMap<EntityId, EntityRecord>,
    /// Entity name -> EntityId mapping, by name (#688)
    name_map: BTreeMap<String, EntityId>,
    /// Parent relationships: child -> parent
    parents: HashMap<EntityId, EntityId>,
    /// The other way: parent -> its children, in name order (#689)
    children: HashMap<EntityId, Vec<EntityId>>,
    /// Reverse index: component name -> the entities that have it, by id (#688)
    component_index: ComponentIndex,
    /// The id the next spawn gets. It only rises: [`FlintWorld::clear`]
    /// keeps it, so a scene reloaded into the same world never reuses an id
    next_id: u64,
    /// What the typed readers ([`FlintWorld::view`]) have reported, once each
    read_reports: ReadReports,
}

impl Default for FlintWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl FlintWorld {
    /// Create a new empty world. Its first entity is id 1.
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            name_map: BTreeMap::new(),
            parents: HashMap::new(),
            children: HashMap::new(),
            component_index: HashMap::new(),
            next_id: 1,
            read_reports: ReadReports::new(),
        }
    }

    /// An empty world whose ids start past every id `previous` gave out:
    /// what replaces a world in a host that goes on (a scene transition, an
    /// editor's reload), so an id held across the change (a script's
    /// persistent value, an event, a renderer's cache keyed by entity)
    /// can never name an entity of the new world.
    pub fn after(previous: &FlintWorld) -> Self {
        Self {
            next_id: previous.next_id,
            ..Self::new()
        }
    }

    /// The id the next [`spawn`](Self::spawn) will give.
    pub fn next_id(&self) -> EntityId {
        EntityId::from_raw(self.next_id)
    }

    /// What the typed readers have found wrong so far: a wrong type or an
    /// unknown choice, each (entity, component, field) once (ADR 0115).
    pub fn read_reports(&self) -> &ReadReports {
        &self.read_reports
    }

    /// Spawn a new entity with a name
    pub fn spawn(&mut self, name: impl Into<String>) -> Result<EntityId> {
        let entity_id = EntityId::from_raw(self.next_id);
        self.insert_entity(entity_id, name.into())?;
        Ok(entity_id)
    }

    /// Spawn an entity with a specific ID (for loading scenes). Refused when
    /// the name is taken or the id is live.
    pub fn spawn_with_id(&mut self, id: EntityId, name: impl Into<String>) -> Result<()> {
        self.insert_entity(id, name.into())
    }

    /// Make the record for a new entity, keeping the counter past its id.
    fn insert_entity(&mut self, id: EntityId, name: String) -> Result<()> {
        if self.name_map.contains_key(&name) {
            return Err(FlintError::DuplicateEntityName(name));
        }
        if let Some(live) = self.entities.get(&id) {
            return Err(FlintError::SceneError(format!(
                "entity id {id} is already '{}'",
                live.name
            )));
        }
        self.next_id = self.next_id.max(id.raw() + 1);
        self.name_map.insert(name.clone(), id);
        self.entities.insert(
            id,
            EntityRecord {
                name,
                components: DynamicComponents::new(),
                writes: next_write_stamp(),
            },
        );
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
        let mut components = self.get_components_mut(id).expect("just spawned");
        components.set_archetype(Some(archetype.to_string()));
        for (comp_name, defaults) in &arch_schema.defaults {
            components.set(comp_name.as_str(), defaults.clone());
        }

        Ok(id)
    }

    /// Despawn an entity and its whole subtree (#691).
    ///
    /// Returns every id removed: the entity's first, then its descendants a
    /// generation at a time (a parent before its children, siblings in name
    /// order): what a host that keeps
    /// its own state per entity (physics bodies, script instances) has to
    /// let go of. A child's transform is in its parent's frame, so leaving
    /// it behind as a root would move it; to keep a child, give it another
    /// parent before despawning this one.
    pub fn despawn(&mut self, id: EntityId) -> Result<Vec<EntityId>> {
        if !self.entities.contains_key(&id) {
            return Err(FlintError::EntityNotFound(id.to_string()));
        }
        // The subtree, parent before child. Out of the parent's list first,
        // so the walk below meets only the subtree
        self.unlink_from_parent(id);
        let mut gone = vec![id];
        let mut at = 0;
        while at < gone.len() {
            if let Some(kids) = self.children.remove(&gone[at]) {
                gone.extend(kids);
            }
            at += 1;
        }
        for &entity in &gone {
            self.parents.remove(&entity);
            if let Some(record) = self.entities.remove(&entity) {
                self.name_map.remove(&record.name);
                for comp_name in record.components.data.keys() {
                    index_remove(&mut self.component_index, comp_name, entity);
                }
            }
        }
        Ok(gone)
    }

    /// Despawn an entity and its subtree by name: see [`Self::despawn`].
    pub fn despawn_by_name(&mut self, name: &str) -> Result<Vec<EntityId>> {
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
        self.entities.get(&id).map(|e| e.name.as_str())
    }

    /// These entities in name order (#689): the order every list a script
    /// is handed comes in, so it is the same however and in whatever order
    /// the world was built. Names are unique; an id that names nothing (a
    /// despawned entity) goes last, by id.
    pub fn in_name_order(&self, ids: impl IntoIterator<Item = EntityId>) -> Vec<EntityId> {
        let mut ids: Vec<EntityId> = ids.into_iter().collect();
        ids.sort_unstable_by(|a, b| self.name_order(*a, *b));
        ids
    }

    /// Name order between two entities: by name, then by id, an entity with
    /// no name after every named one.
    fn name_order(&self, a: EntityId, b: EntityId) -> std::cmp::Ordering {
        let name = |id| self.entities.get(&id).map(|e: &EntityRecord| &e.name);
        match (name(a), name(b)) {
            (Some(x), Some(y)) => x.cmp(y).then(a.cmp(&b)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(&b),
        }
    }

    /// Get components for an entity
    pub fn get_components(&self, id: EntityId) -> Option<&DynamicComponents> {
        self.entities.get(&id).map(|e| &e.components)
    }

    /// An entity's components, to change: a [`ComponentsMut`], whose every
    /// way of adding or removing a component keeps the component index
    /// (#691). `None` for an id that names no live entity.
    pub fn get_components_mut(&mut self, id: EntityId) -> Option<ComponentsMut<'_>> {
        let record = self.entities.get_mut(&id)?;
        record.writes = next_write_stamp();
        Some(ComponentsMut::new(
            id,
            &mut record.components,
            &mut self.component_index,
        ))
    }

    /// One component's value, to change in place: `None` when the entity
    /// doesn't have it. Like [`ComponentsMut::get_mut`], it can't add or
    /// take away a component.
    pub fn get_component_mut(&mut self, id: EntityId, component: &str) -> Option<&mut toml::Value> {
        let record = self.entities.get_mut(&id)?;
        record.writes = next_write_stamp();
        record.components.get_mut(component)
    }

    /// A stamp, new to the process, taken each time the entity's
    /// components are lent out to write ([`Self::get_components_mut`],
    /// [`Self::get_component_mut`] and every setter, which go through
    /// them) or its parent changes. Two equal readings mean nothing of the
    /// entity changed in between, so a system that keeps what it read from
    /// an entity can skip reading it again (physics' colliders, #1221). A
    /// write lent out and not used still moves it. `None` for an id that
    /// names no live entity.
    pub fn write_stamp(&self, id: EntityId) -> Option<u64> {
        self.entities.get(&id).map(|e| e.writes)
    }

    /// Move a child's stamp: its parent link changed, and with it its
    /// world matrix.
    fn touch(&mut self, id: EntityId) {
        if let Some(record) = self.entities.get_mut(&id) {
            record.writes = next_write_stamp();
        }
    }

    /// [`Self::get_components_mut`], or the error a setter returns for an
    /// id that names nothing.
    fn components_mut(&mut self, id: EntityId) -> Result<ComponentsMut<'_>> {
        self.get_components_mut(id)
            .ok_or_else(|| FlintError::EntityNotFound(id.to_string()))
    }

    /// Set a component on an entity
    pub fn set_component(
        &mut self,
        id: EntityId,
        component: &str,
        data: toml::Value,
    ) -> Result<()> {
        self.components_mut(id)?.set(component, data);
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
        self.components_mut(id)?.merge_component(component, data);
        Ok(())
    }

    /// Get a component from an entity
    pub fn get_component(&self, id: EntityId, component: &str) -> Option<&toml::Value> {
        self.get_components(id).and_then(|c| c.get(component))
    }

    /// Set parent relationship.
    ///
    /// Refused with [`FlintError::ParentCycle`] when `parent` is `child`
    /// itself or one of its descendants (#680), and the hierarchy is left as
    /// it was. That refusal is what keeps every parent chain finite, which
    /// every walk up a chain (the matrices, visibility, the renderer's and
    /// physics' own) counts on: this is the only place a parent is written.
    pub fn set_parent(&mut self, child: EntityId, parent: EntityId) -> Result<()> {
        if !self.contains(child) {
            return Err(FlintError::EntityNotFound(child.to_string()));
        }
        if !self.contains(parent) {
            return Err(FlintError::EntityNotFound(parent.to_string()));
        }

        // Walk up from the new parent: reaching the child means the child
        // would become its own ancestor. The chains already here are finite
        // (this check held when each link was made), so the walk ends
        let mut at = parent;
        while at != child {
            match self.parents.get(&at) {
                Some(&up) => at = up,
                None => {
                    self.link(child, parent);
                    return Ok(());
                }
            }
        }

        // Refused: the same walk again, by name, for the message
        let name = |id: EntityId| {
            self.get_name(id)
                .map(String::from)
                .unwrap_or_else(|| id.to_string())
        };
        let mut chain = vec![name(parent)];
        let mut at = parent;
        while at != child {
            at = self.parents[&at];
            chain.push(name(at));
        }
        Err(FlintError::ParentCycle {
            child: name(child),
            parent: name(parent),
            chain,
        })
    }

    /// Make `parent` the parent of `child`, which [`Self::set_parent`] has
    /// checked makes no cycle: out of the old parent's children list, into
    /// the new one's at its place by name (#689).
    fn link(&mut self, child: EntityId, parent: EntityId) {
        if self.parents.get(&child) == Some(&parent) {
            return;
        }
        self.unlink_from_parent(child);
        self.parents.insert(child, parent);
        self.touch(child);
        let at = {
            let siblings = self.children.get(&parent).map_or(&[][..], Vec::as_slice);
            siblings.partition_point(|s| self.name_order(*s, child).is_lt())
        };
        self.children.entry(parent).or_default().insert(at, child);
    }

    /// Take `child` out of its parent's children list, and forget the parent.
    fn unlink_from_parent(&mut self, child: EntityId) {
        let Some(parent) = self.parents.remove(&child) else {
            return;
        };
        self.touch(child);
        if let Some(siblings) = self.children.get_mut(&parent) {
            siblings.retain(|s| *s != child);
            if siblings.is_empty() {
                self.children.remove(&parent);
            }
        }
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

    /// The entity's children, in name order (#689). A lookup in the
    /// children index, not a scan of every entity; [`Self::children`] is
    /// the same list without the copy.
    pub fn get_children(&self, parent: EntityId) -> Vec<EntityId> {
        self.children(parent).to_vec()
    }

    /// The entity's children, in name order: the list the world keeps,
    /// borrowed (#691). Empty for a leaf or an id that names nothing.
    pub fn children(&self, parent: EntityId) -> &[EntityId] {
        self.children.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// Get info about all entities, by name.
    ///
    /// The renderer draws in this order: where two surfaces tie in depth,
    /// the pixels on their edges flipped between runs of the same capture
    /// while the name map was a `HashMap`. It is ordered by name now
    /// (#688), as every walk of the world is.
    pub fn all_entities(&self) -> Vec<EntityInfo> {
        self.name_map
            .iter()
            .map(|(name, id)| {
                let components = self.get_components(*id);
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
        self.entities.contains_key(&id)
    }

    /// Check if an entity with name exists
    pub fn contains_name(&self, name: &str) -> bool {
        self.name_map.contains_key(name)
    }

    /// Clear the world. The id counter is kept: what spawns next gets an
    /// id this world never gave out.
    pub fn clear(&mut self) {
        self.entities.clear();
        self.name_map.clear();
        self.parents.clear();
        self.children.clear();
        self.component_index.clear();
        self.read_reports.clear();
    }

    /// Get transform from an entity's components
    pub fn get_transform(&self, id: EntityId) -> Option<Transform> {
        let components = self.get_components(id)?;
        let transform_data = components.get("transform")?;

        // Parse transform from TOML value
        let pos = transform_data
            .get("position")
            .and_then(toml_to_vec3)
            .unwrap_or(Vec3::ZERO);
        let rot = transform_data
            .get("rotation")
            .and_then(toml_to_vec3)
            .unwrap_or(Vec3::ZERO);
        let scale = transform_data
            .get("scale")
            .and_then(toml_to_vec3)
            .unwrap_or(Vec3::ONE);
        let rotation_quat = transform_data.get("rotation_quat").and_then(toml_vec4);
        let shear = transform_data
            .get("shear")
            .and_then(flint_core::Shear::from_toml);

        Some(Transform {
            position: pos,
            rotation: rot,
            scale,
            rotation_quat,
            shear,
        })
    }

    /// Get the world-space transform matrix for an entity, walking the parent chain.
    /// `None` when the entity has no transform of its own. An ancestor without
    /// one is a bare grouping node: it moves nothing, and the chain goes on
    /// through it to the ancestors above (see [`Self::get_frame_matrix`]).
    pub fn get_world_matrix(&self, id: EntityId) -> Option<[[f32; 4]; 4]> {
        let local = self.get_transform(id)?.to_matrix();
        Some(self.in_parents_frame(id, local))
    }

    /// The frame an entity's children are placed in: its world matrix, with a
    /// missing transform anywhere up the chain, this entity's included, taken
    /// as identity. Unlike [`Self::get_world_matrix`] it always has an answer.
    pub fn get_frame_matrix(&self, id: EntityId) -> [[f32; 4]; 4] {
        self.in_parents_frame(id, self.local_frame(id))
    }

    /// An entity's own matrix as its children see it: identity with no
    /// transform.
    fn local_frame(&self, id: EntityId) -> [[f32; 4]; 4] {
        self.get_transform(id)
            .unwrap_or(Transform::IDENTITY)
            .to_matrix()
    }

    /// `local` (the matrix of `id`) placed in its parent's frame, walking up
    /// the chain with a loop rather than recursion (#680), so no depth of
    /// hierarchy can overflow the stack. The product is taken from the root
    /// down, `root * .. * parent * local`, in the order the recursive walk
    /// multiplied, so the matrices are bit for bit what they were.
    fn in_parents_frame(&self, id: EntityId, local: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let Some(&parent) = self.parents.get(&id) else {
            return local;
        };
        // The ancestors, nearest first: the first NEAR on the stack (deeper
        // than that is rare), the rest in a Vec
        const NEAR: usize = 16;
        let mut near = [parent; NEAR];
        let mut count = 0;
        let mut far = Vec::new();
        let mut at = Some(parent);
        while let Some(ancestor) = at {
            if count < NEAR {
                near[count] = ancestor;
            } else {
                far.push(ancestor);
            }
            count += 1;
            // A chain holds each entity once, so it can't be longer than the
            // map; `set_parent` refuses a cycle, and this cap is the second
            // line should one ever be made some other way
            if count > self.parents.len() {
                break;
            }
            at = self.parents.get(&ancestor).copied();
        }
        let mut root_down = far.iter().rev().chain(near[..count.min(NEAR)].iter().rev());
        let root = root_down.next().expect("the parent is in the chain");
        let mut frame = self.local_frame(*root);
        for ancestor in root_down {
            frame = mat4_mul(&frame, &self.local_frame(*ancestor));
        }
        mat4_mul(&frame, &local)
    }

    /// Get the world-space position for an entity (extracts translation from world matrix)
    pub fn get_world_position(&self, id: EntityId) -> Option<Vec3> {
        let mat = self.get_world_matrix(id)?;
        Some(Vec3::new(mat[3][0], mat[3][1], mat[3][2]))
    }

    /// The entity's own `visibility.visible`: false only when it says so,
    /// true with no `visibility` component (#628). An ancestor's switch isn't
    /// counted here; [`Self::hidden_entities`] is what the renderer skips.
    pub fn is_visible(&self, id: EntityId) -> bool {
        self.get_components(id)
            .and_then(|c| c.get(flint_core::components::VISIBILITY))
            .and_then(|v| v.get("visible"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }

    /// Every entity a `visibility { visible = false }` hides: those that say
    /// so and all their descendants, so hiding a model's root hides its
    /// expanded nodes too (#628). Empty, with nothing walked, while no entity
    /// is hidden, which is what keeps a scene without the switch at no cost.
    pub fn hidden_entities(&self) -> HashSet<EntityId> {
        let roots: HashSet<EntityId> = self
            .entities_with_component(flint_core::components::VISIBILITY)
            .iter()
            .copied()
            .filter(|id| !self.is_visible(*id))
            .collect();
        if roots.is_empty() {
            return roots;
        }
        let mut hidden = roots.clone();
        for &child in self.parents.keys() {
            let mut at = child;
            // Depth-capped: a parent chain never loops (`set_parent` refuses
            // a cycle, #680), but a cap costs nothing
            for _ in 0..256 {
                match self.parents.get(&at) {
                    Some(parent) if roots.contains(parent) => {
                        hidden.insert(child);
                        break;
                    }
                    Some(parent) => at = *parent,
                    None => break,
                }
            }
        }
        hidden
    }

    /// Get all entity IDs that have a given component. O(1) lookup.
    ///
    /// Returns a reference to the internal set, **in ascending id order**
    /// (#688): for a loaded scene that is name order, then the order later
    /// entities were spawned in, the same every run. An empty set for a
    /// component no entity has. A list that must be in name order whatever
    /// spawned what goes through [`Self::in_name_order`] (#689).
    pub fn entities_with_component(&self, component: &str) -> &BTreeSet<EntityId> {
        self.component_index
            .get(component)
            .unwrap_or(&EMPTY_ENTITY_SET)
    }

    /// Get entity IDs that have *all* of the given components (intersection),
    /// in ascending id order.
    pub fn entities_with_components(&self, names: &[&str]) -> Vec<EntityId> {
        if names.is_empty() {
            return Vec::new();
        }
        // Start from the smallest set for efficiency
        let sets: Vec<&BTreeSet<EntityId>> = names
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

    /// Set a single field within a component, creating the component if needed.
    /// Maintains the component index.
    pub fn set_field(
        &mut self,
        id: EntityId,
        component: &str,
        field: &str,
        value: toml::Value,
    ) -> Result<()> {
        self.components_mut(id)?.set_field(component, field, value);
        Ok(())
    }

    /// Remove a component from an entity. Maintains the component index.
    /// Returns the removed component data, or `None` if it didn't exist.
    pub fn remove_component(
        &mut self,
        id: EntityId,
        component: &str,
    ) -> Result<Option<toml::Value>> {
        Ok(self.components_mut(id)?.remove(component))
    }

    /// Iterate over entity names, in name order
    pub fn entity_names(&self) -> impl Iterator<Item = &str> {
        self.name_map.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_entities_take_descendants() {
        // #628: a hidden root hides its subtree; nothing hidden is empty
        let mut world = FlintWorld::new();
        let root = world.spawn("car").unwrap();
        let wheel = world.spawn("wheel").unwrap();
        let hub = world.spawn("hub").unwrap();
        let other = world.spawn("tree").unwrap();
        world.set_parent(wheel, root).unwrap();
        world.set_parent(hub, wheel).unwrap();
        assert!(world.hidden_entities().is_empty());

        world
            .set_field(other, "visibility", "visible", toml::Value::Boolean(true))
            .unwrap();
        assert!(
            world.hidden_entities().is_empty(),
            "visible = true hides nothing"
        );

        world
            .set_field(root, "visibility", "visible", toml::Value::Boolean(false))
            .unwrap();
        let hidden = world.hidden_entities();
        assert_eq!(hidden.len(), 3);
        assert!(hidden.contains(&root) && hidden.contains(&wheel) && hidden.contains(&hub));
        assert!(!world.is_visible(root));
        assert!(
            world.is_visible(wheel),
            "is_visible is the entity's own switch"
        );
    }

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

    fn names_of(world: &FlintWorld, ids: &[EntityId]) -> Vec<String> {
        ids.iter()
            .map(|id| world.get_name(*id).unwrap().to_string())
            .collect()
    }

    /// The children index (#689) is in name order and follows every change
    /// to the hierarchy: a reparent moves the child, despawning a child
    /// takes it out, despawning a parent takes its subtree (#691).
    #[test]
    fn children_are_in_name_order_and_follow_the_hierarchy() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();
        let kids: Vec<EntityId> = ["mid", "zed", "alpha", "kilo"]
            .iter()
            .map(|n| world.spawn(*n).unwrap())
            .collect();
        for &k in &kids {
            world.set_parent(k, a).unwrap();
        }
        assert_eq!(
            names_of(&world, &world.get_children(a)),
            ["alpha", "kilo", "mid", "zed"]
        );

        // Setting the same parent again changes nothing.
        world.set_parent(kids[0], a).unwrap();
        assert_eq!(world.get_children(a).len(), 4);

        // Reparent: out of a's list, into b's.
        world.set_parent(kids[1], b).unwrap();
        assert_eq!(
            names_of(&world, &world.get_children(a)),
            ["alpha", "kilo", "mid"]
        );
        assert_eq!(names_of(&world, &world.get_children(b)), ["zed"]);

        // Despawn a child: gone from its parent's list.
        world.despawn(kids[3]).unwrap();
        assert_eq!(names_of(&world, &world.get_children(a)), ["alpha", "mid"]);
        assert_eq!(world.get_name(kids[3]), None);

        // Despawn the parent: its children go with it, the reparented one
        // stays.
        assert_eq!(world.despawn(a).unwrap(), [a, kids[2], kids[0]]);
        assert!(!world.contains(kids[0]) && !world.contains(kids[2]));
        assert_eq!(world.get_parent(kids[1]), Some(b));
        assert!(world.get_children(a).is_empty());

        world.clear();
        assert!(world.get_children(b).is_empty());
    }

    #[test]
    fn in_name_order_sorts_by_name_and_puts_the_unnamed_last() {
        let mut world = FlintWorld::new();
        let z = world.spawn("z").unwrap();
        let m = world.spawn("m").unwrap();
        let gone = world.spawn("a").unwrap();
        world.despawn(gone).unwrap();
        assert_eq!(world.in_name_order([gone, z, m]), [m, z, gone]);
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

    // ── merge_component tests (previously buggy code path) ─────────

    #[test]
    fn test_merge_component_preserves_existing_fields() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        // Set initial component with two fields
        world
            .set_component(
                id,
                "material",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("color".into(), toml::Value::String("red".into()));
                    m.insert("size".into(), toml::Value::Integer(5));
                    m
                }),
            )
            .unwrap();

        // Merge only overrides size — color must survive
        world
            .merge_component(
                id,
                "material",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("size".into(), toml::Value::Integer(10));
                    m
                }),
            )
            .unwrap();

        let comp = world.get_component(id, "material").unwrap();
        assert_eq!(
            comp.get("color").and_then(|v| v.as_str()),
            Some("red"),
            "existing field 'color' should be preserved"
        );
        assert_eq!(
            comp.get("size").and_then(|v| v.as_integer()),
            Some(10),
            "overridden field 'size' should be updated"
        );
    }

    #[test]
    fn test_merge_component_creates_new_component() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        // Entity has no "health" component yet
        assert!(world.get_component(id, "health").is_none());

        world
            .merge_component(
                id,
                "health",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("current".into(), toml::Value::Integer(100));
                    m
                }),
            )
            .unwrap();

        let comp = world.get_component(id, "health").unwrap();
        assert_eq!(comp.get("current").and_then(|v| v.as_integer()), Some(100));
    }

    #[test]
    fn test_merge_component_replaces_non_table() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        // Set component as a plain string
        world
            .set_component(id, "tag", toml::Value::String("old".into()))
            .unwrap();

        // Merge with another string — should replace
        world
            .merge_component(id, "tag", toml::Value::String("new".into()))
            .unwrap();

        let comp = world.get_component(id, "tag").unwrap();
        assert_eq!(comp.as_str(), Some("new"));
    }

    // ── spawn_with_id tests ────────────────────────────────────────

    #[test]
    fn test_spawn_with_id_basic() {
        let mut world = FlintWorld::new();
        let id = EntityId::from_raw(42);
        world.spawn_with_id(id, "explicit").unwrap();

        assert!(world.contains(id));
        assert_eq!(world.get_name(id), Some("explicit"));
        assert_eq!(world.get_id("explicit"), Some(id));
    }

    #[test]
    fn test_spawn_with_id_counter_sync() {
        let mut world = FlintWorld::new();
        let high_id = EntityId::from_raw(10_000);
        world.spawn_with_id(high_id, "high").unwrap();

        // Next regular spawn must get ID > 10_000
        let next = world.spawn("after_high").unwrap();
        assert!(
            next.raw() > 10_000,
            "new id {} should be > 10000",
            next.raw()
        );
    }

    // ── the world's own ids (#688) ─────────────────────────────────

    #[test]
    fn a_world_counts_its_own_ids_from_one() {
        // Whatever other worlds in the process (other tests) have spawned
        let mut a = FlintWorld::new();
        let mut b = FlintWorld::new();
        for world in [&mut a, &mut b] {
            assert_eq!(world.spawn("first").unwrap(), EntityId::from_raw(1));
            assert_eq!(world.spawn("second").unwrap(), EntityId::from_raw(2));
        }
    }

    #[test]
    fn a_world_after_another_never_reuses_its_ids() {
        let mut old = FlintWorld::new();
        old.spawn("a").unwrap();
        old.spawn_with_id(EntityId::from_raw(40), "b").unwrap();
        let mut next = FlintWorld::after(&old);
        assert_eq!(next.next_id(), EntityId::from_raw(41));
        assert_eq!(next.spawn("a").unwrap(), EntityId::from_raw(41));
    }

    #[test]
    fn clearing_a_world_keeps_its_counter() {
        let mut world = FlintWorld::new();
        world.spawn("a").unwrap();
        world.spawn("b").unwrap();
        world.clear();
        assert_eq!(world.spawn("a").unwrap(), EntityId::from_raw(3));
    }

    #[test]
    fn entities_with_component_come_in_id_order() {
        let mut world = FlintWorld::new();
        let mut ids = Vec::new();
        for i in 0..50 {
            let id = world.spawn(format!("e{i:02}")).unwrap();
            ids.push(id);
        }
        // Tagged in a scrambled order: the set's order is the ids', not this
        for i in [37, 3, 49, 0, 22, 11, 48, 5] {
            world
                .set_component(ids[i], "tag", toml::Value::Boolean(true))
                .unwrap();
        }
        let got: Vec<EntityId> = world
            .entities_with_component("tag")
            .iter()
            .copied()
            .collect();
        let mut want = got.clone();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(got.len(), 8);
        assert_eq!(got[0], ids[0]);
    }

    #[test]
    fn children_and_names_come_in_order() {
        let mut world = FlintWorld::new();
        let root = world.spawn("root").unwrap();
        let kids: Vec<EntityId> = ["z", "m", "a"]
            .iter()
            .map(|n| world.spawn(*n).unwrap())
            .collect();
        for kid in &kids {
            world.set_parent(*kid, root).unwrap();
        }
        // Children by name (#689), whatever order they were spawned or linked
        assert_eq!(world.get_children(root), [kids[2], kids[1], kids[0]]);
        let names: Vec<&str> = world.entity_names().collect();
        assert_eq!(names, ["a", "m", "root", "z"]);
    }

    #[test]
    fn all_entities_come_by_name_whatever_the_spawn_order() {
        let mut world = FlintWorld::new();
        for name in ["walls", "floor", "roof", "door", "chimney"] {
            world.spawn(name).unwrap();
        }
        let names: Vec<String> = world.all_entities().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["chimney", "door", "floor", "roof", "walls"]);
    }

    #[test]
    fn test_spawn_with_id_duplicate_name_error() {
        let mut world = FlintWorld::new();
        let id1 = EntityId::from_raw(100);
        let id2 = EntityId::from_raw(101);

        world.spawn_with_id(id1, "same_name").unwrap();
        let result = world.spawn_with_id(id2, "same_name");

        assert!(matches!(result, Err(FlintError::DuplicateEntityName(_))));
    }

    // ── spawn_archetype tests ──────────────────────────────────────

    #[test]
    fn test_spawn_archetype_applies_defaults() {
        let mut registry = SchemaRegistry::new();
        registry
            .load_archetype_string(
                r#"
[archetype.door]
description = "A door"
components = ["transform", "door"]

[archetype.door.defaults.door]
locked = false
style = "hinged"
"#,
            )
            .unwrap();

        let mut world = FlintWorld::new();
        let id = world.spawn_archetype("my_door", "door", &registry).unwrap();

        let comp = world.get_component(id, "door").unwrap();
        assert_eq!(comp.get("locked").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(comp.get("style").and_then(|v| v.as_str()), Some("hinged"));
    }

    #[test]
    fn test_spawn_archetype_not_found() {
        let registry = SchemaRegistry::new();
        let mut world = FlintWorld::new();

        let result = world.spawn_archetype("e", "nonexistent", &registry);
        assert!(matches!(result, Err(FlintError::ArchetypeNotFound(_))));
    }

    #[test]
    fn test_spawn_archetype_updates_component_index() {
        let mut registry = SchemaRegistry::new();
        registry
            .load_archetype_string(
                r#"
[archetype.lamp]
description = "A lamp"
components = ["transform", "light"]

[archetype.lamp.defaults.light]
intensity = 1.0
"#,
            )
            .unwrap();

        let mut world = FlintWorld::new();
        let id = world.spawn_archetype("lamp1", "lamp", &registry).unwrap();

        assert!(
            world.entities_with_component("light").contains(&id),
            "component index should include archetype-default components"
        );
    }

    // ── Transform hierarchy tests ──────────────────────────────────

    #[test]
    fn test_get_world_matrix_no_parent() {
        let mut world = FlintWorld::new();
        let id = world.spawn("e").unwrap();

        world
            .set_component(
                id,
                "transform",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert(
                        "position".into(),
                        toml::Value::Array(vec![
                            toml::Value::Float(3.0),
                            toml::Value::Float(4.0),
                            toml::Value::Float(5.0),
                        ]),
                    );
                    m
                }),
            )
            .unwrap();

        let mat = world.get_world_matrix(id).unwrap();
        // Translation is in column 3
        assert!((mat[3][0] - 3.0).abs() < 0.001);
        assert!((mat[3][1] - 4.0).abs() < 0.001);
        assert!((mat[3][2] - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_get_world_position_with_parent() {
        let mut world = FlintWorld::new();
        let parent = world.spawn("parent").unwrap();
        let child = world.spawn("child").unwrap();

        // Helper to set position
        let make_pos = |x: f32, y: f32, z: f32| {
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(x as f64),
                        toml::Value::Float(y as f64),
                        toml::Value::Float(z as f64),
                    ]),
                );
                m
            })
        };

        world
            .set_component(parent, "transform", make_pos(10.0, 0.0, 0.0))
            .unwrap();
        world
            .set_component(child, "transform", make_pos(5.0, 0.0, 0.0))
            .unwrap();
        world.set_parent(child, parent).unwrap();

        let world_pos = world.get_world_position(child).unwrap();
        assert!(
            (world_pos.x - 15.0).abs() < 0.001,
            "child world x should be 15, got {}",
            world_pos.x
        );
    }

    #[test]
    fn test_get_world_position_deep_hierarchy() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();
        let c = world.spawn("c").unwrap();

        let make_pos = |x: f32, y: f32, z: f32| {
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(x as f64),
                        toml::Value::Float(y as f64),
                        toml::Value::Float(z as f64),
                    ]),
                );
                m
            })
        };

        world
            .set_component(a, "transform", make_pos(1.0, 0.0, 0.0))
            .unwrap();
        world
            .set_component(b, "transform", make_pos(2.0, 0.0, 0.0))
            .unwrap();
        world
            .set_component(c, "transform", make_pos(3.0, 0.0, 0.0))
            .unwrap();

        world.set_parent(b, a).unwrap();
        world.set_parent(c, b).unwrap();

        let pos = world.get_world_position(c).unwrap();
        assert!(
            (pos.x - 6.0).abs() < 0.001,
            "deep hierarchy world x should be 6, got {}",
            pos.x
        );
    }

    #[test]
    fn test_get_world_matrix_no_transform() {
        let mut world = FlintWorld::new();
        let id = world.spawn("bare").unwrap();
        // No transform component set
        assert!(world.get_world_matrix(id).is_none());
    }

    #[test]
    fn a_bare_parent_keeps_the_childs_transform() {
        let mut world = FlintWorld::new();
        let make_pos = |x: f32, y: f32, z: f32| {
            let mut m = toml::map::Map::new();
            m.insert(
                "position".into(),
                toml::Value::Array(vec![
                    (x as f64).into(),
                    (y as f64).into(),
                    (z as f64).into(),
                ]),
            );
            toml::Value::Table(m)
        };
        let group = world.spawn("group").unwrap();
        let child = world.spawn("child").unwrap();
        world
            .set_component(child, "transform", make_pos(1.0, 2.0, 0.0))
            .unwrap();
        world.set_parent(child, group).unwrap();

        let pos = world.get_world_position(child).unwrap();
        assert!(
            (pos - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5,
            "under a bare parent: {pos:?}"
        );

        // The chain goes on through the bare parent to a placed grandparent.
        let root = world.spawn("root").unwrap();
        world
            .set_component(root, "transform", make_pos(10.0, 0.0, 0.0))
            .unwrap();
        world.set_parent(group, root).unwrap();
        let pos = world.get_world_position(child).unwrap();
        assert!(
            (pos - Vec3::new(11.0, 2.0, 0.0)).length() < 1e-5,
            "through it: {pos:?}"
        );

        // The bare parent itself still has no world matrix, but it has a frame.
        assert!(world.get_world_matrix(group).is_none());
        assert_eq!(world.get_frame_matrix(group)[3][0], 10.0);
    }

    // ── Name-based operations tests ────────────────────────────────

    #[test]
    fn test_despawn_by_name() {
        let mut world = FlintWorld::new();
        world.spawn("target").unwrap();
        assert!(world.contains_name("target"));

        world.despawn_by_name("target").unwrap();
        assert!(!world.contains_name("target"));
    }

    #[test]
    fn test_despawn_by_name_not_found() {
        let mut world = FlintWorld::new();
        let result = world.despawn_by_name("ghost");
        assert!(matches!(result, Err(FlintError::EntityNotFound(_))));
    }

    #[test]
    fn test_set_parent_by_name() {
        let mut world = FlintWorld::new();
        let parent_id = world.spawn("parent").unwrap();
        let child_id = world.spawn("child").unwrap();

        world.set_parent_by_name("child", "parent").unwrap();

        assert_eq!(world.get_parent(child_id), Some(parent_id));
        assert!(world.get_children(parent_id).contains(&child_id));
    }

    // ── Parent cycles (#680) ───────────────────────────────────────

    fn cycle_of(result: Result<()>) -> (String, String, Vec<String>) {
        match result {
            Err(FlintError::ParentCycle {
                child,
                parent,
                chain,
            }) => (child, parent, chain),
            other => panic!("expected a parent cycle, got {other:?}"),
        }
    }

    #[test]
    fn an_entity_cannot_be_its_own_parent() {
        let mut world = FlintWorld::new();
        let lamp = world.spawn("lamp").unwrap();
        let refused = world.set_parent(lamp, lamp);
        assert_eq!(
            refused.as_ref().unwrap_err().to_string(),
            "Parent cycle: `lamp` can't be parented to itself"
        );
        assert_eq!(
            cycle_of(refused),
            ("lamp".into(), "lamp".into(), vec!["lamp".into()])
        );
        assert_eq!(world.get_parent(lamp), None, "nothing was stored");
        // And the walks that used to overflow end
        assert!(world.get_world_matrix(lamp).is_none());
        assert_eq!(
            world.get_frame_matrix(lamp),
            Transform::IDENTITY.to_matrix()
        );
    }

    #[test]
    fn a_two_and_a_three_entity_cycle_are_refused() {
        let mut world = FlintWorld::new();
        let cart = world.spawn("cart").unwrap();
        let wheel = world.spawn("wheel").unwrap();
        world.set_parent(wheel, cart).unwrap();
        let (child, parent, chain) = cycle_of(world.set_parent(cart, wheel));
        assert_eq!((child.as_str(), parent.as_str()), ("cart", "wheel"));
        assert_eq!(chain, ["wheel", "cart"]);
        assert_eq!(world.get_parent(cart), None, "the refused link isn't kept");
        assert_eq!(world.get_parent(wheel), Some(cart), "the other one is");

        let hub = world.spawn("hub").unwrap();
        world.set_parent(hub, wheel).unwrap();
        let refused = world.set_parent(cart, hub);
        assert_eq!(
            refused.as_ref().unwrap_err().to_string(),
            "Parent cycle: `cart` can't be parented to `hub`, which is below it \
             (hub -> wheel -> cart, each the child of the next)"
        );
        // A middle link can't close one either
        let (_, _, chain) = cycle_of(world.set_parent(wheel, hub));
        assert_eq!(chain, ["hub", "wheel"]);
    }

    #[test]
    fn reparenting_that_makes_no_loop_is_allowed() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();
        let c = world.spawn("c").unwrap();
        world.set_parent(b, a).unwrap();
        world.set_parent(c, b).unwrap();
        // The same parent again, a sibling's, and the root under a leaf
        // once the leaf has moved out from under it
        world.set_parent(c, b).unwrap();
        world.set_parent(c, a).unwrap();
        world.set_parent(c, b).unwrap();
        world.set_parent(c, a).unwrap();
        let other = world.spawn("other").unwrap();
        world.set_parent(a, other).unwrap();
        assert_eq!(world.get_parent(a), Some(other));
    }

    /// The loop that replaced the recursion multiplies in the order the
    /// recursion did, so every matrix is the same bits as before.
    #[test]
    fn the_world_matrix_is_the_product_from_the_root_down() {
        let mut world = FlintWorld::new();
        let placed = |p: [f64; 3], r: [f64; 3], s: [f64; 3]| {
            let v = |a: [f64; 3]| toml::Value::Array(a.iter().map(|x| (*x).into()).collect());
            let mut m = toml::map::Map::new();
            m.insert("position".into(), v(p));
            m.insert("rotation".into(), v(r));
            m.insert("scale".into(), v(s));
            toml::Value::Table(m)
        };
        let names = ["root", "arm", "bare", "hand", "finger"];
        let ids: Vec<EntityId> = names.iter().map(|n| world.spawn(*n).unwrap()).collect();
        for (i, id) in ids.iter().enumerate() {
            if names[i] != "bare" {
                let f = i as f64;
                world
                    .set_component(
                        *id,
                        "transform",
                        placed(
                            [0.3 * f, 1.7 - f, 0.11 * f],
                            [13.0 * f, 29.0 + f, -7.0 * f],
                            [1.0 + 0.1 * f, 0.9, 1.3],
                        ),
                    )
                    .unwrap();
            }
            if i > 0 {
                world.set_parent(*id, ids[i - 1]).unwrap();
            }
        }
        let local = |id: EntityId| world.local_frame(id);
        let mut expected = local(ids[0]);
        for id in &ids[1..] {
            expected = mat4_mul(&expected, &local(*id));
        }
        let finger = *ids.last().unwrap();
        assert_eq!(world.get_world_matrix(finger).unwrap(), expected);
        assert_eq!(world.get_frame_matrix(finger), expected);
    }

    /// A chain far deeper than the stack the recursion had: 20,000 levels
    /// on a test thread's 2 MB stack.
    #[test]
    fn a_deep_chain_does_not_overflow_the_stack() {
        let mut world = FlintWorld::new();
        let step = {
            let mut m = toml::map::Map::new();
            m.insert(
                "position".into(),
                toml::Value::Array(vec![1.0.into(), 0.0.into(), 0.0.into()]),
            );
            toml::Value::Table(m)
        };
        let links: Vec<EntityId> = (0..20_000)
            .map(|i| {
                let id = world.spawn(format!("link{i}")).unwrap();
                world.set_component(id, "transform", step.clone()).unwrap();
                id
            })
            .collect();
        // Linked from the bottom up, so each parent has none yet and the
        // cycle check walks one step, not the whole chain
        for pair in links.windows(2).rev() {
            world.set_parent(pair[1], pair[0]).unwrap();
        }
        let pos = world.get_world_position(*links.last().unwrap()).unwrap();
        assert_eq!(pos.x, 20_000.0);
    }

    // ── #691: storage of its own, a guarded index, despawn takes the subtree

    /// The index as it would be rebuilt from what each entity holds: what
    /// `component_index` must always equal (empty sets don't count).
    fn index_from_records(world: &FlintWorld) -> BTreeMap<String, BTreeSet<EntityId>> {
        let mut index: BTreeMap<String, BTreeSet<EntityId>> = BTreeMap::new();
        for (&id, record) in &world.entities {
            for name in record.components.data.keys() {
                index.entry(name.clone()).or_default().insert(id);
            }
        }
        index
    }

    fn assert_index_in_step(world: &FlintWorld) {
        let kept: BTreeMap<String, BTreeSet<EntityId>> = world
            .component_index
            .iter()
            .filter(|(_, set)| !set.is_empty())
            .map(|(name, set)| (name.clone(), set.clone()))
            .collect();
        assert_eq!(kept, index_from_records(world));
    }

    fn table() -> toml::Value {
        toml::Value::Table(Default::default())
    }

    /// Every way there is to write a component, through the world and
    /// through the guard, leaves the index equal to what the entities hold:
    /// the reason `rebuild_component_index` could go.
    #[test]
    fn no_write_puts_the_index_out_of_step() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();

        world.set_component(a, "door", table()).unwrap();
        world.merge_component(a, "material", table()).unwrap();
        world
            .set_field(b, "health", "hp", toml::Value::Integer(3))
            .unwrap();
        assert_index_in_step(&world);

        {
            let mut comps = world.get_components_mut(b).unwrap();
            assert_eq!(comps.id(), b);
            comps.set("light", table());
            comps.set_field("rigidbody", "mass", toml::Value::Float(2.0));
            comps.merge_component("collider", table());
            assert!(comps.remove("health").is_some());
            assert!(comps.remove("never_there").is_none());
            // Changing a component's value in place adds no component
            comps
                .get_mut("light")
                .unwrap()
                .as_table_mut()
                .unwrap()
                .insert("intensity".into(), toml::Value::Float(4.0));
            comps.set_archetype(Some("lamp".into()));
            // Reading goes through Deref
            assert!(comps.has("light") && !comps.has("health"));
        }
        assert_index_in_step(&world);
        assert!(world.entities_with_component("health").is_empty());
        assert!(world.entities_with_component("rigidbody").contains(&b));
        assert_eq!(
            world.get_components(b).unwrap().archetype.as_deref(),
            Some("lamp")
        );

        let mut extra = DynamicComponents::new();
        extra.set("script", table());
        extra.set("door", table());
        world.get_components_mut(b).unwrap().merge(&extra);
        assert_index_in_step(&world);
        assert_eq!(
            world.entities_with_components(&["door", "script"]),
            [b],
            "merge indexes what it brings"
        );

        world.remove_component(a, "door").unwrap();
        world
            .get_component_mut(a, "material")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .insert("color".into(), toml::Value::String("red".into()));
        assert!(world.get_component_mut(a, "door").is_none());
        assert_index_in_step(&world);

        world.despawn(b).unwrap();
        assert_index_in_step(&world);
        assert!(world.get_components_mut(b).is_none());
    }

    /// Despawn takes the subtree, whole and only it (#691): every
    /// descendant's name, components, parent links and index entries go,
    /// the ids come back parent before child, and a child reparented
    /// first stays where it was put.
    #[test]
    fn despawn_takes_the_subtree() {
        let mut world = FlintWorld::new();
        let names = ["car", "body", "wheel_l", "wheel_r", "hub", "garage"];
        let id: HashMap<&str, EntityId> = names
            .iter()
            .map(|n| (*n, world.spawn(*n).unwrap()))
            .collect();
        world.set_parent(id["car"], id["garage"]).unwrap();
        world.set_parent(id["body"], id["car"]).unwrap();
        world.set_parent(id["wheel_r"], id["car"]).unwrap();
        world.set_parent(id["wheel_l"], id["car"]).unwrap();
        world.set_parent(id["hub"], id["wheel_l"]).unwrap();
        for n in names {
            world.set_component(id[n], "transform", table()).unwrap();
        }

        let gone = world.despawn_by_name("car").unwrap();
        assert_eq!(
            names_of_gone(&gone, &id),
            ["car", "body", "wheel_l", "wheel_r", "hub"],
            "the entity, then a generation at a time, siblings by name"
        );
        assert_eq!(world.entity_count(), 1);
        for n in ["car", "body", "wheel_l", "wheel_r", "hub"] {
            assert!(!world.contains(id[n]) && !world.contains_name(n), "{n}");
            assert!(world.get_parent(id[n]).is_none());
            assert!(world.children(id[n]).is_empty());
        }
        assert!(world.children(id["garage"]).is_empty());
        assert_eq!(
            world.entities_with_component("transform").len(),
            1,
            "only the garage is left in the index"
        );
        assert!(world.parents.is_empty() && world.children.is_empty());
        assert_index_in_step(&world);

        // A name is free again, with a new id
        let car = world.spawn("car").unwrap();
        assert!(!gone.contains(&car));

        // Keep a child: reparent it before the despawn
        let rack = world.spawn("rack").unwrap();
        let bike = world.spawn("bike").unwrap();
        world.set_parent(rack, car).unwrap();
        world.set_parent(bike, rack).unwrap();
        world.set_parent(bike, id["garage"]).unwrap();
        assert_eq!(world.despawn(car).unwrap(), [car, rack]);
        assert_eq!(world.get_parent(bike), Some(id["garage"]));
        assert_eq!(world.children(id["garage"]), [bike]);

        // Despawning what's gone is an error, and changes nothing
        assert!(world.despawn(car).is_err());
        assert_eq!(world.entity_count(), 2);
    }

    fn names_of_gone<'a>(gone: &[EntityId], id: &HashMap<&'a str, EntityId>) -> Vec<&'a str> {
        gone.iter()
            .map(|g| *id.iter().find(|(_, v)| *v == g).unwrap().0)
            .collect()
    }

    #[test]
    fn an_id_that_is_live_is_not_spawned_twice() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let err = world.spawn_with_id(a, "b").unwrap_err();
        assert!(err.to_string().contains("'a'"), "{err}");
        assert_eq!(world.get_name(a), Some("a"));
        assert!(!world.contains_name("b"));
    }

    #[test]
    fn a_write_stamp_moves_with_every_write_and_reparent_and_only_then() {
        let mut world = FlintWorld::new();
        let a = world.spawn("a").unwrap();
        let p = world.spawn("p").unwrap();
        let q = world.spawn("q").unwrap();
        let stamp = |w: &FlintWorld| w.write_stamp(a).unwrap();

        let s0 = stamp(&world);
        // Reading moves nothing
        let _ = world.get_components(a);
        let _ = world.get_transform(a);
        assert_eq!(stamp(&world), s0);

        world
            .set_field(a, "transform", "scale", toml::Value::Float(2.0))
            .unwrap();
        let s1 = stamp(&world);
        assert_ne!(s1, s0);
        let _ = world.get_component_mut(a, "transform");
        let s2 = stamp(&world);
        assert_ne!(s2, s1);

        // A parent made, changed and taken away moves the child's stamp,
        // not the parent's
        let ps = world.write_stamp(p).unwrap();
        world.set_parent(a, p).unwrap();
        let s3 = stamp(&world);
        assert_ne!(s3, s2);
        assert_eq!(world.write_stamp(p), Some(ps));
        world.set_parent(a, p).unwrap();
        assert_eq!(stamp(&world), s3, "the same parent again is no change");
        world.set_parent(a, q).unwrap();
        assert_ne!(stamp(&world), s3);

        // A world made again never hands out a stamp already seen
        let mut again = FlintWorld::new();
        let a2 = again.spawn("a").unwrap();
        assert_eq!(a2, a);
        assert!(again.write_stamp(a2).unwrap() > stamp(&world));
        world.despawn(a).unwrap();
        assert_eq!(world.write_stamp(a), None);
    }
}
