//! Two threads on one DX12 device, each submitting a few ms of compute and then
//! blocking in device.poll(Wait), over and over. wgpu-hal 23 shares one auto-reset
//! event among all waiters (#913), so one thread can sleep out wgpu-core's 60 s
//! CLEANUP_WAIT_MS. Prints each thread's slowest poll and the wall time.
//! Built twice: `--features w23` and `--features w30`.
#[cfg(feature = "w23")]
use wgpu23 as wgpu;
#[cfg(feature = "w30")]
use wgpu30 as wgpu;

use std::sync::Arc;
use std::time::{Duration, Instant};

const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var x = data[id.x];
    for (var i = 0u; i < 20000u; i++) { x = x * 1664525u + 1013904223u; }
    data[id.x] = x;
}
"#;

fn main() {
    let iterations: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(40);
    let threads: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let groups: u32 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(1024);
    #[cfg(feature = "w23")]
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..Default::default()
    });
    #[cfg(feature = "w30")]
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions { shader_compiler: wgpu::Dx12Compiler::Fxc, ..Default::default() },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    #[cfg(feature = "w23")]
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    #[cfg(feature = "w30")]
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    println!("adapter: {:?}", adapter.get_info().name);
    #[cfg(feature = "w23")]
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default(), None)).unwrap();
    #[cfg(feature = "w30")]
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = Arc::new(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    }));

    let start = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let (device, queue, pipeline) = (device.clone(), queue.clone(), pipeline.clone());
            std::thread::spawn(move || {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 4 * 64 * groups as u64,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                });
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                });
                let mut slowest = Duration::ZERO;
                for _ in 0..iterations {
                    let mut enc = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = enc.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &bg, &[]);
                        pass.dispatch_workgroups(groups, 1, 1);
                    }
                    queue.submit(Some(enc.finish()));
                    let t0 = Instant::now();
                    #[cfg(feature = "w23")]
                    device.poll(wgpu::Maintain::Wait);
                    #[cfg(feature = "w30")]
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    slowest = slowest.max(t0.elapsed());
                }
                (t, slowest)
            })
        })
        .collect();
    for h in handles {
        let (t, slowest) = h.join().unwrap();
        println!("thread {t}: slowest poll {:.3} s", slowest.as_secs_f64());
    }
    println!("{threads} threads x {iterations} submits: {:.2} s wall", start.elapsed().as_secs_f64());
}
