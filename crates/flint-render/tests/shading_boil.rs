//! The shading boil (#214): paint and ink that change with the drawing, and only
//! with the drawing.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, DebugMode, PostProcessConfig, RendererConfig, SceneRenderer, Vertex};
use flint_wooze::BoilFrame;

const SIZE: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    _lease: shared_gpu::Lease,
}

/// The shared device (#598), for this test's turn (#913).
fn gpu() -> Option<Gpu> {
    shared_gpu::lease().map(frame_for)
}

fn frame_for(lease: shared_gpu::Lease) -> Gpu {
    let shared = lease.gpu;
    let device = &shared.device;
    {
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
        Gpu {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            device,
            queue: &shared.queue,
            _lease: lease,
        }
    }
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

/// A pale square raised over a darker ground: one clean silhouette to ink.
struct Bench {
    renderer: SceneRenderer,
    world: FlintWorld,
    camera: Camera,
}

fn square(half: f32, color: [f32; 4]) -> (Vec<Vertex>, Vec<u32>) {
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        color,
        uv: [0.0, 0.0],
    };
    let vertices = vec![
        v(-half, -half),
        v(-half, half),
        v(half, half),
        v(half, -half),
    ];
    let indices = vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2];
    (vertices, indices)
}

fn spawn(world: &mut FlintWorld, name: &str, position: [f32; 3]) {
    let id = world.spawn(name).unwrap();
    let mut transform = toml::Table::new();
    transform.insert(
        "position".into(),
        toml::Value::Array(position.iter().map(|&p| (p as f64).into()).collect()),
    );
    world
        .set_component(id, "transform", transform.into())
        .unwrap();
    let mut model = toml::Table::new();
    model.insert("asset".into(), name.into());
    model.insert("runtime".into(), true.into());
    world.set_component(id, "model", model.into()).unwrap();
}

impl Gpu {
    fn bench(&self) -> Bench {
        let mut renderer = SceneRenderer::new_headless(
            self.device,
            self.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        renderer.set_debug_mode(DebugMode::Unlit);
        for (name, half, color) in [
            ("ground", 40.0, [0.25, 0.3, 0.2, 1.0]),
            ("card", 3.0, [0.8, 0.75, 0.6, 1.0]),
        ] {
            let (vertices, indices) = square(half, color);
            renderer.load_procedural_mesh(
                self.device,
                name,
                &vertices,
                &indices,
                ImportedMaterial::vertex_colored(name),
            );
        }
        let mut world = FlintWorld::new();
        spawn(&mut world, "ground", [0.0; 3]);
        spawn(&mut world, "card", [0.0, 2.0, 0.0]);
        let mut camera = Camera {
            distance: 14.0,
            pitch: 70f32.to_radians(),
            yaw: 20f32.to_radians(),
            ..Default::default()
        };
        camera.update_orbit();
        Bench {
            renderer,
            world,
            camera,
        }
    }

    /// Render one frame with this config and drawing, and read it all back.
    fn pixels(&self, bench: &mut Bench, config: &PostProcessConfig, boil: BoilFrame) -> Vec<u8> {
        bench.renderer.set_post_process_config(config.clone());
        bench
            .renderer
            .ensure_kuwahara_resources(self.device, self.queue);
        bench.renderer.set_boil(boil);
        bench.renderer.update_from_world(&bench.world, self.device);
        bench.renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &bench.camera,
            &self.color_view,
        );
        self.device.poll(wgpu::Maintain::Wait);

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

fn drawing(seed: u32, amount: f32) -> BoilFrame {
    BoilFrame {
        seed,
        amount,
        step: 0.25,
    }
}

fn bare() -> PostProcessConfig {
    PostProcessConfig {
        ssao_enabled: false,
        bloom_enabled: false,
        vignette_enabled: false,
        ..Default::default()
    }
}

/// `config` with `style`'s boil at `amount`.
fn boiling(mut config: PostProcessConfig, style: &str, amount: f32) -> PostProcessConfig {
    config.styles.set_param(style, "boil", amount);
    config
}

/// `config` with the contour style on or off.
fn contour(mut config: PostProcessConfig, on: bool) -> PostProcessConfig {
    config.styles.set_enabled("contour", on);
    config
}

/// The contour, a registered style since #599, boiling.
fn inked() -> PostProcessConfig {
    boiling(contour(bare(), true), "contour", 1.0)
}

fn painted() -> PostProcessConfig {
    let mut config = bare();
    // Kuwahara is a style since #298, and its boil one of its params (#599)
    config.styles.set_enabled("kuwahara", true);
    config.styles.set_param("kuwahara", "radius", 3.0);
    boiling(config, "kuwahara", 0.5)
}

/// Pixels darker by more than a little than the same pixel of `plain`.
fn darkened(with: &[u8], plain: &[u8]) -> usize {
    with.chunks(4)
        .zip(plain.chunks(4))
        .filter(|(a, b)| (0..3).any(|i| i32::from(b[i]) - i32::from(a[i]) > 40))
        .count()
}

#[test]
fn the_contour_inks_a_silhouette_and_leaves_flat_ground_alone() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let plain = contour(inked(), false);
    let without = gpu.pixels(&mut bench, &plain, BoilFrame::default());
    let still = boiling(inked(), "contour", 0.0);
    let with = gpu.pixels(&mut bench, &still, BoilFrame::default());

    let lines = darkened(&with, &without);
    let frame = (SIZE * SIZE) as usize;
    assert!(lines > 50, "expected a line round the card, got {lines} px");
    // The ground runs to the frame's edges at a steep angle, and draws no line.
    assert!(lines < frame / 4, "ink went everywhere: {lines} px");
    let corner = 0;
    assert_eq!(with[corner..corner + 4], without[corner..corner + 4]);
}

#[test]
fn ink_changes_with_the_drawing_and_with_nothing_else() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let config = inked();
    let first = gpu.pixels(&mut bench, &config, drawing(1, 1.0));
    let held = gpu.pixels(&mut bench, &config, drawing(1, 1.0));
    let next = gpu.pixels(&mut bench, &config, drawing(2, 1.0));
    assert_eq!(first, held, "a held drawing must repeat byte for byte");
    assert_ne!(first, next, "a new drawing must move the line");
}

