//! Bridges ECS `particle_emitter` / `particle_effect` components to the
//! simulation, and packs alive particles for the GPU.
//!
//! Instances live in `BTreeMap`s keyed by entity or detached handle, so
//! iteration — and therefore draw order and RNG consumption — is
//! deterministic frame to frame and run to run (ADR 0068).

use crate::effect::ParticleEffect;
use crate::emitter::{EmitterConfig, EmitterState, ParticleBlendMode, SortMode};
use crate::particle::ParticleInstance;
use crate::sim::{apply_spawn_requests, step_emitter, EmitterFrame, SpawnRequest, IDENTITY};
use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Default cap on alive particles across every emitter.
pub const DEFAULT_BUDGET: usize = 100_000;

/// Where an instance's emitters came from.
#[derive(Clone, Debug)]
pub enum EffectSource {
    /// A single inline `particle_emitter` component.
    Inline,
    /// A registered `*.particles.toml` effect.
    Asset(Arc<ParticleEffect>),
}

/// Identifies one live effect instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InstanceKey {
    Entity(EntityId),
    Detached(u64),
}

impl InstanceKey {
    pub fn entity(self) -> Option<EntityId> {
        match self {
            InstanceKey::Entity(id) => Some(id),
            InstanceKey::Detached(_) => None,
        }
    }
}

/// One effect placed in the world: N emitter states sharing a transform.
pub struct EffectInstance {
    pub source: EffectSource,
    /// Effect name (asset) or entity name (inline).
    pub name: String,
    pub emitters: Vec<EmitterState>,
    pub position: [f32; 3],
    pub prev_position: [f32; 3],
    pub velocity: [f32; 3],
    pub basis: [[f32; 3]; 3],
    pub transform_scale: f32,
    pub effect_scale: f32,
    pub emission_scale: f32,
    /// Effect-level play flag (component `playing || autoplay`).
    pub playing: bool,
    pub seed: u32,
    /// Hash of the source component table; re-resolved only when it changes.
    pub fingerprint: u64,
    /// Optional per-effect alive cap.
    pub budget: Option<usize>,
    has_prev: bool,
    spawn_requests: Vec<SpawnRequest>,
    deferred_requests: Vec<SpawnRequest>,
}

impl EffectInstance {
    pub fn new(source: EffectSource, name: String, configs: Vec<EmitterConfig>, seed: u32) -> Self {
        let mut emitters: Vec<EmitterState> = configs
            .into_iter()
            .enumerate()
            .map(|(i, cfg)| EmitterState::with_seed(cfg, mix_seed(seed, i as u32)))
            .collect();
        resolve_sub_emitter_targets(&mut emitters);
        let playing = emitters.iter().any(|e| e.config.starts_playing());
        let budget = match &source {
            EffectSource::Asset(fx) => fx.budget.map(|b| b as usize),
            EffectSource::Inline => None,
        };
        Self {
            source,
            name,
            emitters,
            position: [0.0; 3],
            prev_position: [0.0; 3],
            velocity: [0.0; 3],
            basis: IDENTITY,
            transform_scale: 1.0,
            effect_scale: 1.0,
            emission_scale: 1.0,
            playing,
            seed,
            fingerprint: 0,
            budget,
            has_prev: false,
            spawn_requests: Vec::new(),
            deferred_requests: Vec::new(),
        }
    }

    pub fn alive(&self) -> usize {
        self.emitters.iter().map(|e| e.pool.alive_count()).sum()
    }

    pub fn capacity(&self) -> usize {
        self.emitters.iter().map(|e| e.pool.capacity()).sum()
    }

    /// Any emitter still emitting?
    pub fn any_emitter_playing(&self) -> bool {
        self.emitters.iter().any(|e| e.playing)
    }

