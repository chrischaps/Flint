//! GPU timing (ADR 0108): a renderer timing its phases on a real device.
//!
//! These need a GPU adapter with timestamp queries. Without one each test
//! prints a note and passes.

use flint_ecs::FlintWorld;
use flint_render::{Camera, RendererConfig, SceneRenderer};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A device that can time, if the adapter allows it.
fn timing_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = flint_render::gpu_select::instance();
        let adapter = flint_render::gpu_select::request_adapter(&instance, None).await?;
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        if features.is_empty() {
            return None;
        }
        adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_features: features,
                    ..Default::default()
                },
                None,
            )
            .await
            .ok()
    })
}

fn target(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[test]
fn a_renderer_times_its_phases_once_asked() {
    let Some((device, queue)) = timing_device() else {
        eprintln!("no GPU adapter with timestamp queries; skipping");
        return;
    };
    let mut renderer = SceneRenderer::new_headless(
        &device,
        &queue,
        FORMAT,
        SIZE,
        SIZE,
        RendererConfig::default(),
    );
    let color = target(&device, FORMAT);
    let depth = target(&device, wgpu::TextureFormat::Depth32Float);
    let world = FlintWorld::new();
    let camera = Camera::default();
    let frame = |renderer: &mut SceneRenderer| {
        renderer.update_from_world(&world, &device);
        renderer.render_to(&device, &queue, &depth, &camera, &color);
        renderer.finish_gpu_timings(&device, &queue);
        device.poll(wgpu::Maintain::Wait);
    };

    // Off by default: nothing timed.
    frame(&mut renderer);
    assert!(!renderer.gpu_timing());
    assert!(renderer.collect_stats().gpu_passes.is_empty());

    assert!(
        renderer.set_gpu_timing(&device, &queue, true),
        "the device can time"
    );
    // A host scope of its own, in the same frame as the renderer's.
    let timed = |renderer: &mut SceneRenderer| {
        renderer.update_from_world(&world, &device);
        renderer.render_to(&device, &queue, &depth, &camera, &color);
        let timer = renderer.gpu_timer_mut().unwrap();
        let scope = timer.begin_submit(&device, &queue, "UI");
        timer.end_submit(&device, &queue, scope);
        renderer.finish_gpu_timings(&device, &queue);
        device.poll(wgpu::Maintain::Wait);
    };
    // The times come back a frame or more late.
    for _ in 0..4 {
        timed(&mut renderer);
    }
    let stats = renderer.collect_stats();
    let labels: Vec<&str> = stats.gpu_passes.iter().map(|p| p.label).collect();
    assert_eq!(
        labels,
        // The post chain is a scope per effect that is on (#120).
        ["Shadows", "Scene", "SSAO", "Bloom", "Composite", "UI"]
    );
    assert!(stats
        .gpu_passes
        .iter()
        .all(|p| p.ms >= 0.0 && p.ms < 1000.0));
    let total: f32 = stats.gpu_passes.iter().map(|p| p.ms).sum();
    assert!(
        stats.gpu_frame_ms > 0.0 && (stats.gpu_frame_ms - total).abs() <= 0.2 * total,
        "the frame totals its scopes: {stats:?}"
    );

    // With FXAA on, it writes the target, so it's the scope apart and the
    // composite joins the chain.
    let mut config = renderer.post_process_config().clone();
    config.fxaa_enabled = true;
    renderer.set_post_process_config(config);
    renderer.ensure_fxaa_resources(&device);
    for _ in 0..4 {
        timed(&mut renderer);
    }
    let labels: Vec<&str> = renderer
        .collect_stats()
        .gpu_passes
        .iter()
        .map(|p| p.label)
        .collect();
    assert_eq!(labels, ["Shadows", "Scene", "SSAO", "Bloom", "FXAA", "UI"]);

    // Off again: the times go with the timer.
    assert!(!renderer.set_gpu_timing(&device, &queue, false));
    frame(&mut renderer);
    assert!(renderer.collect_stats().gpu_passes.is_empty());
}
