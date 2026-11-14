//! Cascaded Shadow Mapping
//!
//! Renders the scene from the primary directional light's perspective into
//! a depth texture array (one layer per cascade). The main shader samples
//! this texture to compute shadow factors.

use crate::primitives::{SkinnedVertex, Vertex};
use crate::wooze_pipeline::ShallowWoozeUniforms;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Number of shadow cascades
pub const CASCADE_COUNT: usize = 3;

/// Default shadow map resolution per cascade
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// Uniform data for a single shadow draw call
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowDrawUniforms {
    pub light_view_proj: [[f32; 4]; 4],
    pub model: [[f32; 4]; 4],
}

/// Byte stride between per-draw shadow uniforms in the shared draw buffer.
/// Dynamic uniform offsets must be multiples of
/// `min_uniform_buffer_offset_alignment`, which is 256 on every backend we
/// ship on; `ShadowDrawUniforms` is 128 bytes, so each slot is half padding.
pub const SHADOW_DRAW_STRIDE: u64 = 256;

/// Initial number of per-draw slots in the shared shadow draw buffer
/// (grows on demand; sized for a few hundred entities × cascades).
const SHADOW_DRAW_INITIAL_CAPACITY: u32 = 1024;

/// Uniform data passed to the main shader for shadow sampling
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowUniforms {
    pub cascade_view_proj: [[[f32; 4]; 4]; CASCADE_COUNT],
    /// xyz = the 3 cascade split distances; w = shadow-map texel size
    /// (1.0 / resolution). w rides the former padding slot: 0.0 means
    /// "unset" and shaders fall back to the legacy hardcoded 1/2048, so
    /// stale/default uniforms keep the exact pre-lever behavior.
    pub cascade_splits: [f32; 4],
    /// PCSS parameters (ADR 0057). xyz = per-cascade light-ortho depth
    /// range in world units (pull_back + radius — converts blocker-depth
    /// deltas back to world distances); w = tan(sun angular size).
    /// w = 0.0 means "PCSS off" and shaders take the legacy 3×3 PCF path
    /// verbatim, so stale/default uniforms keep pre-lever behavior.
    pub pcss: [f32; 4],
}

impl Default for ShadowUniforms {
    fn default() -> Self {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            cascade_view_proj: [identity; CASCADE_COUNT],
            cascade_splits: [0.0; 4],
            pcss: [0.0; 4],
        }
    }
}

/// The shadow mapping system
pub struct ShadowPass {
    pub shadow_pipeline: wgpu::RenderPipeline,
    /// `shadow_pipeline` culling nothing, for glTF `doubleSided` casters (#623).
    pub shadow_pipeline_double_sided: wgpu::RenderPipeline,
    pub shadow_bind_group_layout: wgpu::BindGroupLayout,
    pub skinned_shadow_pipeline: wgpu::RenderPipeline,
    /// `skinned_shadow_pipeline` culling nothing (#623).
    pub skinned_shadow_pipeline_double_sided: wgpu::RenderPipeline,
    pub skinned_shadow_bone_layout: wgpu::BindGroupLayout,
    pub shadow_texture: wgpu::Texture,
    pub shadow_view: wgpu::TextureView,
    pub cascade_views: Vec<wgpu::TextureView>,
    pub shadow_sampler: wgpu::Sampler,
    pub shadow_uniforms_buffer: wgpu::Buffer,
    /// One uniform buffer for every shadow draw of the frame (all cascades),
    /// bound once with a dynamic offset per draw. Replaces the per-draw
    /// `create_buffer_init` + `create_bind_group` pair, which cost more CPU
    /// than the draws themselves once a scene had a few hundred entities.
    pub draw_buffer: wgpu::Buffer,
    pub draw_bind_group: wgpu::BindGroup,
    /// Number of `SHADOW_DRAW_STRIDE` slots `draw_buffer` holds.
    pub draw_capacity: u32,
    pub resolution: u32,
    pub enabled: bool,
    /// Cascades drawn, `1..=CASCADE_COUNT` (FlintKart#55). Each is a draw
    /// of every caster; fewer cover the same distance at coarser texels.
    /// The texture keeps all its layers, so changing this allocates nothing.
    cascades: usize,
    /// How far from the camera the cascades reach, metres.
    distance: f32,
    /// The uniforms on the GPU say "no shadows" (see `write_off_uniforms`).
    pub(crate) shadows_written_off: bool,
    shadow_uniforms: ShadowUniforms,
    /// Its [`gpu_id`](crate::bind_cache::gpu_id): bind groups kept over its
    /// views and buffers name it (#139). A new resolution is a new pass.
    pub id: u64,
    /// The pipelines for shallow wooze's casters (#215), once a scene has some.
    pub(crate) wooze: Option<ShadowWooze>,
}

