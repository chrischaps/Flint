//! Calling scripts' callbacks (#697 split them out of `engine.rs`): `on_init`,
//! `on_update`, `on_draw_ui`, the collision, trigger, action and custom
//! events, `on_expression`, `on_scene_exit` / `on_scene_enter`, sequence cues,
//! animation ends and `on_persist_migrate`. Each call lends the world for its
//! scope (`WorldScope`) and goes through the scripts in [`ScriptEngine::in_order`].

use super::interact::{get_interactable_config, is_near_player};
use super::ScriptEngine;
use crate::api;
use crate::context::WorldScope;
use flint_core::callbacks as cb;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;

impl ScriptEngine {
    /// Call on_init() for all scripts that haven't been initialized yet
    pub fn call_inits(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids = self.in_order(|_| true);
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

        let entity_ids = self.in_order(|_| true);
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            if script.has_on_update {
                {
                    let mut c = crate::lock_or_recover(&self.ctx);
                    c.current_entity = entity_id;
                }
                let began = self.update_times.is_some().then(web_time::Instant::now);
                if let Err(e) =
                    self.engine
                        .call_fn::<()>(&mut script.scope, &script.ast, cb::ON_UPDATE, ())
                {
                    tracing::warn!("on_update error ({}): {}", script.source_path, e);
                }
                if let Some(began) = began {
                    let path = script.source_path.clone();
                    self.add_update_time(&path, began);
                }
            }
        }
    }

    /// Call on_draw_ui() for all scripts that define it
    pub fn call_draw_uis(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids = self.in_order(|_| true);
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
                GameEvent::Custom { name, data } => {
                    self.call_event_on_all(name, data);
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

    /// Broadcast a custom event to every script that defines
    /// `on_event(name, data)`, in [`Self::in_order`] so delivery is
    /// deterministic. `data` always arrives as a map: a non-table payload
    /// is wrapped as `#{ value: .. }` (ADR 0072).
    fn call_event_on_all(&mut self, name: &str, data: &toml::Value) {
        let entity_ids = self.in_order(|s| s.has_on_event);
        if entity_ids.is_empty() {
            return;
        }

        let data = match data {
            toml::Value::Table(_) => api::toml_to_dynamic(data.clone()),
            other => {
                let mut map = rhai::Map::new();
                map.insert("value".into(), api::toml_to_dynamic(other.clone()));
                rhai::Dynamic::from(map)
            }
        };

        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            {
                let mut c = crate::lock_or_recover(&self.ctx);
                c.current_entity = entity_id;
            }
            if let Err(e) = self.engine.call_fn::<()>(
                &mut script.scope,
                &script.ast,
                cb::ON_EVENT,
                (name.to_string(), data.clone()),
            ) {
                tracing::warn!("on_event error ({}): {}", script.source_path, e);
            }
        }
    }

    /// Deliver expression states from outside the process (Flint#518), in
    /// arrival order. Each becomes what `expression()` returns, then every
    /// script that defines `on_expression(state)` hears it, in
    /// [`Self::in_order`]. With no states this does nothing, so `expression()` holds the
    /// last one until another arrives.
    pub fn deliver_expressions(&mut self, world: &mut FlintWorld, states: Vec<rhai::Map>) {
        if states.is_empty() {
            return;
        }
        let _world_scope = WorldScope::new(&self.ctx, world);
        let entity_ids = self.in_order(|s| s.has_on_expression);

        for state in states {
            crate::lock_or_recover(&self.ctx).expression = state.clone();
            for &entity_id in &entity_ids {
                let script = self.scripts.get_mut(&entity_id).unwrap();
                crate::lock_or_recover(&self.ctx).current_entity = entity_id;
                if let Err(e) = self.engine.call_fn::<()>(
                    &mut script.scope,
                    &script.ast,
                    cb::ON_EXPRESSION,
                    (rhai::Dynamic::from_map(state.clone()),),
                ) {
                    tracing::warn!("on_expression error ({}): {}", script.source_path, e);
                }
            }
        }
    }

    fn call_action_on_all(&mut self, action: &str, world: &FlintWorld) {
        let entity_ids = self.in_order(|_| true);
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

    /// Hand save data loaded at an older version to the scripts (#726).
    ///
    /// When the store in scope holds data loaded at a version lower than
    /// the game's (`PersistentStore::pending_migration`) and at least one
    /// loaded script defines `on_persist_migrate(from)`, every such script
    /// is called with that version, in [`Self::in_order`], and the store is
    /// then stamped with the game's version. With no such script loaded the
    /// data stays pending, at its old version, for a later scene's. Does
    /// nothing outside a state scope.
    pub fn migrate_persistent_store(&mut self, world: &mut FlintWorld) {
        // The context lock is taken for each read of the store, never held
        // across the script calls below
        let pending = crate::lock_or_recover(&self.ctx)
            .try_persistent_store()
            .and_then(|s| s.pending_migration());
        let Some(from) = pending else {
            return;
        };
        let entity_ids = self.in_order(|s| s.has_on_persist_migrate);
        if entity_ids.is_empty() {
            return;
        }
        let _world_scope = WorldScope::new(&self.ctx, world);
        for entity_id in entity_ids {
            let script = self.scripts.get_mut(&entity_id).unwrap();
            {
                let mut c = crate::lock_or_recover(&self.ctx);
                c.current_entity = entity_id;
            }
            if let Err(e) = self.engine.call_fn::<()>(
                &mut script.scope,
                &script.ast,
                cb::ON_PERSIST_MIGRATE,
                (from,),
            ) {
                tracing::warn!("on_persist_migrate error ({}): {}", script.source_path, e);
            }
        }
        let mut c = crate::lock_or_recover(&self.ctx);
        let Some(store) = c.try_persistent_store_mut() else {
            return;
        };
        // A script that switched slots mid-migration left another slot's
        // data pending; that one gets its own call
        if store.pending_migration() == Some(from) {
            store.finish_migration();
            tracing::info!(
                "persist: save data migrated from version {from} to {}",
                store.version()
            );
        }
    }

    /// Call on_scene_exit() for all scripts that define it
    pub fn call_scene_exits(&mut self, world: &mut FlintWorld) {
        let _world_scope = WorldScope::new(&self.ctx, world);

        let entity_ids = self.in_order(|_| true);
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

        let entity_ids = self.in_order(|_| true);
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
