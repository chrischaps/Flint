//! The schema registry the player keeps (ADR 0115, #683).
//!
//! The registry is read once and held: a chunk uses it as it is, and a
//! transition reads the disk again only when the next scene's schema
//! directories differ from the ones held. Whatever goes wrong with that
//! read, the registry already held stays in use, so a scene never loads
//! with an empty one: its archetype and schema defaults would vanish with
//! nothing said.
//!
//! What is wrong with a game's own schema files (`old-type-name`,
//! `unknown-type`, `ignored-schema-key`: `SchemaRegistry::game_lints`) is
//! said when a registry is first held, each file's through the summary a
//! scene load prints (#1064), and never twice: a transition that reads no
//! new directory says nothing, and one that adds directories says only what
//! their files add.

use flint_core::diagnostic::{self, Diagnostic};
use flint_schema::SchemaRegistry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The command a schema lint's summary points to: what prints every one.
const FULL_LIST: &str = "flint check";

/// See the module docs. `PlayerApp::set_schemas` fills it; a host that only
/// named directories (`set_schema_dirs`, `set_schema_paths`) has them read
/// on first use.
#[derive(Default)]
pub(super) struct HeldSchemas {
    registry: Option<Arc<SchemaRegistry>>,
    /// The directories `registry` holds, in load order.
    dirs: Vec<PathBuf>,
    /// How many times a directory list was read from the file source.
    reads: usize,
    /// Every game schema lint said so far, as its one line: kept across
    /// registries (and `forget`), so the same mistake is said once a run.
    said: HashSet<String>,
}

impl HeldSchemas {
    /// Hold `registry`, which the host read from `dirs` (possibly over
    /// schemas of its own, as the web player's compiled-in engine set), and
    /// say the game lints of it not said before.
    pub(super) fn set(&mut self, registry: SchemaRegistry, dirs: &[PathBuf]) {
        let lints = self.unsaid(&registry);
        say(&lints);
        self.registry = Some(Arc::new(registry));
        self.dirs = dirs.to_vec();
    }

    /// The game lints of `registry` no registry held before had, marked
    /// said.
    fn unsaid(&mut self, registry: &SchemaRegistry) -> Vec<Diagnostic> {
        registry
            .game_lints()
            .into_iter()
            .filter(|d| self.said.insert(d.one_line()))
            .collect()
    }

    /// The host named new directories without a registry for them: what is
    /// held no longer answers for them, so the next use reads them.
    pub(super) fn forget(&mut self) {
        self.registry = None;
        self.dirs.clear();
    }

    /// The registry for a scene whose schema directories are `dirs`.
    ///
    /// - The list held: the registry held, with no read.
    /// - The list held plus more (a discovered or host list grows by what
    ///   lies beside the new scene): the added directories, over a copy of
    ///   the registry held.
    /// - Another list (the scene belongs to another project): read whole.
    ///
    /// A read that fails is logged with what is kept, and the registry held
    /// is returned as it was.
    pub(super) fn for_dirs(&mut self, dirs: &[PathBuf]) -> Arc<SchemaRegistry> {
        let Some(held) = self.registry.clone() else {
            return self.read_first(dirs);
        };
        if dirs == self.dirs {
            return held;
        }
        self.reads += 1;
        let next = if dirs.starts_with(&self.dirs) {
            let mut next = (*held).clone();
            dirs[self.dirs.len()..]
                .iter()
                .try_for_each(|dir| next.load_directory(dir).map_err(|e| (dir.as_path(), e)))
                .map(|()| next)
                .map_err(|(dir, e)| format!("{}: {e}", shown(dir)))
        } else {
            SchemaRegistry::load_from_directories(dirs).map_err(|e| e.to_string())
        };
        match next {
            Ok(next) => {
                self.set(next, dirs);
                self.current()
            }
            Err(e) => {
                tracing::error!(
                    "Failed to load schemas ({e}); keeping the {} component and {} archetype \
                     schemas already loaded",
                    held.component_names().len(),
                    held.archetype_names().len()
                );
                held
            }
        }
    }

    /// The registry held, for what belongs to the current scene (a chunk):
    /// never a read once there is one. `dirs` is what to read if the host
    /// handed over none.
    pub(super) fn held_or_read(&mut self, dirs: &[PathBuf]) -> Arc<SchemaRegistry> {
        match &self.registry {
            Some(held) => held.clone(),
            None => self.read_first(dirs),
        }
    }

    fn current(&self) -> Arc<SchemaRegistry> {
        self.registry.clone().unwrap_or_default()
    }

