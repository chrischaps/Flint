//! A frame's capture (#1085, ADR 0126): each pass's targets copied as the
//! pass leaves them, named on the pass in the frame's description, read
//! back after the frame. A frame nobody captures is the frame it was.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

#![allow(clippy::disallowed_methods)] // the test's own targets and read-backs, not the renderer's, so uncounted (#1091)

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::frame_capture::{CaptureTargets, CapturedFrame, FrameDescription};
use flint_render::{Camera, FrameHost, FrameInputs, RendererConfig, SceneRenderer, Vertex};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: wgpu::Texture,
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
    let attach = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let color = target(FORMAT, attach);
    let depth = target(
        wgpu::TextureFormat::Depth32Float,
        attach | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    Some(Gpu {
        color_view: color.create_view(&Default::default()),
        depth_view: depth.create_view(&Default::default()),
        color,
        depth,
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
        let v = |x: f32, z: f32| Vertex {
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            color: [0.8, 0.7, 0.5, 1.0],
            uv: [0.0, 0.0],
        };
        let vertices = [v(-2.0, -2.0), v(-2.0, 2.0), v(2.0, 2.0), v(2.0, -2.0)];
        renderer.load_procedural_mesh(
            self.device,
            "tile",
            &vertices,
            &[0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2],
            ImportedMaterial::vertex_colored("tile"),
        );
        renderer
    }

    /// One frame as a host draws it, then read back.
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
        self.queue.submit([encoder.finish()]);
        renderer.frame_submitted();
        self.pixels()
    }

    /// One frame captured: its description with each image named, and the
    /// images read back.
    fn captured(
        &self,
        renderer: &mut SceneRenderer,
        world: &FlintWorld,
    ) -> (FrameDescription, CapturedFrame, Vec<u8>) {
        renderer.capture_next_frame(CaptureTargets {
            surface: Some(self.color.clone()),
            depth: Some(self.depth.clone()),
        });
        let pixels = self.frame(renderer, world);
        let mut capture = renderer.take_frame_capture().expect("a capture was asked");
        capture.wait(self.device);
        let frame = capture.read().expect("the copies read back");
        let d = renderer
            .describe_frame(Some(world), &FrameHost::default())
            .expect("a captured frame is described");
        (d, frame, pixels)
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

fn spawn(world: &mut FlintWorld, name: &str, text: &str) {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
}

/// A tile in front of the default camera, lit by a sun that casts.
fn world() -> FlintWorld {
    let mut world = FlintWorld::new();
    spawn(
        &mut world,
        "tile",
        "[transform]\nposition = [0, 0, 0]\n[model]\nasset = \"tile\"\nruntime = true",
    );
    spawn(
        &mut world,
        "crate",
        "[transform]\nposition = [-1, 0.5, 0]\n[bounds]\nmin = [-0.5, -0.5, -0.5]\nmax = [0.5, 0.5, 0.5]",
    );
    spawn(
        &mut world,
        "sun",
        "[light]\ntype = \"directional\"\ndirection = [0.3, 1, 0.2]\nintensity = 2.0",
    );
    world
}

#[test]
fn each_pass_names_the_images_it_wrote_at_their_sizes() {
    let gpu = gpu_or_skip!();
    let mut renderer = gpu.renderer();
    let world = world();
    let (d, frame, pixels) = gpu.captured(&mut renderer, &world);
    assert!(d.frame.capturing);

    // Every image is on the pass that wrote it, in the order the frame
    // drew them, each read back at the target's size
    let images = d.images();
    assert_eq!(images.len(), frame.images.len(), "an image for every copy");
    let mut listed: Vec<(String, String)> = Vec::new();
    for pass in d.all_passes() {
        for image in &pass.images {
            assert!(
                pass.writes.contains(&image.target),
                "{} on {}",
                image.target,
                pass.name
            );
            let read = frame.image(image).expect("its read-back");
            assert_eq!([read.width, read.height], image.size);
            let texel = read
                .format
                .block_copy_size(Some(wgpu::TextureAspect::DepthOnly))
                .or(read.format.block_copy_size(None))
                .unwrap();
            assert_eq!(read.bytes.len() as u32, read.width * read.height * texel);
            assert!(image.file.starts_with("targets/"), "{}", image.file);
            listed.push((pass.name.clone(), image.target.clone()));
        }
    }
    let has = |pass: &str, target: &str| listed.iter().any(|(p, t)| p == pass && t == target);
    assert!(has("Scene", "hdr_scene"), "{listed:?}");
    assert!(has("Scene", "depth"), "{listed:?}");
    assert!(
        has("SSAO", "ssao") && has("SSAO", "ssao_blur"),
        "{listed:?}"
    );
    assert!(has("Composite", "surface"), "{listed:?}");
    // Each drawn shadow cascade is its own image, on its layer's pass
    let cascades: Vec<_> = listed
        .iter()
        .filter(|(p, _)| p.starts_with("Shadow cascade"))
        .collect();
    assert!(!cascades.is_empty(), "{listed:?}");
    let shadow = d
        .images()
        .into_iter()
        .find(|i| i.target == "shadow_map")
        .unwrap();
    assert_eq!(shadow.layer, Some(0));
    assert_eq!(shadow.stored, "exr-f32");
    // Bloom's steps each have an image: the threshold, the way down, the way up
    let steps: Vec<_> = d
        .pass("Bloom")
        .unwrap()
        .images
        .iter()
        .map(|i| i.step.clone().unwrap_or_default())
        .collect();
    assert_eq!(steps.first().map(String::as_str), Some("threshold"));
    assert_eq!(steps.last().map(String::as_str), Some("up"));
    // HDR as half floats, the 8-bit targets as PNG
    let hdr = d
        .images()
        .into_iter()
        .find(|i| i.target == "hdr_scene")
        .unwrap();
    assert_eq!((hdr.stored.as_str(), hdr.size), ("exr-half", [SIZE, SIZE]));
    let surface = d.pass("Composite").unwrap().images[0].clone();
    assert_eq!(surface.stored, "png");

    // The last surface image is the frame's output, exactly
    let last = images.iter().rfind(|i| i.target == "surface").unwrap();
    assert_eq!(frame.image(last).unwrap().bytes, pixels);
}

#[test]
fn a_frame_nobody_captures_is_the_frame_it_was() {
    let gpu = gpu_or_skip!();
    let world = world();
    let mut plain = gpu.renderer();
    let before = gpu.frame(&mut plain, &world);
    assert!(
        plain.take_frame_capture().is_none(),
        "nothing asked, nothing kept"
    );

    let mut renderer = gpu.renderer();
    let (_, _, captured) = gpu.captured(&mut renderer, &world);
    assert_eq!(captured, before, "a captured frame draws the same pixels");
    // and the frame after it is neither captured nor described
    let after = gpu.frame(&mut renderer, &world);
    assert_eq!(after, before);
    assert!(!renderer.capturing_frame());
    assert!(renderer.take_frame_capture().is_none());
    assert!(renderer
        .describe_frame(Some(&world), &FrameHost::default())
        .is_none());
}

#[test]
fn a_host_that_polls_gets_its_images_and_is_told_what_it_held_back() {
    let gpu = gpu_or_skip!();
    let world = world();
    let mut renderer = gpu.renderer();
    renderer.capture_next_frame(CaptureTargets::default());
    gpu.frame(&mut renderer, &world);
    let mut capture = renderer.take_frame_capture().unwrap();
    assert!(!capture.is_empty());
    // A host without its targets handed over gets the rest, and is told
    while !capture.poll(gpu.device) {
        std::thread::yield_now();
    }
    let frame = capture.read().unwrap();
    assert!(
        frame
            .untapped
            .iter()
            .any(|u| u.contains("surface") && u.contains("didn't hand it over")),
        "{:?}",
        frame.untapped
    );
    assert!(frame.images.iter().any(|i| i.target == "hdr_scene"));
}
