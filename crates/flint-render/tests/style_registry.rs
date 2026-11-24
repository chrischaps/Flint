//! The style registry is a value the renderer owns (#742), not the
//! process's: two renderers in one process draw two user styles of one
//! name, loaded from two folders, each its own, frame after frame and in
//! either order. Until #742 the second load replaced the first in the one
//! process-wide registry, so both renderers drew whichever loaded last.
//!
//! Needs a GPU adapter; without one the test prints a note and passes.

#![allow(clippy::disallowed_methods)] // the test's own targets and read-backs, not the renderer's, so uncounted (#1091)

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_render::stylize::StyleRegistry;
use flint_render::{Camera, RendererConfig, SceneRenderer};

const SIZE: u32 = 16;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A folder `<root>/<name>` holding `styles/utest_twin.shader.toml`: a
/// display style that paints the frame one grey, `level`.
fn twin_in(root: &std::path::Path, name: &str, level: f32) -> std::path::PathBuf {
    let dir = root.join(name);
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_twin.shader.toml"),
        format!(
            "version = 1\nname = \"utest_twin\"\nshader = \"twin.wgsl\"\n\
             [params.level]\ndefault = {level}\nmin = 0.0\nmax = 1.0\n"
        ),
    )
    .unwrap();
    std::fs::write(
        styles.join("twin.wgsl"),
        "fn style(p: StylePixel) -> vec4<f32> {\n    \
         return vec4<f32>(params.level, params.level, params.level, 1.0);\n}\n",
    )
    .unwrap();
    dir
}

