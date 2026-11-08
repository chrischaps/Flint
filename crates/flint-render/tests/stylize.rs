//! Stylize passes (#262) on a real device: a display style inverts the
//! finished frame, switching it off gives back the exact bytes, the chain
//! survives a resize, and an HDR style and a display style run in one
//! frame without a validation error. The ASCII style (#265) draws one
//! glyph per cell on one grid, and its cells scale with pixels per point;
//! with its history on (#277) a wobbling camera leaves its glyphs still.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use flint_ecs::FlintWorld;
use flint_render::{Camera, RendererConfig, SceneRenderer};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

fn gpu() -> Option<Gpu> {
    pollster::block_on(async {
        let instance = flint_render::gpu_select::instance();
        let adapter = flint_render::gpu_select::request_adapter(&instance, None).await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .ok()?;
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
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
        let color = target(
            FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Some(Gpu {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            instance,
            device,
            queue,
        })
    })
}

impl Gpu {
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        renderer.update_from_world(world, &self.device);
        renderer.render_to(
            &self.device,
            &self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        self.device.poll(wgpu::Maintain::Wait);
        self.pixels()
    }

    /// The last frame's RGBA8 pixels, row by row.
    fn pixels(&self) -> Vec<u8> {
        let row = (SIZE * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * SIZE) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device.poll(wgpu::Maintain::Wait);
        let data = buffer.slice(..).get_mapped_range();
        (0..SIZE as usize)
            .flat_map(|y| {
                let start = y * row as usize;
                data[start..start + SIZE as usize * 4].to_vec()
            })
            .collect()
    }
}

#[test]
fn a_display_style_inverts_the_frame_and_lets_it_go() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let plain = gpu.frame(&mut renderer, &world);

    let set = |renderer: &mut SceneRenderer, f: &dyn Fn(&mut flint_render::PostProcessConfig)| {
        let mut config = renderer.post_process_config().clone();
        f(&mut config);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
    };

    // On: the negative of every pixel, alpha kept. The style inverts
    // display values on an sRGB target, so it's exact to a rounding step.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", true));
    let inverted = gpu.frame(&mut renderer, &world);
    for (i, (a, b)) in plain.iter().zip(&inverted).enumerate() {
        let want = if i % 4 == 3 { *a } else { 255 - *a };
        assert!(
            want.abs_diff(*b) <= 1,
            "byte {i}: plain {a}, inverted {b}, want {want}"
        );
    }

    // Off: the composite writes the target again, byte for byte.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", false));
    assert_eq!(gpu.frame(&mut renderer, &world), plain);

    // A resize rebuilds the chain's targets; the style still draws.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", true));
    renderer.resize_postprocess(&gpu.device, SIZE, SIZE);
    assert_eq!(gpu.frame(&mut renderer, &world), inverted);

    // A host that only sets the config (no ensure call) still gets the
    // style: the frame builds it.
    let mut fresh = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let mut config = fresh.post_process_config().clone();
    config.styles.set_enabled("invert", true);
    fresh.set_post_process_config(config);
    assert_eq!(gpu.frame(&mut fresh, &world), inverted);

    // Half strength meets in the middle.
    set(&mut renderer, &|c| {
        c.styles.set_param("invert", "strength", 0.5)
    });
    let half = gpu.frame(&mut renderer, &world);
    for (i, b) in half.iter().enumerate() {
        if i % 4 != 3 {
            assert!((127..=128).contains(b), "byte {i}: {b}");
        }
    }
}

#[test]
fn hdr_and_display_styles_chain_in_one_frame() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("kuwahara", true);
    renderer.set_post_process_config(config.clone());
    renderer.ensure_style_resources(&gpu.device);
    let painted = gpu.frame(&mut renderer, &world);

    config.styles.set_enabled("invert", true);
    config.fxaa_enabled = true;
    renderer.set_post_process_config(config);
    renderer.ensure_style_resources(&gpu.device);
    renderer.ensure_fxaa_resources(&gpu.device);
    // Two frames: the second reuses every bind group the first made
    gpu.frame(&mut renderer, &world);
    let both = gpu.frame(&mut renderer, &world);
    // An empty world is flat sky, which FXAA leaves be: still the negative
    for (i, (a, b)) in painted.iter().zip(&both).enumerate() {
        if i % 4 != 3 {
            assert!((255 - *a).abs_diff(*b) <= 2, "byte {i}: {a} then {b}");
        }
    }
}

/// #302: invert + FXAA holds two full-size surface-format textures (the
/// stage's input and FXAA's intermediate), and so does every longer chain
/// with FXAA, which draws through the intermediate instead of pings. The
/// frame is the one the chain drew before it reused them (sky inverted).
#[test]
fn display_styles_with_fxaa_hold_two_full_size_textures() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let world = FlintWorld::new();
    for styles in [
        &["invert"][..],
        &["ascii", "invert"],
        &["ascii", "pencil", "invert"],
    ] {
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let plain = gpu.frame(&mut renderer, &world);
        let mut config = renderer.post_process_config().clone();
        for style in styles {
            config.styles.set_enabled(style, true);
        }
        config.fxaa_enabled = true;
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
        renderer.ensure_fxaa_resources(&gpu.device);
        gpu.frame(&mut renderer, &world);
        let drawn = gpu.frame(&mut renderer, &world);
        let stats = renderer.collect_stats();
        assert_eq!(stats.full_size_targets, 2, "{styles:?}: {stats:?}");
        if styles.len() == 1 {
            for (i, (a, b)) in plain.iter().zip(&drawn).enumerate() {
                if i % 4 != 3 {
                    assert!((255 - *a).abs_diff(*b) <= 2, "byte {i}: {a} then {b}");
                }
            }
        }
    }
}

/// #299: two styles of one stage in the two orders draw two pictures, and
/// a scene that orders nothing draws what registration order always drew.
#[test]
fn an_authored_order_rearranges_a_stage() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    // A fresh renderer per frame, so ASCII's cell history starts clean
    let draw = |orders: Option<(f32, f32)>| {
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let mut config = renderer.post_process_config().clone();
        config.styles.set_enabled("ascii", true);
        config.styles.set_enabled("invert", true);
        if let Some((ascii, invert)) = orders {
            config.styles.set_order("ascii", Some(ascii));
            config.styles.set_order("invert", Some(invert));
        }
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
        gpu.frame(&mut renderer, &FlintWorld::new())
    };
    let default = draw(None);
    let ascii_first = draw(Some((0.0, 1.0)));
    let invert_first = draw(Some((1.0, 0.0)));
    // Registration order is ascii then invert, and stays the default
    assert_eq!(default, ascii_first);
    let differing = ascii_first
        .chunks(4)
        .zip(invert_first.chunks(4))
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    assert!(
        differing > (SIZE * SIZE / 4) as usize,
        "only {differing} pixels differ between the orders"
    );
}

/// The `period` x `period` block at cell (cx, cy) of an RGBA8 frame.
fn block(pixels: &[u8], period: usize, cx: usize, cy: usize) -> Vec<u8> {
    let size = SIZE as usize;
    (0..period)
        .flat_map(|y| {
            let start = ((cy * period + y) * size + cx * period) * 4;
            pixels[start..start + period * 4].to_vec()
        })
        .collect()
}

#[test]
fn ascii_draws_one_glyph_per_cell_on_one_grid() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let plain = gpu.frame(&mut renderer, &world);
    // An empty world is one flat sky colour, so every cell is the same glyph
    assert!(
        plain.chunks(4).all(|p| p == &plain[..4]),
        "flat sky expected"
    );

    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("ascii", true);
    renderer.set_post_process_config(config.clone());
    renderer.ensure_style_resources(&gpu.device);
    let glyphs = gpu.frame(&mut renderer, &world);
    assert_ne!(glyphs, plain, "the style drew nothing");

    // 8-point cells at 1 px per point: 64 / 8 = 8 cells a side, all alike,
    // and each holds a glyph (ink and ground both appear)
    let first = block(&glyphs, 8, 0, 0);
    let shades: std::collections::BTreeSet<&[u8]> = first.chunks(4).collect();
    assert!(shades.len() >= 2, "the cell is one flat colour: {shades:?}");
    for cy in 0..8 {
        for cx in 0..8 {
            assert_eq!(block(&glyphs, 8, cx, cy), first, "cell ({cx}, {cy})");
        }
    }

    // At 2 px per point (a high-DPI window) the cells are 16 px: 4 a side,
    // each the 8 px cell drawn at twice the size
    renderer.pixels_per_point = 2.0;
    let doubled = gpu.frame(&mut renderer, &world);
    let big = block(&doubled, 16, 0, 0);
    for cy in 0..4 {
        for cx in 0..4 {
            assert_eq!(block(&doubled, 16, cx, cy), big, "cell ({cx}, {cy}) at 2x");
        }
    }
    for y in 0..16 {
        for x in 0..16 {
            let at = |b: &[u8], w: usize, x: usize, y: usize| b[(y * w + x) * 4..][..4].to_vec();
            assert_eq!(
                at(&big, 16, x, y),
                at(&first, 8, x / 2, y / 2),
                "pixel ({x}, {y})"
            );
        }
    }

    // The ramp and colour options reach the shader: a different ramp
    // draws a different glyph for the same sky, the tint another colour
    config.styles.set_option("ascii", "ramp", "blocks");
    renderer.pixels_per_point = 1.0;
    renderer.set_post_process_config(config.clone());
    assert_ne!(block(&gpu.frame(&mut renderer, &world), 8, 0, 0), first);
    config.styles.set_option("ascii", "ramp", "classic");
    config.styles.set_option("ascii", "color", "tint");
    renderer.set_post_process_config(config);
    assert_ne!(block(&gpu.frame(&mut renderer, &world), 8, 0, 0), first);
}

/// The distinct RGB colours of an RGBA8 frame.
fn colours(pixels: &[u8]) -> std::collections::BTreeSet<[u8; 3]> {
    pixels.chunks(4).map(|p| [p[0], p[1], p[2]]).collect()
}

fn hex_bytes(palette: &flint_render::palette::Palette) -> Vec<[u8; 3]> {
    palette
        .colors()
        .iter()
        .chain(std::iter::once(&palette.background()))
        .map(|c| c.map(|v| (v * 255.0).round() as u8))
        .collect()
}

