//! Profiler scopes: one macro, two sinks (ADR 0128).
//!
//! `flint_core::scope!("name")` marks the rest of the enclosing block. It
//! reaches two places from one line:
//!
//! - **Tracy**, through `profiling::scope!`, when a build turns on
//!   `profiling/profile-with-tracy` (flint-player's `profile` feature, ADR
//!   0109). Without it that half is nothing.
//! - **Flint's own recorder**: a ring of [`Record`]s per thread, which a host
//!   can save after the fact. Compiled in under `debug_assertions` or this
//!   crate's `profiler` feature, and recording only while [`set_recording`]
//!   says so. Without either, that half is nothing too.
//!
//! The gate is evaluated here, in flint-core, not in the crate that writes
//! `scope!`: the macro is defined twice under `cfg`, so a crate that scopes
//! its code needs no feature of its own and no dependency but flint-core.
//!
//! ```
//! fn step() {
//!     flint_core::scope!("physics::step");
//!     for _ in 0..3 {
//!         flint_core::scope!("physics::substep");
//!     }
//!     flint_core::scope!("load_model", "models/oak.glb"); // a detail, as text
//! }
//! # step();
//! ```

/// Where a scope was written. One `static` per `scope!` call site, so a
/// record names its scope with one pointer.
#[derive(Debug)]
pub struct Site {
    pub name: &'static str,
    pub file: &'static str,
    pub line: u32,
}

#[doc(hidden)]
pub use profiling as __profiling;

/// Marks the rest of the enclosing block as a scope named by a string
/// literal, with an optional detail: text (an asset's file name) or a count.
/// See the module docs.
#[cfg(any(debug_assertions, feature = "profiler"))]
#[macro_export]
macro_rules! scope {
    ($name:literal) => {
        let _flint_scope = {
            static SITE: $crate::profile::Site = $crate::profile::Site {
                name: $name,
                file: file!(),
                line: line!(),
            };
            $crate::profile::Scope::enter(&SITE)
        };
        $crate::profile::__profiling::scope!($name);
    };
    ($name:literal, $detail:expr) => {
        let _flint_detail = $detail;
        let _flint_scope = {
            static SITE: $crate::profile::Site = $crate::profile::Site {
                name: $name,
                file: file!(),
                line: line!(),
            };
            $crate::profile::Scope::enter_with(&SITE, &_flint_detail)
        };
        $crate::profile::__profiling::scope!(
            $name,
            $crate::profile::__tracy_text(&_flint_detail).as_ref()
        );
    };
}

/// Without the recorder: only Tracy's half, which is itself nothing unless
/// the build turns Tracy on.
#[cfg(not(any(debug_assertions, feature = "profiler")))]
#[macro_export]
macro_rules! scope {
    ($name:literal) => {
        $crate::profile::__profiling::scope!($name);
    };
    ($name:literal, $detail:expr) => {
        let _flint_detail = $detail;
        $crate::profile::__profiling::scope!(
            $name,
            $crate::profile::__tracy_text(&_flint_detail).as_ref()
        );
    };
}

/// A detail as Tracy's zone text.
#[doc(hidden)]
pub fn __tracy_text<D: IntoDetail>(detail: &D) -> std::borrow::Cow<'static, str> {
    match detail.into_detail() {
        DetailValue::Text(s) => std::borrow::Cow::Owned(s.to_string()),
        DetailValue::Count(n) => std::borrow::Cow::Owned(n.to_string()),
    }
}

/// What a scope's detail can be.
pub enum DetailValue<'a> {
    Text(&'a str),
    Count(u64),
}

pub trait IntoDetail {
    fn into_detail(&self) -> DetailValue<'_>;
}
impl IntoDetail for &str {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl IntoDetail for String {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl IntoDetail for &String {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl IntoDetail for usize {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self as u64)
    }
}
impl IntoDetail for u32 {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self as u64)
    }
}
impl IntoDetail for u64 {
    fn into_detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self)
    }
}

/// Marks a frame's end: Tracy's frame mark, and the recorder's frame index.
pub fn finish_frame() {
    profiling::finish_frame!();
    #[cfg(any(debug_assertions, feature = "profiler"))]
    recorder::next_frame();
}

#[cfg(any(debug_assertions, feature = "profiler"))]
pub use recorder::*;