    /// Start (restarting every emitter's timeline) or stop emission.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        for e in &mut self.emitters {
            if playing {
                e.restart();
            } else {
                e.playing = false;
            }
        }
    }

    /// Restart timelines and drop every alive particle.
    pub fn reset(&mut self) {
        for e in &mut self.emitters {
            e.pool.clear();
            e.restart();
            e.playing = e.config.starts_playing();
            e.rng = crate::rand::ParticleRng::new(0);
        }
        // Re-seed deterministically.
        for (i, e) in self.emitters.iter_mut().enumerate() {
            e.rng = crate::rand::ParticleRng::new(mix_seed(self.seed, i as u32));
        }
        self.spawn_requests.clear();
        self.deferred_requests.clear();
        self.has_prev = false;
    }

    /// Move the instance (detached effects, editors).
    pub fn set_transform(&mut self, position: [f32; 3], basis: [[f32; 3]; 3], scale: f32) {
        if !self.has_prev {
            self.prev_position = position;
        }
        self.position = position;
        self.basis = basis;
        self.transform_scale = scale;
    }

    fn frame(&self, time: f32) -> EmitterFrame {
        EmitterFrame {
            position: self.position,
            prev_position: self.prev_position,
            velocity: self.velocity,
            basis: self.basis,
            transform_scale: self.transform_scale,
            effect_scale: self.effect_scale,
            emission_scale: self.emission_scale,
            time,
        }
    }

    /// Advance every emitter by `dt`, resolving sub-emitter requests once.
    /// Returns the number of particles spawned.
    pub fn step(&mut self, dt: f32, time: f32, global_budget_left: &mut usize) {
        if self.has_prev && dt > 0.0 {
            self.velocity = [
                (self.position[0] - self.prev_position[0]) / dt,
                (self.position[1] - self.prev_position[1]) / dt,
                (self.position[2] - self.prev_position[2]) / dt,
            ];
        } else {
            self.velocity = [0.0; 3];
            if !self.has_prev {
                self.prev_position = self.position;
            }
        }
        let frame = self.frame(time);

        // Effective budget: global, further capped by the per-effect one.
        let mut budget_left = match self.budget {
            Some(b) => (*global_budget_left).min(b.saturating_sub(self.alive())),
            None => *global_budget_left,
        };
        let before = budget_left;

        // Requests deferred from last frame spawn first (bounded recursion).
        let deferred = std::mem::take(&mut self.deferred_requests);
        let mut fresh = Vec::new();
        apply_spawn_requests(
            &mut self.emitters,
            &deferred,
            &frame,
            dt,
            &mut budget_left,
            &mut fresh,
        );

        self.spawn_requests.clear();
        for (i, em) in self.emitters.iter_mut().enumerate() {
            step_emitter(
                em,
                i,
                &frame,
                dt,
                &mut budget_left,
                &mut self.spawn_requests,
            );
        }
        let requests = std::mem::take(&mut self.spawn_requests);
        let mut next = fresh;
        apply_spawn_requests(
            &mut self.emitters,
            &requests,
            &frame,
            dt,
            &mut budget_left,
            &mut next,
        );
        self.deferred_requests = next;

        *global_budget_left = global_budget_left.saturating_sub(before - budget_left);

        self.prev_position = self.position;
        self.has_prev = true;
    }
}

fn resolve_sub_emitter_targets(emitters: &mut [EmitterState]) {
    let names: Vec<String> = emitters.iter().map(|e| e.config.name.clone()).collect();
    for e in emitters.iter_mut() {
        e.on_death_target = e
            .config
            .on_death
            .as_ref()
            .and_then(|s| names.iter().position(|n| *n == s.emitter));
        e.on_birth_target = e
            .config
            .on_birth
            .as_ref()
            .and_then(|s| names.iter().position(|n| *n == s.emitter));
    }
}

/// splitmix-style mixing so neighbouring seeds/indices diverge fully.
pub fn mix_seed(seed: u32, salt: u32) -> u32 {
    let mut z = seed ^ salt.wrapping_mul(0x9E37_79B9) ^ 0x6A09_E667;
    z = (z ^ (z >> 16)).wrapping_mul(0x7FEB_352D);
    z = (z ^ (z >> 15)).wrapping_mul(0x846C_A68B);
    z ^= z >> 16;
    if z == 0 {
        1
    } else {
        z
    }
}

/// Stable per-entity salt. Entity *ids* come from a process-wide counter
/// and scene entities load in hash-map order, so ids differ run to run;
/// names do not. FNV-1a over the name keeps headless snapshots reproducible.
fn entity_salt(world: &FlintWorld, id: EntityId) -> u32 {
    match world.get_name(id) {
        Some(name) if !name.is_empty() => fnv1a(name.as_bytes()),
        _ => {
            let v = id.0;
            ((v & 0xFFFF_FFFF) as u32) ^ ((v >> 32) as u32)
        }
    }
}

pub fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Draw data for one emitter, consumed by the renderer
pub struct ParticleDrawData<'a> {
    pub key: InstanceKey,
    pub entity_id: Option<EntityId>,
    pub emitter_index: usize,
    pub emitter_name: &'a str,
    pub instances: &'a [ParticleInstance],
    pub blend_mode: ParticleBlendMode,
    pub texture: &'a str,
    pub frames_x: u32,
    pub frames_y: u32,
    /// View distance of the emitter origin (larger = farther); renderers
    /// draw order-dependent blends far-to-near.
    pub sort_key: f32,
    pub soft_distance: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub lighting: f32,
    pub fog: bool,
    pub stretch: f32,
}

struct DrawRange {
    key: InstanceKey,
    emitter: usize,
    start: usize,
    count: usize,
    sort_key: f32,
}

/// Manages effect instances, syncing between ECS components and simulation
pub struct ParticleSync {
    effects: HashMap<String, Arc<ParticleEffect>>,
    states: BTreeMap<EntityId, EffectInstance>,
    detached: BTreeMap<u64, EffectInstance>,
    budget: usize,
    time: f32,
    instance_buffer: Vec<ParticleInstance>,
    ranges: Vec<DrawRange>,
    order_scratch: Vec<usize>,
    warned_missing: HashSet<String>,
}

