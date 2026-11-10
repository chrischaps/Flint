//! Runtime quality settings (FlintKart#55): MSAA, render scale, shadows and
//! post-process changed between frames on a live renderer, toggled over and
//! over, with no validation error and the frame coming back exactly.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, QualitySettings, RendererConfig, SceneRenderer, Vertex};

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

/// The shared device (#598), with no other test on it: every frame here
/// is drawn in an error scope, and wgpu 23 keeps those per device.
fn gpu() -> Option<Gpu> {
    shared_gpu::lease_alone().map(frame_for)
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
                flint_render::gpu_select::skip_without_gpu("no GPU adapter");
                return;
            }
        }
    };
}

/// A flat, double-sided square of half-width `half` at height `y`.
fn square(half: f32, y: f32, color: [f32; 4]) -> (Vec<Vertex>, Vec<u32>) {
    let v = |x: f32, z: f32| Vertex {
        position: [x, y, z],
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

fn spawn(world: &mut FlintWorld, name: &str, mesh: &str) {
    let id = world.spawn(name).unwrap();
    let mut transform = toml::Table::new();
    transform.insert(
        "position".into(),
        toml::Value::Array(vec![0.0.into(), 0.0.into(), 0.0.into()]),
    );
    world
        .set_component(id, "transform", transform.into())
        .unwrap();
    let mut model = toml::Table::new();
    model.insert("asset".into(), mesh.into());
    model.insert("runtime".into(), true.into());
    world.set_component(id, "model", model.into()).unwrap();
}

/// A ground and a slab floating over it, so the slab's shadow and edges
/// are in the frame.
fn scene(gpu: &Gpu) -> (SceneRenderer, FlintWorld) {
    let mut renderer = SceneRenderer::new_headless(
        gpu.device,
        gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let mut world = FlintWorld::new();
    for (name, (vertices, indices)) in [
        ("ground", square(6.0, 0.0, [0.8, 0.8, 0.8, 1.0])),
        ("slab", square(1.5, 2.0, [0.9, 0.3, 0.2, 1.0])),
    ] {
        renderer.load_procedural_mesh(
            gpu.device,
            name,
            &vertices,
            &indices,
            ImportedMaterial::vertex_colored(name),
        );
        spawn(&mut world, name, name);
    }
    (renderer, world)
}

impl Gpu {
    /// Render one frame inside a validation error scope and read it back.
    /// Panics on any validation error, which is what a pipeline left at
    /// the wrong sample count, or a target at the wrong size, would give.
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.update_from_world(world, self.device);
        renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        let pixels = self.read();
        if let Some(e) = pollster::block_on(self.device.pop_error_scope()) {
            panic!("validation error at {:?}: {e}", renderer.quality());
        }
        pixels
    }

    fn read(&self) -> Vec<u8> {
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
        let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            let at = (row * y) as usize;
            out.extend_from_slice(&data[at..at + (SIZE * 4) as usize]);
        }
        out
    }
}

/// Pixels whose colour differs by more than `tolerance` in any channel.
fn differing(a: &[u8], b: &[u8], tolerance: u8) -> usize {
    a.chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > tolerance)
        })
        .count()
}

/// Sum of the RGB channels: the frame's brightness.
fn brightness(a: &[u8]) -> u64 {
    a.chunks(4)
        .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum()
}

#[test]
fn toggling_every_setting_brings_the_frame_back() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let base = gpu.frame(&mut renderer, &world);
    assert!(brightness(&base) > 0, "the scene drew nothing");

    let d = QualitySettings::default();
    let settings = [
        QualitySettings { msaa: 4, ..d },
        QualitySettings {
            render_scale: 0.5,
            ..d
        },
        QualitySettings {
            msaa: 4,
            render_scale: 0.75,
            ..d
        },
        QualitySettings {
            shadows: false,
            ..d
        },
        QualitySettings {
            shadow_cascades: 1,
            shadow_resolution: 512,
            ..d
        },
        QualitySettings {
            shadow_cascades: 2,
            shadow_distance: 40.0,
            ..d
        },
        QualitySettings {
            post_process: false,
            ..d
        },
        QualitySettings {
            post_process: false,
            msaa: 4,
            ..d
        },
        QualitySettings {
            bloom: false,
            ssao: false,
            fog: false,
            volumetric: false,
            ..d
        },
        QualitySettings {
            render_scale: 0.6,
            msaa: 4,
            shadow_cascades: 1,
            shadow_resolution: 1024,
            ssao_samples: 16,
            bloom: false,
            ..d
        },
    ];
    // Round and round, a frame at each, as a menu or an auto-tuner would.
    let mut msaa_changes = Vec::new();
    for round in 0..5 {
        for q in &settings {
            let msaa_before = renderer.msaa();
            let start = std::time::Instant::now();
            let applied = renderer.set_quality(gpu.device, q);
            if msaa_before != q.msaa {
                msaa_changes.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            assert_eq!(&applied, renderer.quality(), "round {round}");
            assert_eq!(renderer.msaa(), q.msaa);
            gpu.frame(&mut renderer, &world);
        }
    }
    msaa_changes.sort_by(f64::total_cmp);
    eprintln!(
        "{} MSAA changes, rebuild ms: min {:.1} median {:.1} max {:.1}",
        msaa_changes.len(),
        msaa_changes[0],
        msaa_changes[msaa_changes.len() / 2],
        msaa_changes[msaa_changes.len() - 1]
    );

    renderer.set_quality(gpu.device, &d);
    let back = gpu.frame(&mut renderer, &world);
    assert_eq!(
        differing(&base, &back, 0),
        0,
        "after every toggle, the default settings must draw the same frame"
    );
}