#[test]
fn ascii_palette_modes_draw_only_the_palettes_colours() {
    use flint_render::palette::Palette;
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("ascii", true);
    config.styles.set_option("ascii", "color", "palette");

    // At a whole-number scale coverage is 0 or 1, so every pixel is a
    // palette ink or its ground, whichever way round the ground puts them
    for (palette, ground) in [
        ("cga", "dim"),
        ("cga", "inverse"),
        ("cga16", "dim"),
        ("amber", "dim"),
        ("phosphor", "inverse"),
    ] {
        config.styles.set_option("ascii", "palette", palette);
        config.styles.set_option("ascii", "ground", ground);
        renderer.set_post_process_config(config.clone());
        renderer.ensure_style_resources(&gpu.device);
        let frame = gpu.frame(&mut renderer, &world);
        let allowed = hex_bytes(&Palette::builtin(palette).unwrap());
        let seen = colours(&frame);
        assert!(seen.len() >= 2, "{palette}/{ground}: one flat colour");
        for c in &seen {
            assert!(
                allowed
                    .iter()
                    .any(|a| (0..3).all(|i| a[i].abs_diff(c[i]) <= 1)),
                "{palette}/{ground}: {c:?} is not in the palette {allowed:?}"
            );
        }
    }

    // Mono draws greys only; black ground draws pure black behind glyphs
    config.styles.set_option("ascii", "color", "mono");
    config.styles.set_option("ascii", "ground", "black");
    renderer.set_post_process_config(config.clone());
    let mono = colours(&gpu.frame(&mut renderer, &world));
    assert!(mono.contains(&[0, 0, 0]), "{mono:?}");
    assert!(
        mono.iter().all(|c| c[0] == c[1] && c[1] == c[2]),
        "mono drew colour: {mono:?}"
    );

    // Glow adds light around the glyph, never takes any away
    config.styles.set_option("ascii", "color", "source");
    config.styles.set_option("ascii", "ground", "dim");
    renderer.set_post_process_config(config.clone());
    let plain = gpu.frame(&mut renderer, &world);
    config.styles.set_param("ascii", "glow", 1.0);
    renderer.set_post_process_config(config);
    let glowing = gpu.frame(&mut renderer, &world);
    assert_ne!(plain, glowing, "glow changed nothing");
    assert!(
        plain
            .iter()
            .zip(&glowing)
            .all(|(a, b)| b.saturating_add(1) >= *a),
        "glow darkened a pixel"
    );
}

/// The colour of grid pixel (x, y) in the palette agreement test: 16
/// steps of each channel over the 64x64 target.
fn grid_colour(x: u32, y: u32) -> [f32; 3] {
    let i = y * SIZE + x;
    [i % 16, (i / 16) % 16, i / 256].map(|c| c as f32 / 15.0)
}

const PALETTE_HARNESS: &str = r#"
struct Harness {
    inks: array<vec4<f32>, 32>,
    lab: array<vec4<f32>, 32>,
    // len, chroma, dither, unused
    look: vec4<f32>,
};
@group(0) @binding(0) var<uniform> h: Harness;
fn palette_ink(i: u32) -> vec3<f32> { return h.inks[i].rgb; }
fn palette_ink_lab(i: u32) -> vec3<f32> { return h.lab[i].rgb; }
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<u32>(floor(pos.xy));
    let i = p.y * 64u + p.x;
    let c = vec3<f32>(f32(i % 16u), f32((i / 16u) % 16u), f32(i / 256u)) / 15.0;
    let ink = pal_pick(0u, u32(h.look.x), c, h.look.y, h.look.z, vec2<i32>(p));
    // The target is sRGB: hand it linear so its bytes are the display ink
    let lo = ink / 12.92;
    let hi = pow((ink + 0.055) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(hi, lo, ink <= vec3<f32>(0.04045)), 1.0);
}
"#;

/// `Palette::pick` and `palette.wgsl`'s `pal_pick` give every colour of a
/// grid the same ink, with chroma pushed and dither on (#320). A colour
/// within float noise of a tie (two inks, or the dither threshold) may go
/// either way and is left out.
#[test]
fn the_palette_matcher_agrees_with_its_cpu_reference() {
    use flint_render::palette::{bayer4, oklab, Palette, PALETTE_WGSL};
    use wgpu::util::DeviceExt;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let device = &gpu.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("palette harness"),
        source: wgpu::ShaderSource::Wgsl(format!("{PALETTE_WGSL}\n{PALETTE_HARNESS}").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
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
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            }),
        ),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(FORMAT.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    let mut checked = 0;
    for (name, chroma, dither) in [
        ("cga", 1.0, 0.0),
        ("cga16", 1.0, 0.0),
        ("cga16", 2.0, 0.0),
        ("cga", 1.5, 1.0),
        ("cga16", 2.5, 0.6),
        ("cga-warm", 0.0, 0.0),
    ] {
        let palette = Palette::builtin(name).unwrap();
        let mut data: Vec<f32> = Vec::new();
        let pad = |v: &mut Vec<f32>, rows: Vec<[f32; 3]>| {
            for i in 0..32 {
                let c = rows.get(i).copied().unwrap_or_default();
                v.extend([c[0], c[1], c[2], 0.0]);
            }
        };
        pad(&mut data, palette.colors().to_vec());
        pad(&mut data, palette.lab_colors());
        data.extend([palette.colors().len() as f32, chroma, dither, 0.0]);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gpu.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
        let pixels = gpu.pixels();

        for y in 0..SIZE {
            for x in 0..SIZE {
                let colour = grid_colour(x, y);
                let ([_, _], [d1, d2]) = palette.nearest_two(colour, chroma);
                // A tie between the two nearest, or a share on the
                // dither's threshold, is float noise either side
                let share = d1.sqrt() / (d1.sqrt() + d2.sqrt()).max(1e-6);
                if (d2 - d1).abs() < 1e-5
                    || (dither > 0.0
                        && (share * dither - bayer4([x as i32, y as i32])).abs() < 1e-4)
                {
                    continue;
                }
                let want = palette
                    .pick(colour, chroma, dither, [x as i32, y as i32])
                    .map(|v| (v * 255.0).round() as u8);
                let at = ((y * SIZE + x) * 4) as usize;
                let got = &pixels[at..at + 3];
                assert!(
                    (0..3).all(|i| got[i].abs_diff(want[i]) <= 1),
                    "{name} chroma {chroma} dither {dither}: {colour:?} (lab {:?}) at ({x}, {y}) \
                     is {got:?} on the GPU, {want:?} on the CPU",
                    oklab(colour)
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 6 * 4000, "only {checked} colours compared");
}

/// Cells (8 px) whose pixels differ between two RGBA8 frames.
fn changed_cells(a: &[u8], b: &[u8]) -> usize {
    let n = SIZE as usize / 8;
    (0..n * n)
        .filter(|i| block(a, 8, i % n, i / n) != block(b, 8, i % n, i / n))
        .count()
}

#[test]
fn ascii_holds_its_glyphs_still_under_a_wobbling_camera() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    // A box: its edges cut cells, whose tones sit between the box's and
    // the sky's, on ramp boundaries
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("position".into(), vec3([0.0, 0.0, 0.0]));
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-1.0, -1.0, -1.0]));
    bounds.insert("max".into(), vec3([1.0, 1.0, 1.0]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();

    let run = |temporal: &str| -> Vec<Vec<u8>> {
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let mut config = renderer.post_process_config().clone();
        config.styles.set_enabled("ascii", true);
        config.styles.set_option("ascii", "temporal", temporal);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
        (0..24)
            .map(|i| {
                // A camera that wobbles back and forth by a fraction of a
                // cell, as a hand-held or idle-sway camera does
                let mut camera = Camera::default();
                let wobble = if i % 2 == 0 { 0.0 } else { WOBBLE };
                camera.target.x += wobble;
                camera.target.y += wobble;
                renderer.update_from_world(&world, &gpu.device);
                renderer.render_to(
                    &gpu.device,
                    &gpu.queue,
                    &gpu.depth_view,
                    &camera,
                    &gpu.color_view,
                );
                gpu.device.poll(wgpu::Maintain::Wait);
                gpu.pixels()
            })
            .collect()
    };
    let flips = |frames: &[Vec<u8>]| -> usize {
        frames[1..]
            .iter()
            .zip(frames)
            .map(|(b, a)| changed_cells(a, b))
            .sum()
    };

    let off = run("off");
    let on = run("on");
    // The first frame has no past: it is exactly the stateless style's
    assert_eq!(on[0], off[0], "the first frame differs with history on");
    let (off_flips, on_flips) = (flips(&off), flips(&on));
    eprintln!("cells changed over 23 frames: off {off_flips}, on {on_flips}");
    assert!(
        off_flips > 0,
        "the wobble moved no glyph; the bench tests nothing"
    );
    assert!(
        on_flips * 4 <= off_flips,
        "history should hold most glyphs still: off {off_flips}, on {on_flips}"
    );
    // The same frames in, the same frames out
    assert_eq!(run("on"), on, "a second run differs");
}

/// How far the wobbling camera's target moves, in metres.
const WOBBLE: f32 = 0.08;

/// Each 8 px cell's mean RGB, row by row.
fn cell_means(frame: &[u8]) -> Vec<[f32; 3]> {
    let n = SIZE as usize / 8;
    (0..n * n)
        .map(|i| {
            let (cx, cy) = (i % n, i / n);
            let mut sum = [0.0f32; 3];
            for y in 0..8 {
                for x in 0..8 {
                    let p = pixel(frame, cx * 8 + x, cy * 8 + y);
                    for c in 0..3 {
                        sum[c] += p[c] as f32 / 64.0;
                    }
                }
            }
            sum
        })
        .collect()
}

#[test]
fn reprojected_history_reads_the_past_of_the_surface_now_in_the_cell() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    // A wall of boxes two metres wide, red and green by turns, at equal
    // luminance: smoothing can't tell a red cell's past from a green one
    // by its jump, so a past read from the wrong surface shows as a hue
    let mut world = FlintWorld::new();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    for i in -12..12 {
        let id = world.spawn(format!("panel_{i}")).unwrap();
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3([i as f64 * 2.0 + 1.0, 0.0, 0.0]));
        world
            .set_component(id, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-1.0, -12.0, -0.5]));
        bounds.insert("max".into(), vec3([1.0, 12.0, 0.5]));
        world.set_component(id, "bounds", bounds.into()).unwrap();
        let color = if i % 2 == 0 {
            [0.8, 0.2, 0.2]
        } else {
            [0.2, 0.377, 0.2]
        };
        let mut material = toml::Table::new();
        material.insert(
            "color".into(),
            toml::Value::Array(
                color
                    .iter()
                    .map(|&c| c.into())
                    .chain([1.0.into()])
                    .collect(),
            ),
        );
        world
            .set_component(id, "material", material.into())
            .unwrap();
    }

    // Two frames: the camera trucks one panel to the side between them, so
    // every cell's surface last frame was in another cell, and the cell
    // itself last showed the other colour
    let run = |temporal: &str| -> Vec<Vec<u8>> {
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let mut config = renderer.post_process_config().clone();
        config.styles.set_enabled("ascii", true);
        config.styles.set_option("ascii", "temporal", temporal);
        config.styles.set_option("ascii", "color", "raw");
        config.styles.set_param("ascii", "background", 1.0);
        config.styles.set_param("ascii", "smoothing", 0.95);
        config.styles.set_param("ascii", "hysteresis", 0.0);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
        [0.0f32, 2.0]
            .iter()
            .map(|&x| {
                let camera = Camera {
                    position: flint_core::Vec3::new(x, 0.0, 12.0),
                    target: flint_core::Vec3::new(x, 0.0, 0.0),
                    ..Camera::default()
                };
                renderer.update_from_world(&world, &gpu.device);
                renderer.render_to(
                    &gpu.device,
                    &gpu.queue,
                    &gpu.depth_view,
                    &camera,
                    &gpu.color_view,
                );
                gpu.device.poll(wgpu::Maintain::Wait);
                gpu.pixels()
            })
            .collect()
    };
    // Cells whose colour is off the stateless frame's by more than 12 levels
    let wrong = |frames: &[Vec<u8>], truth: &[Vec<u8>]| -> usize {
        cell_means(&frames[1])
            .iter()
            .zip(cell_means(&truth[1]))
            .filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 12.0))
            .count()
    };
    let off = run("off");
    let on = run("on");
    let reproject = run("reproject");
    assert_eq!(reproject[0], off[0], "the first frame differs reprojected");
    assert_ne!(off[0], off[1], "the camera moved nothing");
    let (on_wrong, reproject_wrong) = (wrong(&on, &off), wrong(&reproject, &off));
    eprintln!(
        "cells off the present after a one-panel truck: on {on_wrong}, reproject {reproject_wrong}"
    );
    // Screen-space history smooths most cells towards the other colour
    assert!(
        on_wrong >= 24,
        "screen-space history should show the other panel: {on_wrong}"
    );
    // Reprojected, a cell's past is its own panel, but for the column that
    // came on screen and the cells a panel edge crosses
    assert!(
        reproject_wrong * 3 <= on_wrong,
        "reprojected history should read each surface's own past: on {on_wrong}, reproject {reproject_wrong}"
    );
}

