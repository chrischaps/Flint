//! Every scene pipeline whose MSAA sample count is baked in, as one set
//! (ADR 0118 decision 1, #735).
//!
//! An MSAA change used to rebuild these field by field, and a pipeline
//! added later had to be added there by hand too: shallow wooze's sets, the
//! lighten echo and the soft edge weren't, so the next frame bound a 1x
//! pipeline in a 4x pass (a wgpu validation error, a panic natively).
//! Now [`ScenePipelines::build`] makes the whole set and the renderer
//! replaces it whole, so a new pipeline is one field here and the struct
//! literal in `build` won't compile without it. The sample count lives here
//! and nowhere else, so nothing can be built against a stale one.

use crate::billboard_pipeline::BillboardPipeline;
use crate::grass_pipeline::GrassPipeline;
use crate::ocean_pipeline::OceanPipeline;
use crate::particle_pipeline::ParticlePipeline;
use crate::pipeline::{RenderPipeline, SceneLayouts};
use crate::pipeline_cache::{shared, shared_some};
use crate::skinned_pipeline::SkinnedPipeline;
use crate::sky_pipeline::SkyPipeline;
use crate::skybox_pipeline::SkyboxPipeline;
use crate::sprite2d_pipeline::Sprite2dPipeline;
use crate::terrain_pipeline::TerrainPipeline;
use crate::wooze_pipeline::WoozePipelineSet;
use std::sync::Arc;

/// Which of the pipelines that may be missing a set has, so a rebuild makes
/// the same ones: shallow wooze's are built on the first woozy draw, and
/// grass, the ocean and the sky are left out when their shaders fail to
/// build on this GPU (their buffers are only made when they did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Present {
    pub wooze_standard: bool,
    pub wooze_skinned: bool,
    pub grass: bool,
    pub ocean: bool,
    pub sky: bool,
}

impl Present {
    /// What a new renderer builds: everything but what waits to be needed.
    pub const NEW: Self = Self {
        wooze_standard: false,
        wooze_skinned: false,
        grass: true,
        ocean: true,
        sky: true,
    };
}

/// The scene passes' pipelines at one sample count.
///
/// Each is an `Arc` from [`crate::pipeline_cache::shared`], keyed by the
/// format and sample count: their shaders and layouts are the binary's
/// own, so those two are all a set depends on, and with the cache on the
/// next renderer on the device takes the same set (#911). What they hold
/// besides pipelines and layouts is constant (the billboard quad, grass
/// blade and ocean grid meshes). The 2D sprites' pipeline is the
/// exception, built for each renderer: it holds a uniform buffer the frame
/// writes.
pub(crate) struct ScenePipelines {
    samples: u32,
    format: wgpu::TextureFormat,
    pub main: Arc<RenderPipeline>,
    pub skinned: Option<Arc<SkinnedPipeline>>,
    /// Shallow wooze (#215): built the first frame a draw needs them.
    pub wooze_standard: Option<Arc<WoozePipelineSet>>,
    pub wooze_skinned: Option<Arc<WoozePipelineSet>>,
    pub billboard: Option<Arc<BillboardPipeline>>,
    pub terrain: Option<Arc<TerrainPipeline>>,
    pub grass: Option<Arc<GrassPipeline>>,
    pub ocean: Option<Arc<OceanPipeline>>,
    pub sky: Option<Arc<SkyPipeline>>,
    pub particle: Option<Arc<ParticlePipeline>>,
    pub sprite2d: Option<Sprite2dPipeline>,
    pub skybox: Option<Arc<SkyboxPipeline>>,
}

