//! Profiler scopes: one macro, two sinks (ADR 0128).
//!
//! `flint_core::scope!("name")` marks the rest of the enclosing block. One
//! line reaches two places:
//!
//! - **Tracy**, through `profiling::scope!`, when the build turns on
//!   `profiling/profile-with-tracy` (flint-player's `profile` feature, ADR
//!   0109). Without it that half is nothing.
//! - **Flint's own recorder**: a ring of [`Record`]s per thread, which a host
//!   reads after the fact ([`with_rings`]; the trace file is #1097's).
//!   Compiled in under `debug_assertions` or this crate's `trace` feature, and
//!   recording only after [`set_recording`]`(true)`: until then a scope is one
//!   relaxed load and a branch. Without either, that half is nothing too.
//!
//! The gate is decided here, in flint-core, not in the crate that writes
//! `scope!`: the macro is defined twice under `cfg`, so a crate that scopes
//! its code needs no feature of its own and no dependency but flint-core.
//!
//! ```
//! fn step(draws: &[u32], model: &str) {
//!     flint_core::scope!("physics::step");
//!     for _ in 0..3 {
//!         flint_core::scope!("physics::substep");
//!     }
//!     flint_core::scope!("cull", draws.len()); // a count, the cheap detail
//!     flint_core::scope!("load_model", model); // text, copied: rare scopes only
//! }
//! # step(&[1, 2], "models/oak.glb");
//! ```

/// Where a scope was written: one `static` per `scope!` call site, so a
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
/// literal, with an optional detail: a count (`usize`, `u32`, `u64`) or text
/// (`&str`, `String`). Reaches Tracy and Flint's recorder; see
/// [`crate::profile`].
#[cfg(any(debug_assertions, feature = "trace"))]
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

/// Marks the rest of the enclosing block as a scope. This build has no
/// recorder, so it is Tracy's half alone, which is nothing unless the build
/// turns Tracy on.
#[cfg(not(any(debug_assertions, feature = "trace")))]
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

/// A detail as Tracy's zone text. Only reached when Tracy is on.
#[doc(hidden)]
pub fn __tracy_text<D: ScopeDetail>(detail: &D) -> std::borrow::Cow<'static, str> {
    match detail.detail() {
        DetailValue::Text(s) => std::borrow::Cow::Owned(s.to_string()),
        DetailValue::Count(n) => std::borrow::Cow::Owned(n.to_string()),
    }
}

/// What a scope's detail is.
pub enum DetailValue<'a> {
    Text(&'a str),
    Count(u64),
}

/// A value `scope!` takes as its detail.
pub trait ScopeDetail {
    fn detail(&self) -> DetailValue<'_>;
}
impl ScopeDetail for &str {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl ScopeDetail for String {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl ScopeDetail for &String {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self)
    }
}
impl ScopeDetail for usize {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self as u64)
    }
}
impl ScopeDetail for u32 {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self as u64)
    }
}
impl ScopeDetail for u64 {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Count(*self)
    }
}

/// A frame's end: Tracy's frame mark, and the recorder's frame index.
pub fn finish_frame() {
    profiling::finish_frame!();
    #[cfg(any(debug_assertions, feature = "trace"))]
    recorder::next_frame();
}

#[cfg(any(debug_assertions, feature = "trace"))]
pub use recorder::*;

/// The recorder's file format, Chrome trace-event JSON (#1097). Plain
/// data and text, in every build, so a host writes one code path and asks
/// [`COMPILED_IN`] (a constant: without the recorder the optimizer drops
/// the path).
pub mod chrome;

/// A scope copied out of a ring by `snapshot`, in nanoseconds from the
/// recorder's epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct TakenScope {
    pub name: &'static str,
    /// Where `scope!` was written; empty for a site named at run time.
    pub file: &'static str,
    pub line: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
    pub frame: u32,
    pub depth: u16,
    pub detail: Option<TakenDetail>,
}

