//! ScriptEngine — Rhai engine setup, AST storage, call helpers
//!
//! Wraps rhai::Engine with all API functions registered. Manages compiled ASTs
//! and per-entity Scopes. Provides the call_update / process_events interface
//! that temporarily lends the FlintWorld to scripts.

use crate::api;
use crate::context::{DrawCommand, InputSnapshot, ScriptCallContext, ScriptCommand, WorldScope};
use flint_core::callbacks as cb;
use flint_core::components as comp;
use flint_core::toml_util::toml_f64;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use rhai::{Engine, Scope, AST};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Per-entity script instance
pub struct ScriptInstance {
    pub ast: AST,
    pub scope: Scope<'static>,
    pub source_path: String,
    pub has_on_init: bool,
    pub has_on_update: bool,
    pub has_on_collision: bool,
    pub has_on_collision_exit: bool,
    pub has_on_trigger_enter: bool,
    pub has_on_trigger_exit: bool,
    pub has_on_action: bool,
    pub has_on_interact: bool,
    pub has_on_draw_ui: bool,
    pub has_on_scene_exit: bool,
    pub has_on_scene_enter: bool,
    pub has_on_animation_end: bool,
    pub has_on_sequence_cue: bool,
    pub init_called: bool,
}

impl ScriptInstance {
    pub fn new(ast: AST, source_path: String) -> Self {
        validate_callbacks(&ast, &source_path);
        let has_on_init = has_function(&ast, cb::ON_INIT);
        let has_on_update = has_function(&ast, cb::ON_UPDATE);
        let has_on_collision = has_function(&ast, cb::ON_COLLISION);
        let has_on_collision_exit = has_function(&ast, cb::ON_COLLISION_EXIT);
        let has_on_trigger_enter = has_function(&ast, cb::ON_TRIGGER_ENTER);
        let has_on_trigger_exit = has_function(&ast, cb::ON_TRIGGER_EXIT);
        let has_on_action = has_function(&ast, cb::ON_ACTION);
        let has_on_interact = has_function(&ast, cb::ON_INTERACT);
        let has_on_draw_ui = has_function(&ast, cb::ON_DRAW_UI);
        let has_on_scene_exit = has_function(&ast, cb::ON_SCENE_EXIT);
        let has_on_scene_enter = has_function(&ast, cb::ON_SCENE_ENTER);
        let has_on_animation_end = has_function(&ast, cb::ON_ANIMATION_END);
        let has_on_sequence_cue = has_function(&ast, cb::ON_SEQUENCE_CUE);

        Self {
            ast,
            scope: Scope::new(),
            source_path,
            has_on_init,
            has_on_update,
            has_on_collision,
            has_on_collision_exit,
            has_on_trigger_enter,
            has_on_trigger_exit,
            has_on_action,
            has_on_interact,
            has_on_draw_ui,
            has_on_scene_exit,
            has_on_scene_enter,
            has_on_animation_end,
            has_on_sequence_cue,
            init_called: false,
        }
    }

    /// Recompile with a new AST but preserve the scope (persistent state)
    pub fn hot_reload(&mut self, ast: AST) {
        validate_callbacks(&ast, &self.source_path);
        self.has_on_init = has_function(&ast, cb::ON_INIT);
        self.has_on_update = has_function(&ast, cb::ON_UPDATE);
        self.has_on_collision = has_function(&ast, cb::ON_COLLISION);
        self.has_on_collision_exit = has_function(&ast, cb::ON_COLLISION_EXIT);
        self.has_on_trigger_enter = has_function(&ast, cb::ON_TRIGGER_ENTER);
        self.has_on_trigger_exit = has_function(&ast, cb::ON_TRIGGER_EXIT);
        self.has_on_action = has_function(&ast, cb::ON_ACTION);
        self.has_on_interact = has_function(&ast, cb::ON_INTERACT);
        self.has_on_draw_ui = has_function(&ast, cb::ON_DRAW_UI);
        self.has_on_scene_exit = has_function(&ast, cb::ON_SCENE_EXIT);
        self.has_on_scene_enter = has_function(&ast, cb::ON_SCENE_ENTER);
        self.has_on_animation_end = has_function(&ast, cb::ON_ANIMATION_END);
        self.has_on_sequence_cue = has_function(&ast, cb::ON_SEQUENCE_CUE);
        self.ast = ast;
        // Don't reset init_called — hot-reload preserves state
    }
}

/// Check if an AST contains a function definition with the given name
fn has_function(ast: &AST, name: &str) -> bool {
    ast.iter_functions().any(|f| f.name == name)
}

/// Expected parameter counts for engine callbacks.
const CALLBACK_ARITIES: &[(&str, usize)] = &[
    (cb::ON_INIT, 0),
    (cb::ON_UPDATE, 0),
    (cb::ON_COLLISION, 1),
    (cb::ON_COLLISION_EXIT, 1),
    (cb::ON_TRIGGER_ENTER, 1),
    (cb::ON_TRIGGER_EXIT, 1),
    (cb::ON_ACTION, 1),
    (cb::ON_INTERACT, 0),
    (cb::ON_DRAW_UI, 0),
    (cb::ON_SCENE_EXIT, 0),
    (cb::ON_SCENE_ENTER, 0),
    (cb::ON_ANIMATION_END, 1),
];

/// Warn at load time if a script defines a callback with the wrong number of parameters.
fn validate_callbacks(ast: &AST, source_path: &str) {
    for func in ast.iter_functions() {
        for &(name, expected) in CALLBACK_ARITIES {
            if func.name == name && func.params.len() != expected {
                let hint = if name == cb::ON_UPDATE && func.params.len() == 1 {
                    " Did you mean `fn on_update()`? Use `delta_time()` to access frame delta."
                } else {
                    ""
                };
                tracing::warn!(
                    "warning ({}): `fn {}` expects {} parameter(s) but has {}.{}",
                    source_path,
                    name,
                    expected,
                    func.params.len(),
                    hint
                );
            }
        }
    }
}

/// The scripting engine — owns the Rhai Engine and per-entity script instances
pub struct ScriptEngine {
    engine: Engine,
    pub(crate) ctx: Arc<Mutex<ScriptCallContext>>,
    pub(crate) scripts: HashMap<EntityId, ScriptInstance>,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut engine = Engine::new();

        // Game scripts can have deep nesting (state machines, type checks, etc.)
        engine.set_max_expr_depths(128, 128);

        // Register all API functions
        api::register_all(&mut engine, ctx.clone());

