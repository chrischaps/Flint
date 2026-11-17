//! Calling scripts' callbacks (#697 split them out of `engine.rs`): `on_init`,
//! `on_update`, `on_draw_ui`, the collision, trigger, action and custom
//! events, `on_expression`, `on_scene_exit` / `on_scene_enter`, sequence cues,
//! animation ends and `on_persist_migrate`. Each call lends the world for its
//! scope ([`ScriptEngine::lend`], which first runs the top level of any
//! script added since) and goes through the scripts in
//! [`ScriptEngine::in_order`]. A callback is called on the script's
//! functions and imports (`ScriptInstance::calls`), so its top-level
//! statements are not run again first (#692).
//!
//! Every call is [`ScriptEngine::call`] (#693): the one place a callback
//! runs, so every failure is counted in [`ScriptEngine::errors`] rather than
//! warned about on its own, and the one place a per-callback timer goes
//! (#703).

use super::interact::{get_interactable_config, is_near_player};
use super::{ScriptEngine, ScriptInstance};
use crate::api;
use crate::errors::Failure;
use flint_core::callbacks::{self as cb, Callback};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;

impl ScriptEngine {
    /// Call `callback` with `args` on `entity`'s script, as the entity the
    /// script functions act for. A failure is recorded in
    /// [`Self::errors`] with the script's file, the entity's name, the
    /// callback and the line; returns whether it ran without one. An entity
    /// with no script is not called and is no failure. With timing on, the
    /// call is timed under its script's source and `callback` (#703).
    ///
    /// What the callback returns is dropped: a Rhai function's last
    /// expression is its value, so `if t <= 0.0 { play_sound(..) }` at the
    /// end of `on_update` returns the sound's handle, and calling for `()`
    /// made that a failure every time it played (#1031).
    pub(crate) fn call(
        &mut self,
        entity: EntityId,
        callback: &str,
        args: impl rhai::FuncArgs,
    ) -> bool {
        if !self.enter(entity) {
            return true;
        }
        let script = self.scripts.get_mut(&entity).unwrap();
        let recording = flint_core::profile::recording();
        // The callback's own site, `<source>:<callback>` (#1097), found
        // before the borrow `script::call`'s detail holds; nothing unless
        // the recorder records
        let named = recording.then(|| script.trace_site(callback));
        // Inside it, each function the callback calls (#1100), while its
        // time is taken or the recorder records
        let profiling = self.times.is_some() || recording;
        // A callback called from inside another's call is the outer one's
        let counting = profiling && crate::fn_times::begin(recording);
        if profiling {
            script.make_profiled(&self.engine);
        }
        let calls = match &script.profiled {
            Some(Some(timed)) if profiling => timed,
            _ => &script.calls,
        };
        // Each script's call, by file: one a script a callback (#1096)
        flint_core::scope!("script::call", script.file.as_str());
        // ... and inside it the callback by name, as its time is named,
        // so a trace reads `stall.rhai:on_update` under `script::call`
        let _named = named.map(flint_core::profile::Scope::enter);
        // A timed function is two calls deep (its wrapper, then its body):
        // twice the levels, so a recursion fails at the depth it would untimed
        let levels = self.engine.max_call_levels();
        if profiling {
            self.engine.set_max_call_levels(levels.saturating_mul(2));
        }
        let began = self.times.is_some().then(web_time::Instant::now);
        let result = self
            .engine
            .call_fn::<rhai::Dynamic>(&mut script.scope, calls, callback, args);
        if profiling {
            self.engine.set_max_call_levels(levels);
        }
        if let (Some(times), Some(began)) = (&mut self.times, began) {
            let ms = began.elapsed().as_secs_f32() * 1000.0;
            times.callback(&script.source_path, callback, ms);
        }
        if counting {
            crate::fn_times::end(&mut self.fn_took);
            match &mut self.times {
                Some(times) => times.functions(&script.source_path, callback, &mut self.fn_took),
                None => self.fn_took.clear(),
            }
        }
        match result {
            Ok(_) => true,
            Err(e) => {
                let failure = Failure::eval(&script.file, &name_of(script, entity), callback, &e);
                self.errors.record(failure);
                false
            }
        }
    }

