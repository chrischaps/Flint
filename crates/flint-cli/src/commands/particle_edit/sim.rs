//! The editor's preview simulation: one detached instance of the effect at
//! the origin, stepped in fixed increments so play and scrub agree.

use std::path::Path;
use std::sync::Arc;

use flint_particles::{ParticleEffect, ParticleSystem};
use flint_render::SceneRenderer;

/// Fixed simulation step (120 Hz: smooth scrubbing, cheap enough to re-sim).
pub const FIXED_DT: f32 = 1.0 / 120.0;

/// Handle of the single preview instance.
const HANDLE: u64 = 1;

pub struct PreviewSim {
    system: ParticleSystem,
    name: String,
    time: f32,
    accum: f32,
    last_step_ms: f32,
}

impl Default for PreviewSim {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewSim {
    pub fn new() -> Self {
        Self {
            system: ParticleSystem::with_seed(0x5EED_0068),
            name: String::new(),
            time: 0.0,
            accum: 0.0,
            last_step_ms: 0.0,
        }
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    pub fn alive(&self) -> usize {
        self.system.sync.total_alive()
    }

    pub fn step_ms(&self) -> f32 {
        self.last_step_ms
    }

    /// Alive count per emitter of the preview instance (by index).
    pub fn per_emitter_alive(&self) -> Vec<(usize, usize)> {
        self.system
            .sync
            .instance(flint_particles::InstanceKey::Detached(HANDLE))
            .map(|inst| {
                inst.emitters
                    .iter()
                    .map(|e| (e.pool.alive_count(), e.pool.capacity()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Replace the effect being previewed. Muted / non-solo emitters keep
    /// their definition but emit nothing (rates zeroed in the sim copy only).
    /// Resets the simulation to t = 0.
    pub fn rebuild(&mut self, effect: &ParticleEffect, muted: &[bool], solo: Option<usize>) {
        let mut fx = effect.clone();
        for (i, em) in fx.emitters.iter_mut().enumerate() {
            let silenced = muted.get(i).copied().unwrap_or(false) || solo.is_some_and(|s| s != i);
            if silenced {
                em.emission_rate = 0.0;
                em.emission_per_meter = 0.0;
                em.bursts.clear();
                em.burst_count = None;
            }
        }
        if fx.emitters.is_empty() || fx.validate().is_err() {
            // Keep the last valid preview; the UI shows the validation error.
            self.system.sync.clear_instances();
            self.time = 0.0;
            self.accum = 0.0;
            return;
        }
        self.name = fx.name.clone();
        self.system.sync.clear_instances();
        self.system.sync.register_effect(Arc::new(fx));
        self.system
            .sync
            .spawn_effect(HANDLE, &self.name, [0.0, 0.0, 0.0]);
        self.time = 0.0;
        self.accum = 0.0;
    }

    /// Deterministic seek: re-run from t = 0 in fixed steps.
    pub fn seek(&mut self, t: f32) {
        let steps = (t.max(0.0) / FIXED_DT).round() as u32;
        // Restart the existing instance rather than re-registering.
        self.system.sync.restart_all();
        if self.system.sync.instance_count() == 0 && !self.name.is_empty() {
            self.system
                .sync
                .spawn_effect(HANDLE, &self.name, [0.0, 0.0, 0.0]);
        }
        let start = std::time::Instant::now();
        for _ in 0..steps {
            self.system.step_detached(FIXED_DT);
        }
        self.last_step_ms = start.elapsed().as_secs_f32() * 1000.0;
        self.time = steps as f32 * FIXED_DT;
        self.accum = 0.0;
    }

    /// Advance by real time × speed in fixed steps. Returns `true` when the
    /// loop end was reached and the caller should restart from zero.
    pub fn advance(&mut self, real_dt: f32, speed: f32, looping: bool, loop_end: f32) -> bool {
        self.accum += real_dt.max(0.0) * speed.max(0.0);
        let start = std::time::Instant::now();
        let mut stepped = false;
        while self.accum >= FIXED_DT {
            if self.time >= loop_end {
                self.accum = 0.0;
                return looping;
            }
            self.system.step_detached(FIXED_DT);
            self.time += FIXED_DT;
            self.accum -= FIXED_DT;
            stepped = true;
        }
        if stepped {
            self.last_step_ms = start.elapsed().as_secs_f32() * 1000.0;
        }
        false
    }

    /// Pack + upload for this frame.
    pub fn upload(
        &mut self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_pos: [f32; 3],
    ) {
        self.system.pack(Some(camera_pos));
        renderer.update_particles_from(device, queue, &self.system.sync);
    }

    /// Load referenced textures from the effect's directory, its parent and
    /// grandparent (game root when effects live in `scenes/particles/`).
    pub fn load_textures(
        &self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effect_dir: &Path,
    ) {
        let mut dirs = vec![effect_dir.to_path_buf()];
        if let Some(p) = effect_dir.parent() {
            dirs.push(p.to_path_buf());
            if let Some(gp) = p.parent() {
                dirs.push(gp.to_path_buf());
            }
        }
        flint_render::load_particle_textures(renderer, device, queue, &self.system.sync, &dirs);
    }
}
