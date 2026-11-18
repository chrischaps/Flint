//! One GPU device for a whole test binary, shared by its tests (#598).
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
//! [`lease`] lends it to a test, alongside the other tests holding a
//! `lease` ([`parallel`], the default) or one test at a time, and
//! [`lease_alone`] lends it to one test while every other waits. A test
//! takes it alone only when another test's work would change what it
//! sees: one that counts objects across the whole device
//! (`Instance::generate_report`), or switches something that is the
//! process's (the pipeline cache's switch, `dev_shader`'s folder). Error
//! scopes are no reason: they have been the thread's since wgpu 28, so a
//! test that builds user styles or draws in a scope shares the device.
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
//! Side by side again since #948. #913 lent the device one test at a time
//! because DX12 stalled tests running side by side on wgpu 23: wgpu-hal
//! 23's DX12 `Device::wait` armed one auto-reset event per device for
//! every waiting thread, so of two tests in `device.poll(Wait)` one slept
//! out wgpu-core's 60 s `CLEANUP_WAIT_MS` holding the fence's read lock,
//! and every other test's `queue.submit` queued behind it. wgpu-hal 30
//! makes an event per wait and wgpu-core 30 has no 60 s cap, and
//! side by side is the faster harness on every backend (#948's PR has the
//! A/B). `FLINT_GPU_TEST_PARALLEL=0` lends the device one test at a time,
//! for an A/B or to rule sharing out; `1` asks for side by side. A test
//! sharing the device that finds the cache's switch moved fails, naming
//! itself, since only a test alone may switch it.
//!
//! `FLINT_GPU_TEST_TIMES=1` prints a line per test as its lease drops: how
//! long it held the device, and how much of that went on building
//! pipelines (`pipeline_cache::thread_build_time`). CI's GPU job sets it.
//!
//! A lost device says so (#560). On wgpu 23, with a device per test, two
//! tests of `tests/stylize.rs` once panicked together at
//! `wgpu-core-23.0.1/src/resource.rs:749`. That line is `Buffer::destroy`
//! taking the device's snatch lock to write, and the only way there from a
//! test's draw was the device being lost: a HAL error (`Lost`,
//! `Unexpected`, `ResourceCreationFailed`) met while the thread held the
//! snatch lock to read (a submit, a map) made wgpu-core 23's
//! `Device::lose` call `release_gpu_resources`, which destroys every
//! buffer and so took the lock again on the same thread, and its
//! debug-build guard panicked ("attempted to acquire a snatch lock
//! recursively"). So the panic was a device loss with its message
//! replaced. wgpu-core 30's `lose` releases nothing; `maintain` does, after
//! dropping its snatch guard, and the tests make one device, not one each.
//! What's left is to make a loss name itself: the device's lost callback
//! records why and which tests held it, every test that holds it then or
//! takes it later fails with that line, and the first failure is the one
//! to read.

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
/// Why the device was lost, and who held it then, once it has been (#560).
static LOST: OnceLock<String> = OnceLock::new();
/// Write: a test's turn. Read: a test alongside others ([`parallel`]).
static TURNS: RwLock<()> = RwLock::new(());

/// Whether the tests share pipelines: on unless
/// `FLINT_GPU_TEST_PIPELINE_CACHE` is `0`, `off` or `false`.
pub fn pipeline_cache_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        flint_core::env::GPU_TEST_PIPELINE_CACHE
            .flag()
            .unwrap_or(true)
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
            device.set_device_lost_callback(note_lost);
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

/// Whether [`lease`] lends the device alongside other tests when
/// `FLINT_GPU_TEST_PARALLEL` doesn't say.
pub const PARALLEL_BY_DEFAULT: bool = true;

/// Whether [`lease`] lends the device alongside other tests:
/// `FLINT_GPU_TEST_PARALLEL=1` yes, `0` (or `off`, `false`) one at a time,
/// unset [`PARALLEL_BY_DEFAULT`].
pub fn parallel() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        flint_core::env::GPU_TEST_PARALLEL
            .flag()
            .unwrap_or(PARALLEL_BY_DEFAULT)
    })
}

/// The device, alongside whichever other tests hold a `lease` (or, with
/// [`parallel`] off, for this test's turn alone).
pub fn lease() -> Option<Lease> {
    if !parallel() {
        return lease_alone();
    }
    let gpu = device()?;
    let turn = Turn::Shared(TURNS.read().unwrap_or_else(|e| e.into_inner()));
    check_lost("took the device");
    check_switch("took the device");
    Some(Lease::new(gpu, turn))
}

/// The device with no other test using it, whatever [`parallel`] says
/// (see the module docs for when a test needs this).
pub fn lease_alone() -> Option<Lease> {
    let gpu = device()?;
    let turn = Turn::Alone(TURNS.write().unwrap_or_else(|e| e.into_inner()));
    check_lost("took the device");
    Some(Lease::new(gpu, turn))
}

/// The device's lost callback: keep why, with the tests holding it, and
/// say it past libtest's capture. It may run on a thread inside wgpu-core
/// holding the device's locks, so it touches nothing of wgpu's.
fn note_lost(reason: wgpu::DeviceLostReason, message: String) {
    let held: Vec<String> = HELD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(name, _)| name.clone())
        .collect();
    let line = format!(
        "the shared GPU device was lost ({reason:?}{}) while {} held it",
        if message.is_empty() {
            String::new()
        } else {
            format!(": {message}")
        },
        if held.is_empty() {
            "no test".to_string()
        } else {
            held.join(", ")
        },
    );
    let _ = writeln!(std::io::stderr().lock(), "\nshared_gpu (#560): {line}");
    let _ = LOST.set(line);
}

/// Fail a test that takes or held a device that has been lost: what it
/// drew or read back is no evidence of anything.
fn check_lost(when: &str) {
    if let Some(lost) = LOST.get() {
        panic!(
            "{} {when}, but {lost}; every test that uses it after that fails \
             too, so the first failure is the one to read (#560)",
            std::thread::current().name().unwrap_or("<unnamed>"),
        );
    }
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
        if !std::thread::panicking() {
            check_lost("gave the device back");
        }
        if flint_core::env::GPU_TEST_TIMES.flag() == Some(true) {
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
    let secs = flint_core::env::GPU_TEST_TIMEOUT
        .u32()
        .map_or(DEFAULT_TIMEOUT_S, u64::from);
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