impl Default for ParticleSync {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticleSync {
    pub fn new() -> Self {
        Self {
            effects: HashMap::new(),
            states: BTreeMap::new(),
            detached: BTreeMap::new(),
            budget: DEFAULT_BUDGET,
            time: 0.0,
            instance_buffer: Vec::new(),
            ranges: Vec::new(),
            order_scratch: Vec::new(),
            warned_missing: HashSet::new(),
        }
    }

    // ----- Effects registry -----

    /// Register (or replace) a named effect. Live instances of a replaced
    /// effect are rebuilt on the next `sync_from_world`.
    pub fn register_effect(&mut self, effect: Arc<ParticleEffect>) {
        let name = effect.name.clone();
        self.effects.insert(name.clone(), effect);
        self.warned_missing.remove(&name);
        // Force re-resolution of any instance using this effect.
        for inst in self.states.values_mut() {
            if let EffectSource::Asset(fx) = &inst.source {
                if fx.name == name {
                    inst.fingerprint = 0;
                }
            }
        }
    }

    pub fn effect(&self, name: &str) -> Option<&Arc<ParticleEffect>> {
        self.effects.get(name)
    }

    pub fn effect_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.effects.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    pub fn effects(&self) -> &HashMap<String, Arc<ParticleEffect>> {
        &self.effects
    }

    // ----- Budget -----

    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget.max(1);
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Simulation time (sum of `dt`s since the last clear).
    pub fn time(&self) -> f32 {
        self.time
    }

    // ----- Lifecycle -----

    /// Clear all instances, effects and buffers for a scene transition.
    pub fn clear(&mut self) {
        self.states.clear();
        self.detached.clear();
        self.effects.clear();
        self.instance_buffer.clear();
        self.ranges.clear();
        self.warned_missing.clear();
        self.time = 0.0;
    }

    /// Drop instances and particles but keep registered effects (editors
    /// re-simulating from t = 0).
    pub fn clear_instances(&mut self) {
        self.states.clear();
        self.detached.clear();
        self.instance_buffer.clear();
        self.ranges.clear();
        self.time = 0.0;
    }

    /// Restart every instance's timeline from zero and kill all particles.
    pub fn restart_all(&mut self) {
        for inst in self.states.values_mut().chain(self.detached.values_mut()) {
            inst.reset();
        }
        self.time = 0.0;
    }

    // ----- World sync -----

    /// Scan the world for `particle_emitter` and `particle_effect`
    /// components. Creates instances for new entities, re-resolves configs
    /// whose TOML changed, updates transforms, and drops despawned entities.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        let mut seen: HashSet<EntityId> = HashSet::new();

        // Inline emitters.
        for &entity_id in world.entities_with_component(comp::PARTICLE_EMITTER) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(table) = components
                .get(comp::PARTICLE_EMITTER)
                .and_then(|v| v.as_table())
            else {
                continue;
            };
            seen.insert(entity_id);
            let fp = fingerprint_table(table);

            if let Some(inst) = self.states.get_mut(&entity_id) {
                if inst.fingerprint != fp {
                    let cfg = EmitterConfig::from_toml(table);
                    apply_inline_config(inst, cfg);
                    inst.fingerprint = fp;
                }
            } else {
                let cfg = EmitterConfig::from_toml(table);
                let name = world.get_name(entity_id).unwrap_or("").to_string();
                let seed = mix_seed(0xDEAD_BEEF, entity_salt(world, entity_id));
                let mut inst = EffectInstance::new(EffectSource::Inline, name, vec![cfg], seed);
                inst.fingerprint = fp;
                self.states.insert(entity_id, inst);
            }
            Self::apply_world_transform(world, entity_id, self.states.get_mut(&entity_id).unwrap());
        }

        // Effect references.
        for &entity_id in world.entities_with_component(comp::PARTICLE_EFFECT) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(table) = components
                .get(comp::PARTICLE_EFFECT)
                .and_then(|v| v.as_table())
            else {
                continue;
            };
            let effect_name = table
                .get("effect")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if effect_name.is_empty() {
                continue;
            }
            let fp = fingerprint_table(table);
            let params = EffectParams::from_table(table);

            let needs_create = match self.states.get(&entity_id) {
                Some(inst) => {
                    let same_effect =
                        matches!(&inst.source, EffectSource::Asset(fx) if fx.name == effect_name);
                    !same_effect || inst.fingerprint == 0
                }
                None => true,
            };

