//! The building blocks most styles are made of: a full-screen triangle
//! pipeline, the two bind group shapes the post chain uses, a uniform
//! buffer and the pass that draws one triangle. With these a style is
//! mostly its shader and its uniform struct.

/// A fragment-stage uniform buffer at binding 0.
pub fn uniform_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// A filterable float texture at binding 0 and a filtering sampler at
/// binding 1 (bind it with [`StyleFrame::sampled`](super::StyleFrame::sampled)).
pub fn sampled_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
    })
}

/// A float texture at binding 0 with no sampler, read with `textureLoad`:
/// the shared edges (bind them with
/// [`StyleFrame::edges_bind`](super::StyleFrame::edges_bind), #301).
pub fn texture_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// The entity style mask at binding 0: a `texture_2d<u32>` read with
/// `textureLoad` (bind it with [`StyleFrame::mask_bind`](super::StyleFrame::mask_bind)).
pub fn mask_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    uint_layout(device, label)
}

/// A `texture_2d<u32>` at binding 0, read with `textureLoad`: the style
/// mask, or a style's own integer state (ASCII's cells, #277).
pub fn uint_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// A pipeline drawing one full-screen triangle with `module`'s `vs_main`
/// and `fs_main` into `format`, unblended.
pub fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    fullscreen_pipeline_with(device, label, module, "fs_main", layouts, format, None)
}

/// [`fullscreen_pipeline`] with the fragment entry point `fragment`, for a
/// module that has more than one.
pub fn fullscreen_pipeline_entry(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    fullscreen_pipeline_entry_targets(device, label, module, fragment, layouts, &[format], None)
}

/// [`fullscreen_pipeline_entry`] with a blend: a style with several passes
/// in one module (a bloom chain's threshold, downsample, additive upsample
/// and composite, #321).
pub fn fullscreen_pipeline_with(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    fullscreen_pipeline_entry_targets(device, label, module, fragment, layouts, &[format], blend)
}

/// [`fullscreen_pipeline`] writing one target per format, `fs_main`'s
/// `@location(i)` into the i-th (#313).
pub fn fullscreen_pipeline_targets(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layouts: &[&wgpu::BindGroupLayout],
    formats: &[wgpu::TextureFormat],
) -> wgpu::RenderPipeline {
    fullscreen_pipeline_entry_targets(device, label, module, "fs_main", layouts, formats, None)
}

/// The full-screen pipelines' one maker: entry point `fragment`, a target
/// per format, each with `blend`.
fn fullscreen_pipeline_entry_targets(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    formats: &[wgpu::TextureFormat],
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|format| {
            Some(wgpu::ColorTargetState {
                format: *format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            targets: &targets,
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// A WGSL module.
pub fn shader(device: &wgpu::Device, label: &str, source: &'static str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// A uniform buffer the size of `T`, written with `queue.write_buffer`.
pub fn uniform_buffer<T>(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: std::mem::size_of::<T>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Draw one full-screen triangle into `target`, cleared to black first,
/// with `groups` bound in order.
pub fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
) {
    crate::postprocess::fullscreen_pass(
        encoder,
        label,
        target,
        Some(wgpu::Color::BLACK),
        pipeline,
        groups,
    );
}

/// [`fullscreen_pass`] into several targets at once (a pipeline from
/// [`fullscreen_pipeline_targets`]), each cleared to black first.
pub fn fullscreen_pass_targets(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    targets: &[&wgpu::TextureView],
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
) {
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = targets
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.draw(0..3, 0..1);
}