/// How a caster with shallow wooze goes into the cascades: the same depth pipelines
/// with a vertex stage that displaces, reading further into each draw's slot.
pub(crate) struct ShadowWooze {
    layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
    pub skinned_pipeline: wgpu::RenderPipeline,
    /// The two above culling nothing, for glTF `doubleSided` casters (#623).
    pub pipeline_double_sided: wgpu::RenderPipeline,
    pub skinned_pipeline_double_sided: wgpu::RenderPipeline,
    /// Over the same buffer as `draw_bind_group`, seeing more of each slot.
    pub bind_group: wgpu::BindGroup,
}

/// How much of a shadow draw slot a woozy caster reads.
const WOOZY_SHADOW_BINDING: u64 = (std::mem::size_of::<ShadowDrawUniforms>()
    + std::mem::size_of::<ShallowWoozeUniforms>()) as u64;

impl ShadowPass {
    pub fn new(device: &wgpu::Device, resolution: u32) -> Self {
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow_shader.wgsl").into()),
        });

        // Bind group layout for shadow draw uniforms
        let shadow_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<
                            ShadowDrawUniforms,
                        >() as u64),
                    },
                    count: None,
                }],
                label: Some("Shadow Draw Bind Group Layout"),
            });

        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Shadow Pipeline Layout"),
                bind_group_layouts: &[Some(&shadow_bind_group_layout)],
                immediate_size: 0,
            });

        let make_shadow = |label: &str, cull_mode: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_shadow"),
                    buffers: &[Some(Vertex::desc())],
                    compilation_options: Default::default(),
                },
                fragment: None, // Depth only, no fragment shader
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let shadow_pipeline = make_shadow("Shadow Depth Pipeline", Some(wgpu::Face::Back));
        // glTF `doubleSided` casters (#623): a card shadows from either side
        let shadow_pipeline_double_sided =
            make_shadow("Shadow Depth Pipeline (double-sided)", None);

        // Skinned shadow pipeline: bind group 0 = shadow uniforms, bind group 1 = bone matrices
        let skinned_shadow_bone_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Skinned Shadow Bone Bind Group Layout"),
            });

        let skinned_shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Skinned Shadow Pipeline Layout"),
                bind_group_layouts: &[
                    Some(&shadow_bind_group_layout),
                    Some(&skinned_shadow_bone_layout),
                ],
                immediate_size: 0,
            });

        let make_skinned_shadow = |label: &str, cull_mode: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&skinned_shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_skinned_shadow"),
                    buffers: &[Some(SkinnedVertex::desc())],
                    compilation_options: Default::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let skinned_shadow_pipeline =
            make_skinned_shadow("Skinned Shadow Depth Pipeline", Some(wgpu::Face::Back));
        let skinned_shadow_pipeline_double_sided =
            make_skinned_shadow("Skinned Shadow Depth Pipeline (double-sided)", None);

        // Create shadow depth texture array (one layer per cascade)
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shadow Map Array"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: CASCADE_COUNT as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        // Full array view for sampling in the main shader
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Shadow Map Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // Per-cascade views for rendering
        let cascade_views: Vec<wgpu::TextureView> = (0..CASCADE_COUNT as u32)
            .map(|i| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Shadow Cascade {} View", i)),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        // Comparison sampler for PCF
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow Comparison Sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Shadow uniforms for the main shader (cascade matrices + splits)
        let shadow_uniforms = ShadowUniforms::default();
        let shadow_uniforms_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shadow Uniforms Buffer"),
            contents: bytemuck::cast_slice(&[shadow_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let (draw_buffer, draw_bind_group) = Self::create_draw_buffer(
            device,
            &shadow_bind_group_layout,
            SHADOW_DRAW_INITIAL_CAPACITY,
        );

        Self {
            shadow_pipeline,
            shadow_pipeline_double_sided,
            shadow_bind_group_layout,
            skinned_shadow_pipeline,
            skinned_shadow_pipeline_double_sided,
            skinned_shadow_bone_layout,
            shadow_texture,
            shadow_view,
            cascade_views,
            shadow_sampler,
            shadow_uniforms_buffer,
            draw_buffer,
            draw_bind_group,
            draw_capacity: SHADOW_DRAW_INITIAL_CAPACITY,
            resolution,
            enabled: true,
            cascades: CASCADE_COUNT,
            distance: flint_core::quality::DEFAULT_SHADOW_DISTANCE,
            shadows_written_off: false,
            shadow_uniforms,
            id: crate::bind_cache::gpu_id(),
            wooze: None,
        }
    }

    fn create_draw_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow Draw Uniforms (all draws)"),
            size: SHADOW_DRAW_STRIDE * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ShadowDrawUniforms>() as u64),
                }),
            }],
            label: Some("Shadow Draw Bind Group (dynamic offset)"),
        });
        (buffer, bind_group)
    }

    /// Make sure the shared draw buffer holds at least `draws` slots,
    /// growing geometrically so a busy scene reallocates a handful of times.
    pub fn ensure_draw_capacity(&mut self, device: &wgpu::Device, draws: u32) {
        if draws <= self.draw_capacity {
            return;
        }
        let mut capacity = self.draw_capacity.max(1);
        while capacity < draws {
            capacity *= 2;
        }
        let (buffer, bind_group) =
            Self::create_draw_buffer(device, &self.shadow_bind_group_layout, capacity);
        self.draw_buffer = buffer;
        self.draw_bind_group = bind_group;
        self.draw_capacity = capacity;
        if let Some(wooze) = &mut self.wooze {
            wooze.bind_group = Self::woozy_bind_group(device, &wooze.layout, &self.draw_buffer);
        }
    }

    /// Build what woozy casters draw with, if it isn't already (#215).
    pub(crate) fn ensure_wooze(&mut self, device: &wgpu::Device) {
        if self.wooze.is_some() {
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shallow Wooze Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wooze_pipeline::SHADOW_SOURCE.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(WOOZY_SHADOW_BINDING),
                },
                count: None,
            }],
            label: Some("Shallow Wooze Shadow Bind Group Layout"),
        });
        let pipeline_for = |label: &str,
                            entry: &str,
                            groups: &[&wgpu::BindGroupLayout],
                            buffer: wgpu::VertexBufferLayout<'static>,
                            cull_mode: Option<wgpu::Face>| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups.iter().copied().map(Some).collect::<Vec<_>>(),
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    buffers: &[Some(buffer)],
                    compilation_options: Default::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = pipeline_for(
            "Shallow Wooze Shadow Pipeline",
            "vs_shadow_wooze",
            &[&layout],
            Vertex::desc(),
            Some(wgpu::Face::Back),
        );
        let pipeline_double_sided = pipeline_for(
            "Shallow Wooze Shadow Pipeline (double-sided)",
            "vs_shadow_wooze",
            &[&layout],
            Vertex::desc(),
            None,
        );
        let skinned_pipeline = pipeline_for(
            "Shallow Wooze Skinned Shadow Pipeline",
            "vs_skinned_shadow_wooze",
            &[&layout, &self.skinned_shadow_bone_layout],
            SkinnedVertex::desc(),
            Some(wgpu::Face::Back),
        );
        let skinned_pipeline_double_sided = pipeline_for(
            "Shallow Wooze Skinned Shadow Pipeline (double-sided)",
            "vs_skinned_shadow_wooze",
            &[&layout, &self.skinned_shadow_bone_layout],
            SkinnedVertex::desc(),
            None,
        );
        let bind_group = Self::woozy_bind_group(device, &layout, &self.draw_buffer);
        self.wooze = Some(ShadowWooze {
            layout,
            pipeline,
            skinned_pipeline,
            pipeline_double_sided,
            skinned_pipeline_double_sided,
            bind_group,
        });
    }

    fn woozy_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(WOOZY_SHADOW_BINDING),
                }),
            }],
            label: Some("Shallow Wooze Shadow Bind Group"),
        })
    }

    /// Upload one frame's shadow draw uniforms, one `SHADOW_DRAW_STRIDE`
    /// slot each, in the order the passes will bind them. `wooze` holds the drift
    /// of the slots whose casters have any (#215), by slot.
    pub(crate) fn write_draw_uniforms(
        &self,
        queue: &wgpu::Queue,
        draws: &[ShadowDrawUniforms],
        wooze: &[(u32, ShallowWoozeUniforms)],
    ) {
        if draws.is_empty() {
            return;
        }
        debug_assert!(draws.len() as u32 <= self.draw_capacity);
        let stride = SHADOW_DRAW_STRIDE as usize;
        let mut bytes = vec![0u8; stride * draws.len()];
        for (i, d) in draws.iter().enumerate() {
            let src = bytemuck::bytes_of(d);
            bytes[i * stride..i * stride + src.len()].copy_from_slice(src);
        }
        let past = std::mem::size_of::<ShadowDrawUniforms>();
        for (slot, drift) in wooze {
            let src = bytemuck::bytes_of(drift);
            let at = *slot as usize * stride + past;
            bytes[at..at + src.len()].copy_from_slice(src);
        }
        queue.write_buffer(&self.draw_buffer, 0, &bytes);
    }

    /// Upload uniforms that turn shadows off in every shader that samples
    /// the maps: a shadow far plane of 0 lights everything (the state a
    /// renderer starts in). Once, until the pass draws again.
    pub fn write_off_uniforms(&mut self, queue: &wgpu::Queue) {
        if self.shadows_written_off {
            return;
        }
        let off = ShadowUniforms::default();
        queue.write_buffer(
            &self.shadow_uniforms_buffer,
            0,
            bytemuck::cast_slice(&[off]),
        );
        self.shadows_written_off = true;
    }

    /// Cascades drawn; see [`Self::set_cascades`].
    pub fn cascades(&self) -> usize {
        self.cascades
    }

    /// Draw `n` cascades, clamped to `1..=CASCADE_COUNT`. The rest are left
    /// out of the shader's choice: their splits sit at the far plane.
    pub fn set_cascades(&mut self, n: usize) {
        self.cascades = n.clamp(1, CASCADE_COUNT);
    }

    /// How far shadows reach from the camera, metres.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// Reach `metres` from the camera (clamped as `QualitySettings` clamps).
    pub fn set_distance(&mut self, metres: f32) {
        use flint_core::quality::{MAX_SHADOW_DISTANCE, MIN_SHADOW_DISTANCE};
        if metres.is_finite() {
            self.distance = metres.clamp(MIN_SHADOW_DISTANCE, MAX_SHADOW_DISTANCE);
        }
    }

    /// Compute cascade splits and light view-projection matrices.
    ///
    /// `max_shadow_distance` extends the shadow frustum depth along the light
    /// direction so that off-screen shadow casters (ceilings, walls behind the
    /// camera, etc.) are captured in the shadow map.
    ///
    /// `cam_near` / `cam_far` are the camera projection's own planes, which
    /// turn the split distances into the frustum's slices; the cascades
    /// cover view depths from `cam_near` to [`Self::distance`] (or the far
    /// plane, if nearer). Before FlintKart#55 the planes were taken as 0.1
    /// and 200 whatever the camera's were, so with the usual 1000 m far
    /// plane the last cascade stretched to 1000 m while drawing to 200.
    #[allow(clippy::too_many_arguments)]
    pub fn update_cascades(
        &mut self,
        light_dir: [f32; 3],
        camera_pos: [f32; 3],
        camera_view_proj_inv: [[f32; 4]; 4],
        cam_near: f32,
        cam_far: f32,
        max_shadow_distance: f32,
        sun_tan_angular: f32,
    ) {
        // Practical cascade split scheme (logarithmic/uniform blend)
        let lambda = 0.5f32; // blend factor
        let count = self.cascades.clamp(1, CASCADE_COUNT);
        let (near, far) = (cam_near, self.distance.min(cam_far).max(cam_near * 2.0));
        let splits = cascade_split_slots(&compute_cascade_splits(near, far, count, lambda));

        self.shadow_uniforms.cascade_splits = [
            splits[1],
            splits[2],
            splits[3],
            1.0 / self.resolution as f32,
        ];

        let light_dir_norm = normalize_3(light_dir);

        for i in 0..count {
            let cascade_near = splits[i];
            let cascade_far = splits[i + 1];

            // Compute frustum corners for this cascade
            let corners = frustum_corners(
                cascade_near,
                cascade_far,
                cam_near,
                cam_far,
                &camera_view_proj_inv,
            );

            // Compute tight orthographic bounds from light's perspective
            let (light_vp, depth_range) = compute_light_matrix(
                &corners,
                &light_dir_norm,
                camera_pos,
                self.resolution,
                max_shadow_distance,
            );
            self.shadow_uniforms.cascade_view_proj[i] = light_vp;
            self.shadow_uniforms.pcss[i] = depth_range;
        }
        // Cascades not drawn copy the last one drawn: the shader never picks
        // them (their splits are the far plane), but stale matrices would be
        // a trap for anything that does.
        for i in count..CASCADE_COUNT {
            self.shadow_uniforms.cascade_view_proj[i] =
                self.shadow_uniforms.cascade_view_proj[count - 1];
            self.shadow_uniforms.pcss[i] = self.shadow_uniforms.pcss[count - 1];
        }
        // 0 = PCSS off → shaders take the legacy 3×3 PCF path (ADR 0057).
        self.shadow_uniforms.pcss[3] = sun_tan_angular;
    }

    pub fn shadow_uniforms(&self) -> &ShadowUniforms {
        &self.shadow_uniforms
    }
}

