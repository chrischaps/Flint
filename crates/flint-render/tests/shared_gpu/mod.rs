//! One GPU device for a whole test binary, lent to its tests in turn (#598).
//!
//! Tests that each made their own `wgpu::Instance` and device hung for
//! hours on Windows with an NVIDIA card: `vkCreateDevice` takes the
//! driver's lock and then calls `LoadLibraryExW`, which needs the loader
//! lock, while a finished test thread exiting holds the loader lock and
//! its `DLL_THREAD_DETACH` into the driver waits for the driver's lock.
//! Once two such threads met, every other thread that touched the driver
//! queued behind them, and the binary sat still: `tests/stylize.rs`, run
//! alone on an RTX, hung in 5 of 11 runs (and for 3.5 hours in a full
//! workspace `cargo test`); sharing one device, it finished 18 of 18.
//!
//! So a binary makes its device exactly once and its tests share it:
//! [`lease`] lends it alongside the other tests, [`lease_alone`] with no
//! other test running. A test needs it alone when it counts objects
//! across the whole device (`Instance::generate_report`), or when it
//! builds user styles, which catch the GPU's errors in an error scope,
//! and wgpu 23 keeps one stack of those per device, not per thread.
//!
//! And a test can't hang for hours again: a watchdog aborts the binary,
//! naming the test, once any lease has been held past
//! `FLINT_GPU_TEST_TIMEOUT` seconds (default [`DEFAULT_TIMEOUT_S`]). It
//! aborts rather than exits because exiting needs the loader lock, which
//! is what a hung thread is holding.

#![allow(dead_code)] // each test binary uses its own share of this

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// Seconds a test may hold the GPU before the watchdog calls it a hang.
/// The slowest test here takes about 30 s in a debug build alone.
pub const DEFAULT_TIMEOUT_S: u64 = 300;

/// The binary's one device.
pub struct SharedGpu {
    pub instance: wgpu::Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The adapter's name and backend, for messages.
    pub adapter: String,
}

/// The device, for as long as a test holds it. Dropping it gives the
/// turn back.
pub struct Lease {
    pub gpu: &'static SharedGpu,
    _turn: Turn,
    _watch: Watch,
}

enum Turn {
    Shared(RwLockReadGuard<'static, ()>),
    Alone(RwLockWriteGuard<'static, ()>),
}

static GPU: OnceLock<Option<SharedGpu>> = OnceLock::new();
/// Read: a test running alongside others. Write: a test running alone.
static TURNS: RwLock<()> = RwLock::new(());

/// The device, made on first use; `None` when there's no adapter.
pub fn device() -> Option<&'static SharedGpu> {
    GPU.get_or_init(|| {
        let made = pollster::block_on(async {
            let instance = flint_render::gpu_select::instance();
            let adapter = flint_render::gpu_select::request_adapter(&instance, None).await?;
            let info = adapter.get_info();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .ok()?;
            Some(SharedGpu {
                // On DX12 it names the shader compiler (#912)
                adapter: flint_render::gpu_select::adapter_line(&info),
                instance,
                device,
                queue,
            })
        });
        if made.is_some() {
            start_watchdog();
        }
        made
    })
    .as_ref()
}

/// The device, shared with whichever other tests hold a [`lease`].
pub fn lease() -> Option<Lease> {
    let gpu = device()?;
    let turn = Turn::Shared(TURNS.read().unwrap_or_else(|e| e.into_inner()));
    Some(Lease {
        gpu,
        _turn: turn,
        _watch: Watch::start(),
    })
}

/// The device with no other test using it (see the module docs for
/// when a test needs this).
pub fn lease_alone() -> Option<Lease> {
    let gpu = device()?;
    let turn = Turn::Alone(TURNS.write().unwrap_or_else(|e| e.into_inner()));
    Some(Lease {
        gpu,
        _turn: turn,
        _watch: Watch::start(),
    })
}

/// Leases held now: id -> (test name, since).
static HELD: Mutex<BTreeMap<u64, (String, Instant)>> = Mutex::new(BTreeMap::new());
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Watch(u64);

impl Watch {
    fn start() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        // libtest names each test's thread for the test
        let name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        HELD.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (name, Instant::now()));
        Watch(id)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        HELD.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

fn timeout() -> Duration {
    let secs = std::env::var("FLINT_GPU_TEST_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_S);
    Duration::from_secs(secs)
}

/// The tests over `limit`, longest first, as `name (seconds)` lines.
fn overdue(limit: Duration, now: Instant) -> Vec<String> {
    let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let mut late: Vec<(Duration, &str)> = held
        .values()
        .map(|(name, since)| (now.duration_since(*since), name.as_str()))
        .filter(|(d, _)| *d > limit)
        .collect();
    late.sort_by_key(|&(d, _)| std::cmp::Reverse(d));
    late.iter()
        .map(|(d, name)| format!("{name} ({:.0} s)", d.as_secs_f64()))
        .collect()
}

/// One thread for the binary's life (so it never exits, and never meets
/// the driver on the way out), looking every second.
fn start_watchdog() {
    let limit = timeout();
    std::thread::Builder::new()
        .name("gpu-test-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let late = overdue(limit, Instant::now());
            if late.is_empty() {
                continue;
            }
            let mut err = std::io::stderr().lock();
            let _ = writeln!(
                err,
                "\nGPU test watchdog (#598): held the GPU past {} s \
                 (FLINT_GPU_TEST_TIMEOUT), so this binary is hung; aborting:",
                limit.as_secs()
            );
            for line in &late {
                let _ = writeln!(err, "  {line}");
            }
            let _ = err.flush();
            std::process::abort();
        })
        .expect("spawn the GPU test watchdog");
}
