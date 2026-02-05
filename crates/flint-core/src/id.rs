//! Stable entity identifiers

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for generating unique IDs
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A stable entity identifier that persists across save/load cycles.
///
/// Unlike internal ECS entity IDs which may be recycled or change,
/// `EntityId` provides a stable reference for scene files and queries.
#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u64);

impl EntityId {
    /// Create a new unique EntityId
    pub fn new() -> Self {
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Create an EntityId from a raw value (for deserialization/testing)
    pub fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw u64 value
    pub fn raw(&self) -> u64 {
        self.0
    }

    /// Reset the ID counter (for testing only)
    #[cfg(test)]
    pub fn reset_counter() {
        NEXT_ID.store(1, Ordering::Relaxed);
    }

    /// Set the counter to at least the given value (for loading scenes)
    pub fn ensure_counter_above(value: u64) {
        let mut current = NEXT_ID.load(Ordering::Relaxed);
        while current <= value {
            match NEXT_ID.compare_exchange_weak(
                current,
                value + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(c) => current = c,
            }
        }
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
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
    fn test_id_generation() {
        EntityId::reset_counter();
        let id1 = EntityId::new();
        let id2 = EntityId::new();
        assert_ne!(id1, id2);
        assert!(id2.0 > id1.0);
    }

    #[test]
    fn test_from_raw() {
        let id = EntityId::from_raw(42);
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn test_ensure_counter_above() {
        EntityId::reset_counter();
        EntityId::ensure_counter_above(100);
        let id = EntityId::new();
        assert!(id.0 > 100);
    }
}