#[test]
fn each_setting_changes_what_is_drawn() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let d = QualitySettings::default();
    let base = gpu.frame(&mut renderer, &world);

    let mut at = |q: QualitySettings| {
        renderer.set_quality(gpu.device, &q);
        gpu.frame(&mut renderer, &world)
    };
    let msaa = at(QualitySettings { msaa: 4, ..d });
    let half = at(QualitySettings {
        render_scale: 0.5,
        ..d
    });
    let unshadowed = at(QualitySettings {
        shadows: false,
        ..d
    });
    let one_cascade = at(QualitySettings {
        shadow_cascades: 1,
        ..d
    });
    let two_cascades = at(QualitySettings {
        shadow_cascades: 2,
        ..d
    });
    let one_near = at(QualitySettings {
        shadow_cascades: 1,
        shadow_distance: 40.0,
        ..d
    });

    let edges = differing(&base, &msaa, 8);
    eprintln!("msaa changed {edges} pixels");
    assert!(edges > 0, "4x MSAA should soften the slab's edges");
    // But only the edges: the faces are the same colour.
    assert!(
        edges < (SIZE * SIZE / 4) as usize,
        "MSAA changed {edges} pixels"
    );

    assert!(
        differing(&base, &half, 8) > 0,
        "half scale should draw a coarser frame"
    );

    // FLINT_QUALITY_PNG=<dir> writes the frames, to look at.
    if let Ok(dir) = std::env::var("FLINT_QUALITY_PNG") {
        let frames = [
            ("base", &base),
            ("msaa", &msaa),
            ("half", &half),
            ("unshadowed", &unshadowed),
            ("one_cascade", &one_cascade),
            ("two_cascades", &two_cascades),
            ("one_cascade_40m", &one_near),
        ];
        for (name, px) in frames {
            let path = format!("{dir}/{name}.png");
            image::save_buffer(path, px, SIZE, SIZE, image::ColorType::Rgba8).unwrap();
        }
    }

    assert!(
        brightness(&unshadowed) > brightness(&base),
        "with shadows off, the slab's shadow should go"
    );
    // The shadow's darkness, against the unshadowed frame.
    let shadow = |f: &[u8]| brightness(&unshadowed) as f64 - brightness(f) as f64;
    let full = shadow(&base);
    assert!(full > 0.0);
    // One cascade covers the same 200 m at a third the texels: the shadow
    // stays (before the cascades used the camera's own far plane it all
    // but vanished, the one map stretched over 1000 m).
    assert!(
        shadow(&one_cascade) > 0.8 * full,
        "one cascade drew {} of {full}",
        shadow(&one_cascade)
    );
    assert!(shadow(&two_cascades) > 0.9 * full);
    assert!(
        shadow(&one_near) > 0.9 * full,
        "one cascade over 40 m is sharp"
    );
}

#[test]
fn a_scene_load_keeps_the_caps() {
    let gpu = gpu_or_skip!();
    let (mut renderer, _) = scene(&gpu);
    let q = QualitySettings {
        bloom: false,
        ssao_samples: 16,
        ..QualitySettings::default()
    };
    renderer.set_quality(gpu.device, &q);

    // A scene load hands the renderer its own look, bloom and all.
    let mut look = renderer.post_process_config().clone();
    look.bloom_enabled = true;
    look.ssao_enabled = true;
    look.ssao_samples = 64;
    renderer.set_post_process_config(look);

    assert!(
        renderer.post_process_config().bloom_enabled,
        "the look is kept"
    );
    assert!(!renderer.effective_post_process_config().bloom_enabled);
    assert!(renderer.effective_post_process_config().ssao_enabled);
    assert_eq!(renderer.effective_post_process_config().ssao_samples, 16);

    // Lifting the cap brings the scene's bloom back.
    renderer.set_quality(gpu.device, &QualitySettings::default());
    assert!(renderer.effective_post_process_config().bloom_enabled);
    assert_eq!(renderer.effective_post_process_config().ssao_samples, 64);
}

#[test]
fn the_shadow_settings_hold_through_a_resolution_change() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let q = QualitySettings {
        shadow_cascades: 2,
        shadow_distance: 80.0,
        shadow_resolution: 1024,
        ..QualitySettings::default()
    };
    let applied = renderer.set_quality(gpu.device, &q);
    assert_eq!(applied, q);
    assert_eq!(renderer.shadow_resolution(), 1024);
    // A debug panel changing the resolution alone keeps the cascades.
    renderer.set_shadow_resolution(gpu.device, 2048);
    gpu.frame(&mut renderer, &world);
    // Two casters (ground and slab), each drawn into two cascades.
    assert_eq!(renderer.collect_stats().shadow_draw_calls, 4);
    renderer.set_shadows(false);
    assert_eq!(renderer.collect_stats().shadow_draw_calls, 0);
}
