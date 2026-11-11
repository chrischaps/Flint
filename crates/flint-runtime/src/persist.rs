//! Persistent Data Store — key-value storage that survives scene transitions.
//!
//! Stores data as `toml::Value` for consistency with the ECS dynamic component
//! system. Data can be saved to / loaded from TOML files for cross-session persistence.
//!
//! # The saved form (#726)
//!
//! Every game key is a top-level key, as it always was, and the engine's
//! own facts sit in one reserved table:
//!
//! ```toml
//! coins = 7
//! selected_track = "scenes/oval.scene.toml"
//!
//! [meta]
//! version = 2
//! ```
//!
//! A file with no `[meta]` is **version 0**, every key the game's: the form
//! written before versions existed, and still the form written while the
//! version is 0. `meta` is the one name the engine keeps. A game key called
//! `meta` keeps working: it is saved inside the table as `key_meta`, and a
//! top-level `meta` that is not the engine's table (anything but a table
//! holding an integer `version` and at most `key_meta`) is read as the
//! game's key, from a version 0 file.
//!
//! # Slots, and where they are kept
//!
//! The store holds one **slot** at a time, [`DEFAULT_SLOT`] to begin with.
//! Where a slot's text lives is the host's business, told to the store as a
//! [`SaveBackend`] (the player's is files under the OS user directory, or
//! `localStorage` on the web). With one installed the store can
//! [`load`](PersistentStore::load), [`save`](PersistentStore::save) and
//! [`open_slot`](PersistentStore::open_slot) by itself, which is what lets a
//! script switch slots and read the new one in the same call.
//!
//! A slot whose text doesn't parse is never written over: the backend sets
//! it aside (`<file>.bak`), the store starts empty, and
//! [`corrupt`](PersistentStore::corrupt) says what happened. If it can't be
//! set aside, saving that slot is refused for the run.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use web_time::Instant;

/// The slot a store holds until told otherwise: the one save file games had
/// before slots.
pub const DEFAULT_SLOT: &str = "default";

/// The top-level key the engine keeps for itself in a saved store.
pub const META_KEY: &str = "meta";
const META_VERSION: &str = "version";
/// Where a game key named `meta` is saved, inside the engine's table.
const META_GAME_KEY: &str = "key_meta";

/// The longest slot name.
pub const MAX_SLOT_NAME: usize = 32;

/// Whether `name` can be a slot: 1 to 32 of `a-z`, `0-9`, `_` and `-`,
/// starting with a letter or digit. A slot is a file name on every platform,
/// so there is no dot (no `..`), no separator, no upper case (two names
/// that are one file on Windows), and none of the names Windows keeps for
/// devices. `persist` is refused too: it is the default slot's file where
/// saves are kept in the project folder.
pub fn check_slot_name(name: &str) -> Result<(), String> {
    const RESERVED: [&str; 5] = ["persist", "con", "prn", "aux", "nul"];
    let bad = |why: &str| Err(format!("'{name}' can't be a save slot: {why}"));
    if name.is_empty() {
        return bad("it is empty");
    }
    if name.len() > MAX_SLOT_NAME {
        return bad("it is longer than 32 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return bad("a slot is named with a-z, 0-9, _ and - only");
    }
    if name.starts_with(['_', '-']) {
        return bad("it must start with a letter or a digit");
    }
    let device = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n != "0" && n.chars().all(|c| c.is_ascii_digit()))
    };
    if RESERVED.contains(&name) || device("com") || device("lpt") {
        return bad("the name is reserved");
    }
    Ok(())
}

/// A slot's saved text, as a [`SaveBackend`] read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveText {
    pub text: String,
    /// The file (or storage key) it came from, for logs.
    pub from: String,
    /// It came from a place saves used to be kept, not from where the slot
    /// is written. Such a file is never written to, so an unreadable one
    /// needs no setting aside.
    pub old_place: bool,
}

