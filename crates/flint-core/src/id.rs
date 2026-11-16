//! Stable entity identifiers

use serde::{Deserialize, Serialize};
use std::fmt;

/// A stable entity identifier that persists across save/load cycles.
///
/// Unlike internal ECS entity IDs which may be recycled or change,
/// `EntityId` provides a stable reference for scene files and queries.
///
/// Ids are handed out by a world, never minted on their own (#688): each
/// `FlintWorld` counts from 1 (or from past the world before it, so a scene
/// transition never reuses an id something may still hold), and a scene
/// spawns its entities in name order, so the same scene always gives the
/// same names the same ids. There was a process-wide counter, which made
/// ids depend on whatever had been spawned before; it is gone, and with it
/// `EntityId::new()`. An id no world gave out (a test's "nobody") is
/// [`EntityId::from_raw`].
#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u64);

impl EntityId {
    /// Create an EntityId from a raw value (for deserialization/testing)
    pub fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw u64 value
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_raw() {
        let id = EntityId::from_raw(42);
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn ids_order_by_value() {
        assert!(EntityId::from_raw(2) < EntityId::from_raw(10));
    }
}