            if needs_create {
                let Some(fx) = self.effects.get(&effect_name).cloned() else {
                    if self.warned_missing.insert(effect_name.clone()) {
                        tracing::warn!(
                            "particle_effect '{}' not found (looked in particles/ next to the scene)",
                            effect_name
                        );
                    }
                    continue;
                };
                let configs = match fx.resolve_all() {
                    Ok(c) => c,
                    Err(e) => {
                        if self.warned_missing.insert(effect_name.clone()) {
                            tracing::warn!(
                                "particle_effect '{}' failed to resolve: {e}",
                                effect_name
                            );
                        }
                        continue;
                    }
                };
                let base_seed = if params.seed != 0 {
                    params.seed
                } else if fx.seed != 0 {
                    fx.seed
                } else {
                    0xDEAD_BEEF
                };
                let seed = mix_seed(base_seed, entity_salt(world, entity_id));
                let mut inst = EffectInstance::new(
                    EffectSource::Asset(fx),
                    effect_name.clone(),
                    configs,
                    seed,
                );
                inst.effect_scale = params.scale;
                inst.emission_scale = params.emission_scale;
                inst.fingerprint = fp;
                if !params.playing {
                    inst.set_playing(false);
                }
                self.states.insert(entity_id, inst);
            } else if let Some(inst) = self.states.get_mut(&entity_id) {
                if inst.fingerprint != fp {
                    inst.effect_scale = params.scale;
                    inst.emission_scale = params.emission_scale;
                    if params.playing != inst.playing {
                        inst.set_playing(params.playing);
                    }
                    inst.fingerprint = fp;
                }
            }
            seen.insert(entity_id);
            if let Some(inst) = self.states.get_mut(&entity_id) {
                Self::apply_world_transform(world, entity_id, inst);
            }
        }

        // Remove states for despawned entities
        self.states.retain(|id, _| seen.contains(id));
    }

    fn apply_world_transform(world: &FlintWorld, id: EntityId, inst: &mut EffectInstance) {
        if let Some(m) = world.get_world_matrix(id) {
            let (pos, basis, scale) = decompose(&m);
            inst.set_transform(pos, basis, scale);
        }
    }

    // ----- Simulation -----

    /// Run particle simulation for all instances
    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        let total: usize = self.total_alive();
        let mut budget_left = self.budget.saturating_sub(total);
        let time = self.time;
        for inst in self.states.values_mut() {
            inst.step(dt, time, &mut budget_left);
        }
        let mut finished = Vec::new();
        for (handle, inst) in self.detached.iter_mut() {
            inst.step(dt, time, &mut budget_left);
            if !inst.any_emitter_playing() && inst.alive() == 0 {
                finished.push(*handle);
            }
        }
        for h in finished {
            self.detached.remove(&h);
        }
    }

    /// Pack alive particles into the instance buffer for GPU upload.
    /// `camera_pos` enables back-to-front sorting; call after `update()`.
    pub fn pack_instances(&mut self, camera_pos: Option<[f32; 3]>) {
        self.instance_buffer.clear();
        self.ranges.clear();

        let mut scratch = std::mem::take(&mut self.order_scratch);
        let all = self
            .states
            .iter()
            .map(|(id, inst)| (InstanceKey::Entity(*id), inst))
            .chain(
                self.detached
                    .iter()
                    .map(|(h, inst)| (InstanceKey::Detached(*h), inst)),
            );

        for (key, inst) in all {
            let sort_key = camera_pos.map_or(0.0, |c| dist2(c, inst.position).sqrt());
            for (ei, em) in inst.emitters.iter().enumerate() {
                let count = em.pool.alive_count();
                if count == 0 {
                    continue;
                }
                let cfg = &em.config;
                let alive = em.pool.alive_slice();
                let to_world = |p: [f32; 3]| -> [f32; 3] {
                    if cfg.world_space {
                        p
                    } else {
                        let q = if cfg.local_axes {
                            mul3(&inst.basis, p)
                        } else {
                            p
                        };
                        [
                            q[0] + inst.position[0],
                            q[1] + inst.position[1],
                            q[2] + inst.position[2],
                        ]
                    }
                };

                scratch.clear();
                scratch.extend(0..count);
                match (cfg.sort, camera_pos) {
                    (SortMode::BackToFront, Some(cam)) => {
                        scratch.sort_by(|&a, &b| {
                            let da = dist2(cam, to_world(alive[a].position));
                            let db = dist2(cam, to_world(alive[b].position));
                            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                    (SortMode::YoungestFirst, _) => scratch.sort_by(|&a, &b| {
                        alive[a]
                            .age
                            .partial_cmp(&alive[b].age)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }),
                    (SortMode::OldestFirst, _) => scratch.sort_by(|&a, &b| {
                        alive[b]
                            .age
                            .partial_cmp(&alive[a].age)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }),
                    _ => {}
                }

                let start = self.instance_buffer.len();
                for &i in &scratch {
                    let p = &alive[i];
                    let mut instance =
                        ParticleInstance::from_particle(p, cfg.frames_x, cfg.frames_y, cfg.stretch);
                    if !cfg.world_space {
                        let w = to_world(p.position);
                        instance.pos_size[0] = w[0];
                        instance.pos_size[1] = w[1];
                        instance.pos_size[2] = w[2];
                    }
                    self.instance_buffer.push(instance);
                }
                self.ranges.push(DrawRange {
                    key,
                    emitter: ei,
                    start,
                    count,
                    sort_key,
                });
            }
        }
        self.order_scratch = scratch;
    }

    /// Get the packed instance data
    pub fn instance_data(&self) -> &[ParticleInstance] {
        &self.instance_buffer
    }

    /// Draw data for each emitter that has alive particles, in deterministic
    /// instance order.
    pub fn draw_data(&self) -> Vec<ParticleDrawData<'_>> {
        self.ranges
            .iter()
            .filter_map(|r| {
                let inst = self.instance(r.key)?;
                let cfg = &inst.emitters.get(r.emitter)?.config;
                Some(ParticleDrawData {
                    key: r.key,
                    entity_id: r.key.entity(),
                    emitter_index: r.emitter,
                    emitter_name: &cfg.name,
                    instances: &self.instance_buffer[r.start..r.start + r.count],
                    blend_mode: cfg.blend_mode,
                    texture: &cfg.texture,
                    frames_x: cfg.frames_x,
                    frames_y: cfg.frames_y,
                    sort_key: r.sort_key,
                    soft_distance: cfg.soft_distance,
                    fade_near: cfg.fade_near,
                    fade_far: cfg.fade_far,
                    lighting: cfg.lighting,
                    fog: cfg.fog,
                    stretch: cfg.stretch,
                })
            })
            .collect()
    }

    // ----- Script / panel controls -----

    /// Queue a burst on every emitter of an entity's effect (script command)
    pub fn queue_burst(&mut self, entity_id: EntityId, count: u32) {
        if let Some(inst) = self.states.get_mut(&entity_id) {
            for e in &mut inst.emitters {
                e.pending_burst += count;
            }
        }
    }

    /// Start or stop an entity's effect without touching its component.
    pub fn set_playing(&mut self, entity_id: EntityId, playing: bool) {
        if let Some(inst) = self.states.get_mut(&entity_id) {
            inst.set_playing(playing);
        }
    }

    /// Spawn a detached, one-shot instance of a registered effect at
    /// `position`. Returns `false` if the effect is unknown.
    pub fn spawn_effect(&mut self, handle: u64, name: &str, position: [f32; 3]) -> bool {
        let Some(fx) = self.effects.get(name).cloned() else {
            return false;
        };
        let configs = match fx.resolve_all() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("play_effect('{name}') failed: {e}");
                return false;
            }
        };
        let base = if fx.seed != 0 { fx.seed } else { 0xDEAD_BEEF };
        let seed = mix_seed(base, (handle as u32) ^ ((handle >> 32) as u32));
        let mut inst =
            EffectInstance::new(EffectSource::Asset(fx), name.to_string(), configs, seed);
        inst.set_transform(position, IDENTITY, 1.0);
        inst.set_playing(true);
        self.detached.insert(handle, inst);
        true
    }

    /// Stop emission on a detached instance; its particles finish naturally.
    pub fn stop_effect(&mut self, handle: u64) {
        if let Some(inst) = self.detached.get_mut(&handle) {
            inst.set_playing(false);
        }
    }

    /// Adjust a detached instance. Supported params: `emission_scale`,
    /// `scale`, `playing` (> 0.5), `x`/`y`/`z` (position).
    pub fn set_effect_param(&mut self, handle: u64, param: &str, value: f64) -> bool {
        let Some(inst) = self.detached.get_mut(&handle) else {
            return false;
        };
        set_param(inst, param, value)
    }

    /// Same parameter surface for entity-bound instances (panels).
    pub fn set_instance_param(&mut self, key: InstanceKey, param: &str, value: f64) -> bool {
        match self.instance_mut(key) {
            Some(inst) => set_param(inst, param, value),
            None => false,
        }
    }

    // ----- Introspection -----

    pub fn instance(&self, key: InstanceKey) -> Option<&EffectInstance> {
        match key {
            InstanceKey::Entity(id) => self.states.get(&id),
            InstanceKey::Detached(h) => self.detached.get(&h),
        }
    }

    pub fn instance_mut(&mut self, key: InstanceKey) -> Option<&mut EffectInstance> {
        match key {
            InstanceKey::Entity(id) => self.states.get_mut(&id),
            InstanceKey::Detached(h) => self.detached.get_mut(&h),
        }
    }

    /// Every live instance in deterministic order (entities, then detached).
    pub fn instances(&self) -> impl Iterator<Item = (InstanceKey, &EffectInstance)> {
        self.states
            .iter()
            .map(|(id, inst)| (InstanceKey::Entity(*id), inst))
            .chain(
                self.detached
                    .iter()
                    .map(|(h, inst)| (InstanceKey::Detached(*h), inst)),
            )
    }

    pub fn instances_mut(&mut self) -> impl Iterator<Item = (InstanceKey, &mut EffectInstance)> {
        self.states
            .iter_mut()
            .map(|(id, inst)| (InstanceKey::Entity(*id), inst))
            .chain(
                self.detached
                    .iter_mut()
                    .map(|(h, inst)| (InstanceKey::Detached(*h), inst)),
            )
    }

    /// Number of live effect instances (entities + detached).
    pub fn instance_count(&self) -> usize {
        self.states.len() + self.detached.len()
    }

    /// Number of tracked emitters across all instances
    pub fn emitter_count(&self) -> usize {
        self.instances().map(|(_, i)| i.emitters.len()).sum()
    }

    /// Total alive particles across all emitters
    pub fn total_alive(&self) -> usize {
        self.instances().map(|(_, i)| i.alive()).sum()
    }

    /// Texture names referenced by every live instance and registered effect.
    pub fn texture_names(&self) -> Vec<String> {
        let mut names: HashSet<String> = HashSet::new();
        for (_, inst) in self.instances() {
            for e in &inst.emitters {
                if !e.config.texture.is_empty() {
                    names.insert(e.config.texture.clone());
                }
            }
        }
        for fx in self.effects.values() {
            for e in &fx.emitters {
                if !e.texture.is_empty() {
                    names.insert(e.texture.clone());
                }
            }
        }
        let mut v: Vec<String> = names.into_iter().collect();
        v.sort();
        v
    }

    /// Test/editor hook: insert an instance bound to an entity id directly.
    pub fn insert_instance(&mut self, entity_id: EntityId, inst: EffectInstance) {
        self.states.insert(entity_id, inst);
    }
}

