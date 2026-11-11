//! ScriptSync — entity discovery, .rhai file loading, hot-reload
//!
//! Scans the ECS world for entities with a `script` component and loads
//! the corresponding .rhai source files. Watches for file changes to
//! support live hot-reload during play.

use crate::engine::ScriptEngine;
use crate::module_resolver::ModuleBase;
use flint_core::components as comp;
use flint_core::file_watch;
use flint_core::project::{AssetKind, Project};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tracks which entities have scripts and manages file-based hot-reload
#[derive(Default)]
pub struct ScriptSync {
    /// Set of entity IDs that have been discovered and loaded
    pub(crate) discovered: HashSet<EntityId>,
    /// Source path → last modified time for hot-reload detection
    file_timestamps: HashMap<PathBuf, SystemTime>,
    /// Where scripts are looked for
    search: Option<ScriptSearch>,
    /// The file each script was loaded from, and the nearer places it
    /// wasn't found: a file that appears in one of them takes over.
    located: HashMap<EntityId, Located>,
    /// Snapshot of every `scripts/lib/**/*.rhai` the search can see → stamp.
    /// Any difference (edit, add, remove) invalidates the module cache and
    /// recompiles every script, since imports are resolved per script.
    lib_timestamps: BTreeMap<PathBuf, file_watch::Stamp>,
    /// Scripts seen changed and not yet recompiled, oldest first. On the
    /// caller's thread one is recompiled a call (#633), so a lib change
    /// doesn't compile them all in one frame; with a worker, all are sent.
    to_reload: Vec<(EntityId, PathBuf)>,
    /// Reads and compiles off the frame thread (#633), once
    /// [`file_watch`] answers from its own thread (the player), or a test
    /// asks for it. Dropped by [`Self::clear`], so nothing from the last
    /// scene lands in the next.
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<crate::reload_worker::ReloadWorker>,
    /// Scripts the worker is compiling.
    #[cfg(not(target_arch = "wasm32"))]
    in_flight: HashSet<EntityId>,
    /// The worker is compiling a changed lib: the old modules run until it's
    /// done, then every script reloads against the new ones.
    #[cfg(not(target_arch = "wasm32"))]
    lib_in_flight: bool,
    /// Use the worker whatever [`file_watch`]'s mode (tests).
    #[cfg(not(target_arch = "wasm32"))]
    force_worker: bool,
}

impl ScriptSync {
    pub fn new() -> Self {
        Self {
            discovered: HashSet::new(),
            file_timestamps: HashMap::new(),
            search: None,
            located: HashMap::new(),
            lib_timestamps: BTreeMap::new(),
            to_reload: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            worker: None,
            #[cfg(not(target_arch = "wasm32"))]
            in_flight: HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            lib_in_flight: false,
            #[cfg(not(target_arch = "wasm32"))]
            force_worker: false,
        }
    }

