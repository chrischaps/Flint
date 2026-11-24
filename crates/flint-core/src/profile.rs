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
/// A file's path, as written (a path that isn't UTF-8 reads `?`). Read only
/// while a scope records, so a load's scope converts nothing when it doesn't.
impl ScopeDetail for &std::path::Path {
    fn detail(&self) -> DetailValue<'_> {
        DetailValue::Text(self.to_str().unwrap_or("?"))
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
    /// A summed record (#1288, [`record_calls`]): `calls` calls of one
    /// function folded into one record, whose duration is their total and
    /// whose start is the first call's; `worst_ns` the longest of them.
    Calls {
        calls: u32,
        worst_ns: u64,
    },
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

    /// Where this thread's record begins inside the window it was copied
    /// for, ns from the epoch, when its ring lost records the window wanted
    /// (#1288): it wrapped, and everything it still held was in the window,
    /// so what it overwrote was too. `None` when the copy is whole. It is
    /// the oldest held record's close: records are kept in close order, so
    /// any scope that closed after it, and every frame that began after it,
    /// is whole.
    pub fn cut_at_ns(&self) -> Option<u64> {
        if !self.wrapped() || self.scopes.len() < self.held {
            return None;
        }
        self.scopes.iter().map(|s| s.start_ns + s.dur_ns).min()
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

    /// Bytes of detail text a ring keeps a record, as the recorder's build
    /// says.
    pub const TEXT_BYTES_PER_RECORD: usize = 32;

    /// Nothing to size: the recorder is not in this build.
    pub fn set_ring_capacity(_records: usize) {}

    /// Nothing to grow: the recorder is not in this build.
    pub fn set_ring_growth(_seconds: f64, _max_records: usize) {}

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

    /// Nothing recorded, nothing to copy.
    pub fn snapshot_records(_since_ns: Option<u64>) -> Records {
        Records
    }

    /// Nothing to warm: the recorder is not in this build.
    pub fn warm_copies() -> Option<std::thread::JoinHandle<()>> {
        None
    }

    /// The records a save copied: none in this build.
    #[derive(Debug, Default)]
    pub struct Records;

    impl Records {
        pub fn copied(&self) -> usize {
            0
        }

        pub fn scopes(self) -> Vec<super::ThreadScopes> {
            Vec::new()
        }
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

    /// A call that may fold into a summed record: nothing in this build.
    pub struct Call;

    impl Call {
        #[inline(always)]
        pub fn enter() -> Call {
            Call
        }

        #[inline(always)]
        pub fn close(self, _site: &'static super::Site) -> Option<(u64, u64)> {
            None
        }
    }

    /// Nothing to record: the recorder is not in this build.
    #[inline(always)]
    pub fn record_calls(
        _site: &'static super::Site,
        _start: u64,
        _dur: u64,
        _calls: u32,
        _worst: u64,
    ) {
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
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Whether the recorder is in this build.
    pub const COMPILED_IN: bool = true;

    static RECORDING: AtomicBool = AtomicBool::new(false);
    static FRAME: AtomicU32 = AtomicU32::new(0);
    static RING_CAPACITY: AtomicU32 = AtomicU32::new(DEFAULT_RING_CAPACITY as u32);
    /// [`set_ring_growth`]: the seconds a ring should reach back (as `f64`
    /// bits; 0 never grows), and the most records it may grow to.
    static GROW_SECONDS: AtomicU64 = AtomicU64::new(0);
    static GROW_MAX: AtomicU32 = AtomicU32::new(0);

    /// Let a ring that laps in less than `seconds` grow, doubling, up to
    /// `max_records` (#1288): a ring is sized by a guess at the scopes a
    /// second, and a game that records more would lose most of its
    /// look-back. It checks once a lap (when its writing comes round to the
    /// oldest record, so they are in order and the growth copies nothing
    /// out of place), and a growth is a frame note on its thread
    /// ([`NoteKind::TraceRing`](crate::frame_notes::NoteKind::TraceRing),
    /// a tool's cost). `seconds` 0 turns it off.
    pub fn set_ring_growth(seconds: f64, max_records: usize) {
        GROW_SECONDS.store(seconds.max(0.0).to_bits(), Ordering::Relaxed);
        GROW_MAX.store(max_records.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    /// The ring's want, in clock ticks, and its most records, if it grows.
    fn growth() -> Option<(u64, usize)> {
        let seconds = f64::from_bits(GROW_SECONDS.load(Ordering::Relaxed));
        let max = GROW_MAX.load(Ordering::Relaxed) as usize;
        (seconds > 0.0 && max > 0).then(|| ((seconds * 1e9 / clock::ns_per_tick()) as u64, max))
    }

    /// Records a thread's ring holds unless [`set_ring_capacity`] says
    /// otherwise: 65 536, 2.6 MB of records a thread, and 2.1 MB of text
    /// beside them once the thread records a text detail.
    pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;
    /// Bytes of detail text a ring keeps for each record it holds (#1259),
    /// so the text lasts as long as the records that name it: a ring of
    /// text-detail scopes keeps every one's text while each is 32 bytes or
    /// less. A Ruins frame's 105 scopes write about 700 bytes of text (34
    /// details at ~20 bytes), 6.7 a record, so there the text outlasts its
    /// records nearly five times over. It was a fixed 64 KB, which held
    /// the last ~100 frames' details against the records' ~600. Taken at
    /// the thread's first text detail, so a thread that writes none pays
    /// nothing for it, and grown with the ring.
    pub const TEXT_BYTES_PER_RECORD: usize = 32;

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

        /// The reading of [`now`] that is `ns` from the epoch: the inverse
        /// of [`since_epoch_ns`], to the tick.
        pub fn ticks_at_ns(ns: u64, ns_per_tick: f64) -> u64 {
            #[cfg(target_arch = "x86_64")]
            {
                epoch().tsc + (ns as f64 / ns_per_tick) as u64
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = ns_per_tick;
                ns
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
        /// A summed record ([`record_calls`]): how many calls, and the
        /// longest in ticks (saturating; 1.4 s at 3 GHz). Eight bytes, so a
        /// record stays 40.
        Calls {
            calls: u32,
            worst: u32,
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
        /// The details' text: a ring of [`Ring::text_capacity`] bytes (or,
        /// unbounded, every byte), taken at the first text detail.
        text: Vec<u8>,
        text_written: u64,
        /// The oldest text position a growth kept: what lay before it had
        /// been overwritten already, though the grown ring would reach it.
        text_kept_from: u64,
    }

    /// Writes `bytes` into the ring `buf` at absolute position `at`.
    fn write_wrapped(buf: &mut [u8], at: u64, bytes: &[u8]) {
        let start = (at % buf.len() as u64) as usize;
        let first = bytes.len().min(buf.len() - start);
        buf[start..start + first].copy_from_slice(&bytes[..first]);
        buf[..bytes.len() - first].copy_from_slice(&bytes[first..]);
    }

    /// The `len` bytes of the ring `buf` from absolute position `at`, as the
    /// two slices they lie in.
    fn read_wrapped(buf: &[u8], at: u64, len: usize) -> (&[u8], &[u8]) {
        let start = (at % buf.len() as u64) as usize;
        let first = len.min(buf.len() - start);
        (&buf[start..start + first], &buf[..len - first])
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
                text_kept_from: 0,
            }
        }

        /// Bytes of detail text it holds at most: [`TEXT_BYTES_PER_RECORD`]
        /// for each record; `None` for every byte.
        pub fn text_capacity(&self) -> Option<usize> {
            self.capacity().map(|cap| cap * TEXT_BYTES_PER_RECORD)
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

        /// Push a record; the ring's new size, and the ticks growing took,
        /// if it grew.
        fn push(&mut self, r: Record) -> Option<(usize, u64)> {
            if self.unbounded() {
                self.records.push(r);
                self.written += 1;
                return None;
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
            self.written += 1;
            if self.next == self.cap {
                self.next = 0;
                // A lap ends: oldest at 0, newest last, so a longer ring
                // carries on after them with nothing moved
                if let Some((want, max)) = growth() {
                    return self.grow_if_short(want, max);
                }
            }
            None
        }

        /// At a lap's end: double (up to `max`) when the lap spanned less
        /// than `want` ticks.
        fn grow_if_short(&mut self, want: u64, max: usize) -> Option<(usize, u64)> {
            let (first, last) = (self.records.first()?, self.records.last()?);
            let span = (last.start + last.dur).saturating_sub(first.start);
            if span >= want || self.cap >= max {
                return None;
            }
            let before = clock::now();
            let cap = (self.cap * 2).min(max);
            self.records.reserve_exact(cap - self.records.len());
            self.next = self.records.len();
            self.grow_text(cap * TEXT_BYTES_PER_RECORD);
            self.cap = cap;
            Some((cap, clock::now().saturating_sub(before)))
        }

        /// The text ring at `bytes`, every byte still held kept at its
        /// absolute position, so each record's text reads as before.
        fn grow_text(&mut self, bytes: usize) {
            if self.text.is_empty() {
                return;
            }
            let held = self.text_written.min(self.text.len() as u64);
            let from = self.text_written - held;
            let mut text = vec![0; bytes];
            let (a, b) = read_wrapped(&self.text, from, held as usize);
            write_wrapped(&mut text, from, a);
            write_wrapped(&mut text, from + a.len() as u64, b);
            self.text = text;
            self.text_kept_from = from;
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
                self.text.resize(self.cap * TEXT_BYTES_PER_RECORD, 0);
            }
            let len = s.len().min(u16::MAX as usize).min(self.text.len());
            let at = self.text_written;
            write_wrapped(&mut self.text, at, &s.as_bytes()[..len]);
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
                Detail::Calls { calls, .. } => Some(format!("{calls} calls")),
                Detail::Text { at, len } => {
                    let (a, b) = self.text_at(at, len)?;
                    let mut bytes = Vec::with_capacity(len as usize);
                    bytes.extend_from_slice(a);
                    bytes.extend_from_slice(b);
                    Some(String::from_utf8_lossy(&bytes).into_owned())
                }
            }
        }

        /// The oldest text position still held: text written before it
        /// has been overwritten, by the ring lapping it or before a growth
        /// (`text_kept_from`).
        fn text_held_from(&self) -> u64 {
            if self.unbounded() {
                0
            } else {
                self.text_written
                    .saturating_sub(self.text.len() as u64)
                    .max(self.text_kept_from)
            }
        }

        /// The `len` bytes of text from absolute position `at`, as the two
        /// slices of the ring they lie in. `at` must be held.
        fn text_slices(&self, at: u64, len: usize) -> (&[u8], &[u8]) {
            if len == 0 {
                return (&[], &[]);
            }
            if self.unbounded() {
                return (&self.text[at as usize..at as usize + len], &[]);
            }
            read_wrapped(&self.text, at, len)
        }

        /// The text written at `at`, unless it has been overwritten since.
        fn text_at(&self, at: u64, len: u16) -> Option<(&[u8], &[u8])> {
            (at >= self.text_held_from()).then(|| self.text_slices(at, len as usize))
        }

        /// The held records, oldest first, as the two slices they lie in.
        fn halves(&self) -> (&[Record], &[Record]) {
            if self.unbounded() || self.records.len() < self.cap {
                (&self.records[..], &[])
            } else {
                let (newer, older) = self.records.split_at(self.next);
                (older, newer)
            }
        }

        /// The records that closed at or after the tick `from` (`None` for
        /// all), oldest first, as they lie, and the text they name: the
        /// frame thread's whole share of a save (#1267). Records lie in the
        /// order they closed, so the window is the newest end of the ring:
        /// a scan back from the newest finds where it begins, and the
        /// records and their text are each one copy. A summed record's
        /// start and length don't give its close, so the scan passes over
        /// them, and the few before the window that come along are cut by
        /// [`Records::scopes`]. A record's text is read as the ring would
        /// read it now: overwritten text is no detail.
        fn copy_from(&self, from: Option<u64>) -> ThreadRecords {
            let (older, newer) = self.halves();
            let held = older.len() + newer.len();
            let mut first = held;
            let mut text_from = self.text_written;
            for r in newer.iter().rev().chain(older.iter().rev()) {
                match r.detail {
                    Detail::Calls { .. } => {}
                    _ if from.is_some_and(|from| r.start + r.dur < from) => break,
                    Detail::Text { at, .. } => text_from = text_from.min(at),
                    _ => {}
                }
                first -= 1;
            }
            let text_from = text_from.max(self.text_held_from());
            let len = (self.text_written - text_from) as usize;
            let Spare {
                mut records,
                mut text,
            } = Spare::take(held - first, len);
            if first < older.len() {
                records.extend_from_slice(&older[first..]);
                records.extend_from_slice(newer);
            } else {
                records.extend_from_slice(&newer[first - older.len()..]);
            }
            let (a, b) = self.text_slices(text_from, len);
            text.extend_from_slice(a);
            text.extend_from_slice(b);
            ThreadRecords {
                thread: self.thread.clone(),
                records,
                text_from,
                text,
                held,
                written: self.written,
                capacity: self.capacity(),
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
    }

    /// The memory a copy writes into, kept between saves (#1267). Copying a
    /// window is a `memcpy`, and into fresh memory most of its time is the
    /// first touch of each page: 0.6 ms of a 90 000-record copy's 0.7, where
    /// memory touched before takes 0.1. So [`Records::scopes`] hands its
    /// buffers back here once read, and [`warm_copies`] touches some ahead
    /// of a save it knows is coming.
    struct Spare {
        records: Vec<Record>,
        text: Vec<u8>,
    }

    /// Spares kept at most, the largest: one a recording thread is plenty.
    const SPARES_KEPT: usize = 8;
    /// Bytes the spares keep at most, all told: two copies of a ring grown
    /// to its most for 5 s (1 310 720 records, 63 MB), whatever the count.
    const SPARE_BYTES_KEPT: usize = 128 << 20;

    impl Spare {
        fn bytes(&self) -> usize {
            self.records.capacity() * std::mem::size_of::<Record>() + self.text.capacity()
        }
    }

    fn spares() -> std::sync::MutexGuard<'static, Vec<Spare>> {
        static SPARES: Mutex<Vec<Spare>> = Mutex::new(Vec::new());
        SPARES.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Spare {
        /// Empty buffers for `records` records and `text` bytes: the
        /// smallest spare that holds the records, else new ones.
        fn take(records: usize, text: usize) -> Spare {
            if records == 0 {
                return Spare {
                    records: Vec::new(),
                    text: Vec::with_capacity(text),
                };
            }
            let mut spare = {
                let mut kept = spares();
                let best = (0..kept.len())
                    .filter(|&i| kept[i].records.capacity() >= records)
                    .min_by_key(|&i| kept[i].records.capacity());
                best.map(|i| kept.swap_remove(i))
            }
            .unwrap_or_else(|| Spare {
                records: Vec::with_capacity(records),
                text: Vec::new(),
            });
            spare.text.reserve(text);
            spare
        }

        /// Kept for the next copy, emptied: the smallest go when there are
        /// more than [`SPARES_KEPT`], the largest when they hold more than
        /// [`SPARE_BYTES_KEPT`].
        fn give_back(mut self) {
            self.records.clear();
            self.text.clear();
            let mut kept = spares();
            kept.push(self);
            if kept.len() > SPARES_KEPT {
                let smallest = (0..kept.len())
                    .min_by_key(|&i| kept[i].records.capacity())
                    .unwrap_or(0);
                kept.swap_remove(smallest);
            }
            while kept.iter().map(Spare::bytes).sum::<usize>() > SPARE_BYTES_KEPT {
                let largest = (0..kept.len())
                    .max_by_key(|&i| kept[i].bytes())
                    .unwrap_or(0);
                kept.swap_remove(largest);
            }
        }

        /// Buffers for `records` records and `text` bytes, every page
        /// touched.
        #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
        fn warm(records: usize, text: usize) -> Spare {
            let mut spare = Spare {
                records: Vec::with_capacity(records),
                text: Vec::with_capacity(text),
            };
            spare
                .records
                .spare_capacity_mut()
                .fill(std::mem::MaybeUninit::zeroed());
            // Written, not `vec![0; n]`, whose zeroed pages are untouched
            spare.text.resize(text, 0);
            spare.text.clear();
            spare
        }
    }

    /// Ready the memory the next save's copy will write to, on a thread of
    /// its own, for a host that knows a save is coming (the player's
    /// `--trace-on-hitch`, eight frames after the hitch): one spare a ring,
    /// as large as all it holds. Then the frame thread's copy is a `memcpy`
    /// into warm pages. A save the warming hasn't reached copies into fresh
    /// memory, as it would have anyway. The thread, to join or let go;
    /// `None` on the web, which has no threads to do it on.
    pub fn warm_copies() -> Option<std::thread::JoinHandle<()>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut wanted = Vec::new();
            with_rings(|ring| {
                let text = (ring.text_written - ring.text_held_from()) as usize;
                wanted.push((ring.held(), text));
            });
            std::thread::Builder::new()
                .name("trace-warm".into())
                .spawn(move || {
                    for (records, text) in wanted {
                        Spare::warm(records, text).give_back();
                    }
                })
                .ok()
        }
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// Forget every spare, so the next copy writes into fresh memory: how
    /// a measurement takes the first save's cost.
    #[cfg(test)]
    pub(crate) fn drop_spares() {
        spares().clear();
    }

    /// One ring's records as [`snapshot_records`] copied them, and the text
    /// they name: the ring's text from absolute position `text_from` to
    /// what it had written. A record whose text begins before it had lost
    /// its text when the copy was made.
    #[derive(Debug)]
    struct ThreadRecords {
        thread: String,
        records: Vec<Record>,
        text_from: u64,
        text: Vec<u8>,
        held: usize,
        written: u64,
        capacity: Option<usize>,
    }

    impl ThreadRecords {
        fn taken_detail(&self, r: &Record, ns_per_tick: f64) -> Option<TakenDetail> {
            match r.detail {
                Detail::None => None,
                Detail::Count(n) => Some(TakenDetail::Count(n)),
                Detail::Calls { calls, worst } => Some(TakenDetail::Calls {
                    calls,
                    worst_ns: (worst as f64 * ns_per_tick) as u64,
                }),
                Detail::Text { at, len } => {
                    let from = at.checked_sub(self.text_from)? as usize;
                    let bytes = self.text.get(from..from + len as usize)?;
                    Some(TakenDetail::Text(
                        String::from_utf8_lossy(bytes).into_owned(),
                    ))
                }
            }
        }
    }

    /// Every thread's records in a window, as they lie in the rings:
    /// what [`snapshot_records`] copies on the frame thread, for
    /// [`Records::scopes`] to read out on another (#1267).
    #[derive(Debug, Default)]
    pub struct Records {
        ns_per_tick: f64,
        since_ns: Option<u64>,
        threads: Vec<ThreadRecords>,
    }

    impl Records {
        /// Records copied, every thread's; a few more than
        /// [`scopes`](Self::scopes) keeps, since the copy's window is a
        /// microsecond wider.
        pub fn copied(&self) -> usize {
            self.threads.iter().map(|t| t.records.len()).sum()
        }

        /// The copy as [`snapshot`] gives it: each record that closed at or
        /// after the window's start, owned, in nanoseconds from the epoch,
        /// its text a `String`.
        pub fn scopes(self) -> Vec<ThreadScopes> {
            let k = self.ns_per_tick;
            let since_ns = self.since_ns;
            self.threads
                .into_iter()
                .map(|t| {
                    let scopes: Vec<TakenScope> = t
                        .records
                        .iter()
                        .filter_map(|r| {
                            let start_ns = r.start_ns(k);
                            let dur_ns = r.dur_ns(k);
                            if since_ns.is_some_and(|since| start_ns + dur_ns < since) {
                                return None;
                            }
                            Some(TakenScope {
                                name: r.site.name,
                                file: r.site.file,
                                line: r.site.line,
                                start_ns,
                                dur_ns,
                                frame: r.frame,
                                depth: r.depth,
                                detail: t.taken_detail(r, k),
                            })
                        })
                        .collect();
                    let ThreadRecords {
                        thread,
                        records,
                        text,
                        held,
                        written,
                        capacity,
                        ..
                    } = t;
                    Spare { records, text }.give_back();
                    ThreadScopes {
                        thread,
                        scopes,
                        held,
                        written,
                        capacity,
                    }
                })
                .collect()
        }
    }

    /// How much wider than its window [`snapshot_records`] copies: the
    /// window is in nanoseconds and the rings in ticks, so the copy takes
    /// a microsecond more and [`Records::scopes`] cuts it exactly.
    const COPY_SLACK_NS: u64 = 1_000;

    /// The records of every thread that closed at or after `since_ns`
    /// (nanoseconds from the epoch; `None` for all), as they lie in the
    /// rings: the frame thread's share of a save (#1267). Each ring is
    /// locked only while it is copied, and the rings keep recording.
    /// Converting them ([`Records::scopes`]) is for the thread that writes
    /// the file. 90 000 records copy in about 0.35 ms into memory a spare
    /// kept warm, 0.7 ms into fresh memory (release; `cost::snapshot_cost`),
    /// where converting them on the frame thread took 3.6.
    pub fn snapshot_records(since_ns: Option<u64>) -> Records {
        let k = clock::ns_per_tick();
        let from = since_ns.map(|since| {
            clock::ticks_at_ns(since.saturating_sub(COPY_SLACK_NS), k).saturating_sub(1)
        });
        let mut threads = Vec::new();
        with_rings(|ring| threads.push(ring.copy_from(from)));
        Records {
            ns_per_tick: k,
            since_ns,
            threads,
        }
    }

    /// A copy of every thread's scopes that closed at or after `since_ns`
    /// (nanoseconds from the epoch; `None` for all): [`snapshot_records`]
    /// and [`Records::scopes`] on one thread. The copy is proportional to
    /// what it takes, not to the ring.
    pub fn snapshot(since_ns: Option<u64>) -> Vec<ThreadScopes> {
        snapshot_records(since_ns).scopes()
    }

    fn rings() -> &'static Mutex<Vec<Arc<Mutex<Ring>>>> {
        static RINGS: OnceLock<Mutex<Vec<Arc<Mutex<Ring>>>>> = OnceLock::new();
        RINGS.get_or_init(|| Mutex::new(Vec::new()))
    }

    thread_local! {
        static DEPTH: Cell<u16> = const { Cell::new(0) };
        /// Records this thread has written: how a [`Call`] knows whether
        /// anything was recorded inside it.
        static RECORDED: Cell<u64> = const { Cell::new(0) };
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
        push(Record {
            site,
            start,
            dur: end.saturating_sub(start),
            frame: FRAME.load(Ordering::Relaxed),
            depth,
            detail,
        });
    }

    fn push(r: Record) {
        RECORDED.with(|n| n.set(n.get() + 1));
        RING.with(|ring| {
            if let Ok(mut ring) = ring.lock() {
                if let Some((cap, ticks)) = ring.push(r) {
                    // The growth's copy is the recorder's cost, not the game's
                    let ms = (ticks as f64 * clock::ns_per_tick() / 1e6) as f32;
                    let thread = ring.thread.clone();
                    drop(ring);
                    crate::frame_notes::note(
                        crate::frame_notes::NoteKind::TraceRing,
                        || format!("{thread} ring grew to {cap} records"),
                        Some(ms),
                    );
                }
            }
        });
    }

    /// A call that is a scope only if something inside it is (#1288).
    ///
    /// A script's engine functions are called hundreds of times a frame
    /// (Float: 800 records a frame, most of them `get_field()` and
    /// `clamp()`), which filled a ring sized for the frame's own scopes in
    /// under a second. Such a call opens a level (so a scope inside it
    /// nests as it would under a scope) but records nothing itself: its
    /// [`close`](Call::close) hands back its start and length for the
    /// caller to sum into one [`record_calls`] record a function, unless
    /// something was recorded inside it, when it is recorded as a scope of
    /// its own after all and the caller sums nothing.
    pub struct Call {
        start: Option<u64>,
        recorded: u64,
    }

    impl Call {
        #[inline]
        pub fn enter() -> Call {
            if !RECORDING.load(Ordering::Relaxed) {
                return Call {
                    start: None,
                    recorded: 0,
                };
            }
            DEPTH.with(|d| d.set(d.get().saturating_add(1)));
            Call {
                start: Some(clock::now()),
                recorded: RECORDED.with(Cell::get),
            }
        }

        /// The call ends: `(start, length)` in clock ticks for the caller
        /// to sum when nothing was recorded inside it, else `None`, with
        /// the call recorded under `site`. `None` too when it began before
        /// recording did.
        #[inline]
        pub fn close(self, site: &'static Site) -> Option<(u64, u64)> {
            let start = self.start?;
            if RECORDED.with(Cell::get) != self.recorded {
                record(site, start, Detail::None);
                return None;
            }
            let end = clock::now();
            DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
            Some((start, end.saturating_sub(start)))
        }
    }

    /// One record standing for `calls` calls of `site` (#1288): from the
    /// first call's `start` for their summed length `dur`, both in clock
    /// ticks as [`Call::close`] gave them, with the longest call `worst`.
    /// It nests one level inside the scopes open now, as the calls did,
    /// and is a leaf: a reader never takes it for anything's parent, since
    /// it may overlap its siblings (its length is the calls', laid from
    /// the first). Nothing while not recording.
    pub fn record_calls(site: &'static Site, start: u64, dur: u64, calls: u32, worst: u64) {
        if !RECORDING.load(Ordering::Relaxed) || calls == 0 {
            return;
        }
        push(Record {
            site,
            start,
            dur,
            frame: FRAME.load(Ordering::Relaxed),
            depth: DEPTH.with(Cell::get),
            detail: Detail::Calls {
                calls,
                worst: worst.min(u32::MAX as u64) as u32,
            },
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
        fn a_ring_that_laps_too_fast_doubles_at_the_lap_and_keeps_its_order() {
            // A full first lap: records 0..=3, spanning 4 ticks
            let mut ring = Ring::new("t".into(), 4);
            for start in 0..4 {
                ring.push(record_at(start));
            }
            // ... against a want of 10
            let grown = ring.grow_if_short(10, 6).map(|(cap, _)| cap);
            assert_eq!(grown, Some(6), "doubled, held at the most");
            for start in 4..8 {
                ring.push(record_at(start));
            }
            let starts: Vec<u64> = ring.records().map(|r| r.start).collect();
            assert_eq!(
                starts,
                vec![2, 3, 4, 5, 6, 7],
                "oldest first, nothing lost to the growth"
            );
            assert_eq!(ring.grow_if_short(10, 6), None, "at its most");
            let mut long = Ring::new("t".into(), 2);
            long.records.push(record_at(0));
            long.records.push(record_at(50));
            assert_eq!(
                long.grow_if_short(10, 8),
                None,
                "a lap that spans its want stays"
            );
        }

        #[test]
        fn a_ring_of_capacity_zero_keeps_every_record_and_its_text() {
            let mut ring = Ring::new("t".into(), 0);
            let mut r = record_at(0);
            r.detail = ring.push_text("models/oak.glb");
            ring.push(r);
            let filler = "x".repeat(DEFAULT_RING_CAPACITY * TEXT_BYTES_PER_RECORD);
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

        /// #1267: what the frame thread copies is the ring's records as they
        /// lie, and its text; read out later, each detail is what the ring
        /// would have said then.
        #[test]
        fn a_copy_reads_out_later_as_the_ring_read_when_it_was_taken() {
            let mut ring = Ring::new("t".into(), 16);
            let mut lost = record_at(0);
            lost.detail = ring.push_text("models/oak.glb");
            ring.push(lost);
            // Text up to five bytes short of the text ring's end, so the
            // next detail straddles it and the first is overwritten
            let text_cap = ring.text_capacity().unwrap();
            ring.push_text(&"x".repeat(text_cap - 5 - "models/oak.glb".len()));
            let mut straddles = record_at(1);
            straddles.detail = ring.push_text("scripts/door.rhai");
            ring.push(straddles);
            let mut counted = record_at(2);
            counted.detail = Detail::Count(42);
            ring.push(counted);
            let mut summed = record_at(3);
            summed.detail = Detail::Calls {
                calls: 500,
                worst: 7,
            };
            ring.push(summed);
            for start in 4..10 {
                let mut r = record_at(start);
                if start == 6 {
                    r.detail = ring.push_text("encode_part bloom");
                }
                ring.push(r);
            }
            let said: Vec<Option<String>> = ring.records().map(|r| ring.detail(r)).collect();
            assert_eq!(said[0], None, "the first text was overwritten");
            assert_eq!(said[1].as_deref(), Some("scripts/door.rhai"));

            let copy = ring.copy_from(None);
            // The ring records on after the copy; the copy keeps its text
            ring.push_text(&"y".repeat(text_cap));
            let read = Records {
                ns_per_tick: 1.0,
                since_ns: None,
                threads: vec![copy],
            }
            .scopes();
            let details: Vec<Option<String>> = read[0]
                .scopes
                .iter()
                .map(|s| match &s.detail {
                    None => None,
                    Some(TakenDetail::Text(t)) => Some(t.clone()),
                    Some(TakenDetail::Count(n)) => Some(n.to_string()),
                    Some(TakenDetail::Calls { calls, .. }) => Some(format!("{calls} calls")),
                })
                .collect();
            assert_eq!(details, said);
            assert_eq!(read[0].held, 10);
            assert_eq!(read[0].written, 10);
            assert_eq!(read[0].capacity, Some(16));
        }

        #[test]
        fn a_copy_takes_what_closed_from_its_tick_oldest_first() {
            let mut ring = Ring::new("t".into(), 4);
            for start in 0..6 {
                ring.push(record_at(start));
            }
            let starts = |copy: ThreadRecords| -> Vec<u64> {
                copy.records.iter().map(|r| r.start).collect()
            };
            assert_eq!(starts(ring.copy_from(None)), vec![2, 3, 4, 5]);
            // Each record closes a tick after it starts
            assert_eq!(starts(ring.copy_from(Some(4))), vec![3, 4, 5]);
            assert_eq!(starts(ring.copy_from(Some(7))), Vec::<u64>::new());
        }

        /// A summed record's start and length are its calls', not its
        /// span's, so they can put it before the window though it closed
        /// inside: the scan for the window's start passes over it, and the
        /// read-out cuts it by its own times, as a snapshot always has.
        #[test]
        fn a_summed_record_comes_along_and_the_read_out_cuts_it() {
            clock::start();
            let k = clock::ns_per_tick();
            let ms = |ms: f64| clock::ticks_at_ns((ms * 1e6) as u64, k);
            let at = |start_ms: f64| Record {
                start: ms(start_ms),
                dur: ms(start_ms + 1.0) - ms(start_ms),
                ..record_at(0)
            };
            let mut ring = Ring::new("t".into(), 16);
            for start in 0..4 {
                ring.push(at(start as f64)); // closing at 1, 2, 3, 4 ms
            }
            let mut summed = at(0.0);
            summed.detail = Detail::Calls { calls: 3, worst: 1 };
            ring.push(summed);
            ring.push(at(4.0));
            let since_ns = 3_500_000;
            let copy = ring.copy_from(Some(ms(3.5 - 1e-3) - 1));
            let starts: Vec<u64> = copy.records.iter().map(|r| r.start).collect();
            assert_eq!(starts, vec![ms(3.0), ms(0.0), ms(4.0)], "it came along");
            let read = Records {
                ns_per_tick: k,
                since_ns: Some(since_ns),
                threads: vec![copy],
            }
            .scopes();
            let kept: Vec<u64> = read[0]
                .scopes
                .iter()
                .map(|s| (s.start_ns as f64 / 1e6).round() as u64)
                .collect();
            assert_eq!(kept, vec![3, 4], "and was cut");
        }

        #[test]
        fn text_overwritten_since_reads_as_none() {
            let mut ring = Ring::new("t".into(), 4);
            let first = ring.push_text("models/oak.glb");
            let filler = "x".repeat(ring.text_capacity().unwrap());
            ring.push_text(&filler);
            let mut r = record_at(0);
            r.detail = first;
            assert_eq!(ring.detail(&r), None);
        }

        /// A scope with a text detail, as `enter_with` and its drop push it:
        /// the text at the scope's start, the record at its close.
        fn push_scope_with_text(ring: &mut Ring, start: u64, text: &str) {
            let mut r = record_at(start);
            r.detail = ring.push_text(text);
            ring.push(r);
        }

        /// The `i`th detail of a fill: 32 bytes, the most each record is
        /// sure to keep, and each one different.
        fn script_named(i: u64) -> String {
            format!("{:x<32}", format!("scripts/door_{i:08}.rhai"))
        }

        #[test]
        fn a_full_ring_of_text_details_keeps_its_oldest_records_text() {
            // #1259: the default ring filled, and half lapped, with scopes
            // that each carry 32 bytes of text
            let mut ring = Ring::new("t".into(), DEFAULT_RING_CAPACITY);
            let pushed = DEFAULT_RING_CAPACITY as u64 * 3 / 2;
            for i in 0..pushed {
                push_scope_with_text(&mut ring, i, &script_named(i));
            }
            assert_eq!(ring.held(), DEFAULT_RING_CAPACITY);
            assert_eq!(ring.text_capacity(), Some(2 << 20), "2 MiB of text");
            let oldest = ring.records().next().unwrap();
            assert_eq!(oldest.start, pushed - DEFAULT_RING_CAPACITY as u64);
            assert_eq!(
                ring.detail(oldest),
                Some(script_named(oldest.start)),
                "the oldest record held still names its script"
            );
            assert!(
                ring.records()
                    .all(|r| ring.detail(r) == Some(script_named(r.start))),
                "and so does every record after it"
            );
        }

        #[test]
        fn a_ruins_frame_of_details_lasts_as_long_as_its_records() {
            // #1096's scopes in a frame of the Overgrown Ruins: 105 scopes,
            // 34 of them with text (a script's file, a post pass's label, a
            // stage), about 700 bytes. 64 KB of text held ~100 frames of it
            // against the records' ~600.
            let texts: Vec<String> = (0..34)
                .map(|i| match i % 3 {
                    0 => format!("scripts/overgrown_ruins_{i:02}.rhai"),
                    1 => format!("encode_part bloom {i:02}"),
                    _ => format!("subsystems {i:02}"),
                })
                .collect();
            let bytes: usize = texts.iter().map(String::len).sum();
            assert!((600..800).contains(&bytes), "{bytes} bytes a frame");
            let mut ring = Ring::new("main".into(), DEFAULT_RING_CAPACITY);
            let frames = 2 * DEFAULT_RING_CAPACITY as u64 / 105;
            let mut start = 0;
            for _ in 0..frames {
                for scope in 0..105 {
                    match texts.get(scope) {
                        Some(text) => push_scope_with_text(&mut ring, start, text),
                        None => {
                            ring.push(record_at(start));
                        }
                    }
                    start += 1;
                }
            }
            let texts_held = ring
                .records()
                .filter(|r| matches!(r.detail, Detail::Text { .. }));
            let (held, lost) = texts_held.fold((0, 0), |(held, lost), r| {
                (held + 1, lost + ring.detail(r).is_none() as usize)
            });
            assert!(held > 20_000, "{held} details held");
            assert_eq!(lost, 0, "no record held has lost its text");
        }

        /// What a growth costs, with and without the text beside the
        /// records (#1259): `cargo test -p flint-core --release --features
        /// trace --lib ring_growth_cost -- --ignored --nocapture`.
        #[test]
        #[ignore]
        fn ring_growth_cost() {
            const FROM: usize = 327_680;
            let full = |text: bool| {
                let mut ring = Ring::new("t".into(), FROM);
                for i in 0..FROM as u64 {
                    if text {
                        push_scope_with_text(&mut ring, i, &script_named(i));
                    } else {
                        ring.push(record_at(i));
                    }
                }
                ring
            };
            for text in [false, true] {
                let mut least = f64::MAX;
                for _ in 0..5 {
                    let mut ring = full(text);
                    let start = std::time::Instant::now();
                    ring.grow_if_short(u64::MAX, FROM * 2).unwrap();
                    least = least.min(start.elapsed().as_secs_f64() * 1e3);
                }
                let what = if text {
                    "records and text"
                } else {
                    "records alone"
                };
                println!("{FROM} to {} records, {what}: {least:.2} ms", FROM * 2);
            }
        }

        #[test]
        fn a_grown_ring_keeps_its_text_and_holds_twice_as_much() {
            let mut ring = Ring::new("t".into(), 4);
            // Five laps' worth of text in a ring of four: its text has
            // wrapped, so the oldest text is not at the buffer's start
            for i in 0..8 {
                push_scope_with_text(&mut ring, i, &script_named(i));
            }
            let overwritten = Record {
                detail: Detail::Text { at: 0, len: 32 },
                ..record_at(0)
            };
            assert!(ring.grow_if_short(100, 8).is_some(), "doubled");
            assert_eq!(ring.text_capacity(), Some(8 * TEXT_BYTES_PER_RECORD));
            assert_eq!(ring.text.len(), 8 * TEXT_BYTES_PER_RECORD);
            assert_eq!(
                ring.detail(&overwritten),
                None,
                "text lost before the growth stays lost, though the grown ring reaches it"
            );
            for i in 8..12 {
                push_scope_with_text(&mut ring, i, &script_named(i));
            }
            let starts: Vec<u64> = ring.records().map(|r| r.start).collect();
            assert_eq!(starts, (4..12).collect::<Vec<_>>());
            for r in ring.records() {
                assert_eq!(
                    ring.detail(r),
                    Some(script_named(r.start)),
                    "record {}",
                    r.start
                );
            }
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
    fn a_path_detail_is_its_text() {
        set_recording(true);
        let seen = recorded_on_a_thread("profile-path", || {
            let path = std::path::Path::new("models/oak.glb");
            crate::scope!("import_model", path);
        });
        assert_eq!(
            seen,
            vec![("import_model", 0, Some("models/oak.glb".to_string()))]
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

    static CALLED: Site = Site {
        name: "get_field()",
        file: "",
        line: 0,
    };

    #[test]
    fn calls_with_nothing_inside_fold_into_one_record_a_leaf_at_their_depth() {
        set_recording(true);
        let seen = recorded_on_a_thread("profile-calls", || {
            crate::scope!("on_update");
            let (mut first, mut total, mut worst, mut calls) = (None, 0u64, 0u64, 0u32);
            for _ in 0..500 {
                let call = Call::enter();
                if let Some((start, dur)) = call.close(&CALLED) {
                    first.get_or_insert(start);
                    total += dur;
                    worst = worst.max(dur);
                    calls += 1;
                }
            }
            record_calls(&CALLED, first.unwrap(), total, calls, worst);
        });
        assert_eq!(
            seen,
            vec![
                ("get_field()", 1, Some("500 calls".to_string())),
                ("on_update", 0, None),
            ],
            "500 calls are one record, one level in"
        );
    }

    #[test]
    fn a_call_with_a_scope_inside_is_a_scope_of_its_own() {
        set_recording(true);
        let seen = recorded_on_a_thread("profile-call-scope", || {
            crate::scope!("on_update");
            let call = Call::enter();
            {
                crate::scope!("physics::raycast");
            }
            assert_eq!(call.close(&CALLED), None, "nothing to sum");
            // and the depth is back where it was
            crate::scope!("after");
        });
        assert_eq!(
            seen,
            vec![
                ("physics::raycast", 2, None),
                ("get_field()", 1, None),
                ("after", 1, None),
                ("on_update", 0, None),
            ]
        );
    }

    #[test]
    fn a_wrapped_ring_says_where_its_window_begins() {
        set_recording(true);
        let scope = |start_ns: u64| TakenScope {
            name: "x",
            file: "",
            line: 0,
            start_ns,
            dur_ns: 10,
            frame: 0,
            depth: 0,
            detail: None,
        };
        let mut t = ThreadScopes {
            thread: "main".into(),
            scopes: vec![scope(500), scope(600)],
            held: 2,
            written: 9,
            capacity: Some(2),
        };
        assert_eq!(t.cut_at_ns(), Some(510), "the oldest held record's close");
        t.held = 4;
        assert_eq!(
            t.cut_at_ns(),
            None,
            "it still held records before the window"
        );
        t.held = 2;
        t.written = 2;
        assert_eq!(t.cut_at_ns(), None, "it never wrapped");
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

/// What a scope costs here, recording and not (#1096, #1266): the one
/// bench, which the test below and the web player's `?scope_bench=1` both
/// run, so a browser's figure is the desktop's measured the same way.
pub mod cost {
    use web_time::Instant;

    /// Each figure is the least over [`ScopeCost::rounds`] rounds, in
    /// nanoseconds a call, nothing subtracted (`bare_ns` is the loop alone).
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct ScopeCost {
        /// The loop with no scope in it
        pub bare_ns: f64,
        /// `scope!` with the recorder compiled in, not recording
        pub off_ns: f64,
        /// `scope!`, recording
        pub on_ns: f64,
        /// `scope!` with a count, recording
        pub counted_ns: f64,
        /// `scope!` with a file name, recording
        pub named_ns: f64,
        /// Two reads of `Instant::now()`: a scope's two clock reads where
        /// the clock is `Instant` (`performance.now()` in a browser)
        pub clock_pair_ns: f64,
        /// The smallest step `Instant` was seen to take, in nanoseconds:
        /// a scope shorter than it may read as 0
        pub clock_step_ns: f64,
        /// The median of those steps
        pub clock_step_median_ns: f64,
        /// How long each measure ran a round, at least
        pub round_ms: f64,
        pub rounds: u32,
    }

    impl ScopeCost {
        /// One line, as the test and the web console print it.
        pub fn line(&self) -> String {
            format!(
                "per scope: bare loop {:.1} ns, not recording {:.1} ns, recording {:.1} ns, \
                 with a count {:.1} ns, with a file name {:.1} ns; two clock reads {:.1} ns; \
                 the clock steps by {} (median {}); best of {} rounds of {:.0} ms",
                self.bare_ns,
                self.off_ns,
                self.on_ns,
                self.counted_ns,
                self.named_ns,
                self.clock_pair_ns,
                step(self.clock_step_ns),
                step(self.clock_step_median_ns),
                self.rounds,
                self.round_ms,
            )
        }
    }

    fn step(ns: f64) -> String {
        if ns >= 1e6 {
            format!("{:.2} ms", ns / 1e6)
        } else if ns >= 1e3 {
            format!("{:.1} us", ns / 1e3)
        } else {
            format!("{ns:.0} ns")
        }
    }

    #[inline(never)]
    fn scoped(n: u64) -> u64 {
        crate::scope!("bench");
        std::hint::black_box(n + 1)
    }

    #[inline(never)]
    fn counted(n: usize) -> usize {
        crate::scope!("bench_counted", n);
        std::hint::black_box(n + 1)
    }

    #[inline(never)]
    fn named(file: &str) -> usize {
        crate::scope!("bench_named", file);
        std::hint::black_box(file.len())
    }

    #[inline(never)]
    fn clock_pair() -> u128 {
        let a = Instant::now();
        let b = Instant::now();
        std::hint::black_box(b.duration_since(a).as_nanos())
    }

    /// Calls `f` in batches until `round_ms` have passed, then ns a call.
    /// Timed by the run, not by the call, so a clock as coarse as a
    /// browser's (100 us, 1 ms) still divides a long enough round finely.
    fn per_call(round_ms: f64, f: impl Fn(u64)) -> f64 {
        const BATCH: u64 = 1000;
        let start = Instant::now();
        let mut calls = 0u64;
        loop {
            for i in 0..BATCH {
                f(calls + i);
            }
            calls += BATCH;
            let ms = start.elapsed().as_secs_f64() * 1e3;
            if ms >= round_ms {
                return ms * 1e6 / calls as f64;
            }
        }
    }

    /// The clock's steps: read it until it changes, `want` times (or for
    /// 200 ms), and keep each change's size.
    fn clock_steps(want: usize) -> (f64, f64) {
        let begun = Instant::now();
        let mut steps = Vec::with_capacity(want);
        let mut last = Instant::now();
        while steps.len() < want && begun.elapsed().as_millis() < 200 {
            let now = Instant::now();
            let d = now.duration_since(last).as_nanos();
            if d > 0 {
                steps.push(d as f64);
                last = now;
            }
        }
        if steps.is_empty() {
            return (0.0, 0.0);
        }
        steps.sort_by(f64::total_cmp);
        (steps[0], steps[steps.len() / 2])
    }

    /// Measure a scope on this thread: each measure for `round_ms` a round,
    /// `rounds` rounds, the least kept. Recording is on while it runs and
    /// as it was after, and its records go to this thread's ring, so run it
    /// before anything reads the ring (a test thread of its own, the web
    /// page before its recorder starts). `None` without the recorder.
    pub fn measure(round_ms: f64, rounds: u32) -> Option<ScopeCost> {
        if !super::COMPILED_IN {
            return None;
        }
        let was = super::recording();
        let rounds = rounds.max(1);
        let mut best = [f64::INFINITY; 6];
        for _ in 0..rounds {
            let mut take = |i: usize, ns: f64| best[i] = best[i].min(ns);
            take(
                0,
                per_call(round_ms, |i| {
                    std::hint::black_box(i + 1);
                }),
            );
            super::set_recording(false);
            take(
                1,
                per_call(round_ms, |i| {
                    scoped(i);
                }),
            );
            super::set_recording(true);
            take(
                2,
                per_call(round_ms, |i| {
                    scoped(i);
                }),
            );
            take(
                3,
                per_call(round_ms, |i| {
                    counted(i as usize);
                }),
            );
            take(
                4,
                per_call(round_ms, |_| {
                    named("scripts/kart.rhai");
                }),
            );
            take(
                5,
                per_call(round_ms, |_| {
                    clock_pair();
                }),
            );
        }
        super::set_recording(was);
        let (clock_step_ns, clock_step_median_ns) = clock_steps(2000);
        Some(ScopeCost {
            bare_ns: best[0],
            off_ns: best[1],
            on_ns: best[2],
            counted_ns: best[3],
            named_ns: best[4],
            clock_pair_ns: best[5],
            clock_step_ns,
            clock_step_median_ns,
            round_ms,
            rounds,
        })
    }

    /// `cargo test -p flint-core --release --features trace --lib scope_cost
    /// -- --ignored --nocapture`. Ignored: it turns recording off and on,
    /// which every thread's scopes share.
    #[cfg(all(test, any(debug_assertions, feature = "trace")))]
    #[test]
    #[ignore]
    fn scope_cost() {
        std::thread::Builder::new()
            .name("profile-cost".into())
            .spawn(|| {
                let cost = measure(200.0, 5).expect("the recorder is in this build");
                println!("{}", cost.line());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// What a save's copy costs (#1267): 90 000 scopes, #1096's coverage of
    /// a 5 s window at 60 fps, recorded as the Overgrown Ruins' frames
    /// record them (105 scopes, 34 with a script's file, a pass's label or
    /// a stage, 20 with a count). `cargo test -p flint-core --release
    /// --features trace --lib snapshot_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn snapshot_cost() {
        const FRAMES: usize = 858;
        const SCOPES: usize = FRAMES * 105;
        // An unbounded ring keeps every text, as #1259's sized one does
        super::set_ring_capacity(0);
        super::set_recording(true);
        let texts: Vec<String> = (0..34)
            .map(|i| match i % 3 {
                0 => format!("scripts/overgrown_ruins_{i:02}.rhai"),
                1 => format!("encode_part bloom {i:02}"),
                _ => format!("subsystems {i:02}"),
            })
            .collect();
        std::thread::Builder::new()
            .name("profile-snapshot-cost".into())
            .spawn(move || {
                for _ in 0..FRAMES {
                    for scope in 0..105 {
                        match texts.get(scope) {
                            Some(text) => {
                                named(text);
                            }
                            None if scope < 54 => {
                                counted(scope);
                            }
                            None => {
                                scoped(scope as u64);
                            }
                        }
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
        super::set_recording(false);
        super::set_ring_capacity(super::DEFAULT_RING_CAPACITY);
        let mine = |threads: &[super::ThreadScopes]| {
            threads
                .iter()
                .find(|t| t.thread == "profile-snapshot-cost")
                .map(|t| t.scopes.len())
        };
        let ms = |start: std::time::Instant| start.elapsed().as_secs_f64() * 1e3;
        // The frame thread's copy, the least of seven, into memory as
        // `before` leaves it
        let copy = |before: &dyn Fn()| {
            let mut least = f64::MAX;
            for _ in 0..7 {
                before();
                let start = std::time::Instant::now();
                let records = super::snapshot_records(None);
                least = least.min(ms(start));
                assert_eq!(mine(&records.scopes()), Some(SCOPES));
            }
            least
        };
        let cold = copy(&|| super::drop_spares());
        let warmed = copy(&|| {
            super::drop_spares();
            super::warm_copies().unwrap().join().unwrap();
        });
        let reused = copy(&|| {});
        let mut read = f64::MAX;
        for _ in 0..7 {
            let records = super::snapshot_records(None);
            let start = std::time::Instant::now();
            let taken = records.scopes();
            read = read.min(ms(start));
            assert_eq!(mine(&taken), Some(SCOPES));
        }
        println!(
            "{SCOPES} scopes, the frame thread's copy: into fresh memory {cold:.2} ms, \
             warmed ahead {warmed:.2} ms, reused from the last save {reused:.2} ms; \
             the writer's read-out {read:.2} ms"
        );
    }
}
