//! The scene pass's overdraw, on a replay (#1203): how many of its draws
//! shade each pixel.
//!
//! [`DrawControls::overdraw`](super::DrawControls) draws the scene pass a
//! second time after it, into targets of its own: every draw the frame's
//! description lists (the list's items through the main pass's selector,
//! grass, 2D sprite batches) through its pipeline's twin
//! ([`crate::pipeline_twins`]), which keeps the vertex stage and the depth
//! test and adds one to an `R16Float` count instead of shading. Depth is
//! written as the draw writes it, so a draw hidden behind an earlier one
//! adds nothing, as early depth spares it in the frame. The water is drawn
//! too, depth alone, so what lies beneath it is tested as in the frame; the
//! sky and the editor grid are not draws of the list and aren't drawn.
//! A heat-map pass then colours the count (`overdraw`, 8-bit). One sample
//! a pixel, whatever the frame's MSAA.
//!
//! What it can't see: a fragment the draw's own shader discards (an alpha
//! cut-out) is counted, as it is shaded, and writes depth it doesn't in the
//! frame.

use crate::gpu_counts::CountedDevice;
use crate::TrustedShader;
use std::collections::HashMap;

/// The count's format: blendable, exact to 2048 draws a pixel.
pub const COUNT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// The heat map's: its bytes are the palette's sRGB values as written.
pub const HEAT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The draws a pixel at which the heat map reaches its last colour: each
/// colour step is twice the draws of the one before (1, 3, 7, 15, 31).
pub const HEAT_TOP: u32 = 31;

const COUNT_WGSL: &str = "
@fragment
fn fs_count() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 0.0);
}
";

const HEAT_WGSL: &str = "
@group(0) @binding(0) var counts: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// Night, indigo, plum, rose, coral, peach, cream: a stop each doubling
const STOPS = array<vec3<f32>, 7>(
    vec3<f32>(0.051, 0.043, 0.102),
    vec3<f32>(0.173, 0.067, 0.373),
    vec3<f32>(0.447, 0.122, 0.506),
    vec3<f32>(0.718, 0.216, 0.475),
    vec3<f32>(0.945, 0.376, 0.365),
    vec3<f32>(0.996, 0.690, 0.471),
    vec3<f32>(0.988, 0.992, 0.749),
);

@fragment
fn fs_heat(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let n = max(textureLoad(counts, vec2<i32>(p.xy), 0).r, 0.0);
    // log2(1 + n) / 5: 1 draw 0.2, 3 draws 0.4 ... 31 draws 1
    let t = clamp(log2(1.0 + n) / 5.0, 0.0, 1.0);
    // Stops at 0, .2, .4, .6, .8, .9, 1
    var at = array<f32, 7>(0.0, 0.2, 0.4, 0.6, 0.8, 0.9, 1.0);
    var c = STOPS[6];
    for (var k = 0; k < 6; k = k + 1) {
        if t <= at[k + 1] {
            let f = (t - at[k]) / (at[k + 1] - at[k]);
            c = mix(STOPS[k], STOPS[k + 1], f);
            break;
        }
    }
    return vec4<f32>(c, 1.0);
}
";

pub(super) struct Targets {
    pub size: (u32, u32),
    pub count: wgpu::Texture,
    pub count_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub heat: wgpu::Texture,
    pub heat_view: wgpu::TextureView,
    heat_bind: wgpu::BindGroup,
}