/// The GPU objects the application holds: buffers, bind groups, textures
/// and texture views.
fn held(gpu: &Gpu) -> [usize; 4] {
    let Some(report) = gpu.instance.generate_report() else {
        return [0; 4];
    };
    let hub = report.hub_report();
    [
        hub.buffers.num_kept_from_user,
        hub.bind_groups.num_kept_from_user,
        hub.textures.num_kept_from_user,
        hub.texture_views.num_kept_from_user,
    ]
}

#[test]
fn a_script_set_style_animates_without_allocating_and_clears_exactly() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-1.0, -1.0, -1.0]));
    bounds.insert("max".into(), vec3([1.0, 1.0, 1.0]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();

    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);

    // What set_entity_style does: the cube, and only the cube, in glyphs
    flint_render::entity_style::set_entity_style(&mut world, cube, "ascii").unwrap();
    let styled = gpu.frame(&mut renderer, &world);
    assert_ne!(styled, plain, "the style drew nothing");
    assert_eq!(
        styled[..4],
        plain[..4],
        "the sky's corner should stay smooth"
    );

    // Warm up (the mask target and the first binds), then animate the
    // cell and glow every frame, as a script tweening them would
    let animate = |renderer: &mut SceneRenderer, world: &mut FlintWorld, i: usize| {
        let t = i as f64 / 24.0;
        flint_render::entity_style::set_style_param(world, cube, "cell", (6.0 + 6.0 * t).into())
            .unwrap();
        flint_render::entity_style::set_style_param(world, cube, "glow", t.into()).unwrap();
        gpu.frame(renderer, world)
    };
    animate(&mut renderer, &mut world, 0);
    let (binds, objects) = (renderer.kept_post_binds(), held(&gpu));
    let mut seen = std::collections::BTreeSet::new();
    for i in 1..=24 {
        seen.insert(animate(&mut renderer, &mut world, i));
    }
    eprintln!(
        "24 animated frames: {} distinct, post binds {binds} -> {}, held {objects:?} -> {:?}",
        seen.len(),
        renderer.kept_post_binds(),
        held(&gpu)
    );
    assert!(seen.len() > 4, "the animation changed nothing on screen");
    assert_eq!(
        renderer.kept_post_binds(),
        binds,
        "a param change kept a new bind group"
    );
    assert_eq!(
        held(&gpu),
        objects,
        "a param change left GPU objects behind"
    );

    // What clear_entity_style does: the plain frame, byte for byte
    assert!(flint_render::entity_style::clear_entity_style(
        &mut world, cube
    ));
    assert_eq!(gpu.frame(&mut renderer, &world), plain);
}

#[test]
fn a_blend_fades_and_dissolves_an_entity_into_ascii() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::{set_entity_style, set_style_param};
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-2.5, -2.5, -2.5]));
    bounds.insert("max".into(), vec3([2.5, 2.5, 2.5]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    set_entity_style(&mut world, cube, "ascii").unwrap();
    // Small cells, so a 64-pixel cube holds enough of them to dissolve
    set_style_param(&mut world, cube, "cell", 4.0.into()).unwrap();
    let full = gpu.frame(&mut renderer, &world);
    assert_ne!(full, plain);

    let at = |renderer: &mut SceneRenderer, world: &mut FlintWorld, b: f64| {
        set_style_param(world, cube, "blend", b.into()).unwrap();
        gpu.frame(renderer, world)
    };
    assert_eq!(
        at(&mut renderer, &mut world, 0.0),
        plain,
        "blend 0 is the entity's own shading"
    );
    assert_eq!(
        at(&mut renderer, &mut world, 1.0),
        full,
        "blend 1 is the full style"
    );

    // Fade: every pixel that differs lies between the two, none outside
    let half = at(&mut renderer, &mut world, 0.5);
    let styled_px = plain
        .chunks(4)
        .zip(full.chunks(4))
        .filter(|(p, f)| p != f)
        .count();
    let mut between = 0;
    for ((p, f), h) in plain.chunks(4).zip(full.chunks(4)).zip(half.chunks(4)) {
        if p == f {
            assert_eq!(h, p, "a pixel the style never touches changed");
            continue;
        }
        let inside = (0..3).all(|c| {
            let (lo, hi) = (p[c].min(f[c]), p[c].max(f[c]));
            h[c].saturating_add(1) >= lo && h[c] <= hi.saturating_add(1)
        });
        assert!(inside, "a faded pixel {h:?} left the span {p:?}..{f:?}");
        between += (h != p && h != f) as usize;
    }
    eprintln!("fade 0.5: {between} of {styled_px} styled pixels between the two");
    assert!(between * 2 > styled_px, "the fade mixed too few pixels");

    // Dissolve: each pixel is wholly one or the other, and more cells
    // turn as the blend rises
    set_style_param(&mut world, cube, "transition", "dissolve".into()).unwrap();
    let mut turned = Vec::new();
    for b in [0.25, 0.5, 0.75] {
        let frame = at(&mut renderer, &mut world, b);
        let mut glyph = 0;
        for ((p, f), d) in plain.chunks(4).zip(full.chunks(4)).zip(frame.chunks(4)) {
            assert!(d == p || d == f, "dissolve drew a mixed pixel");
            glyph += (p != f && d == f) as usize;
        }
        turned.push(glyph);
    }
    eprintln!("dissolve 0.25/0.5/0.75 of {styled_px}: {turned:?}");
    assert!(turned[0] < turned[1] && turned[1] < turned[2], "{turned:?}");
    assert!(turned[0] > 0 && turned[2] < styled_px, "{turned:?}");

    // A ramp allocates nothing: the blend is a mask channel, not a variant
    let (binds, objects) = (renderer.kept_post_binds(), held(&gpu));
    let ids = renderer.style_ids().clone();
    for i in 0..=12 {
        at(&mut renderer, &mut world, i as f64 / 12.0);
        assert_eq!(renderer.style_ids(), &ids, "a blend step made a variant");
    }
    assert_eq!(renderer.kept_post_binds(), binds);
    assert_eq!(held(&gpu), objects, "a blend ramp left GPU objects behind");
    // ASCII blends itself (#396): the framework's cross-fade never ran
    assert_eq!(renderer.style_fade_runs(), 0, "ASCII was cross-faded twice");
}

/// #396: a per-entity style with no blend code of its own (pencil) fades
/// with `blend`, the framework cross-fading its output back toward its
/// input: blend 0 is the entity's own shading, 1 the full style, and 0.5
/// lands every styled pixel halfway between the two. Fully styled, the
/// cross-fade isn't drawn at all, and a ramp allocates nothing.
#[test]
fn a_style_with_no_blend_code_fades_by_the_framework() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::{set_entity_style, set_style_param};
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-2.5, -2.5, -2.5]));
    bounds.insert("max".into(), vec3([2.5, 2.5, 2.5]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    set_entity_style(&mut world, cube, "pencil").unwrap();
    let full = gpu.frame(&mut renderer, &world);
    assert_ne!(full, plain, "pencil drew nothing");
    assert_eq!(
        renderer.style_fade_runs(),
        0,
        "a full blend was cross-faded"
    );

    let at = |renderer: &mut SceneRenderer, world: &mut FlintWorld, b: f64| {
        set_style_param(world, cube, "blend", b.into()).unwrap();
        gpu.frame(renderer, world)
    };
    let close = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1);
    let none = at(&mut renderer, &mut world, 0.0);
    assert!(close(&none, &plain), "blend 0 is the entity's own shading");
    assert!(renderer.style_fade_runs() > 0, "the cross-fade never ran");
    assert_eq!(
        at(&mut renderer, &mut world, 1.0),
        full,
        "blend 1 is the full style, byte for byte"
    );

    // Every pixel pencil touches lands near the midpoint in linear light
    // (an sRGB target blends there); none it doesn't touch moves
    let to_linear = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let half = at(&mut renderer, &mut world, 0.5);
    let (mut styled_px, mut worst) = (0, 0.0f32);
    for ((p, f), h) in plain.chunks(4).zip(full.chunks(4)).zip(half.chunks(4)) {
        if p == f {
            assert!(close(h, p), "a pixel the style never touches changed");
            continue;
        }
        styled_px += 1;
        for c in 0..3 {
            let mid = 0.5 * (to_linear(p[c]) + to_linear(f[c]));
            worst = worst.max((to_linear(h[c]) - mid).abs());
        }
    }
    eprintln!("fade 0.5 over {styled_px} styled pixels: worst {worst:.4} from the midpoint");
    assert!(styled_px > 100, "pencil styled only {styled_px} pixels");
    assert!(worst < 0.02, "a pixel strayed {worst} from the midpoint");

    // A ramp allocates nothing: the table and binds are made once
    at(&mut renderer, &mut world, 0.25);
    let (binds, objects) = (renderer.kept_post_binds(), held(&gpu));
    for i in 0..12 {
        at(&mut renderer, &mut world, i as f64 / 12.0);
    }
    assert_eq!(renderer.kept_post_binds(), binds);
    assert_eq!(held(&gpu), objects, "a blend ramp left GPU objects behind");
}

