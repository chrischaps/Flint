//! A frame's description (#1084, ADR 0126): the passes in the order the
//! frame recorded them, named as the GPU timer names them, each scene draw
//! by its entity, the culled with their reason, and skipped passes with
//! theirs. Asking for none keeps nothing and changes no pixel.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::frame_capture::FrameDescription;
use flint_render::{Camera, FrameHost, FrameInputs, RendererConfig, SceneRenderer, Vertex};

const SIZE: u32 = 64;
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
        device: &shared.device,
        queue: &shared.queue,
        _lease: lease,
    })
}

macro_rules! gpu_or_skip {
    () => {
        match gpu() {
            Some(g) => g,
            None => {
                flint_render::gpu_select::skip_without_gpu("no GPU adapter");
                return;
            }
        }
    };
}

impl Gpu {
    fn renderer(&self) -> SceneRenderer {
        let mut renderer = SceneRenderer::new_headless(
            self.device,
            self.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let (vertices, indices) = square(1.0);
        renderer.load_procedural_mesh(
            self.device,
            "tile",
            &vertices,
            &indices,
            ImportedMaterial::vertex_colored("tile"),
        );
        renderer
    }

    /// One frame as a host draws it, timed when the device can time,
    /// then read back.
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        renderer.prepare(self.device, self.queue, world, &FrameInputs::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        renderer.encode_to(
            self.device,
            self.queue,
            &mut encoder,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        renderer.encode_gpu_timings(self.device, &mut encoder);
        self.queue.submit([encoder.finish()]);
        renderer.frame_submitted();
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

/// A flat, double-sided square of half-width `half`.
fn square(half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        color: [0.8, 0.7, 0.5, 1.0],
        uv: [0.0, 0.0],
    };
    let vertices = vec![
        v(-half, -half),
        v(-half, half),
        v(half, half),
        v(half, -half),
    ];
    (vertices, vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2])
}

fn spawn(world: &mut FlintWorld, name: &str, text: &str) {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
}

/// The fixture: the default camera at (10, 10, 10) looks at the origin.
/// A tile there and a box beside it draw; a tile behind the camera is
/// outside the frustum; one in view but past its own draw distance isn't
/// drawn; a sun casts.
fn world() -> FlintWorld {
    let mut world = FlintWorld::new();
    let tile = |x: f64, z: f64| {
        format!("[transform]\nposition = [{x}, 0, {z}]\n[model]\nasset = \"tile\"\nruntime = true")
    };
    spawn(&mut world, "in_view", &tile(0.0, 0.0));
    spawn(&mut world, "behind", &tile(60.0, 60.0));
    spawn(
        &mut world,
        "too_far",
        &format!("{}\n[lod]\ndraw_distance = 5.0", tile(1.5, -1.5)),
    );
    spawn(
        &mut world,
        "crate",
        "[transform]\nposition = [-2, 0.5, 0]\n[bounds]\nmin = [-0.5, -0.5, -0.5]\nmax = [0.5, 0.5, 0.5]",
    );
    spawn(
        &mut world,
        "sun",
        "[light]\ntype = \"directional\"\ndirection = [0.3, 1, 0.2]\nintensity = 2.0",
    );
    world
}

/// Draw one frame of [`world`] described, timed where the device can time,
/// its raw times laid on.
fn describe(gpu: &Gpu, renderer: &mut SceneRenderer, world: &FlintWorld) -> FrameDescription {
    let timed = renderer.set_gpu_timing(gpu.device, gpu.queue, true);
    if let Some(timer) = renderer.gpu_timer_mut() {
        timer.record(true);
    }
    renderer.describe_next_frame();
    gpu.frame(renderer, world);
    let mut d = renderer
        .describe_frame(Some(world), &FrameHost::default())
        .expect("the frame was described");
    if timed {
        let timer = renderer.gpu_timer_mut().unwrap();
        timer.flush(gpu.device);
        let t = timer.recorded().last().cloned().expect("a frame read back");
        // The description's timed passes are the timer's scopes, in order
        let timed: Vec<String> = d
            .all_passes()
            .into_iter()
            .filter(|p| p.timed)
            .map(|p| p.name.clone())
            .collect();
        let scopes: Vec<&str> = t.passes.iter().map(|p| p.label).collect();
        assert_eq!(timed, scopes, "described passes and GPU rows differ");
        d.lay_gpu_times(&t);
    }
    d
}

#[test]
fn a_frame_is_described_pass_by_pass_and_draw_by_draw() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    let world = world();
    let d = describe(&gpu, &mut renderer, &world);
    assert_eq!((d.format.as_str(), d.version), ("flintframe", 1));

    // The passes in the order the frame recorded them: what ran, and what
    // didn't with its reason
    let order: Vec<&str> = d.passes.iter().map(|p| p.name.as_str()).collect();
    let at = |name: &str| {
        order.iter().position(|n| *n == name).unwrap_or_else(|| {
            panic!("no {name} pass in {order:?}");
        })
    };
    assert!(at("Grass") < at("Shadows") && at("Shadows") < at("Scene"));
    assert!(at("Scene") < at("SSAO") && at("SSAO") < at("Composite"));
    let grass = d.pass("Grass").unwrap();
    assert!(!grass.ran);
    assert_eq!(grass.skipped.as_deref(), Some("no grass in the scene"));
    let volumetric = d.pass("Volumetric").unwrap();
    assert!(!volumetric.ran && volumetric.skipped.is_some());
    assert!(d.pass("Composite").unwrap().ran);

    // The scene pass: its draws are the frame's draw calls, each named by
    // its entity, and the culled beside them with why
    let scene = d.pass("Scene").unwrap();
    assert_eq!(scene.draws.len() as u32, d.stats.draw_calls);
    let drawn: Vec<&str> = scene
        .draws
        .iter()
        .map(|dr| dr.entity.as_deref().expect("a draw names its entity"))
        .collect();
    // In the list's order: a layer at a time, each by entity name
    assert_eq!(drawn, ["crate", "in_view"]);
    let tile = &scene.draws[1];
    assert_eq!(tile.mesh.as_deref(), Some("tile#0"));
    assert_eq!(
        (tile.kind.as_str(), tile.layer.as_str()),
        ("Static", "Opaque static")
    );
    assert_eq!(tile.triangles, 4);
    assert_eq!(scene.draws[0].mesh.as_deref(), Some("box"));
    let culled: Vec<(&str, &str)> = scene
        .culled
        .iter()
        .map(|c| (c.entity.as_deref().unwrap(), c.culled.as_deref().unwrap()))
        .collect();
    assert_eq!(
        culled,
        [("behind", "frustum"), ("too_far", "draw distance")]
    );
    assert_eq!(scene.culled.len() as u32, d.stats.culled_draws);

    // The sun's cascades are passes inside Shadows, their casters named
    let shadows = d.pass("Shadows").unwrap();
    assert!(shadows.ran);
    assert!(!shadows.passes.is_empty());
    assert_eq!(shadows.passes[0].name, "Shadow cascade 0");
    assert!(shadows.passes[0]
        .draws
        .iter()
        .any(|dr| dr.entity.as_deref() == Some("in_view")));
    // Past its draw distance it casts nothing either
    assert!(!shadows
        .passes
        .iter()
        .flat_map(|p| &p.draws)
        .any(|dr| dr.entity.as_deref() == Some("too_far")));

    // The frame's settings, targets and lights
    assert_eq!(d.frame.size, [SIZE, SIZE]);
    assert!(d.targets.iter().any(|t| t.id == "hdr_scene"));
    assert!(d.targets.iter().any(|t| t.id == "shadow_map"));
    assert_eq!(d.lights.kept.len(), 1);
    assert_eq!(d.lights.kept[0].entity.as_deref(), Some("sun"));

    // It reads back as the same description
    let json = serde_json::to_string(&d).unwrap();
    let back: FrameDescription = serde_json::from_str(&json).unwrap();
    assert_eq!(back.pass("Scene").unwrap().draws, scene.draws);
}

#[test]
fn a_frame_nobody_asked_about_keeps_no_log_and_draws_the_same() {
    let gpu = gpu_or_skip!();
    let world = world();
    let mut plain = gpu.renderer();
    // Two frames, so the second's history and caches are a running frame's
    gpu.frame(&mut plain, &world);
    let before = gpu.frame(&mut plain, &world);
    assert!(!plain.describing_frame());
    assert!(plain
        .describe_frame(Some(&world), &FrameHost::default())
        .is_none());

    let mut described = gpu.renderer();
    gpu.frame(&mut described, &world);
    described.describe_next_frame();
    let during = gpu.frame(&mut described, &world);
    assert_eq!(during, before, "describing the frame changed its pixels");
    assert!(described
        .describe_frame(Some(&world), &FrameHost::default())
        .is_some());
    // Taken: the next frame is described only if asked again
    assert!(!described.describing_frame());
    gpu.frame(&mut described, &world);
    assert!(described
        .describe_frame(Some(&world), &FrameHost::default())
        .is_none());
}
