//! Hot reload of the built-in styles' WGSL (#418) on a real device: with
//! the shader folder pointed at a copy of `src/stylize`, every built-in
//! style builds from the copy, an edit to `invert.wgsl` changes the next
//! frames, a typo keeps the last good picture and names its line, a
//! shader naga accepts but wgpu refuses keeps it too, and putting the
//! file back brings the style back.
//!
//! A binary of its own, since the folder is the process's. Needs a GPU
//! adapter; without one it prints a note and passes.

mod shared_gpu;

use flint_ecs::FlintWorld;
use flint_render::stylize::{dev_shader, registry};
use flint_render::{Camera, RendererConfig, SceneRenderer};
use std::path::{Path, PathBuf};

const SIZE: u32 = 32;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Frame {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

impl Frame {
    fn new(device: &'static wgpu::Device, queue: &'static wgpu::Queue) -> Self {
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
        Self {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            device,
            queue,
        }
    }

    fn draw(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
        renderer.update_from_world(world, self.device);
        renderer.render_to(
            self.device,
            self.queue,
            &self.depth_view,
            &Camera::default(),
            &self.color_view,
        );
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
        (0..SIZE as usize)
            .flat_map(|y| {
                let start = y * row as usize;
                data[start..start + SIZE as usize * 4].to_vec()
            })
            .collect()
    }
}

/// Every byte within one rounding step of `want`'s.
fn close(a: &[u8], want: &[u8]) -> bool {
    a.len() == want.len() && a.iter().zip(want).all(|(x, y)| x.abs_diff(*y) <= 1)
}

/// The frames a host draws while it looks at the files once.
fn settle(renderer: &mut SceneRenderer, device: &wgpu::Device) {
    for _ in 0..dev_shader::POLL_FRAMES {
        renderer.ensure_style_resources(device);
    }
}

/// `src/stylize/*.wgsl` and `src/palette.wgsl` (which `../palette.wgsl`
/// names) copied under `root`.
fn copy_shaders(root: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let dir = root.join("stylize");
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(src.join("stylize")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    std::fs::copy(src.join("palette.wgsl"), root.join("palette.wgsl")).unwrap();
    dir
}

/// Write `text` as `path`, with a stamp the poll can't miss.
fn write(path: &Path, text: &str) {
    let before = flint_core::file_watch::stamp(path);
    std::fs::write(path, text).unwrap();
    // Same length and a coarse clock could give the same stamp: wait it out
    while flint_core::file_watch::stamp(path) == before {
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn a_built_in_styles_shader_reloads_from_the_folder() {
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let (device, queue) = (&lease.gpu.device, &lease.gpu.queue);
    let frame = Frame::new(device, queue);
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("dev_shader_{}", std::process::id()));
    let dir = copy_shaders(&root);
    dev_shader::set_dir(Some(dir.clone()));

    // Every built-in style builds from the copy, with nothing to say
    let world = FlintWorld::new();
    let mut all =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    let mut config = all.post_process_config().clone();
    for d in registry() {
        config.styles.set_enabled(d.name, true);
    }
    all.set_post_process_config(config);
    all.ensure_style_resources(device);
    frame.draw(&mut all, &world);
    assert_eq!(dev_shader::errors(), Vec::<(String, String)>::new());
    for d in registry() {
        assert!(
            all.post_process_config().styles.is_enabled(d.name),
            "{} failed to build from {}",
            d.name,
            dir.display()
        );
    }
    drop(all);

    let mut renderer =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    let plain = frame.draw(&mut renderer, &world);
    let mut config = renderer.post_process_config().clone();
    config.styles.set_enabled("invert", true);
    renderer.set_post_process_config(config);
    renderer.ensure_style_resources(device);
    let inverted = frame.draw(&mut renderer, &world);
    assert!(!close(&inverted, &plain), "invert drew nothing");

    // An edit: invert by nothing. The next poll builds it again
    let file = dir.join("invert.wgsl");
    let original = std::fs::read_to_string(&file).unwrap();
    let strength = "vec3<f32>(1.0) - shown, params.strength)";
    assert!(original.contains(strength));
    let unchanged = original.replace(strength, "vec3<f32>(1.0) - shown, 0.0 * params.strength)");
    write(&file, &unchanged);
    // Not before the poll comes round
    renderer.ensure_style_resources(device);
    assert!(close(&frame.draw(&mut renderer, &world), &inverted));
    settle(&mut renderer, device);
    assert!(
        close(&frame.draw(&mut renderer, &world), &plain),
        "the edited shader wasn't built"
    );
    assert!(dev_shader::errors().is_empty());

    // A typo: the last good shader stays, and the error names its line
    let typo = unchanged.replace("let out = select(", "let out = = select(");
    let line = typo
        .lines()
        .position(|l| l.contains("= = select("))
        .unwrap()
        + 1;
    write(&file, &typo);
    settle(&mut renderer, device);
    assert!(
        close(&frame.draw(&mut renderer, &world), &plain),
        "a typo changed the frame"
    );
    let errors = dev_shader::errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let (style, message) = &errors[0];
    assert_eq!(style, "invert");
    let at = format!("{}:{line}:", file.display());
    assert!(message.starts_with(&at), "{message}");
    assert!(message.contains(&format!("{line} | ")), "{message}");

    // A shader naga takes and wgpu doesn't (no `fs_main` for the
    // pipeline): built again from the last good one, and said so
    write(&file, &unchanged.replace("fn fs_main(", "fn fs_renamed("));
    settle(&mut renderer, device);
    assert!(
        close(&frame.draw(&mut renderer, &world), &plain),
        "a refused shader changed the frame"
    );
    assert!(renderer.post_process_config().styles.is_enabled("invert"));
    let errors = dev_shader::errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, "invert");
    assert!(errors[0].1.contains("panicked"), "{}", errors[0].1);

    // Put back: inverting again, nothing to say
    write(&file, &original);
    settle(&mut renderer, device);
    assert!(close(&frame.draw(&mut renderer, &world), &inverted));
    assert!(
        dev_shader::errors().is_empty(),
        "{:?}",
        dev_shader::errors()
    );

    // The rebuilt pass's own uniforms are the ones bound: a kept bind
    // group would still read the first pass's buffer, at strength 1
    let mut config = renderer.post_process_config().clone();
    config.styles.set_param("invert", "strength", 0.5);
    renderer.set_post_process_config(config);
    let half = frame.draw(&mut renderer, &world);
    for (i, b) in half.iter().enumerate() {
        if i % 4 != 3 {
            assert!(
                (127..=128).contains(b),
                "byte {i}: {b} (a stale bind group?)"
            );
        }
    }
    let mut config = renderer.post_process_config().clone();
    config.styles.set_param("invert", "strength", 1.0);
    renderer.set_post_process_config(config);

    // Off: the compiled-in shader, and no file is looked at
    dev_shader::set_dir(None);
    write(&file, &typo);
    let mut off =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    let mut config = off.post_process_config().clone();
    config.styles.set_enabled("invert", true);
    off.set_post_process_config(config);
    settle(&mut off, device);
    assert!(close(&frame.draw(&mut off, &world), &inverted));
    assert!(dev_shader::errors().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