/// #335: two entities in one frame draw in two glyph sets. The right cube
/// names a set whose every glyph is solid ink in tall 4x8 cells: tinted
/// red, all of its pixels turn red, while the left cube, in the built-in
/// letters, keeps its ground between glyphs and doesn't change a pixel.
#[test]
fn two_entities_draw_in_two_glyph_sets_in_one_frame() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use flint_core::vfs::{self, MemorySource};
    use flint_render::entity_style::{set_entity_style, set_style_param};
    let mut src = MemorySource::new();
    let solid = "####\n".repeat(8);
    src.insert("fonts/solid.txt", format!("== #\n{solid}"));
    src.insert(
        "fonts/solid.glyphs.toml",
        "[font]\nfile = \"solid.txt\"\ncell = [4, 8]\n[ramps.ink]\nglyphs = \"#\"\n\
         [edges]\nhorizontal = \"#\"\nvertical = \"#\"\ndiagonal = \"#\"\nantidiagonal = \"#\"\n",
    );
    vfs::scoped(std::sync::Arc::new(src), || {
        let mut world = FlintWorld::new();
        boxed(&mut world, "left", 1.0, 0.0, None);
        boxed(&mut world, "right", 1.0, 0.0, None);
        let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
        for (name, x) in [("left", -2.2), ("right", 2.2)] {
            let id = world.get_id(name).unwrap();
            let mut transform = toml::Table::new();
            transform.insert("position".into(), vec3([x, 0.0, 0.0]));
            transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
            world
                .set_component(id, "transform", transform.into())
                .unwrap();
            set_entity_style(&mut world, id, "ascii").unwrap();
            set_style_param(&mut world, id, "cell", 4.0.into()).unwrap();
            set_style_param(&mut world, id, "color", "tint".into()).unwrap();
            set_style_param(&mut world, id, "tint", vec3([1.0, 0.0, 0.0])).unwrap();
            set_style_param(&mut world, id, "outline", 0.0.into()).unwrap();
            set_style_param(&mut world, id, "min_coverage", 0.0.into()).unwrap();
        }
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let letters = gpu.frame(&mut renderer, &world);
        let right = world.get_id("right").unwrap();
        set_style_param(&mut world, right, "font", "solid".into()).unwrap();
        let fonts = gpu.frame(&mut renderer, &world);
        assert_eq!(renderer.style_ids().len(), 2, "one id each");

        // Per half: pixels the style inked red, and red-tinted pixels that
        // aren't solid red (ground between glyphs, or a glyph's edge)
        let side = |frame: &[u8], right: bool| {
            let (mut red, mut ground) = (0, 0);
            for (i, p) in frame.chunks(4).enumerate() {
                if ((i % SIZE as usize) >= SIZE as usize / 2) != right {
                    continue;
                }
                let tinted = p[1] < 8 && p[2] < 8;
                red += (tinted && p[0] > 250) as usize;
                ground += (tinted && p[0] <= 250) as usize;
            }
            (red, ground)
        };
        let half = |frame: &[u8], right: bool| -> Vec<u8> {
            frame
                .chunks(SIZE as usize * 4)
                .flat_map(|row| {
                    let mid = row.len() / 2;
                    if right {
                        row[mid..].to_vec()
                    } else {
                        row[..mid].to_vec()
                    }
                })
                .collect()
        };
        let (letters_red, letters_ground) = side(&letters, true);
        let (solid_red, solid_ground) = side(&fonts, true);
        eprintln!(
            "right cube: letters {letters_red} ink / {letters_ground} ground, \
             solid {solid_red} ink / {solid_ground} ground"
        );
        assert!(
            letters_ground > letters_red / 4,
            "letters leave ground showing"
        );
        assert!(
            solid_red > letters_red + letters_ground / 2,
            "the solid set inks the cube"
        );
        assert!(
            solid_ground * 20 < solid_red,
            "almost nothing of it is ground"
        );
        assert_eq!(
            half(&fonts, false),
            half(&letters, false),
            "the left cube's letters don't change"
        );
    });
}

/// How far `scene` is from whole `cell`-pixel cells: over every grid
/// offset (0..cell on each axis), the least sum, block by block, of the
/// pixels on the block's minority side (0 = every block all scene or all
/// glyph).
fn part_cell_pixels(scene: &[bool], cell: usize) -> usize {
    let n = SIZE as usize;
    let mut best = usize::MAX;
    for oy in 0..cell {
        for ox in 0..cell {
            let mut blocks = std::collections::HashMap::<(usize, usize), (usize, usize)>::new();
            for (i, s) in scene.iter().enumerate() {
                let b = ((i % n + cell - ox) / cell, (i / n + cell - oy) / cell);
                let e = blocks.entry(b).or_default();
                if *s {
                    e.0 += 1;
                } else {
                    e.1 += 1;
                }
            }
            best = best.min(blocks.values().map(|(a, b)| *a.min(b)).sum());
        }
    }
    best
}

/// #338: `silhouette = "cell"` makes a per-entity silhouette of whole
/// glyphs: every 8x8 cell is all scene or all glyph, where the pixel
/// silhouette cuts cells, and a dropped cell shows the scene, not the
/// cube's own shading. The mask misses a few of the cube's grazing
/// silhouette pixels (its depth tolerance; they pass through as they are in
/// both modes), and a dropped pixel whose nearest scene sample is one of
/// them takes its colour, so the cell frame gets a budget of twice those.
#[test]
fn a_cell_silhouette_draws_every_edge_cell_whole_or_not_at_all() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::{set_entity_style, set_style_param};
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let empty = gpu.frame(&mut renderer, &FlintWorld::new());
    let mut world = box_world();
    let plain = gpu.frame(&mut renderer, &world);
    let cube = world.get_id("cube").unwrap();
    set_entity_style(&mut world, cube, "ascii").unwrap();
    set_style_param(&mut world, cube, "cell", 8.0.into()).unwrap();
    let pixel = gpu.frame(&mut renderer, &world);
    set_style_param(&mut world, cube, "silhouette", "cell".into()).unwrap();
    let cell = gpu.frame(&mut renderer, &world);

    // A pixel shows the scene where it is (near enough) the empty frame's
    let scene = |frame: &[u8]| -> Vec<bool> {
        frame
            .chunks(4)
            .zip(empty.chunks(4))
            .map(|(p, e)| (0..3).all(|c| p[c].abs_diff(e[c]) <= 2))
            .collect()
    };
    let cube_px: Vec<bool> = scene(&plain).iter().map(|s| !s).collect();
    let covered = cube_px.iter().filter(|c| **c).count();
    // The cube's pixels a frame leaves exactly as they were: its own shading
    let unchanged = |frame: &[u8]| -> usize {
        (0..cube_px.len())
            .filter(|&i| cube_px[i] && plain[i * 4..i * 4 + 4] == frame[i * 4..i * 4 + 4])
            .count()
    };
    let missed = unchanged(&pixel);
    let pixel_part = part_cell_pixels(&scene(&pixel), 8);
    let cell_part = part_cell_pixels(&scene(&cell), 8);
    eprintln!(
        "cube {covered} px, {missed} unmasked; part-cell px: pixel silhouette {pixel_part}, cell {cell_part}"
    );
    assert!(covered > 400, "the cube fills the middle of the frame");
    assert!(missed < 20, "the mask covers the cube");
    assert!(
        pixel_part > 4 * missed,
        "the pixel silhouette cuts cells (the test's control)"
    );
    assert!(
        cell_part <= 2 * missed,
        "a cell silhouette leaves part cells"
    );
    assert_eq!(
        unchanged(&cell),
        missed,
        "a dropped cell left the cube's own shading"
    );
}