/// The recorder's half is compiled out: a scope is a zero-sized value.
#[cfg(not(any(debug_assertions, feature = "profiler")))]
mod off {
    pub fn set_recording(_on: bool) {}
    pub fn recording() -> bool {
        false
    }
    pub const COMPILED_IN: bool = false;
}
#[cfg(not(any(debug_assertions, feature = "profiler")))]
pub use off::*;

#[cfg(any(debug_assertions, feature = "profiler"))]
mod recorder {
    use super::{DetailValue, IntoDetail, Site};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    /// The recorder is in this build.
    pub const COMPILED_IN: bool = true;

    static RECORDING: AtomicBool = AtomicBool::new(false);
    static FRAME: AtomicU32 = AtomicU32::new(0);

    /// The scopes' clock. On x86_64 the time-stamp counter, as Tracy's is
    /// (an invariant TSC, which every x86_64 CPU Windows 11 runs on has; QPC
    /// is built on it): two reads cost about 14 ns against `Instant::now`'s
    /// 47 (ADR 0128, measured). Ticks become nanoseconds when read out,
    /// calibrated against `Instant` over the whole run. Elsewhere, and on the
    /// web, `web_time::Instant` in nanoseconds from the epoch.
    pub mod clock {
        use std::sync::OnceLock;
        use web_time::Instant;

        struct Epoch {
            instant: Instant,
            #[cfg(target_arch = "x86_64")]
            tsc: u64,
        }
        static EPOCH: OnceLock<Epoch> = OnceLock::new();

        fn epoch() -> &'static Epoch {
            EPOCH.get_or_init(|| Epoch {
                instant: Instant::now(),
                #[cfg(target_arch = "x86_64")]
                tsc: tsc(),
            })
        }

        #[cfg(target_arch = "x86_64")]
        #[inline(always)]
        fn tsc() -> u64 {
            // SAFETY: rdtsc has no preconditions on x86_64.
            unsafe { core::arch::x86_64::_rdtsc() }
        }

        /// Starts the clock (the epoch). Called by `set_recording`.
        pub fn start() {
            epoch();
        }

        /// Ticks now.
        #[inline(always)]
        pub fn now() -> u64 {
            #[cfg(target_arch = "x86_64")]
            {
                tsc()
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                epoch().instant.elapsed().as_nanos() as u64
            }
        }

        /// Nanoseconds per tick, measured over the run so far.
        pub fn ns_per_tick() -> f64 {
            #[cfg(target_arch = "x86_64")]
            {
                let e = epoch();
                let ns = e.instant.elapsed().as_nanos() as f64;
                let ticks = tsc().wrapping_sub(e.tsc) as f64;
                if ticks > 0.0 && ns > 0.0 {
                    ns / ticks
                } else {
                    1.0
                }
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                1.0
            }
        }

