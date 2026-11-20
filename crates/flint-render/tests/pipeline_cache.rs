//! Pipelines kept across renderers on one device (#911), on a real device:
//! a second renderer takes the first one's pipelines and none of its
//! state, so its first frame is the bytes a renderer built from nothing
//! draws, though the first had ASCII's cell history full of another
//! camera's frames; and a style whose WGSL changed builds a new pipeline
//! instead of taking the one kept for the old text. A model's or file's
//! texture and a mesh's vertex and index buffers ride the same cache
//! (#961): uploaded once a device, keyed by their bytes, and never written
//! in place while kept.
//!
//! A binary of its own, since the cache (like the shader folder the second
//! test points at) is the process's. Each test switches it holding the
//! device alone, and its lease puts the harness's setting back (#913).
//! Needs a GPU adapter; without one each test prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_render::pipeline_cache;
use flint_render::stylize::dev_shader;
use flint_render::{Camera, RendererConfig, SceneRenderer};
use std::path::Path;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Frame {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

impl Frame {
    fn new(device: &'static wgpu::Device, queue: &'static wgpu::Queue) -> Self {
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
        Self {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            device,
            queue,
        }
    }

    fn draw(&self, renderer: &mut SceneRenderer, world: &FlintWorld, camera: &Camera) -> Vec<u8> {
        renderer.prepare(
            self.device,
            self.queue,
            world,
            &flint_render::FrameInputs::default(),
        );
        renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            camera,
            &self.color_view,
        );
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
        let _ = flint_render::wait_for_gpu(&self.device);
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

/// A tilted box: its edges cut cells, so a camera that moves a little
/// changes glyphs, and ASCII's history has something to hold.
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
    bounds.insert("min".into(), vec3([-1.0, -1.0, -1.0]));
    bounds.insert("max".into(), vec3([1.0, 1.0, 1.0]));
    world.set_component(cube, "bounds", bounds.into()).unwrap();
    world
}

/// The camera, moved off its target by `by` metres.
fn camera(by: f32) -> Camera {
    let mut camera = Camera::default();
    camera.target.x += by;
    camera.target.y += by;
    camera
}

/// A renderer with ASCII on and its cell history kept (#277).
fn ascii(frame: &Frame) -> SceneRenderer {
    let mut renderer = SceneRenderer::new_headless(
        frame.device,
        frame.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("ascii", true);
    config.styles.set_option("ascii", "temporal", "on");
    renderer.set_post_process_config(config);
    renderer.ensure_style_resources(frame.device);
    renderer
}

fn count(label: &str) -> pipeline_cache::Count {
    pipeline_cache::counts()
        .get(label)
        .copied()
        .unwrap_or_default()
}

#[test]
fn a_second_renderer_takes_the_first_ones_pipelines_and_none_of_its_state() {
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let frame = Frame::new(&lease.gpu.device, &lease.gpu.queue);
    let world = box_world();

    // Built from nothing: what a case drew before #911
    pipeline_cache::set_enabled(false);
    let fresh = frame.draw(&mut ascii(&frame), &world, &camera(0.08));

    pipeline_cache::set_enabled(true);
    pipeline_cache::reset_counts();
    // The first case: two dozen frames of another camera fill its history
    let mut first = ascii(&frame);
    for i in 0..24 {
        frame.draw(
            &mut first,
            &world,
            &camera(if i % 2 == 0 { 0.0 } else { 0.3 }),
        );
    }
    // That history shows: the first renderer's next frame isn't the fresh
    // one, so a second renderer that saw it would differ too
    let remembered = frame.draw(&mut first, &world, &camera(0.08));
    assert_ne!(
        remembered, fresh,
        "the history changed nothing; the test can't tell a leak"
    );
    let glyph = count("ASCII Pipeline");
    assert_eq!(glyph.built, 1, "{glyph:?}");

    // The next case, on the same device: the same pipelines, a clean slate
    let mut second = ascii(&frame);
    let next = frame.draw(&mut second, &world, &camera(0.08));
    assert_eq!(next, fresh, "the second renderer saw the first one's state");
    let glyph = count("ASCII Pipeline");
    assert_eq!(
        glyph.built, 1,
        "ASCII's glyph pipeline built again: {glyph:?}"
    );
    assert_eq!(glyph.reused, 1, "{glyph:?}");
    let pbr = count("scene: PBR");
    assert_eq!((pbr.built, pbr.reused), (1, 1), "{pbr:?}");

    // Off again: nothing is kept, so a third builds its own
    drop((first, second));
    pipeline_cache::set_enabled(false);
    let mut third = ascii(&frame);
    assert_eq!(frame.draw(&mut third, &world, &camera(0.08)), fresh);
    assert_eq!(count("ASCII Pipeline").built, 2);
}

#[test]
#[allow(clippy::disallowed_methods)] // a test reads its own files off the disk
fn a_changed_shader_builds_a_new_pipeline_not_the_kept_one() {
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let frame = Frame::new(&lease.gpu.device, &lease.gpu.queue);
    let world = FlintWorld::new();
    // The built-in styles' WGSL from a folder (#418), so the test can edit
    // one between two renderers the way a hot reload or a swapped
    // FLINT_STYLE_SHADER_DIR does
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pipeline_cache_{}", std::process::id()));
    let dir = root.join("stylize");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&dir).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(src.join("stylize")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    // `../palette.wgsl`, which a stylize shader names
    std::fs::copy(src.join("palette.wgsl"), root.join("palette.wgsl")).unwrap();
    let file = dir.join("invert.wgsl");
    dev_shader::set_dir(Some(dir.clone()));
    pipeline_cache::set_enabled(true);
    // The harness keeps the cache on (#913), and the copy's text is the
    // built-in one, so an invert another test built would be taken here
    pipeline_cache::clear();
    pipeline_cache::reset_counts();

    let invert = || {
        let mut renderer = SceneRenderer::new_headless(
            frame.device,
            frame.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let mut config = renderer.post_process_config().clone();
        config.styles.set_enabled("invert", true);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(frame.device);
        renderer
    };
    let mut plain = SceneRenderer::new_headless(
        frame.device,
        frame.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let plain = frame.draw(&mut plain, &world, &Camera::default());
    let inverted = frame.draw(&mut invert(), &world, &Camera::default());
    assert_ne!(inverted, plain, "invert drew nothing");
    // The same text again: the kept pipeline
    assert_eq!(
        frame.draw(&mut invert(), &world, &Camera::default()),
        inverted
    );
    assert_eq!(count("Invert Pipeline").built, 1);
    assert_eq!(count("Invert Pipeline").reused, 1);

    // Invert by nothing: a new text, so a new pipeline, drawing the plain
    // frame, never the kept one's inverted frame
    let original = std::fs::read_to_string(&file).unwrap();
    let strength = "vec3<f32>(1.0) - shown, params.strength)";
    assert!(original.contains(strength));
    std::fs::write(
        &file,
        original.replace(strength, "vec3<f32>(1.0) - shown, 0.0 * params.strength)"),
    )
    .unwrap();
    assert_eq!(frame.draw(&mut invert(), &world, &Camera::default()), plain);
    assert_eq!(count("Invert Pipeline").built, 2);

    // And back: the first text's pipeline, kept all along
    std::fs::write(&file, &original).unwrap();
    assert_eq!(
        frame.draw(&mut invert(), &world, &Camera::default()),
        inverted
    );
    assert_eq!(count("Invert Pipeline").built, 2);

    pipeline_cache::set_enabled(false);
    dev_shader::set_dir(None);
    let _ = std::fs::remove_dir_all(&root);
}

/// #961: on WARP each golden case uploaded its scene's textures again, up
/// to 8 s a case. With the cache on, a second renderer's cache takes the
/// texture the first one uploaded from the same file; one whose file now
/// holds other pixels gets a texture of its own; a kept texture refuses
/// `update_rgba`, whose write would reach every renderer drawing it; and
/// with the cache off nothing is kept and the write goes through.
#[test]
#[allow(clippy::disallowed_methods)] // a test writes its own files to the disk
fn a_file_texture_is_uploaded_once_a_device_and_never_written_while_kept() {
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let (device, queue) = (&lease.gpu.device, &lease.gpu.queue);
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("texture_keep_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("probe.png");
    let paint = |shade: u8| {
        image::RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([shade, x as u8 * 8, y as u8 * 8, 255])
        })
        .save(&file)
        .unwrap();
    };
    let label = flint_render::KEPT_TEXTURE_LABEL;
    pipeline_cache::set_enabled(true);
    pipeline_cache::clear();
    pipeline_cache::reset_counts();

    paint(40);
    let load = || {
        let mut cache = flint_render::TextureCache::new(device, queue);
        cache
            .load_file_mipmapped(device, queue, "probe", &file)
            .unwrap();
        cache
    };
    let (first, second) = (load(), load());
    let texture = |c: &flint_render::TextureCache| c.get("probe").unwrap().texture.clone();
    assert_eq!(
        texture(&first),
        texture(&second),
        "the second cache uploaded its own"
    );
    assert_eq!((count(label).built, count(label).reused), (1, 1));
    let pixels = vec![0u8; 32 * 32 * 4];
    let refused = second.update_rgba(queue, "probe", &pixels).unwrap_err();
    assert!(refused.contains("shared"), "{refused}");

    // Other pixels behind the same name and path: a texture of its own
    paint(200);
    let third = load();
    assert_ne!(
        texture(&third),
        texture(&first),
        "took the old file's texture"
    );
    assert_eq!(count(label).built, 2);

    // Off: nothing kept, so the write is the cache's own to make
    pipeline_cache::set_enabled(false);
    let own = load();
    assert_ne!(texture(&own), texture(&third));
    own.update_rgba(queue, "probe", &pixels).unwrap();
    assert_eq!(count(label).built, 2, "counted with the cache off");

    pipeline_cache::clear();
    let _ = std::fs::remove_dir_all(&dir);
}

/// #961: an imported mesh's vertex and index buffers ride the cache too
/// (on WARP the skinned YBot's took 0.57 s a case). A second mesh cache on
/// the device takes the buffers the first uploaded from the same model,
/// moved vertices get their own, a procedural mesh is never kept (a host
/// replaces one at run time), and with the cache off nothing is shared.
#[test]
fn a_mesh_is_uploaded_once_a_device_by_its_bytes() {
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let device = &lease.gpu.device;
    let label = flint_render::KEPT_GEOMETRY_LABEL;
    pipeline_cache::set_enabled(true);
    pipeline_cache::clear();
    pipeline_cache::reset_counts();

    let sphere = flint_import::import_gltf(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/scenes/models/Sphere.glb"),
    )
    .unwrap();
    let mut moved_sphere = sphere.clone();
    moved_sphere.meshes[0].positions[0][0] += 0.25;
    let upload = |model: &flint_import::ImportResult| {
        let mut cache = flint_render::MeshCache::new();
        cache.upload_imported(device, "probe", model, [1.0; 4]);
        let mesh = &cache.get("probe").unwrap()[0];
        (mesh.vertex_buffer.clone(), mesh.index_buffer.clone())
    };
    let (first, second) = (upload(&sphere), upload(&sphere));
    assert!(
        std::sync::Arc::ptr_eq(&first.0, &second.0),
        "vertices uploaded twice"
    );
    assert!(
        std::sync::Arc::ptr_eq(&first.1, &second.1),
        "indices uploaded twice"
    );
    let kept = count(label);
    assert_eq!(kept.reused, kept.built, "{kept:?}");

    // Other vertices: a buffer of their own; the same indices: the kept ones
    let moved = upload(&moved_sphere);
    assert!(
        !std::sync::Arc::ptr_eq(&moved.0, &first.0),
        "took the other mesh's vertices"
    );
    assert!(std::sync::Arc::ptr_eq(&moved.1, &first.1));

    // A procedural mesh: never kept, so replacing one never piles up
    let before = count(label);
    let procedural = || {
        let mut cache = flint_render::MeshCache::new();
        let v = |x: f32| flint_render::Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 4],
            uv: [0.0; 2],
        };
        cache.upload_procedural(
            device,
            "made",
            &[v(0.0), v(1.0), v(2.0)],
            &[0, 1, 2],
            Default::default(),
        );
        cache.get("made").unwrap()[0].vertex_buffer.clone()
    };
    assert!(!std::sync::Arc::ptr_eq(&procedural(), &procedural()));
    assert_eq!(
        count(label),
        before,
        "a procedural mesh went through the cache"
    );

    pipeline_cache::set_enabled(false);
    let own = upload(&sphere);
    assert!(
        !std::sync::Arc::ptr_eq(&own.0, &first.0),
        "shared with the cache off"
    );
    pipeline_cache::clear();
}