fn set_param(inst: &mut EffectInstance, param: &str, value: f64) -> bool {
    match param {
        "emission_scale" => inst.emission_scale = value.max(0.0) as f32,
        "scale" => inst.effect_scale = value.max(0.0) as f32,
        "playing" => inst.set_playing(value > 0.5),
        "x" => inst.position[0] = value as f32,
        "y" => inst.position[1] = value as f32,
        "z" => inst.position[2] = value as f32,
        _ => return false,
    }
    true
}

/// Apply a re-parsed inline config, preserving play-state edges the way
/// scripts expect (`start_emitter` / `stop_emitter` flip `playing`).
fn apply_inline_config(inst: &mut EffectInstance, cfg: EmitterConfig) {
    let Some(em) = inst.emitters.first_mut() else {
        return;
    };
    let was = em.config.starts_playing();
    let now = cfg.starts_playing();
    em.replace_config(cfg);
    if now && !was {
        em.restart();
        inst.playing = true;
    } else if !now && was {
        em.playing = false;
        inst.playing = false;
    }
}

/// `particle_effect` component fields.
struct EffectParams {
    playing: bool,
    scale: f32,
    seed: u32,
    emission_scale: f32,
}

impl EffectParams {
    fn from_table(t: &toml::value::Table) -> Self {
        let playing = t.get("playing").and_then(|v| v.as_bool()).unwrap_or(false);
        let autoplay = t.get("autoplay").and_then(|v| v.as_bool()).unwrap_or(true);
        Self {
            playing: playing || autoplay,
            scale: t.get("scale").and_then(toml_f32).unwrap_or(1.0).max(0.0),
            seed: t
                .get("seed")
                .and_then(|v| v.as_integer())
                .map(|i| i as u32)
                .unwrap_or(0),
            emission_scale: t
                .get("emission_scale")
                .and_then(toml_f32)
                .unwrap_or(1.0)
                .max(0.0),
        }
    }
}

