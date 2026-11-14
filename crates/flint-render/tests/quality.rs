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
        let guard = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.update_from_world(world, self.device);
        renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        let pixels = self.read();
        if let Some(e) = pollster::block_on(guard.pop()) {
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
        self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped buffer range");
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

/// A skinned quad of half-width `half` at height `y`, every vertex on the
/// one joint, so a bone matrix moves it whole.
fn skinned_square(name: &str, half: f32, y: f32) -> flint_import::ImportResult {
    let (vertices, indices) = square(half, y, [0.3, 0.6, 0.9, 1.0]);
    flint_import::ImportResult {
        asset_meta: flint_asset::AssetMeta {
            name: name.to_string(),
            asset_type: flint_asset::AssetType::Mesh,
            hash: String::new(),
            source_path: None,
            format: None,
            properties: Default::default(),
            tags: Vec::new(),
        },
        meshes: vec![flint_import::ImportedMesh {
            name: name.to_string(),
            positions: vertices.iter().map(|v| v.position).collect(),
            normals: vertices.iter().map(|v| v.normal).collect(),
            uvs: vertices.iter().map(|v| v.uv).collect(),
            indices,
            material_index: Some(0),
            joint_indices: Some(vec![[0; 4]; vertices.len()]),
            joint_weights: Some(vec![[1.0, 0.0, 0.0, 0.0]; vertices.len()]),
            skin_index: Some(0),
            lod: None,
        }],
        textures: Vec::new(),
        materials: vec![ImportedMaterial {
            base_color: [0.3, 0.6, 0.9, 1.0],
            ..ImportedMaterial::vertex_colored(name)
        }],
        skeletons: vec![flint_import::ImportedSkeleton {
            name: name.to_string(),
            joints: vec![flint_import::ImportedJoint {
                name: "root".to_string(),
                index: 0,
                parent: None,
                inverse_bind_matrix: IDENTITY,
                rest_translation: [0.0; 3],
                rest_rotation: [0.0, 0.0, 0.0, 1.0],
                rest_scale: [1.0; 3],
                parent_offset: None,
            }],
        }],
        skeletal_clips: Vec::new(),
        node_clips: Vec::new(),
        nodes: Vec::new(),
        root_nodes: Vec::new(),
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn translation(x: f32, y: f32, z: f32) -> [[f32; 4]; 4] {
    let mut m = IDENTITY;
    m[3] = [x, y, z, 1.0];
    m
}

fn set_material(world: &mut FlintWorld, name: &str, keys: &[(&str, toml::Value)]) {
    let id = world.get_id(name).unwrap();
    let mut look = toml::Table::new();
    for (k, v) in keys {
        look.insert((*k).into(), v.clone());
    }
    world.set_component(id, "material", look.into()).unwrap();
}

/// Every kind of scene draw at once (#735): plain and blended static meshes,
/// both woozy (#215), a skinned mesh woozy too, a skinned prop with a
/// `lighten` echo and a soft-edged part (#582), a billboard and particles.
fn every_draw_kind(gpu: &Gpu) -> (SceneRenderer, FlintWorld) {
    let (mut renderer, mut world) = scene(gpu);
    for (name, (vertices, indices)) in [
        ("woozy", square(0.8, 1.0, [0.2, 0.9, 0.3, 1.0])),
        ("woozy_glass", square(0.8, 3.0, [0.9, 0.9, 0.2, 1.0])),
        ("glass", square(0.8, 3.5, [0.2, 0.2, 0.9, 1.0])),
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
    set_material(
        &mut world,
        "woozy_glass",
        &[("opacity", 0.5.into()), ("blend_mode", "alpha".into())],
    );
    set_material(
        &mut world,
        "glass",
        &[("opacity", 0.5.into()), ("blend_mode", "additive".into())],
    );

    for name in ["prop", "woozy_figure"] {
        renderer.load_skinned_model(gpu.device, gpu.queue, name, &skinned_square(name, 0.7, 2.5));
        spawn(&mut world, name, name);
    }
    let prop = world.get_id("prop").unwrap();
    let figure = world.get_id("woozy_figure").unwrap();
    renderer.write_entity_bones(
        gpu.device,
        gpu.queue,
        prop,
        0,
        &[translation(-2.0, 0.0, 0.0)],
    );
    renderer.write_entity_echo_bones(
        gpu.device,
        gpu.queue,
        prop,
        0,
        0.6,
        0,
        &[translation(-2.5, 0.3, 0.5)],
    );
    renderer.set_entity_echo_lighten(prop, true);
    renderer.write_entity_bones(
        gpu.device,
        gpu.queue,
        figure,
        0,
        &[translation(2.0, 0.0, 0.0)],
    );
    renderer.set_entity_part_softness(figure, 0, 0.5);

    let woozy = flint_render::ShallowWoozeDraw {
        amplitude: 0.3,
        wavelength: 2.0,
        index: 3,
        weight: 0.4,
    };
    renderer.set_shallow_wooze(
        ["woozy", "woozy_glass", "woozy_figure"].map(|n| (world.get_id(n).unwrap(), woozy)),
    );

    let sprite_id = world.spawn("sprite").unwrap();
    let mut transform = toml::Table::new();
    transform.insert(
        "position".into(),
        toml::Value::Array(vec![0.0.into(), 4.0.into(), 2.0.into()]),
    );
    world
        .set_component(sprite_id, "transform", transform.into())
        .unwrap();
    let mut sprite = toml::Table::new();
    sprite.insert("width".into(), 1.0.into());
    sprite.insert("height".into(), 1.0.into());
    world
        .set_component(sprite_id, "sprite", sprite.into())
        .unwrap();

    let instances: Vec<flint_render::ParticleInstance> = (0..8)
        .map(|i| flint_render::ParticleInstance {
            pos_size: [i as f32 * 0.4 - 1.6, 1.5, 2.5, 0.4],
            color: [1.0, 0.5, 0.2, 0.8],
            rotation_frame: [0.0, 0.0, 1.0, 1.0],
            vel_stretch: [0.0, 0.0, 0.0, 0.4],
        })
        .collect();
    renderer.update_particles(
        gpu.device,
        gpu.queue,
        &[flint_render::ParticleDrawData {
            instances: &instances,
            texture: "",
            blend: flint_render::ParticleBlendMode::Additive,
            sort_key: 0.0,
        }],
    );
    (renderer, world)
}

/// MSAA 1 to 4 to 1 at run time with every kind of draw on screen (#735).
/// Each frame is drawn in a validation error scope, so a pipeline left at
/// the old sample count fails here, naming the pass. Twice: once with the
/// lazily built pipelines (shallow wooze's) made before the switch, once
/// with the switch before they're first needed.
#[test]
fn msaa_switches_with_every_draw_kind_on_screen() {
    let gpu = gpu_or_skip!();
    let d = QualitySettings::default();
    let four = QualitySettings { msaa: 4, ..d };
    for built_first in [true, false] {
        let (mut renderer, world) = every_draw_kind(&gpu);
        let base = if built_first {
            Some(gpu.frame(&mut renderer, &world))
        } else {
            None
        };
        renderer.set_quality(gpu.device, &four);
        assert_eq!(renderer.msaa(), 4);
        let smooth = gpu.frame(&mut renderer, &world);
        renderer.set_quality(gpu.device, &d);
        assert_eq!(renderer.msaa(), 1);
        let back = gpu.frame(&mut renderer, &world);
        if let Some(base) = base {
            assert!(brightness(&base) > 0, "the scene drew nothing");
            assert_eq!(
                differing(&base, &back, 0),
                0,
                "1 to 4 to 1 must draw the first frame again"
            );
            assert!(
                differing(&base, &smooth, 8) > 0,
                "4x should smooth some edge"
            );
        }
    }
}
