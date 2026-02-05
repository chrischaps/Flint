//! Entity information and metadata

use flint_core::EntityId;
use serde::{Deserialize, Serialize};

/// Information about an entity for queries and serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityInfo {
    /// The stable entity ID
    pub id: EntityId,
    /// Human-readable name
    pub name: String,
    /// Archetype name (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Parent entity name (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Component names present on this entity
    pub components: Vec<String>,
}

impl EntityInfo {
    pub fn new(id: EntityId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            archetype: None,
            parent: None,
            components: Vec::new(),
        }
    }

    pub fn with_archetype(mut self, archetype: impl Into<String>) -> Self {
        self.archetype = Some(archetype.into());
        self
    }

    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    pub fn with_components(mut self, components: Vec<String>) -> Self {
        self.components = components;
        self
    }
}