/// Where slots are kept. The host installs one
/// ([`PersistentStore::set_backend`]); every method names a slot that
/// passed [`check_slot_name`].
pub trait SaveBackend: Send + Sync {
    /// The slot's text, or `None` when nothing is saved for it.
    fn read(&self, slot: &str) -> Result<Option<SaveText>, String>;
    /// Replace the slot's text in one step: after a failure the old text
    /// is still whole.
    fn write(&self, slot: &str, text: &str) -> Result<(), String>;
    /// Move the slot's text out of the way of the next `write`, keeping it,
    /// and say where it is now.
    fn set_aside(&self, slot: &str) -> Result<String, String>;
    /// The slots that have something saved, in name order.
    fn slots(&self) -> Vec<String>;
    /// The file (or storage key) the slot is written to, for logs.
    fn place(&self, slot: &str) -> String;
}

/// A slot whose saved text could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptSave {
    pub slot: String,
    /// The file that didn't parse.
    pub file: String,
    /// Where its text is now: `<file>.bak` (or `.bak.1`, ...) once set
    /// aside, or `file` itself when it was left in place (an old place, or
    /// a file that couldn't be moved, in which case saving is refused).
    pub backup: String,
    pub error: String,
}

/// What [`PersistentStore::load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// No backend is installed: the store is in memory only.
    NoBackend,
    /// Nothing is saved for the slot: an empty store, at the game's version.
    Fresh { place: String },
    /// The slot's keys were read.
    Read {
        keys: usize,
        from: String,
        version: i64,
        /// `Some(place)`: read from where saves used to be kept; `place` is
        /// where the slot is saved from now on.
        moved_to: Option<String>,
    },
    /// The text didn't parse; it was kept, and the store is empty.
    Corrupt(CorruptSave),
    /// The slot couldn't be read at all; it is left alone, and saving it is
    /// refused.
    Unreadable { place: String, error: String },
}

impl Loaded {
    /// Whether this is something the person playing should hear about.
    pub fn is_error(&self) -> bool {
        matches!(self, Loaded::Corrupt(_) | Loaded::Unreadable { .. })
    }

    /// Whether the save was read from an old place.
    pub fn is_move(&self) -> bool {
        matches!(
            self,
            Loaded::Read {
                moved_to: Some(_),
                ..
            }
        )
    }
}

impl std::fmt::Display for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Loaded::NoBackend => write!(f, "no save location; the store is in memory only"),
            Loaded::Fresh { place } => write!(f, "nothing saved at {place} (fresh store)"),
            Loaded::Read {
                keys,
                from,
                version,
                moved_to: None,
            } => write!(f, "loaded {keys} keys from {from} (version {version})"),
            Loaded::Read {
                keys,
                from,
                version,
                moved_to: Some(place),
            } => write!(
                f,
                "loaded {keys} keys (version {version}) from {from}, where saves used to be \
                 kept. This game's save now lives at {place}: it is written there from now on, \
                 the old file is left where it is, and it is read for one release"
            ),
            Loaded::Corrupt(c) if c.backup == c.file => write!(
                f,
                "the save file {} could not be parsed ({}). It is left where it is and the \
                 game starts with an empty store",
                c.file, c.error
            ),
            Loaded::Corrupt(c) => write!(
                f,
                "the save file {} could not be parsed ({}). It is kept as {} and the game \
                 starts with an empty store",
                c.file, c.error, c.backup
            ),
            Loaded::Unreadable { place, error } => write!(
                f,
                "the save file {place} could not be read ({error}). It is left as it is, and \
                 nothing is saved over it in this run"
            ),
        }
    }
}

/// A key-value store that persists across scene transitions.
///
/// Values are stored as [`toml::Value`] to match the engine's dynamic component
/// system. The store can be serialized to / deserialized from TOML files.
///
/// Mutations raise a `dirty` flag; the host (the player) polls it through
/// [`SaveDebounce`] and writes the store to disk shortly after the last
/// change. A script can ask for an immediate write with `persist_save()`,
/// which sets `flush_requested`.
pub struct PersistentStore {
    data: HashMap<String, toml::Value>,
    dirty: bool,
    flush_requested: bool,
    /// The version of the data held: the file's `[meta] version` (0 with
    /// none), or the game's for a store that began empty.
    version: i64,
    /// The version the game is at (`[project] save_version`).
    game_version: i64,
    /// The data was loaded at this lower version and no script has
    /// migrated it yet.
    migrate_from: Option<i64>,
    slot: String,
    backend: Option<Box<dyn SaveBackend>>,
    corrupt: Option<CorruptSave>,
    /// Why saving this slot is refused, when it is.
    write_blocked: Option<String>,
}