/// Splits for all `CASCADE_COUNT` slots from `count` cascades' splits: the
/// slots past `count` start at the far plane, so the shader's
/// `view_depth > split` test never picks them.
fn cascade_split_slots(splits: &[f32]) -> [f32; CASCADE_COUNT + 1] {
    let far = *splits.last().unwrap_or(&0.0);
    let mut out = [far; CASCADE_COUNT + 1];
    for (o, s) in out.iter_mut().zip(splits) {
        *o = *s;
    }
    out
}

/// Compute logarithmic/uniform blend cascade split distances
fn compute_cascade_splits(near: f32, far: f32, count: usize, lambda: f32) -> Vec<f32> {
    let mut splits = Vec::with_capacity(count + 1);
    splits.push(near);

    for i in 1..count {
        let ratio = i as f32 / count as f32;
        let log_split = near * (far / near).powf(ratio);
        let uniform_split = near + (far - near) * ratio;
        splits.push(lambda * log_split + (1.0 - lambda) * uniform_split);
    }

    splits.push(far);
    splits
}

/// Get the 8 corners of a frustum sub-section in world space
fn frustum_corners(
    cascade_near: f32,
    cascade_far: f32,
    cam_near: f32,
    cam_far: f32,
    inv_view_proj: &[[f32; 4]; 4],
) -> [[f32; 3]; 8] {
    // Perspective-correct NDC Z mapping (wgpu convention: near → 0, far → 1;
    // paired with perspective_matrix, ADR 0055).
    // The relationship between view distance d and NDC Z is hyperbolic, not linear:
    //   ndc_z = F/(F-N) - F*N / ((F-N)*d)  =  F*(d-N) / ((F-N)*d)
    let depth_range = cam_far - cam_near;
    let far_ratio = cam_far / depth_range;
    let prod_term = cam_far * cam_near / depth_range;
    let ndc_near = far_ratio - prod_term / cascade_near;
    let ndc_far = far_ratio - prod_term / cascade_far;

    let ndc_corners = [
        [-1.0, -1.0, ndc_near],
        [1.0, -1.0, ndc_near],
        [1.0, 1.0, ndc_near],
        [-1.0, 1.0, ndc_near],
        [-1.0, -1.0, ndc_far],
        [1.0, -1.0, ndc_far],
        [1.0, 1.0, ndc_far],
        [-1.0, 1.0, ndc_far],
    ];

    let mut world_corners = [[0.0f32; 3]; 8];
    for (i, ndc) in ndc_corners.iter().enumerate() {
        let p = mat4_mul_point(inv_view_proj, ndc);
        world_corners[i] = p;
    }
    world_corners
}