        /// A tick count from [`now`] as nanoseconds from the epoch.
        pub fn since_epoch_ns(ticks: u64, ns_per_tick: f64) -> u64 {
            #[cfg(target_arch = "x86_64")]
            {
                (ticks.saturating_sub(epoch().tsc) as f64 * ns_per_tick) as u64
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = ns_per_tick;
                ticks
            }
        }
    }
    /// Records a thread's ring holds before the oldest is overwritten.
    pub static RING_CAPACITY: AtomicU32 = AtomicU32::new(1 << 16);
    /// Bytes of detail text a thread's ring holds.
    const TEXT_CAPACITY: usize = 64 * 1024;

    /// Turns recording on or off for every thread. Off, a scope costs one
    /// relaxed load and a branch.
    pub fn set_recording(on: bool) {
        clock::start();
        RECORDING.store(on, Ordering::Relaxed);
    }

    pub fn recording() -> bool {
        RECORDING.load(Ordering::Relaxed)
    }

    pub(super) fn next_frame() {
        FRAME.fetch_add(1, Ordering::Relaxed);
    }

    /// The frame index records are stamped with now.
    pub fn frame() -> u32 {
        FRAME.load(Ordering::Relaxed)
    }

    /// One finished scope. 40 bytes.
    #[derive(Clone, Copy, Debug)]
    pub struct Record {
        pub site: &'static Site,
        /// Clock ticks ([`clock::now`]); [`Record::start_ns`] converts.
        pub start: u64,
        pub dur: u64,
        /// [`frame`] when the scope closed.
        pub frame: u32,
        /// 0 for a scope with no open scope around it on its thread.
        pub depth: u16,
        detail: Detail,
    }

    #[derive(Clone, Copy, Debug)]
    enum Detail {
        None,
        Count(u64),
        /// `at` is the absolute write position in the thread's text ring.
        Text {
            at: u64,
            len: u16,
        },
    }

    /// A thread's records, oldest overwritten first.
    pub struct Ring {
        pub thread: String,
        records: Vec<Record>,
        next: usize,
        written: u64,
        text: Vec<u8>,
        text_written: u64,
    }

    impl Ring {
        fn new(thread: String) -> Self {
            Ring {
                thread,
                records: Vec::new(),
                next: 0,
                written: 0,
                text: Vec::new(),
                text_written: 0,
            }
        }

        fn push(&mut self, r: Record) {
            let cap = RING_CAPACITY.load(Ordering::Relaxed).max(1) as usize;
            if self.records.len() < cap {
                // Grows to its capacity once, then never again.
                if self.records.capacity() == 0 {
                    self.records.reserve_exact(cap);
                }
                self.records.push(r);
            } else {
                self.records[self.next] = r;
            }
            self.next += 1;
            if self.next >= cap {
                self.next = 0;
            }
            self.written += 1;
        }

        fn push_text(&mut self, s: &str) -> Detail {
            if self.text.is_empty() {
                self.text.resize(TEXT_CAPACITY, 0);
            }
            let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize).min(TEXT_CAPACITY)];
            let at = self.text_written;
            for (i, b) in bytes.iter().enumerate() {
                self.text[((at + i as u64) % TEXT_CAPACITY as u64) as usize] = *b;
            }
            self.text_written += bytes.len() as u64;
            Detail::Text {
                at,
                len: bytes.len() as u16,
            }
        }

        /// The records, oldest first.
        pub fn records(&self) -> impl Iterator<Item = &Record> {
            let (a, b) = if self.written <= self.records.len() as u64 || self.next == 0 {
                (&self.records[..], &self.records[..0])
            } else {
                let (tail, head) = self.records.split_at(self.next);
                (head, tail)
            };
            a.iter().chain(b.iter())
        }

        /// A record's detail as text, if it had one and it hasn't been
        /// overwritten since.
        pub fn detail(&self, r: &Record) -> Option<String> {
            match r.detail {
                Detail::None => None,
                Detail::Count(n) => Some(n.to_string()),
                Detail::Text { at, len } => {
                    if self.text_written - at > TEXT_CAPACITY as u64 {
                        return None;
                    }
                    let bytes: Vec<u8> = (0..len as u64)
                        .map(|i| self.text[((at + i) % TEXT_CAPACITY as u64) as usize])
                        .collect();
                    Some(String::from_utf8_lossy(&bytes).into_owned())
                }
            }
        }

        /// Records pushed since the start, overwritten or not.
        pub fn written(&self) -> u64 {
            self.written
        }
    }

    fn rings() -> &'static Mutex<Vec<Arc<Mutex<Ring>>>> {
        static RINGS: OnceLock<Mutex<Vec<Arc<Mutex<Ring>>>>> = OnceLock::new();
        RINGS.get_or_init(|| Mutex::new(Vec::new()))
    }

    thread_local! {
        static DEPTH: Cell<u16> = const { Cell::new(0) };
        static RING: Arc<Mutex<Ring>> = {
            let name = std::thread::current()
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{:?}", std::thread::current().id()));
            let ring = Arc::new(Mutex::new(Ring::new(name)));
            if let Ok(mut all) = rings().lock() {
                all.push(ring.clone());
            }
            ring
        };
    }

    /// Calls `f` with every thread's ring that has recorded anything.
    pub fn with_rings(mut f: impl FnMut(&Ring)) {
        let all: Vec<_> = rings().lock().map(|v| v.clone()).unwrap_or_default();
        for ring in all {
            if let Ok(r) = ring.lock() {
                f(&r);
            }
        }
    }

    /// An open scope: recorded when it drops.
    pub struct Scope {
        site: &'static Site,
        start: Option<u64>,
        detail: Detail,
    }

    impl Scope {
        #[inline]
        pub fn enter(site: &'static Site) -> Scope {
            if !RECORDING.load(Ordering::Relaxed) {
                return Scope {
                    site,
                    start: None,
                    detail: Detail::None,
                };
            }
            DEPTH.with(|d| d.set(d.get() + 1));
            Scope {
                site,
                start: Some(clock::now()),
                detail: Detail::None,
            }
        }

        #[inline]
        pub fn enter_with<D: IntoDetail>(site: &'static Site, detail: &D) -> Scope {
            let mut s = Scope::enter(site);
            if s.start.is_some() {
                s.detail = match detail.into_detail() {
                    DetailValue::Count(n) => Detail::Count(n),
                    DetailValue::Text(t) => {
                        RING.with(|r| r.lock().map(|mut r| r.push_text(t)).unwrap_or(Detail::None))
                    }
                };
            }
            s
        }
    }

    impl Drop for Scope {
        #[inline]
        fn drop(&mut self) {
            if let Some(start) = self.start {
                record(self.site, start, self.detail);
            }
        }
    }

    #[inline(never)]
    fn record(site: &'static Site, start: u64, detail: Detail) {
        let end = clock::now();
        let depth = DEPTH.with(|d| {
            let v = d.get().saturating_sub(1);
            d.set(v);
            v
        });
        let r = Record {
            site,
            start,
            dur: end.saturating_sub(start),
            frame: FRAME.load(Ordering::Relaxed),
            depth,
            detail,
        };
        RING.with(|ring| {
            if let Ok(mut ring) = ring.lock() {
                ring.push(r);
            }
        });
    }

    /// Every ring as Chrome trace-event JSON (opens in Perfetto and
    /// chrome://tracing): one `X` event per record, a thread-name record per
    /// ring. The spike's writer; #1097 makes the real one.
    pub fn chrome_json() -> String {
        fn esc(s: &str) -> String {
            s.replace('\\', "\\\\").replace('"', "\\\"")
        }
        let mut out = String::from("{\"displayTimeUnit\":\"ns\",\"traceEvents\":[\n");
        let mut first = true;
        let mut tid = 0;
        let k = clock::ns_per_tick();
        with_rings(|ring| {
            tid += 1;
            if !first {
                out.push_str(",\n");
            }
            first = false;
            out.push_str(&format!(
                "{{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":{tid},\"args\":{{\"name\":\"{}\"}}}}",
                esc(&ring.thread)
            ));
            for r in ring.records() {
                let detail = ring
                    .detail(r)
                    .map(|d| format!(",\"detail\":\"{}\"", esc(&d)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    ",\n{{\"ph\":\"X\",\"name\":\"{}\",\"pid\":1,\"tid\":{tid},\"ts\":{:.3},\"dur\":{:.3},\"args\":{{\"frame\":{},\"depth\":{}{detail}}}}}",
                    esc(r.site.name),
                    clock::since_epoch_ns(r.start, k) as f64 / 1000.0,
                    r.dur as f64 * k / 1000.0,
                    r.frame,
                    r.depth,
                ));
            }
        });
        out.push_str("\n]}\n");
        out
    }
}

#[cfg(all(test, any(debug_assertions, feature = "profiler")))]
mod tests {
    use super::*;

    #[test]
    fn nested_scopes_record_depth_and_detail() {
        set_recording(true);
        std::thread::Builder::new()
            .name("profile-test".into())
            .spawn(|| {
                crate::scope!("outer");
                {
                    crate::scope!("inner", "models/oak.glb");
                }
                {
                    crate::scope!("counted", 42usize);
                }
            })
            .unwrap()
            .join()
            .unwrap();
        let mut seen = Vec::new();
        with_rings(|ring| {
            if ring.thread == "profile-test" {
                for r in ring.records() {
                    seen.push((r.site.name, r.depth, ring.detail(r)));
                }
            }
        });
        assert_eq!(
            seen,
            vec![
                ("inner", 1, Some("models/oak.glb".to_string())),
                ("counted", 1, Some("42".to_string())),
                ("outer", 0, None),
            ]
        );
        let json = chrome_json();
        assert!(json.contains("\"name\":\"profile-test\""));
    }
}
