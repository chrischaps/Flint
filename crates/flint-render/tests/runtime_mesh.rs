//! Runtime procedural meshes: create, replace and remove after init without
//! leaking GPU memory, vertex colours, and frustum culling by mesh bounds.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, DebugMode, RendererConfig, SceneRenderer, Vertex};

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
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let color = target(FORMAT, attach | wgpu::TextureUsages::COPY_SRC);
        let depth = target(
            wgpu::TextureFormat::Depth32Float,
            attach | wgpu::TextureUsages::TEXTURE_BINDING,
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

macro_rules! gpu_or_skip {
    () => {
        match gpu() {
            Some(g) => g,
            None => {
                eprintln!("no GPU adapter; skipping");
                return;
            }
        }
    };
}

impl Gpu {
    fn renderer(&self) -> SceneRenderer {
        SceneRenderer::new_headless(
            &self.device,
            &self.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        )
    }

    /// Extract and render one frame, then wait for the GPU.
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld, camera: &Camera) {
        renderer.update_from_world(world, &self.device);
        renderer.render_to(
            &self.device,
            &self.queue,
            &self.depth_view,
            camera,
            &self.color_view,
        );
        self.device.poll(wgpu::Maintain::Wait);
    }

    /// Buffers the application holds, and bytes the allocator holds where
    /// the backend reports them (DX12).
    fn memory(&self) -> (usize, Option<u64>) {
        let buffers = self
            .instance
            .generate_report()
            .map(|r| r.hub_report().buffers.num_kept_from_user)
            .unwrap_or(0);
        let bytes = self
            .device
            .generate_allocator_report()
            .map(|r| r.total_allocated_bytes);
        (buffers, bytes)
    }

    /// The RGBA8 pixel at the centre of the last frame.
    fn centre_pixel(&self) -> [u8; 4] {
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
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device.poll(wgpu::Maintain::Wait);
        let data = buffer.slice(..).get_mapped_range();
        let at = (row * (SIZE / 2) + (SIZE / 2) * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }
}

/// A flat, double-sided square of half-width `half` at the origin.
fn square(half: f32, color: [f32; 4]) -> (Vec<Vertex>, Vec<u32>) {
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        color,
        uv: [0.0, 0.0],
    };
    let vertices = vec![v(-half, -half), v(-half, half), v(half, half), v(half, -half)];
    let indices = vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2];
    (vertices, indices)
}

fn spawn(world: &mut FlintWorld, name: &str, mesh: &str, position: [f32; 3]) {
    let id = world.spawn(name).unwrap();
    let mut transform = toml::Table::new();
    transform.insert(
        "position".into(),
        toml::Value::Array(position.iter().map(|&p| (p as f64).into()).collect()),
    );
    world.set_component(id, "transform", transform.into()).unwrap();
    let mut model = toml::Table::new();
    model.insert("asset".into(), mesh.into());
    model.insert("runtime".into(), true.into());
    world.set_component(id, "model", model.into()).unwrap();
}

#[test]
fn replacing_a_mesh_1000_times_does_not_grow_gpu_memory() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    let mut world = FlintWorld::new();
    spawn(&mut world, "a", "m", [0.0; 3]);
    let camera = Camera::default();

    // Sizes cycle so the allocator can't reuse one block in place.
    let upload = |renderer: &mut SceneRenderer, i: usize| {
        let (vertices, indices) = square(1.0 + (i % 7) as f32, [0.5, 0.5, 0.5, 1.0]);
        let vertices: Vec<Vertex> = (0..1 + i % 5).flat_map(|_| vertices.clone()).collect();
        renderer.load_procedural_mesh(
            &gpu.device,
            "m",
            &vertices,
            &indices,
            ImportedMaterial::vertex_colored("m"),
        );
    };
    for i in 0..20 {
        upload(&mut renderer, i);
        gpu.frame(&mut renderer, &world, &camera);
    }
    let (buffers_before, bytes_before) = gpu.memory();

    for i in 20..1020 {
        upload(&mut renderer, i);
        gpu.frame(&mut renderer, &world, &camera);
    }
    let (buffers_after, bytes_after) = gpu.memory();
    eprintln!(
        "buffers {buffers_before} -> {buffers_after}, allocator bytes {bytes_before:?} -> {bytes_after:?}"
    );
    assert_eq!(buffers_after, buffers_before, "buffers leaked");
    if let (Some(before), Some(after)) = (bytes_before, bytes_after) {
        assert!(
            after <= before + 64 * 1024,
            "GPU memory grew from {before} to {after} bytes"
        );
    }
    assert_eq!(renderer.collect_stats().entity_draws, 1);

    // Removing frees the mesh's vertex and index buffers.
    assert!(renderer.remove_mesh("m"));
    assert!(!renderer.remove_mesh("m"));
    gpu.frame(&mut renderer, &world, &camera);
    assert!(!renderer.mesh_cache().contains("m"));
    assert_eq!(renderer.collect_stats().entity_draws, 0);
    let (buffers_removed, _) = gpu.memory();
    assert!(
        buffers_removed < buffers_after,
        "remove kept its buffers ({buffers_removed} vs {buffers_after})"
    );

    // And it can come back under the same name.
    upload(&mut renderer, 0);
    gpu.frame(&mut renderer, &world, &camera);
    assert_eq!(renderer.collect_stats().entity_draws, 1);
}

