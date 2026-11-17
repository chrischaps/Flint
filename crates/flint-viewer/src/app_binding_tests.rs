//! The viewer's share of a scene goes through the same binding as the
//! player's and `flint render`'s (#739). Before it, `flint edit` loaded no
//! terrain (so a styled hillside wasn't there at all) and read no
//! `[environment]`.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use super::*;

const SIZE: u32 = 96;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// One device for this test binary's GPU tests (#598: a device a test
/// would hang the binary on some drivers).
pub(super) fn gpu() -> Option<&'static (wgpu::Device, wgpu::Queue)> {
    static GPU: std::sync::OnceLock<Option<(wgpu::Device, wgpu::Queue)>> =
        std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        pollster::block_on(async {
            let instance = flint_render::gpu_select::instance();
            let adapter = flint_render::gpu_select::request_adapter(&instance, None).await?;
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .ok()
        })
    })
    .as_ref()
}

/// The fixture scene as `run` reads it: schemas, world, the scene's own
/// tables.
fn viewer_state(scene: &str) -> ViewerState {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes")
        .join(scene);
    let scene_path = path.to_string_lossy().into_owned();
    let (registry, constraint_registry, _) = load_schemas(&scene_path, &[]).unwrap();
    let (world, scene_file) = load_scene(&scene_path, &registry).unwrap();
    ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.clone(),
        needs_reload: false,
        scene_doc: None,
        scene_post: scene_file.post_process.clone(),
        wooze: field_of(&scene_file),
        scene_camera: scene_file.camera.clone(),
        environment: scene_file.environment.clone(),
    }
}

/// The renderer the viewer makes, its scene staged as the viewer stages it.
fn staged(
    state: &mut ViewerState,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (SceneRenderer, flint_render::BoundScene) {
    let mut renderer =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    let config = ModelLoadConfig::from_scene_path(&state.scene_path);
    let binding = scene_binding(&state.scene_path);
    let bound = stage_scene(&mut renderer, device, queue, state, binding, &config);
    (renderer, bound)
}

fn gpu_renderer(device: &wgpu::Device, queue: &wgpu::Queue) -> SceneRenderer {
    SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default())
}

/// Draw the frame the viewer would, and read it back.
fn frame(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &FlintWorld,
    camera: &Camera,
) -> Vec<u8> {
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
    renderer.prepare(device, queue, world, &FrameInputs::default());
    renderer.render_to(
        device,
        queue,
        &depth.create_view(&Default::default()),
        camera,
        &color.create_view(&Default::default()),
    );
    let row = (SIZE * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * SIZE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
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
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped buffer range");
    (0..SIZE as usize)
        .flat_map(|y| data[y * row as usize..][..SIZE as usize * 4].to_vec())
        .collect()
}

/// The fixture's authored framing, as the viewer seeds its orbit camera.
fn scene_camera(state: &ViewerState) -> Camera {
    let mut camera = Camera::new();
    camera.aspect = 1.0;
    if let Some(def) = &state.scene_camera {
        if let Some(p) = def.position {
            camera.position = flint_core::Vec3::new(p[0], p[1], p[2]);
        }
        if let Some(t) = def.target {
            camera.target = flint_core::Vec3::new(t[0], t[1], t[2]);
        }
        if let Some(fov) = def.fov {
            camera.fov = fov;
        }
    }
    camera
}

/// #739: `flint edit` of a scene with a styled heightmap terrain draws the
/// terrain, and its chunks are in the style mask (#310).
#[test]
fn the_viewer_draws_a_scenes_terrain_in_the_style_mask() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut state = viewer_state("style_terrain.scene.toml");
    let (mut renderer, bound) = staged(&mut state, device, queue);
    let terrain = bound
        .terrain
        .expect("the viewer loaded the scene's terrain");
    assert_eq!(state.world.get_name(terrain.entity), Some("ground"));
    assert_eq!(renderer.collect_stats().terrain_total_chunks, 16);

    // Drawn and in the mask: its rows change when its style goes, as only a
    // masked entity's do (with no terrain, both frames show sky there)
    let camera = scene_camera(&state);
    if let Some(def) = &state.scene_post {
        renderer.set_post_process_config(post_process_config_from_def(def, renderer.styles()));
    }
    let styled = frame(&mut renderer, device, queue, &state.world, &camera);
    assert!(renderer.styled_entities().contains_key(&terrain.entity));
    assert!(flint_render::entity_style::clear_entity_style(
        &mut state.world,
        renderer.styles(),
        terrain.entity
    ));
    let plain = frame(&mut renderer, device, queue, &state.world, &camera);
    assert!(!renderer.styled_entities().contains_key(&terrain.entity));
    let bottom = (SIZE as usize - 8) * SIZE as usize * 4;
    assert_ne!(
        styled[bottom..],
        plain[bottom..],
        "the terrain's style changed nothing at the bottom of the frame"
    );
}

/// #739: the viewer sets a scene's `[environment]`: a scene with
/// `default_lights = false` is lit by its ambient alone, as in the player.
#[test]
fn the_viewer_sets_the_scenes_environment() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let mut state = viewer_state("style_terrain.scene.toml");
    let (renderer, _) = staged(&mut state, device, queue);
    let built_in = gpu_renderer(device, queue).lighting_levers();
    assert_eq!(
        renderer.lighting_levers(),
        built_in,
        "the fixture has no [environment]"
    );

    let mut environment: flint_scene::EnvironmentDef =
        toml::from_str("ambient_sky = [0.9, 0.2, 0.1]\noren_nayar = 0.5").unwrap();
    environment.diffuse_wrap = Some(0.3);
    state.environment = Some(environment);
    let (renderer, _) = staged(&mut state, device, queue);
    let levers = renderer.lighting_levers();
    assert_eq!(levers.ambient_sky, [0.9, 0.2, 0.1]);
    assert_eq!(levers.oren_nayar, 0.5);
    assert_eq!(levers.diffuse_wrap, 0.3);
}
