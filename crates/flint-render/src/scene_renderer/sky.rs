//! The sky behind the scene, owned by [`SkyRenderer`] (ADR 0118, #736):
//! the procedural sky a `sky` component drives, and the texture skybox a
//! scene's panorama loads. The procedural one wins when both are present.
//! Their pipelines are in `ScenePipelines`, since they bake in the sample
//! count; what they draw from is here.

use super::helpers::identity_matrix;
use crate::camera::{mat4_inverse, mat4_mul, Camera};
use crate::pipeline::{DirectionalLight, LightUniforms};
use crate::sky_pipeline::{SkyParams, SkyPipeline, SkyUniformsGpu};
use crate::skybox_pipeline::{SkyboxPipeline, SkyboxUniforms};
use crate::texture_cache::open_image;
use flint_core::components as comp;
use flint_ecs::FlintWorld;
use std::path::Path;
use wgpu::util::DeviceExt;

/// The procedural sky's uniforms and the scene's `sky` values, and the
/// texture skybox's binds when one is loaded.
pub(crate) struct SkyRenderer {
    // Procedural sky (replaces the texture skybox when a `sky` component exists)
    uniform_buffer: Option<wgpu::Buffer>,
    uniform_bind_group: Option<wgpu::BindGroup>,
    params: SkyParams,
    active: bool,
    skybox: Option<Skybox>,
}

/// A loaded panorama's uniform buffer and bind groups.
struct Skybox {
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_group: wgpu::BindGroup,
}