    /// Whether `entity` has a script and `defines` says it has the callback.
    fn defines(&self, entity: EntityId, defines: impl Fn(&ScriptInstance) -> bool) -> bool {
        self.scripts.get(&entity).is_some_and(defines)
    }

    /// Call on_init() for all scripts that haven't been initialized yet
    pub fn call_inits(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("script::on_init");
        let _world_scope = self.lend(world);
        let entity_ids = self.in_order(|s| s.defines(Callback::Init) && !s.init_called);
        self.init(&entity_ids);
    }

    /// Call on_init() for specific entities (chunk loading)
    pub fn call_inits_for(&mut self, world: &mut FlintWorld, entity_ids: &[EntityId]) {
        flint_core::scope!("script::on_init");
        let _world_scope = self.lend(world);
        self.init(entity_ids);
    }

    /// `on_init` once for each of `entity_ids` that defines it.
    fn init(&mut self, entity_ids: &[EntityId]) {
        for &entity_id in entity_ids {
            let Some(script) = self.scripts.get_mut(&entity_id) else {
                continue;
            };
            if script.defines(Callback::Init) && !script.init_called {
                script.init_called = true;
                self.call(entity_id, cb::ON_INIT, ());
            }
        }
    }

    /// Call on_update() for all scripts
    pub fn call_updates(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("script::on_update");
        let _world_scope = self.lend(world);

        for entity_id in self.in_order(|s| s.defines(Callback::Update)) {
            self.call(entity_id, cb::ON_UPDATE, ());
        }
    }

    /// The UI documents' input (#731), then on_draw_ui() for all scripts
    /// that define it. Both run paused or not, so a pause menu works: this
    /// frame's input moves focus and presses buttons, and what happened is
    /// `on_ui(element_id, event)` on the script that loaded each document,
    /// before any `on_draw_ui`.
    pub fn call_draw_uis(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("script::on_draw_ui");
        let _world_scope = self.lend(world);

        self.route_ui();
        for entity_id in self.in_order(|s| s.defines(Callback::DrawUi)) {
            self.call(entity_id, cb::ON_DRAW_UI, ());
        }
    }

    /// Route this frame's input through the UI documents and deliver what
    /// it did. A script's answer can make more (`ui_focus` in `on_ui` is a
    /// `blur` and a `focus`): those are delivered too, up to a few rounds a
    /// frame, the rest the next frame. A document loaded outside any
    /// script's call is heard by every script that defines `on_ui`.
    fn route_ui(&mut self) {
        {
            let mut c = crate::lock_or_recover(&self.ctx);
            let c = &mut *c;
            let screen = (c.screen_width, c.screen_height);
            c.ui_system
                .route_snapshot(&c.input, c.mouse_pos_logical, screen, c.delta_time);
        }
        for _round in 0..4 {
            let events = crate::lock_or_recover(&self.ctx).ui_system.take_events();
            if events.is_empty() {
                return;
            }
            let everyone = self.in_order(|s| s.defines(Callback::Ui));
            for event in events {
                let hearers = match event.owner {
                    Some(owner) => vec![owner],
                    None => everyone.clone(),
                };
                for entity_id in hearers {
                    if self.defines(entity_id, |s| s.defines(Callback::Ui)) {
                        let args = (event.element.clone(), event.event.to_string());
                        self.call(entity_id, cb::ON_UI, args);
                    }
                }
            }
        }
    }

    /// Route game events to appropriate script callbacks
    pub fn process_events(&mut self, events: &[GameEvent], world: &mut FlintWorld) {
        if events.is_empty() {
            return;
        }
        flint_core::scope!("script::events", events.len());

        let _world_scope = self.lend(world);

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
                    if self.defines(*trigger, |s| s.defines(Callback::TriggerEnter)) {
                        self.call(*trigger, cb::ON_TRIGGER_ENTER, (entity.raw() as i64,));
                    }
                }
                GameEvent::TriggerExited { entity, trigger } => {
                    if self.defines(*trigger, |s| s.defines(Callback::TriggerExit)) {
                        self.call(*trigger, cb::ON_TRIGGER_EXIT, (entity.raw() as i64,));
                    }
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
        if self.defines(entity, |s| s.defines(Callback::Collision)) {
            self.call(entity, cb::ON_COLLISION, (other.raw() as i64,));
        }
    }