/// A tilted box filling the middle of the frame: silhouettes against the
/// sky and creases where its faces meet.
fn box_world() -> FlintWorld {
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("position".into(), vec3([0.0, 0.0, 0.0]));
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-3.0, -3.0, -3.0]));
    bounds.insert("max".into(), vec3([3.0, 3.0, 3.0]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();
    world
}

/// The pixels whose red, green and blue differ: `edges_view` paints edges
/// in hues and everything else in grey.
fn coloured(pixels: &[u8]) -> Vec<bool> {
    pixels
        .chunks(4)
        .map(|p| p[0].abs_diff(p[1]) > 8 || p[1].abs_diff(p[2]) > 8 || p[0].abs_diff(p[2]) > 8)
        .collect()
}

/// #301: the shared geometry edges. `edges_view` draws them; a user style
/// asking for them (`inputs = ["edges"]`) in the same frame reads the very
/// same texture; and however many styles ask, a frame draws the edge pass
/// once, and a frame where none asks draws it not at all.
#[test]
fn two_styles_share_one_edge_pass_a_frame() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    // A user style that paints white where the shared edges say edge
    let dir = std::env::temp_dir().join(format!("flint-edges-301-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_edge_ink.shader.toml"),
        "version = 1\nname = \"utest_edge_ink\"\nshader = \"utest_edge_ink.wgsl\"\ninputs = [\"edges\"]\n",
    )
    .unwrap();
    std::fs::write(
        styles.join("utest_edge_ink.wgsl"),
        "fn style(p: StylePixel) -> vec4<f32> {\n    \
         return vec4<f32>(vec3<f32>(select(0.0, 1.0, flint_edge(p.uv) >= 1.0)), 1.0);\n}\n",
    )
    .unwrap();
    let messages = flint_render::stylize::custom::load_dir(&dir);
    assert!(messages.is_empty(), "{messages:?}");
    let ink = flint_render::stylize::find("utest_edge_ink").expect("the user style registered");
    assert!(ink.needs_edges());

    let world = box_world();
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let set = |renderer: &mut SceneRenderer, f: &dyn Fn(&mut flint_render::PostProcessConfig)| {
        let mut config = renderer.post_process_config().clone();
        f(&mut config);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
    };

    // No style asks: no pass
    gpu.frame(&mut renderer, &world);
    assert_eq!(renderer.style_edge_runs(), 0, "drawn with no style asking");
    set(&mut renderer, &|c| c.styles.set_enabled("invert", true));
    gpu.frame(&mut renderer, &world);
    assert_eq!(renderer.style_edge_runs(), 0, "invert doesn't need edges");

    // edges_view alone: one pass a frame, and the box's outline in hues
    set(&mut renderer, &|c| {
        c.styles.set_enabled("invert", false);
        c.styles.set_enabled("edges_view", true);
        c.styles.set_param("edges_view", "ghost", 0.0);
    });
    let viewed = gpu.frame(&mut renderer, &world);
    assert_eq!(renderer.style_edge_runs(), 1);
    let edges = coloured(&viewed);
    let count = edges.iter().filter(|&&e| e).count();
    eprintln!("edge pixels: {count} of {}", SIZE * SIZE);
    assert!(count > 30, "only {count} edge pixels around a box");
    assert!(
        count < (SIZE * SIZE / 3) as usize,
        "{count} edge pixels: the box's faces are lit up, not outlined"
    );
    // The two pixels at each border are never edges (degenerate normals)
    let size = SIZE as usize;
    for (i, &e) in edges.iter().enumerate() {
        let (x, y) = (i % size, i / size);
        if x < 2 || y < 2 || x >= size - 2 || y >= size - 2 {
            assert!(!e, "border pixel {x},{y} is an edge");
        }
    }
    // Several directions: a box's outline isn't one hue
    let hues: std::collections::BTreeSet<[u8; 3]> = viewed
        .chunks(4)
        .zip(&edges)
        .filter(|(_, &e)| e)
        .map(|(p, _)| [p[0] / 64, p[1] / 64, p[2] / 64])
        .collect();
    assert!(hues.len() >= 2, "every edge has one direction: {hues:?}");

    // Both styles on: still one pass a frame, over several frames
    set(&mut renderer, &|c| {
        c.styles.set_enabled("utest_edge_ink", true)
    });
    let before = renderer.style_edge_runs();
    let mut both = Vec::new();
    for _ in 0..3 {
        both = gpu.frame(&mut renderer, &world);
    }
    assert_eq!(
        renderer.style_edge_runs() - before,
        3,
        "two styles reading the edges drew the pass more than once a frame"
    );
    // The user style ran after edges_view and saw the same texture: its
    // white is exactly edges_view's hues
    let inked: Vec<bool> = both.chunks(4).map(|p| p[0] > 127).collect();
    let disagree = inked.iter().zip(&edges).filter(|(a, b)| a != b).count();
    assert_eq!(disagree, 0, "the two styles read different edges");

    // Both off: the pass stops
    set(&mut renderer, &|c| {
        c.styles.set_enabled("utest_edge_ink", false);
        c.styles.set_enabled("edges_view", false);
    });
    let after = renderer.style_edge_runs();
    gpu.frame(&mut renderer, &world);
    assert_eq!(
        renderer.style_edge_runs(),
        after,
        "drawn after every style let go"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #414: a user style that reads the mask is per-entity. Its generated
/// `<name>_style` component (or a `style_mask` naming it) switches it on
/// with no `[post_process.styles]` table, and each entity's settings reach
/// its own pixels as `params`: three boxes, three `glow`s, one frame.
#[test]
fn a_user_style_takes_settings_per_entity() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    // Paints its entities (r = glow, g = 1, b = 0) and passes the rest
    let dir = std::env::temp_dir().join(format!("flint-glow-414-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_glow.shader.toml"),
        "version = 1\nname = \"utest_glow\"\nshader = \"utest_glow.wgsl\"\ninputs = [\"mask\"]\n\
         [params.glow]\ndefault = 0.5\nmin = 0.0\nmax = 1.0\n",
    )
    .unwrap();
    std::fs::write(
        styles.join("utest_glow.wgsl"),
        "fn style(p: StylePixel) -> vec4<f32> {\n    \
         if (flint_style_blend(p.uv) <= 0.0) {\n        return p.color;\n    }\n    \
         return vec4<f32>(params.glow, 1.0, 0.0, 1.0);\n}\n",
    )
    .unwrap();
    let messages = flint_render::stylize::custom::load_dir(&dir);
    assert!(messages.is_empty(), "{messages:?}");
    let glow = flint_render::stylize::find("utest_glow").expect("the user style registered");
    assert_eq!(glow.component, Some("utest_glow_style"));

    // Three boxes in a row across the default camera's view
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let mut world = FlintWorld::new();
    let mut boxes = Vec::new();
    for (i, a) in [-3.2, 0.0, 3.2].into_iter().enumerate() {
        let e = world.spawn(format!("box{i}")).unwrap();
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3([a * s, -a * s, 0.0]));
        world
            .set_component(e, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-1.0; 3]));
        bounds.insert("max".into(), vec3([1.0; 3]));
        world.set_component(e, "bounds", bounds.into()).unwrap();
        boxes.push(e);
    }
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    let painted = |f: &[u8]| f.chunks(4).filter(|p| p[1] == 255 && p[2] == 0).count();
    assert_eq!(
        painted(&plain),
        0,
        "the test's colour is in the plain frame"
    );

    let glow_of = |g: f64| {
        let mut t = toml::Table::new();
        t.insert("glow".into(), g.into());
        toml::Value::Table(t)
    };
    world
        .set_component(boxes[0], "utest_glow_style", glow_of(0.25))
        .unwrap();
    world
        .set_component(boxes[1], "utest_glow_style", glow_of(0.75))
        .unwrap();
    let mut mask = toml::Table::new();
    mask.insert("style".into(), "utest_glow".into());
    world
        .set_component(boxes[2], "style_mask", mask.into())
        .unwrap();
    assert!(
        !renderer
            .post_process_config()
            .styles
            .is_enabled("utest_glow"),
        "the scene has no table for it"
    );
    renderer.update_from_world(&world, &gpu.device);
    renderer.ensure_style_resources(&gpu.device);
    let styled = gpu.frame(&mut renderer, &world);

    // Each box in its own glow (display values; 6 levels for the sRGB
    // round trip), the third at the default
    let quarter = near(&styled, [0.25, 1.0, 0.0]);
    let three_quarters = near(&styled, [0.75, 1.0, 0.0]);
    let half = near(&styled, [0.5, 1.0, 0.0]);
    eprintln!(
        "glow 0.25: {quarter} px, 0.75: {three_quarters} px, default 0.5: {half} px, all {}",
        painted(&styled)
    );
    for (n, what) in [
        (quarter, "glow = 0.25"),
        (three_quarters, "glow = 0.75"),
        (half, "the style_mask's default"),
    ] {
        assert!(n > 40, "only {n} pixels in {what}");
    }
    assert_eq!(
        quarter + three_quarters + half,
        painted(&styled),
        "a painted pixel took none of the three settings"
    );
    // The rest of the frame is the plain one
    let changed_off_boxes = plain
        .chunks(4)
        .zip(styled.chunks(4))
        .filter(|(a, b)| a != b && !(b[1] == 255 && b[2] == 0))
        .count();
    assert_eq!(changed_off_boxes, 0, "pixels off the boxes changed");

    // An opt-out leaves its box as it was
    let mut off = toml::Table::new();
    off.insert("enabled".into(), false.into());
    world
        .set_component(boxes[1], "utest_glow_style", off.into())
        .unwrap();
    let opted = gpu.frame(&mut renderer, &world);
    assert_eq!(near(&opted, [0.75, 1.0, 0.0]), 0, "the opt-out was painted");
    assert!(near(&opted, [0.25, 1.0, 0.0]) > 40);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pixels within 6 of `rgb` (8-bit display values) in each channel.
fn near(pixels: &[u8], rgb: [f32; 3]) -> usize {
    let want = rgb.map(|v| (v * 255.0).round() as i32);
    pixels
        .chunks(4)
        .filter(|p| (0..3).all(|c| (i32::from(p[c]) - want[c]).abs() <= 6))
        .count()
}

/// #349: `DebugMode::StyleEdges` draws the ASCII style's edge view, ASCII
/// switched on for it in a scene that has none: the box's silhouette in the
/// depth step's colour, its cells' line glyphs in green, and no glyphs.
/// Leaving the mode gives the plain frame back exactly.
#[test]
fn the_edges_debug_view_shows_sources_and_cells_and_lets_go() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let world = box_world();
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    assert!(!renderer
        .effective_post_process_config()
        .styles
        .is_enabled("ascii"));

    renderer.set_debug_mode(flint_render::DebugMode::StyleEdges);
    let viewed = gpu.frame(&mut renderer, &world);
    assert!(
        renderer
            .effective_post_process_config()
            .styles
            .is_enabled("ascii"),
        "the view switched ASCII on"
    );
    let legend = flint_render::stylize::EDGE_VIEW_LEGEND;
    let colour = |name: &str| legend.iter().find(|(_, n)| n.starts_with(name)).unwrap().0;
    let depth = near(&viewed, colour("depth"));
    let crease = near(&viewed, colour("crease"));
    let line = near(&viewed, colour("cell draws"));
    eprintln!("depth {depth}, crease {crease}, line glyph {line} pixels");
    assert!(depth > 20, "only {depth} depth-step pixels around the box");
    assert!(crease > 5, "only {crease} crease pixels on a turned box");
    assert!(line > 20, "only {line} pixels of line glyphs");

    renderer.set_debug_mode(flint_render::DebugMode::Pbr);
    let after = gpu.frame(&mut renderer, &world);
    assert!(
        !renderer
            .effective_post_process_config()
            .styles
            .is_enabled("ascii"),
        "ASCII stayed on after the view"
    );
    assert_eq!(after, plain, "the frame changed after the view let go");
}

