//! ScriptSync — entity discovery, .rhai file loading, hot-reload
//!
//! Scans the ECS world for entities with a `script` component and loads
//! the corresponding .rhai source files. Watches for file changes to
//! support live hot-reload during play.

use crate::engine::ScriptEngine;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tracks which entities have scripts and manages file-based hot-reload
pub struct ScriptSync {
    /// Set of entity IDs that have been discovered and loaded
    discovered: HashSet<EntityId>,
    /// Source path → last modified time for hot-reload detection
    file_timestamps: HashMap<PathBuf, SystemTime>,
    /// Base scripts directory
    scripts_dir: Option<PathBuf>,
}

impl ScriptSync {
    pub fn new() -> Self {
        Self {
            discovered: HashSet::new(),
            file_timestamps: HashMap::new(),
            scripts_dir: None,
        }
    }

    /// Clear all discovery state for a scene transition.
    pub fn clear(&mut self) {
        self.discovered.clear();
        self.file_timestamps.clear();
        self.scripts_dir = None;
    }

    /// Set the scripts directory (called during initialization)
    pub fn set_scripts_dir(&mut self, dir: PathBuf) {
        self.scripts_dir = Some(dir);
    }

    /// Discover entities with `script` component and compile their scripts
    pub fn discover_and_load(&mut self, world: &FlintWorld, engine: &mut ScriptEngine) {
        let scripts_dir = match &self.scripts_dir {
            Some(d) => d.clone(),
            None => return,
        };

        for entity in world.all_entities() {
            if self.discovered.contains(&entity.id) {
                continue;
            }

            let script_comp = world.get_components(entity.id)
                .and_then(|comps| comps.get("script").cloned());

            let Some(script_data) = script_comp else { continue };

            // Check enabled
            let enabled = script_data.get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !enabled {
                self.discovered.insert(entity.id);
                continue;
            }

            // Get source file path
            let source = script_data.get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if source.is_empty() {
                self.discovered.insert(entity.id);
                continue;
            }

            let script_path = scripts_dir.join(source);
            if !script_path.exists() {
                eprintln!("[script] File not found: {}", script_path.display());
                self.discovered.insert(entity.id);
                continue;
            }

            match engine.compile_file(&script_path) {
                Ok(ast) => {
                    println!("[script] Loaded: {} → {}", entity.name, source);
                    // Record file timestamp
                    if let Ok(meta) = std::fs::metadata(&script_path) {
                        if let Ok(modified) = meta.modified() {
                            self.file_timestamps.insert(script_path.clone(), modified);
                        }
                    }
                    engine.add_script(entity.id, ast, source.to_string());
                }
                Err(e) => {
                    eprintln!("[script] Compile error in {}: {}", source, e);
                }
            }

            self.discovered.insert(entity.id);
        }
    }

    /// Check for modified script files and hot-reload them
    pub fn check_hot_reload(&mut self, engine: &mut ScriptEngine) {
        let scripts_dir = match &self.scripts_dir {
            Some(d) => d.clone(),
            None => return,
        };

        // Collect scripts that need reloading
        let mut to_reload: Vec<(EntityId, PathBuf)> = Vec::new();

        for (entity_id, script) in &engine.scripts {
            let script_path = scripts_dir.join(&script.source_path);

            let current_modified = match std::fs::metadata(&script_path) {
                Ok(meta) => meta.modified().ok(),
                Err(_) => continue,
            };

            let Some(current) = current_modified else { continue };
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
                    eprintln!("[script] Hot-reload compile error: {}", e);
                }
            }
        }
    }
}

impl Default for ScriptSync {
    fn default() -> Self {
        Self::new()
    }
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
