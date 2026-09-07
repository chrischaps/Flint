//! Persistent Data Store — key-value storage that survives scene transitions.
//!
//! Stores data as `toml::Value` for consistency with the ECS dynamic component
//! system. Data can be saved to / loaded from TOML files for cross-session persistence.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// A key-value store that persists across scene transitions.
///
/// Values are stored as [`toml::Value`] to match the engine's dynamic component
/// system. The store can be serialized to / deserialized from TOML files.
///
/// Mutations raise a `dirty` flag; the host (the player) polls it through
/// [`SaveDebounce`] and writes the store to disk shortly after the last
/// change. A script can ask for an immediate write with `persist_save()`,
/// which sets `flush_requested`.
#[derive(Default)]
pub struct PersistentStore {
    data: HashMap<String, toml::Value>,
    dirty: bool,
    flush_requested: bool,
}

impl PersistentStore {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            dirty: false,
            flush_requested: false,
        }
    }

    /// True when the store has changed since the last `mark_clean`.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag (after a successful save).
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Ask the host to write the store now rather than after the debounce.
    pub fn request_flush(&mut self) {
        self.flush_requested = true;
    }

    /// Consume a pending flush request.
    pub fn take_flush_request(&mut self) -> bool {
        std::mem::take(&mut self.flush_requested)
    }

    /// Set a value by key. Overwrites any existing value.
    pub fn set(&mut self, key: &str, value: toml::Value) {
        self.data.insert(key.to_string(), value);
        self.dirty = true;
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
        let old = self.data.remove(key);
        if old.is_some() {
            self.dirty = true;
        }
        old
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        if !self.data.is_empty() {
            self.dirty = true;
        }
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
        self.dirty = false;
        Ok(())
    }
}

/// Decides *when* a dirty [`PersistentStore`] gets written: one write per
/// burst of changes, `delay` after the last change began the burst, or at
/// once when a flush is forced (scene transition, exit, `persist_save()`).
///
/// Kept free of I/O so the timing can be unit tested with synthetic clocks.
#[derive(Debug, Clone)]
pub struct SaveDebounce {
    delay: Duration,
    dirty_since: Option<Instant>,
}

impl Default for SaveDebounce {
    fn default() -> Self {
        Self::new(Duration::from_secs(1))
    }
}

impl SaveDebounce {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            dirty_since: None,
        }
    }

    /// Record that the store is dirty as of `now`. The first change of a
    /// burst starts the clock; later changes do not push it back, so a
    /// script writing every frame still gets saved once a second.
    pub fn note_dirty(&mut self, now: Instant) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(now);
        }
    }

    pub fn is_pending(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// True when a save should happen now: `force`, or the delay has
    /// elapsed since the burst began. Resets the pending state when it
    /// returns true; the caller performs the write.
    pub fn should_flush(&mut self, now: Instant, force: bool) -> bool {
        let Some(since) = self.dirty_since else {
            return false;
        };
        if force || now.saturating_duration_since(since) >= self.delay {
            self.dirty_since = None;
            true
        } else {
            false
        }
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
    fn dirty_flag_tracks_mutations() {
        let mut store = PersistentStore::new();
        assert!(!store.is_dirty());

        store.set("a", toml::Value::Integer(1));
        assert!(store.is_dirty());
        store.mark_clean();

        store.remove("missing");
        assert!(!store.is_dirty(), "removing an absent key is not a change");
        store.remove("a");
        assert!(store.is_dirty());
        store.mark_clean();

        store.clear();
        assert!(!store.is_dirty(), "clearing an empty store is not a change");

        assert!(!store.take_flush_request());
        store.request_flush();
        assert!(store.take_flush_request());
        assert!(!store.take_flush_request(), "request is consumed");
    }

    #[test]
    fn debounce_waits_then_flushes_once() {
        let t0 = Instant::now();
        let mut d = SaveDebounce::new(Duration::from_secs(1));
        assert!(!d.should_flush(t0, false), "nothing pending");

        d.note_dirty(t0);
        assert!(d.is_pending());
        assert!(!d.should_flush(t0 + Duration::from_millis(500), false));

        // A later change inside the window does not restart the clock.
        d.note_dirty(t0 + Duration::from_millis(800));
        assert!(d.should_flush(t0 + Duration::from_millis(1000), false));
        assert!(!d.is_pending());
        assert!(!d.should_flush(t0 + Duration::from_secs(5), false));
    }

    #[test]
    fn debounce_force_flushes_immediately() {
        let t0 = Instant::now();
        let mut d = SaveDebounce::default();
        d.note_dirty(t0);
        assert!(d.should_flush(t0, true));
        assert!(
            !d.should_flush(t0, true),
            "force with nothing pending is a no-op"
        );
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