/// A transparent pane in front of a styled entity takes back the share of
/// each pixel it covers (#290): nearly opaque glass gives the unstyled
/// pixels back exactly, half-clear glass leaves the style showing through
/// faintly, and glass that is itself styled stays out of the veil.
#[test]
fn glass_in_front_of_a_styled_entity_unmasks_it_by_its_coverage() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::{clear_entity_style, set_entity_style, set_style_param};
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let boxed = |world: &mut FlintWorld, name: &str, at: [f64; 3], half: f64| {
        let e = world.spawn(name).unwrap();
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3(at));
        world
            .set_component(e, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-half; 3]));
        bounds.insert("max".into(), vec3([half; 3]));
        world.set_component(e, "bounds", bounds.into()).unwrap();
        e
    };
    let mut world = FlintWorld::new();
    let cube = boxed(&mut world, "cube", [0.0; 3], 2.5);
    // A box of glass between the default camera (10, 10, 10) and the
    // cube, wide enough to cover all of it
    let glass = boxed(&mut world, "glass", [5.0; 3], 3.0);
    let set_opacity = |world: &mut FlintWorld, opacity: f64| {
        let mut material = toml::Table::new();
        material.insert("color".into(), vec3([0.4, 0.8, 0.7]));
        material.insert("opacity".into(), opacity.into());
        world
            .set_component(glass, "material", material.into())
            .unwrap();
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let styled = |renderer: &mut SceneRenderer, world: &mut FlintWorld, on: bool| {
        if on {
            set_entity_style(world, cube, "ascii").unwrap();
            set_style_param(world, cube, "cell", 4.0.into()).unwrap();
        } else {
            clear_entity_style(world, cube);
        }
        gpu.frame(renderer, world)
    };
    let weight =
        |a: &[u8], b: &[u8]| -> u32 { a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u32).sum() };

    // Nearly opaque glass: its two faces cover all but a millionth, so the
    // cube's blend rounds to 0 and the frame is the unstyled one
    set_opacity(&mut world, 0.999);
    let plain = styled(&mut renderer, &mut world, false);
    let veiled = styled(&mut renderer, &mut world, true);
    assert_eq!(
        veiled, plain,
        "glass in front still drew the cube in glyphs"
    );

    // Half-clear glass covers 75% (two faces): the glyphs show, faintly
    set_opacity(&mut world, 0.5);
    let plain = styled(&mut renderer, &mut world, false);
    let through = styled(&mut renderer, &mut world, true);
    let weak = weight(&plain, &through);
    assert!(weak > 0, "half-clear glass hid the style entirely");

    // Styled glass is no veil: behind it the cube is wholly glyphs. Since
    // #310 a styled transparent is a lens, so they are the glass's glyphs,
    // measured against the frame with nothing styled (the `plain` above)
    set_entity_style(&mut world, glass, "ascii").unwrap();
    let full = styled(&mut renderer, &mut world, true);
    let strong = weight(&plain, &full);
    eprintln!("half-clear glass: style weight {weak}; styled glass: {strong}");
    assert!(
        strong > weak * 2,
        "the veil didn't weaken the style ({weak} vs {strong})"
    );
}

/// A flat terrain chunk `half` metres each way around the origin.
fn flat_chunk(half: f32) -> flint_terrain::TerrainChunk {
    let corners = [[-half, -half], [half, -half], [half, half], [-half, half]];
    flint_terrain::TerrainChunk {
        grid_pos: (0, 0),
        positions: corners.iter().map(|[x, z]| [*x, 0.0, *z]).collect(),
        normals: vec![[0.0, 1.0, 0.0]; 4],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        indices: vec![0, 2, 1, 0, 3, 2],
        aabb_min: [-half, 0.0, -half],
        aabb_max: [half, 0.0, half],
    }
}

/// A procedural box `half` metres each way, centred `y` up, made
/// transparent by `opacity`.
fn boxed(world: &mut FlintWorld, name: &str, half: f64, y: f64, opacity: Option<f64>) {
    let id = world.spawn(name).unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("position".into(), vec3([0.0, y, 0.0]));
    world
        .set_component(id, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-half, -half, -half]));
    bounds.insert("max".into(), vec3([half, half, half]));
    world.set_component(id, "bounds", bounds.into()).unwrap();
    if let Some(opacity) = opacity {
        let mut material = toml::Table::new();
        material.insert(
            "color".into(),
            toml::Value::Array(vec![0.6.into(), 0.9.into(), 1.0.into(), 1.0.into()]),
        );
        material.insert("opacity".into(), opacity.into());
        world
            .set_component(id, "material", material.into())
            .unwrap();
    }
}

/// #385: a per-entity style draws only inside its entities' screen bounds.
/// Two small props, one in pencil and one in ASCII, over a floor nobody
/// styles: each id's bounds are a small part of the frame, they hold every
/// pixel the style changed, and the scissored frame is the full one byte
/// for byte, frame after frame.
#[test]
fn per_entity_styles_scissored_to_their_props_draw_the_same_bytes() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::set_entity_style;
    use flint_render::style_mask::IdBounds;
    let mut world = FlintWorld::new();
    boxed(&mut world, "floor", 30.0, -30.0, None);
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    for (name, at) in [("sketch", [2.5, 0.0, -2.5]), ("glyphs", [-2.5, 0.0, 2.5])] {
        let id = world.spawn(name).unwrap();
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3(at));
        transform.insert("rotation".into(), vec3([10.0, 30.0, 0.0]));
        world
            .set_component(id, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-0.6; 3]));
        bounds.insert("max".into(), vec3([0.6; 3]));
        world.set_component(id, "bounds", bounds.into()).unwrap();
    }
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    let (sketch, glyphs) = (
        world.get_id("sketch").unwrap(),
        world.get_id("glyphs").unwrap(),
    );
    set_entity_style(&mut world, sketch, "pencil").unwrap();
    set_entity_style(&mut world, glyphs, "ascii").unwrap();
    renderer.set_style_scissor(true);
    let scissored = gpu.frame(&mut renderer, &world);

    // Each id is bounded, and small
    let bounds = renderer.style_screen_bounds(&Camera::default());
    let ids = renderer.style_ids().clone();
    assert_eq!(ids.len(), 2, "{ids:?}");
    let mut rects = Vec::new();
    for (id, name) in ids.iter() {
        let IdBounds::Uv(r) = bounds.get(id) else {
            panic!("{name}: {:?}", bounds.get(id));
        };
        eprintln!("{name}: {r:?}");
        assert!(
            r[2] - r[0] < 0.25 && r[3] - r[1] < 0.25,
            "{name} fills {r:?}"
        );
        rects.push(r);
    }
    // Every pixel the styles changed is inside an id's bounds, padded by
    // a few pixels of ASCII's reach (two 8 px cells)
    let size = SIZE as f32;
    let inside = |x: usize, y: usize| {
        rects.iter().any(|r| {
            let pad = 20.0;
            (x as f32) >= r[0] * size - pad
                && (x as f32) < r[2] * size + pad
                && (y as f32) >= r[1] * size - pad
                && (y as f32) < r[3] * size + pad
        })
    };
    let mut changed = 0;
    for (i, (a, b)) in plain.chunks(4).zip(scissored.chunks(4)).enumerate() {
        if a != b {
            changed += 1;
            let (x, y) = (i % SIZE as usize, i / SIZE as usize);
            assert!(inside(x, y), "pixel {x},{y} changed outside the props");
        }
    }
    assert!(changed > 20, "the styles changed only {changed} pixels");

    // The full passes draw the same frame
    renderer.set_style_scissor(false);
    for _ in 0..2 {
        let full = gpu.frame(&mut renderer, &world);
        assert!(
            full == scissored,
            "the scissored frame differs from the full one"
        );
    }
    renderer.set_style_scissor(true);
    assert!(gpu.frame(&mut renderer, &world) == scissored);
}

fn pixel(frame: &[u8], x: usize, y: usize) -> &[u8] {
    &frame[(y * SIZE as usize + x) * 4..][..4]
}

#[test]
fn a_styled_terrain_is_masked_around_the_props_on_it() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use flint_render::entity_style::set_entity_style;
    let mut world = FlintWorld::new();
    let ground = world.spawn("ground").unwrap();
    boxed(&mut world, "cube", 2.5, 2.5, None);
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let no_textures: [String; 4] = Default::default();
    renderer.load_terrain(
        &gpu.device,
        &gpu.queue,
        &[flat_chunk(40.0)],
        &flint_core::Transform::default(),
        1.0,
        0.0,
        0.9,
        "",
        &no_textures,
        std::path::Path::new("."),
    );
    renderer.set_terrain_entity(Some(ground));
    let plain = gpu.frame(&mut renderer, &world);

    set_entity_style(&mut world, ground, "ascii").unwrap();
    let styled = gpu.frame(&mut renderer, &world);
    // The bottom rows are all ground: glyphs there
    let bottom = (SIZE as usize - 8) * SIZE as usize * 4;
    assert_ne!(
        styled[bottom..],
        plain[bottom..],
        "the terrain took no style"
    );
    // The unstyled cube in the middle cuts its own hole in the mask
    for (x, y) in [(31, 30), (32, 30), (31, 31), (32, 31)] {
        assert_eq!(
            pixel(&styled, x, y),
            pixel(&plain, x, y),
            "the unstyled cube at ({x},{y}) was styled"
        );
    }
    // A terrain the host didn't name stays out of the mask, as before
    renderer.set_terrain_entity(None);
    assert_eq!(gpu.frame(&mut renderer, &world), plain);
}