impl ScenePipelines {
    /// Every pipeline for targets of `format` at `samples` MSAA samples,
    /// against `layouts`, building the optional ones `present` names. The
    /// layouts each pipeline makes for itself are equal to the last set's,
    /// which wgpu (and WebGPU) treat as the same layout, so bind groups made
    /// against the old set stay valid with the new one.
    pub fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        layouts: &SceneLayouts,
        present: Present,
    ) -> Self {
        let key = (format, samples);
        let main = shared(device, "scene: PBR", key, || {
            RenderPipeline::new(device, format, samples, layouts)
        });
        let skinned = shared(device, "scene: skinned", key, || {
            SkinnedPipeline::new(
                device,
                format,
                &layouts.transform_bind_group_layout,
                &layouts.material_bind_group_layout,
                &layouts.light_bind_group_layout,
                samples,
            )
        });
        let wooze_standard = present
            .wooze_standard
            .then(|| wooze_standard(device, &main, format, samples));
        let wooze_skinned = present
            .wooze_skinned
            .then(|| wooze_skinned(device, &skinned, format, samples));

        // Graceful degradation: a GPU that can't build one of these (no
        // compute shaders, a driver that crashes compiling) goes without it.
        let grass = present
            .grass
            .then(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared_some(device, "scene: grass", key, || {
                        GrassPipeline::new(
                            device,
                            format,
                            &layouts.transform_bind_group_layout,
                            &layouts.light_bind_group_layout,
                            samples,
                        )
                    })
                }))
                .unwrap_or_else(|_| {
                    tracing::warn!("Grass pipeline creation failed — grass disabled");
                    None
                })
            })
            .flatten();
        let ocean = present
            .ocean
            .then(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared(device, "scene: ocean", key, || {
                        OceanPipeline::new(
                            device,
                            format,
                            &layouts.transform_bind_group_layout,
                            &layouts.light_bind_group_layout,
                            samples,
                        )
                    })
                }))
                .map_err(|_| tracing::warn!("Ocean pipeline creation failed — ocean disabled"))
                .ok()
            })
            .flatten();
        let sky = present
            .sky
            .then(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared(device, "scene: sky", key, || {
                        SkyPipeline::new(device, format, samples)
                    })
                }))
                .map_err(|_| {
                    tracing::warn!("Sky pipeline creation failed — procedural sky disabled")
                })
                .ok()
            })
            .flatten();

        Self {
            samples,
            format,
            billboard: Some(shared(device, "scene: billboard", key, || {
                BillboardPipeline::new(device, format, samples)
            })),
            terrain: Some(shared(device, "scene: terrain", key, || {
                TerrainPipeline::new(
                    device,
                    format,
                    &layouts.transform_bind_group_layout,
                    &layouts.light_bind_group_layout,
                    samples,
                )
            })),
            particle: Some(shared(device, "scene: particles", key, || {
                ParticlePipeline::new(device, format, samples)
            })),
            sprite2d: Some(Sprite2dPipeline::new(device, format, samples)),
            skybox: Some(shared(device, "scene: skybox", key, || {
                SkyboxPipeline::new(device, format, samples)
            })),
            main,
            skinned: Some(skinned),
            wooze_standard,
            wooze_skinned,
            grass,
            ocean,
            sky,
        }
    }

    /// The same set at another sample count: what an MSAA change does.
    pub fn rebuilt(&self, device: &wgpu::Device, samples: u32, layouts: &SceneLayouts) -> Self {
        Self::build(device, self.format, samples, layouts, self.present())
    }

    /// The sample count every pipeline here was built with.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Which optional pipelines this set has. Names every field, so one
    /// added to the struct must be placed here (and in [`Present`]) to
    /// compile.
    pub fn present(&self) -> Present {
        let Self {
            samples: _,
            format: _,
            main: _,
            skinned: _,
            wooze_standard,
            wooze_skinned,
            billboard: _,
            terrain: _,
            grass,
            ocean,
            sky,
            particle: _,
            sprite2d: _,
            skybox: _,
        } = self;
        Present {
            wooze_standard: wooze_standard.is_some(),
            wooze_skinned: wooze_skinned.is_some(),
            grass: grass.is_some(),
            ocean: ocean.is_some(),
            sky: sky.is_some(),
        }
    }

    /// Build shallow wooze's standard and skinned sets if asked for and not
    /// built yet, at this set's sample count.
    pub fn ensure_wooze(&mut self, device: &wgpu::Device, standard: bool, skinned: bool) {
        if standard && self.wooze_standard.is_none() {
            self.wooze_standard = Some(wooze_standard(
                device,
                &self.main,
                self.format,
                self.samples,
            ));
        }
        if skinned && self.wooze_skinned.is_none() {
            if let Some(base) = &self.skinned {
                self.wooze_skinned = Some(wooze_skinned(device, base, self.format, self.samples));
            }
        }
    }
}

/// Shallow wooze's standard set over `main`, which is itself keyed by the
/// same format and sample count.
fn wooze_standard(
    device: &wgpu::Device,
    main: &RenderPipeline,
    format: wgpu::TextureFormat,
    samples: u32,
) -> Arc<WoozePipelineSet> {
    shared(device, "scene: wooze", (format, samples), || {
        WoozePipelineSet::standard(device, main, format, samples)
    })
}

/// Shallow wooze's skinned set over `base`.
fn wooze_skinned(
    device: &wgpu::Device,
    base: &SkinnedPipeline,
    format: wgpu::TextureFormat,
    samples: u32,
) -> Arc<WoozePipelineSet> {
    shared(device, "scene: wooze skinned", (format, samples), || {
        WoozePipelineSet::skinned(device, base, format, samples)
    })
}
