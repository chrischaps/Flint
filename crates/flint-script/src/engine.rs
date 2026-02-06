//! ScriptEngine — Rhai engine setup, AST storage, call helpers
//!
//! Wraps rhai::Engine with all API functions registered. Manages compiled ASTs
//! and per-entity Scopes. Provides the call_update / process_events interface
//! that temporarily lends the FlintWorld to scripts.

use crate::api;
use crate::context::{InputSnapshot, ScriptCallContext, ScriptCommand};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use rhai::{AST, Engine, Scope};
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
    pub has_on_trigger_enter: bool,
    pub has_on_trigger_exit: bool,
    pub has_on_action: bool,
    pub has_on_interact: bool,
    pub init_called: bool,
}

impl ScriptInstance {
    pub fn new(ast: AST, source_path: String) -> Self {
        let has_on_init = has_function(&ast, "on_init");
        let has_on_update = has_function(&ast, "on_update");
        let has_on_collision = has_function(&ast, "on_collision");
        let has_on_trigger_enter = has_function(&ast, "on_trigger_enter");
        let has_on_trigger_exit = has_function(&ast, "on_trigger_exit");
        let has_on_action = has_function(&ast, "on_action");
        let has_on_interact = has_function(&ast, "on_interact");

        Self {
            ast,
            scope: Scope::new(),
            source_path,
            has_on_init,
            has_on_update,
            has_on_collision,
            has_on_trigger_enter,
            has_on_trigger_exit,
            has_on_action,
            has_on_interact,
            init_called: false,
        }
    }

    /// Recompile with a new AST but preserve the scope (persistent state)
    pub fn hot_reload(&mut self, ast: AST) {
        self.has_on_init = has_function(&ast, "on_init");
        self.has_on_update = has_function(&ast, "on_update");
        self.has_on_collision = has_function(&ast, "on_collision");
        self.has_on_trigger_enter = has_function(&ast, "on_trigger_enter");
        self.has_on_trigger_exit = has_function(&ast, "on_trigger_exit");
        self.has_on_action = has_function(&ast, "on_action");
        self.has_on_interact = has_function(&ast, "on_interact");
        self.ast = ast;
        // Don't reset init_called — hot-reload preserves state
    }
}

/// Check if an AST contains a function definition with the given name
fn has_function(ast: &AST, name: &str) -> bool {
    ast.iter_functions().any(|f| f.name == name)
}