#[test]
fn a_styled_transparent_styles_what_is_seen_through_it() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use flint_render::entity_style::{clear_entity_style, set_entity_style};
    let mut world = FlintWorld::new();
    boxed(&mut world, "floor", 30.0, -30.0, None);
    boxed(&mut world, "glass", 2.5, 2.5, Some(0.4));
    let glass = world.get_id("glass").unwrap();
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = gpu.frame(&mut renderer, &world);
    set_entity_style(&mut world, glass, "ascii").unwrap();
    let styled = gpu.frame(&mut renderer, &world);
    let changed = plain
        .chunks(4)
        .zip(styled.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("the styled glass changed {changed} pixels");
    assert!(changed > 50, "a styled transparent took no style");
    // The glass is the lens, not the floor: the corners stay as they were
    for (x, y) in [(1, 62), (62, 62)] {
        assert_eq!(
            pixel(&styled, x, y),
            pixel(&plain, x, y),
            "the floor at ({x},{y}) was styled"
        );
    }
    clear_entity_style(&mut world, glass);
    assert_eq!(gpu.frame(&mut renderer, &world), plain);
}

#[test]
fn the_mask_buffer_fits_the_scene_and_masks_every_styled_draw() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let probe = |on: bool| {
        let mut t = toml::Table::new();
        t.insert("style".into(), "probe".into());
        t.insert("enabled".into(), on.into());
        toml::Value::Table(t)
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    renderer.set_debug_mode(flint_render::DebugMode::StyleMask);

    // One styled box: the buffer stays at its 4 KB start (#311; it was 1 MB)
    let mut one = FlintWorld::new();
    let e = one.spawn("box").unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-1.0; 3]));
    bounds.insert("max".into(), vec3([1.0; 3]));
    one.set_component(e, "bounds", bounds.into()).unwrap();
    one.set_component(e, "style_mask", probe(true)).unwrap();
    gpu.frame(&mut renderer, &one);
    let (bytes, draws) = renderer.style_mask_draws().expect("no mask pass");
    eprintln!("one styled box: {bytes} bytes for {draws} draws");
    assert_eq!(draws, 1);
    assert!(bytes <= 4096, "{bytes} bytes for one draw");

    // A wall of small boxes facing the default camera at (10, 10, 10),
    // more than the old fixed 4096 slots held, few hiding another
    let side = 70;
    let (s, t) = (std::f64::consts::FRAC_1_SQRT_2, 6f64.sqrt().recip());
    let (u, v) = ([s, -s, 0.0], [t, t, -2.0 * t]);
    let mut world = FlintWorld::new();
    let mut boxes = Vec::new();
    for i in 0..side * side {
        let e = world.spawn(format!("box{i}")).unwrap();
        let (a, b) = (
            (i % side) as f64 - side as f64 / 2.0,
            (i / side) as f64 - side as f64 / 2.0,
        );
        let at: [f64; 3] = std::array::from_fn(|k| (a * u[k] + b * v[k]) * 0.19);
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3(at));
        world
            .set_component(e, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-0.07; 3]));
        bounds.insert("max".into(), vec3([0.07; 3]));
        world.set_component(e, "bounds", bounds.into()).unwrap();
        boxes.push(e);
    }
    let set_style = |world: &mut FlintWorld, on: bool| {
        for &e in &boxes {
            world.set_component(e, "style_mask", probe(on)).unwrap();
        }
    };
    set_style(&mut world, false);
    let unstyled = gpu.frame(&mut renderer, &world);
    set_style(&mut world, true);
    let styled = gpu.frame(&mut renderer, &world);
    let (bytes, draws) = renderer.style_mask_draws().unwrap();
    eprintln!("{} boxes: {bytes} bytes for {draws} draws", side * side);
    assert!(draws > 4096, "only {draws} styled draws reached the mask");
    assert!(bytes >= draws as u64 * 256);

    // Every box drawn is masked: no pixel the boxes cover stays unstyled
    let empty = gpu.frame(&mut renderer, &FlintWorld::new());
    let px = |f: &[u8], i: usize| [f[i * 4], f[i * 4 + 1], f[i * 4 + 2]];
    let n = (SIZE * SIZE) as usize;
    let covered: Vec<usize> = (0..n)
        .filter(|&i| px(&unstyled, i) != px(&empty, i))
        .collect();
    let missed = covered
        .iter()
        .filter(|&&i| px(&styled, i) == px(&unstyled, i))
        .count();
    eprintln!("{} covered pixels, {missed} left unmasked", covered.len());
    assert!(covered.len() > n / 8, "the boxes cover too little to tell");
    assert_eq!(missed, 0, "{missed} box pixels were not masked");
}

/// #324: the palette style draws every pixel in its table's colours, the
/// colour `Palette::quantize` gives for what was there (flat, and with
/// chroma and dither), and `pixel` makes blocks of one colour. Its nearest
/// pick is `pal_pick`, the matcher ASCII draws with, which
/// `the_palette_matcher_agrees_with_its_cpu_reference` holds to
/// `Palette::pick`; the style's ladder is its own and is checked here.
#[test]
fn the_palette_style_quantizes_the_frame_to_its_table() {
    use flint_render::palette::Palette;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = box_world();
    let plain = gpu.frame(&mut renderer, &world);
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("palette", true);
    for (name, chroma, dither) in [
        ("cga", 1.0, 0.0),
        ("cga16", 1.0, 0.0),
        ("phosphor", 1.0, 0.0),
        ("paper", 1.0, 0.0),
        ("cga16", 2.0, 1.0),
        ("paper", 1.0, 1.0),
    ] {
        let palette = Palette::builtin(name).unwrap();
        config.styles.set_option("palette", "palette", name);
        config.styles.set_param("palette", "pixel", 1.0);
        config.styles.set_param("palette", "dither", dither);
        config.styles.set_param("palette", "chroma", chroma);
        renderer.set_post_process_config(config.clone());
        renderer.ensure_style_resources(&gpu.device);
        let styled = gpu.frame(&mut renderer, &world);
        let table: Vec<[u8; 3]> = palette
            .table()
            .iter()
            .map(|c| c.map(|v| (v * 255.0).round() as u8))
            .collect();
        let seen = colours(&styled);
        assert!(seen.len() >= 2, "{name}: one flat colour");
        let mut off = 0;
        let mut ties = 0;
        for (i, (p, s)) in plain.chunks(4).zip(styled.chunks(4)).enumerate() {
            let s = [s[0], s[1], s[2]];
            let cell = [(i % SIZE as usize) as i32, (i / SIZE as usize) as i32];
            assert!(
                table
                    .iter()
                    .any(|t| (0..3).all(|i| t[i].abs_diff(s[i]) <= 1)),
                "{name}: {s:?} is not in {table:?}"
            );
            // The CPU reads the plain frame's 8-bit bytes and the GPU its
            // input's floats: a colour within a rounding step of a tie
            // between two colours may go either way, and is left out (as
            // the matcher test leaves out float ties)
            let colour = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
            if palette.matching() == flint_render::palette::PaletteMatch::Nearest {
                let ([_, _], [d1, d2]) =
                    flint_render::palette::nearest_two_in(&palette.table(), colour, chroma);
                if d2 - d1 < 1e-4 {
                    ties += 1;
                    continue;
                }
            }
            let want = palette
                .quantize(colour, chroma, dither, cell)
                .map(|v| (v * 255.0).round() as u8);
            if (0..3).any(|i| want[i].abs_diff(s[i]) > 1) {
                off += 1;
            }
        }
        let n = (SIZE * SIZE) as usize;
        eprintln!(
            "{name} chroma {chroma} dither {dither}: {} colours, {off} of {n} pixels off the CPU's pick, {ties} near ties left out",
            seen.len()
        );
        assert!(
            ties * 20 <= n,
            "{name}: {ties} near ties is most of the frame"
        );
        assert!(
            off * 200 <= n,
            "{name} chroma {chroma} dither {dither}: {off} pixels disagree with Palette::quantize"
        );
    }

    // Four-point blocks: every 4x4 block is one colour
    config.styles.set_option("palette", "palette", "cga16");
    config.styles.set_param("palette", "pixel", 4.0);
    config.styles.set_param("palette", "dither", 1.0);
    renderer.set_post_process_config(config);
    let chunky = gpu.frame(&mut renderer, &world);
    let n = SIZE as usize / 4;
    for i in 0..n * n {
        let b = block(&chunky, 4, i % n, i / n);
        assert!(
            b.chunks(4).all(|p| p == &b[..4]),
            "block {i} is not one colour"
        );
    }
}

/// #336: a wall of 506 props, each in ASCII on a grid of its own
/// (`grid = "object"`, so every styled root is a variant of its own, with
/// mask ids far past 256). The variant table used to hold 63 variants among
/// ids below 256, and the rest drew like the whole frame, which here (only
/// entities asked) passes the picture through: most of the wall stayed
/// plain. Now every prop is a row of the table and every pixel a prop
/// covers is drawn in glyphs; a frame later the table's buffers hold still.
#[test]
fn five_hundred_anchored_props_each_draw_on_their_own_grid() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let anchored = || {
        let mut t = toml::Table::new();
        t.insert("grid".into(), "object".into());
        t.insert("cell".into(), 3.0.into());
        toml::Value::Table(t)
    };
    // Facing the default camera at (10, 10, 10), as the mask buffer test's
    let (cols, rows) = (23, 22);
    let (s, t) = (std::f64::consts::FRAC_1_SQRT_2, 6f64.sqrt().recip());
    let (u, v) = ([s, -s, 0.0], [t, t, -2.0 * t]);
    let mut world = FlintWorld::new();
    let mut props = Vec::new();
    for i in 0..cols * rows {
        let e = world.spawn(format!("prop{i:03}")).unwrap();
        let (a, b) = (
            (i % cols) as f64 - cols as f64 / 2.0,
            (i / cols) as f64 - rows as f64 / 2.0,
        );
        let at: [f64; 3] = std::array::from_fn(|k| (a * u[k] + b * v[k]) * 0.6);
        let mut transform = toml::Table::new();
        transform.insert("position".into(), vec3(at));
        transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
        world
            .set_component(e, "transform", transform.into())
            .unwrap();
        let mut bounds = toml::Table::new();
        bounds.insert("min".into(), vec3([-0.24; 3]));
        bounds.insert("max".into(), vec3([0.24; 3]));
        world.set_component(e, "bounds", bounds.into()).unwrap();
        props.push(e);
    }
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let empty = gpu.frame(&mut renderer, &FlintWorld::new());
    let plain = gpu.frame(&mut renderer, &world);
    for &e in &props {
        world.set_component(e, "ascii_style", anchored()).unwrap();
    }
    let styled = gpu.frame(&mut renderer, &world);
    // One variant a prop, and ids well past the old 256
    let ids = renderer.style_ids();
    let variants = ids.variants_of("ascii").count();
    let top = ids
        .variants_of("ascii")
        .map(|(id, _)| id)
        .max()
        .unwrap_or(0);
    eprintln!("{variants} ascii variants, highest id {top}");
    assert_eq!(variants, props.len(), "a prop shares another's variant");
    assert!(top > 256);

    let px = |f: &[u8], i: usize| [f[i * 4], f[i * 4 + 1], f[i * 4 + 2]];
    let n = (SIZE * SIZE) as usize;
    // A prop's own pixels, not the ground its ambient occlusion darkens by
    // a level or two
    let covered: Vec<usize> = (0..n)
        .filter(|&i| {
            let (a, b) = (px(&plain, i), px(&empty, i));
            (0..3).any(|k| a[k].abs_diff(b[k]) > 16)
        })
        .collect();
    let untouched = covered
        .iter()
        .filter(|&&i| px(&styled, i) == px(&plain, i))
        .count();
    eprintln!("{} covered pixels, {untouched} drawn plain", covered.len());
    assert!(covered.len() > n / 4, "the props cover too little to tell");
    // A glyph cell can land on the very colour of the shading here and
    // there; a prop left out of the table leaves all its pixels plain
    assert!(
        untouched * 50 < covered.len(),
        "{untouched} of {} prop pixels drew plain",
        covered.len()
    );

    // Warmed up, the table holds: no buffer or bind group a frame
    gpu.frame(&mut renderer, &world);
    let (binds, objects) = (renderer.kept_post_binds(), held(&gpu));
    assert_eq!(gpu.frame(&mut renderer, &world), styled);
    assert_eq!(renderer.kept_post_binds(), binds);
    assert_eq!(held(&gpu), objects, "the variant table allocated a frame");
}