    fn call_collision_exit(&mut self, entity: EntityId, other: EntityId) {
        if self.defines(entity, |s| s.defines(Callback::CollisionExit)) {
            self.call(entity, cb::ON_COLLISION_EXIT, (other.raw() as i64,));
        }
    }

    /// Broadcast a custom event to every script that defines
    /// `on_event(name, data)`, in [`Self::in_order`] so delivery is
    /// deterministic. `data` always arrives as a map: a non-table payload
    /// is wrapped as `#{ value: .. }` (ADR 0072).
    fn call_event_on_all(&mut self, name: &str, data: &toml::Value) {
        let entity_ids = self.in_order(|s| s.defines(Callback::Event));
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
            self.call(entity_id, cb::ON_EVENT, (name.to_string(), data.clone()));
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
        flint_core::scope!("script::on_expression", states.len());
        let _world_scope = self.lend(world);
        let entity_ids = self.in_order(|s| s.defines(Callback::Expression));

        for state in states {
            crate::lock_or_recover(&self.ctx).expression = state.clone();
            for &entity_id in &entity_ids {
                self.call(
                    entity_id,
                    cb::ON_EXPRESSION,
                    (rhai::Dynamic::from_map(state.clone()),),
                );
            }
        }
    }

    fn call_action_on_all(&mut self, action: &str, world: &FlintWorld) {
        for entity_id in
            self.in_order(|s| s.defines(Callback::Action) || s.defines(Callback::Interact))
        {
            if self.defines(entity_id, |s| s.defines(Callback::Action)) {
                self.call(entity_id, cb::ON_ACTION, (action.to_string(),));
            }

            // on_interact sugar: ActionPressed("interact") + proximity + interactable check
            if action == "interact" && self.defines(entity_id, |s| s.defines(Callback::Interact)) {
                // Read range from interactable component, fall back to 3.0
                let (range, enabled) = get_interactable_config(entity_id, world);
                if enabled && is_near_player(entity_id, world, range) {
                    self.call(entity_id, cb::ON_INTERACT, ());
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
        let entity_ids = self.in_order(|s| s.defines(Callback::PersistMigrate));
        if entity_ids.is_empty() {
            return;
        }
        flint_core::scope!("script::on_persist_migrate");
        let _world_scope = self.lend(world);
        for entity_id in entity_ids {
            self.call(entity_id, cb::ON_PERSIST_MIGRATE, (from,));
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
        flint_core::scope!("script::on_scene_exit");
        let _world_scope = self.lend(world);

        for entity_id in self.in_order(|s| s.defines(Callback::SceneExit)) {
            self.call(entity_id, cb::ON_SCENE_EXIT, ());
        }
    }

    /// Call on_scene_enter() for all scripts that define it
    pub fn call_scene_enters(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("script::on_scene_enter");
        let _world_scope = self.lend(world);

        for entity_id in self.in_order(|s| s.defines(Callback::SceneEnter)) {
            self.call(entity_id, cb::ON_SCENE_ENTER, ());
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

        flint_core::scope!("script::on_sequence_cue", cues.len());
        let _world_scope = self.lend(world);

        for cue in cues {
            if self.defines(cue.entity_id, |s| s.defines(Callback::SequenceCue)) {
                self.call(
                    cue.entity_id,
                    cb::ON_SEQUENCE_CUE,
                    (cue.sequence.clone(), cue.cue.clone()),
                );
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

        flint_core::scope!("script::on_animation_end", events.len());
        let _world_scope = self.lend(world);

        for event in events {
            if self.defines(event.entity_id, |s| s.defines(Callback::AnimationEnd)) {
                self.call(
                    event.entity_id,
                    cb::ON_ANIMATION_END,
                    (event.clip_name.clone(),),
                );
            }
        }
    }
}

/// The name an error gives a script's entity: its own, or its id when
/// nothing named it (a chunk's script loaded by path).
pub(super) fn name_of(script: &ScriptInstance, entity: EntityId) -> String {
    if script.entity_name.is_empty() {
        format!("entity {}", entity.raw())
    } else {
        script.entity_name.clone()
    }
}