/// A taken scope's detail.
#[derive(Clone, Debug, PartialEq)]
pub enum TakenDetail {
    Count(u64),
    Text(String),
}

/// One thread's scopes, as `snapshot` copied them, oldest first by when
/// they closed.
#[derive(Clone, Debug, Default)]
pub struct ThreadScopes {
    pub thread: String,
    pub scopes: Vec<TakenScope>,
    /// Records the ring held, copied or not.
    pub held: usize,
    /// Records pushed since the ring was made.
    pub written: u64,
    /// Its capacity; `None` keeps every record.
    pub capacity: Option<usize>,
}

impl ThreadScopes {
    /// Whether the ring has overwritten anything: then its oldest held
    /// record is where the thread's record begins.
    pub fn wrapped(&self) -> bool {
        self.written > self.held as u64
    }
}

#[cfg(not(any(debug_assertions, feature = "trace")))]
pub use off::*;

/// The recorder's half is compiled out of this build: the same names, each
/// nothing, so a host needs no `cfg` of its own.
#[cfg(not(any(debug_assertions, feature = "trace")))]
mod off {
    /// Whether the recorder is in this build.
    pub const COMPILED_IN: bool = false;

    /// Records a ring holds by default, as the recorder's build says.
    pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;

    /// Nothing to size: the recorder is not in this build.
    pub fn set_ring_capacity(_records: usize) {}

    pub fn ring_capacity() -> usize {
        0
    }

    /// Always 0: no frames are counted without the recorder.
    pub fn frame() -> u32 {
        0
    }

    /// Nothing recorded, nothing to copy.
    pub fn snapshot(_since_ns: Option<u64>) -> Vec<super::ThreadScopes> {
        Vec::new()
    }

    /// The recorder's clock, which this build has none of.
    pub mod clock {
        use web_time::Instant;

        pub fn instant_ns(_at: Instant) -> u64 {
            0
        }

        pub fn now_ns() -> u64 {
            0
        }
    }

    /// Does nothing: the recorder is not in this build.
    pub fn set_recording(_on: bool) {}

    /// Always false: the recorder is not in this build.
    #[inline(always)]
    pub fn recording() -> bool {
        false
    }

    /// A scope named at run time ([`site_named`]): nothing in this build.
    pub struct Scope;

    impl Scope {
        #[inline(always)]
        pub fn enter(_site: &'static super::Site) -> Scope {
            Scope
        }
    }

    /// The one site of a name made at run time. With no recorder in the
    /// build there is nothing to name, so every name is one empty site;
    /// callers ask only while [`recording`], which is never here.
    pub fn site_named(_name: &str) -> &'static super::Site {
        static NONE: super::Site = super::Site {
            name: "",
            file: "",
            line: 0,
        };
        &NONE
    }
}

#[cfg(any(debug_assertions, feature = "trace"))]
mod recorder {
    use super::{DetailValue, ScopeDetail, Site, TakenDetail, TakenScope, ThreadScopes};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Whether the recorder is in this build.
    pub const COMPILED_IN: bool = true;

    static RECORDING: AtomicBool = AtomicBool::new(false);
    static FRAME: AtomicU32 = AtomicU32::new(0);
    static RING_CAPACITY: AtomicU32 = AtomicU32::new(DEFAULT_RING_CAPACITY as u32);

    /// Records a thread's ring holds unless [`set_ring_capacity`] says
    /// otherwise: 65 536, 2.6 MB a thread.
    pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;
    /// Bytes of detail text a thread's ring holds.
    const TEXT_CAPACITY: usize = 64 * 1024;

    /// Turns recording on or off for every thread.
    pub fn set_recording(on: bool) {
        clock::start();
        RECORDING.store(on, Ordering::Relaxed);
    }

    #[inline]
    pub fn recording() -> bool {
        RECORDING.load(Ordering::Relaxed)
    }