/// #342: ASCII's `smoothing` is per 60 Hz frame, so a step in the cells'
/// colour settles in the same game time at 30 Hz and at 120 Hz (four
/// times as many frames), within a 30 Hz frame. Under the old per-frame
/// weight the 120 Hz run settled in a quarter of the 30 Hz run's time.
#[test]
fn ascii_smoothing_settles_in_the_same_time_at_any_frame_rate() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let world = FlintWorld::new();
    // The sky, a little brighter after the step: a change under the
    // snap threshold, so the cells average it in rather than take it
    const STEP: f32 = 0.3;
    let config = |renderer: &SceneRenderer, exposure: f32, temporal: &str| {
        let mut config = renderer.post_process_config().clone();
        config.styles.set_enabled("ascii", true);
        config.styles.set_option("ascii", "temporal", temporal);
        config.styles.set_param("ascii", "smoothing", 0.9);
        config.styles.set_param("ascii", "hysteresis", 0.0);
        config.styles.set_param("ascii", "exposure", exposure);
        // Ink and ground both in the cell's own colour, so the frame
        // follows the cells' colour smoothly whatever glyph a cell draws
        config.styles.set_option("ascii", "color", "raw");
        config.styles.set_param("ascii", "background", 1.0);
        config
    };
    let mean = |pixels: &[u8]| -> f64 {
        let rgb = pixels.chunks(4).flat_map(|p| &p[..3]);
        rgb.map(|&b| b as f64).sum::<f64>() / (pixels.len() / 4 * 3) as f64
    };
    let renderer = || {
        let mut r = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        r.ensure_style_resources(&gpu.device);
        r
    };
    // Where each run starts and ends, drawn without history
    let still = |exposure: f32| {
        let mut r = renderer();
        r.set_post_process_config(config(&r, exposure, "off"));
        mean(&gpu.frame(&mut r, &world))
    };
    let (from, to) = (still(0.0), still(STEP));
    eprintln!("mean byte {from:.2} -> {to:.2}");
    assert!(
        (to - from).abs() > 2.0,
        "the step moved the frame by {:.2} of a byte; the bench tests nothing",
        to - from
    );
    // The game time at which a run of frames `hz` a second is nine tenths
    // of the way there; the style is told each frame is `style_dt` long
    let run = |hz: f32, style_dt: f32| -> f32 {
        let dt = 1.0 / hz;
        let mut r = renderer();
        r.style_dt = style_dt;
        r.set_post_process_config(config(&r, 0.0, "on"));
        gpu.frame(&mut r, &world);
        r.set_post_process_config(config(&r, STEP, "on"));
        let mut trace = Vec::new();
        for i in 1..=(2.0 * hz) as usize {
            let gone = (mean(&gpu.frame(&mut r, &world)) - from) / (to - from);
            trace.push(format!("{:.3}", gone));
            if gone >= 0.9 {
                eprintln!("{hz} Hz: {}", trace.join(" "));
                return i as f32 * dt;
            }
        }
        panic!("{hz} Hz never settled: {}", trace.join(" "));
    };
    let settle = |hz: f32| run(hz, 1.0 / hz);
    let (slow, fast) = (settle(30.0), settle(120.0));
    eprintln!("settled at 30 Hz in {slow:.3} s, at 120 Hz in {fast:.3} s");
    assert!(
        (slow - fast).abs() <= 1.0 / 30.0 + 1e-4,
        "30 Hz settled in {slow:.3} s, 120 Hz in {fast:.3} s"
    );
    // The control: told nothing of the rate (every frame a 60 Hz one, the
    // old per-frame weight), the two runs are far apart, so the bench can
    // tell. 120 Hz settles in a quarter of 30 Hz's time
    let (slow, fast) = (run(30.0, 1.0 / 60.0), run(120.0, 1.0 / 60.0));
    eprintln!("per frame: 30 Hz in {slow:.3} s, 120 Hz in {fast:.3} s");
    assert!(
        slow - fast > 3.0 / 30.0,
        "per frame, 30 Hz settled in {slow:.3} s, 120 Hz in {fast:.3} s"
    );
}

#[test]
fn a_script_cross_fades_whole_frame_styles_without_allocating_and_clears_exactly() {
    // What set_post_style / set_post_style_param do (#369): edit the
    // renderer's [post_process.styles] table, here a lit cube under
    // Kuwahara (HDR) and invert (display), each faded by its `blend`
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::post_style::{reset_post_style, set_post_style, set_post_style_param};
    let mut world = FlintWorld::new();
    let cube = world.spawn("cube").unwrap();
    let vec3 = |v: [f64; 3]| toml::Value::Array(v.iter().map(|&x| x.into()).collect());
    let mut transform = toml::Table::new();
    transform.insert("rotation".into(), vec3([20.0, 35.0, 0.0]));
    world
        .set_component(cube, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-1.0, -1.0, -1.0]));
    bounds.insert("max".into(), vec3([1.0, 1.0, 1.0]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();

    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let authored = renderer.post_process_config().clone();
    let plain = gpu.frame(&mut renderer, &world);
    let edit = |renderer: &mut SceneRenderer, f: &dyn Fn(&mut flint_render::PostProcessConfig)| {
        let mut config = renderer.post_process_config().clone();
        f(&mut config);
        renderer.set_post_process_config(config);
    };

    // Invert on at blend 0 shows none of it, at 1 all of it
    edit(&mut renderer, &|c| {
        set_post_style(c, "invert", true).unwrap();
        set_post_style_param(c, "invert", "blend", 0.0.into()).unwrap();
    });
    let none = gpu.frame(&mut renderer, &world);
    let worst = plain.iter().zip(&none).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(
        worst <= Some(1),
        "blend 0 should be the plain frame, off by {worst:?}"
    );
    edit(&mut renderer, &|c| {
        set_post_style_param(c, "invert", "blend", 1.0.into()).unwrap()
    });
    let full = gpu.frame(&mut renderer, &world);
    for (i, (a, b)) in plain.iter().zip(&full).enumerate() {
        if i % 4 != 3 {
            assert!(
                (255 - *a).abs_diff(*b) <= 1,
                "byte {i}: plain {a}, inverted {b}"
            );
        }
    }

    // A cross-fade: Kuwahara and invert ramp in together every frame, as a
    // script tweening them would. Warm up (Kuwahara's first build, the
    // blend pipelines and binds), then count what is held.
    let fade = |renderer: &mut SceneRenderer, i: usize| {
        let t = i as f64 / 24.0;
        edit(renderer, &|c| {
            set_post_style(c, "kuwahara", true).unwrap();
            set_post_style_param(c, "kuwahara", "blend", t.into()).unwrap();
            set_post_style_param(c, "kuwahara", "radius", (2.0 + 4.0 * t).into()).unwrap();
            set_post_style_param(c, "invert", "blend", (0.5 * t).into()).unwrap();
        });
        gpu.frame(renderer, &world)
    };
    fade(&mut renderer, 0);
    fade(&mut renderer, 1);
    let (binds, objects) = (renderer.kept_post_binds(), held(&gpu));
    let mut seen = std::collections::BTreeSet::new();
    for i in 1..=24 {
        seen.insert(fade(&mut renderer, i));
    }
    eprintln!(
        "24 faded frames: {} distinct, post binds {binds} -> {}, held {objects:?} -> {:?}",
        seen.len(),
        renderer.kept_post_binds(),
        held(&gpu)
    );
    assert!(seen.len() > 12, "the fade changed little on screen");
    assert!(!seen.contains(&plain), "a faded frame was the plain one");
    assert_eq!(
        renderer.kept_post_binds(),
        binds,
        "a fade step kept a new bind group"
    );
    assert_eq!(held(&gpu), objects, "a fade step left GPU objects behind");

    // Clear: both off gives back the plain frame byte for byte, and so
    // does putting the scene's own settings back
    edit(&mut renderer, &|c| {
        set_post_style(c, "kuwahara", false).unwrap();
        set_post_style(c, "invert", false).unwrap();
    });
    assert_eq!(gpu.frame(&mut renderer, &world), plain);
    edit(&mut renderer, &|c| {
        set_post_style(c, "kuwahara", true).unwrap();
        reset_post_style(c, &authored, "kuwahara").unwrap();
        reset_post_style(c, &authored, "invert").unwrap();
    });
    assert_eq!(gpu.frame(&mut renderer, &world), plain);
}
