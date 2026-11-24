//! The fixed cost of a small pass, render against compute (#1269):
//! `cargo run -p flint-render --example pass_floor [iterations] [reps]`
//! (`WGPU_BACKEND=dx12` for the other backend; a debug build times the GPU
//! as well as a release one).
//!
//! ASCII's pre-passes over styled props (three tone-statistics passes and
//! the variant cells) are four tiny render passes, three of them each
//! waiting on the last. #1269 asked whether running them as dispatches of
//! one compute pass, a barrier between dispatches in place of a render
//! pass's begin, clear and end, is much cheaper. This measures the floor of
//! each shape with timestamp queries, on targets the size ASCII's are:
//!
//! - `render x1`, `compute x1`: one step alone.
//! - `render x4 chained`: four render passes, each a full-screen triangle
//!   whose fragment writes zeros, cleared first; steps 2 and 3 read the step
//!   before, step 4 the picture, as ASCII's do.
//! - `compute 1 pass x4 chained`: one compute pass, four dispatches writing
//!   the same texels with `textureStore`, reading the same way.
//! - `compute 4 passes chained`: four compute passes of one dispatch each.
//! - `... free`: the same four steps, each reading only the picture.
//! - `nothing`: the two timestamps alone.
//!
//! Each case is timed `reps` times inside one bracket and divided (16 by
//! default; 1 is a frame's single run, at the timestamps' resolution), the
//! cases alternated every iteration; the report is the minimum, p10 and
//! median, in microseconds.
//!
//! The answer on an RTX 4070 laptop (2026-11-24, minimum of three
//! processes, 16 reps): a chain of four costs the same as render passes or
//! as dispatches on Vulkan (7.4 against 7.3 us) and more as dispatches on
//! DX12 (3.8 against 5.8 us); one compute pass alone is twice a render
//! pass on Vulkan (4.7 against 2.3 us). Dispatches win only when no step
//! reads another's (Vulkan 7.3 against 5.9 us). The barrier a dependency
//! needs is the cost, not the render pass around it, so #1269 kept ASCII's
//! render passes (the book's Profiling guide, "What a pass costs before it
//! draws").

#![allow(clippy::print_stdout, clippy::disallowed_methods)]

use flint_render::TrustedShader;

/// Sequences timed inside one bracket (the second argument overrides it).
const DEFAULT_REPS: u32 = 16;

/// ASCII's tone statistics' sizes: a column a slot (8 slots here), the runs
/// STATS_ROWS x STATS_CHUNKS tall, the rows STATS_ROWS, the resolve one.
const STATS_W: u32 = 8;
const RUNS_H: u32 = 180 * 16;
const ROWS_H: u32 = 180;
/// A variant cell band: about 1080p's cells at 8 x 8 points.
const CELLS: [u32; 2] = [240, 135];

const RENDER_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    // A dependence on the last pass the compiler can't drop, never taken
    if p.x < -1.0 { return textureLoad(src, vec2<i32>(0), 0); }
    return vec4<f32>(0.0);
}
@fragment fn fs_u(@builtin(position) p: vec4<f32>) -> @location(0) vec4<u32> {
    if p.x < -1.0 { return vec4<u32>(textureLoad(src, vec2<i32>(0), 0)); }
    return vec4<u32>(0u);
}
"#;

const COMPUTE_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba32float, write>;
@compute @workgroup_size(8, 8) fn cs(@builtin(global_invocation_id) g: vec3<u32>) {
    let size = textureDimensions(dst);
    if any(g.xy >= size) { return; }
    var v = vec4<f32>(0.0);
    if f32(g.x) < -1.0 { v = textureLoad(src, vec2<i32>(0), 0); }
    textureStore(dst, g.xy, v);
}
"#;

