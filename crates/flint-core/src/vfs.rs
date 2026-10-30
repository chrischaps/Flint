//! A read-only file layer for the runtime.
//!
//! Every file the engine reads while a game runs (scenes, schemas, scripts,
//! models, textures, audio, charts) goes through these functions instead of
//! `std::fs`. On desktop the source is [`DiskSource`] and nothing changes:
//! each call is the `std::fs` call it replaced. A browser has no file system,
//! so the web build installs a [`MemorySource`] filled from a fetched bundle
//! before the player starts, and the same loaders read from it.
//!
//! ```
//! use flint_core::vfs::{self, MemorySource};
//! use std::sync::Arc;
//!
//! let mut bundle = MemorySource::new();
//! bundle.insert("levels/demo.scene.toml", "[scene]\nname = \"demo\"\n");
//! vfs::scoped(Arc::new(bundle), || {
//!     assert!(vfs::is_dir("levels"));
//!     assert!(vfs::read_to_string("./levels/demo.scene.toml").unwrap().contains("demo"));
//! });
//! ```
//!
//! **Paths** are the ones the runtime already uses: relative to the working
//! directory (the game root), with `..` for prefab and schema references and
//! either slash on Windows. [`MemorySource`] keys them by [`normalize`]:
//! forward slashes, `.` dropped and `..` folded, so `levels/../models\a.glb`
//! and `models/a.glb` are the same file. [`read_dir`] returns each entry as
//! the directory path the caller passed joined with the entry's name, the way
//! `std::fs::read_dir(..).path()` does.
//!
//! **Writes are out of scope.** Scene saving, input-config overrides, save
//! games and replay recordings still write with `std::fs`, natively only.
//! Hot reload also stays on the disk: it polls `std::fs::metadata` for
//! modification times, which a memory source has none of, so on the web it
//! simply never fires.
//!
//! **Which source.** [`install`] sets the process-wide source (the web
//! player, once, at startup). [`scoped`] overrides it for the current thread
//! while a closure runs, which is what tests use so they don't leak a bundle
//! into other tests running in parallel.
//!
//! **Android** (`flint-android/src/asset_extractor.rs`) copies the APK's
//! assets into internal storage at first launch and then reads them from
//! disk. It could instead install a source backed by `AAssetManager` here and
//! skip the copy; nothing in this module stops that, but it is not done yet.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

/// What a path names, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

/// Somewhere files can be read from.
pub trait FileSource: Send + Sync {
    /// The whole file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// The entries directly inside a directory, each as `path.join(name)`.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// Whether `path` is a file, a directory or nothing.
    fn kind(&self, path: &Path) -> Option<EntryKind>;
}

/// The local disk, through `std::fs`. The default source.
#[derive(Debug, Default, Clone, Copy)]
pub struct DiskSource;

// The one place the runtime may touch `std::fs` for reading.
#[allow(clippy::disallowed_methods)]
impl FileSource for DiskSource {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        // An entry that can't be read is skipped, as the loops this replaced
        // (`entries.flatten()`) did.
        Ok(std::fs::read_dir(path)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect())
    }

    fn kind(&self, path: &Path) -> Option<EntryKind> {
        let meta = std::fs::metadata(path).ok()?;
        Some(if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        })
    }
}

/// Files held in memory, keyed by [`normalize`]d path. Directories exist
/// wherever a file's path passes through them.
#[derive(Debug, Default, Clone)]
pub struct MemorySource {
    files: BTreeMap<String, Arc<[u8]>>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a file.
    pub fn insert(&mut self, path: impl AsRef<Path>, bytes: impl Into<Vec<u8>>) {
        self.files
            .insert(normalize(path.as_ref()), Arc::from(bytes.into()));
    }

    /// How many files it holds.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every file's normalized path, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// The files under `dir/`, as `(name after the prefix, bytes)`.
    fn under<'a>(&'a self, dir: &str) -> impl Iterator<Item = (&'a str, &'a Arc<[u8]>)> + 'a {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let skip = prefix.len();
        self.files
            .range(prefix.clone()..)
            .take_while(move |(k, _)| k.starts_with(&prefix))
            .map(move |(k, v)| (&k[skip..], v))
            .filter(|(rest, _)| !rest.is_empty())
    }
}

