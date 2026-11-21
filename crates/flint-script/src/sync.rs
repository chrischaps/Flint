//! ScriptSync — entity discovery, .rhai file loading, hot-reload
//!
//! Scans the ECS world for entities with a `script` component and loads
//! the corresponding .rhai source files. Watches for file changes to
//! support live hot-reload during play.

use crate::engine::ScriptEngine;
use crate::errors::Failure;
use crate::module_resolver::ModuleBase;
use flint_core::components as comp;
use flint_core::file_watch;
use flint_core::frame_notes::{self, NoteKind};
use flint_core::project::{AssetKind, Project};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A hot-reload poll that takes this long is a frame note (#1077); one
/// under it, every frame's, is not news.
const POLL_NOTE_MS: f32 = 1.0;

/// Tracks which entities have scripts and manages file-based hot-reload
#[derive(Default)]
pub struct ScriptSync {
    /// Set of entity IDs that have been discovered and loaded
    pub(crate) discovered: HashSet<EntityId>,
    /// Source path → last modified time for hot-reload detection
    file_timestamps: HashMap<PathBuf, SystemTime>,
    /// Where scripts are looked for
    search: Option<ScriptSearch>,
    /// Every `lib/` folder of the search, worked out when it is set: the
    /// walk stats the disk (each level's directory key, the engine mount),
    /// which on the frame thread cost every poll 0.2-0.3 ms and a busy
    /// disk's frame 1-35 ms (#1122). The folders don't change while the
    /// search doesn't; what is in them is `file_watch`'s.
    lib_dirs: Vec<PathBuf>,
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
    /// Scripts that didn't compile at load, with the file's time then
    /// (#693): saving the file again loads it afresh, as a change to a
    /// loaded script reloads that.
    failed_at_load: HashMap<EntityId, (PathBuf, Option<SystemTime>)>,
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
            lib_dirs: Vec::new(),
            located: HashMap::new(),
            lib_timestamps: BTreeMap::new(),
            to_reload: Vec::new(),
            failed_at_load: HashMap::new(),
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
        self.lib_dirs.clear();
        self.located.clear();
        self.lib_timestamps.clear();
        self.to_reload.clear();
        self.failed_at_load.clear();
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
        self.set(ScriptSearch::Dir(dir));
    }

    /// Look for scripts where the project's one asset search does
    /// (`AssetKind::Scripts`, ADR 0114): `scripts/` beside the scene, in
    /// each directory above it up to the project root, then in the engine.
    pub fn set_search(&mut self, project: Project, scene_dir: PathBuf) {
        self.set(ScriptSearch::Project { project, scene_dir });
    }

    /// Set the search and work out its `lib/` folders: the one time hot
    /// reload walks the search, which asks the disk (#1122).
    fn set(&mut self, search: ScriptSearch) {
        self.lib_dirs = match &search {
            ScriptSearch::Dir(dir) => vec![dir.join("lib")],
            ScriptSearch::Project { project, scene_dir } => project
                .search_dirs(AssetKind::Scripts, scene_dir)
                .into_iter()
                .map(|dir| dir.join("lib"))
                .collect(),
        };
        self.search = Some(search);
        self.located.clear();
        self.lib_timestamps = scan_dirs(&self.lib_dirs);
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
    /// (whether or not it exists yet), as found when the search was set.
    pub fn lib_dirs(&self) -> &[PathBuf] {
        &self.lib_dirs
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

            // Through the typed view (ADR 0115): a wrong type is reported once
            let script_data = world.view(entity_id, comp::SCRIPT);
            if !script_data.exists() {
                continue;
            }

            // Check enabled
            if !script_data.bool("enabled", true) {
                self.discovered.insert(entity_id);
                continue;
            }

            // Get source file path
            let source = script_data.str("source", "");
            if source.is_empty() {
                self.discovered.insert(entity_id);
                continue;
            }

            let entity_name = world.get_name(entity_id).unwrap_or("?");
            let located = match self.locate(source, None) {
                Ok(located) => located,
                Err(not_found) => {
                    engine.record_error(Failure::load(source, entity_name, &not_found));
                    self.discovered.insert(entity_id);
                    continue;
                }
            };
            let script_path = located.path.clone();
            self.located.insert(entity_id, located);

            // Read, compiled and its top level queued, on the frame (#1077)
            let _note = flint_core::frame_notes::begin(NoteKind::ScriptLoad, || {
                flint_core::project::shown(&script_path)
            });
            match engine.compile_file(&script_path) {
                Ok(ast) => {
                    // stderr, so a caller's stdout stays its own (`flint render
                    // --report json` with `--scripts`, #8)
                    tracing::debug!(target: "load", kind = "script", "[script] Loaded: {} → {}", entity_name, source);
                    // Record file timestamp (hot reload watches the disk)
                    if let Some(modified) = file_watch::modified(&script_path) {
                        self.file_timestamps.insert(script_path.clone(), modified);
                    }
                    let file = crate::errors::shown_file(&script_path);
                    engine.add_named_script(
                        entity_id,
                        ast,
                        source.to_string(),
                        entity_name,
                        Some(&file),
                    );
                }
                Err(e) => {
                    let file = crate::errors::shown_file(&script_path);
                    engine.record_error(Failure::compile(&file, entity_name, &e));
                    let modified = file_watch::modified(&script_path);
                    self.failed_at_load
                        .insert(entity_id, (script_path.clone(), modified));
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
    /// A reloaded script's new top level runs with `world` lent (#692).
    pub fn check_hot_reload(&mut self, engine: &mut ScriptEngine, world: &mut FlintWorld) {
        if self.search.is_none() {
            return;
        }
        if file_watch::mode() == file_watch::Mode::Off {
            return;
        }
        // The poll itself runs every frame: a frame note only when it took
        // long enough to matter (#1077; #633 was a poll that waited on the
        // disk). What it reloads notes itself, inside this one.
        let began = frame_notes::listening().then(web_time::Instant::now);
        self.poll_hot_reload(engine, world);
        if let Some(began) = began {
            let ms = began.elapsed().as_secs_f32() * 1000.0;
            if ms >= POLL_NOTE_MS {
                let watched = engine.scripts.len();
                frame_notes::note(
                    NoteKind::ReloadPoll,
                    || format!("{watched} scripts watched"),
                    Some(ms),
                );
            }
        }
    }

    fn poll_hot_reload(&mut self, engine: &mut ScriptEngine, world: &mut FlintWorld) {
        let worker = self.reload_worker();
        if worker {
            flint_core::scope!("script::poll_take_finished");
            self.take_finished(engine, world);
        }

        // A script that didn't compile at load, saved since: discovered
        // afresh by the `discover_and_load` that follows (#693)
        let saved: Vec<EntityId> = {
            flint_core::scope!("script::poll_failed_at_load", self.failed_at_load.len());
            self.failed_at_load
                .iter()
                .filter(|(_, (path, then))| {
                    file_watch::modified(path).is_some_and(|now| then.is_none_or(|t| now > t))
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for entity_id in saved {
            if let Some((path, _)) = self.failed_at_load.remove(&entity_id) {
                engine.file_reloaded(&crate::errors::shown_file(&path));
                self.discovered.remove(&entity_id);
            }
        }

        // A changed shared module invalidates every importer: drop the
        // resolver cache and recompile every script.
        let lib_now = {
            flint_core::scope!("script::poll_scan_libs", self.lib_dirs.len());
            scan_dirs(&self.lib_dirs)
        };
        if lib_now != self.lib_timestamps {
            let files: Vec<PathBuf> = lib_now.keys().cloned().collect();
            self.lib_timestamps = lib_now;
            if !(worker && self.send_lib(files)) {
                tracing::info!(target: "script", "scripts/lib changed - reloading all scripts");
                engine.reset_module_cache();
                self.queue_all(engine);
            }
        }

        {
            flint_core::scope!("script::poll_each_script", engine.scripts.len());
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
                            tracing::info!(target: "script",
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
        // Compiled here, on the frame: the read and compile are the note's
        // too, where the worker's reloads note only the swap (#1077)
        let _note = frame_notes::begin(NoteKind::ScriptReload, || {
            format!(
                "{}, compiled on the frame",
                flint_core::project::shown(&script_path)
            )
        });
        // What it is now, taken before the read: a write after it is a change
        let modified = file_watch::modified(&script_path);
        let ast = engine.compile_file(&script_path);
        self.apply(engine, world, entity_id, script_path, modified, ast);
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

    /// Swap a compiled script in (or keep the old one on an error), and
    /// remember the stamp it was read at.
    fn apply(
        &mut self,
        engine: &mut ScriptEngine,
        world: &mut FlintWorld,
        entity_id: EntityId,
        script_path: PathBuf,
        modified: Option<SystemTime>,
        ast: Result<rhai::AST, String>,
    ) {
        match ast {
            Ok(ast) => {
                if let Some(script) = engine.scripts.get(&entity_id) {
                    tracing::info!(target: "script", "Hot-reloaded: {}", script.source_path);
                }
                // New top-level lets get their initial values (#634). The
                // swap and its top level run on the frame (#1077)
                let added = frame_notes::timed(
                    NoteKind::ScriptReload,
                    || flint_core::project::shown(&script_path),
                    || engine.reload_script(world, entity_id, ast),
                );
                if !added.is_empty() {
                    tracing::info!(
                        target: "script",
                        "{}: new top-level variables: {}",
                        engine
                            .scripts
                            .get(&entity_id)
                            .map_or("", |s| s.source_path.as_str()),
                        added.join(", ")
                    );
                }
            }
            Err(e) => {
                // Keep old AST on compile error; tried again once it changes
                if let Some(script) = engine.scripts.get(&entity_id) {
                    let failure = Failure::compile(&script.file, &script.entity_name, &e);
                    engine.record_error(failure);
                } else {
                    tracing::warn!("Hot-reload compile error: {}", e);
                }
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
    fn take_finished(&mut self, engine: &mut ScriptEngine, world: &mut FlintWorld) {
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
                    self.apply(engine, world, entity, path, modified, ast);
                }
                Done::Lib { compiled } => {
                    tracing::info!(target: "script", "scripts/lib changed - reloading all scripts");
                    self.lib_in_flight = false;
                    engine.reset_module_cache_with(compiled);
                    self.queue_all(engine);
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn take_finished(&mut self, _: &mut ScriptEngine, _: &mut FlintWorld) {}

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

/// [`scan_lib`] over each of `libs`, into one snapshot.
fn scan_dirs(libs: &[PathBuf]) -> BTreeMap<PathBuf, file_watch::Stamp> {
    let mut out = BTreeMap::new();
    for lib in libs {
        out.extend(scan_lib(lib));
    }
    out
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
