//! Comic panels (#880, ADR 0120): a page of panels drawn on a live
//! renderer, each panel its own camera and look at its own size, with no
//! validation error; a whole-frame panel is the frame byte for byte, and a
//! cut away and back draws the same page again from the views it kept.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::panels::PanelLayout;
use flint_render::{Camera, QualitySettings, RendererConfig, SceneRenderer, Vertex};

const W: u32 = 160;
const H: u32 = 96;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    _lease: shared_gpu::Lease,
}

/// The shared device (#598) alone: frames are drawn in an error scope.
fn gpu() -> Option<Gpu> {
    let lease = shared_gpu::lease_alone()?;
    let shared = lease.gpu;
    let device = &shared.device;
    let target = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: W,
                height: H,
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
        device,
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
    (vertices, vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2])
}

/// A ground and a red slab over it, and the slab's name to follow.
fn scene(gpu: &Gpu) -> (SceneRenderer, FlintWorld) {
    let mut renderer = SceneRenderer::new_headless(
        gpu.device,
        gpu.queue,
        FORMAT,
        W,
        H,
        RendererConfig::default(),
    );
    let mut world = FlintWorld::new();
    for (name, (vertices, indices), y) in [
        ("ground", square(6.0, 0.0, [0.8, 0.8, 0.8, 1.0]), 0.0),
        ("slab", square(1.5, 0.0, [0.9, 0.3, 0.2, 1.0]), 2.0),
    ] {
        renderer.load_procedural_mesh(
            gpu.device,
            name,
            &vertices,
            &indices,
            ImportedMaterial::vertex_colored(name),
        );
        let id = world.spawn(name).unwrap();
        let mut transform = toml::Table::new();
        transform.insert(
            "position".into(),
            toml::Value::Array(vec![0.0.into(), y.into(), 0.0.into()]),
        );
        world
            .set_component(id, "transform", transform.into())
            .unwrap();
        let mut model = toml::Table::new();
        model.insert("asset".into(), name.into());
        model.insert("runtime".into(), true.into());
        world.set_component(id, "model", model.into()).unwrap();
    }
    (renderer, world)
}

fn layout(text: &str) -> Option<PanelLayout> {
    let table: toml::Table = text.parse().unwrap();
    PanelLayout::parse(table["panels"].as_table().unwrap()).unwrap()
}

/// A wide panel at the frame's camera beside one looking down on the slab,
/// inverted: every pixel of the second is its own look.
const TWO: &str = r#"
    [panels]
    gutter = 8
    border = 2
    gutter_color = [1.0, 1.0, 0.0]
    border_color = [0.0, 0.0, 1.0]
    [panels.panel.wide]
    rect = [0.0, 0.0, 0.5, 1.0]
    [panels.panel.down]
    rect = [0.5, 0.0, 0.5, 1.0]
    follow = "slab"
    position = [0.0, 6.0, 0.1]
    fov = 50
    [panels.panel.down.post_process.styles.invert]
"#;

impl Gpu {
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.update_from_world(world, self.device);
        renderer.render_frame_to(
            self.device,
            self.queue,
            &self.depth_view,
            &frame_camera(),
            &self.color_view,
            Some(world),
        );
        let pixels = self.read();
        if let Some(e) = pollster::block_on(self.device.pop_error_scope()) {
            panic!("validation error: {e}");
        }
        pixels
    }

    fn read(&self) -> Vec<u8> {
        let row = (W * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * H) as u64,
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
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device.poll(wgpu::Maintain::Wait);
        let data = buffer.slice(..).get_mapped_range();
        let mut out = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            let at = (row * y) as usize;
            out.extend_from_slice(&data[at..at + (W * 4) as usize]);
        }
        out
    }
}

/// The frame's camera, at the frame's shape as a host keeps it
fn frame_camera() -> Camera {
    Camera {
        aspect: W as f32 / H as f32,
        ..Camera::default()
    }
}

/// `FLINT_PANELS_DUMP=<dir>` writes a frame there, to look at a failure.
fn dump(name: &str, frame: &[u8]) {
    if let Ok(dir) = std::env::var("FLINT_PANELS_DUMP") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        image::save_buffer(path, frame, W, H, image::ColorType::Rgba8).unwrap();
    }
}

fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * W + x) * 4) as usize;
    frame[at..at + 4].try_into().unwrap()
}

#[test]
fn a_whole_frame_panel_is_the_frame_byte_for_byte() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let plain = gpu.frame(&mut renderer, &world);
    renderer.set_panels(
        layout("[panels]\ngutter = 0\nborder = 0\n[panels.panel.all]\nrect = [0, 0, 1, 1]\n"),
        None,
    );
    let page = gpu.frame(&mut renderer, &world);
    dump("whole_plain", &plain);
    dump("whole_page", &page);
    assert_eq!(renderer.panel_submissions(), 1);
    assert!(
        plain == page,
        "a whole-frame panel at the frame's camera and look moved {} pixels",
        plain
            .chunks(4)
            .zip(page.chunks(4))
            .filter(|(a, b)| a != b)
            .count()
    );
}

#[test]
fn two_panels_draw_their_own_cameras_and_looks_on_a_page() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let plain = gpu.frame(&mut renderer, &world);
    let plain_again = gpu.frame(&mut renderer, &world);
    dump("plain", &plain);
    dump("plain_again", &plain_again);
    renderer.set_panels(layout(TWO), None);
    let page = gpu.frame(&mut renderer, &world);
    dump("two", &page);
    // One submission a panel, the last in the caller's
    assert_eq!(renderer.panel_submissions(), 2);
    assert_eq!(renderer.panel_views_kept(), 2);
    // The gutter round the page and between the panels, the ink round each
    assert_eq!(pixel(&page, 0, 0), [255, 255, 0, 255]);
    assert_eq!(pixel(&page, W / 2, H / 2), [255, 255, 0, 255]);
    assert_eq!(pixel(&page, 8, H / 2), [0, 0, 255, 255]);
    assert_eq!(pixel(&page, W - 9, H / 2), [0, 0, 255, 255]);
    // The second panel looks down on the slab, inverted: mostly the slab's
    // inverse (cyan-ish: red low, green and blue high)
    let c = pixel(&page, W * 3 / 4, H / 2);
    assert!(
        c[0] < c[1] && c[0] < c[2],
        "the down panel's centre is {c:?}"
    );
    // And the frame is the page, not the plain frame under it
    assert_ne!(pixel(&plain, W * 3 / 4, H / 2), c);
}

#[test]
fn a_cut_away_and_back_draws_the_same_page_from_the_views_kept() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    let plain = gpu.frame(&mut renderer, &world);
    renderer.set_panels(layout(TWO), None);
    let first = gpu.frame(&mut renderer, &world);
    // Cut to one view: the frame as it was, the window's own resources
    // untouched by the panels'
    renderer.set_panel_layout(None);
    let one = gpu.frame(&mut renderer, &world);
    dump("cut_plain", &plain);
    dump("cut_one", &one);
    assert!(
        plain == one,
        "one view after the page isn't the plain frame"
    );
    // And back: no new views, the same page
    renderer.set_panel_layout(layout(TWO));
    let again = gpu.frame(&mut renderer, &world);
    assert_eq!(renderer.panel_views_kept(), 2);
    assert!(first == again, "the page drawn again differs");
    // A steady page makes no bind groups a frame in the chain it swaps in
    let kept = renderer.kept_post_binds();
    gpu.frame(&mut renderer, &world);
    assert_eq!(renderer.kept_post_binds(), kept);
}

#[test]
fn panels_draw_under_msaa_and_a_render_scale() {
    let gpu = gpu_or_skip!();
    let (mut renderer, world) = scene(&gpu);
    renderer.set_panels(layout(TWO), None);
    gpu.frame(&mut renderer, &world);
    let q = QualitySettings {
        msaa: 4,
        render_scale: 0.5,
        ..QualitySettings::default()
    };
    renderer.set_quality(gpu.device, &q);
    let page = gpu.frame(&mut renderer, &world);
    assert_eq!(pixel(&page, 0, 0), [255, 255, 0, 255]);
    renderer.set_quality(gpu.device, &QualitySettings::default());
    gpu.frame(&mut renderer, &world);
}
