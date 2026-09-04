//! Particle upload bridge and editor helpers (ADR 0068).
//!
//! Player, viewer, particle editor and `flint render` all feed particles to
//! the renderer through [`SceneRenderer::update_particles_from`], so the
//! `flint-particles` → GPU mapping lives in exactly one place.

use super::{DrawCall, SceneRenderer};
use crate::particle_pipeline::{sort_particle_draws, ParticleDrawCall, ParticleDrawData};
use crate::pipeline::{MaterialUniforms, TransformUniforms};
use crate::primitives::Mesh;
use flint_particles::ParticleSync;
use std::path::PathBuf;
use std::sync::Arc;

/// Cache name of the generated fallback sprite.
pub const SOFT_DISC_TEXTURE: &str = "__particle_soft_disc";

impl SceneRenderer {
    /// Upload every packed emitter of a [`ParticleSync`]. Call after
    /// `ParticleSystem::pack`.
    pub fn update_particles_from(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sync: &ParticleSync,
    ) {
        let draws: Vec<ParticleDrawData<'_>> = sync
            .draw_data()
            .into_iter()
            .map(|d| ParticleDrawData {
                instances: d.instances,
                texture: d.texture,
                blend: d.blend_mode,
                sort_key: d.sort_key,
            })
            .collect();
        self.update_particles(device, queue, &draws);
    }

    /// Low-level upload: concatenate all emitters' instances into the shared
    /// storage buffer (one `write_buffer`) and build sorted draw calls.
    pub fn update_particles(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[ParticleDrawData<'_>],
    ) {
        self.particle_draws.clear();
        if self.particle_pipeline.is_none() {
            return;
        }
        self.ensure_soft_disc_texture(device, queue);

        let total: usize = draws.iter().map(|d| d.instances.len()).sum();
        if total == 0 {
            return;
        }

        let mut scratch = std::mem::take(&mut self.particle_upload_scratch);
        scratch.clear();
        scratch.reserve(total);

        let mut calls = Vec::with_capacity(draws.len());
        for data in draws {
            if data.instances.is_empty() {
                continue;
            }
            let first_instance = scratch.len() as u32;
            scratch.extend_from_slice(data.instances);
            let texture_bind_group = self.particle_texture_bind_group(device, data.texture);
            calls.push(ParticleDrawCall {
                first_instance,
                instance_count: data.instances.len() as u32,
                texture_bind_group,
                blend: data.blend,
                sort_key: data.sort_key,
            });
        }

        if let Some(pp) = &mut self.particle_pipeline {
            pp.ensure_capacity(device, scratch.len() as u32);
            pp.write_instances(queue, &scratch);
        }
        sort_particle_draws(&mut calls);
        self.particle_draws = calls;
        self.particle_upload_scratch = scratch;
    }

    /// Cached texture bind group for a particle sprite (white when the name
    /// is empty or not loaded).
    fn particle_texture_bind_group(
        &mut self,
        device: &wgpu::Device,
        texture: &str,
    ) -> Arc<wgpu::BindGroup> {
        let key = if !texture.is_empty()
            && self
                .texture_cache
                .as_ref()
                .is_some_and(|tc| tc.contains(texture))
        {
            texture
        } else {
            ""
        };
        if let Some(bg) = self.particle_texture_bind_groups.get(key) {
            return bg.clone();
        }
        let pp = self
            .particle_pipeline
            .as_ref()
            .expect("particle pipeline present");
        let tc = self
            .texture_cache
            .as_ref()
            .expect("TextureCache required for particle rendering");
        let tex = if key.is_empty() {
            tc.get(SOFT_DISC_TEXTURE).unwrap_or(&tc.default_white)
        } else {
            tc.get(key).expect("checked above")
        };
        let bg = Arc::new(pp.create_texture_bind_group(
            device,
            &tex.view,
            &tex.sampler,
            &format!("Particle Texture Bind Group ({key})"),
        ));
        self.particle_texture_bind_groups
            .insert(key.to_string(), bg.clone());
        bg
    }

    /// Upload the procedural soft-disc sprite used when an emitter names no
    /// texture: a radial falloff so untextured particles read as glows,
    /// not squares (ADR 0068). Idempotent.
    fn ensure_soft_disc_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let already = self
            .texture_cache
            .as_ref()
            .is_some_and(|tc| tc.contains(SOFT_DISC_TEXTURE));
        if already {
            return;
        }
        const N: u32 = 64;
        let mut data = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                let u = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                let r = (u * u + v * v).sqrt();
                // Smooth falloff: solid core, feathered edge.
                let a = (1.0 - ((r - 0.35) / 0.65).clamp(0.0, 1.0)).powf(1.6);
                data.extend_from_slice(&[255, 255, 255, (a * 255.0).round() as u8]);
            }
        }
        if let Err(e) = self.load_texture_rgba(device, queue, SOFT_DISC_TEXTURE, N, N, &data, false)
        {
            tracing::warn!("soft disc particle texture upload failed: {e}");
        }
    }

    /// Drop cached particle texture bind groups (call when textures reload).
    pub fn invalidate_particle_textures(&mut self) {
        self.particle_texture_bind_groups.clear();
    }

    /// Editor/gizmo overlay: a line-list mesh drawn on top of the scene
    /// regardless of debug mode (unlike the skeleton overlay).
    pub fn set_debug_overlay(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        self.debug_overlay_draws.clear();
        if mesh.indices.is_empty() {
            return;
        }
        let tex_cache = self.texture_cache.as_ref().expect("texture cache");
        let draw: DrawCall = Self::create_draw_call(
            device,
            &self.pipeline,
            mesh,
            true,
            TransformUniforms::new(),
            MaterialUniforms::procedural(),
            tex_cache,
        );
        self.debug_overlay_draws.push(draw);
    }

    pub fn clear_debug_overlay(&mut self) {
        self.debug_overlay_draws.clear();
    }

    /// Background colour of the main pass (linear RGBA). Editors use this
    /// to review alpha-blended effects against light and dark backdrops.
    pub fn set_clear_color(&mut self, rgba: [f32; 4]) {
        self.clear_color = rgba;
    }

    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }
}

/// Load every texture referenced by the live particle instances and
/// registered effects into the renderer's texture cache. Names are cache
/// keys verbatim; files are searched in `search_dirs` in order.
pub fn load_particle_textures(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sync: &ParticleSync,
    search_dirs: &[PathBuf],
) {
    let mut any_new = false;
    for tex_name in sync.texture_names() {
        let mut found = false;
        for dir in search_dirs {
            let path = dir.join(&tex_name);
            if path.exists() {
                match renderer.load_texture_file(device, queue, &tex_name, &path) {
                    Ok(true) => {
                        println!("Loaded particle texture: {tex_name}");
                        any_new = true;
                    }
                    Ok(false) => {} // already cached
                    Err(e) => {
                        tracing::warn!("Failed to load particle texture '{}': {}", tex_name, e)
                    }
                }
                found = true;
                break;
            }
        }
        if !found {
            tracing::warn!("Particle texture not found: {}", tex_name);
        }
    }
    if any_new {
        renderer.invalidate_particle_textures();
    }
}