const COMPUTE_U_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba32uint, write>;
@compute @workgroup_size(8, 8) fn cs(@builtin(global_invocation_id) g: vec3<u32>) {
    let size = textureDimensions(dst);
    if any(g.xy >= size) { return; }
    var v = vec4<u32>(0u);
    if f32(g.x) < -1.0 { v = vec4<u32>(textureLoad(src, vec2<i32>(0), 0)); }
    textureStore(dst, g.xy, v);
}
"#;

struct Target {
    view: wgpu::TextureView,
    size: [u32; 2],
    uint: bool,
}

fn target(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> Target {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("floor target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    Target {
        view: t.create_view(&Default::default()),
        size,
        uint: format == wgpu::TextureFormat::Rgba32Uint,
    }
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(400);
    let reps: u32 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_REPS);
    let instance = flint_render::gpu_select::instance();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let info = adapter.get_info();
    println!("{} ({:?})", info.name, info.backend);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pass floor"),
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))
    .expect("device");
    let period = queue.get_timestamp_period() as f64;

    let f = wgpu::TextureFormat::Rgba32Float;
    let u = wgpu::TextureFormat::Rgba32Uint;
    // A picture the first pass and the cells read, then the chain
    let picture = target(&device, [16, 16], f);
    let runs = target(&device, [STATS_W, RUNS_H], f);
    let rows = target(&device, [STATS_W, ROWS_H], f);
    let stats = target(&device, [STATS_W, 1], f);
    let cells = target(&device, CELLS, u);
    // (source, destination) per step, in the frame's order
    let chain: [(&Target, &Target); 4] = [
        (&picture, &runs),
        (&runs, &rows),
        (&rows, &stats),
        (&picture, &cells),
    ];
    // The same writes, none reading another's: every step reads the picture
    let free: [(&Target, &Target); 4] = [
        (&picture, &runs),
        (&picture, &rows),
        (&picture, &stats),
        (&picture, &cells),
    ];

    let render_module = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("floor render"),
        source: wgpu::ShaderSource::Wgsl(RENDER_WGSL.into()),
    });
    let compute_f = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("floor compute f"),
        source: wgpu::ShaderSource::Wgsl(COMPUTE_WGSL.into()),
    });
    let compute_u = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("floor compute u"),
        source: wgpu::ShaderSource::Wgsl(COMPUTE_U_WGSL.into()),
    });

    let sampled = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[sampled(0, wgpu::ShaderStages::FRAGMENT)],
    });
    let storage_bgl = |format| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                sampled(0, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        })
    };
    let bgl_f = storage_bgl(f);
    let bgl_u = storage_bgl(u);

    let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&render_bgl)],
        immediate_size: 0,
    });
    let render_pipe = |entry: &str, format| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor render"),
            layout: Some(&render_layout),
            vertex: wgpu::VertexState {
                module: &render_module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_module,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let render_f = render_pipe("fs", f);
    let render_u = render_pipe("fs_u", u);
    let compute_pipe = |bgl: &wgpu::BindGroupLayout, module: &wgpu::ShaderModule| {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(bgl)],
            immediate_size: 0,
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("floor compute"),
            layout: Some(&layout),
            module,
            entry_point: Some("cs"),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let comp_f = compute_pipe(&bgl_f, &compute_f);
    let comp_u = compute_pipe(&bgl_u, &compute_u);

    let render_binds_of = |chain: &[(&Target, &Target); 4]| -> Vec<wgpu::BindGroup> {
        chain
            .iter()
            .map(|(src, _)| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &render_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src.view),
                    }],
                })
            })
            .collect()
    };
    let compute_binds_of = |chain: &[(&Target, &Target); 4]| -> Vec<wgpu::BindGroup> {
        chain
            .iter()
            .map(|(src, dst)| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: if dst.uint { &bgl_u } else { &bgl_f },
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&dst.view),
                        },
                    ],
                })
            })
            .collect()
    };
    let binds = [
        (render_binds_of(&chain), compute_binds_of(&chain)),
        (render_binds_of(&free), compute_binds_of(&free)),
    ];

    let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let stamp = |encoder: &mut wgpu::CommandEncoder, i: u32| {
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("stamp"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &query_set,
                beginning_of_pass_write_index: Some(i),
                end_of_pass_write_index: None,
            }),
        });
    };

    #[derive(Clone, Copy, PartialEq)]
    enum Case {
        Nothing,
        Render,
        ComputeOnePass,
        ComputePasses,
    }
    let groups = |dst: &Target| [dst.size[0].div_ceil(8), dst.size[1].div_ceil(8)];
    let record = |encoder: &mut wgpu::CommandEncoder, case: Case, set: usize, steps: usize| {
        let (render_binds, compute_binds) = &binds[set];
        let chain = &chain[..steps];
        match case {
            Case::Nothing => {}
            Case::Render => {
                for (i, (_, dst)) in chain.iter().enumerate() {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("floor render"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &dst.view,
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
                    pass.set_pipeline(if dst.uint { &render_u } else { &render_f });
                    pass.set_bind_group(0, &render_binds[i], &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            Case::ComputeOnePass => {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("floor compute"),
                    timestamp_writes: None,
                });
                for (i, (_, dst)) in chain.iter().enumerate() {
                    pass.set_pipeline(if dst.uint { &comp_u } else { &comp_f });
                    pass.set_bind_group(0, &compute_binds[i], &[]);
                    let [x, y] = groups(dst);
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
            Case::ComputePasses => {
                for (i, (_, dst)) in chain.iter().enumerate() {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("floor compute"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(if dst.uint { &comp_u } else { &comp_f });
                    pass.set_bind_group(0, &compute_binds[i], &[]);
                    let [x, y] = groups(dst);
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
        }
    };

    // (name, case, 0 chained / 1 independent, steps)
    let cases = [
        ("nothing", Case::Nothing, 0, 4),
        ("render x1", Case::Render, 0, 1),
        ("compute x1", Case::ComputeOnePass, 0, 1),
        ("render x4 chained", Case::Render, 0, 4),
        ("compute 1 pass x4 chained", Case::ComputeOnePass, 0, 4),
        ("compute 4 passes chained", Case::ComputePasses, 0, 4),
        ("render x4 free", Case::Render, 1, 4),
        ("compute 1 pass x4 free", Case::ComputeOnePass, 1, 4),
    ];
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); cases.len()];
    // Alternate the cases every iteration, so a clock change hits them all
    for it in 0..iterations + 20 {
        for (c, (_, case, set, steps)) in cases.iter().enumerate() {
            let mut encoder = device.create_command_encoder(&Default::default());
            stamp(&mut encoder, 0);
            for _ in 0..reps {
                record(&mut encoder, *case, *set, *steps);
            }
            stamp(&mut encoder, 1);
            encoder.resolve_query_set(&query_set, 0..2, &resolve, 0);
            encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, 16);
            queue.submit([encoder.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            flint_render::wait_for_gpu(&device).expect("wait");
            let ticks: [u64; 2] = {
                let data = read.slice(..).get_mapped_range().expect("mapped");
                [
                    u64::from_le_bytes(data[0..8].try_into().unwrap()),
                    u64::from_le_bytes(data[8..16].try_into().unwrap()),
                ]
            };
            read.unmap();
            // The first iterations warm the clocks and caches
            if it >= 20 && ticks[1] > ticks[0] {
                let us = (ticks[1] - ticks[0]) as f64 * period / 1e3 / reps as f64;
                times[c].push(us);
            }
        }
    }
    println!("per sequence, us (min / p10 / median), {iterations} runs of {reps}:");
    for ((name, ..), t) in cases.iter().zip(&mut times) {
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |q: f64| t[((t.len() - 1) as f64 * q) as usize];
        println!(
            "  {name:<26} {:7.2} {:7.2} {:7.2}",
            at(0.0),
            at(0.1),
            at(0.5)
        );
    }
}
