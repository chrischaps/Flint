//! GPU timing (ADR 0108): a renderer timing its phases on a real device.
//!
//! These need a GPU adapter with timestamp queries. Without one each test
//! prints a note and passes.

#![allow(clippy::disallowed_methods)] // the test's own targets and read-backs, not the renderer's, so uncounted (#1091)

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
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
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
        flint_render::gpu_select::skip_without_gpu("no GPU adapter with timestamp queries");
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
        renderer.prepare(
            &device,
            &queue,
            &world,
            &flint_render::FrameInputs::default(),
        );
        renderer.render_to(&device, &queue, &depth, &camera, &color);
        renderer.finish_gpu_timings(&device, &queue);
        let _ = flint_render::wait_for_gpu(&device);
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
        renderer.prepare(
            &device,
            &queue,
            &world,
            &flint_render::FrameInputs::default(),
        );
        renderer.render_to(&device, &queue, &depth, &camera, &color);
        let timer = renderer.gpu_timer_mut().unwrap();
        let scope = timer.begin_submit(&device, &queue, "UI");
        timer.end_submit(&device, &queue, scope);
        renderer.finish_gpu_timings(&device, &queue);
        let _ = flint_render::wait_for_gpu(&device);
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
    config.fxaa = true;
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

    // Recording (#292): every frame drawn from now on, raw, once flushed;
    // a frame that waited on the GPU each time is never skipped.
    renderer.gpu_timer_mut().unwrap().record(true);
    for _ in 0..5 {
        timed(&mut renderer);
    }
    let timer = renderer.gpu_timer_mut().unwrap();
    timer.flush(&device);
    assert_eq!(timer.recorded().len(), 5, "every frame, the last included");
    let spread = flint_render::gpu_timer::spread(timer.recorded(), 1);
    assert_eq!(spread.frames, 4);
    let ui = spread.passes.iter().find(|p| p.label == "UI").unwrap();
    assert_eq!(ui.samples.len(), 4);
    assert!(ui.min <= ui.p50 && ui.p50 <= ui.p90 && ui.p90 <= ui.max);
    timer.record(false);
    assert!(timer.recorded().is_empty());

    // Each frame's busy time comes back under the index the host gave the
    // frame it timed (#1104), not the frame it arrives in.
    let _ = renderer.take_gpu_frame_times().count();
    for n in 100..103u64 {
        renderer.gpu_timer_mut().unwrap().set_frame_index(Some(n));
        timed(&mut renderer);
    }
    renderer.gpu_timer_mut().unwrap().set_frame_index(None);
    timed(&mut renderer);
    renderer.gpu_timer_mut().unwrap().flush(&device);
    let mut times: Vec<_> = renderer.take_gpu_frame_times().collect();
    assert_eq!(times.len(), 4, "every frame came back: {times:?}");
    times.sort_by_key(|t| t.index);
    let indices: Vec<Option<u64>> = times.iter().map(|t| t.index).collect();
    assert_eq!(indices, [None, Some(100), Some(101), Some(102)]);
    assert!(
        times.iter().all(|t| t.ms > 0.0 && t.ms < 1000.0),
        "{times:?}"
    );
    assert_eq!(renderer.take_gpu_frame_times().count(), 0, "taken once");

    // Off again: the times go with the timer.
    assert!(!renderer.set_gpu_timing(&device, &queue, false));
    frame(&mut renderer);
    assert!(renderer.collect_stats().gpu_passes.is_empty());
}