#[test]
fn paint_changes_with_the_drawing_and_with_nothing_else() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let config = painted();
    let first = gpu.pixels(&mut bench, &config, drawing(1, 1.0));
    let held = gpu.pixels(&mut bench, &config, drawing(1, 1.0));
    let next = gpu.pixels(&mut bench, &config, drawing(2, 1.0));
    assert_eq!(first, held, "a held drawing must repeat byte for byte");
    assert_ne!(first, next, "a new drawing must move the paint");
}

#[test]
fn with_no_amount_the_drawing_means_nothing() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    for config in [inked(), painted()] {
        // A field whose jitter is 0, and a scene with no field at all.
        let a = gpu.pixels(&mut bench, &config, drawing(1, 0.0));
        let b = gpu.pixels(&mut bench, &config, drawing(2, 0.0));
        let none = gpu.pixels(&mut bench, &config, BoilFrame::default());
        assert_eq!(a, b);
        assert_eq!(a, none);
    }
    // Likewise a scene with a field that hasn't asked its passes to boil.
    for config in [
        boiling(inked(), "contour", 0.0),
        boiling(painted(), "kuwahara", 0.0),
    ] {
        let a = gpu.pixels(&mut bench, &config, drawing(1, 1.0));
        let b = gpu.pixels(&mut bench, &config, drawing(2, 1.0));
        assert_eq!(a, b);
    }
}

#[test]
fn the_contour_survives_a_resize() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let config = inked();
    let before = gpu.pixels(&mut bench, &config, drawing(3, 1.0));
    bench.renderer.resize_postprocess(gpu.device, SIZE, SIZE);
    // Nothing calls ensure here: the resize must have made its texture again.
    bench.renderer.set_boil(drawing(3, 1.0));
    let after = gpu.pixels(&mut bench, &config, drawing(3, 1.0));
    assert_eq!(before, after);
}

#[test]
fn turning_the_contour_off_goes_back_to_the_same_pixels() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let off = contour(inked(), false);
    let never_on = gpu.pixels(&mut bench, &off, drawing(4, 1.0));
    let on = gpu.pixels(&mut bench, &inked(), drawing(4, 1.0));
    let off_again = gpu.pixels(&mut bench, &off, drawing(4, 1.0));
    assert_ne!(never_on, on);
    assert_eq!(never_on, off_again);
}