    /// Read and compile changed scripts on a worker thread even when
    /// [`file_watch`] answers on the caller's (tests of the worker).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn use_reload_worker(&mut self) {
        self.force_worker = true;
    }

    /// Clear all discovery state for a scene transition.
    pub fn clear(&mut self) {
        self.discovered.clear();
        self.file_timestamps.clear();
        self.search = None;
        self.located.clear();
        self.lib_timestamps.clear();
        self.to_reload.clear();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.worker = None;
            self.in_flight.clear();
            self.lib_in_flight = false;
        }
    }

    /// Look for scripts in this one directory and nowhere else (tests, and
    /// a host with its own layout). A scene's scripts go through
    /// [`Self::set_search`].
    pub fn set_scripts_dir(&mut self, dir: PathBuf) {
        self.search = Some(ScriptSearch::Dir(dir));
        self.located.clear();
        self.lib_timestamps = self.scan_libs();
    }

    /// Look for scripts where the project's one asset search does
    /// (`AssetKind::Scripts`, ADR 0114): `scripts/` beside the scene, in
    /// each directory above it up to the project root, then in the engine.
    pub fn set_search(&mut self, project: Project, scene_dir: PathBuf) {
        self.search = Some(ScriptSearch::Project { project, scene_dir });
        self.located.clear();
        self.lib_timestamps = self.scan_libs();
    }

    /// What `import` resolves against, once a search is set: the same
    /// places, under `lib/`.
    pub fn module_base(&self) -> Option<ModuleBase> {
        Some(match self.search.as_ref()? {
            ScriptSearch::Dir(dir) => ModuleBase::Dir(dir.join("lib")),
            ScriptSearch::Project { project, scene_dir } => ModuleBase::Project {
                project: project.clone(),
                scene_dir: scene_dir.clone(),
            },
        })
    }

    /// Every `lib/` folder an `import` can be answered from, nearest first
    /// (whether or not it exists yet).
    pub fn lib_dirs(&self) -> Vec<PathBuf> {
        match &self.search {
            None => Vec::new(),
            Some(ScriptSearch::Dir(dir)) => vec![dir.join("lib")],
            Some(ScriptSearch::Project { project, scene_dir }) => project
                .search_dirs(AssetKind::Scripts, scene_dir)
                .into_iter()
                .map(|dir| dir.join("lib"))
                .collect(),
        }
    }

    /// The file a script component's `source` names, from `from_dir` (the
    /// scene's directory when `None`), with the nearer places it was looked
    /// for; or the not-found text, every path tried.
    fn locate(&self, source: &str, from_dir: Option<&Path>) -> Result<Located, String> {
        match self.search.as_ref().ok_or("no scripts search is set")? {
            ScriptSearch::Dir(dir) => {
                let path = dir.join(source);
                if flint_core::vfs::exists(&path) {
                    Ok(Located {
                        path,
                        nearer: Vec::new(),
                        from_dir: None,
                    })
                } else {
                    Err(format!("File not found: {}", path.display()))
                }
            }
            ScriptSearch::Project { project, scene_dir } => {
                let dir = from_dir.unwrap_or(scene_dir);
                let answer = project.explain(AssetKind::Scripts, source, dir);
                if let Some(change) = &answer.order_change {
                    change.log();
                }
                match answer.found {
                    Some(path) => Ok(Located {
                        path,
                        nearer: answer.tried,
                        from_dir: from_dir.map(Path::to_path_buf),
                    }),
                    None => Err(flint_core::project::NotFound {
                        kind: AssetKind::Scripts,
                        name: answer.name,
                        tried: answer.tried,
                    }
                    .to_string()),
                }
            }
        }
    }

    /// The file `source` resolves to from `from_dir` (a chunk's folder),
    /// remembered for `entity` so hot reload follows it. `Err` is the
    /// not-found text.
    pub fn locate_for(
        &mut self,
        entity: EntityId,
        source: &str,
        from_dir: &Path,
    ) -> Result<PathBuf, String> {
        let located = self.locate(source, Some(from_dir))?;
        let path = located.path.clone();
        self.located.insert(entity, located);
        Ok(path)
    }

    /// Discover entities with `script` component and compile their scripts
    pub fn discover_and_load(&mut self, world: &FlintWorld, engine: &mut ScriptEngine) {
        if self.search.is_none() {
            return;
        }

        for &entity_id in world.entities_with_component(comp::SCRIPT) {
            if self.discovered.contains(&entity_id) {
                continue;
            }

            let script_comp = world
                .get_components(entity_id)
                .and_then(|comps| comps.get(comp::SCRIPT).cloned());

            let Some(script_data) = script_comp else {
                continue;
            };

            // Check enabled
            let enabled = script_data
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !enabled {
                self.discovered.insert(entity_id);
                continue;
            }

            // Get source file path
            let source = script_data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if source.is_empty() {
                self.discovered.insert(entity_id);
                continue;
            }

            let located = match self.locate(source, None) {
                Ok(located) => located,
                Err(not_found) => {
                    tracing::warn!("{not_found}");
                    self.discovered.insert(entity_id);
                    continue;
                }
            };
            let script_path = located.path.clone();
            self.located.insert(entity_id, located);

            let entity_name = world.get_name(entity_id).unwrap_or("?");
            match engine.compile_file(&script_path) {
                Ok(ast) => {
                    // stderr, so a caller's stdout stays its own (`flint render
                    // --report json` with `--scripts`, #8)
                    eprintln!("[script] Loaded: {} → {}", entity_name, source);
                    // Record file timestamp (hot reload watches the disk)
                    if let Some(modified) = file_watch::modified(&script_path) {
                        self.file_timestamps.insert(script_path.clone(), modified);
                    }
                    engine.add_script(entity_id, ast, source.to_string());
                    engine.name_script(entity_id, entity_name);
                }
                Err(e) => {
                    tracing::warn!("Compile error in {}: {}", source, e);
                }
            }

            self.discovered.insert(entity_id);
        }
    }

    /// Check for modified script files and hot-reload them. The checks ask
    /// [`file_watch`], which in the player never waits on the disk, and
    /// there a worker thread reads and compiles what changed, so a frame
    /// only swaps finished ASTs in (#633). Without the worker (tools, tests)
    /// one changed script is recompiled a call, the rest in the calls after.
    pub fn check_hot_reload(&mut self, engine: &mut ScriptEngine) {
        if self.search.is_none() {
            return;
        }
        if file_watch::mode() == file_watch::Mode::Off {
            return;
        }
        let worker = self.reload_worker();
        if worker {
            self.take_finished(engine);
        }

        // A changed shared module invalidates every importer: drop the
        // resolver cache and recompile every script.
        let lib_now = self.scan_libs();
        if lib_now != self.lib_timestamps {
            let files: Vec<PathBuf> = lib_now.keys().cloned().collect();
            self.lib_timestamps = lib_now;
            if !(worker && self.send_lib(files)) {
                println!("[script] scripts/lib changed - reloading all scripts");
                engine.reset_module_cache();
                self.queue_all(engine);
            }
        }

        for (entity_id, script) in &engine.scripts {
            // A script a test or a host added by hand is located now.
            if !self.located.contains_key(entity_id) {
                match self.locate(&script.source_path, None) {
                    Ok(located) => drop(self.located.insert(*entity_id, located)),
                    Err(_) => continue,
                }
            }
            // A file of this name has appeared somewhere nearer than the one
            // loaded: the search now answers with it, so it takes over.
            let nearer = self.located[entity_id]
                .nearer
                .iter()
                .any(|path| file_watch::stamp(path).is_some());
            if nearer {
                let from = self.located[entity_id].from_dir.clone();
                if let Ok(now) = self.locate(&script.source_path, from.as_deref()) {
                    if now.path != self.located[entity_id].path {
                        eprintln!(
                            "[script] {} is now {}",
                            script.source_path,
                            flint_core::project::shown(&now.path)
                        );
                        Self::queue(&mut self.to_reload, *entity_id, now.path.clone());
                    }
                    self.located.insert(*entity_id, now);
                }
            }
            let script_path = self.located[entity_id].path.clone();
            // Hot reload watches the disk (a memory source has no mtimes).
            let changed = match file_watch::modified(&script_path) {
                Some(current) => self
                    .file_timestamps
                    .get(&script_path)
                    .is_none_or(|last| current > *last),
                None => false,
            };
            if changed {
                Self::queue(&mut self.to_reload, *entity_id, script_path);
            }
        }

        if worker {
            self.send_queued();
            return;
        }

        // Reload the oldest change here
        if self.to_reload.is_empty() {
            return;
        }
        let (entity_id, script_path) = self.to_reload.remove(0);
        // What it is now, taken before the read: a write after it is a change
        let modified = file_watch::modified(&script_path);
        let ast = engine.compile_file(&script_path);
        self.apply(engine, entity_id, script_path, modified, ast);
    }

    fn queue(to_reload: &mut Vec<(EntityId, PathBuf)>, entity: EntityId, path: PathBuf) {
        if !to_reload.iter().any(|(e, _)| *e == entity) {
            to_reload.push((entity, path));
        }
    }

    fn queue_all(&mut self, engine: &ScriptEngine) {
        for entity_id in engine.scripts.keys() {
            if let Some(located) = self.located.get(entity_id) {
                Self::queue(&mut self.to_reload, *entity_id, located.path.clone());
            }
        }
    }

    /// Every `.rhai` under every `lib/` the search can see, with its stamp.
    fn scan_libs(&self) -> BTreeMap<PathBuf, file_watch::Stamp> {
        let mut out = BTreeMap::new();
        for lib in self.lib_dirs() {
            out.extend(scan_lib(&lib));
        }
        out
    }

    /// Swap a compiled script in (or keep the old one on an error), and
    /// remember the stamp it was read at.
    fn apply(
        &mut self,
        engine: &mut ScriptEngine,
        entity_id: EntityId,
        script_path: PathBuf,
        modified: Option<SystemTime>,
        ast: Result<rhai::AST, String>,
    ) {
        match ast {
            Ok(ast) => {
                if let Some(script) = engine.scripts.get(&entity_id) {
                    println!("[script] Hot-reloaded: {}", script.source_path);
                }
                // New top-level lets get their initial values (#634)
                let added = engine.reload_script(entity_id, ast);
                if !added.is_empty() {
                    println!("[script]   new top-level variables: {}", added.join(", "));
                }
            }
            Err(e) => {
                // Keep old AST on compile error; tried again once it changes
                tracing::warn!("Hot-reload compile error: {}", e);
            }
        }
        if let Some(modified) = modified {
            self.file_timestamps.insert(script_path, modified);
        }
    }

    /// Whether this call reads and compiles on the worker, starting it if
    /// it should and hasn't.
    #[cfg(not(target_arch = "wasm32"))]
    fn reload_worker(&mut self) -> bool {
        let wanted = self.force_worker || file_watch::mode() == file_watch::Mode::Background;
        if wanted && self.worker.is_none() {
            self.worker = crate::reload_worker::ReloadWorker::spawn();
        }
        wanted && self.worker.is_some()
    }

    #[cfg(target_arch = "wasm32")]
    fn reload_worker(&mut self) -> bool {
        false
    }

    /// Apply what the worker has finished.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_finished(&mut self, engine: &mut ScriptEngine) {
        use crate::reload_worker::Done;
        let Some(worker) = &self.worker else { return };
        for done in worker.finished() {
            match done {
                Done::Script {
                    entity,
                    path,
                    modified,
                    ast,
                } => {
                    self.in_flight.remove(&entity);
                    self.apply(engine, entity, path, modified, ast);
                }
                Done::Lib { compiled } => {
                    println!("[script] scripts/lib changed - reloading all scripts");
                    self.lib_in_flight = false;
                    engine.reset_module_cache_with(compiled);
                    self.queue_all(engine);
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn take_finished(&mut self, _: &mut ScriptEngine) {}

    /// Hand a changed lib to the worker; false if it has gone.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_lib(&mut self, files: Vec<PathBuf>) -> bool {
        use crate::reload_worker::Job;
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|w| w.send(Job::Lib { files }));
        self.lib_in_flight |= sent;
        sent
    }

    #[cfg(target_arch = "wasm32")]
    fn send_lib(&mut self, _: Vec<PathBuf>) -> bool {
        false
    }

    /// Hand every queued script the worker isn't already compiling to it.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_queued(&mut self) {
        use crate::reload_worker::Job;
        let Some(worker) = &self.worker else { return };
        let in_flight = &mut self.in_flight;
        self.to_reload.retain(|(entity, path)| {
            if in_flight.contains(entity) {
                return true; // after this one's done
            }
            let sent = worker.send(Job::Script {
                entity: *entity,
                path: path.clone(),
            });
            if sent {
                in_flight.insert(*entity);
            }
            !sent
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn send_queued(&mut self) {}
}

/// Snapshot every `.rhai` file under `lib` (recursively) with its stamp.
/// Empty when the folder does not exist. Hot reload watches the disk, through
/// [`file_watch`].
fn scan_lib(lib: &Path) -> BTreeMap<PathBuf, file_watch::Stamp> {
    file_watch::tree(lib, "rhai")
}

/// Where a [`ScriptSync`] looks for scripts.
enum ScriptSearch {
    /// One directory, as given.
    Dir(PathBuf),
    /// The project's one asset search, from a scene's directory.
    Project {
        project: Project,
        scene_dir: PathBuf,
    },
}

/// Where a script was found.
struct Located {
    /// The file loaded.
    path: PathBuf,
    /// The places tried before it, nearest first.
    nearer: Vec<PathBuf>,
    /// The directory the search began in, when not the scene's (a chunk's).
    from_dir: Option<PathBuf>,
}

/// Point `sync` at the scripts a scene can see: `scripts/` beside the scene,
/// in each directory above it up to the project root, then in the engine
/// (`AssetKind::Scripts`, ADR 0114). Each script is found on its own, so a
/// `scripts/` beside the scene no longer hides the project's.
pub fn load_scripts_from_scene(scene_path: &str, project: &Project, sync: &mut ScriptSync) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync.set_search(project.clone(), scene_dir.to_path_buf());
}
