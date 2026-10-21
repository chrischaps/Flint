//! Runtime procedural meshes: create, replace and remove after init without
//! leaking GPU memory, vertex colours, and frustum culling by mesh bounds.
//! Also how an imported model's textures are sampled (the same frame rig),
//! a grass mask repainted in part after upload, and grass blades standing
//! on the blend of their ground's heights, and a mesh kept out of the
//! shadow map by `material.cast_shadows`.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use flint_ecs::FlintWorld;
use flint_import::{ImportResult, ImportedMaterial, ImportedMesh, ImportedTexture};
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

/// `material.emissive_scale` (ADR 0090) changes a mesh's glow every frame
/// with no re-upload: it multiplies the material's own emission and the
/// per-vertex kind, a parent's scale reaches its children, and at 0 a
/// pure-light vertex goes dark instead of turning back into lit surface.
#[test]
fn emissive_scale_changes_glow_after_upload() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    renderer.set_ambient([0.0; 3], [0.0; 3]);
    let mut world = FlintWorld::new();
    let camera = Camera::default();
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let set_scale = |world: &mut FlintWorld, name: &str, scale: Option<f64>| {
        let id = world.get_id(name).unwrap();
        let mut material = toml::Table::new();
        if let Some(s) = scale {
            material.insert("emissive_scale".into(), s.into());
        }
        world.set_component(id, "material", material.into()).unwrap();
    };

    // Per vertex, uploaded once: green light at radiance 0.25.
    let (light, indices) = square(20.0, [0.0, 0.25, 0.0, 0.0]);
    let glowing = ImportedMaterial {
        roughness: 1.0,
        metallic: 1.0,
        vertex_emission: true,
        ..ImportedMaterial::vertex_colored("windows")
    };
    renderer.load_procedural_mesh(&gpu.device, "windows", &light, &indices, glowing);
    spawn(&mut world, "windows", "windows", [0.0; 3]);
    gpu.frame(&mut renderer, &world, &camera);
    let one = gpu.centre_pixel();
    assert!(one[1] > 100 && one[0] < 30, "unscaled light: {one:?}");

    set_scale(&mut world, "windows", Some(1.0));
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(one, gpu.centre_pixel()), "a scale of 1 is the upload");

    set_scale(&mut world, "windows", Some(2.0));
    gpu.frame(&mut renderer, &world, &camera);
    let two = gpu.centre_pixel();
    assert!(two[1] > one[1] + 30, "a scale of 2 is brighter: {one:?} vs {two:?}");

    set_scale(&mut world, "windows", Some(0.0));
    gpu.frame(&mut renderer, &world, &camera);
    let [r, g, b, _] = gpu.centre_pixel();
    assert!(r < 10 && g < 10 && b < 10, "a scale of 0 is dark glass: {r} {g} {b}");

    set_scale(&mut world, "windows", None);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(one, gpu.centre_pixel()), "no scale is the upload again");

    // A lit surface vertex keeps its light at a scale of 0.
    renderer.set_ambient([1.0; 3], [1.0; 3]);
    let (surface, _) = square(20.0, [0.0, 0.25, 0.0, 1.0]);
    let surface_mat = ImportedMaterial {
        roughness: 1.0,
        metallic: 0.0,
        vertex_emission: true,
        ..ImportedMaterial::vertex_colored("windows")
    };
    renderer.load_procedural_mesh(&gpu.device, "windows", &surface, &indices, surface_mat);
    gpu.frame(&mut renderer, &world, &camera);
    let lit = gpu.centre_pixel();
    set_scale(&mut world, "windows", Some(0.0));
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(lit, gpu.centre_pixel()), "surface at scale 0: {lit:?}");
    assert!(lit[1] > 20, "the surface vertex takes the ambient: {lit:?}");
    renderer.set_ambient([0.0; 3], [0.0; 3]);
    world.despawn(world.get_id("windows").unwrap()).unwrap();

    // The material's own emission, scaled from the parent.
    let lamp = ImportedMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        roughness: 1.0,
        metallic: 1.0,
        emissive: [0.0, 0.25, 0.0],
        ..ImportedMaterial::default()
    };
    renderer.load_procedural_mesh(&gpu.device, "lamp", &light, &indices, lamp);
    let parent = world.spawn("lamp_post").unwrap();
    spawn(&mut world, "lamp", "lamp", [0.0; 3]);
    let child = world.get_id("lamp").unwrap();
    world.set_parent(child, parent).unwrap();
    gpu.frame(&mut renderer, &world, &camera);
    let own = gpu.centre_pixel();
    assert!(close(one, own), "material emission matches per vertex: {own:?}");
    set_scale(&mut world, "lamp_post", Some(2.0));
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(two, gpu.centre_pixel()), "the parent's scale reaches the child");
    set_scale(&mut world, "lamp", Some(0.0));
    gpu.frame(&mut renderer, &world, &camera);
    let [r, g, b, _] = gpu.centre_pixel();
    assert!(r < 10 && g < 10 && b < 10, "the child's own scale wins: {r} {g} {b}");
}