    /// Nothing is held yet: read `dirs`. With nothing to fall back on, a
    /// failure is an error in the log and an empty registry for this load
    /// only (the scene's diagnostics then say `no-schemas`); the next load
    /// tries again.
    fn read_first(&mut self, dirs: &[PathBuf]) -> Arc<SchemaRegistry> {
        self.reads += 1;
        match SchemaRegistry::load_from_directories(dirs) {
            Ok(registry) => {
                self.set(registry, dirs);
                self.current()
            }
            Err(e) => {
                tracing::error!(
                    "Failed to load schemas: {e}. No schemas are loaded: archetype and schema \
                     defaults are missing from this scene"
                );
                Arc::new(SchemaRegistry::new())
            }
        }
    }

    #[cfg(test)]
    pub(super) fn reads(&self) -> usize {
        self.reads
    }
}

fn shown(path: &Path) -> String {
    flint_core::project::shown(path)
}

/// `lints` as a scene load says its diagnostics, one summary a schema file
/// (in the order the files were loaded), so each line names its file.
fn say(lints: &[Diagnostic]) {
    let mut files: Vec<(Option<&Path>, Vec<Diagnostic>)> = Vec::new();
    for lint in lints {
        let file = lint.file.as_deref();
        match files.iter_mut().find(|(f, _)| *f == file) {
            Some((_, list)) => list.push(lint.clone()),
            None => files.push((file, vec![lint.clone()])),
        }
    }
    for (file, list) in files {
        // A schema read from text (no file) is the host's own
        let subject = file.map_or_else(|| "schemas".to_string(), |f| shown(&from_here(f)));
        diagnostic::log_summary_of(&subject, FULL_LIST, &list);
    }
}

