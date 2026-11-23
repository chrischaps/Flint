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

/// [`target`]'s texture, which the capture can read (`COPY_SRC`).
fn tapped(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

#[test]
fn a_captures_copies_are_timed_on_their_own_queries() {
    // #1160: every copy a capture makes is timed on the capture's own query
    // set, so the frame's timer still times every pass it did before
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
    let color = tapped(&device, FORMAT);
    let depth = tapped(&device, wgpu::TextureFormat::Depth32Float);
    let (color_view, depth_view) = (
        color.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    );
    let world = FlintWorld::new();
    let camera = Camera::default();
    assert!(renderer.set_gpu_timing(&device, &queue, true));
    renderer.gpu_timer_mut().unwrap().record(true);
    renderer.prepare(
        &device,
        &queue,
        &world,
        &flint_render::FrameInputs::default(),
    );
    // Armed after the frame's prepare, as `flint render`'s still does: the
    // copies are timed all the same
    renderer.capture_next_frame(flint_render::frame_capture::CaptureTargets {
        surface: Some(color.clone()),
        depth: Some(depth.clone()),
    });
    renderer.render_to(&device, &queue, &depth_view, &camera, &color_view);
    renderer.finish_gpu_timings(&device, &queue);
    let mut capture = renderer.take_frame_capture().expect("a capture was asked");
    capture.wait(&device);
    let frame = capture.read().expect("the copies read back");

    // Every copy timed, and the capture's figure is theirs together
    assert!(!frame.images.is_empty());
    assert_eq!(frame.gpu_copies_timed as usize, frame.images.len());
    let each: Vec<f64> = frame
        .images
        .iter()
        .map(|i| i.gpu_ms.expect("every copy timed"))
        .collect();
    assert!(each.iter().all(|ms| (0.0..1000.0).contains(ms)), "{each:?}");
    let total = frame.gpu_copy_ms.expect("timed");
    assert!((total - each.iter().sum::<f64>()).abs() < 1e-9);

    // The frame's own timer kept its scopes for the passes: the same
    // labels a frame nobody captures times
    let timer = renderer.gpu_timer_mut().unwrap();
    timer.flush(&device);
    let labels: Vec<&str> = timer.recorded()[0].passes.iter().map(|p| p.label).collect();
    assert_eq!(labels, ["Shadows", "Scene", "SSAO", "Bloom", "Composite"]);
}

/// A capture's images at 1920 x 1080 besides the shadow cascades (#1160),
/// as the tavern's and the grove's manifests list them: format, width and
/// height. Both have the scene's depth and HDR, SSAO and its blur, nine
/// bloom steps and the surface; the grove has `more_hdr` full-size half
/// float targets more (the fog veil's two, the style edges, Kuwahara's ping
/// and two tensors, Contour's ping: 7).
fn captured_images(more_hdr: usize) -> Vec<(wgpu::TextureFormat, u32, u32)> {
    use wgpu::TextureFormat as F;
    let full = |f| (f, 1920, 1080);
    let mut images = vec![full(F::Depth32Float)];
    images.extend(std::iter::repeat_n(full(F::Rgba16Float), 1 + more_hdr));
    images.extend([full(F::R8Unorm), full(F::R8Unorm)]);
    for (w, h) in [(960, 540), (480, 270), (240, 135), (120, 67), (60, 33)] {
        images.push((F::Rgba16Float, w, h));
    }
    for (w, h) in [(120, 67), (240, 135), (480, 270), (960, 540)] {
        images.push((F::Rgba16Float, w, h));
    }
    images.push(full(F::Rgba8UnormSrgb));
    images
}

#[test]
#[ignore = "a measurement (#1160): cargo test --release -p flint-render --test gpu_timer spike -- --ignored --nocapture"]
fn spike_shaped_copies_of_the_tavern_and_the_grove() {
    // ADR 0126's spike read every target back in one submission after the
    // frame, mapped every buffer and waited, and called that the GPU wait;
    // its second capture made its buffers again (the first's were dropped).
    // This does the same with each scene's images, the GPU idle before each
    // round as the spike's was (it had read the frame's pixels), timing the
    // copies with a timestamp either side of them all and splitting the
    // wait into the GPU finishing and the buffers mapping.
    let Some((device, queue)) = timing_device() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter with timestamp queries");
        return;
    };
    // Dropped after each round, as the spike's were: the next capture's
    // staging can take the memory the last one gave back
    spike_rounds(&device, &queue, "tavern", 0, false);
    spike_rounds(&device, &queue, "grove", 7, false);
    // Held: each round's staging is memory the process never had
    spike_rounds(&device, &queue, "tavern, held", 0, true);
    spike_rounds(&device, &queue, "grove, held", 7, true);
}