impl Default for PersistentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PersistentStore {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            dirty: false,
            flush_requested: false,
            version: 0,
            game_version: 0,
            migrate_from: None,
            slot: DEFAULT_SLOT.to_string(),
            backend: None,
            corrupt: None,
            write_blocked: None,
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

    // ─── Version ──────────────────────────────────────────

    /// The version of the data held: what the slot's file said (0 for a
    /// file with no `[meta]`), until a migration or [`Self::set_version`]
    /// moves it on. A store that began empty is at the game's version.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Stamp the data with `version`: what the next save writes. Reaching
    /// the game's version ends a pending migration.
    pub fn set_version(&mut self, version: i64) {
        if version != self.version {
            self.version = version;
            self.dirty = true;
        }
        if version >= self.game_version {
            self.migrate_from = None;
        }
    }

    /// The version the game is at. Set by the host before the first load.
    pub fn game_version(&self) -> i64 {
        self.game_version
    }

    pub fn set_game_version(&mut self, version: i64) {
        self.game_version = version;
        if self.data.is_empty() && !self.dirty && self.migrate_from.is_none() {
            // Nothing loaded yet: an empty store is already current
            self.version = self.version.max(version);
        }
    }

    /// The version the held data was loaded at, while it is lower than the
    /// game's and nothing has migrated it: what `on_persist_migrate(from)`
    /// is called with.
    pub fn pending_migration(&self) -> Option<i64> {
        self.migrate_from
    }

    /// The scripts have had their `on_persist_migrate`: the data is at the
    /// game's version now, and the next save says so.
    pub fn finish_migration(&mut self) {
        if self.migrate_from.take().is_some() {
            self.set_version(self.game_version);
        }
    }

    // ─── Slots ────────────────────────────────────────────

    /// Tell the store where slots are kept.
    pub fn set_backend(&mut self, backend: Box<dyn SaveBackend>) {
        self.backend = Some(backend);
    }

    pub fn has_backend(&self) -> bool {
        self.backend.is_some()
    }

    /// The slot held.
    pub fn slot(&self) -> &str {
        &self.slot
    }

    /// The slots that have something saved, in name order. The slot held
    /// is in the list only once it has been saved.
    pub fn slots(&self) -> Vec<String> {
        let mut slots = self.backend.as_ref().map_or_else(Vec::new, |b| b.slots());
        slots.sort();
        slots.dedup();
        slots
    }

    /// The file (or storage key) the slot held is saved to.
    pub fn place(&self) -> Option<String> {
        self.backend.as_ref().map(|b| b.place(&self.slot))
    }

    /// The slot held didn't parse when it was loaded.
    pub fn corrupt(&self) -> Option<&CorruptSave> {
        self.corrupt.as_ref()
    }

    /// Why saving the slot held is refused, when it is.
    pub fn write_blocked(&self) -> Option<&str> {
        self.write_blocked.as_deref()
    }

