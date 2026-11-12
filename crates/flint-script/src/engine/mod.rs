//! ScriptEngine — Rhai engine setup, AST storage, call helpers
//!
//! Wraps rhai::Engine with all API functions registered. Manages compiled ASTs
//! and per-entity Scopes. Provides the call_update / process_events interface
//! that temporarily lends the FlintWorld to scripts.
//!
//! Split by what each part does (#697): this file is the engine itself
//! (setup, compiling, the script table, module search, timing, the drains);
//! [`instance`] is one loaded script and the callbacks it defines,
//! `callbacks` calls them (init, update, draw UI, events, scene changes,
//! animation ends, save migration), and [`interact`] finds what the player
//! can interact with.

use crate::api;
use crate::context::{DrawItem, InputSnapshot, ScriptCallContext, ScriptCommand};
use crate::module_resolver::{ModuleBase, VfsModuleResolver};
use flint_core::vfs;
use flint_core::EntityId;
use rhai::{Engine, Scope, AST};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod callbacks;
pub mod instance;
pub mod interact;

pub use instance::ScriptInstance;
#[cfg(test)]
pub(crate) use instance::CALLBACK_ARITIES;
pub use interact::{find_nearest_interactable, NearestInteractable};

/// A Rhai engine that parses as Flint's does, with none of its API: what
/// [`ScriptEngine::new`] starts from, and what the hot-reload worker compiles
/// with off the frame thread (#633). Compiling needs no registered function
/// (the default optimizer folds only built-in operators, and Flint registers
/// no operator, custom syntax or symbol), so its ASTs are the same; a limit
/// or parser setting belongs here, not in `new`, to keep them so.
pub fn compiler() -> Engine {
    let mut engine = Engine::new();
    // Game scripts can have deep nesting (state machines, type checks, etc.)
    engine.set_max_expr_depths(128, 128);
    engine
}

/// The scripting engine — owns the Rhai Engine and per-entity script instances
pub struct ScriptEngine {
    engine: Engine,
    pub(crate) ctx: Arc<Mutex<ScriptCallContext>>,
    pub(crate) scripts: HashMap<EntityId, ScriptInstance>,
    /// Base directory for `import "name"` (`<scripts>/lib`). `None` until a
    /// scene sets it; the resolver then maps `import "x"` to `<lib>/x.rhai`.
    module_base: Option<ModuleBase>,
    /// Each script's last `on_update`, ms, by source path: kept only while
    /// [`set_update_timing`](Self::set_update_timing) is on (the perf
    /// readout, Flint#120). Scripts sharing a file add up.
    update_times: Option<Vec<(String, f32)>>,
    /// Scripts' `random()` sequence, seeded once from the clock (#580)
    pub(crate) random: crate::random::ScriptRandom,
    /// What the declared API registered (ADR 0116); empty on the web
    registry: api::ApiRegistry,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut engine = compiler();

        // Register all API functions
        let mut registry = api::ApiRegistry::default();
        let random = crate::random::ScriptRandom::from_clock();
        api::register_all(
            &mut api::ScriptApi::new(&mut engine, ctx.clone(), &mut registry),
            &random,
        );

