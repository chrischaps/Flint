//! ScriptSync — entity discovery, .rhai file loading, hot-reload
//!
//! Scans the ECS world for entities with a `script` component and loads
//! the corresponding .rhai source files. Watches for file changes to
//! support live hot-reload during play.

use crate::engine::ScriptEngine;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tracks which entities have scripts and manages file-based hot-reload
#[derive(Default)]
pub struct ScriptSync {
    /// Set of entity IDs that have been discovered and loaded
    pub(crate) discovered: HashSet<EntityId>,
    /// Source path → last modified time for hot-reload detection
    file_timestamps: HashMap<PathBuf, SystemTime>,
    /// Base scripts directory
    scripts_dir: Option<PathBuf>,
    /// Snapshot of `<scripts>/lib/**/*.rhai` → modified time. Any
    /// difference (edit, add, remove) invalidates the module cache and
    /// recompiles every script, since imports are resolved per script.
    lib_timestamps: HashMap<PathBuf, SystemTime>,
}

impl ScriptSync {
    pub fn new() -> Self {
        Self {
            discovered: HashSet::new(),
            file_timestamps: HashMap::new(),
            scripts_dir: None,
            lib_timestamps: HashMap::new(),
        }
    }

    /// Clear all discovery state for a scene transition.
    pub fn clear(&mut self) {
        self.discovered.clear();
        self.file_timestamps.clear();
        self.scripts_dir = None;
        self.lib_timestamps.clear();
    }

    /// Set the scripts directory (called during initialization)
    pub fn set_scripts_dir(&mut self, dir: PathBuf) {
        self.lib_timestamps = scan_lib(&dir.join("lib"));
        self.scripts_dir = Some(dir);
    }

    /// `<scripts>/lib`, the shared-module base for `import`, once a
    /// scripts directory is known (the folder itself need not exist yet).
    pub fn lib_dir(&self) -> Option<PathBuf> {
        self.scripts_dir.as_ref().map(|d| d.join("lib"))
    }

    /// Discover entities with `script` component and compile their scripts
    pub fn discover_and_load(&mut self, world: &FlintWorld, engine: &mut ScriptEngine) {
        let scripts_dir = match &self.scripts_dir {
            Some(d) => d.clone(),
            None => return,
        };

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

            let script_path = scripts_dir.join(source);
            if !script_path.exists() {
                tracing::warn!("File not found: {}", script_path.display());
                self.discovered.insert(entity_id);
                continue;
            }

            let entity_name = world.get_name(entity_id).unwrap_or("?");
            match engine.compile_file(&script_path) {
                Ok(ast) => {
                    println!("[script] Loaded: {} → {}", entity_name, source);
                    // Record file timestamp
                    if let Ok(meta) = std::fs::metadata(&script_path) {
                        if let Ok(modified) = meta.modified() {
                            self.file_timestamps.insert(script_path.clone(), modified);
                        }
                    }
                    engine.add_script(entity_id, ast, source.to_string());
                }
                Err(e) => {
                    tracing::warn!("Compile error in {}: {}", source, e);
                }
            }

            self.discovered.insert(entity_id);
        }
    }

    /// Check for modified script files and hot-reload them
    pub fn check_hot_reload(&mut self, engine: &mut ScriptEngine) {
        let scripts_dir = match &self.scripts_dir {
            Some(d) => d.clone(),
            None => return,
        };

        // A changed shared module invalidates every importer: drop the
        // resolver cache and recompile all scripts this frame.
        let lib_now = scan_lib(&scripts_dir.join("lib"));
        let lib_changed = lib_now != self.lib_timestamps;
        if lib_changed {
            println!("[script] scripts/lib changed - reloading all scripts");
            self.lib_timestamps = lib_now;
            engine.reset_module_cache();
        }

        // Collect scripts that need reloading
        let mut to_reload: Vec<(EntityId, PathBuf)> = Vec::new();

        for (entity_id, script) in &engine.scripts {
            let script_path = scripts_dir.join(&script.source_path);
            if lib_changed {
                to_reload.push((*entity_id, script_path));
                continue;
            }

            let current_modified = match std::fs::metadata(&script_path) {
                Ok(meta) => meta.modified().ok(),
                Err(_) => continue,
            };

            let Some(current) = current_modified else {
                continue;
            };
            let last = self.file_timestamps.get(&script_path);

            if last.is_none_or(|last| current > *last) {
                to_reload.push((*entity_id, script_path));
            }
        }

        // Reload changed scripts
        for (entity_id, script_path) in to_reload {
            match engine.compile_file(&script_path) {
                Ok(ast) => {
                    if let Some(script) = engine.scripts.get_mut(&entity_id) {
                        println!("[script] Hot-reloaded: {}", script.source_path);
                        script.hot_reload(ast);
                    }
                    if let Ok(meta) = std::fs::metadata(&script_path) {
                        if let Ok(modified) = meta.modified() {
                            self.file_timestamps.insert(script_path, modified);
                        }
                    }
                }
                Err(e) => {
                    // Keep old AST on compile error
                    tracing::warn!("Hot-reload compile error: {}", e);
                }
            }
        }
    }
}

/// Snapshot every `.rhai` file under `lib` (recursively) with its mtime.
/// Empty when the folder does not exist.
fn scan_lib(lib: &Path) -> HashMap<PathBuf, SystemTime> {
    fn walk(dir: &Path, out: &mut HashMap<PathBuf, SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rhai") {
                if let Some(m) = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                {
                    out.insert(path, m);
                }
            }
        }
    }
    let mut out = HashMap::new();
    walk(lib, &mut out);
    out
}

/// Load scripts from the `scripts/` directory next to the scene file.
/// Also checks one level up (e.g. game root) for projects that use a `scenes/` subdirectory.
pub fn load_scripts_from_scene(scene_path: &str, sync: &mut ScriptSync) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let scripts_dir = scene_dir.join("scripts");
    if scripts_dir.is_dir() {
        sync.set_scripts_dir(scripts_dir);
        return;
    }

    // Check parent directory (game project structure: scenes/ and scripts/ are siblings)
    if let Some(parent) = scene_dir.parent() {
        let scripts_dir = parent.join("scripts");
        if scripts_dir.is_dir() {
            sync.set_scripts_dir(scripts_dir);
        }
    }
}