struct Heat {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// The overdraw view's state: off on every frame a game draws.
#[derive(Default)]
pub(super) struct Overdraw {
    /// This frame draws it (`prepare`, from the draw controls).
    pub wanted: bool,
    /// Each scene pipeline's counting twin.
    pub twins: HashMap<wgpu::RenderPipeline, wgpu::RenderPipeline>,
    /// The water's, drawing depth alone.
    pub depth_twins: HashMap<wgpu::RenderPipeline, wgpu::RenderPipeline>,
    /// Pipelines with no twin (built while descriptions weren't kept).
    pub untwinned: Vec<String>,
    count_module: Option<wgpu::ShaderModule>,
    heat: Option<Heat>,
    pub targets: Option<Targets>,
}

impl Overdraw {
    /// Make the twins of `pipelines` (counting) and `depth_only` (the
    /// water's) this frame binds that aren't made yet.
    pub fn ensure_twins(
        &mut self,
        device: &wgpu::Device,
        pipelines: &[wgpu::RenderPipeline],
        depth_only: &[wgpu::RenderPipeline],
    ) {
        let module = self
            .count_module
            .get_or_insert_with(|| {
                device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Overdraw Count"),
                    source: wgpu::ShaderSource::Wgsl(COUNT_WGSL.into()),
                })
            })
            .clone();
        let counting = crate::pipeline_twins::TwinFragment {
            module: &module,
            entry_point: "fs_count",
            target: wgpu::ColorTargetState {
                format: COUNT_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
                write_mask: wgpu::ColorWrites::RED,
            },
            depth_format: DEPTH_FORMAT,
        };
        let depth = crate::pipeline_twins::TwinFragment {
            target: wgpu::ColorTargetState {
                format: COUNT_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            },
            ..counting.clone()
        };
        for (list, fragment, map) in [
            (pipelines, &counting, &mut self.twins),
            (depth_only, &depth, &mut self.depth_twins),
        ] {
            for p in list {
                if map.contains_key(p) {
                    continue;
                }
                match crate::pipeline_twins::twin(device, p, fragment) {
                    Some(t) => {
                        map.insert(p.clone(), t);
                    }
                    None => {
                        let name = format!("{p:?}");
                        if !self.untwinned.contains(&name) {
                            self.untwinned.push(name);
                        }
                    }
                }
            }
        }
    }

    /// The targets at `size`, made or remade.
    pub fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let heat = self.heat.get_or_insert_with(|| {
            let module = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Overdraw Heat Map"),
                source: wgpu::ShaderSource::Wgsl(HEAT_WGSL.into()),
            });
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Overdraw Heat Map Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Overdraw Heat Map Pipeline Layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
                label: Some("Overdraw Heat Map"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_heat"),
                    compilation_options: Default::default(),
                    targets: &[Some(HEAT_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
            Heat { pipeline, layout }
        });
        let texture = |label, format, usage| {
            device.create_texture_counted(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let read = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let count = texture(
            "Overdraw Count",
            COUNT_FORMAT,
            read | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture(
            "Overdraw Depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let heat_texture = texture("Overdraw Heat Map", HEAT_FORMAT, read);
        let count_view = count.create_view(&Default::default());
        let heat_bind = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            label: Some("Overdraw Heat Map"),
            layout: &heat.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&count_view),
            }],
        });
        self.targets = Some(Targets {
            size,
            depth_view: depth.create_view(&Default::default()),
            heat_view: heat_texture.create_view(&Default::default()),
            heat: heat_texture,
            count,
            count_view,
            heat_bind,
        });
    }

    /// Colour the count into the heat map.
    pub fn encode_heat(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(t), Some(heat)) = (&self.targets, &self.heat) else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Overdraw Heat Map"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.heat_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&heat.pipeline);
        pass.set_bind_group(0, &t.heat_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl super::SceneRenderer {
    /// Draw the scene pass's overdraw (#1203) into the overdraw targets,
    /// after the frame's own scene pass, in the same encoder: the same
    /// draws, camera and uniforms, so it is the frame's pass counted.
    pub(super) fn encode_overdraw(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        flint_core::scope!("encode_overdraw");
        let size = self
            .postprocess_resources
            .as_ref()
            .map_or(self.surface_size, |r| (r.width, r.height));
        // What this frame binds: the list's items, the grass, the sprites;
        // the water's, depth alone
        let mut counted: Vec<wgpu::RenderPipeline> = Vec::new();
        for item in self.draws.iter().filter(|i| i.drawn()) {
            if self.main_pass_draws(item) {
                for p in self.item_pipelines(item) {
                    if !counted.contains(&p) {
                        counted.push(p);
                    }
                }
            }
        }
        if let Some(g) = &self.pipelines.grass {
            counted.push(g.render_pipeline.clone());
        }
        if let Some(s) = &self.pipelines.sprite2d {
            counted.push(s.pipeline.clone());
        }
        let depth_only: Vec<wgpu::RenderPipeline> = self
            .pipelines
            .ocean
            .as_ref()
            .map(|o| vec![o.pipeline.clone(), o.body_pipeline.clone()])
            .unwrap_or_default();
        self.overdraw.ensure_twins(device, &counted, &depth_only);
        self.overdraw.ensure_targets(device, size);
        let Some(targets) = &self.overdraw.targets else {
            return;
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Overdraw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.count_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // As the frame's scene pass begins: the lights at group 2
            pass.set_bind_group(2, &self.lights.bind_group, &[]);
            self.render_normal_pass(
                &mut pass,
                super::RenderPhase::ALL,
                Some((&self.overdraw.twins, &self.overdraw.depth_twins)),
            );
        }
        self.overdraw.encode_heat(encoder);
    }

    /// The overdraw view's count and heat map, once a frame drew them.
    pub(super) fn overdraw_textures(&self) -> Option<(&wgpu::Texture, &wgpu::Texture)> {
        self.overdraw.targets.as_ref().map(|t| (&t.count, &t.heat))
    }

    /// Pipelines the overdraw view found no twin for, by name: their draws
    /// are left out of it.
    pub fn overdraw_untwinned(&self) -> &[String] {
        &self.overdraw.untwinned
    }
}