    /// Replace what the store holds with the saved slot.
    pub fn load(&mut self) -> Loaded {
        self.data.clear();
        self.dirty = false;
        self.corrupt = None;
        self.write_blocked = None;
        self.migrate_from = None;
        self.version = self.game_version;
        let Some(backend) = self.backend.as_ref() else {
            return Loaded::NoBackend;
        };
        let place = backend.place(&self.slot);
        let saved = match backend.read(&self.slot) {
            Ok(None) => return Loaded::Fresh { place },
            Ok(Some(saved)) => saved,
            Err(error) => {
                self.write_blocked = Some(format!("{place} could not be read ({error})"));
                return Loaded::Unreadable { place, error };
            }
        };
        match parse(&saved.text) {
            Ok((data, version)) => {
                self.data = data;
                self.version = version;
                if version < self.game_version {
                    self.migrate_from = Some(version);
                }
                // Read from an old place: owed to the new one
                self.dirty = saved.old_place;
                Loaded::Read {
                    keys: self.data.len(),
                    from: saved.from,
                    version,
                    moved_to: saved.old_place.then_some(place),
                }
            }
            Err(error) => {
                let backup = if saved.old_place {
                    // Never written to, so it is its own evidence
                    saved.from.clone()
                } else {
                    match backend.set_aside(&self.slot) {
                        Ok(backup) => backup,
                        Err(e) => {
                            self.write_blocked = Some(format!(
                                "{} did not parse and could not be set aside ({e})",
                                saved.from
                            ));
                            saved.from.clone()
                        }
                    }
                };
                let corrupt = CorruptSave {
                    slot: self.slot.clone(),
                    file: saved.from,
                    backup,
                    error,
                };
                self.corrupt = Some(corrupt.clone());
                Loaded::Corrupt(corrupt)
            }
        }
    }

    /// Write the slot held, in one step, and say where. Refused for a slot
    /// whose unreadable file is still in the way.
    pub fn save(&mut self) -> Result<String, String> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| "no save location".to_string())?;
        if let Some(why) = &self.write_blocked {
            return Err(format!("not saved: {why}"));
        }
        let text = self.to_toml_string()?;
        backend.write(&self.slot, &text)?;
        self.dirty = false;
        Ok(backend.place(&self.slot))
    }

    /// Hold another slot: the one held is saved first if it has unsaved
    /// changes (a failure there refuses the switch, so nothing is lost),
    /// then `name` is loaded, or begun empty when nothing is saved for it.
    /// Naming the slot already held does nothing.
    pub fn open_slot(&mut self, name: &str) -> Result<Loaded, String> {
        check_slot_name(name)?;
        if self.backend.is_none() {
            return Err("there is no save location in this run, so no slots".into());
        }
        if name == self.slot {
            return Ok(Loaded::Fresh {
                place: self.place().unwrap_or_default(),
            });
        }
        if self.dirty && self.write_blocked.is_none() {
            self.save()
                .map_err(|e| format!("slot '{}' could not be saved first: {e}", self.slot))?;
        }
        self.slot = name.to_string();
        Ok(self.load())
    }

    // ─── Text and files ───────────────────────────────────

    /// The store as TOML text, the form it is saved in.
    pub fn to_toml_string(&self) -> Result<String, String> {
        let mut table = toml::map::Map::new();
        let mut meta = toml::map::Map::new();
        for (k, v) in &self.data {
            if k == META_KEY {
                meta.insert(META_GAME_KEY.into(), v.clone());
            } else {
                table.insert(k.clone(), v.clone());
            }
        }
        // A version 0 store with no key called `meta` is written exactly
        // as it was before versions
        if self.version != 0 || !meta.is_empty() {
            meta.insert(META_VERSION.into(), toml::Value::Integer(self.version));
            table.insert(META_KEY.into(), toml::Value::Table(meta));
        }
        toml::to_string_pretty(&table).map_err(|e| format!("serialize error: {e}"))
    }

    /// Replace all current data with the store saved as `content`. On an
    /// error the store is as it was.
    pub fn load_from_str(&mut self, content: &str) -> Result<(), String> {
        let (data, version) = parse(content)?;
        self.data = data;
        self.version = version;
        self.migrate_from = (version < self.game_version).then_some(version);
        self.dirty = false;
        Ok(())
    }

    /// Save the store to a TOML file of the caller's choosing (temp file,
    /// then rename: [`crate::atomic_file`]).
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let content = self.to_toml_string()?;
        crate::atomic_file::write(path, &content).map_err(|e| format!("write error: {e}"))
    }

    /// Load the store from a TOML file, replacing all current data.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), String> {
        let content =
            flint_core::vfs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
        self.load_from_str(&content)
    }
}