/// Compute a tight orthographic light view-projection matrix from frustum corners.
/// Snaps the shadow origin to texel boundaries to eliminate shadow swimming/shimmering
/// caused by sub-texel shifts when the camera moves.
///
/// Returns `(light_view_proj, depth_range)` where `depth_range` is the
/// ortho projection's world-unit depth extent (pull_back + radius) — PCSS
/// needs it to convert [0,1] blocker-depth deltas back to world distances
/// (ADR 0057).
fn compute_light_matrix(
    corners: &[[f32; 3]; 8],
    light_dir: &[f32; 3],
    _camera_pos: [f32; 3],
    shadow_resolution: u32,
    max_shadow_distance: f32,
) -> ([[f32; 4]; 4], f32) {
    // Compute frustum center
    let mut center = [0.0f32; 3];
    for c in corners {
        center[0] += c[0];
        center[1] += c[1];
        center[2] += c[2];
    }
    center[0] /= 8.0;
    center[1] /= 8.0;
    center[2] /= 8.0;

    // Compute the radius that encompasses all corners
    let mut radius = 0.0f32;
    for c in corners {
        let dx = c[0] - center[0];
        let dy = c[1] - center[1];
        let dz = c[2] - center[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist > radius {
            radius = dist;
        }
    }

    // Snap radius to avoid shimmering
    radius = radius.ceil();

    // Pull the light eye back far enough to capture shadow casters beyond the
    // camera frustum (ceilings, walls behind the camera, etc.).  XY bounds stay
    // tight to the frustum sphere so shadow resolution is preserved.
    let pull_back = radius.max(max_shadow_distance);
    let eye = [
        center[0] + light_dir[0] * pull_back,
        center[1] + light_dir[1] * pull_back,
        center[2] + light_dir[2] * pull_back,
    ];

    let view = look_at(&eye, &center);
    let proj = ortho(-radius, radius, -radius, radius, 0.0, pull_back + radius);

    let mut light_matrix = mat4_mul(&proj, &view);

    // Snap shadow origin to texel boundaries to prevent shadow swimming.
    // As the camera moves, the frustum center shifts continuously, causing
    // the shadow map texels to land on slightly different world positions
    // each frame. By rounding the projected origin to the nearest texel,
    // we quantize the shadow map position so it only moves in whole-texel
    // increments.
    let half_res = shadow_resolution as f32 * 0.5;
    // Column-major: m[col][row]. Column 3 holds the translation-like terms.
    let texel_x = light_matrix[3][0] * half_res;
    let texel_y = light_matrix[3][1] * half_res;
    light_matrix[3][0] += (texel_x.round() - texel_x) / half_res;
    light_matrix[3][1] += (texel_y.round() - texel_y) / half_res;

    (light_matrix, pull_back + radius)
}

fn normalize_3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        return [0.0, -1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross(a: &[f32; 3], b: &[f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Look-at view matrix (column-major for wgpu)
fn look_at(eye: &[f32; 3], target: &[f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize_3([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let up = [0.0, 1.0, 0.0];
    let s = normalize_3(cross(&f, &up));
    let u = cross(&s, &f);

    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(&s, eye), -dot(&u, eye), dot(&f, eye), 1.0],
    ]
}

/// Orthographic projection matrix (column-major)
fn ortho(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let rml = right - left;
    let tmb = top - bottom;
    let fmn = far - near;

    [
        [2.0 / rml, 0.0, 0.0, 0.0],
        [0.0, 2.0 / tmb, 0.0, 0.0],
        [0.0, 0.0, -1.0 / fmn, 0.0],
        [
            -(right + left) / rml,
            -(top + bottom) / tmb,
            -near / fmn,
            1.0,
        ],
    ]
}

fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

fn mat4_mul_point(m: &[[f32; 4]; 4], p: &[f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        out[row] = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row] * 1.0;
    }
    let w = out[3];
    if w.abs() < 1e-10 {
        return [out[0], out[1], out[2]];
    }
    [out[0] / w, out[1] / w, out[2] / w]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_cascades_cover_the_same_reach() {
        for count in 1..=CASCADE_COUNT {
            let splits = cascade_split_slots(&compute_cascade_splits(0.1, 120.0, count, 0.5));
            assert_eq!(splits[0], 0.1);
            // The shadow far plane (slot 3, the shader's `.z`) is the reach.
            assert_eq!(splits[CASCADE_COUNT], 120.0, "{count} cascades");
            for (i, s) in splits.iter().enumerate().skip(count + 1) {
                assert_eq!(*s, 120.0, "slot {i} of {count} must be unreachable");
            }
            assert!(splits.windows(2).all(|w| w[0] <= w[1]));
        }
    }
}