/// Order-independent structural hash of a TOML table (floats by bit pattern).
pub fn fingerprint_table(table: &toml::value::Table) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_table(table, &mut h);
    let v = h.finish();
    if v == 0 {
        1
    } else {
        v
    }
}

fn hash_table<H: Hasher>(table: &toml::value::Table, h: &mut H) {
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    keys.len().hash(h);
    for k in keys {
        k.hash(h);
        hash_value(&table[k], h);
    }
}

fn hash_value<H: Hasher>(v: &toml::Value, h: &mut H) {
    match v {
        toml::Value::String(s) => {
            0u8.hash(h);
            s.hash(h);
        }
        toml::Value::Integer(i) => {
            1u8.hash(h);
            i.hash(h);
        }
        toml::Value::Float(f) => {
            2u8.hash(h);
            f.to_bits().hash(h);
        }
        toml::Value::Boolean(b) => {
            3u8.hash(h);
            b.hash(h);
        }
        toml::Value::Datetime(d) => {
            4u8.hash(h);
            d.to_string().hash(h);
        }
        toml::Value::Array(a) => {
            5u8.hash(h);
            a.len().hash(h);
            for x in a {
                hash_value(x, h);
            }
        }
        toml::Value::Table(t) => {
            6u8.hash(h);
            hash_table(t, h);
        }
    }
}

/// Split a column-major 4×4 into translation, unit rotation basis and
/// uniform scale (mean column length).
pub fn decompose(m: &[[f32; 4]; 4]) -> ([f32; 3], [[f32; 3]; 3], f32) {
    let pos = [m[3][0], m[3][1], m[3][2]];
    let mut basis = IDENTITY;
    let mut scale_sum = 0.0;
    for c in 0..3 {
        let col = [m[c][0], m[c][1], m[c][2]];
        let len = (col[0] * col[0] + col[1] * col[1] + col[2] * col[2]).sqrt();
        if len > 1e-8 {
            basis[c] = [col[0] / len, col[1] / len, col[2] / len];
            scale_sum += len;
        } else {
            scale_sum += 1.0;
        }
    }
    (pos, basis, scale_sum / 3.0)
}