impl<P: AsRef<Path>, B: Into<Vec<u8>>> FromIterator<(P, B)> for MemorySource {
    fn from_iter<I: IntoIterator<Item = (P, B)>>(iter: I) -> Self {
        let mut source = Self::new();
        for (path, bytes) in iter {
            source.insert(path, bytes);
        }
        source
    }
}

impl FileSource for MemorySource {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .get(&normalize(path))
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| not_found(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let dir = normalize(path);
        if self.files.contains_key(&dir) {
            return Err(io::Error::other(format!(
                "{} is a file, not a directory",
                path.display()
            )));
        }
        let mut names: Vec<&str> = self
            .under(&dir)
            .map(|(rest, _)| rest.split('/').next().unwrap_or(rest))
            .collect();
        names.dedup();
        if names.is_empty() {
            return Err(not_found(path));
        }
        Ok(names.into_iter().map(|name| path.join(name)).collect())
    }

    fn kind(&self, path: &Path) -> Option<EntryKind> {
        let key = normalize(path);
        if self.files.contains_key(&key) {
            Some(EntryKind::File)
        } else if self.under(&key).next().is_some() {
            Some(EntryKind::Dir)
        } else {
            None
        }
    }
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} is not in the file source", path.display()),
    )
}

/// The key a [`MemorySource`] files a path under: forward slashes, no `.`
/// components, each `..` folded into the component before it (a leading `..`
/// that has nothing to fold stays), no trailing slash. An absolute path keeps
/// its leading `/` (or drive prefix). The empty string is the root.
pub fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let absolute = text.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

// --- the active source ---

fn global() -> &'static RwLock<Arc<dyn FileSource>> {
    static GLOBAL: OnceLock<RwLock<Arc<dyn FileSource>>> = OnceLock::new();
    GLOBAL.get_or_init(|| RwLock::new(Arc::new(DiskSource)))
}

thread_local! {
    static SCOPED: RefCell<Vec<Arc<dyn FileSource>>> = const { RefCell::new(Vec::new()) };
}

/// Make `source` the process-wide source, replacing the disk (or whatever was
/// installed before).
pub fn install(source: Arc<dyn FileSource>) {
    *global().write().unwrap_or_else(|e| e.into_inner()) = source;
}

/// Run `f` with `source` as this thread's source, then restore the previous
/// one (even if `f` panics). Other threads are unaffected.
pub fn scoped<R>(source: Arc<dyn FileSource>, f: impl FnOnce() -> R) -> R {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPED.with(|s| s.borrow_mut().pop());
        }
    }
    SCOPED.with(|s| s.borrow_mut().push(source));
    let _pop = Pop;
    f()
}

/// The source reads go to right now on this thread.
pub fn current() -> Arc<dyn FileSource> {
    SCOPED
        .with(|s| s.borrow().last().cloned())
        .unwrap_or_else(|| global().read().unwrap_or_else(|e| e.into_inner()).clone())
}

// --- the calls the runtime makes ---

/// Read a whole file (`std::fs::read`).
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    current().read(path.as_ref())
}

/// Read a whole file as UTF-8 (`std::fs::read_to_string`).
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    String::from_utf8(read(path)?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not valid UTF-8", path.display()),
        )
    })
}

/// The entries directly inside a directory, each as `dir.join(name)`
/// (`std::fs::read_dir` and `DirEntry::path`). The disk lists them in its own
/// order, a memory source alphabetically; sort if order matters.
pub fn read_dir(dir: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
    current().read_dir(dir.as_ref())
}

/// Whether anything is at `path` (`Path::exists`).
pub fn exists(path: impl AsRef<Path>) -> bool {
    current().kind(path.as_ref()).is_some()
}

/// Whether `path` is a file (`Path::is_file`).
pub fn is_file(path: impl AsRef<Path>) -> bool {
    current().kind(path.as_ref()) == Some(EntryKind::File)
}