    /// The records a thread's ring will hold, for rings not yet made: set it
    /// before recording starts. `0` keeps every record (a whole session,
    /// `--trace-seconds 0`): the ring and its text then grow without bound.
    pub fn set_ring_capacity(records: usize) {
        RING_CAPACITY.store(records.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    /// What [`set_ring_capacity`] last said: the records a new thread's
    /// ring holds, `0` for every record.
    pub fn ring_capacity() -> usize {
        RING_CAPACITY.load(Ordering::Relaxed) as usize
    }

    /// The site of a scope named at run time (a script's callback, #1097),
    /// made once a name and kept for the run: a name made at run time
    /// can't be a `static` as `scope!`'s literal is. Ask only while
    /// [`recording`]: it takes a lock and, the first time, allocates.
    pub fn site_named(name: &str) -> &'static Site {
        use std::collections::HashMap;
        static SITES: OnceLock<Mutex<HashMap<String, &'static Site>>> = OnceLock::new();
        let mut sites = SITES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(site) = sites.get(name) {
            return site;
        }
        let site: &'static Site = Box::leak(Box::new(Site {
            name: Box::leak(name.to_owned().into_boxed_str()),
            file: "",
            line: 0,
        }));
        sites.insert(name.to_owned(), site);
        site
    }

    pub(super) fn next_frame() {
        FRAME.fetch_add(1, Ordering::Relaxed);
    }

    /// The frame index records are stamped with now: how many times
    /// [`finish_frame`](super::finish_frame) has run.
    pub fn frame() -> u32 {
        FRAME.load(Ordering::Relaxed)
    }

    /// The scopes' clock. On x86_64 the time-stamp counter, as Tracy's is (an
    /// invariant TSC, which every x86_64 CPU Windows 11 runs on has, and which
    /// QPC reads too): two reads cost about 14 ns against `Instant::now`'s 47
    /// (ADR 0128). Ticks become nanoseconds when read out, calibrated against
    /// `Instant` over the whole run ([`ns_per_tick`]). Elsewhere, and on the
    /// web, a tick is a nanosecond of `web_time::Instant`.
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

        /// Fixes the epoch, if it isn't yet. `set_recording` calls it.
        pub fn start() {
            epoch();
        }

        /// `at` as nanoseconds from the epoch (0 before it): how a host
        /// puts its own instants on the scopes' timeline.
        pub fn instant_ns(at: Instant) -> u64 {
            at.saturating_duration_since(epoch().instant).as_nanos() as u64
        }

        /// Nanoseconds from the epoch to now, on `Instant`'s clock.
        pub fn now_ns() -> u64 {
            instant_ns(Instant::now())
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

        /// Nanoseconds a tick, measured from the epoch to now.
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

        /// A reading of [`now`] as nanoseconds from the epoch.
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

    /// One finished scope: 40 bytes.
    #[derive(Clone, Copy, Debug)]
    pub struct Record {
        pub site: &'static Site,
        /// Clock ticks ([`clock::now`]); [`Record::start_ns`] converts.
        pub start: u64,
        /// Ticks; [`Record::dur_ns`] converts.
        pub dur: u64,
        /// [`frame`] when the scope closed.
        pub frame: u32,
        /// Scopes open around it on its thread: 0 for the outermost.
        pub depth: u16,
        detail: Detail,
    }

    impl Record {
        /// Nanoseconds from the epoch, given [`clock::ns_per_tick`] (read it
        /// once for a whole read-out).
        pub fn start_ns(&self, ns_per_tick: f64) -> u64 {
            clock::since_epoch_ns(self.start, ns_per_tick)
        }

        pub fn dur_ns(&self, ns_per_tick: f64) -> u64 {
            (self.dur as f64 * ns_per_tick) as u64
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Detail {
        None,
        Count(u64),
        /// `at` is the absolute write position in the ring's text.
        Text {
            at: u64,
            len: u16,
        },
    }

    /// One thread's records, the oldest overwritten first. Its memory is
    /// taken once, at its first record. A ring made with capacity 0 keeps
    /// every record instead, and grows.
    pub struct Ring {
        thread: String,
        /// Records held; 0 for every record.
        cap: usize,
        records: Vec<Record>,
        next: usize,
        written: u64,
        text: Vec<u8>,
        text_written: u64,
    }

    impl Ring {
        fn new(thread: String, cap: usize) -> Self {
            Ring {
                thread,
                cap,
                records: Vec::new(),
                next: 0,
                written: 0,
                text: Vec::new(),
                text_written: 0,
            }
        }

        /// The thread's name (or its id, unnamed).
        pub fn thread(&self) -> &str {
            &self.thread
        }

        /// The records it holds at most; `None` for every record.
        pub fn capacity(&self) -> Option<usize> {
            (self.cap > 0).then_some(self.cap)
        }

        fn unbounded(&self) -> bool {
            self.cap == 0
        }

        fn push(&mut self, r: Record) {
            if self.unbounded() {
                self.records.push(r);
                self.written += 1;
                return;
            }
            if self.records.len() < self.cap {
                if self.records.capacity() == 0 {
                    self.records.reserve_exact(self.cap);
                }
                self.records.push(r);
            } else {
                self.records[self.next] = r;
            }
            self.next += 1;
            if self.next == self.cap {
                self.next = 0;
            }
            self.written += 1;
        }

        fn push_text(&mut self, s: &str) -> Detail {
            if self.unbounded() {
                let len = s.len().min(u16::MAX as usize);
                let at = self.text.len() as u64;
                self.text.extend_from_slice(&s.as_bytes()[..len]);
                self.text_written += len as u64;
                return Detail::Text {
                    at,
                    len: len as u16,
                };
            }
            if self.text.is_empty() {
                self.text.resize(TEXT_CAPACITY, 0);
            }
            let len = s.len().min(u16::MAX as usize).min(TEXT_CAPACITY);
            let at = self.text_written;
            for (i, b) in s.as_bytes()[..len].iter().enumerate() {
                self.text[((at + i as u64) % TEXT_CAPACITY as u64) as usize] = *b;
            }
            self.text_written += len as u64;
            Detail::Text {
                at,
                len: len as u16,
            }
        }

        /// The records held, oldest first.
        pub fn records(&self) -> impl Iterator<Item = &Record> {
            let (older, newer) = if self.unbounded() || self.records.len() < self.cap {
                (&self.records[..], &self.records[..0])
            } else {
                let (newer, older) = self.records.split_at(self.next);
                (older, newer)
            };
            older.iter().chain(newer.iter())
        }

        /// A record's detail as text: its count, or its text unless that has
        /// been overwritten since.
        pub fn detail(&self, r: &Record) -> Option<String> {
            match r.detail {
                Detail::None => None,
                Detail::Count(n) => Some(n.to_string()),
                Detail::Text { at, len } if self.unbounded() => {
                    let bytes = &self.text[at as usize..at as usize + len as usize];
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
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

        /// Records pushed since the ring was made, overwritten or not.
        pub fn written(&self) -> u64 {
            self.written
        }

        /// Records it holds now.
        pub fn held(&self) -> usize {
            self.records.len()
        }

        /// What a detail is, for a copy that outlives the ring.
        fn taken_detail(&self, r: &Record) -> Option<TakenDetail> {
            match r.detail {
                Detail::None => None,
                Detail::Count(n) => Some(TakenDetail::Count(n)),
                Detail::Text { .. } => self.detail(r).map(TakenDetail::Text),
            }
        }
    }

    /// A copy of every thread's scopes that closed at or after `since_ns`
    /// (nanoseconds from the epoch; `None` for all), each ring locked only
    /// while it is copied. The rings keep recording. The copy is
    /// proportional to what it takes, not to the ring: a five-second
    /// window of the player's 16 scopes is about 5 000 records.
    pub fn snapshot(since_ns: Option<u64>) -> Vec<ThreadScopes> {
        let k = clock::ns_per_tick();
        let mut out = Vec::new();
        with_rings(|ring| {
            let mut scopes = Vec::new();
            for r in ring.records() {
                let start_ns = r.start_ns(k);
                let dur_ns = r.dur_ns(k);
                if since_ns.is_some_and(|since| start_ns + dur_ns < since) {
                    continue;
                }
                scopes.push(TakenScope {
                    name: r.site.name,
                    file: r.site.file,
                    line: r.site.line,
                    start_ns,
                    dur_ns,
                    frame: r.frame,
                    depth: r.depth,
                    detail: ring.taken_detail(r),
                });
            }
            out.push(ThreadScopes {
                thread: ring.thread.clone(),
                scopes,
                held: ring.held(),
                written: ring.written,
                capacity: ring.capacity(),
            });
        });
        out
    }

    fn rings() -> &'static Mutex<Vec<Arc<Mutex<Ring>>>> {
        static RINGS: OnceLock<Mutex<Vec<Arc<Mutex<Ring>>>>> = OnceLock::new();
        RINGS.get_or_init(|| Mutex::new(Vec::new()))
    }

    thread_local! {
        static DEPTH: Cell<u16> = const { Cell::new(0) };
        // Made at a thread's first recorded scope. Each ring is behind a lock
        // nobody else takes except a reader (`with_rings`), so it is
        // uncontended on the frame.
        static RING: Arc<Mutex<Ring>> = {
            let thread = std::thread::current();
            let name = thread
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{:?}", thread.id()));
            let cap = RING_CAPACITY.load(Ordering::Relaxed) as usize;
            let ring = Arc::new(Mutex::new(Ring::new(name, cap)));
            if let Ok(mut all) = rings().lock() {
                all.push(ring.clone());
            }
            ring
        };
    }

    /// Calls `f` with every thread's ring that has recorded anything, in the
    /// order the threads first recorded.
    pub fn with_rings(mut f: impl FnMut(&Ring)) {
        let all: Vec<_> = rings().lock().map(|v| v.clone()).unwrap_or_default();
        for ring in all {
            if let Ok(r) = ring.lock() {
                f(&r);
            }
        }
    }

    /// An open scope, recorded when it drops. Made by `scope!`.
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
            DEPTH.with(|d| d.set(d.get().saturating_add(1)));
            Scope {
                site,
                start: Some(clock::now()),
                detail: Detail::None,
            }
        }

        #[inline]
        pub fn enter_with<D: ScopeDetail>(site: &'static Site, detail: &D) -> Scope {
            let mut scope = Scope::enter(site);
            if scope.start.is_some() {
                scope.detail = match detail.detail() {
                    DetailValue::Count(n) => Detail::Count(n),
                    DetailValue::Text(t) => {
                        RING.with(|r| r.lock().map(|mut r| r.push_text(t)).unwrap_or(Detail::None))
                    }
                };
            }
            scope
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

    #[cfg(test)]
    mod tests {
        use super::*;

        static SITE: Site = Site {
            name: "test",
            file: file!(),
            line: line!(),
        };

        fn record_at(start: u64) -> Record {
            Record {
                site: &SITE,
                start,
                dur: 1,
                frame: 0,
                depth: 0,
                detail: Detail::None,
            }
        }

        #[test]
        fn a_full_ring_overwrites_its_oldest_and_reads_oldest_first() {
            let mut ring = Ring::new("t".into(), 4);
            for start in 0..6 {
                ring.push(record_at(start));
            }
            let starts: Vec<u64> = ring.records().map(|r| r.start).collect();
            assert_eq!(starts, vec![2, 3, 4, 5]);
            assert_eq!(ring.written(), 6);
            assert_eq!(ring.records.capacity(), 4, "the ring never grows");
        }

        #[test]
        fn a_ring_of_capacity_zero_keeps_every_record_and_its_text() {
            let mut ring = Ring::new("t".into(), 0);
            let mut r = record_at(0);
            r.detail = ring.push_text("models/oak.glb");
            ring.push(r);
            let filler = "x".repeat(TEXT_CAPACITY);
            ring.push_text(&filler);
            for start in 1..100_000 {
                ring.push(record_at(start));
            }
            assert_eq!(ring.held(), 100_000);
            assert_eq!(ring.capacity(), None);
            let first = ring.records().next().unwrap();
            assert_eq!(first.start, 0, "oldest first");
            assert_eq!(ring.detail(first).as_deref(), Some("models/oak.glb"));
        }

        #[test]
        fn text_overwritten_since_reads_as_none() {
            let mut ring = Ring::new("t".into(), 4);
            let first = ring.push_text("models/oak.glb");
            let filler = "x".repeat(TEXT_CAPACITY);
            ring.push_text(&filler);
            let mut r = record_at(0);
            r.detail = first;
            assert_eq!(ring.detail(&r), None);
        }
    }
}

#[cfg(all(test, any(debug_assertions, feature = "trace")))]
mod tests {
    use super::*;

    /// Every scope `f` records on a thread of its own, as (name, depth, detail).
    fn recorded_on_a_thread(
        name: &str,
        f: impl FnOnce() + Send + 'static,
    ) -> Vec<(&'static str, u16, Option<String>)> {
        std::thread::Builder::new()
            .name(name.into())
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
        let mut seen = Vec::new();
        with_rings(|ring| {
            if ring.thread() == name {
                for r in ring.records() {
                    seen.push((r.site.name, r.depth, ring.detail(r)));
                }
            }
        });
        seen
    }

    #[test]
    fn nested_scopes_record_their_depth_and_detail_when_they_close() {
        set_recording(true);
        let seen = recorded_on_a_thread("profile-nested", || {
            crate::scope!("outer");
            {
                crate::scope!("inner", "models/oak.glb");
            }
            {
                crate::scope!("counted", 42usize);
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
    }

    #[test]
    fn a_detail_expression_runs_once() {
        set_recording(true);
        let seen = recorded_on_a_thread("profile-once", || {
            let mut calls = 0usize;
            {
                crate::scope!("once", {
                    calls += 1;
                    calls
                });
            }
            assert_eq!(calls, 1);
        });
        assert_eq!(seen, vec![("once", 0, Some("1".to_string()))]);
    }

    #[test]
    fn a_snapshot_takes_the_scopes_that_closed_in_its_window() {
        set_recording(true);
        std::thread::Builder::new()
            .name("profile-window".into())
            .spawn(|| {
                {
                    crate::scope!("before");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                let since = clock::now_ns();
                {
                    crate::scope!("after", 3u32);
                }
                {
                    let named = site_named("scripts/a.rhai:on_update");
                    assert!(std::ptr::eq(named, site_named("scripts/a.rhai:on_update")));
                    let _s = Scope::enter(named);
                }
                let taken = snapshot(Some(since));
                let mine = taken.iter().find(|t| t.thread == "profile-window").unwrap();
                let names: Vec<_> = mine.scopes.iter().map(|s| s.name).collect();
                assert_eq!(names, vec!["after", "scripts/a.rhai:on_update"]);
                assert_eq!(mine.scopes[0].detail, Some(TakenDetail::Count(3)));
                assert_eq!(mine.held, 3);
                assert!(!mine.wrapped());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn a_scope_times_what_it_covers() {
        set_recording(true);
        std::thread::Builder::new()
            .name("profile-time".into())
            .spawn(|| {
                crate::scope!("sleep");
                std::thread::sleep(std::time::Duration::from_millis(20));
            })
            .unwrap()
            .join()
            .unwrap();
        let k = clock::ns_per_tick();
        let mut dur_ms = None;
        with_rings(|ring| {
            if ring.thread() == "profile-time" {
                dur_ms = ring.records().next().map(|r| r.dur_ns(k) as f64 / 1e6);
            }
        });
        let dur_ms = dur_ms.expect("the scope was recorded");
        assert!(
            (19.0..500.0).contains(&dur_ms),
            "a 20 ms sleep read as {dur_ms} ms"
        );
    }
}