fn spike_rounds(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &str,
    more_hdr: usize,
    hold: bool,
) {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let make = |format, w, h, layers| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    // (texture, layer): the shadow array's three cascades, then the rest
    let shadow = make(wgpu::TextureFormat::Depth32Float, 2048, 2048, 3);
    let mut copies: Vec<(wgpu::Texture, u32)> = (0..3).map(|l| (shadow.clone(), l)).collect();
    for (f, w, h) in captured_images(more_hdr) {
        copies.push((make(f, w, h, 1), 0));
    }
    // Each written once, as a frame writes it
    let mut encoder = device.create_command_encoder(&Default::default());
    for (texture, layer) in &copies {
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: *layer,
            array_layer_count: Some(1),
            ..Default::default()
        });
        let depth = texture.format().is_depth_stencil_format();
        let color = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.25,
                    g: 0.5,
                    b: 0.75,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })];
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: if depth { &[] } else { &color },
            depth_stencil_attachment: depth.then_some(wgpu::RenderPassDepthStencilAttachment {
                view: &view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.5),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
    }
    queue.submit([encoder.finish()]);
    let _ = flint_render::wait_for_gpu(device);

    let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let stamp = |encoder: &mut wgpu::CommandEncoder, i| {
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &queries,
                beginning_of_pass_write_index: Some(i),
                end_of_pass_write_index: None,
            }),
        });
    };
    let period = f64::from(queue.get_timestamp_period());
    let mut held = Vec::new();
    for round in ["cold", "a second at once", "a third"] {
        let started = web_time::Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        stamp(&mut encoder, 0);
        let mut buffers = Vec::new();
        let mut bytes = 0u64;
        for (texture, layer) in &copies {
            let format = texture.format();
            let aspect = if format.is_depth_stencil_format() {
                wgpu::TextureAspect::DepthOnly
            } else {
                wgpu::TextureAspect::All
            };
            let texel = format.block_copy_size(Some(aspect)).unwrap();
            let row = (texture.width() * texel).div_ceil(256) * 256;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row) * u64::from(texture.height()),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            bytes += buffer.size();
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: *layer,
                    },
                    aspect,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(texture.height()),
                    },
                },
                wgpu::Extent3d {
                    width: texture.width(),
                    height: texture.height(),
                    depth_or_array_layers: 1,
                },
            );
            buffers.push(buffer);
        }
        stamp(&mut encoder, 1);
        let ticks = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.resolve_query_set(&queries, 0..2, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &ticks, 0, 16);
        let recorded = started.elapsed().as_secs_f64() * 1000.0;
        queue.submit([encoder.finish()]);
        let submitted = web_time::Instant::now();
        // The spike's wait, split: the GPU done (only the timestamps mapped),
        // then every staging buffer mapped
        ticks
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        let _ = flint_render::wait_for_gpu(device);
        let gpu_done = submitted.elapsed().as_secs_f64() * 1000.0;
        let mapping = web_time::Instant::now();
        for buffer in &buffers {
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        }
        let _ = flint_render::wait_for_gpu(device);
        let mapped = mapping.elapsed().as_secs_f64() * 1000.0;
        let t: Vec<u64> =
            bytemuck::pod_collect_to_vec(&ticks.slice(..).get_mapped_range().unwrap());
        let gpu = (t[1] - t[0]) as f64 * period / 1e6;
        println!(
            "{scene}, {round}: {} copies, {:.1} MB; record {recorded:.2} ms, GPU done {gpu_done:.2} ms after the submit, then mapped in {mapped:.2} ms; copies on the GPU {gpu:.2} ms",
            copies.len(),
            bytes as f64 / 1e6
        );
        if hold {
            held.push(buffers);
        }
    }
}