/// Whether `path` is a directory (`Path::is_dir`).
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    current().kind(path.as_ref()) == Some(EntryKind::Dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> Arc<MemorySource> {
        Arc::new(
            [
                ("levels/demo.scene.toml", "scene"),
                ("prefabs/kart.prefab.toml", "kart"),
                ("schemas/components/transform.toml", "t"),
                ("schemas/archetypes/player.toml", "p"),
                ("scripts/ui/hud.rhai", "hud"),
            ]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn normalize_folds_slashes_dots_and_parents() {
        let n = |s: &str| normalize(Path::new(s));
        assert_eq!(n("levels/demo.scene.toml"), "levels/demo.scene.toml");
        assert_eq!(n("./levels//demo.scene.toml"), "levels/demo.scene.toml");
        assert_eq!(
            n("levels\\..\\prefabs\\kart.prefab.toml"),
            "prefabs/kart.prefab.toml"
        );
        assert_eq!(n("levels/../../engine/schemas"), "../engine/schemas");
        assert_eq!(n("../.."), "../..");
        assert_eq!(n("schemas/"), "schemas");
        assert_eq!(n("."), "");
        assert_eq!(n("/abs/./x/../y"), "/abs/y");
        assert_eq!(n("/../y"), "/y");
    }

    #[test]
    fn memory_reads_through_prefab_relative_paths() {
        vfs_scoped(|| {
            // A prefab is looked up next to the scene, then one level up.
            let scene_dir = Path::new("levels");
            assert!(!exists(scene_dir.join("prefabs/kart.prefab.toml")));
            assert!(exists(scene_dir.join("../prefabs/kart.prefab.toml")));
            assert_eq!(
                read_to_string(
                    scene_dir
                        .join("..")
                        .join("prefabs")
                        .join("kart.prefab.toml")
                )
                .unwrap(),
                "kart"
            );
        });
    }

    #[test]
    fn memory_kinds_and_listing() {
        vfs_scoped(|| {
            assert!(is_dir("schemas"));
            assert!(is_dir("schemas/components/"));
            assert!(is_file("schemas/components/transform.toml"));
            assert!(!is_dir("schemas/components/transform.toml"));
            assert!(!exists("schemas/comp"), "a name prefix is not a directory");
            assert!(is_dir("."), "the root holds everything");

            let top = read_dir(".").unwrap();
            assert_eq!(
                top,
                ["levels", "prefabs", "schemas", "scripts"]
                    .map(|n| Path::new(".").join(n))
                    .to_vec()
            );
            // Entries keep the caller's spelling of the directory.
            let schemas = read_dir("schemas").unwrap();
            assert_eq!(
                schemas,
                vec![
                    PathBuf::from("schemas").join("archetypes"),
                    PathBuf::from("schemas").join("components"),
                ]
            );
            assert_eq!(
                read_dir("scripts").unwrap(),
                vec![PathBuf::from("scripts").join("ui")]
            );
            assert_eq!(
                read_dir("nope").unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            assert!(read_dir("scripts/ui/hud.rhai").is_err());
            assert_eq!(
                read("nope.toml").unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
        });
    }

    #[test]
    fn scoped_restores_the_previous_source_and_stays_on_its_thread() {
        let here = env!("CARGO_MANIFEST_DIR");
        assert!(is_dir(here), "the disk is the default");
        vfs_scoped(|| {
            assert!(!is_dir(here));
            let other = std::thread::spawn(move || is_dir(here)).join().unwrap();
            assert!(other, "another thread still sees the disk");
        });
        assert!(is_dir(here));
    }

    #[test]
    fn the_disk_lists_like_std_fs() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut ours = read_dir(&dir).unwrap();
        let mut std: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        ours.sort();
        std.sort();
        assert_eq!(ours, std);
        assert!(is_file(dir.join("vfs.rs")));
        assert!(read_to_string(dir.join("vfs.rs"))
            .unwrap()
            .contains("MemorySource"));
    }

    fn vfs_scoped(f: impl FnOnce()) {
        scoped(bundle(), f);
    }
}