/// A colour target and depth of SIZE, read back after each frame.
struct Target {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

impl Target {
    fn new(device: &wgpu::Device) -> Self {
        let make = |format, usage| {
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
        let color = make(
            FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = make(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Self {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
        }
    }

    /// One frame of `renderer`, and its top-left pixel's red.
    fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut SceneRenderer,
        world: &FlintWorld,
    ) -> u8 {
        renderer.prepare(device, queue, world, &flint_render::FrameInputs::default());
        renderer.render_to(
            device,
            queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        let row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * SIZE) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
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
        queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        let _ = flint_render::wait_for_gpu(device);
        let data = buffer.slice(..).get_mapped_range().expect("mapped");
        data[0]
    }
}

/// A renderer drawing with the scene registry of `dir`, its user style on.
fn renderer_for(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dir: &std::path::Path,
) -> (SceneRenderer, StyleRegistry) {
    let scope = flint_core::project::AssetScope::for_dir(dir);
    let (styles, messages) = StyleRegistry::new().for_scene(&scope);
    assert!(messages.is_empty(), "{messages:?}");
    let mut renderer =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    renderer.set_styles(styles.clone());
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("utest_twin", true);
    renderer.set_post_process_config(config);
    renderer.ensure_style_resources(device);
    (renderer, styles)
}

#[test]
fn two_renderers_draw_same_named_user_styles_from_two_folders() {
    let Some(lease) = shared_gpu::lease() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let (device, queue) = (&lease.gpu.device, &lease.gpu.queue);
    let root = std::env::temp_dir().join(format!("flint-twins-742-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    let dark_dir = twin_in(&root, "dark", 0.25);
    let light_dir = twin_in(&root, "light", 0.75);

    let world = FlintWorld::new();
    let target = Target::new(device);
    let (mut dark, dark_styles) = renderer_for(device, queue, &dark_dir);
    // The second load, in the same process, of a style of the same name
    let (mut light, light_styles) = renderer_for(device, queue, &light_dir);

    // Two installs of one name, side by side
    let (a, b) = (
        dark_styles.style("utest_twin").unwrap(),
        light_styles.style("utest_twin").unwrap(),
    );
    assert!(!a.same(b), "two installs: {a:?} and {b:?}");
    assert_eq!(
        dark.styles()
            .find("utest_twin")
            .unwrap()
            .param(&Default::default(), "level"),
        0.25
    );
    assert_eq!(
        light
            .styles()
            .find("utest_twin")
            .unwrap()
            .param(&Default::default(), "level"),
        0.75
    );

    // Each draws its own, in either order, frame after frame (display
    // values: 0.25 and 0.75 of 255, within a level or two)
    let near = |got: u8, want: f32| (got as f32 - want * 255.0).abs() < 4.0;
    for round in 0..3 {
        let d = target.draw(device, queue, &mut dark, &world);
        let l = target.draw(device, queue, &mut light, &world);
        eprintln!("round {round}: dark renderer drew {d}, light renderer drew {l}");
        assert!(near(d, 0.25), "round {round}: the dark renderer drew {d}");
        assert!(near(l, 0.75), "round {round}: the light renderer drew {l}");
        let l = target.draw(device, queue, &mut light, &world);
        let d = target.draw(device, queue, &mut dark, &world);
        assert!(near(l, 0.75), "round {round}, light first: {l}");
        assert!(near(d, 0.25), "round {round}, light first: dark drew {d}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// #1271: a user style reloaded in one renderer leaves another renderer's
/// kept bind groups as they were, though the two installs share their
/// bind-cache names (interned, one name for every install). Until #1271 a
/// replaced install's names went into one process-wide log that every
/// cache drained, so the other renderer dropped its groups under those
/// names at its next frame and made them again: as many groups a reload as
/// the style binds.
#[test]
fn a_reload_in_one_renderer_leaves_the_others_bind_groups() {
    let Some(lease) = shared_gpu::lease() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let (device, queue) = (&lease.gpu.device, &lease.gpu.queue);
    let root = std::env::temp_dir().join(format!("flint-twins-1271-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    let dark_dir = twin_in(&root, "dark", 0.25);
    let light_dir = twin_in(&root, "light", 0.75);
    // The dark style's other copy: loading one file and then the other
    // replaces the style each time, as a save does
    let other_dark = twin_in(&root, "dark_again", 0.25);
    let tomls = [&dark_dir, &other_dark].map(|d| d.join("styles").join("utest_twin.shader.toml"));

    let world = FlintWorld::new();
    let target = Target::new(device);
    let (mut dark, mut dark_styles) = renderer_for(device, queue, &dark_dir);
    let (mut light, _light_styles) = renderer_for(device, queue, &light_dir);
    let near = |got: u8, want: f32| (got as f32 - want * 255.0).abs() < 4.0;
    // Two frames each: whatever a first frame makes once is made
    for _ in 0..2 {
        target.draw(device, queue, &mut dark, &world);
        target.draw(device, queue, &mut light, &world);
    }
    let (dark_kept, light_kept) = (dark.kept_post_binds(), light.kept_post_binds());
    let light_made = light.post_binds_made();
    assert!(
        light_kept > 0,
        "the light renderer keeps its style's groups"
    );

    let mut generation = dark_styles.style("utest_twin").unwrap().generation;
    let mut seen = Vec::new();
    for reload in 1..=10usize {
        // A save of the dark folder's style, loaded into the dark
        // renderer's registry alone: a new install in its old place
        let mut messages = Vec::new();
        let toml = &tomls[reload % 2];
        dark_styles.load_file(toml, &mut messages).unwrap();
        assert!(messages.is_empty(), "{messages:?}");
        let now = dark_styles.style("utest_twin").unwrap().generation;
        assert_ne!(now, generation, "reload {reload} is a new install");
        generation = now;
        dark.set_styles(dark_styles.clone());
        let d = target.draw(device, queue, &mut dark, &world);
        let l = target.draw(device, queue, &mut light, &world);
        assert!(near(d, 0.25), "reload {reload}: the dark renderer drew {d}");
        assert!(
            near(l, 0.75),
            "reload {reload}: the light renderer drew {l}"
        );
        seen.push((
            dark.kept_post_binds(),
            light.kept_post_binds(),
            light.post_binds_made() - light_made,
        ));
    }
    eprintln!("after each reload (dark kept, light kept, light made since): {seen:?}");
    // The reloading renderer drops its old install's groups (#427) ...
    assert!(
        seen.iter().all(|&(d, _, _)| d == dark_kept),
        "the dark renderer's cache grew with its reloads: {seen:?}"
    );
    // ... and the other one keeps every group it had, making none
    assert!(
        seen.iter()
            .all(|&(_, l, made)| l == light_kept && made == 0),
        "the light renderer made bind groups again for the dark one's reloads: {seen:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
