//! Persistent Data Store — key-value storage that survives scene transitions.
//!
//! Stores data as `toml::Value` for consistency with the ECS dynamic component
//! system. Data can be saved to / loaded from TOML files for cross-session persistence.

use std::collections::HashMap;
use std::path::Path;

/// A key-value store that persists across scene transitions.
///
/// Values are stored as [`toml::Value`] to match the engine's dynamic component
/// system. The store can be serialized to / deserialized from TOML files.
pub struct PersistentStore {
    data: HashMap<String, toml::Value>,
}

impl PersistentStore {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Set a value by key. Overwrites any existing value.
    pub fn set(&mut self, key: &str, value: toml::Value) {
        self.data.insert(key.to_string(), value);
    }

    /// Get a value by key.
    pub fn get(&self, key: &str) -> Option<&toml::Value> {
        self.data.get(key)
    }

    /// Check if a key exists.
    pub fn has(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// Remove a key, returning the old value if it existed.
    pub fn remove(&mut self, key: &str) -> Option<toml::Value> {
        self.data.remove(key)
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Return all keys.
    pub fn keys(&self) -> Vec<&str> {
        self.data.keys().map(|k| k.as_str()).collect()
    }

    /// Save the store to a TOML file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        // Build a TOML table from the data
        let mut table = toml::map::Map::new();
        for (k, v) in &self.data {
            table.insert(k.clone(), v.clone());
        }
        let content =
            toml::to_string_pretty(&table).map_err(|e| format!("serialize error: {e}"))?;
        std::fs::write(path, content).map_err(|e| format!("write error: {e}"))
    }

    /// Load the store from a TOML file, replacing all current data.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
        let table: toml::map::Map<String, toml::Value> =
            toml::from_str(&content).map_err(|e| format!("parse error: {e}"))?;
        self.data.clear();
        for (k, v) in table {
            self.data.insert(k, v);
        }
        Ok(())
    }
}

impl Default for PersistentStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get() {
        let mut store = PersistentStore::new();
        store.set("score", toml::Value::Integer(42));
        assert_eq!(store.get("score"), Some(&toml::Value::Integer(42)));
    }

    #[test]
    fn has_and_remove() {
        let mut store = PersistentStore::new();
        store.set("name", toml::Value::String("Alice".into()));
        assert!(store.has("name"));
        assert!(!store.has("missing"));

        let removed = store.remove("name");
        assert_eq!(removed, Some(toml::Value::String("Alice".into())));
        assert!(!store.has("name"));
    }

    #[test]
    fn clear() {
        let mut store = PersistentStore::new();
        store.set("a", toml::Value::Integer(1));
        store.set("b", toml::Value::Integer(2));
        assert_eq!(store.keys().len(), 2);

        store.clear();
        assert_eq!(store.keys().len(), 0);
    }

    #[test]
    fn keys() {
        let mut store = PersistentStore::new();
        store.set("x", toml::Value::Boolean(true));
        store.set("y", toml::Value::Boolean(false));
        let mut keys = store.keys();
        keys.sort();
        assert_eq!(keys, vec!["x", "y"]);
    }

    #[test]
    fn overwrite() {
        let mut store = PersistentStore::new();
        store.set("val", toml::Value::Integer(1));
        store.set("val", toml::Value::Integer(2));
        assert_eq!(store.get("val"), Some(&toml::Value::Integer(2)));
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join("flint_persist_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_store.toml");

        let mut store = PersistentStore::new();
        store.set("score", toml::Value::Integer(100));
        store.set("name", toml::Value::String("Player".into()));
        store.save_to_file(&path).expect("save failed");

        let mut loaded = PersistentStore::new();
        loaded.load_from_file(&path).expect("load failed");
        assert_eq!(loaded.get("score"), Some(&toml::Value::Integer(100)));
        assert_eq!(
            loaded.get("name"),
            Some(&toml::Value::String("Player".into()))
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_replaces_existing() {
        let dir = std::env::temp_dir().join("flint_persist_test2");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_replace.toml");

        let mut store = PersistentStore::new();
        store.set("only_in_file", toml::Value::Boolean(true));
        store.save_to_file(&path).expect("save failed");

        let mut store2 = PersistentStore::new();
        store2.set("old_key", toml::Value::Integer(0));
        store2.load_from_file(&path).expect("load failed");

        assert!(store2.has("only_in_file"));
        assert!(!store2.has("old_key")); // replaced

        let _ = std::fs::remove_file(&path);
    }
}
