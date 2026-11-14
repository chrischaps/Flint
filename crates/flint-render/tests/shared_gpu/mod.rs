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
//! So a binary makes its device exactly once and its tests share it,
//! one at a time: [`lease`] and [`lease_alone`] each lend it to one test
//! while the others wait their turn. (They lent it alongside other tests
//! until #913; see below. `lease_alone` still names a test that must
//! never share it: one that counts objects across the whole device,
//! `Instance::generate_report`, or builds user styles, which catch the
//! GPU's errors in an error scope, and wgpu 23 keeps one stack of those
//! per device, not per thread.)
//!
//! And a test can't hang for hours again: a watchdog aborts the binary,
//! naming the test, once any lease has been held past
//! `FLINT_GPU_TEST_TIMEOUT` seconds (default [`DEFAULT_TIMEOUT_S`]). It
//! aborts rather than exits because exiting needs the loader lock, which
//! is what a hung thread is holding.
//!
//! The tests share compiled pipelines too (#913): the device is made with
//! `flint_render::pipeline_cache` on, as `flint goldens` runs its cases
//! (#911), so what one test built the next one takes. Building pipelines
//! was most of these tests' time: of `tests/stylize.rs` run one test at a
//! time without it (RTX 4070, debug), 69% on Vulkan, 88% on DX12 with DXC
//! and 96% with FXC. `FLINT_GPU_TEST_PIPELINE_CACHE=0` runs a binary with
//! it off. The switch is the process's: a test that
//! wants pipelines built from nothing, or counts builds, switches it while
//! it holds the device, and its lease puts the harness's setting back when
//! it drops.
//!
//! One test at a time, because DX12 stalled tests running side by side:
//! wgpu-hal 23's DX12 `Device::wait` sets one auto-reset event per device
//! for every waiting thread, so when two tests are in `device.poll(Wait)`
//! at once, one wakes and the other sleeps out wgpu-core's 60 s timeout
//! (`CLEANUP_WAIT_MS`), holding the fence's read lock, and every other
//! test's `queue.submit` queues behind it. It cost a minute now and then
//! while the tests were mostly compiling; with pipelines shared it came
//! every minute, and `tests/stylize.rs` on DX12 took 400 s side by side
//! against 37 s in turn. Vulkan has no such wait, and takes 9-11 s either
//! way.
//! `FLINT_GPU_TEST_PARALLEL=1` lends the device alongside other tests as
//! before, for A/Bs; then a test sharing it that finds the cache's switch
//! moved fails, naming itself, since only a test alone may switch it.
//!
//! `FLINT_GPU_TEST_TIMES=1` prints a line per test as its lease drops: how
//! long it held the device, and how much of that went on building
//! pipelines (`pipeline_cache::thread_build_time`). CI's GPU job sets it.

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
/// turn back, putting the pipeline cache's switch back first.
pub struct Lease {
    pub gpu: &'static SharedGpu,
    _turn: Turn,
    _watch: Watch,
    since: Instant,
    built_before: Duration,
}

enum Turn {
    Shared(RwLockReadGuard<'static, ()>),
    Alone(RwLockWriteGuard<'static, ()>),
}

static GPU: OnceLock<Option<SharedGpu>> = OnceLock::new();
/// Write: a test's turn. Read: a test alongside others ([`parallel`]).
static TURNS: RwLock<()> = RwLock::new(());

/// Whether the tests share pipelines: on unless
/// `FLINT_GPU_TEST_PIPELINE_CACHE` is `0`, `off` or `false`.
pub fn pipeline_cache_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("FLINT_GPU_TEST_PIPELINE_CACHE")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "0" | "off" | "false"
        )
    })
}

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
            flint_render::pipeline_cache::set_enabled(pipeline_cache_on());
            start_watchdog();
        }
        made
    })
    .as_ref()
}

/// Whether [`lease`] lends the device alongside other tests, as it did
/// before #913: `FLINT_GPU_TEST_PARALLEL=1`.
pub fn parallel() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("FLINT_GPU_TEST_PARALLEL").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// The device, for this test's turn (or, with [`parallel`], alongside
/// whichever other tests hold a `lease`).
pub fn lease() -> Option<Lease> {
    if !parallel() {
        return lease_alone();
    }
    let gpu = device()?;
    let turn = Turn::Shared(TURNS.read().unwrap_or_else(|e| e.into_inner()));
    check_switch("took the device");
    Some(Lease::new(gpu, turn))
}

/// The device with no other test using it, whatever [`parallel`] says
/// (see the module docs for when a test needs this).
pub fn lease_alone() -> Option<Lease> {
    let gpu = device()?;
    let turn = Turn::Alone(TURNS.write().unwrap_or_else(|e| e.into_inner()));
    Some(Lease::new(gpu, turn))
}

/// Fail a test sharing the device that finds the pipeline cache switched
/// from the harness's setting: only a test alone on it may switch it.
fn check_switch(when: &str) {
    let (want, now) = (pipeline_cache_on(), flint_render::pipeline_cache::enabled());
    assert_eq!(
        now,
        want,
        "{} {when} sharing the device with the pipeline cache switched {} \
         (the harness has it {}): only a test holding it alone \
         (lease_alone) may switch it (#913)",
        std::thread::current().name().unwrap_or("<unnamed>"),
        if now { "on" } else { "off" },
        if want { "on" } else { "off" },
    );
}

impl Lease {
    fn new(gpu: &'static SharedGpu, turn: Turn) -> Self {
        Lease {
            gpu,
            _turn: turn,
            _watch: Watch::start(),
            since: Instant::now(),
            built_before: flint_render::pipeline_cache::thread_build_time(),
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        match self._turn {
            // Still alone: whatever the test switched, the next one finds
            // the harness's setting
            Turn::Alone(_) => {
                if flint_render::pipeline_cache::enabled() != pipeline_cache_on() {
                    flint_render::pipeline_cache::set_enabled(pipeline_cache_on());
                }
            }
            Turn::Shared(_) => {
                if !std::thread::panicking() {
                    check_switch("gave the device back");
                }
            }
        }
        if std::env::var_os("FLINT_GPU_TEST_TIMES").is_some_and(|v| v != "0") {
            let held = self.since.elapsed().as_secs_f64();
            let building = (flint_render::pipeline_cache::thread_build_time() - self.built_before)
                .as_secs_f64();
            // Past libtest's capture, as the watchdog writes
            let _ = writeln!(
                std::io::stderr().lock(),
                "gpu-test-time {}: held {held:.2} s, building pipelines {building:.2} s \
                 (pipeline cache {}, {})",
                std::thread::current().name().unwrap_or("<unnamed>"),
                if pipeline_cache_on() { "on" } else { "off" },
                if parallel() { "parallel" } else { "in turn" },
            );
        }
    }
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