/// A saved store's game keys and its version.
fn parse(content: &str) -> Result<(HashMap<String, toml::Value>, i64), String> {
    let table: toml::map::Map<String, toml::Value> =
        toml::from_str(content).map_err(|e| format!("parse error: {e}"))?;
    let mut data: HashMap<String, toml::Value> = table.into_iter().collect();
    let mut version = 0;
    let engine_meta = data.get(META_KEY).and_then(|meta| {
        let meta = meta.as_table()?;
        let version = meta.get(META_VERSION)?.as_integer()?;
        meta.keys()
            .all(|k| k == META_VERSION || k == META_GAME_KEY)
            .then_some(version)
    });
    if let Some(v) = engine_meta {
        version = v;
        let meta = data.remove(META_KEY);
        if let Some(toml::Value::Table(mut meta)) = meta {
            if let Some(game_key) = meta.remove(META_GAME_KEY) {
                data.insert(META_KEY.into(), game_key);
            }
        }
    }
    Ok((data, version))
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
    fn toml_string_round_trip() {
        let mut store = PersistentStore::new();
        store.set(
            "selected_track",
            toml::Value::String("scenes/oval_plus.scene.toml".into()),
        );
        store.set("best", toml::Value::Float(69.29));
        let text = store.to_toml_string().unwrap();

        let mut back = PersistentStore::new();
        back.set("stale", toml::Value::Boolean(true));
        back.load_from_str(&text).unwrap();
        assert!(!back.has("stale"));
        assert!(!back.is_dirty());
        assert_eq!(back.get("best"), Some(&toml::Value::Float(69.29)));
        assert_eq!(
            back.get("selected_track"),
            Some(&toml::Value::String("scenes/oval_plus.scene.toml".into()))
        );
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

    /// #726 reproduction: a save must never write into the bytes of the file
    /// it replaces. A hard link to the old file is a second name for the
    /// same bytes: a write through the final path changes what the witness
    /// reads (and a cut mid-write would leave it half written); a temp file
    /// renamed over the path leaves the old bytes whole.
    #[test]
    fn a_save_never_writes_into_the_file_it_replaces() {
        let dir = std::env::temp_dir().join(format!("flint_persist_atomic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("persist.toml");
        let witness = dir.join("witness");
        std::fs::write(
            &path,
            "coins = 7
",
        )
        .unwrap();
        std::fs::hard_link(&path, &witness).unwrap();

        let mut store = PersistentStore::new();
        store.set("coins", toml::Value::Integer(8));
        store.save_to_file(&path).expect("save failed");

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "coins = 8
"
        );
        assert_eq!(
            std::fs::read_to_string(&witness).unwrap(),
            "coins = 7
",
            "the old file's bytes were written over in place"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── #726: the saved form, versions, slots, a corrupt slot ─────────

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// Slots in memory, shared with the test so it can look at them.
    #[derive(Clone, Default)]
    struct Memory {
        files: Arc<Mutex<BTreeMap<String, String>>>,
        refuse_set_aside: bool,
        refuse_write: Arc<Mutex<bool>>,
    }

    impl Memory {
        fn with(files: &[(&str, &str)]) -> Self {
            let memory = Memory::default();
            for (name, text) in files {
                memory.put(name, text);
            }
            memory
        }
        fn put(&self, name: &str, text: &str) {
            self.files.lock().unwrap().insert(name.into(), text.into());
        }
        fn get(&self, name: &str) -> Option<String> {
            self.files.lock().unwrap().get(name).cloned()
        }
        fn names(&self) -> Vec<String> {
            self.files.lock().unwrap().keys().cloned().collect()
        }
        fn store(&self) -> PersistentStore {
            let mut store = PersistentStore::new();
            store.set_backend(Box::new(self.clone()));
            store
        }
    }

    impl SaveBackend for Memory {
        fn read(&self, slot: &str) -> Result<Option<SaveText>, String> {
            let from = self.place(slot);
            if let Some(text) = self.get(&from) {
                return Ok(Some(SaveText {
                    text,
                    from,
                    old_place: false,
                }));
            }
            // The default slot alone has an old place
            Ok(self
                .get("old/persist.toml")
                .filter(|_| slot == DEFAULT_SLOT)
                .map(|text| SaveText {
                    text,
                    from: "old/persist.toml".into(),
                    old_place: true,
                }))
        }
        fn write(&self, slot: &str, text: &str) -> Result<(), String> {
            if *self.refuse_write.lock().unwrap() {
                return Err("the disk is full".into());
            }
            self.put(&self.place(slot), text);
            Ok(())
        }
        fn set_aside(&self, slot: &str) -> Result<String, String> {
            if self.refuse_set_aside {
                return Err("read-only".into());
            }
            let from = self.place(slot);
            let backup = format!("{from}.bak");
            let text = self.files.lock().unwrap().remove(&from).unwrap();
            self.put(&backup, &text);
            Ok(backup)
        }
        fn slots(&self) -> Vec<String> {
            self.names()
                .iter()
                .filter_map(|n| n.strip_suffix(".toml"))
                .filter(|n| check_slot_name(n).is_ok())
                .map(str::to_string)
                .collect()
        }
        fn place(&self, slot: &str) -> String {
            format!("{slot}.toml")
        }
    }

    #[test]
    fn slot_names_are_file_names_on_every_platform() {
        for good in ["default", "a", "slot-2", "new_game_plus", "0", "com10"] {
            assert_eq!(check_slot_name(good), Ok(()), "{good}");
        }
        let long = "a".repeat(33);
        for bad in [
            "", "..", "a/b", "a\\b", "../up", "a.b", "Save", "a b", "_a", "-a", "persist", "con",
            "nul", "com1", "lpt9", "ü", &long,
        ] {
            let err = check_slot_name(bad).unwrap_err();
            assert!(err.contains("can't be a save slot"), "{bad}: {err}");
        }
        assert_eq!(check_slot_name(&"a".repeat(32)), Ok(()));
    }

    /// The form written before versions is the form written at version 0,
    /// byte for byte, and is read as version 0 with every key the game's.
    #[test]
    fn a_file_with_no_meta_is_version_0_and_is_written_back_the_same() {
        let old = "best = 69.29\ncoins = 7\n\n[garage]\nkart = \"red\"\n";
        let mut store = PersistentStore::new();
        store.load_from_str(old).unwrap();
        assert_eq!(store.version(), 0);
        let mut keys = store.keys();
        keys.sort();
        assert_eq!(keys, ["best", "coins", "garage"]);
        assert_eq!(store.to_toml_string().unwrap(), old);
    }

    #[test]
    fn the_version_is_saved_in_meta_and_is_not_a_game_key() {
        let mut store = PersistentStore::new();
        store.set("coins", toml::Value::Integer(7));
        store.set_version(3);
        let text = store.to_toml_string().unwrap();
        assert_eq!(text, "coins = 7\n\n[meta]\nversion = 3\n");

        let mut back = PersistentStore::new();
        back.load_from_str(&text).unwrap();
        assert_eq!(back.version(), 3);
        assert_eq!(back.keys(), ["coins"]);
        assert!(!back.has("meta"));
    }

    /// A game that already keeps a key called `meta` keeps it: read from
    /// its old top-level place, saved inside the engine's table from then
    /// on, and the same value either way.
    #[test]
    fn a_game_key_called_meta_keeps_working() {
        for old in [
            "coins = 1\nmeta = \"seen the intro\"\n",
            "coins = 1\n\n[meta]\nlaps = 3\n",
            // An integer `version` beside other keys is still the game's
            "coins = 1\n\n[meta]\nlaps = 3\nversion = 9\n",
        ] {
            let mut store = PersistentStore::new();
            store.load_from_str(old).unwrap();
            assert_eq!(store.version(), 0, "{old}");
            let was = store.get("meta").cloned().expect("the game's key");
            store.set_version(2);

            let text = store.to_toml_string().unwrap();
            let mut back = PersistentStore::new();
            back.load_from_str(&text).unwrap();
            assert_eq!(back.version(), 2);
            assert_eq!(back.get("meta"), Some(&was), "{text}");
            assert_eq!(back.get("coins"), Some(&toml::Value::Integer(1)));
        }
        // Set at run time, at version 0: `[meta]` is written to carry it
        let mut store = PersistentStore::new();
        store.set("meta", toml::Value::Integer(5));
        let text = store.to_toml_string().unwrap();
        assert_eq!(text, "[meta]\nkey_meta = 5\nversion = 0\n");
        let mut back = PersistentStore::new();
        back.load_from_str(&text).unwrap();
        assert_eq!(back.get("meta"), Some(&toml::Value::Integer(5)));
    }

    #[test]
    fn a_bad_load_from_str_leaves_the_store_as_it_was() {
        let mut store = PersistentStore::new();
        store.set("coins", toml::Value::Integer(7));
        assert!(store.load_from_str("coins = [1, 2").is_err());
        assert_eq!(store.get("coins"), Some(&toml::Value::Integer(7)));
    }

    #[test]
    fn a_fresh_store_is_at_the_games_version_and_an_older_file_waits_to_migrate() {
        // Nothing saved: nothing to migrate
        let memory = Memory::default();
        let mut store = memory.store();
        store.set_game_version(2);
        assert!(matches!(store.load(), Loaded::Fresh { .. }));
        assert_eq!((store.version(), store.pending_migration()), (2, None));
        assert!(!store.is_dirty());

        // A file from before versions, in a game now at 2
        let memory = Memory::with(&[("default.toml", "coins = 7\n")]);
        let mut store = memory.store();
        store.set_game_version(2);
        assert_eq!(
            store.load(),
            Loaded::Read {
                keys: 1,
                from: "default.toml".into(),
                version: 0,
                moved_to: None
            }
        );
        assert_eq!((store.version(), store.pending_migration()), (0, Some(0)));
        // Until a script has migrated it, a save keeps the old version
        store.set("coins", toml::Value::Integer(8));
        store.save().unwrap();
        assert_eq!(memory.get("default.toml").unwrap(), "coins = 8\n");

        store.finish_migration();
        assert_eq!((store.version(), store.pending_migration()), (2, None));
        assert!(store.is_dirty(), "the new version is owed to the file");
        store.save().unwrap();
        assert_eq!(
            memory.get("default.toml").unwrap(),
            "coins = 8\n\n[meta]\nversion = 2\n"
        );

        // A save from a newer build is not migrated and keeps its version
        let memory = Memory::with(&[("default.toml", "[meta]\nversion = 5\n")]);
        let mut store = memory.store();
        store.set_game_version(2);
        store.load();
        assert_eq!((store.version(), store.pending_migration()), (5, None));
    }

    #[test]
    fn slots_are_separate_files_and_switching_saves_the_one_held_first() {
        let memory = Memory::with(&[("default.toml", "coins = 7\n")]);
        let mut store = memory.store();
        store.load();
        assert_eq!(store.slot(), DEFAULT_SLOT);
        assert_eq!(store.slots(), ["default"]);

        store.set("coins", toml::Value::Integer(8));
        assert!(matches!(store.open_slot("b"), Ok(Loaded::Fresh { .. })));
        assert_eq!(store.slot(), "b");
        assert_eq!(memory.get("default.toml").unwrap(), "coins = 8\n");
        assert!(store.keys().is_empty(), "a new slot begins empty");
        assert_eq!(store.slots(), ["default"], "not saved yet, so not listed");

        store.set("coins", toml::Value::Integer(100));
        store.save().unwrap();
        assert_eq!(store.slots(), ["b", "default"]);

        store.open_slot("default").unwrap();
        assert_eq!(store.get("coins"), Some(&toml::Value::Integer(8)));
        assert!(!store.is_dirty());

        // A bad name changes nothing
        let err = store.open_slot("../b").unwrap_err();
        assert!(err.contains("can't be a save slot"), "{err}");
        assert_eq!(store.slot(), "default");
        assert_eq!(memory.names(), ["b.toml", "default.toml"]);
    }

    #[test]
    fn a_switch_is_refused_when_the_slot_held_cannot_be_saved() {
        let memory = Memory::default();
        let mut store = memory.store();
        store.load();
        store.set("coins", toml::Value::Integer(8));
        *memory.refuse_write.lock().unwrap() = true;
        let err = store.open_slot("b").unwrap_err();
        assert_eq!(
            err,
            "slot 'default' could not be saved first: the disk is full"
        );
        assert_eq!(store.slot(), "default");
        assert_eq!(store.get("coins"), Some(&toml::Value::Integer(8)));
    }

    #[test]
    fn a_store_with_no_backend_has_no_slots() {
        let mut store = PersistentStore::new();
        assert_eq!(store.load(), Loaded::NoBackend);
        assert!(store.open_slot("b").is_err());
        assert!(store.save().is_err());
        assert!(store.slots().is_empty());
        assert_eq!(store.slot(), DEFAULT_SLOT);
    }

    /// The second done-when box, at the store: a slot that doesn't parse
    /// is set aside before anything can be saved over it, and reported.
    #[test]
    fn a_corrupt_slot_is_set_aside_reported_and_survives_the_next_save() {
        let garbage = "coins = 7\nbest_lap = [61.2, 59.8\n";
        let memory = Memory::with(&[("default.toml", garbage)]);
        let mut store = memory.store();
        let loaded = store.load();
        let Loaded::Corrupt(report) = &loaded else {
            panic!("{loaded:?}");
        };
        assert!(loaded.is_error());
        assert_eq!(report.slot, "default");
        assert_eq!(report.file, "default.toml");
        assert_eq!(report.backup, "default.toml.bak");
        assert!(report.error.starts_with("parse error"), "{}", report.error);
        assert_eq!(store.corrupt(), Some(report));
        let said = loaded.to_string();
        assert!(
            said.contains("default.toml ") && said.contains("default.toml.bak"),
            "both paths are named: {said}"
        );
        assert!(store.keys().is_empty());

        store.set("coins", toml::Value::Integer(0));
        store.save().unwrap();
        assert_eq!(memory.get("default.toml").unwrap(), "coins = 0\n");
        assert_eq!(memory.get("default.toml.bak").unwrap(), garbage);

        // Another slot is its own story
        store.open_slot("b").unwrap();
        assert_eq!(store.corrupt(), None);
    }

    #[test]
    fn a_corrupt_slot_that_cannot_be_set_aside_is_never_saved_over() {
        let garbage = "coins = [";
        let mut memory = Memory::with(&[("default.toml", garbage)]);
        memory.refuse_set_aside = true;
        let mut store = memory.store();
        let Loaded::Corrupt(report) = store.load() else {
            panic!()
        };
        assert_eq!(report.backup, "default.toml", "left where it is");

        store.set("coins", toml::Value::Integer(0));
        let err = store.save().unwrap_err();
        assert!(err.contains("could not be set aside"), "{err}");
        assert_eq!(memory.get("default.toml").unwrap(), garbage);
        assert!(store.is_dirty());
        // Leaving for another slot doesn't try to save this one
        assert!(store.open_slot("b").is_ok());
    }

    #[test]
    fn a_save_read_from_an_old_place_is_owed_to_the_new_one() {
        let memory = Memory::with(&[("old/persist.toml", "coins = 7\n")]);
        let mut store = memory.store();
        let loaded = store.load();
        assert!(loaded.is_move(), "{loaded:?}");
        assert!(loaded.to_string().contains("now lives at default.toml"));
        assert!(store.is_dirty());
        store.save().unwrap();
        assert_eq!(memory.get("default.toml").unwrap(), "coins = 7\n");
        assert_eq!(memory.get("old/persist.toml").unwrap(), "coins = 7\n");
        assert!(!store.load().is_move(), "the new place wins from then on");

        // An unreadable file at the old place is left there: nothing is
        // ever written to it
        let memory = Memory::with(&[("old/persist.toml", "coins = [")]);
        let mut store = memory.store();
        let Loaded::Corrupt(report) = store.load() else {
            panic!()
        };
        assert_eq!(report.backup, "old/persist.toml");
        store.set("coins", toml::Value::Integer(0));
        store.save().unwrap();
        assert_eq!(memory.names(), ["default.toml", "old/persist.toml"]);
    }
}
