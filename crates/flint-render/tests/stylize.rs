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
