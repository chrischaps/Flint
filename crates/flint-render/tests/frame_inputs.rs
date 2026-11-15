//! The host protocol (ADR 0118 decision 5, #739): `bind_scene` once a scene,
//! `prepare` once a frame. A host that leaves a frame input at its default
//! gets the feature off, never the last frame's value; a scene bound loads
//! its terrain and masks it when it is styled, and forgets the last scene's.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

mod shared_gpu;

use flint_core::world_line::WorldLine;
use flint_ecs::FlintWorld;
use flint_render::{Camera, FrameInputs, RendererConfig, SceneBinding, SceneRenderer};
use flint_wooze::BoilFrame;
use std::path::PathBuf;

const SIZE: u32 = 96;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    _lease: shared_gpu::Lease,
}

fn gpu() -> Option<Gpu> {
    let lease = shared_gpu::lease()?;
    let shared = lease.gpu;
    let target = |format, usage| {
        shared.device.create_texture(&wgpu::TextureDescriptor {
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
        device: &shared.device,
        queue: &shared.queue,
        _lease: lease,
    })
}

impl Gpu {
    fn renderer(&self) -> SceneRenderer {
        SceneRenderer::new_headless(
            self.device,
            self.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        )
    }

    /// One frame of `world` with `inputs`, as every host draws it:
    /// `prepare`, then the frame, read back.
    fn frame(
        &self,
        renderer: &mut SceneRenderer,
        world: &FlintWorld,
        inputs: &FrameInputs,
    ) -> Vec<u8> {
        renderer.prepare(self.device, self.queue, world, inputs);
        renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        self.pixels()
    }

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
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        let _ = flint_render::wait_for_gpu(self.device);
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped buffer range");
        (0..SIZE as usize)
            .flat_map(|y| {
                let start = y * row as usize;
                data[start..start + SIZE as usize * 4].to_vec()
            })
            .collect()
    }
}

fn vec3(v: [f64; 3]) -> toml::Value {
    toml::Value::Array(v.iter().map(|&x| x.into()).collect())
}

/// A box `half` metres each way at the origin.
fn boxed(world: &mut FlintWorld, name: &str, half: f64) {
    let id = world.spawn(name).unwrap();
    let mut transform = toml::Table::new();
    transform.insert("position".into(), vec3([0.0, 0.0, 0.0]));
    world
        .set_component(id, "transform", transform.into())
        .unwrap();
    let mut bounds = toml::Table::new();
    bounds.insert("min".into(), vec3([-half, -half, -half]));
    bounds.insert("max".into(), vec3([half, half, half]));
    world.set_component(id, "bounds", bounds.into()).unwrap();
}

/// The fixture scene whose folder the terrain's heightmap is found from.
fn fixture_scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/style_terrain.scene.toml")
}

/// A world with one heightmap terrain, 80 m square around the origin,
/// just below it.
fn terrain_world() -> (FlintWorld, flint_core::EntityId) {
    let mut world = FlintWorld::new();
    let ground = world.spawn("ground").unwrap();
    let mut transform = toml::Table::new();
    transform.insert("position".into(), vec3([-40.0, -1.0, -40.0]));
    world
        .set_component(ground, "transform", transform.into())
        .unwrap();
    let mut terrain = toml::Table::new();
    // The path the fixture scene names (#985 keeps one copy, in examples/)
    terrain.insert(
        "heightmap".into(),
        "../../../examples/terrain/heightmap.png".into(),
    );
    terrain.insert("width".into(), 80.0.into());
    terrain.insert("depth".into(), 80.0.into());
    terrain.insert("height_scale".into(), 1.0.into());
    terrain.insert("chunk_resolution".into(), 64.into());
    world
        .set_component(ground, "terrain", terrain.into())
        .unwrap();
    (world, ground)
}