/// `path` from the working directory when it is under it, as `flint check`
/// spells its files: a manifest's directories are joined to the project's
/// root, which is absolute.
fn from_here(path: &Path) -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|here| path.strip_prefix(here).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::{self, MemorySource};

    const LAMP: &str = "[component.lamp]\n[component.lamp.fields]\non = \"bool\"\n";
    const DOOR: &str = "[archetype.door]\ncomponents = [\"lamp\"]\n";
    const LOOT: &str = "[component.loot]\n[component.loot.fields]\ngold = \"i32\"\n";

    fn files(extra: &[(&str, &str)]) -> Arc<MemorySource> {
        let mut source = MemorySource::new();
        source.insert("engine/schemas/components/lamp.toml", LAMP);
        source.insert("engine/schemas/archetypes/door.toml", DOOR);
        source.insert("schemas/components/loot.toml", LOOT);
        for (path, text) in extra {
            source.insert(*path, *text);
        }
        Arc::new(source)
    }

    fn dirs(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// The ticket's third box: the registry is read once, a transition
    /// inside one project reads nothing, and a chunk never does.
    #[test]
    fn the_registry_is_read_once_and_kept() {
        vfs::scoped(files(&[]), || {
            let list = dirs(&["engine/schemas", "schemas"]);
            let mut held = HeldSchemas::default();
            held.set(SchemaRegistry::load_from_directories(&list).unwrap(), &list);
            let first = held.for_dirs(&list);
            let second = held.for_dirs(&list);
            let chunk = held.held_or_read(&list);
            assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &chunk));
            assert_eq!(held.reads(), 0, "the same directories are never re-read");
            assert!(first.get_archetype("door").is_some());
        });
    }

    /// A host that named directories and no registry (`set_schema_paths`):
    /// one read, on first use.
    #[test]
    fn directories_alone_are_read_on_first_use() {
        vfs::scoped(files(&[]), || {
            let list = dirs(&["engine/schemas"]);
            let mut held = HeldSchemas::default();
            assert!(held.held_or_read(&list).get_component("lamp").is_some());
            assert!(held.for_dirs(&list).get_component("lamp").is_some());
            assert_eq!(held.reads(), 1);
        });
    }

    /// A list that grew is the registry held plus the new directories, so
    /// schemas the host loaded from no directory (the web player's
    /// compiled-in engine set) survive the transition.
    #[test]
    fn a_grown_list_adds_to_what_is_held() {
        vfs::scoped(files(&[]), || {
            let mut compiled_in = SchemaRegistry::new();
            compiled_in.load_component_string(LAMP).unwrap();
            let mut held = HeldSchemas::default();
            held.set(compiled_in, &[]);
            let next = held.for_dirs(&dirs(&["schemas"]));
            assert!(next.get_component("lamp").is_some(), "kept");
            assert!(next.get_component("loot").is_some(), "added");
        });
    }

    /// The bug this replaces: a transition whose schemas failed to read
    /// loaded its scene with an empty registry and said nothing. Now the
    /// registry held is what the scene gets, whichever way the list changed.
    #[test]
    fn a_failed_read_keeps_the_registry_held() {
        let broken = [("other/schemas/components/bad.toml", "[component.bad\n")];
        vfs::scoped(files(&broken), || {
            let list = dirs(&["engine/schemas"]);
            let mut held = HeldSchemas::default();
            held.set(SchemaRegistry::load_from_directories(&list).unwrap(), &list);

            // Another project's list, unreadable
            let other = held.for_dirs(&dirs(&["other/schemas"]));
            assert!(other.get_component("lamp").is_some());
            assert!(other.get_archetype("door").is_some());
            // The same list grown by an unreadable directory
            let grown = held.for_dirs(&dirs(&["engine/schemas", "other/schemas"]));
            assert!(grown.get_archetype("door").is_some());
            // ...and the list held is still the good one: no read for it
            let reads = held.reads();
            assert!(held.for_dirs(&list).get_component("lamp").is_some());
            assert_eq!(held.reads(), reads);
        });
    }

    /// What `run` logged at warn and above, without the subscriber's
    /// decoration.
    fn warned(run: impl FnOnce()) -> String {
        use std::sync::Mutex;
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let log = Captured::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_level(false)
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        text
    }

    const OLD_TORCH: &str = "[component.torch.fields]\nwatts = { type = \"float\" }\n";
    const OLD_SAIL: &str = "[component.sail.fields]\nfurled = { type = \"boolean\" }\n";

    /// The ticket's box (#1064): a game whose schema spells a type the old
    /// way is told at start, once, naming the file and the field, and a
    /// transition that reads no new directory says nothing.
    #[test]
    fn a_games_schema_lint_is_said_once_at_start() {
        let game = [("schemas/components/torch.toml", OLD_TORCH)];
        vfs::scoped(files(&game), || {
            let list = dirs(&["engine/schemas", "schemas"]);
            let mut held = HeldSchemas::default();
            let start =
                warned(|| held.set(SchemaRegistry::load_from_directories(&list).unwrap(), &list));
            assert_eq!(
                start.lines().map(str::trim).collect::<Vec<_>>(),
                [
                    "schemas/components/torch.toml: 1 warning (the full list: flint check)",
                    "warning[old-type-name] torch.watts: `float` is an old spelling of `f32` \
                     (write type = \"f32\"; `float` still loads)",
                ],
                "{start}"
            );
            let transition = warned(|| {
                held.for_dirs(&list);
                held.held_or_read(&list);
            });
            assert_eq!(transition, "", "no new directory, nothing said");
        });
    }

    /// A list that grows says what the added directory's files add, not
    /// again what was said at start; a host that names only directories is
    /// told on the first read.
    #[test]
    fn a_grown_list_says_only_its_new_files_lints() {
        let game = [
            ("schemas/components/torch.toml", OLD_TORCH),
            ("boat/schemas/components/sail.toml", OLD_SAIL),
        ];
        vfs::scoped(files(&game), || {
            let list = dirs(&["engine/schemas", "schemas"]);
            let mut held = HeldSchemas::default();
            let first = warned(|| {
                held.held_or_read(&list);
            });
            assert!(first.contains("schemas/components/torch.toml"), "{first}");
            let grown = warned(|| {
                held.for_dirs(&dirs(&["engine/schemas", "schemas", "boat/schemas"]));
            });
            assert!(
                grown.contains("boat/schemas/components/sail.toml: 1 warning")
                    && grown.contains("sail.furled"),
                "{grown}"
            );
            assert!(!grown.contains("torch"), "said at start already: {grown}");
        });
    }

    /// The engine's own files are rewritten rather than warned about (ADR
    /// 0115 decision 3): a lint in an engine checkout is not the game's.
    #[test]
    fn an_engine_files_lint_is_not_said() {
        let engine = [
            ("engine/crates/flint-core/Cargo.toml", ""),
            ("engine/schemas/components/torch.toml", OLD_TORCH),
        ];
        vfs::scoped(files(&engine), || {
            let list = dirs(&["engine/schemas"]);
            let mut held = HeldSchemas::default();
            let said =
                warned(|| held.set(SchemaRegistry::load_from_directories(&list).unwrap(), &list));
            assert_eq!(said, "");
        });
    }

    #[test]
    fn another_projects_list_replaces_the_registry() {
        vfs::scoped(files(&[]), || {
            let list = dirs(&["engine/schemas"]);
            let mut held = HeldSchemas::default();
            held.set(SchemaRegistry::load_from_directories(&list).unwrap(), &list);
            let next = held.for_dirs(&dirs(&["schemas"]));
            assert!(next.get_component("loot").is_some());
            assert!(next.get_component("lamp").is_none(), "not this project's");
        });
    }
}