#[test]
fn meshes_outside_the_view_are_culled() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    let mut world = FlintWorld::new();
    let (vertices, indices) = square(1.0, [1.0; 4]);
    renderer.load_procedural_mesh(
        &gpu.device,
        "tile",
        &vertices,
        &indices,
        ImportedMaterial::vertex_colored("tile"),
    );
    // The default camera sits at (10, 10, 10) looking at the origin.
    spawn(&mut world, "in_view", "tile", [0.0; 3]);
    spawn(&mut world, "behind", "tile", [60.0, 0.0, 60.0]);
    spawn(&mut world, "far_left", "tile", [-200.0, 0.0, 200.0]);
    let mut camera = Camera::default();

    gpu.frame(&mut renderer, &world, &camera);
    let stats = renderer.collect_stats();
    assert_eq!(stats.entity_draws, 1, "only the tile in view draws");
    assert_eq!(stats.culled_draws, 2);
    assert_eq!(stats.triangles, 4);

    // Turn to look at the one behind: the culled lists come back each
    // render, so a second camera on the same extract sees its own set.
    camera.target = flint_core::Vec3::new(60.0, 0.0, 60.0);
    camera.position = flint_core::Vec3::new(70.0, 10.0, 70.0);
    renderer.render_to(
        &gpu.device,
        &gpu.queue,
        &gpu.depth_view,
        &camera,
        &gpu.color_view,
    );
    let stats = renderer.collect_stats();
    assert_eq!(stats.entity_draws, 1);
    assert_eq!(stats.culled_draws, 2);
}

#[test]
fn vertex_colours_render() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_debug_mode(DebugMode::Unlit);
    let mut world = FlintWorld::new();
    spawn(&mut world, "ground", "ground", [0.0; 3]);
    let camera = Camera::default();

    // Green vertices under a red material colour: the vertices win.
    let (vertices, indices) = square(20.0, [0.0, 1.0, 0.0, 1.0]);
    let material = ImportedMaterial {
        base_color: [1.0, 0.0, 0.0, 1.0],
        ..ImportedMaterial::vertex_colored("ground")
    };
    renderer.load_procedural_mesh(&gpu.device, "ground", &vertices, &indices, material.clone());
    gpu.frame(&mut renderer, &world, &camera);
    let [r, g, b, _] = gpu.centre_pixel();
    assert!(g > 150 && r < 60 && b < 60, "expected green, got {r} {g} {b}");

    // The same mesh with vertex colour off shows the material's red.
    let material = ImportedMaterial {
        use_vertex_color: false,
        ..material
    };
    renderer.load_procedural_mesh(&gpu.device, "ground", &vertices, &indices, material);
    gpu.frame(&mut renderer, &world, &camera);
    let [r, g, b, _] = gpu.centre_pixel();
    assert!(r > 150 && g < 60 && b < 60, "expected red, got {r} {g} {b}");
}