/// #739: binding a scene loads its terrain and names it for the style mask,
/// a step each host took by hand before (and the viewer never). Binding the
/// next scene forgets it.
#[test]
fn a_bound_scene_draws_its_terrain_masked_and_the_next_forgets_it() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    use flint_render::entity_style::set_entity_style;
    let (mut world, ground) = terrain_world();
    let binding = SceneBinding::for_scene(&fixture_scene());
    let mut renderer = gpu.renderer();
    let bound = renderer.bind_scene(gpu.device, gpu.queue, &world, &binding);
    let terrain = bound.terrain.expect("the scene's terrain loaded");
    assert_eq!(terrain.entity, ground);
    assert_eq!(terrain.heightmap_size, (257, 257));
    let plain = gpu.frame(&mut renderer, &world, &FrameInputs::default());

    set_entity_style(&mut world, ground, "ascii").unwrap();
    let styled = gpu.frame(&mut renderer, &world, &FrameInputs::default());
    // The bottom rows are all ground: glyphs there, since the binding named
    // the terrain's entity for the mask
    let bottom = (SIZE as usize - 8) * SIZE as usize * 4;
    assert_ne!(
        styled[bottom..],
        plain[bottom..],
        "the bound terrain took no style"
    );

    // The next scene has no terrain: nothing of this one's is drawn
    let empty = FlintWorld::new();
    let bound = renderer.bind_scene(gpu.device, gpu.queue, &empty, &binding);
    assert!(bound.terrain.is_none() && bound.grass.is_none());
    let after = gpu.frame(&mut renderer, &empty, &FrameInputs::default());
    let mut fresh = gpu.renderer();
    fresh.bind_scene(gpu.device, gpu.queue, &empty, &binding);
    assert_eq!(
        after,
        gpu.frame(&mut fresh, &empty, &FrameInputs::default()),
        "the last scene's terrain is still drawn"
    );
}

/// #739: what a frame's inputs leave at the default is off that frame,
/// whatever an earlier frame set: no host can leave a stale world line or
/// boil behind by not setting it.
#[test]
fn a_frame_input_left_at_its_default_is_off() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut world = FlintWorld::new();
    boxed(&mut world, "cube", 1.0);
    let lines = [WorldLine {
        a: [-6.0, 0.0, 6.0],
        b: [6.0, 0.0, -6.0],
        color: [1.0, 0.1, 0.1, 1.0],
        thickness: 6.0,
    }];
    let boil = BoilFrame {
        seed: 7,
        amount: 0.5,
        step: 0.25,
    };

    let mut fresh = gpu.renderer();
    let bare = gpu.frame(&mut fresh, &world, &FrameInputs::default());

    let mut renderer = gpu.renderer();
    let with_line = gpu.frame(
        &mut renderer,
        &world,
        &FrameInputs {
            world_lines: &lines,
            boil,
            ..Default::default()
        },
    );
    assert_ne!(with_line, bare, "the world line drew nothing");
    assert_eq!(renderer.post_process_config().boil, boil);

    let next = gpu.frame(&mut renderer, &world, &FrameInputs::default());
    assert_eq!(next, bare, "a world line outlived the frame that drew it");
    assert_eq!(
        renderer.post_process_config().boil,
        BoilFrame::default(),
        "the boil outlived its frame"
    );
}

/// The frame's boil is drawn whatever config a host sets after `prepare`
/// (a look, a panel's edit), as the player's did when it set the boil last.
#[test]
fn the_prepared_boil_survives_a_config_set_after_it() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let world = FlintWorld::new();
    let boil = BoilFrame {
        seed: 3,
        amount: 1.0,
        step: 0.5,
    };
    let mut renderer = gpu.renderer();
    renderer.prepare(
        gpu.device,
        gpu.queue,
        &world,
        &FrameInputs {
            boil,
            ..Default::default()
        },
    );
    // A look arrives with a fresh config, which carries no boil
    renderer.set_post_process_config(flint_render::PostProcessConfig::default());
    renderer.render_to(
        gpu.device,
        gpu.queue,
        &gpu.depth_view,
        &Camera::default(),
        &gpu.color_view,
    );
    assert_eq!(renderer.effective_post_process_config().boil, boil);
}