        Self {
            engine,
            ctx,
            scripts: HashMap::new(),
        }
    }

    /// Compile a Rhai source file into an AST
    pub fn compile(&self, source: &str) -> Result<AST, String> {
        self.engine.compile(source).map_err(|e| format!("{}", e))
    }

    /// Compile from a file path
    pub fn compile_file(&self, path: &std::path::Path) -> Result<AST, String> {
        let source = std::fs::read_to_string(path)
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

    /// Provide context for the current frame
    pub fn provide_context(&self, input: InputSnapshot, delta_time: f64, total_time: f64) {
        let mut c = crate::lock_or_recover(&self.ctx);
        c.input = input;
        c.delta_time = delta_time;
        c.total_time = total_time;
    }

    /// Call on_init() for all scripts that haven't been initialized yet
    pub fn call_inits(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_init && !script.init_called {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                script.init_called = true;
                if let Err(e) =
                    self.engine
                        .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_INIT, ())
                {
                    tracing::warn!("on_init error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    /// Call on_init() for specific entities (chunk loading)
    pub fn call_inits_for(&mut self, world: &mut FlintWorld, entity_ids: &[EntityId]) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        for &entity_id in entity_ids {
            if let Some(script) = self.scripts.get_mut(&entity_id) {
                if script.has_on_init && !script.init_called {
                    {
                        let mut c = crate::lock_or_recover(&self.ctx);
                        c.current_entity = entity_id;
                    }
                    script.init_called = true;
                    if let Err(e) =
                        self.engine
                            .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_INIT, ())
                    {
                        tracing::warn!("on_init error ({}): {}", script.source_path, e);
                    }
                }
            }
        }
    }

    /// Call on_update() for all scripts
    pub fn call_updates(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_update {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                if let Err(e) =
                    self.engine
                        .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_UPDATE, ())
                {
                    tracing::warn!("on_update error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    /// Call on_draw_ui() for all scripts that define it
    pub fn call_draw_uis(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_draw_ui {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                if let Err(e) =
                    self.engine
                        .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_DRAW_UI, ())
                {
                    tracing::warn!("on_draw_ui error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    /// Drain all accumulated draw commands
    pub fn drain_draw_commands(&self) -> Vec<DrawCommand> {
        let mut c = crate::lock_or_recover(&self.ctx);
        std::mem::take(&mut c.draw_commands)
    }

    /// Route game events to appropriate script callbacks
    pub fn process_events(&mut self, events: &[GameEvent], world: &mut FlintWorld) {
        if events.is_empty() {
            return;
        }

        let _world_scope = WorldScope::new(&self.ctx, world);

        for event in events {
            match event {
                GameEvent::CollisionStarted { entity_a, entity_b } => {
                    self.call_collision(*entity_a, *entity_b);
                    self.call_collision(*entity_b, *entity_a);
                }
                GameEvent::CollisionEnded { entity_a, entity_b } => {
                    self.call_collision_exit(*entity_a, *entity_b);
                    self.call_collision_exit(*entity_b, *entity_a);
                }
                GameEvent::TriggerEntered { entity, trigger } => {
                    self.call_trigger_enter(*trigger, *entity);
                }
                GameEvent::TriggerExited { entity, trigger } => {
                    self.call_trigger_exit(*trigger, *entity);
                }
                GameEvent::ActionPressed(action) => {
                    self.call_action_on_all(action, world);
                }
                _ => {}
            }
        }
    }

    fn call_collision(&mut self, entity: EntityId, other: EntityId) {
        if let Some(script) = self.scripts.get_mut(&entity) {
            if script.has_on_collision {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity;
                }
                let other_id = other.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_COLLISION,
                    (other_id,),
                ) {
                    tracing::warn!("on_collision error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_collision_exit(&mut self, entity: EntityId, other: EntityId) {
        if let Some(script) = self.scripts.get_mut(&entity) {
            if script.has_on_collision_exit {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity;
                }
                let other_id = other.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_COLLISION_EXIT,
                    (other_id,),
                ) {
                    tracing::warn!("on_collision_exit error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_trigger_enter(&mut self, trigger: EntityId, entity: EntityId) {
        if let Some(script) = self.scripts.get_mut(&trigger) {
            if script.has_on_trigger_enter {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = trigger;
                }
                let entity_id = entity.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_TRIGGER_ENTER,
                    (entity_id,),
                ) {
                    tracing::warn!("on_trigger_enter error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_trigger_exit(&mut self, trigger: EntityId, entity: EntityId) {
        if let Some(script) = self.scripts.get_mut(&trigger) {
            if script.has_on_trigger_exit {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = trigger;
                }
                let entity_id = entity.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_TRIGGER_EXIT,
                    (entity_id,),
                ) {
                    tracing::warn!("on_trigger_exit error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_action_on_all(&mut self, action: &str, world: &FlintWorld) {
        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();

            // on_action callback
            if script.has_on_action {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                let action_str = action.to_string();
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_ACTION,
                    (action_str,),
                ) {
                    tracing::warn!("on_action error ({}): {}", script.source_path, e);
                }
            }

            // on_interact sugar: ActionPressed("interact") + proximity + interactable check
            if script.has_on_interact && action == "interact" {
                // Read range from interactable component, fall back to 3.0
                let (range, enabled) = get_interactable_config(entity_id, world);
                if !enabled {
                    continue;
                }
                let close_enough = is_near_player(entity_id, world, range);
                if close_enough {
                    {
                        let mut c = crate::lock_or_recover(&self.ctx);
                        c.current_entity = entity_id;
                    }
                    if let Err(e) = self.engine.call_fn::<()>(
                        &mut script.scope,
                        &script.ast,
                        cb::ON_INTERACT,
                        (),
                    ) {
                        tracing::warn!("on_interact error ({}): {}", script.source_path, e);
                    }
                }
            }
        }
    }

    /// Drain all accumulated script commands
    pub fn drain_commands(&self) -> Vec<ScriptCommand> {
        let mut c = crate::lock_or_recover(&self.ctx);
        std::mem::take(&mut c.commands)
    }

    /// Clear all script instances. Preserves the Rhai Engine and registered API.
    pub fn clear(&mut self) {
        self.scripts.clear();
        let mut c = crate::lock_or_recover(&self.ctx);
        c.commands.clear();
        c.draw_commands.clear();
        c.ui_system.clear();
    }

    /// Call on_scene_exit() for all scripts that define it
    pub fn call_scene_exits(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_scene_exit {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                if let Err(e) =
                    self.engine
                        .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_SCENE_EXIT, ())
                {
                    tracing::warn!("on_scene_exit error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    /// Call on_scene_enter() for all scripts that define it
    pub fn call_scene_enters(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_scene_enter {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_SCENE_ENTER,
                    (),
                ) {
                    tracing::warn!("on_scene_enter error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    /// Call on_sequence_cue(sequence_name, cue_name) on the owning entity's
    /// script for every cue an animation sequence passed this frame.
    pub fn call_sequence_cues(
        &mut self,
        world: &mut FlintWorld,
        cues: &[flint_animation::SequenceCueEvent],
    ) {
        if cues.is_empty() {
            return;
        }

        let _world_scope = WorldScope::new(&self.ctx, world);

        for cue in cues {
            if let Some(script) = self.scripts.get_mut(&cue.entity_id) {
                if script.has_on_sequence_cue {
                    {
                        let mut c = crate::lock_or_recover(&self.ctx);
                        c.current_entity = cue.entity_id;
                    }
                    if let Err(e) = self.engine.call_fn::<()>(
                        &mut script.scope,
                        &script.ast,
                        cb::ON_SEQUENCE_CUE,
                        (cue.sequence.clone(), cue.cue.clone()),
                    ) {
                        tracing::warn!("on_sequence_cue error ({}): {}", script.source_path, e);
                    }
                }
            }
        }
    }

    /// Call on_animation_end(clip_name) for entities whose sprite animation completed.
    pub fn call_sprite_anim_ends(
        &mut self,
        world: &mut FlintWorld,
        events: &[flint_animation::sprite_sync::SpriteAnimEndEvent],
    ) {
        if events.is_empty() {
            return;
        }

        let _world_scope = WorldScope::new(&self.ctx, world);

        for event in events {
            if let Some(script) = self.scripts.get_mut(&event.entity_id) {
                if script.has_on_animation_end {
                    {
                        let mut c = crate::lock_or_recover(&self.ctx);
                        c.current_entity = event.entity_id;
                    }
                    let clip_name = event.clip_name.clone();
                    if let Err(e) = self.engine.call_fn::<()>(
                        &mut script.scope,
                        &script.ast,
                        cb::ON_ANIMATION_END,
                        (clip_name,),
                    ) {
                        tracing::warn!("on_animation_end error ({}): {}", script.source_path, e);
                    }
                }
            }
        }
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Read interactable range and enabled state from entity components.
/// Returns (range, enabled) — defaults to (3.0, true) if no interactable component.
fn get_interactable_config(entity: EntityId, world: &FlintWorld) -> (f64, bool) {
    let Some(comps) = world.get_components(entity) else {
        return (3.0, true);
    };
    let Some(interactable) = comps.get(comp::INTERACTABLE) else {
        return (3.0, true);
    };
    let range = interactable.get("range").and_then(toml_f64).unwrap_or(3.0);
    let enabled = interactable
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    (range, enabled)
}

/// Information about the nearest interactable entity
pub struct NearestInteractable {
    pub entity_id: EntityId,
    pub prompt_text: String,
    pub interaction_type: String,
    pub distance: f64,
}

/// Find the nearest in-range interactable entity to the player.
/// Checks all entities with an `interactable` component that are enabled and within range.
pub fn find_nearest_interactable(world: &FlintWorld) -> Option<NearestInteractable> {
    // Find player entity
    let player_id = world
        .all_entities()
        .iter()
        .find(|e| {
            world
                .get_components(e.id)
                .map(|c| c.has(comp::CHARACTER_CONTROLLER))
                .unwrap_or(false)
        })
        .map(|e| e.id)?;
    let pt = world.get_transform(player_id)?;

    let mut best: Option<NearestInteractable> = None;

    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let Some(interactable) = comps.get(comp::INTERACTABLE) else {
            continue;
        };

        let enabled = interactable
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            continue;
        }

        let range = interactable.get("range").and_then(toml_f64).unwrap_or(3.0);

        let Some(et) = world.get_transform(entity.id) else {
            continue;
        };
        let dx = (pt.position.x - et.position.x) as f64;
        let dy = (pt.position.y - et.position.y) as f64;
        let dz = (pt.position.z - et.position.z) as f64;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if dist <= range {
            let is_closer = best.as_ref().is_none_or(|b| dist < b.distance);
            if is_closer {
                let prompt_text = interactable
                    .get("prompt_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Interact")
                    .to_string();
                let interaction_type = interactable
                    .get("interaction_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("use")
                    .to_string();
                best = Some(NearestInteractable {
                    entity_id: entity.id,
                    prompt_text,
                    interaction_type,
                    distance: dist,
                });
            }
        }
    }

    best
}

/// Check if an entity is within `range` distance of the player entity
fn is_near_player(entity: EntityId, world: &FlintWorld, range: f64) -> bool {
    // Find player entity (entity with character_controller component)
    let player_id = world
        .all_entities()
        .iter()
        .find(|e| {
            world
                .get_components(e.id)
                .map(|c| c.has(comp::CHARACTER_CONTROLLER))
                .unwrap_or(false)
        })
        .map(|e| e.id);

    let Some(player) = player_id else {
        return false;
    };
    let Some(pt) = world.get_transform(player) else {
        return false;
    };
    let Some(et) = world.get_transform(entity) else {
        return false;
    };

    let dx = (pt.position.x - et.position.x) as f64;
    let dy = (pt.position.y - et.position.y) as f64;
    let dz = (pt.position.z - et.position.z) as f64;
    (dx * dx + dy * dy + dz * dz).sqrt() <= range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = ScriptEngine::new();
        assert!(engine.scripts.is_empty());
    }

    #[test]
    fn test_compile_valid_script() {
        let engine = ScriptEngine::new();
        let result = engine.compile("fn on_update() { let x = 1 + 2; }");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_invalid_script() {
        let engine = ScriptEngine::new();
        let result = engine.compile("fn on_update( { }");
        assert!(result.is_err());
    }

    #[test]
    fn test_callback_detection() {
        let engine = ScriptEngine::new();
        let ast = engine
            .compile(
                r#"
            fn on_init() {}
            fn on_update() {}
            fn on_collision(other) {}
        "#,
            )
            .unwrap();

        let instance = ScriptInstance::new(ast, "test.rhai".into());
        assert!(instance.has_on_init);
        assert!(instance.has_on_update);
        assert!(instance.has_on_collision);
        assert!(!instance.has_on_trigger_enter);
        assert!(!instance.has_on_action);
        assert!(!instance.has_on_interact);
    }

    #[test]
    fn test_entity_api_get_set() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("test_entity").unwrap();

        // Set a component
        world
            .set_component(
                id,
                "health",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("current".into(), toml::Value::Integer(100));
                    m
                }),
            )
            .unwrap();

        // Compile a script that reads and writes
        let ast = engine
            .compile(&format!(
                r#"
            fn on_init() {{
                let me = self_entity();
                let hp = get_field(me, "health", "current");
                set_field(me, "health", "current", hp - 25);
            }}
        "#
            ))
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        // Check that health was modified
        let hp = world
            .get_components(id)
            .unwrap()
            .get_field("health", "current")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(hp, 75);
    }

    #[test]
    fn test_on_update_uses_delta_time() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("mover").unwrap();

        world
            .set_component(
                id,
                "state",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("accumulated".into(), toml::Value::Float(0.0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_update() {
                let dt = delta_time();
                let me = self_entity();
                let acc = get_field(me, "state", "accumulated");
                set_field(me, "state", "accumulated", acc + dt);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());

        // Simulate 3 frames
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world);
        engine.call_updates(&mut world);
        engine.call_updates(&mut world);

        let acc = world
            .get_components(id)
            .unwrap()
            .get_field("state", "accumulated")
            .unwrap()
            .as_float()
            .unwrap();
        assert!((acc - 0.048).abs() < 1e-10);
    }

    // ─── Conducted-parameters API (ADR 0020) ──────────────────

    fn probe_component(world: &mut FlintWorld, id: flint_core::EntityId) {
        world
            .set_component(
                id,
                "probe",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    for f in [
                        "coh",
                        "lx",
                        "ty",
                        "npulses",
                        "kind_ok",
                        "sec_ok",
                        "age",
                        "err",
                        "reassembly",
                        "preroll",
                    ] {
                        m.insert(f.into(), toml::Value::Float(-1.0));
                    }
                    m.insert("bar".into(), toml::Value::Integer(-1));
                    m
                }),
            )
            .unwrap();
    }

    fn probe_float(world: &FlintWorld, id: flint_core::EntityId, field: &str) -> f64 {
        world
            .get_components(id)
            .unwrap()
            .get_field("probe", field)
            .unwrap()
            .as_float()
            .unwrap()
    }

    #[test]
    fn test_conducted_getters_round_trip() {
        use crate::context::{ConductedPulse, ConductedSnapshot};
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("binder").unwrap();
        probe_component(&mut world, id);

        let ast = engine
            .compile(
                r#"
            fn on_update() {
                let me = self_entity();
                set_field(me, "probe", "coh", conducted_coherence());
                set_field(me, "probe", "lx", conducted_lean().x);
                set_field(me, "probe", "ty", conducted_target().y);
                let nt = conducted_next_target();
                set_field(me, "probe", "ntx", nt.x);
                set_field(me, "probe", "nty", nt.y);
                set_field(me, "probe", "ntb", nt.beats);
                set_field(me, "probe", "bar", conducted_bar());
                let pulses = conducted_pulses();
                set_field(me, "probe", "npulses", pulses.len().to_float());
                if pulses.len() >= 2 {
                    set_field(me, "probe", "kind_ok",
                        if pulses[0].kind == "hit" && pulses[1].kind == "miss" { 1.0 } else { 0.0 });
                    set_field(me, "probe", "age", pulses[0].age);
                    set_field(me, "probe", "err", pulses[0].err_ms);
                }
                set_field(me, "probe", "sec_ok",
                    if conducted_section() == "verse" { 1.0 } else { 0.0 });
                set_field(me, "probe", "reassembly", conducted_reassembly());
                set_field(me, "probe", "preroll",
                    if conducted_preroll() { 1.0 } else { 0.0 });
                let sw = conducted_sway();
                set_field(me, "probe", "swx", sw.x);
                set_field(me, "probe", "pl", conducted_pressure_l());
                set_field(me, "probe", "pr", conducted_pressure_r());
                let np = conducted_next_pulse();
                set_field(me, "probe", "npb", np.beats);
                set_field(me, "probe", "np_open", if np.open { 1.0 } else { 0.0 });
                let cues = conducted_cues();
                set_field(me, "probe", "ncues", cues.len().to_float());
                if cues.len() > 0 {
                    set_field(me, "probe", "cue_ok",
                        if cues[0].name == "surge" { 1.0 } else { 0.0 });
                    set_field(me, "probe", "cdepth", cues[0].params.depth);
                    set_field(me, "probe", "cmode_ok",
                        if cues[0].params.mode == "bank" { 1.0 } else { 0.0 });
                }
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "binder.rhai".into());

        {
            let mut c = engine.ctx.lock().unwrap();
            c.conducted = ConductedSnapshot {
                lean: [0.25, -0.5],
                sway: [0.6, -0.2],
                pressure_l: 0.15,
                pressure_r: 0.85,
                cues: vec![crate::context::ConductedCue {
                    name: "surge".into(),
                    age: 0.004,
                    params: vec![
                        ("depth".into(), crate::context::CueParam::Number(0.7)),
                        ("mode".into(), crate::context::CueParam::Text("bank".into())),
                    ],
                }],
                target: [0.1, 0.75],
                next_target: [0.3, -0.4],
                next_target_beats: 2.5,
                next_pulse_beats: 1.75,
                pulse_window_open: true,
                coherence: 0.625,
                beat_phase: 0.5,
                bar_phase: 0.125,
                bar: 7,
                beat: 30.5,
                section: "verse".into(),
                pulses: vec![
                    ConductedPulse {
                        age: 0.031,
                        err_ms: -12.5,
                        kind: "hit".into(),
                    },
                    ConductedPulse {
                        age: 0.002,
                        err_ms: 0.0,
                        kind: "miss".into(),
                    },
                ],
                desaturate: 0.3,
                blur: 0.2,
                chromatic: 0.1,
                reassembly: 0.4,
                rewind: 0.0,
                no_input: false,
                preroll: false,
            };
        }
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world);

        assert_eq!(probe_float(&world, id, "coh"), 0.625);
        assert_eq!(probe_float(&world, id, "lx"), 0.25);
        assert_eq!(probe_float(&world, id, "ty"), 0.75);
        assert_eq!(probe_float(&world, id, "ntx"), 0.3);
        assert_eq!(probe_float(&world, id, "nty"), -0.4);
        assert_eq!(probe_float(&world, id, "ntb"), 2.5);
        let bar = world
            .get_components(id)
            .unwrap()
            .get_field("probe", "bar")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(bar, 7);
        assert_eq!(probe_float(&world, id, "npulses"), 2.0);
        assert_eq!(probe_float(&world, id, "kind_ok"), 1.0);
        assert_eq!(probe_float(&world, id, "age"), 0.031);
        assert_eq!(probe_float(&world, id, "err"), -12.5);
        assert_eq!(probe_float(&world, id, "sec_ok"), 1.0);
        assert_eq!(probe_float(&world, id, "reassembly"), 0.4);
        assert_eq!(probe_float(&world, id, "preroll"), 0.0);
        assert_eq!(probe_float(&world, id, "swx"), 0.6);
        assert_eq!(probe_float(&world, id, "pl"), 0.15);
        assert_eq!(probe_float(&world, id, "pr"), 0.85);
        assert_eq!(probe_float(&world, id, "npb"), 1.75);
        assert_eq!(probe_float(&world, id, "np_open"), 1.0);
        assert_eq!(probe_float(&world, id, "ncues"), 1.0);
        assert_eq!(probe_float(&world, id, "cue_ok"), 1.0);
        assert_eq!(probe_float(&world, id, "cdepth"), 0.7);
        assert_eq!(probe_float(&world, id, "cmode_ok"), 1.0);
    }

    #[test]
    fn test_conducted_neutral_defaults() {
        // Never call set_conducted: getters must read as a clean, settled
        // world so bindings never branch on session existence.
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("binder").unwrap();
        probe_component(&mut world, id);

        let ast = engine
            .compile(
                r#"
            fn on_update() {
                let me = self_entity();
                set_field(me, "probe", "coh", conducted_coherence());
                set_field(me, "probe", "reassembly", conducted_reassembly());
                set_field(me, "probe", "lx", conducted_lean().x);
                let nt = conducted_next_target();
                set_field(me, "probe", "ntx", nt.x + nt.y);
                set_field(me, "probe", "ntb", nt.beats);
                let np = conducted_next_pulse();
                set_field(me, "probe", "npb", np.beats);
                set_field(me, "probe", "np_open", if np.open { 1.0 } else { 0.0 });
                set_field(me, "probe", "npulses", conducted_pulses().len().to_float());
                set_field(me, "probe", "sec_ok",
                    if conducted_section() == "" { 1.0 } else { 0.0 });
                set_field(me, "probe", "preroll",
                    if conducted_preroll() || conducted_no_input() { 1.0 } else { 0.0 });
                set_field(me, "probe", "err", conducted_desaturate() + conducted_blur()
                    + conducted_chromatic() + conducted_rewind() + conducted_beat_phase());
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "binder.rhai".into());
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world);

        assert_eq!(probe_float(&world, id, "coh"), 1.0);
        assert_eq!(probe_float(&world, id, "reassembly"), 1.0);
        assert_eq!(probe_float(&world, id, "lx"), 0.0);
        assert_eq!(probe_float(&world, id, "ntx"), 0.0);
        assert_eq!(
            probe_float(&world, id, "ntb"),
            1e6,
            "sentinel: nothing inbound"
        );
        assert_eq!(
            probe_float(&world, id, "npb"),
            1e6,
            "sentinel: no window inbound"
        );
        assert_eq!(probe_float(&world, id, "np_open"), 0.0);
        assert_eq!(probe_float(&world, id, "npulses"), 0.0);
        assert_eq!(probe_float(&world, id, "sec_ok"), 1.0);
        assert_eq!(probe_float(&world, id, "preroll"), 0.0);
        assert_eq!(probe_float(&world, id, "err"), 0.0);
    }

    #[test]
    fn test_conducted_bad_script_continues() {
        // A binding that throws every frame must not stop other scripts from
        // reading conducted values (log-and-continue, verified not assumed).
        use crate::context::ConductedSnapshot;
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let bad = world.spawn("bad").unwrap();
        let good = world.spawn("good").unwrap();
        probe_component(&mut world, good);

        let bad_ast = engine
            .compile("fn on_update() { this_function_does_not_exist(); }")
            .unwrap();
        engine.add_script(bad, bad_ast, "bad.rhai".into());
        let good_ast = engine
            .compile(
                r#"
            fn on_update() {
                let me = self_entity();
                let acc = get_field(me, "probe", "coh");
                set_field(me, "probe", "coh", acc + conducted_coherence());
            }
        "#,
            )
            .unwrap();
        engine.add_script(good, good_ast, "good.rhai".into());

        engine.ctx.lock().unwrap().conducted = ConductedSnapshot {
            coherence: 0.5,
            ..Default::default()
        };
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        // Two frames: the good script must accumulate across both even
        // though the bad one errors every time.
        engine.call_updates(&mut world);
        engine.call_updates(&mut world);
        assert_eq!(probe_float(&world, good, "coh"), -1.0 + 0.5 + 0.5);
    }

    // ─── Camera roll override (ADR 0022) ──────────────────────

    #[test]
    fn test_camera_roll_override_round_trip() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("roller").unwrap();

        // Not set until a script calls the API.
        assert_eq!(engine.ctx.lock().unwrap().camera_roll_override, None);

        let ast = engine
            .compile("fn on_update() { set_camera_roll(0.25); }")
            .unwrap();
        engine.add_script(id, ast, "roller.rhai".into());
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world);

        // Set after the frame, and take() clears it (one-frame override).
        assert_eq!(
            engine.ctx.lock().unwrap().camera_roll_override.take(),
            Some(0.25)
        );
        assert_eq!(engine.ctx.lock().unwrap().camera_roll_override, None);
    }

    #[test]
    fn test_conducted_hot_reload_and_bad_edit() {
        // Hot-reload picks up a binding edit; a syntax error keeps the old
        // AST running (the compile-error branch in check_hot_reload).
        use crate::sync::ScriptSync;
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("binder").unwrap();
        probe_component(&mut world, id);

        let dir =
            std::env::temp_dir().join(format!("flint-conducted-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("binder.rhai");
        let v1 = r#"fn on_update() {
            set_field(self_entity(), "probe", "coh", conducted_coherence());
        }"#;
        std::fs::write(&path, v1).unwrap();

        let mut sync = ScriptSync::new();
        sync.set_scripts_dir(dir.clone());
        let ast = engine.compile_file(&path).unwrap();
        engine.add_script(id, ast, "binder.rhai".into());
        sync.check_hot_reload(&mut engine); // records the initial timestamp

        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world);
        assert_eq!(probe_float(&world, id, "coh"), 1.0);

        // v2 writes a different field; bump mtime explicitly (Windows mtime
        // granularity would otherwise swallow a fast rewrite).
        let v2 = r#"fn on_update() {
            set_field(self_entity(), "probe", "lx", conducted_lean().x + 2.0);
        }"#;
        std::fs::write(&path, v2).unwrap();
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        drop(f);
        sync.check_hot_reload(&mut engine);
        engine.call_updates(&mut world);
        assert_eq!(probe_float(&world, id, "lx"), 2.0, "v2 must be live");

        // A broken edit: the old (v2) AST keeps running, no panic.
        std::fs::write(&path, "fn on_update( {").unwrap();
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(f);
        sync.check_hot_reload(&mut engine);
        world
            .set_component(
                id,
                "probe",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("lx".into(), toml::Value::Float(0.0));
                    m
                }),
            )
            .unwrap();
        engine.call_updates(&mut world);
        assert_eq!(
            probe_float(&world, id, "lx"),
            2.0,
            "old AST must keep running after a bad edit"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_input_api() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("input_test").unwrap();

        world
            .set_component(
                id,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("jumped".into(), toml::Value::Boolean(false));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_update() {
                let me = self_entity();
                if is_action_just_pressed("jump") {
                    set_field(me, "result", "jumped", true);
                }
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());

        let mut input = InputSnapshot::default();
        input.actions_just_pressed.insert("jump".into());
        engine.provide_context(input, 0.016, 0.0);
        engine.call_updates(&mut world);

        let jumped = world
            .get_components(id)
            .unwrap()
            .get_field("result", "jumped")
            .unwrap()
            .as_bool()
            .unwrap();
        assert!(jumped);
    }

    #[test]
    fn test_play_sound_command() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("snd_test").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                play_sound("bang.ogg");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let commands = engine.drain_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            ScriptCommand::PlaySound { name, volume } => {
                assert_eq!(name, "bang.ogg");
                assert!((volume - 1.0).abs() < 1e-10);
            }
            _ => panic!("Expected PlaySound command"),
        }
    }

    #[test]
    fn test_fire_event_command() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("evt_test").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                fire_event("door_opened");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let commands = engine.drain_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            ScriptCommand::FireEvent { name, .. } => {
                assert_eq!(name, "door_opened");
            }
            _ => panic!("Expected FireEvent command"),
        }
    }

    #[test]
    fn test_collision_event_routing() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let entity_a = world.spawn("entity_a").unwrap();
        let entity_b = world.spawn("entity_b").unwrap();

        world
            .set_component(
                entity_a,
                "hits",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("count".into(), toml::Value::Integer(0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_collision(other) {
                let me = self_entity();
                let count = get_field(me, "hits", "count");
                set_field(me, "hits", "count", count + 1);
            }
        "#,
            )
            .unwrap();

        engine.add_script(entity_a, ast, "test.rhai".into());

        let events = vec![GameEvent::CollisionStarted { entity_a, entity_b }];
        engine.process_events(&events, &mut world);

        let count = world
            .get_components(entity_a)
            .unwrap()
            .get_field("hits", "count")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_animation_api_play_clip() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("anim_test").unwrap();

        // Set up an animator component
        world
            .set_component(
                id,
                "animator",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("clip".into(), toml::Value::String("idle".into()));
                    m.insert("playing".into(), toml::Value::Boolean(false));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                play_clip(me, "run");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let clip = world
            .get_components(id)
            .unwrap()
            .get_field("animator", "clip")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let playing = world
            .get_components(id)
            .unwrap()
            .get_field("animator", "playing")
            .unwrap()
            .as_bool()
            .unwrap();
        assert_eq!(clip, "run");
        assert!(playing);
    }

    #[test]
    fn test_position_get_set() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("pos_test").unwrap();

        // Set up transform
        world
            .set_component(
                id,
                "transform",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert(
                        "position".into(),
                        toml::Value::Array(vec![
                            toml::Value::Float(1.0),
                            toml::Value::Float(2.0),
                            toml::Value::Float(3.0),
                        ]),
                    );
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let pos = get_position(me);
                set_position(me, pos.x + 10.0, pos.y, pos.z);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let transform = world.get_transform(id).unwrap();
        assert!((transform.position.x - 11.0).abs() < 0.01);
        assert!((transform.position.y - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_hot_reload_preserves_scope() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("reload_test").unwrap();

        // First version: set a persistent variable
        let ast1 = engine
            .compile(
                r#"
            fn on_init() {
                // No persistent state in this version
            }
            fn on_update() {
                let me = self_entity();
                log("v1 running");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast1, "test.rhai".into());
        engine.call_inits(&mut world);

        // Hot-reload with new version
        let ast2 = engine
            .compile(
                r#"
            fn on_update() {
                let me = self_entity();
                log("v2 running");
            }
        "#,
            )
            .unwrap();

        let script = engine.scripts.get_mut(&id).unwrap();
        script.hot_reload(ast2);

        // init_called should still be true
        assert!(script.init_called);
        assert!(script.has_on_update);
        assert!(!script.has_on_init);
    }

    #[test]
    fn test_module_level_variables_persist() {
        // Verifies that `let x = value;` at module scope is accessible
        // from both on_init and on_update via Scope population.
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("persist_test").unwrap();

        world
            .set_component(
                id,
                "state",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("value".into(), toml::Value::Float(0.0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            let counter = 10.0;

            fn on_init() {
                // Modify the module-level variable
                counter = 42.0;
            }

            fn on_update() {
                // Read the value set in on_init
                let me = self_entity();
                set_field(me, "state", "value", counter);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);
        engine.call_updates(&mut world);

        let value = world
            .get_components(id)
            .unwrap()
            .get_field("state", "value")
            .unwrap()
            .as_float()
            .unwrap();
        assert!(
            (value - 42.0).abs() < 1e-10,
            "Module-level var should persist: got {}",
            value
        );
    }

    #[test]
    fn test_arity_mismatch_still_detects_function() {
        // A script with the wrong arity should still be detected (warning, not error)
        let engine = ScriptEngine::new();
        let ast = engine
            .compile(
                r#"
            fn on_update(dt) {
                // Wrong arity — should warn but still detect
            }
        "#,
            )
            .unwrap();

        let instance = ScriptInstance::new(ast, "test_mismatch.rhai".into());
        assert!(
            instance.has_on_update,
            "has_on_update should be true even with wrong arity"
        );
    }

    // ── Phase 4a: Mutable world access ─────────────────────────────

    #[test]
    fn test_spawn_entity_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("spawner").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                spawn_entity("spawned_child");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        assert!(
            world.contains_name("spawned_child"),
            "entity spawned from script should exist"
        );
    }

    #[test]
    fn test_despawn_entity_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("controller").unwrap();
        let target = world.spawn("target_to_kill").unwrap();

        let ast = engine
            .compile(&format!(
                r#"
            fn on_init() {{
                despawn_entity({});
            }}
        "#,
                target.raw() as i64
            ))
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        assert!(
            !world.contains_name("target_to_kill"),
            "despawned entity should be gone"
        );
    }

    #[test]
    fn test_set_rotation_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("rotator").unwrap();

        world
            .set_component(
                id,
                "transform",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert(
                        "position".into(),
                        toml::Value::Array(vec![
                            toml::Value::Float(0.0),
                            toml::Value::Float(0.0),
                            toml::Value::Float(0.0),
                        ]),
                    );
                    m.insert(
                        "rotation".into(),
                        toml::Value::Array(vec![
                            toml::Value::Float(0.0),
                            toml::Value::Float(0.0),
                            toml::Value::Float(0.0),
                        ]),
                    );
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                set_rotation(me, 0.0, 90.0, 0.0);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let t = world.get_transform(id).unwrap();
        assert!(
            (t.rotation.y - 90.0).abs() < 0.01,
            "rotation Y should be 90, got {}",
            t.rotation.y
        );
    }

    fn quat_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
        (0..4).map(|i| a[i] * b[i]).sum::<f32>().abs()
    }

    fn spawn_with_quat(world: &mut FlintWorld, name: &str, q: Option<[f32; 4]>) -> EntityId {
        let id = world.spawn(name).unwrap();
        let mut m = toml::map::Map::new();
        m.insert(
            "position".into(),
            toml::Value::Array(vec![
                toml::Value::Float(0.0),
                toml::Value::Float(0.0),
                toml::Value::Float(0.0),
            ]),
        );
        if let Some(q) = q {
            m.insert(
                "rotation_quat".into(),
                toml::Value::Array(q.iter().map(|c| toml::Value::Float(*c as f64)).collect()),
            );
        }
        world
            .set_component(id, "transform", toml::Value::Table(m))
            .unwrap();
        id
    }

    #[test]
    fn rotate_local_composes_onto_identity() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = spawn_with_quat(&mut world, "spinner", None);
        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                rotate_local(me, 0.0, 45.0, 0.0);
                rotate_local(me, 0.0, 45.0, 0.0);
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let t = world.get_transform(id).unwrap();
        let expected = flint_core::euler_deg_to_quat(0.0, 90.0, 0.0);
        assert!(
            quat_dot(t.effective_quat(), expected) > 0.9999,
            "two 45° yaws should be 90°: {:?}",
            t.rotation_quat
        );
    }

    #[test]
    fn rotate_local_composes_onto_rest_quaternion() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        // Rest pose leaned 15° about X (like a fork axis); drive 30° about its own X.
        let rest = flint_core::euler_deg_to_quat(15.0, 0.0, 0.0);
        let id = spawn_with_quat(&mut world, "fork", Some(rest));
        let ast = engine
            .compile(
                r#"
            fn on_init() {
                rotate_local(self_entity(), 30.0, 0.0, 0.0);
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let t = world.get_transform(id).unwrap();
        let expected = flint_core::euler_deg_to_quat(45.0, 0.0, 0.0);
        assert!(
            quat_dot(t.effective_quat(), expected) > 0.9999,
            "rest 15° + 30° should be 45°: {:?}",
            t.rotation_quat
        );
    }

    #[test]
    fn set_and_get_rotation_quat_round_trip() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = spawn_with_quat(&mut world, "quat", None);
        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                set_rotation_quat(me, 0.0, 0.7071, 0.0, 0.7071);
                let q = get_rotation_quat(me);
                set_field(me, "probe", "y", q.y);
                set_field(me, "probe", "w", q.w);
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let comps = world.get_components(id).unwrap();
        let y = comps
            .get_field("probe", "y")
            .and_then(|v| v.as_float())
            .unwrap();
        let w = comps
            .get_field("probe", "w")
            .and_then(|v| v.as_float())
            .unwrap();
        assert!((y - 0.70710677).abs() < 1e-4 && (w - 0.70710677).abs() < 1e-4);
        let t = world.get_transform(id).unwrap();
        assert!(t.rotation_quat.is_some());
        assert_eq!(
            t.rotation.y, 0.0,
            "Euler zeroed when the quaternion is authoritative"
        );
    }

    #[test]
    fn set_joint_target_writes_motor_target() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = spawn_with_quat(&mut world, "piston", None);
        let mut j = toml::map::Map::new();
        j.insert("type".into(), toml::Value::String("prismatic".into()));
        world
            .set_component(id, "joint", toml::Value::Table(j))
            .unwrap();
        let ast = engine
            .compile(
                r#"
            fn on_init() {
                set_joint_target(self_entity(), 0.25);
                set_field(self_entity(), "probe", "t", get_joint_target(self_entity()));
            }
        "#,
            )
            .unwrap();
        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let comps = world.get_components(id).unwrap();
        let t = comps
            .get_field("joint", "motor_target")
            .and_then(|v| v.as_float())
            .unwrap();
        assert!((t - 0.25).abs() < 1e-9);
        let probe = comps
            .get_field("probe", "t")
            .and_then(|v| v.as_float())
            .unwrap();
        assert!((probe - 0.25).abs() < 1e-9);
    }

    #[test]
    fn test_set_material_color_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("colored").unwrap();

        world
            .set_component(
                id,
                "material",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert(
                        "color".into(),
                        toml::Value::Array(vec![
                            toml::Value::Float(1.0),
                            toml::Value::Float(1.0),
                            toml::Value::Float(1.0),
                        ]),
                    );
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                set_material_color(me, 1.0, 0.0, 0.0, 1.0);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let mat = world.get_component(id, "material").unwrap();
        // set_material_color stores individual base_color_r/g/b/a fields
        assert!((mat.get("base_color_r").unwrap().as_float().unwrap() - 1.0).abs() < 0.01);
        assert!((mat.get("base_color_g").unwrap().as_float().unwrap() - 0.0).abs() < 0.01);
        assert!((mat.get("base_color_b").unwrap().as_float().unwrap() - 0.0).abs() < 0.01);
        assert!((mat.get("base_color_a").unwrap().as_float().unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_find_entities_with_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let controller = world.spawn("controller").unwrap();

        // Create 3 entities with "health" component
        for name in &["enemy_a", "enemy_b", "enemy_c"] {
            let eid = world.spawn(*name).unwrap();
            world
                .set_component(
                    eid,
                    "health",
                    toml::Value::Table({
                        let mut m = toml::map::Map::new();
                        m.insert("current".into(), toml::Value::Integer(100));
                        m
                    }),
                )
                .unwrap();
        }

        world
            .set_component(
                controller,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("count".into(), toml::Value::Integer(0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let enemies = find_entities_with("health");
                set_field(me, "result", "count", enemies.len());
            }
        "#,
            )
            .unwrap();

        engine.add_script(controller, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let count = world
            .get_components(controller)
            .unwrap()
            .get_field("result", "count")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 3, "should find 3 entities with 'health' component");
    }

    // ── Phase 4b: Null pointer guards ──────────────────────────────

    #[test]
    fn test_persist_api_without_store() {
        // persist_set with null store should not crash, just silently fail
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("persist_test").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                // persistent_store pointer is null — these should not crash
                persist_set("key", 42);
                let val = persist_get("key");
                let has = persist_has("key");
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        // Should not panic
        engine.call_inits(&mut world);
    }

    #[test]
    fn test_state_api_without_state_machine() {
        // current_state with null state_machine should return "playing" fallback
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("state_test").unwrap();

        world
            .set_component(
                id,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("state".into(), toml::Value::String(String::new()));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let state = current_state();
                set_field(me, "result", "state", state);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let state = world
            .get_components(id)
            .unwrap()
            .get_field("result", "state")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(state, "playing", "fallback state should be 'playing'");
    }

    #[test]
    fn test_physics_api_without_physics() {
        // raycast with null physics should return unit, not crash
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("physics_test").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                // physics pointer is null — should return () without crashing
                let hit = raycast(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 100.0, -1);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        // Should not panic
        engine.call_inits(&mut world);
    }

    #[test]
    fn test_negative_entity_id_guards() {
        // Negative entity IDs should not crash
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("guard_test").unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let exists = entity_exists(-1);
                let pos = get_position(-1);
            }
        "#,
            )
            .unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        // Should not panic
        engine.call_inits(&mut world);
    }

    // ── Phase 4c: Entity lifecycle and hierarchy ───────────────────

    #[test]
    fn test_set_parent_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let controller = world.spawn("controller").unwrap();
        let parent = world.spawn("parent_e").unwrap();
        let child = world.spawn("child_e").unwrap();

        let ast = engine
            .compile(&format!(
                r#"
            fn on_init() {{
                set_parent({child_id}, {parent_id});
            }}
        "#,
                child_id = child.raw() as i64,
                parent_id = parent.raw() as i64,
            ))
            .unwrap();

        engine.add_script(controller, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        assert_eq!(
            world.get_parent(child),
            Some(parent),
            "parent should be set from script"
        );
    }

    #[test]
    fn test_get_children_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let parent = world.spawn("parent").unwrap();
        let child1 = world.spawn("child1").unwrap();
        let child2 = world.spawn("child2").unwrap();

        world.set_parent(child1, parent).unwrap();
        world.set_parent(child2, parent).unwrap();

        world
            .set_component(
                parent,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("child_count".into(), toml::Value::Integer(0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let kids = get_children(me);
                set_field(me, "result", "child_count", kids.len());
            }
        "#,
            )
            .unwrap();

        engine.add_script(parent, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let count = world
            .get_components(parent)
            .unwrap()
            .get_field("result", "child_count")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 2, "should have 2 children");
    }

    #[test]
    fn test_get_world_position_from_script() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();

        let parent = world.spawn("parent").unwrap();
        let child = world.spawn("child").unwrap();

        let make_pos = |x: f64, y: f64, z: f64| {
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(x),
                        toml::Value::Float(y),
                        toml::Value::Float(z),
                    ]),
                );
                m
            })
        };

        world
            .set_component(parent, "transform", make_pos(10.0, 0.0, 0.0))
            .unwrap();
        world
            .set_component(child, "transform", make_pos(5.0, 0.0, 0.0))
            .unwrap();
        world.set_parent(child, parent).unwrap();

        world
            .set_component(
                child,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("wx".into(), toml::Value::Float(0.0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let wp = get_world_position(me);
                set_field(me, "result", "wx", wp.x);
            }
        "#,
            )
            .unwrap();

        engine.add_script(child, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let wx = world
            .get_components(child)
            .unwrap()
            .get_field("result", "wx")
            .unwrap()
            .as_float()
            .unwrap();
        assert!(
            (wx - 15.0).abs() < 0.01,
            "world x should be 15.0, got {}",
            wx
        );
    }

    // ── Phase 4d: Queries ──────────────────────────────────────────

    #[test]
    fn test_distance_between_entities() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();

        let a = world.spawn("a").unwrap();
        let b = world.spawn("b").unwrap();

        let make_pos = |x: f64, y: f64, z: f64| {
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(x),
                        toml::Value::Float(y),
                        toml::Value::Float(z),
                    ]),
                );
                m
            })
        };

        world
            .set_component(a, "transform", make_pos(0.0, 0.0, 0.0))
            .unwrap();
        world
            .set_component(b, "transform", make_pos(3.0, 4.0, 0.0))
            .unwrap();

        world
            .set_component(
                a,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("dist".into(), toml::Value::Float(0.0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(&format!(
                r#"
            fn on_init() {{
                let me = self_entity();
                let d = distance(me, {b_id});
                set_field(me, "result", "dist", d);
            }}
        "#,
                b_id = b.raw() as i64,
            ))
            .unwrap();

        engine.add_script(a, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let dist = world
            .get_components(a)
            .unwrap()
            .get_field("result", "dist")
            .unwrap()
            .as_float()
            .unwrap();
        assert!(
            (dist - 5.0).abs() < 0.01,
            "distance should be 5.0 (3-4-5 triangle), got {}",
            dist
        );
    }

    #[test]
    fn test_entity_count_with_component() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let controller = world.spawn("counter").unwrap();

        // Create entities, some with "light" component
        for i in 0..4 {
            let name = format!("light_{}", i);
            let eid = world.spawn(&name).unwrap();
            world
                .set_component(eid, "light", toml::Value::Table(Default::default()))
                .unwrap();
        }
        // Also create one without light
        world.spawn("no_light").unwrap();

        world
            .set_component(
                controller,
                "result",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("count".into(), toml::Value::Integer(0));
                    m
                }),
            )
            .unwrap();

        let ast = engine
            .compile(
                r#"
            fn on_init() {
                let me = self_entity();
                let lights = find_entities_with("light");
                set_field(me, "result", "count", lights.len());
            }
        "#,
            )
            .unwrap();

        engine.add_script(controller, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let count = world
            .get_components(controller)
            .unwrap()
            .get_field("result", "count")
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 4, "should find exactly 4 entities with 'light'");
    }
}
