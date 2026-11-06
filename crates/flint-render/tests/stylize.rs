//! Stylize passes (#262) on a real device: a display style inverts the
//! finished frame, switching it off gives back the exact bytes, the chain
//! survives a resize, and an HDR style and a display style run in one
//! frame without a validation error. The ASCII style (#265) draws one
//! glyph per cell on one grid, and its cells scale with pixels per point.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use flint_ecs::FlintWorld;
use flint_render::{Camera, RendererConfig, SceneRenderer};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
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
    let world = FlintWorld::new();
    let mut config = renderer.post_process_config().clone();
    config.kuwahara_enabled = true;
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