        Self {
            engine,
            ctx,
            scripts: HashMap::new(),
            module_base: None,
            update_times: None,
            random,
            registry,
        }
    }

    /// Declare functions through the builder (ADR 0116), into the same
    /// registry as Flint's own, so `api_listing` lists them.
    pub fn script_api(&mut self) -> api::ScriptApi<'_> {
        api::ScriptApi::new(&mut self.engine, self.ctx.clone(), &mut self.registry)
    }

    /// Every function declared through the builder (empty on the web).
    pub fn api_registry(&self) -> &api::ApiRegistry {
        &self.registry
    }

    /// Point `import "name"` at `<dir>/name.rhai` and drop any cached
    /// modules. Installing a fresh `VfsModuleResolver` is the cache clear:
    /// the Engine owns its resolver, so re-creating it is simpler than
    /// keeping a handle to its cache.
    ///
    /// Modules are read through the vfs, so `import` resolves on every
    /// target, from the disk or a memory source.
    pub fn set_module_base(&mut self, dir: std::path::PathBuf) {
        self.set_module_search(ModuleBase::Dir(dir));
    }

    /// [`Self::set_module_base`] for any [`ModuleBase`]: a scene's is the
    /// project's one search for `scripts/lib/<name>.rhai` (ADR 0114).
    pub fn set_module_search(&mut self, base: ModuleBase) {
        self.engine
            .set_module_resolver(VfsModuleResolver::over(base.clone()));
        self.module_base = Some(base);
    }

    /// Re-install the module resolver on the same base path, discarding
    /// every cached module (used when a `scripts/lib` file changes).
    pub fn reset_module_cache(&mut self) {
        if let Some(base) = self.module_base.clone() {
            self.set_module_search(base);
        }
    }

    /// [`Self::reset_module_cache`] with modules already compiled (by the
    /// hot-reload worker, #633), keyed by file: an import of one of them
    /// evaluates its AST instead of reading the file in a frame.
    pub fn reset_module_cache_with(
        &mut self,
        compiled: std::collections::HashMap<std::path::PathBuf, Result<AST, String>>,
    ) {
        if let Some(base) = self.module_base.clone() {
            self.engine
                .set_module_resolver(VfsModuleResolver::over(base).with_compiled(compiled));
        }
    }

    /// Where `import` looks, if a scene has set it.
    pub fn module_base(&self) -> Option<&ModuleBase> {
        self.module_base.as_ref()
    }

    /// The underlying Rhai engine, for registering native functions on top
    /// of Flint's API (see `ScriptSystem::rhai_engine_mut`).
    pub fn rhai_engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// The underlying Rhai engine, read-only.
    pub fn rhai_engine(&self) -> &Engine {
        &self.engine
    }

    /// Compile a Rhai source file into an AST
    pub fn compile(&self, source: &str) -> Result<AST, String> {
        self.engine.compile(source).map_err(|e| format!("{}", e))
    }

    /// Compile from a file path
    pub fn compile_file(&self, path: &std::path::Path) -> Result<AST, String> {
        let source = vfs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        self.compile(&source)
    }

    /// Add a compiled script for an entity
    pub fn add_script(&mut self, entity: EntityId, ast: AST, source_path: String) {
        let mut instance = ScriptInstance::new(ast, source_path);
        // Evaluate top-level statements to populate scope with module-level variables.
        // Without this, `let x = 0.0;` at the top of a script never enters the Scope,
        // so `call_fn` can't find it from within `on_init`/`on_update`.
        if let Err(e) = self
            .engine
            .run_ast_with_scope(&mut instance.scope, &instance.ast)
        {
            tracing::warn!("module init error ({}): {}", instance.source_path, e);
        }
        self.scripts.insert(entity, instance);
    }

    /// Hot-reload an entity's script with a new AST (#634). The entity's scope
    /// is kept, so every variable it already has keeps its value; a top-level
    /// `let` or `const` the new source adds gets its initial value. Returns the
    /// names it added (empty when there is no script on `entity`).
    ///
    /// The new source's top-level statements run once, into a scope of their
    /// own, and only names the kept scope lacks are copied over. Running them
    /// again has no cost a script doesn't already pay: `call_fn` evaluates a
    /// script's top-level statements before every callback (that is how a
    /// global `import` reaches `on_update`), rewinding the `let`s it pushes.
    pub fn reload_script(&mut self, entity: EntityId, ast: AST) -> Vec<String> {
        let Some(script) = self.scripts.get_mut(&entity) else {
            return Vec::new();
        };
        script.hot_reload(ast);

        let mut fresh = Scope::new();
        if let Err(e) = self.engine.run_ast_with_scope(&mut fresh, &script.ast) {
            tracing::warn!(
                "module init error on reload ({}): {}",
                script.source_path,
                e
            );
        }
        // A name the source declares twice appears twice, oldest first; the
        // last declaration is the one the callbacks would see.
        let mut new_vars: Vec<(String, bool, rhai::Dynamic)> = Vec::new();
        for (name, is_constant, value) in fresh.iter() {
            if script.scope.contains(name) {
                continue;
            }
            match new_vars.iter_mut().find(|(n, ..)| n == name) {
                Some(entry) => *entry = (name.to_string(), is_constant, value),
                None => new_vars.push((name.to_string(), is_constant, value)),
            }
        }
        for (name, is_constant, value) in &new_vars {
            if *is_constant {
                script
                    .scope
                    .push_constant_dynamic(name.clone(), value.clone());
            } else {
                script.scope.push_dynamic(name.clone(), value.clone());
            }
        }
        new_vars.into_iter().map(|(name, ..)| name).collect()
    }

    /// Whether any loaded script may call `complete_transition()` (#362).
    pub fn can_complete_transition(&self) -> bool {
        self.scripts.values().any(|s| s.completes_transition)
    }

    /// Say which entity a script is on, by name, so it runs in its place in
    /// [`Self::in_order`].
    pub fn name_script(&mut self, entity: EntityId, name: &str) {
        if let Some(script) = self.scripts.get_mut(&entity) {
            script.entity_name = name.to_string();
        }
    }

    /// The scripts `keep` picks, in the order every callback runs them: by the
    /// name of the entity each is on, then by id among scripts with the same
    /// name. Ids come from a process-wide counter and scenes load from a
    /// `HashMap`, so ordering by id (or by the map) changed run to run, and a
    /// script that read what another wrote that frame could see it or not.
    /// Names are the scene's, so the order is the same every run (#8).
    pub(crate) fn in_order(&self, keep: impl Fn(&ScriptInstance) -> bool) -> Vec<EntityId> {
        let mut ids: Vec<(&str, EntityId)> = self
            .scripts
            .iter()
            .filter(|(_, script)| keep(script))
            .map(|(id, script)| (script.entity_name.as_str(), *id))
            .collect();
        ids.sort_by(|a, b| a.0.cmp(b.0).then(a.1.raw().cmp(&b.1.raw())));
        ids.into_iter().map(|(_, id)| id).collect()
    }

    /// Provide context for the current frame
    pub fn provide_context(&self, input: InputSnapshot, delta_time: f64, total_time: f64) {
        let mut c = crate::lock_or_recover(&self.ctx);
        c.input = input;
        c.delta_time = delta_time;
        c.total_time = total_time;
    }

    /// Time each script's `on_update` (see [`update_times`](Self::update_times)).
    pub fn set_update_timing(&mut self, on: bool) {
        self.update_times = on.then(Vec::new);
    }

    /// The last frame's time per script file, ms; empty unless timing is
    /// on. The script system's own steps are there too, in brackets.
    pub fn update_times(&self) -> &[(String, f32)] {
        self.update_times.as_deref().unwrap_or(&[])
    }

    /// Whether [`set_update_timing`](Self::set_update_timing) is on.
    pub fn timing(&self) -> bool {
        self.update_times.is_some()
    }

    /// Start a frame's times afresh.
    pub(crate) fn clear_update_times(&mut self) {
        if let Some(times) = &mut self.update_times {
            times.clear();
        }
    }

    /// Add the time since `began` to `label`'s, when timing.
    pub(crate) fn add_update_time(&mut self, label: &str, began: web_time::Instant) {
        let Some(times) = &mut self.update_times else {
            return;
        };
        let ms = began.elapsed().as_secs_f32() * 1000.0;
        match times.iter_mut().find(|(p, _)| p == label) {
            Some((_, t)) => *t += ms,
            None => times.push((label.to_string(), ms)),
        }
    }

    /// Drain all accumulated draw commands (and reset the clip stack)
    pub fn drain_draw_commands(&self) -> Vec<DrawItem> {
        let mut c = crate::lock_or_recover(&self.ctx);
        c.take_draw_items()
    }

    /// Drain the depth-tested world lines scripts queued this frame
    pub fn drain_world_lines(&self) -> Vec<flint_core::world_line::WorldLine> {
        let mut c = crate::lock_or_recover(&self.ctx);
        std::mem::take(&mut c.world_lines)
    }

    /// Drain the depth-tested world text scripts queued this frame
    pub fn drain_world_texts(&self) -> Vec<flint_core::world_text::WorldText> {
        let mut c = crate::lock_or_recover(&self.ctx);
        std::mem::take(&mut c.world_texts)
    }

    /// Drain what scripts asked of the host: the queued commands, then the
    /// frame's requests (`ScriptCallContext::take_commands`).
    pub fn drain_commands(&self) -> Vec<ScriptCommand> {
        crate::lock_or_recover(&self.ctx).take_commands()
    }

    /// What scripts' `get_entity_style` and `get_style_param` ask (#372).
    pub fn set_style_reader(&self, reader: Arc<dyn crate::style_reader::StyleReader>) {
        crate::lock_or_recover(&self.ctx).style_reader = Some(reader);
    }

    /// Clear all script instances. Preserves the Rhai Engine and registered API.
    pub fn clear(&mut self) {
        self.scripts.clear();
        let mut c = crate::lock_or_recover(&self.ctx);
        c.commands.clear();
        c.draw_commands.clear();
        c.clip_stack.clear();
        c.ui_system.clear();
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod hot_reload_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