/// `material.glow_surface` (ADR 0096) lights a per-vertex light part as
/// surface too, every frame, without taking any of its light: with
/// `emissive_scale` it slides a face uploaded once as light from lit paint
/// (1, 0) to light only (0, 1), and the parent's value reaches a child.
#[test]
fn glow_surface_slides_light_to_surface_after_upload() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    renderer.set_ambient([0.0; 3], [0.0; 3]);
    let mut world = FlintWorld::new();
    let camera = Camera::default();
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let set = |world: &mut FlintWorld, name: &str, fields: &[(&str, f64)]| {
        let id = world.get_id(name).unwrap();
        let mut material = toml::Table::new();
        for (k, v) in fields {
            material.insert((*k).into(), (*v).into());
        }
        world.set_component(id, "material", material.into()).unwrap();
    };
    let material = |name: &str| ImportedMaterial {
        roughness: 1.0,
        metallic: 0.0,
        vertex_emission: true,
        ..ImportedMaterial::vertex_colored(name)
    };

    // The same colour twice: once as lit surface, once as light.
    let (paint, indices) = square(20.0, [0.0, 0.25, 0.0, 1.0]);
    let (light, _) = square(20.0, [0.0, 0.25, 0.0, 0.0]);
    renderer.load_procedural_mesh(&gpu.device, "paint", &paint, &indices, material("paint"));
    renderer.load_procedural_mesh(&gpu.device, "gold", &light, &indices, material("gold"));
    // A sun that's off, so the default scene lights stay out: the only
    // light is the ambient each step sets.
    let sun = world.spawn("sun").unwrap();
    let mut off = toml::Table::new();
    off.insert("type".into(), "directional".into());
    off.insert("intensity".into(), 0.0.into());
    world.set_component(sun, "light", off.into()).unwrap();
    let parent = world.spawn("plot").unwrap();
    spawn(&mut world, "gold", "gold", [0.0; 3]);
    world.set_parent(world.get_id("gold").unwrap(), parent).unwrap();

    // In the dark it's its light, whatever its surface share.
    gpu.frame(&mut renderer, &world, &camera);
    let glow = gpu.centre_pixel();
    assert!(glow[1] > 100 && glow[0] < 30, "light: {glow:?}");
    set(&mut world, "gold", &[("glow_surface", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(glow, gpu.centre_pixel()), "glow_surface takes no light away");

    // Under the sky: none of it is paint at 0, all of it at 1.
    renderer.set_ambient([1.0; 3], [1.0; 3]);
    set(&mut world, "gold", &[("emissive_scale", 0.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    let [r, g, b, _] = gpu.centre_pixel();
    assert!(r < 10 && g < 10 && b < 10, "light at scale 0 is dark: {r} {g} {b}");
    set(&mut world, "gold", &[("emissive_scale", 0.0), ("glow_surface", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    let lit = gpu.centre_pixel();
    set(&mut world, "gold", &[("emissive_scale", 0.0), ("glow_surface", 0.5)]);
    gpu.frame(&mut renderer, &world, &camera);
    let half = gpu.centre_pixel();
    set(&mut world, "gold", &[("glow_surface", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    let both = gpu.centre_pixel();
    assert!(
        both[1] > lit[1] + 5 && both[1] > glow[1] + 5,
        "paint and light together: {both:?}, paint {lit:?}, light {glow:?}"
    );

    // From the parent, as from the entity itself.
    set(&mut world, "gold", &[]);
    set(&mut world, "plot", &[("glow_surface", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(both, gpu.centre_pixel()), "the parent's glow_surface reaches the child");

    // Paint at 1 is exactly a surface vertex of the same colour.
    world.despawn(world.get_id("gold").unwrap()).unwrap();
    spawn(&mut world, "paint", "paint", [0.0; 3]);
    gpu.frame(&mut renderer, &world, &camera);
    let painted = gpu.centre_pixel();
    assert!(painted[1] > 20, "the paint takes the ambient: {painted:?}");
    assert!(close(lit, painted), "glow_surface 1 is paint: {lit:?} vs {painted:?}");
    assert!(
        half[1] > 5 && half[1] + 5 < painted[1],
        "0.5 is between: {half:?} vs {painted:?}"
    );
    // A surface vertex is surface already: glow_surface leaves it alone.
    set(&mut world, "paint", &[("glow_surface", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(painted, gpu.centre_pixel()), "surface vertices don't change");
}

/// `material.vertex_tint_*` (ADR 0102) multiplies a mesh's vertex colours
/// every frame, its per-vertex light and its lit surface alike: white
/// light uploaded once and tinted green draws as green light uploaded, and
/// as green paint under `glow_surface`. The parent's tint reaches a child,
/// the child's own wins, and a tint missing a channel is no tint.
#[test]
fn vertex_tint_colours_light_and_surface_after_upload() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    renderer.set_ambient([0.0; 3], [0.0; 3]);
    let mut world = FlintWorld::new();
    let camera = Camera::default();
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let set = |world: &mut FlintWorld, name: &str, fields: &[(&str, f64)]| {
        let id = world.get_id(name).unwrap();
        let mut material = toml::Table::new();
        for (k, v) in fields {
            material.insert((*k).into(), (*v).into());
        }
        world.set_component(id, "material", material.into()).unwrap();
    };
    let green = [("vertex_tint_r", 0.0), ("vertex_tint_g", 1.0), ("vertex_tint_b", 0.0)];
    let material = |name: &str| ImportedMaterial {
        roughness: 1.0,
        metallic: 0.0,
        vertex_emission: true,
        ..ImportedMaterial::vertex_colored(name)
    };
    let (white, indices) = square(20.0, [0.25, 0.25, 0.25, 0.0]);
    let (light, _) = square(20.0, [0.0, 0.25, 0.0, 0.0]);
    let (paint, _) = square(20.0, [0.0, 0.25, 0.0, 1.0]);
    renderer.load_procedural_mesh(&gpu.device, "lamp", &white, &indices, material("lamp"));
    renderer.load_procedural_mesh(&gpu.device, "light", &light, &indices, material("light"));
    renderer.load_procedural_mesh(&gpu.device, "paint", &paint, &indices, material("paint"));
    // The sun off, as in the test above: the ambient is the only light.
    let sun = world.spawn("sun").unwrap();
    let mut off = toml::Table::new();
    off.insert("type".into(), "directional".into());
    off.insert("intensity".into(), 0.0.into());
    world.set_component(sun, "light", off.into()).unwrap();
    let shoot = |renderer: &mut SceneRenderer, world: &mut FlintWorld, mesh: &str| {
        spawn(world, "shot", mesh, [0.0; 3]);
        gpu.frame(renderer, world, &camera);
        world.despawn(world.get_id("shot").unwrap()).unwrap();
        gpu.centre_pixel()
    };

    // The references: green light, and green paint under the sky.
    let green_light = shoot(&mut renderer, &mut world, "light");
    assert!(green_light[1] > 100 && green_light[0] < 30, "light: {green_light:?}");
    renderer.set_ambient([1.0; 3], [1.0; 3]);
    let green_paint = shoot(&mut renderer, &mut world, "paint");
    assert!(green_paint[1] > 20 && green_paint[0] < 10, "paint: {green_paint:?}");
    renderer.set_ambient([0.0; 3], [0.0; 3]);

    let parent = world.spawn("window").unwrap();
    spawn(&mut world, "lamp", "lamp", [0.0; 3]);
    world.set_parent(world.get_id("lamp").unwrap(), parent).unwrap();
    gpu.frame(&mut renderer, &world, &camera);
    let grey = gpu.centre_pixel();
    assert!(grey[0] > 100 && grey[1] > 100, "untinted, it's white light: {grey:?}");

    // Tinted, the white light is the green light.
    set(&mut world, "lamp", &green);
    gpu.frame(&mut renderer, &world, &camera);
    let tinted = gpu.centre_pixel();
    assert!(close(green_light, tinted), "tinted light: {tinted:?} vs {green_light:?}");

    // And as paint, the green paint.
    renderer.set_ambient([1.0; 3], [1.0; 3]);
    let painted = [green.as_slice(), &[("glow_surface", 1.0), ("emissive_scale", 0.0)]].concat();
    set(&mut world, "lamp", &painted);
    gpu.frame(&mut renderer, &world, &camera);
    let seen = gpu.centre_pixel();
    assert!(close(green_paint, seen), "tinted paint: {seen:?} vs {green_paint:?}");
    renderer.set_ambient([0.0; 3], [0.0; 3]);

    // From the parent; the child's own tint wins.
    set(&mut world, "lamp", &[]);
    set(&mut world, "window", &green);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(green_light, gpu.centre_pixel()), "the parent's tint reaches the child");
    set(&mut world, "lamp", &[("vertex_tint_r", 1.0), ("vertex_tint_g", 1.0), ("vertex_tint_b", 1.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(grey, gpu.centre_pixel()), "the child's own tint wins");

    // A tint missing a channel is none: the parent's, then white.
    set(&mut world, "lamp", &[("vertex_tint_r", 0.0)]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(green_light, gpu.centre_pixel()), "half a tint falls to the parent's");
    set(&mut world, "window", &[]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(grey, gpu.centre_pixel()), "and with none, white");
}

/// An imported model's data maps are sampled as stored, whatever they're
/// called (a glTF texture is often just `texture_<i>`): a metallic-roughness
/// texel of 0.5 shades like factors of 0.5, not the 0.21 an sRGB decode
/// makes of it. Colour maps stay sRGB, and a texture used both ways gets a
/// copy of each.
#[test]
fn model_data_maps_are_linear() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    let mut world = FlintWorld::new();
    spawn(&mut world, "plate", "plate", [0.0; 3]);
    let sun = world.spawn("sun").unwrap();
    let mut light = toml::Table::new();
    light.insert("type".into(), "directional".into());
    light.insert("intensity".into(), 3.0.into());
    light.insert(
        "direction".into(),
        toml::Value::Array(vec![0.3.into(), 1.0.into(), 0.2.into()]),
    );
    world.set_component(sun, "light", light.into()).unwrap();
    renderer.set_ambient([0.3; 3], [0.3; 3]);
    let camera = Camera::default();

    let (vertices, indices) = square(20.0, [1.0; 4]);
    let mesh = ImportedMesh {
        name: "plate".into(),
        positions: vertices.iter().map(|v| v.position).collect(),
        normals: vertices.iter().map(|v| v.normal).collect(),
        uvs: vertices.iter().map(|v| v.uv).collect(),
        indices: indices.clone(),
        material_index: Some(0),
        joint_indices: None,
        joint_weights: None,
        skin_index: None,
    };
    let texel = |name: &str, rgba: [u8; 4]| ImportedTexture {
        name: name.into(),
        width: 1,
        height: 1,
        format: "rgba8".into(),
        data: rgba.to_vec(),
    };
    let surface = ImportedMaterial {
        base_color: [0.8, 0.3, 0.2, 1.0],
        ..ImportedMaterial::default()
    };
    // glTF packs roughness in green and metallic in blue; the factors scale them.
    let mapped = ImportedMaterial {
        metallic: 1.0,
        roughness: 1.0,
        metallic_roughness_texture: Some("texture_2".into()),
        normal_texture: Some("texture_1".into()),
        base_color_texture: Some("texture_0".into()),
        ..surface.clone()
    };
    // A second material reads the colour map as its metallic-roughness map.
    let odd = ImportedMaterial {
        metallic_roughness_texture: Some("texture_0".into()),
        base_color_texture: None,
        normal_texture: None,
        ..mapped.clone()
    };
    let model = ImportResult {
        asset_meta: flint_asset::AssetMeta {
            name: "plate".into(),
            asset_type: flint_asset::AssetType::Mesh,
            hash: String::new(),
            source_path: None,
            format: None,
            properties: Default::default(),
            tags: Vec::new(),
        },
        meshes: vec![mesh],
        textures: vec![
            texel("texture_0", [255; 4]),
            texel("texture_1", [128, 128, 255, 255]),
            texel("texture_2", [0, 128, 128, 255]),
        ],
        materials: vec![mapped, odd],
        skeletons: Vec::new(),
        skeletal_clips: Vec::new(),
        node_clips: Vec::new(),
        nodes: Vec::new(),
        root_nodes: Vec::new(),
    };
    renderer.load_model(&gpu.device, &gpu.queue, "plate", &model);

    let format = |name: &str| {
        let cache = renderer.texture_cache().unwrap();
        cache.get(name).map(|t| t.texture.format())
    };
    let (srgb, linear) = (
        Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        Some(wgpu::TextureFormat::Rgba8Unorm),
    );
    assert_eq!(format("plate::texture_0"), srgb, "the base-colour map");
    assert_eq!(format("plate::texture_1"), linear, "the normal map");
    assert_eq!(format("plate::texture_2"), linear, "the metallic-roughness map");
    assert_eq!(format("plate::texture_0#linear"), linear, "a colour map read as data");

    gpu.frame(&mut renderer, &world, &camera);
    let seen = gpu.centre_pixel();
    let mut shoot = |metallic: f32, roughness: f32| {
        let material = ImportedMaterial {
            metallic,
            roughness,
            ..surface.clone()
        };
        renderer.load_procedural_mesh(&gpu.device, "plate", &vertices, &indices, material);
        gpu.frame(&mut renderer, &world, &camera);
        gpu.centre_pixel()
    };
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let want = shoot(0.5, 0.5);
    assert!(close(seen, want), "the map shades unlike factors of 0.5: {seen:?} vs {want:?}");
    // And the test can tell: an sRGB decode would shade like 0.21.
    let srgb_read = shoot(0.216, 0.216);
    assert!(!close(want, srgb_read), "0.5 and 0.21 look alike: {want:?} vs {srgb_read:?}");
}

#[test]
fn part_of_a_grass_mask_repaints_in_place() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    let world = FlintWorld::new();
    let camera = Camera::default();
    if !renderer.has_grass_pipeline() {
        eprintln!("no compute grass on this adapter; skipping");
        return;
    }
    // Nothing to repaint before a ground is set.
    assert!(renderer
        .update_grass_mask(&gpu.queue, 0, 0, 1, 1, &[255])
        .is_err());

    // A bare 20 m square round the origin, which the default camera
    // (at (10, 10, 10)) looks down on.
    const N: u32 = 64;
    let bare = vec![0u8; (N * N) as usize];
    let config = flint_terrain::GrassConfig {
        enabled: true,
        density: 20.0,
        max_distance: 60.0,
        ..flint_terrain::GrassConfig::default()
    };
    let ground = flint_render::GrassGround {
        origin: [-10.0, -10.0],
        size: [20.0, 20.0],
        mask: &bare,
        mask_width: N,
        mask_height: N,
        scale_by_mask: false,
        height: flint_render::GrassHeight::Flat(0.0),
    };
    renderer
        .set_grass_ground(&gpu.device, &gpu.queue, &config, &ground)
        .unwrap();
    // The blade count is read back a frame or two late.
    let blades = |renderer: &mut SceneRenderer| {
        for _ in 0..4 {
            gpu.frame(renderer, &world, &camera);
        }
        renderer.collect_stats().grass_instances
    };
    assert_eq!(blades(&mut renderer), 0, "a bare mask grows nothing");

    // Fill the middle quarter: grass grows there, with the same buffers.
    let full = vec![255u8; (N / 2 * N / 2) as usize];
    renderer
        .update_grass_mask(&gpu.queue, N / 4, N / 4, N / 2, N / 2, &full)
        .unwrap();
    let grown = blades(&mut renderer);
    assert!(grown > 0, "the repainted patch grows grass");

    // Clear a corner of it again: fewer blades, but some.
    let clear = vec![0u8; (N / 4 * N / 4) as usize];
    renderer
        .update_grass_mask(&gpu.queue, N / 4, N / 4, N / 4, N / 4, &clear)
        .unwrap();
    let fewer = blades(&mut renderer);
    assert!(fewer > 0 && fewer < grown, "{fewer} of {grown} blades left");

    // A patch off the mask's edge, or of the wrong length, is refused.
    assert!(renderer
        .update_grass_mask(&gpu.queue, N - 1, 0, 2, 1, &[0, 0])
        .is_err());
    assert!(renderer
        .update_grass_mask(&gpu.queue, 0, 0, 2, 2, &[0; 3])
        .is_err());

    // Once the grass is gone, so is the mask.
    renderer.unload_grass();
    assert!(renderer
        .update_grass_mask(&gpu.queue, 0, 0, 1, 1, &[255])
        .is_err());
}

/// A tilted plane, world `y` at (`x`, `z`): a blend of four heights on it
/// lies on it, while the nearest one misses by up to half a texel's rise.
fn slope(x: f32, z: f32) -> f32 {
    0.3 * x + 0.2 * z
}

/// One frame of grass over the 20 m square round the origin, which the
/// default camera looks down on, and the blades it grew.
fn grown_blades(gpu: &Gpu, renderer: &mut SceneRenderer) -> Vec<flint_render::GrassInstanceGpu> {
    gpu.frame(renderer, &FlintWorld::new(), &Camera::default());
    let blades = renderer.read_grass_blades(&gpu.device, &gpu.queue);
    assert!(blades.len() > 500, "only {} blades grew", blades.len());
    blades
}

fn grass_config() -> flint_terrain::GrassConfig {
    flint_terrain::GrassConfig {
        enabled: true,
        density: 20.0,
        max_distance: 60.0,
        ..flint_terrain::GrassConfig::default()
    }
}

#[test]
fn grass_stands_on_the_blend_of_a_ground_maps_heights() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    if !renderer.has_grass_pipeline() {
        eprintln!("no compute grass on this adapter; skipping");
        return;
    }
    // Eight texels a side, 2.5 m each, every height taken at the middle of
    // its cell, as `GrassHeight::Map` says.
    const N: u32 = 8;
    const CELL: f32 = 20.0 / N as f32;
    let centre = |i: u32| -10.0 + (i as f32 + 0.5) * CELL;
    let heights: Vec<f32> = (0..N)
        .flat_map(|j| (0..N).map(move |i| slope(centre(i), centre(j))))
        .collect();
    let ground = flint_render::GrassGround {
        origin: [-10.0, -10.0],
        size: [20.0, 20.0],
        mask: &[255],
        mask_width: 1,
        mask_height: 1,
        scale_by_mask: false,
        height: flint_render::GrassHeight::Map {
            heights: &heights,
            width: N,
            depth: N,
        },
    };
    renderer
        .set_grass_ground(&gpu.device, &gpu.queue, &grass_config(), &ground)
        .unwrap();

    // On the plane between the outermost cells' middles, and level past them.
    let inner = 10.0 - CELL / 2.0;
    for blade in grown_blades(&gpu, &mut renderer) {
        let [x, y, z] = blade.position;
        let want = slope(x.clamp(-inner, inner), z.clamp(-inner, inner));
        assert!(
            (y - want).abs() < 1e-3,
            "a blade at ({x}, {z}) stands at {y}, the ground is at {want}"
        );
    }
}

#[test]
fn terrain_grass_stands_where_the_terrain_mesh_does() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    if !renderer.has_grass_pipeline() {
        eprintln!("no compute grass on this adapter; skipping");
        return;
    }
    // A terrain's samples sit on its corners: nine a side over 20 m, the
    // first on the near edge and the last on the far one, as
    // `Heightmap::sample` reads them. They're scaled by `height_scale` and
    // raised by the offset's `y`.
    const N: u32 = 9;
    const SCALE: f32 = 4.0;
    const RAISE: f32 = 1.5;
    let corner = |i: u32| -10.0 + i as f32 * 20.0 / (N - 1) as f32;
    let heights: Vec<f32> = (0..N)
        .flat_map(|j| (0..N).map(move |i| slope(corner(i), corner(j)) / SCALE))
        .collect();
    renderer.load_grass(
        &gpu.device,
        &gpu.queue,
        &grass_config(),
        &heights,
        N,
        N,
        &[255, 0, 0, 0],
        1,
        1,
        [-10.0, RAISE, -10.0],
        20.0,
        20.0,
        SCALE,
    );

    for blade in grown_blades(&gpu, &mut renderer) {
        let [x, y, z] = blade.position;
        let want = RAISE + slope(x, z);
        assert!(
            (y - want).abs() < 1e-3,
            "a blade at ({x}, {z}) stands at {y}, the terrain is at {want}"
        );
    }
}

/// `material.cast_shadows = false` (ADR 0100) keeps a mesh out of the
/// shadow cascades. A sheet that's all but clear, hung over lit ground,
/// shadows it as if it were solid until it says it doesn't cast, on its own
/// material or its parent's.
#[test]
fn cast_shadows_false_keeps_a_mesh_out_of_the_shadow_map() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    renderer.set_tonemapping(false);
    renderer.set_ambient([0.0; 3], [0.0; 3]);
    let mut world = FlintWorld::new();
    let camera = Camera::default();
    let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let set = |world: &mut FlintWorld, name: &str, fields: &[(&str, toml::Value)]| {
        let id = world.get_id(name).unwrap();
        let mut material = toml::Table::new();
        for (k, v) in fields {
            material.insert((*k).into(), v.clone());
        }
        world.set_component(id, "material", material.into()).unwrap();
    };
    let material = |name: &str| ImportedMaterial {
        roughness: 1.0,
        metallic: 0.0,
        ..ImportedMaterial::vertex_colored(name)
    };

    // Grey ground under a slanting sun (its direction points at it), and a
    // sheet 2 m up whose shadow falls on the middle of the view, while the
    // camera sees past the sheet's side to the ground there.
    let (ground, indices) = square(20.0, [0.5, 0.5, 0.5, 1.0]);
    let (sheet, _) = square(1.5, [0.5, 0.5, 0.5, 1.0]);
    renderer.load_procedural_mesh(&gpu.device, "ground", &ground, &indices, material("ground"));
    renderer.load_procedural_mesh(&gpu.device, "sheet", &sheet, &indices, material("sheet"));
    let sun = world.spawn("sun").unwrap();
    let mut light = toml::Table::new();
    light.insert("type".into(), "directional".into());
    light.insert("intensity".into(), 3.0.into());
    light.insert(
        "direction".into(),
        toml::Value::Array(vec![0.5.into(), 1.0.into(), 0.0.into()]),
    );
    world.set_component(sun, "light", light.into()).unwrap();
    spawn(&mut world, "ground", "ground", [0.0; 3]);
    gpu.frame(&mut renderer, &world, &camera);
    let lit = gpu.centre_pixel();
    assert!(lit[0] > 40, "the ground is lit: {lit:?}");

    // 2% opaque, it still casts a solid shadow by default. Its parent is a
    // bare group, with no transform, there only to carry `cast_shadows`.
    let parent = world.spawn("mist").unwrap();
    spawn(&mut world, "sheet", "sheet", [1.0, 2.0, 0.0]);
    world.set_parent(world.get_id("sheet").unwrap(), parent).unwrap();
    set(&mut world, "sheet", &[("opacity", 0.02.into())]);
    gpu.frame(&mut renderer, &world, &camera);
    let shaded = gpu.centre_pixel();
    assert!(shaded[0] + 30 < lit[0], "a clear sheet casts: {shaded:?} vs {lit:?}");

    set(&mut world, "sheet", &[("opacity", 0.02.into()), ("cast_shadows", false.into())]);
    gpu.frame(&mut renderer, &world, &camera);
    let seen = gpu.centre_pixel();
    assert!(close(lit, seen), "cast_shadows = false: {seen:?} vs {lit:?}");

    // Opaque too, and from the parent.
    set(&mut world, "sheet", &[]);
    gpu.frame(&mut renderer, &world, &camera);
    let solid = gpu.centre_pixel();
    set(&mut world, "mist", &[("cast_shadows", false.into())]);
    gpu.frame(&mut renderer, &world, &camera);
    let seen = gpu.centre_pixel();
    assert!(seen[0] > solid[0] + 30, "the parent's cast_shadows: {seen:?} vs {solid:?}");
    set(&mut world, "sheet", &[("cast_shadows", true.into())]);
    gpu.frame(&mut renderer, &world, &camera);
    assert!(close(solid, gpu.centre_pixel()), "the child's own value wins");
}
