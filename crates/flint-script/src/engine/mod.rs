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
use crate::context::{DrawItem, InputSnapshot, ScriptCallContext, ScriptCommand, WorldScope};
use crate::module_resolver::{ModuleBase, VfsModuleResolver};
use flint_core::vfs;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use rhai::{Engine, Scope, AST};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod callbacks;
pub mod instance;
pub mod interact;
pub mod output;

pub use instance::ScriptInstance;
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
    /// Each callback's time, ms, by source path and callback, since the
    /// host last took them: kept only while [`set_timing`](Self::set_timing)
    /// is on (the perf readout, Flint#120; every callback since #703).
    times: Option<output::Times>,
    /// What a profiled callback's functions took (#1100), kept for its room
    fn_took: Vec<crate::fn_times::Took>,
    /// Scripts' `random()` sequence, seeded once from the clock (#580)
    pub(crate) random: crate::random::ScriptRandom,
    /// What the declared API registered (ADR 0116); empty on the web
    registry: api::ApiRegistry,
    /// Every script error of the run, counted (#693)
    pub(crate) errors: crate::errors::ScriptErrors,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut engine = compiler();
        // print and debug to the log, never stdout (#703)
        output::route_speech(&mut engine, &ctx);
        // What a profiled script's function wrappers open (#1100)
        crate::fn_times::register(&mut engine);

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
            times: None,
            fn_took: Vec::new(),
            random,
            registry,
            errors: crate::errors::ScriptErrors::new(),
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
        flint_core::scope!("script::compile", path);
        let source = vfs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        self.compile(&source)
    }

    /// Add a compiled script for an entity. Its top-level statements (the
    /// `let`s the callbacks share, and any call they make) run once, at the
    /// next lending of the world ([`Self::lend`], which every callback batch
    /// starts with), so a top-level `get_entity("door")` finds the door and
    /// `self_entity()` is `entity` (#692).
    pub fn add_script(&mut self, entity: EntityId, ast: AST, source_path: String) {
        self.add_named_script(entity, ast, source_path, "", None);
    }

    /// [`Self::add_script`], with the entity's name (see
    /// [`Self::name_script`]) and the file it was found at (#693), so an
    /// error in its top level (run at the next [`Self::lend`]) or in a
    /// callback names them.
    pub fn add_named_script(
        &mut self,
        entity: EntityId,
        ast: AST,
        source_path: String,
        entity_name: &str,
        file: Option<&str>,
    ) {
        let mut instance = ScriptInstance::new(ast, source_path);
        instance.entity_name = entity_name.to_string();
        if let Some(file) = file {
            instance.file = file.to_string();
        }
        self.scripts.insert(entity, instance);
    }

    /// Lend `world` to scripts until the guard drops, first running the top
    /// level of every script added since the last lending, in
    /// [`Self::in_order`], each with `self_entity()` its own entity. A
    /// failure there is counted as the script's `top level` error (#693).
    pub(crate) fn lend(&mut self, world: &mut FlintWorld) -> WorldScope {
        let scope = WorldScope::new(&self.ctx, world);
        for entity in self.in_order(|s| s.top_level_pending) {
            self.enter(entity);
            let script = self.scripts.get_mut(&entity).unwrap();
            script.top_level_pending = false;
            if let Err(e) = self
                .engine
                .run_ast_with_scope(&mut script.scope, &script.ast)
            {
                let name = callbacks::name_of(script, entity);
                self.errors.record(crate::errors::Failure::eval(
                    &script.file,
                    &name,
                    crate::errors::TOP_LEVEL,
                    &e,
                ));
            }
            script.top_level_ran();
        }
        scope
    }

    /// Every script error of the run (#693).
    pub fn errors(&self) -> &crate::errors::ScriptErrors {
        &self.errors
    }

    /// Count an error found outside a callback: a load or compile error.
    pub fn record_error(&mut self, failure: crate::errors::Failure) {
        self.errors.record(failure);
    }

    /// `file` is being loaded again: its errors so far are off the screen.
    pub(crate) fn file_reloaded(&mut self, file: &str) {
        self.errors.file_reloaded(file);
    }

    /// Hot-reload an entity's script with a new AST (#634). The entity's scope
    /// is kept, so every variable it already has keeps its value; a top-level
    /// `let` or `const` the new source adds gets its initial value. Returns the
    /// names it added (empty when there is no script on `entity`).
    ///
    /// The new source's top-level statements run once, into a scope of their
    /// own, with `world` lent and `self_entity()` the script's entity, and
    /// only names the kept scope lacks are copied over. A script whose top
    /// level hasn't run yet takes the new source and runs it at the next
    /// lending, as it would have.
    pub fn reload_script(
        &mut self,
        world: &mut FlintWorld,
        entity: EntityId,
        ast: AST,
    ) -> Vec<String> {
        let Some(script) = self.scripts.get_mut(&entity) else {
            return Vec::new();
        };
        script.hot_reload(ast);
        // What failed in the old source is not what runs now
        self.errors.file_reloaded(&script.file);
        if script.top_level_pending {
            return Vec::new();
        }
        let _world = self.lend(world);
        self.enter(entity);
        let script = self.scripts.get_mut(&entity).unwrap();
        let mut fresh = Scope::new();
        if let Err(e) = self.engine.run_ast_with_scope(&mut fresh, &script.ast) {
            let name = callbacks::name_of(script, entity);
            self.errors.record(crate::errors::Failure::eval(
                &script.file,
                &name,
                crate::errors::TOP_LEVEL,
                &e,
            ));
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
        script.top_level_ran();
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
            script.speaker = None;
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

    /// Time every callback from now on (see [`take_times`](Self::take_times)).
    pub fn set_timing(&mut self, on: bool) {
        self.times = on.then(output::Times::default);
    }

    /// What was timed since the last take, ms: each callback under
    /// `<source>:<callback>` (`scripts/hud.rhai:on_draw_ui`), scripts sharing
    /// a file adding up, and the script system's own steps in brackets,
    /// without the callbacks they ran. Empty unless timing is on.
    pub fn take_times(&mut self) -> Vec<(String, f32)> {
        self.times
            .as_mut()
            .map(output::Times::take)
            .unwrap_or_default()
    }

    /// What each callback's functions took since the last take (#1100):
    /// see [`crate::ScriptSystem::take_function_times`].
    pub fn take_function_times(&mut self) -> Vec<output::FnTime> {
        self.times
            .as_mut()
            .map(output::Times::take_functions)
            .unwrap_or_default()
    }

    /// Whether [`set_timing`](Self::set_timing) is on.
    pub fn timing(&self) -> bool {
        self.times.is_some()
    }

    /// A step of the script system begins now, when timing.
    pub(crate) fn step_began(&self) -> Option<output::StepStart> {
        self.times.as_ref().map(output::Times::begin)
    }

    /// The step `label` that began at `start` is done.
    pub(crate) fn step_done(&mut self, label: &str, start: Option<output::StepStart>) {
        if let (Some(times), Some(start)) = (&mut self.times, start) {
            times.step(label, start);
        }
    }

    /// `entity`'s script is the one running: what `self_entity()` answers
    /// and what its `print`s name. Whether the entity has a script.
    pub(crate) fn enter(&mut self, entity: EntityId) -> bool {
        let Some(script) = self.scripts.get_mut(&entity) else {
            return false;
        };
        let speaker = script.speaker(entity);
        let mut c = crate::lock_or_recover(&self.ctx);
        c.current_entity = entity;
        c.speaker = speaker;
        true
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

    /// Take the screen field scripts filled this frame (#859); the next
    /// frame starts with none.
    pub fn drain_style_field(&self) -> flint_core::style_field::StyleField {
        let mut c = crate::lock_or_recover(&self.ctx);
        std::mem::take(&mut c.style_field)
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
        // Kept for the run's count, off the screen
        self.errors.scene_cleared();
        let mut c = crate::lock_or_recover(&self.ctx);
        c.commands.clear();
        c.draw_commands.clear();
        c.style_field.clear();
        c.clip_stack.clear();
        c.ui_system.clear();
        c.camera_rig.clear();
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod callback_tests;
#[cfg(test)]
mod error_tests;
#[cfg(test)]
mod fn_time_tests;
#[cfg(test)]
mod hot_reload_tests;
#[cfg(test)]
mod output_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod top_level_tests;
#[cfg(test)]
mod ui_tests;
