//! GPU-instanced particle render pipeline
//!
//! Renders camera-facing quads via instanced draw calls. One pipeline per
//! [`ParticleBlendMode`] (alpha, additive, premultiplied, multiply), all
//! sharing a single persistent storage buffer of [`ParticleInstance`]s that
//! is grown on demand and written once per frame (ADR 0068). Instance data
//! is the same `#[repr(C)]` type the simulation packs — no per-crate copy.

use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub use flint_particles::{ParticleBlendMode, ParticleInstance};

/// Data for one emitter's particle draw, provided by the particle system
pub struct ParticleDrawData<'a> {
    pub instances: &'a [ParticleInstance],
    pub texture: &'a str,
    pub blend: ParticleBlendMode,
    /// View distance of the emitter origin; order-dependent blends draw
    /// far-to-near.
    pub sort_key: f32,
}

/// Camera uniforms shared across all particle draws in a frame
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ParticleUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub camera_right: [f32; 3],
    pub _pad0: f32,
    pub camera_up: [f32; 3],
    pub _pad1: f32,
}

/// A particle draw call (one per emitter with alive particles), indexing a
/// range of the shared instance buffer.
pub struct ParticleDrawCall {
    pub first_instance: u32,
    pub instance_count: u32,
    pub texture_bind_group: Arc<wgpu::BindGroup>,
    pub blend: ParticleBlendMode,
    pub sort_key: f32,
}

/// The particle rendering pipeline (one variant per blend mode)
pub struct ParticlePipeline {
    pipelines: [wgpu::RenderPipeline; 4],
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
    pub instance_bind_group_layout: wgpu::BindGroupLayout,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub quad_index_buffer: wgpu::Buffer,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
    instance_bind_group: wgpu::BindGroup,
}

const INITIAL_INSTANCE_CAPACITY: u32 = 1024;

impl ParticlePipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Particle Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particle_shader.wgsl").into()),
        });

        // Group 0: ParticleUniforms (camera data)
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Particle Uniform Bind Group Layout"),
            });

        // Group 1: Instance storage buffer (read-only)
        let instance_bind_group_layout =
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
                label: Some("Particle Instance Bind Group Layout"),
            });

        // Group 2: Texture + sampler
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("Particle Texture Bind Group Layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Particle Pipeline Layout"),
            bind_group_layouts: &[
                &uniform_bind_group_layout,
                &instance_bind_group_layout,
                &texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });

        // Shared depth stencil: depth test enabled, depth write DISABLED (translucent)
        let depth_stencil = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let make = |mode: ParticleBlendMode| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&format!("Particle {} Pipeline", mode.as_str())),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_particle"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_particle"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend_state(mode)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(depth_stencil.clone()),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };
        let pipelines = [
            make(ParticleBlendMode::Alpha),
            make(ParticleBlendMode::Additive),
            make(ParticleBlendMode::Premultiplied),
            make(ParticleBlendMode::Multiply),
        ];

        // Shared quad index buffer
        let quad_indices: [u32; 6] = [0, 1, 2, 2, 1, 3];
        let quad_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Quad Index Buffer"),
            contents: bytemuck::cast_slice(&quad_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Persistent uniform buffer for camera data
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Uniform Buffer"),
            contents: bytemuck::cast_slice(&[ParticleUniforms {
                view_proj: [[0.0; 4]; 4],
                camera_right: [1.0, 0.0, 0.0],
                _pad0: 0.0,
                camera_up: [0.0, 1.0, 0.0],
                _pad1: 0.0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("Particle Uniform Bind Group"),
        });

        let (instance_buffer, instance_bind_group) = Self::create_instance_buffer(
            device,
            &instance_bind_group_layout,
            INITIAL_INSTANCE_CAPACITY,
        );

        Self {
            pipelines,
            uniform_bind_group_layout,
            instance_bind_group_layout,
            texture_bind_group_layout,
            quad_index_buffer,
            uniform_buffer,
            uniform_bind_group,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            instance_bind_group,
        }
    }

    fn create_instance_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Particle Instance Buffer"),
            size: capacity as u64 * std::mem::size_of::<ParticleInstance>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Particle Instance Bind Group"),
        });
        (buffer, bind_group)
    }

    /// Pipeline variant for a blend mode.
    pub fn pipeline(&self, mode: ParticleBlendMode) -> &wgpu::RenderPipeline {
        &self.pipelines[mode.index()]
    }

    pub fn instance_bind_group(&self) -> &wgpu::BindGroup {
        &self.instance_bind_group
    }

    pub fn instance_capacity(&self) -> u32 {
        self.instance_capacity
    }

    /// Grow the shared instance buffer (next power of two) so it holds at
    /// least `needed` instances. Returns `true` when it was reallocated.
    pub fn ensure_capacity(&mut self, device: &wgpu::Device, needed: u32) -> bool {
        if needed <= self.instance_capacity {
            return false;
        }
        let capacity = needed.max(INITIAL_INSTANCE_CAPACITY).next_power_of_two();
        let (buffer, bind_group) =
            Self::create_instance_buffer(device, &self.instance_bind_group_layout, capacity);
        self.instance_buffer = buffer;
        self.instance_bind_group = bind_group;
        self.instance_capacity = capacity;
        true
    }

    /// Upload the frame's packed instances (call after `ensure_capacity`).
    pub fn write_instances(&self, queue: &wgpu::Queue, instances: &[ParticleInstance]) {
        if instances.is_empty() {
            return;
        }
        debug_assert!(instances.len() as u32 <= self.instance_capacity);
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
    }

    /// Bind group for a sprite texture.
    pub fn create_texture_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
            label: Some(label),
        })
    }
}

/// wgpu blend state for each particle blend mode.
pub fn blend_state(mode: ParticleBlendMode) -> wgpu::BlendState {
    use wgpu::{BlendComponent, BlendFactor, BlendOperation, BlendState};
    match mode {
        ParticleBlendMode::Alpha => BlendState::ALPHA_BLENDING,
        ParticleBlendMode::Additive => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
        },
        ParticleBlendMode::Premultiplied => BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        ParticleBlendMode::Multiply => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Dst,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::Zero,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
        },
    }
}

/// Order draws for correct blending: order-dependent modes far-to-near
/// (stable on the caller's deterministic order), additive last.
pub fn sort_particle_draws(draws: &mut [ParticleDrawCall]) {
    draws.sort_by(|a, b| {
        let ka = a.blend.is_order_independent();
        let kb = b.blend.is_order_independent();
        ka.cmp(&kb).then_with(|| {
            if ka {
                std::cmp::Ordering::Equal
            } else {
                b.sort_key
                    .partial_cmp(&a.sort_key)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_instance_type_is_64_bytes() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 64);
    }

    #[test]
    fn blend_index_matches_pipeline_order() {
        for (i, m) in ParticleBlendMode::ALL.iter().enumerate() {
            assert_eq!(m.index(), i);
        }
    }
}
