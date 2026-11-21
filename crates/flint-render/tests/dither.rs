//! Dither in display levels (#503): the composite's dither the same size on
//! screen at every brightness, so one step of it can hide banding in the
//! dark without laying a pattern over it.
//!
//! A ramp from black to a mid grey fills the frame; each test renders it
//! with the dither off and on, and reads how far the dither moved each
//! pixel, in 8-bit display levels, against how bright the pixel was.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

#![allow(clippy::disallowed_methods)] // the test's own targets and read-backs, not the renderer's, so uncounted (#1091)

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, DebugMode, PostProcessConfig, RendererConfig, SceneRenderer, Vertex};

const SIZE: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// One 8-bit display step, as a dither intensity in the display styles.
const STEP: f32 = 1.0 / 255.0;

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

    /// A ramp in X from black to `top` (linear), unlit, filling the view.
    fn bench(&self, top: f32) -> Bench {
        let mut renderer = SceneRenderer::new_headless(
            self.device,
            self.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        renderer.set_debug_mode(DebugMode::Unlit);
        // The view is about ±2.5 m across at the ramp, so it just fills it
        let half = 2.6;
        let v = |x: f32, z: f32, c: f32| Vertex {
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            color: [c, c, c, 1.0],
            uv: [0.0, 0.0],
        };
        let vertices = vec![
            v(-half, -half, 0.0),
            v(-half, half, 0.0),
            v(half, half, top),
            v(half, -half, top),
        ];
        let indices = vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2];
        renderer.load_procedural_mesh(
            self.device,
            "ramp",
            &vertices,
            &indices,
            ImportedMaterial::vertex_colored("ramp"),
        );
        let mut world = FlintWorld::new();
        let id = world.spawn("ramp").unwrap();
        let transform = toml::toml! {
            position = [0.0, 0.0, 0.0]
        };
        let model = toml::toml! {
            asset = "ramp"
            runtime = true
        };
        world
            .set_component(id, "transform", transform.into())
            .unwrap();
        world.set_component(id, "model", model.into()).unwrap();
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

/// Nothing but the dither: no bloom, SSAO, vignette.
fn bare(dither: Option<(u32, f32)>) -> PostProcessConfig {
    let mut config = PostProcessConfig {
        ssao_enabled: false,
        bloom_enabled: false,
        vignette_enabled: false,
        ..Default::default()
    };
    if let Some((style, intensity)) = dither {
        config.dither_enabled = true;
        config.dither_style = style;
        config.dither_intensity = intensity;
    }
    config
}

/// How far the dither moved pixels, in display levels, split by how bright
/// the undithered pixel was: (the most in the dark, at or under level
/// `dark`; the most above it; how many dark pixels it moved at all).
struct Moved {
    dark_most: i32,
    bright_most: i32,
    dark_moved: usize,
}

fn moved(plain: &[u8], dithered: &[u8], dark: u8) -> Moved {
    let mut m = Moved {
        dark_most: 0,
        bright_most: 0,
        dark_moved: 0,
    };
    for (p, d) in plain.chunks(4).zip(dithered.chunks(4)) {
        // The ramp is grey, so the green channel speaks for the pixel
        let shift = (i32::from(d[1]) - i32::from(p[1])).abs();
        if p[1] <= dark {
            m.dark_most = m.dark_most.max(shift);
            m.dark_moved += usize::from(shift > 0);
        } else {
            m.bright_most = m.bright_most.max(shift);
        }
    }
    m
}

/// The ramp covers the dark and the light: a sanity check on the bench.
#[test]
fn the_ramp_runs_from_black_into_the_midtones() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench(0.6);
    let plain = gpu.pixels(&mut bench, &bare(None));
    let greens: Vec<u8> = plain.chunks(4).map(|p| p[1]).collect();
    let (lo, hi) = (greens.iter().min().unwrap(), greens.iter().max().unwrap());
    assert!(*lo <= 4, "the ramp's dark end is level {lo}");
    assert!(*hi >= 150, "the ramp's light end is level {hi}");
}

/// The issue: one step's worth of dither in linear light is many levels in
/// the dark, and less than one in the light. Kept, as style 0.
#[test]
fn linear_dither_is_many_levels_in_the_dark() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench(0.6);
    let plain = gpu.pixels(&mut bench, &bare(None));
    let linear = gpu.pixels(&mut bench, &bare(Some((0, STEP))));
    let m = moved(&plain, &linear, 24);
    assert!(
        m.dark_most >= 4,
        "linear dither of one step moved the dark by at most {} levels",
        m.dark_most
    );
}

/// Bayer in display levels: one step of intensity is half a step either
/// way, so no pixel moves more than one level, in the dark or the light,
/// and the dark does move.
#[test]
fn ordered_dither_in_display_levels_is_one_level_everywhere() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench(0.6);
    let plain = gpu.pixels(&mut bench, &bare(None));
    let ordered = gpu.pixels(&mut bench, &bare(Some((1, STEP))));
    let m = moved(&plain, &ordered, 24);
    assert!(m.dark_most <= 1, "moved the dark {} levels", m.dark_most);
    assert!(
        m.bright_most <= 1,
        "moved the light {} levels",
        m.bright_most
    );
    assert!(m.dark_moved > 0, "moved no dark pixel at all");
}

/// Triangular noise in display levels: up to a step either way, so a pixel
/// moves a level, two at most where it sat on a rounding edge.
#[test]
fn noise_dither_in_display_levels_is_a_level_or_two_everywhere() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench(0.6);
    let plain = gpu.pixels(&mut bench, &bare(None));
    let noise = gpu.pixels(&mut bench, &bare(Some((2, STEP))));
    let m = moved(&plain, &noise, 24);
    assert!(m.dark_most <= 2, "moved the dark {} levels", m.dark_most);
    assert!(
        m.bright_most <= 2,
        "moved the light {} levels",
        m.bright_most
    );
    assert!(m.dark_moved > 0, "moved no dark pixel at all");
}

/// Off is off in every style: an intensity of 0 draws the plain frame.
#[test]
fn no_intensity_is_no_dither_in_any_style() {
    let gpu = gpu_or_skip!();
    let mut bench = gpu.bench(0.6);
    let plain = gpu.pixels(&mut bench, &bare(None));
    for style in 0..=2 {
        let none = gpu.pixels(&mut bench, &bare(Some((style, 0.0))));
        assert!(
            plain == none,
            "style {style} at intensity 0 changed the frame"
        );
    }
}
