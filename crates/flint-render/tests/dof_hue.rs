//! Depth of field keeps a surface's hue (#1156): an emissive orange strip
//! seen out of focus spreads as orange, never green.
//!
//! The composite blurred green through depth of field (and the radial blur)
//! but, while chromatic aberration was on, read red and blue from the sharp
//! picture at its offsets. Wherever the blur spread a bright surface, green
//! spread with it and red and blue didn't, so emissive edges rang green.
//! Kindling's strike left the aberration at 0.0033 (a decay that never
//! reaches its 0.001 cut), far too small to see, and every later shot with
//! depth of field showed the ring.
//!
//! The bench: an unlit orange strip, brighter than white, a metre above a
//! black ground, the camera looking straight down, everything out of
//! focus. Orange over black blends to orange at every strength, so red is
//! at least green in every pixel of any blur of it; a pixel greener than
//! it is red is a hue the frame doesn't hold.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

#![allow(clippy::disallowed_methods)] // the test's own targets and read-backs, not the renderer's, so uncounted (#1091)

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, DebugMode, PostProcessConfig, RendererConfig, SceneRenderer, Vertex};

const SIZE: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The strip's colour: Kindling's rune emission, (1.0, 0.36, 0.08) at 2.6.
const ORANGE: [f32; 3] = [2.6, 0.936, 0.208];

struct Gpu {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    _lease: shared_gpu::Lease,
}

macro_rules! gpu_or_skip {
    () => {
        match shared_gpu::lease().map(Gpu::new) {
            Some(g) => g,
            None => {
                eprintln!("no GPU adapter; skipping");
                return;
            }
        }
    };
}

struct Bench {
    renderer: SceneRenderer,
    world: FlintWorld,
    camera: Camera,
}

impl Gpu {
    fn new(lease: shared_gpu::Lease) -> Gpu {
        let shared = lease.gpu;
        let device = &shared.device;
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

    /// The strip over the ground, unlit, seen from straight above.
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
        let quad = |name: &str, (x0, x1): (f32, f32), (z0, z1): (f32, f32), y: f32, c: [f32; 3]| {
            let v = |x: f32, z: f32| Vertex {
                position: [x, y, z],
                normal: [0.0, 1.0, 0.0],
                color: [c[0], c[1], c[2], 1.0],
                uv: [0.0, 0.0],
            };
            let vertices = vec![v(x0, z0), v(x0, z1), v(x1, z1), v(x1, z0)];
            // Both windings, so the quad shows whichever way it faces
            let indices = vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2];
            (name.to_string(), vertices, indices)
        };
        let mut world = FlintWorld::new();
        for (name, vertices, indices) in [
            quad("ground", (-6.0, 6.0), (-6.0, 6.0), 0.0, [0.0; 3]),
            // A metre up, a strip about 16 pixels wide across the frame
            quad("strip", (-1.5, 1.5), (-0.25, 0.25), 1.0, ORANGE),
        ] {
            renderer.load_procedural_mesh(
                self.device,
                &name,
                &vertices,
                &indices,
                ImportedMaterial::vertex_colored(&name),
            );
            let id = world.spawn(&name).unwrap();
            let transform = toml::toml! {
                position = [0.0, 0.0, 0.0]
            };
            let mut model = toml::map::Map::new();
            model.insert("asset".into(), toml::Value::String(name.clone()));
            model.insert("runtime".into(), toml::Value::Boolean(true));
            world
                .set_component(id, "transform", transform.into())
                .unwrap();
            world
                .set_component(id, "model", toml::Value::Table(model))
                .unwrap();
        }
        let mut camera = Camera {
            distance: 6.0,
            pitch: 89f32.to_radians(),
            yaw: 0.0,
            ..Default::default()
        };
        camera.update_orbit();
        Bench {
            renderer,
            world,
            camera,
        }
    }

    /// One frame with this config, read back as RGBA8.
    fn pixels(&self, bench: &mut Bench, config: &PostProcessConfig) -> Vec<u8> {
        bench.renderer.set_post_process_config(config.clone());
        bench.renderer.prepare(
            self.device,
            self.queue,
            &bench.world,
            &flint_render::FrameInputs::default(),
        );
        bench.renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &bench.camera,
            &self.color_view,
        );
        let _ = flint_render::wait_for_gpu(self.device);

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

/// The lens effects alone: no bloom, SSAO, vignette or dither.
fn lens(dof: f32, aberration: f32, radial_blur: f32) -> PostProcessConfig {
    PostProcessConfig {
        ssao_enabled: false,
        bloom_enabled: false,
        vignette_enabled: false,
        dither_enabled: false,
        // Focused far below the ground: the strip and the ground are both
        // fully out of focus
        dof_strength: dof,
        dof_focus_distance: 40.0,
        dof_focus_range: 3.0,
        chromatic_aberration: aberration,
        radial_blur,
        ..Default::default()
    }
}

/// How much greener than red the greenest pixel is, in display levels (0
/// when red is at least green everywhere), and how many pixels are lit.
fn greenest(pixels: &[u8]) -> (i32, usize) {
    let mut worst = 0;
    let mut lit = 0;
    for p in pixels.chunks(4) {
        worst = worst.max(i32::from(p[1]) - i32::from(p[0]));
        lit += usize::from(p[0] > 8);
    }
    (worst, lit)
}

/// A level of slack for rounding, nothing more: the bug was 100 and up.
const SLACK: i32 = 1;

/// The bench is what it says: sharp, the strip is orange on black, and
/// depth of field spreads it over more pixels.
#[test]
fn the_strip_is_orange_and_the_blur_spreads_it() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let sharp = gpu.pixels(&mut bench, &lens(0.0, 0.0, 0.0));
    let (worst, lit_sharp) = greenest(&sharp);
    assert!(
        worst <= SLACK,
        "the sharp strip has a pixel {worst} levels greener than red"
    );
    assert!(lit_sharp > 100, "the strip lights only {lit_sharp} pixels");
    let blurred = gpu.pixels(&mut bench, &lens(0.6, 0.0, 0.0));
    let (_, lit_blurred) = greenest(&blurred);
    assert!(
        lit_blurred > lit_sharp + 50,
        "depth of field spread the strip from {lit_sharp} to only {lit_blurred} pixels"
    );
}

/// Depth of field alone never made the ring; held here so it never does.
#[test]
fn depth_of_field_keeps_the_strip_orange() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let (worst, _) = greenest(&gpu.pixels(&mut bench, &lens(0.6, 0.0, 0.0)));
    assert!(worst <= SLACK, "a pixel {worst} levels greener than red");
}

/// The issue: depth of field with an aberration too small to see (the
/// 0.0033 Kindling's strike left behind) rang the strip in green.
#[test]
fn depth_of_field_under_a_faint_aberration_keeps_the_strip_orange() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let (worst, _) = greenest(&gpu.pixels(&mut bench, &lens(0.6, 0.0033, 0.0)));
    assert!(
        worst <= SLACK,
        "a pixel {worst} levels greener than red: green was blurred and red and blue weren't"
    );
}

/// The radial blur had the same split: blurred green, sharp red and blue.
#[test]
fn the_radial_blur_under_an_aberration_keeps_the_strip_orange() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench();
    let (worst, _) = greenest(&gpu.pixels(&mut bench, &lens(0.0, 0.0033, 1.0)));
    assert!(worst <= SLACK, "a pixel {worst} levels greener than red");
}
