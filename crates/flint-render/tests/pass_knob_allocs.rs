//! The pass knobs cost a steady frame nothing (#1317): a frame drawn under
//! knobs that change no work (`Scene:1`, and a skip of a part this scene
//! never draws) allocates exactly what a frame with no knobs does, once
//! the knobs have seen the frame's labels. Its own binary, since the
//! counting allocator is the process's. Needs a GPU adapter; without one
//! it prints a note and passes.
//!
//! `steady_frame_allocations` prints the count a frame with no knobs makes,
//! the figure #1317 compared with main's binary.
#![allow(clippy::disallowed_methods)] // the test's own targets, not the renderer's, so uncounted (#1091)

mod shared_gpu;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use flint_ecs::FlintWorld;
use flint_render::{Camera, FrameInputs, RendererConfig, SceneRenderer};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn spawn(world: &mut FlintWorld, name: &str, text: &str) {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
}

/// A box and a sun: the shadow, scene and post passes all run.
fn world() -> FlintWorld {
    let mut world = FlintWorld::new();
    spawn(
        &mut world,
        "crate",
        "[transform]\nposition = [0, 0.5, 0]\n[bounds]\nmin = [-0.5, -0.5, -0.5]\nmax = [0.5, 0.5, 0.5]",
    );
    spawn(
        &mut world,
        "sun",
        "[light]\ntype = \"directional\"\ndirection = [0.3, 1, 0.2]\nintensity = 2.0",
    );
    world
}

/// The fewest allocations any of 40 steady frames made, after 20 to warm
/// every cache.
fn steady_allocations(
    lease: &shared_gpu::Lease,
    configure: impl FnOnce(&mut SceneRenderer),
) -> u64 {
    let (device, queue) = (&lease.gpu.device, &lease.gpu.queue);
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
    let color = target(FORMAT, attach).create_view(&Default::default());
    let depth = target(
        wgpu::TextureFormat::Depth32Float,
        attach | wgpu::TextureUsages::TEXTURE_BINDING,
    )
    .create_view(&Default::default());
    let mut renderer =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    configure(&mut renderer);
    let world = world();
    let mut fewest = u64::MAX;
    for frame in 0..60 {
        let before = ALLOCS.load(Ordering::Relaxed);
        renderer.prepare(device, queue, &world, &FrameInputs::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.encode_to(
            device,
            queue,
            &mut encoder,
            &depth,
            &Camera::default(),
            &color,
        );
        queue.submit([encoder.finish()]);
        renderer.frame_submitted();
        let _ = flint_render::wait_for_gpu(device);
        let made = ALLOCS.load(Ordering::Relaxed) - before;
        if frame >= 20 {
            fewest = fewest.min(made);
        }
    }
    fewest
}

#[test]
fn steady_frame_allocations() {
    // Alone: the count is the process's, so no other test may draw meanwhile
    let Some(lease) = shared_gpu::lease_alone() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let plain = steady_allocations(&lease, |_| {});
    println!("allocations in a steady frame, no knobs: {plain}");

    let knobbed = steady_allocations(&lease, |r| {
        let mut knobs = flint_render::PassKnobs::new();
        knobs.repeat_list("Scene:1").unwrap();
        knobs.skip_list("World lines");
        r.set_pass_knobs(Some(knobs));
    });
    println!("allocations in a steady frame, knobs that change no work: {knobbed}");
    assert_eq!(knobbed, plain, "the knobs allocate in a steady frame");
}