/// Emission (ADR 0076) is added after lighting: a glowing face reads the
/// same whichever way it faces, whatever the ambient and however bright the
/// sun, both from the material and per vertex.
#[test]
fn emission_ignores_facing_and_light() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    let mut world = FlintWorld::new();
    spawn(&mut world, "lamp", "lamp", [0.0; 3]);
    // The test's own sun, overhead, so the default scene lights stay out.
    let sun = world.spawn("sun").unwrap();
    let camera = Camera::default();
    for (name, texel) in [("lamp_green", [0, 255, 0, 255]), ("lamp_black", [0, 0, 0, 255])] {
        renderer
            .load_texture_rgba(&gpu.device, &gpu.queue, name, 1, 1, &texel, false)
            .unwrap();
    }

    let mut shoot =|vertices: &[Vertex], material: ImportedMaterial, ambient: f32, sun_on: f32| {
        let (_, indices) = square(20.0, [0.0; 4]);
        let mut light = toml::Table::new();
        light.insert("type".into(), "directional".into());
        light.insert("intensity".into(), (4.0 * sun_on as f64).into());
        light.insert(
            "direction".into(),
            toml::Value::Array(vec![0.0.into(), 1.0.into(), 0.0.into()]),
        );
        world.set_component(sun, "light", light.into()).unwrap();
        renderer.set_ambient([ambient; 3], [ambient; 3]);
        renderer.load_procedural_mesh(&gpu.device, "lamp", vertices, &indices, material);
        gpu.frame(&mut renderer, &world, &camera);
        gpu.centre_pixel()
    };
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let face_down = |mut v: Vec<Vertex>| {
        v.iter_mut().for_each(|v| v.normal = [0.0, -1.0, 0.0]);
        v
    };

    // A rough black metal reflects next to nothing (F0 = 0), so what's left
    // is the green it gives off.
    let lamp = ImportedMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        roughness: 1.0,
        metallic: 1.0,
        emissive: [0.0, 0.25, 0.0],
        emissive_strength: 2.0,
        ..ImportedMaterial::default()
    };
    let (up, _) = square(20.0, [1.0; 4]);
    let lit = shoot(&up, lamp.clone(), 0.0, 0.0);
    let [r, g, b, _] = lit;
    assert!(g > 150 && r < 30 && b < 30, "expected green, got {r} {g} {b}");
    for (vertices, ambient, sun_on, what) in [
        (&up, 1.0, 0.0, "the ambient"),
        (&up, 0.0, 1.0, "the sun"),
        (&face_down(up.clone()), 1.0, 1.0, "facing away"),
    ] {
        let seen = shoot(vertices, lamp.clone(), ambient, sun_on);
        assert!(close(lit, seen), "{what} changed it: {lit:?} vs {seen:?}");
    }

    // An emissive map multiplies the factor: a grey factor over a green
    // texel gives off the same green, and a black texel gives off nothing.
    let painted = |map: &str| ImportedMaterial {
        emissive: [0.25; 3],
        emissive_texture: Some(map.into()),
        ..lamp.clone()
    };
    let seen = shoot(&up, painted("lamp_green"), 1.0, 1.0);
    assert!(close(lit, seen), "a green emissive map: {lit:?} vs {seen:?}");
    let [r, g, b, _] = shoot(&up, painted("lamp_black"), 0.0, 0.0);
    assert!(r < 10 && g < 10 && b < 10, "a black emissive map glows: {r} {g} {b}");

    // Per vertex: alpha 0 is light, alpha 1 an ordinary lit surface.
    let glowing = ImportedMaterial {
        roughness: 1.0,
        metallic: 1.0,
        vertex_emission: true,
        emissive_strength: 2.0,
        ..ImportedMaterial::vertex_colored("lamp")
    };
    let (light, _) = square(20.0, [0.0, 0.25, 0.0, 0.0]);
    for (vertices, ambient, sun_on, what) in [
        (&light, 0.0, 0.0, "per-vertex light"),
        (&light, 1.0, 1.0, "light on a per-vertex lamp"),
        (&face_down(light.clone()), 1.0, 1.0, "facing away from a per-vertex lamp"),
    ] {
        let seen = shoot(vertices, glowing.clone(), ambient, sun_on);
        assert!(close(lit, seen), "{what}: {lit:?} vs {seen:?}");
    }
    let (surface, _) = square(20.0, [0.0, 0.25, 0.0, 1.0]);
    let dark = shoot(&surface, glowing.clone(), 0.0, 0.0);
    let bright = shoot(&surface, glowing, 1.0, 0.0);
    assert!(bright[1] > dark[1] + 20, "a surface vertex takes the ambient: {dark:?} {bright:?}");
}