#[inline]
fn mul3(b: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        b[0][0] * v[0] + b[1][0] * v[1] + b[2][0] * v[2],
        b[0][1] * v[0] + b[1][1] * v[1] + b[2][1] * v[2],
        b[0][2] * v[0] + b[1][2] * v[1] + b[2][2] * v[2],
    ]
}

#[inline]
fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::Burst;

    fn burst_cfg(n: u32) -> EmitterConfig {
        EmitterConfig {
            emission_rate: 0.0,
            max_particles: 64,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            bursts: vec![Burst {
                time: 0.0,
                count_min: n,
                count_max: n,
                cycles: 1,
                interval: 0.0,
                probability: 1.0,
            }],
            ..Default::default()
        }
    }

    fn world_with_emitter(extra: &str) -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        // Pinned id: `spawn` draws from a process-wide counter, and seeds
        // derive from the entity id.
        let id = EntityId(4242);
        world.spawn_with_id(id, "fx").unwrap();
        let table: toml::Value = toml::from_str(&format!(
            "emission_rate = 0.0\nburst_count = 5\nlifetime_min = 100.0\nlifetime_max = 100.0\n{extra}"
        ))
        .unwrap();
        world
            .set_component(id, comp::PARTICLE_EMITTER, table)
            .unwrap();
        let tf: toml::Value = toml::from_str("position = [1.0, 2.0, 3.0]").unwrap();
        world.set_component(id, comp::TRANSFORM, tf).unwrap();
        (world, id)
    }

    #[test]
    fn pack_instances_produces_correct_count() {
        let mut sync = ParticleSync::new();
        let inst = EffectInstance::new(EffectSource::Inline, "a".into(), vec![burst_cfg(5)], 1);
        sync.insert_instance(EntityId(1), inst);
        sync.update(1.0 / 60.0);
        sync.pack_instances(None);
        assert_eq!(sync.instance_data().len(), 5);
        assert_eq!(sync.draw_data().len(), 1);
        assert_eq!(sync.draw_data()[0].entity_id, Some(EntityId(1)));
    }

    #[test]
    fn sync_discovers_and_reparses_only_on_change() {
        let (mut world, id) = world_with_emitter("");
        let mut sync = ParticleSync::new();
        sync.sync_from_world(&world);
        assert_eq!(sync.emitter_count(), 1);
        let fp = sync.instance(InstanceKey::Entity(id)).unwrap().fingerprint;
        assert_eq!(
            sync.instance(InstanceKey::Entity(id)).unwrap().position,
            [1.0, 2.0, 3.0]
        );

        sync.sync_from_world(&world);
        assert_eq!(
            sync.instance(InstanceKey::Entity(id)).unwrap().fingerprint,
            fp
        );

        world
            .set_field(
                id,
                comp::PARTICLE_EMITTER,
                "emission_rate",
                toml::Value::Float(9.0),
            )
            .unwrap();
        sync.sync_from_world(&world);
        let inst = sync.instance(InstanceKey::Entity(id)).unwrap();
        assert_ne!(inst.fingerprint, fp);
        assert_eq!(inst.emitters[0].config.emission_rate, 9.0);

        world.despawn(id).unwrap();
        sync.sync_from_world(&world);
        assert_eq!(sync.emitter_count(), 0);
    }

    #[test]
    fn stop_and_start_edges_follow_component_flags() {
        let (mut world, id) = world_with_emitter("autoplay = false\nplaying = true");
        let mut sync = ParticleSync::new();
        sync.sync_from_world(&world);
        sync.update(0.1);
        assert_eq!(sync.total_alive(), 5);
        world
            .set_field(
                id,
                comp::PARTICLE_EMITTER,
                "playing",
                toml::Value::Boolean(false),
            )
            .unwrap();
        sync.sync_from_world(&world);
        assert!(!sync.instance(InstanceKey::Entity(id)).unwrap().emitters[0].playing);
        world
            .set_field(
                id,
                comp::PARTICLE_EMITTER,
                "playing",
                toml::Value::Boolean(true),
            )
            .unwrap();
        sync.sync_from_world(&world);
        sync.update(0.1);
        assert_eq!(sync.total_alive(), 10, "restart re-fires the burst");
    }

    #[test]
    fn two_syncs_same_seed_are_bit_identical() {
        let run = || {
            let (world, _) = world_with_emitter("spread = 90.0\ndamping = 0.5\nspeed_max = 6.0");
            let mut sync = ParticleSync::new();
            for _ in 0..120 {
                sync.sync_from_world(&world);
                sync.update(1.0 / 60.0);
            }
            sync.pack_instances(Some([0.0, 5.0, 10.0]));
            sync.instance_data().to_vec()
        };
        let a = run();
        let b = run();
        assert!(!a.is_empty());
        assert_eq!(a, b);
    }

    #[test]
    fn budget_caps_total_alive() {
        let mut sync = ParticleSync::new();
        sync.set_budget(12);
        for i in 0..3 {
            let inst =
                EffectInstance::new(EffectSource::Inline, "a".into(), vec![burst_cfg(10)], i);
            sync.insert_instance(EntityId(i as u64 + 1), inst);
        }
        sync.update(1.0 / 60.0);
        assert_eq!(sync.total_alive(), 12);
    }

    #[test]
    fn pack_back_to_front_sorted() {
        let cfg = EmitterConfig {
            sort: SortMode::BackToFront,
            ..burst_cfg(0)
        };
        let mut inst = EffectInstance::new(EffectSource::Inline, "a".into(), vec![cfg], 1);
        for x in [1.0f32, 5.0, 3.0] {
            let p = inst.emitters[0].pool.spawn().unwrap();
            p.position = [x, 0.0, 0.0];
            p.lifetime = 10.0;
        }
        let mut sync = ParticleSync::new();
        sync.insert_instance(EntityId(1), inst);
        sync.pack_instances(Some([0.0, 0.0, 0.0]));
        let xs: Vec<f32> = sync.instance_data().iter().map(|i| i.pos_size[0]).collect();
        assert_eq!(xs, vec![5.0, 3.0, 1.0]);
    }

    #[test]
    fn local_space_particles_ride_the_emitter() {
        let cfg = EmitterConfig {
            world_space: false,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: [0.0; 3],
            ..burst_cfg(1)
        };
        let mut inst = EffectInstance::new(EffectSource::Inline, "a".into(), vec![cfg], 1);
        inst.set_transform([10.0, 0.0, 0.0], IDENTITY, 1.0);
        let mut sync = ParticleSync::new();
        sync.insert_instance(EntityId(1), inst);
        sync.update(1.0 / 60.0);
        sync.pack_instances(None);
        assert!((sync.instance_data()[0].pos_size[0] - 10.0).abs() < 1e-4);
        sync.instance_mut(InstanceKey::Entity(EntityId(1)))
            .unwrap()
            .position = [20.0, 0.0, 0.0];
        sync.pack_instances(None);
        assert!((sync.instance_data()[0].pos_size[0] - 20.0).abs() < 1e-4);
    }

    #[test]
    fn effect_component_spawns_n_emitters_and_detached_effects_expire() {
        let fx = ParticleEffect::from_toml_str(
            "name = \"fx\"\n[[emitters]]\nname = \"a\"\nemission_rate = 0.0\nburst_count = 2\nlifetime = 0.2\n[[emitters]]\nname = \"b\"\nemission_rate = 0.0\nburst_count = 3\nlifetime = 0.2\nduration = 0.1\nlooping = false\n",
            "t",
        )
        .unwrap();
        let mut world = FlintWorld::new();
        let id = world.spawn("holder").unwrap();
        let comp_val: toml::Value = toml::from_str("effect = \"fx\"\nscale = 2.0").unwrap();
        world
            .set_component(id, comp::PARTICLE_EFFECT, comp_val)
            .unwrap();

        let mut sync = ParticleSync::new();
        sync.sync_from_world(&world);
        assert_eq!(sync.emitter_count(), 0, "unknown effect is skipped");
        sync.register_effect(Arc::new(fx));
        sync.sync_from_world(&world);
        assert_eq!(sync.emitter_count(), 2);
        assert_eq!(
            sync.instance(InstanceKey::Entity(id)).unwrap().effect_scale,
            2.0
        );
        sync.update(1.0 / 60.0);
        assert_eq!(sync.total_alive(), 5);

        assert!(sync.spawn_effect(7, "fx", [1.0, 1.0, 1.0]));
        assert!(!sync.spawn_effect(8, "missing", [0.0; 3]));
        sync.update(1.0 / 60.0);
        assert_eq!(sync.instance_count(), 2);
        sync.stop_effect(7);
        for _ in 0..30 {
            sync.update(0.1);
        }
        assert_eq!(
            sync.instance_count(),
            1,
            "detached instance removed once silent and empty"
        );
    }

    #[test]
    fn fingerprint_stable_and_sensitive() {
        let a: toml::value::Table =
            toml::from_str("x = 1.0\ny = [1, 2]\nz = { k = \"v\" }").unwrap();
        let b: toml::value::Table =
            toml::from_str("z = { k = \"v\" }\ny = [1, 2]\nx = 1.0").unwrap();
        let c: toml::value::Table =
            toml::from_str("x = 1.5\ny = [1, 2]\nz = { k = \"v\" }").unwrap();
        assert_eq!(fingerprint_table(&a), fingerprint_table(&b));
        assert_ne!(fingerprint_table(&a), fingerprint_table(&c));
    }

    #[test]
    fn decompose_extracts_rotation_and_scale() {
        // 90° about Y, uniform scale 2, translated.
        let m = [
            [0.0, 0.0, -2.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0],
            [1.0, 2.0, 3.0, 1.0],
        ];
        let (pos, basis, scale) = decompose(&m);
        assert_eq!(pos, [1.0, 2.0, 3.0]);
        assert!((scale - 2.0).abs() < 1e-6);
        assert!((basis[0][2] + 1.0).abs() < 1e-6);
    }
}