/// The scripting engine — owns the Rhai Engine and per-entity script instances
pub struct ScriptEngine {
    engine: Engine,
    pub ctx: Arc<Mutex<ScriptCallContext>>,
    pub scripts: HashMap<EntityId, ScriptInstance>,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut engine = Engine::new();

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
        if let Err(e) = self.engine.run_ast_with_scope(&mut instance.scope, &instance.ast) {
            eprintln!("[script] module init error ({}): {}", instance.source_path, e);
        }
        self.scripts.insert(entity, instance);
    }

    /// Provide context for the current frame
    pub fn provide_context(&self, input: InputSnapshot, delta_time: f64, total_time: f64) {
        let mut c = self.ctx.lock().unwrap();
        c.input = input;
        c.delta_time = delta_time;
        c.total_time = total_time;
    }

    /// Call on_init() for all scripts that haven't been initialized yet
    pub fn call_inits(&mut self, world: &mut FlintWorld) {
        // Set world pointer for the duration of this call
        {
            let mut c = self.ctx.lock().unwrap();
            c.world = world as *mut FlintWorld;
        }

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_init && !script.init_called {
                {
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = entity_id;
                }
                script.init_called = true;
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_init", ()) {
                    eprintln!("[script] on_init error ({}): {}", script.source_path, e);
                }
            }
        }

        // Clear world pointer
        {
            let mut c = self.ctx.lock().unwrap();
            c.world = std::ptr::null_mut();
        }
    }

    /// Call on_update(dt) for all scripts
    pub fn call_updates(&mut self, world: &mut FlintWorld, dt: f64) {
        {
            let mut c = self.ctx.lock().unwrap();
            c.world = world as *mut FlintWorld;
        }

        let entity_ids: Vec<EntityId> = self.scripts.keys().copied().collect();
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_update {
                {
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = entity_id;
                }
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_update", (dt,)) {
                    eprintln!("[script] on_update error ({}): {}", script.source_path, e);
                }
            }
        }

        {
            let mut c = self.ctx.lock().unwrap();
            c.world = std::ptr::null_mut();
        }
    }

    /// Route game events to appropriate script callbacks
    pub fn process_events(&mut self, events: &[GameEvent], world: &mut FlintWorld) {
        if events.is_empty() {
            return;
        }

        {
            let mut c = self.ctx.lock().unwrap();
            c.world = world as *mut FlintWorld;
        }

        for event in events {
            match event {
                GameEvent::CollisionStarted { entity_a, entity_b } => {
                    self.call_collision(*entity_a, *entity_b);
                    self.call_collision(*entity_b, *entity_a);
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

        {
            let mut c = self.ctx.lock().unwrap();
            c.world = std::ptr::null_mut();
        }
    }

    fn call_collision(&mut self, entity: EntityId, other: EntityId) {
        if let Some(script) = self.scripts.get_mut(&entity) {
            if script.has_on_collision {
                {
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = entity;
                }
                let other_id = other.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_collision", (other_id,)) {
                    eprintln!("[script] on_collision error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_trigger_enter(&mut self, trigger: EntityId, entity: EntityId) {
        if let Some(script) = self.scripts.get_mut(&trigger) {
            if script.has_on_trigger_enter {
                {
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = trigger;
                }
                let entity_id = entity.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_trigger_enter", (entity_id,)) {
                    eprintln!("[script] on_trigger_enter error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_trigger_exit(&mut self, trigger: EntityId, entity: EntityId) {
        if let Some(script) = self.scripts.get_mut(&trigger) {
            if script.has_on_trigger_exit {
                {
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = trigger;
                }
                let entity_id = entity.raw() as i64;
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_trigger_exit", (entity_id,)) {
                    eprintln!("[script] on_trigger_exit error ({}): {}", script.source_path, e);
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
                    let mut c = self.ctx.lock().unwrap();
                    c.current_entity = entity_id;
                }
                let action_str = action.to_string();
                if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_action", (action_str,)) {
                    eprintln!("[script] on_action error ({}): {}", script.source_path, e);
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
                        let mut c = self.ctx.lock().unwrap();
                        c.current_entity = entity_id;
                    }
                    if let Err(e) = self.engine.call_fn::<()>(&mut script.scope, &script.ast, "on_interact", ()) {
                        eprintln!("[script] on_interact error ({}): {}", script.source_path, e);
                    }
                }
            }
        }
    }

    /// Drain all accumulated script commands
    pub fn drain_commands(&self) -> Vec<ScriptCommand> {
        let mut c = self.ctx.lock().unwrap();
        std::mem::take(&mut c.commands)
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
    let Some(interactable) = comps.get("interactable") else {
        return (3.0, true);
    };
    let range = interactable
        .get("range")
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .unwrap_or(3.0);
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
    let player_id = world.all_entities().iter()
        .find(|e| {
            world.get_components(e.id)
                .map(|c| c.has("character_controller"))
                .unwrap_or(false)
        })
        .map(|e| e.id)?;
    let pt = world.get_transform(player_id)?;

    let mut best: Option<NearestInteractable> = None;

    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else { continue; };
        let Some(interactable) = comps.get("interactable") else { continue; };

        let enabled = interactable.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        if !enabled {
            continue;
        }

        let range = interactable
            .get("range")
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(3.0);

        let Some(et) = world.get_transform(entity.id) else { continue; };
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
    let player_id = world.all_entities().iter()
        .find(|e| {
            world.get_components(e.id)
                .map(|c| c.has("character_controller"))
                .unwrap_or(false)
        })
        .map(|e| e.id);

    let Some(player) = player_id else { return false; };
    let Some(pt) = world.get_transform(player) else { return false; };
    let Some(et) = world.get_transform(entity) else { return false; };

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
        let result = engine.compile("fn on_update(dt) { let x = 1 + 2; }");
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
        let ast = engine.compile(r#"
            fn on_init() {}
            fn on_update(dt) {}
            fn on_collision(other) {}
        "#).unwrap();

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
        world.set_component(id, "health", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("current".into(), toml::Value::Integer(100));
            m
        })).unwrap();

        // Compile a script that reads and writes
        let ast = engine.compile(&format!(r#"
            fn on_init() {{
                let me = self_entity();
                let hp = get_field(me, "health", "current");
                set_field(me, "health", "current", hp - 25);
            }}
        "#)).unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        // Check that health was modified
        let hp = world.get_components(id).unwrap()
            .get_field("health", "current").unwrap()
            .as_integer().unwrap();
        assert_eq!(hp, 75);
    }

    #[test]
    fn test_on_update_receives_dt() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("mover").unwrap();

        world.set_component(id, "state", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("accumulated".into(), toml::Value::Float(0.0));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            fn on_update(dt) {
                let me = self_entity();
                let acc = get_field(me, "state", "accumulated");
                set_field(me, "state", "accumulated", acc + dt);
            }
        "#).unwrap();

        engine.add_script(id, ast, "test.rhai".into());

        // Simulate 3 frames
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(&mut world, 0.016);
        engine.call_updates(&mut world, 0.016);
        engine.call_updates(&mut world, 0.016);

        let acc = world.get_components(id).unwrap()
            .get_field("state", "accumulated").unwrap()
            .as_float().unwrap();
        assert!((acc - 0.048).abs() < 1e-10);
    }

    #[test]
    fn test_input_api() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("input_test").unwrap();

        world.set_component(id, "result", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("jumped".into(), toml::Value::Boolean(false));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            fn on_update(dt) {
                let me = self_entity();
                if is_action_just_pressed("jump") {
                    set_field(me, "result", "jumped", true);
                }
            }
        "#).unwrap();

        engine.add_script(id, ast, "test.rhai".into());

        let mut input = InputSnapshot::default();
        input.actions_just_pressed.insert("jump".into());
        engine.provide_context(input, 0.016, 0.0);
        engine.call_updates(&mut world, 0.016);

        let jumped = world.get_components(id).unwrap()
            .get_field("result", "jumped").unwrap()
            .as_bool().unwrap();
        assert!(jumped);
    }

    #[test]
    fn test_play_sound_command() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("snd_test").unwrap();

        let ast = engine.compile(r#"
            fn on_init() {
                play_sound("bang.ogg");
            }
        "#).unwrap();

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

        let ast = engine.compile(r#"
            fn on_init() {
                fire_event("door_opened");
            }
        "#).unwrap();

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

        world.set_component(entity_a, "hits", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("count".into(), toml::Value::Integer(0));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            fn on_collision(other) {
                let me = self_entity();
                let count = get_field(me, "hits", "count");
                set_field(me, "hits", "count", count + 1);
            }
        "#).unwrap();

        engine.add_script(entity_a, ast, "test.rhai".into());

        let events = vec![GameEvent::CollisionStarted {
            entity_a,
            entity_b,
        }];
        engine.process_events(&events, &mut world);

        let count = world.get_components(entity_a).unwrap()
            .get_field("hits", "count").unwrap()
            .as_integer().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_animation_api_play_clip() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("anim_test").unwrap();

        // Set up an animator component
        world.set_component(id, "animator", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("clip".into(), toml::Value::String("idle".into()));
            m.insert("playing".into(), toml::Value::Boolean(false));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            fn on_init() {
                let me = self_entity();
                play_clip(me, "run");
            }
        "#).unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);

        let clip = world.get_components(id).unwrap()
            .get_field("animator", "clip").unwrap()
            .as_str().unwrap()
            .to_string();
        let playing = world.get_components(id).unwrap()
            .get_field("animator", "playing").unwrap()
            .as_bool().unwrap();
        assert_eq!(clip, "run");
        assert!(playing);
    }

    #[test]
    fn test_position_get_set() {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("pos_test").unwrap();

        // Set up transform
        world.set_component(id, "transform", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("position".into(), toml::Value::Array(vec![
                toml::Value::Float(1.0),
                toml::Value::Float(2.0),
                toml::Value::Float(3.0),
            ]));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            fn on_init() {
                let me = self_entity();
                let pos = get_position(me);
                set_position(me, pos.x + 10.0, pos.y, pos.z);
            }
        "#).unwrap();

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
        let ast1 = engine.compile(r#"
            fn on_init() {
                // No persistent state in this version
            }
            fn on_update(dt) {
                let me = self_entity();
                log("v1 running");
            }
        "#).unwrap();

        engine.add_script(id, ast1, "test.rhai".into());
        engine.call_inits(&mut world);

        // Hot-reload with new version
        let ast2 = engine.compile(r#"
            fn on_update(dt) {
                let me = self_entity();
                log("v2 running");
            }
        "#).unwrap();

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

        world.set_component(id, "state", toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("value".into(), toml::Value::Float(0.0));
            m
        })).unwrap();

        let ast = engine.compile(r#"
            let counter = 10.0;

            fn on_init() {
                // Modify the module-level variable
                counter = 42.0;
            }

            fn on_update(dt) {
                // Read the value set in on_init
                let me = self_entity();
                set_field(me, "state", "value", counter);
            }
        "#).unwrap();

        engine.add_script(id, ast, "test.rhai".into());
        engine.call_inits(&mut world);
        engine.call_updates(&mut world, 0.016);

        let value = world.get_components(id).unwrap()
            .get_field("state", "value").unwrap()
            .as_float().unwrap();
        assert!((value - 42.0).abs() < 1e-10, "Module-level var should persist: got {}", value);
    }
}