impl SkyRenderer {
    /// The procedural sky's uniform buffer and bind group, when its pipeline
    /// built (`ScenePipelines` leaves it out when its shader fails, and the
    /// renderer falls back to the texture skybox).
    pub(crate) fn new(device: &wgpu::Device, pipeline: Option<&SkyPipeline>) -> Self {
        let (uniform_buffer, uniform_bind_group) = match pipeline {
            Some(pipeline) => {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Sky Uniform Buffer"),
                    size: std::mem::size_of::<SkyUniformsGpu>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &pipeline.uniform_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                    label: Some("Sky Uniform Bind Group"),
                });
                (Some(buffer), Some(bind_group))
            }
            None => (None, None),
        };
        Self {
            uniform_buffer,
            uniform_bind_group,
            params: SkyParams::default(),
            active: false,
            skybox: None,
        }
    }

    /// The scene's `sky` values: the last `sky` component seen, or the
    /// defaults. The ocean reflects their colours.
    pub(crate) fn params(&self) -> &SkyParams {
        &self.params
    }

    /// Whether the procedural sky draws this frame.
    pub(crate) fn active(&self) -> bool {
        self.active
    }

    /// Extract the first `sky` component. When present, the procedural sky
    /// replaces the texture skybox, and its optional ambient fields override
    /// the hemisphere ambient in the light uniforms (so a time-of-day script
    /// can darken nights by writing component fields). `has_pipeline` is
    /// whether the procedural sky's pipeline built.
    pub(crate) fn extract(
        &mut self,
        world: &FlintWorld,
        has_pipeline: bool,
        light_uniforms: &mut LightUniforms,
    ) {
        self.active = false;
        for &entity_id in world.entities_with_component(comp::SKY) {
            let sky_comp = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::SKY).cloned());
            let Some(sky_comp) = sky_comp else { continue };

            self.params = SkyParams::from_component(&sky_comp);
            if let Some(ambient_sky) = self.params.ambient_sky {
                light_uniforms.ambient_sky = ambient_sky;
            }
            if let Some(ambient_ground) = self.params.ambient_ground {
                light_uniforms.ambient_ground = ambient_ground;
            }
            self.active = has_pipeline;
            break;
        }
    }

    /// Draw the sky at the far plane: the procedural sky when it is active
    /// and its pipeline built, else the texture skybox when one is loaded.
    /// The sun disc is `sun`, the scene's first directional light, so the
    /// drawn sun and the lighting can never disagree.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        queue: &wgpu::Queue,
        camera: &Camera,
        sky_pipeline: Option<&'a SkyPipeline>,
        skybox_pipeline: Option<&'a SkyboxPipeline>,
        sun: &DirectionalLight,
        time: f64,
    ) {
        let mut sky_drawn = false;
        if self.active {
            if let (Some(sp), Some(ub), Some(ubg)) =
                (sky_pipeline, &self.uniform_buffer, &self.uniform_bind_group)
            {
                let view = camera.view_matrix();
                let view_rot_only = [view[0], view[1], view[2], [0.0, 0.0, 0.0, 1.0]];
                let proj = camera.projection_matrix();
                let vp = mat4_mul(&proj, &view_rot_only);
                let inv_vp = mat4_inverse(&vp);

                let p = &self.params;
                let uniforms = SkyUniformsGpu {
                    inv_view_proj: inv_vp,
                    sun_dir: sun.direction,
                    sun_disc_size: p.sun_disc_size,
                    sun_color: [
                        sun.color[0] * sun.intensity,
                        sun.color[1] * sun.intensity,
                        sun.color[2] * sun.intensity,
                        1.0,
                    ],
                    zenith_color: p.zenith_color,
                    horizon_color: p.horizon_color,
                    haze_color: p.haze_color,
                    cloud_tint: p.cloud_tint,
                    sun_glow: p.sun_glow,
                    star_opacity: p.star_opacity,
                    cloud_coverage: p.cloud_coverage,
                    cloud_density: p.cloud_density,
                    cloud_scale: p.cloud_scale,
                    cloud_drift_x: p.cloud_drift_x,
                    cloud_drift_y: p.cloud_drift_y,
                    time: (time % 100_000.0) as f32,
                };
                queue.write_buffer(ub, 0, bytemuck::cast_slice(&[uniforms]));

                render_pass.set_pipeline(&sp.pipeline);
                render_pass.set_bind_group(0, ubg, &[]);
                render_pass.draw(0..3, 0..1);
                sky_drawn = true;
            }
        }

        // Render texture skybox (before everything else, at the far plane)
        if !sky_drawn {
            if let (Some(sp), Some(skybox)) = (skybox_pipeline, &self.skybox) {
                // Build view matrix with translation stripped (rotation only)
                let view = camera.view_matrix();
                let view_rot_only = [
                    view[0],
                    view[1],
                    view[2],
                    [0.0, 0.0, 0.0, 1.0], // zero out translation column
                ];
                let proj = camera.projection_matrix();
                let vp = mat4_mul(&proj, &view_rot_only);
                let inv_vp = mat4_inverse(&vp);

                queue.write_buffer(
                    &skybox.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[SkyboxUniforms {
                        inv_view_proj: inv_vp,
                    }]),
                );

                render_pass.set_pipeline(&sp.pipeline);
                render_pass.set_bind_group(0, &skybox.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, &skybox.texture_bind_group, &[]);
                render_pass.draw(0..3, 0..1);
            }
        }
    }

    /// Draw no skybox until the next [`load_skybox`](Self::load_skybox): a
    /// scene that names none starts without the last scene's (#723).
    pub(crate) fn clear_skybox(&mut self) {
        self.skybox = None;
    }

    /// Whether a skybox is loaded and will be drawn.
    pub(crate) fn has_skybox(&self) -> bool {
        self.skybox.is_some()
    }

    /// Load an equirectangular panorama image as a skybox
    pub(crate) fn load_skybox(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        skybox_pipeline: Option<&SkyboxPipeline>,
        path: &Path,
    ) {
        let skybox_pipeline = match skybox_pipeline {
            Some(p) => p,
            None => {
                tracing::warn!("Skybox pipeline not available");
                return;
            }
        };

        // Load panorama image
        let img = match open_image(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("Failed to load skybox '{}': {:?}", path.display(), e);
                return;
            }
        };
        let (width, height) = img.dimensions();

        // Create GPU texture
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Skybox Panorama Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            texture_size,
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Skybox Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skybox Uniform Buffer"),
            contents: bytemuck::cast_slice(&[SkyboxUniforms {
                inv_view_proj: identity_matrix(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create bind groups
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("Skybox Uniform Bind Group"),
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("Skybox Texture Bind Group"),
        });

        self.skybox = Some(Skybox {
            uniform_buffer,
            uniform_bind_group,
            texture_bind_group,
        });

        println!("Loaded skybox: {} ({}x{})", path.display(), width, height);
    }
}
