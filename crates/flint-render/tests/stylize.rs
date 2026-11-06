//! Stylize passes (#262) on a real device: a display style inverts the
//! finished frame, switching it off gives back the exact bytes, the chain
//! survives a resize, and an HDR style and a display style run in one
//! frame without a validation error.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use flint_ecs::FlintWorld;
use flint_render::{Camera, RendererConfig, SceneRenderer};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
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
        let color = target(
            FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Some(Gpu {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            device,
            queue,
        })
    })
}

impl Gpu {
    fn frame(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        renderer.update_from_world(world, &self.device);
        renderer.render_to(
            &self.device,
            &self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
        self.device.poll(wgpu::Maintain::Wait);
        self.pixels()
    }

    /// The last frame's RGBA8 pixels, row by row.
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

#[test]
fn a_display_style_inverts_the_frame_and_lets_it_go() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let plain = gpu.frame(&mut renderer, &world);

    let set = |renderer: &mut SceneRenderer, f: &dyn Fn(&mut flint_render::PostProcessConfig)| {
        let mut config = renderer.post_process_config().clone();
        f(&mut config);
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&gpu.device);
    };

    // On: the negative of every pixel, alpha kept. The style inverts
    // display values on an sRGB target, so it's exact to a rounding step.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", true));
    let inverted = gpu.frame(&mut renderer, &world);
    for (i, (a, b)) in plain.iter().zip(&inverted).enumerate() {
        let want = if i % 4 == 3 { *a } else { 255 - *a };
        assert!(
            want.abs_diff(*b) <= 1,
            "byte {i}: plain {a}, inverted {b}, want {want}"
        );
    }

    // Off: the composite writes the target again, byte for byte.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", false));
    assert_eq!(gpu.frame(&mut renderer, &world), plain);

    // A resize rebuilds the chain's targets; the style still draws.
    set(&mut renderer, &|c| c.styles.set_enabled("invert", true));
    renderer.resize_postprocess(&gpu.device, SIZE, SIZE);
    assert_eq!(gpu.frame(&mut renderer, &world), inverted);

    // A host that only sets the config (no ensure call) still gets the
    // style: the frame builds it.
    let mut fresh = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let mut config = fresh.post_process_config().clone();
    config.styles.set_enabled("invert", true);
    fresh.set_post_process_config(config);
    assert_eq!(gpu.frame(&mut fresh, &world), inverted);

    // Half strength meets in the middle.
    set(&mut renderer, &|c| {
        c.styles.set_param("invert", "strength", 0.5)
    });
    let half = gpu.frame(&mut renderer, &world);
    for (i, b) in half.iter().enumerate() {
        if i % 4 != 3 {
            assert!((127..=128).contains(b), "byte {i}: {b}");
        }
    }
}

#[test]
fn hdr_and_display_styles_chain_in_one_frame() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let world = FlintWorld::new();
    let mut config = renderer.post_process_config().clone();
    config.kuwahara_enabled = true;
    renderer.set_post_process_config(config.clone());
    renderer.ensure_style_resources(&gpu.device);
    let painted = gpu.frame(&mut renderer, &world);

    config.styles.set_enabled("invert", true);
    config.fxaa_enabled = true;
    renderer.set_post_process_config(config);
    renderer.ensure_style_resources(&gpu.device);
    renderer.ensure_fxaa_resources(&gpu.device);
    // Two frames: the second reuses every bind group the first made
    gpu.frame(&mut renderer, &world);
    let both = gpu.frame(&mut renderer, &world);
    // An empty world is flat sky, which FXAA leaves be: still the negative
    for (i, (a, b)) in painted.iter().zip(&both).enumerate() {
        if i % 4 != 3 {
            assert!((255 - *a).abs_diff(*b) <= 2, "byte {i}: {a} then {b}");
        }
    }
}
