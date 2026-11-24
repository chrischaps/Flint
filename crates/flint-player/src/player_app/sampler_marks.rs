//! Hitches on a sampling profiler's timeline (#1250): with `--hitch-log`,
//! each hitch is also marked where a sampler records it, at its own time,
//! so a hitch is found on the sampler's timeline by its name, not by
//! matching the shape of its samples.
//!
//! - **Linux**: samply's marker file. A file named `marker-<pid>.txt`
//!   beside the hitch log, mapped into memory once (samply sees the
//!   mapping and reads the file when the recording ends), holds a line
//!   `<start> <end> <name>` a hitch, in `CLOCK_MONOTONIC` nanoseconds:
//!   the clock samply's timeline counts on. Its first line marks the
//!   frame clock's zero, `wall_time` 0, so any moment of the log maps.
//! - **Windows**: an ETW TraceLogging event a hitch, from the provider
//!   [`PROVIDER`] (`Flint.Player`). A session that enables the provider
//!   records them on the kernel's timeline beside the samples (WPA's
//!   Generic Events; `samply import --unknown-event-markers`). An event
//!   is timestamped as it is written, on the frame after the hitch, so
//!   it carries the `wall_time` it was written at (`now_us`) beside the
//!   hitch's own (`wall_time_us`, `duration_us`): the hitch began
//!   `now_us - wall_time_us` before the event. A `WallTimeZero { now_us }`
//!   event at the log's opening does the same for the whole run.
//!
//! Nothing here runs without `--hitch-log`. With it, the cost is one file
//! (Linux) or one provider registration (Windows) at the start, and per
//! hitch a line or an event; ETW drops an event no session listens for
//! after one load and compare. macOS and the web mark nothing.

use super::hitch::Hitch;
use std::path::Path;
use web_time::Instant;

/// The ETW provider's name. Its GUID is the name's hash (the EventSource
/// rule every ETW tool knows): [`PROVIDER_GUID`].
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const PROVIDER: &str = "Flint.Player";
/// `Flint.Player`'s GUID, for `xperf -on` and WPR profiles.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const PROVIDER_GUID: &str = "89d78a2a-cb1c-507a-4e4b-896621b7b194";

/// Where a run's hitches are marked, for its report.
pub(crate) struct SamplerMarks {
    inner: Option<platform::Marks>,
    /// What the report says of them.
    said: String,
}

impl SamplerMarks {
    /// Mark the hitches of a log written to `log`, whose `wall_time`
    /// counts from `zero`. A mark that can't be made is said, never fatal:
    /// the log is the run's output, the marks a help.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the web opens no log
    pub(crate) fn open(log: &Path, zero: Instant) -> Self {
        match platform::Marks::open(log, zero) {
            Ok((marks, said)) => Self {
                inner: Some(marks),
                said,
            },
            Err(why) => {
                tracing::warn!(target: "perf", "--hitch-log: no sampler marks: {why}");
                Self {
                    inner: None,
                    said: format!("none ({why})"),
                }
            }
        }
    }

    /// Mark `h`, found just now.
    pub(crate) fn hitch(&mut self, h: &Hitch) {
        if let Some(marks) = &mut self.inner {
            marks.hitch(h);
        }
    }

    /// Where the marks go, for the report's `hitch log` line.
    pub(crate) fn said(&self) -> &str {
        &self.said
    }
}

/// A hitch's name on the sampler's timeline: what it was, then where it
/// is in the log.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn mark_name(class: &str, ms: f32, frame: u64, wall_time: f64, cause: &str) -> String {
    format!("hitch {ms:.1} ms {class}: {cause} (frame {frame}, wall_time {wall_time:.3})")
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{mark_name, Hitch};
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use web_time::Instant;

    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }

    unsafe extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    const CLOCK_MONOTONIC: i32 = 1;
    const PROT_READ: i32 = 1;
    const PROT_EXEC: i32 = 4;
    const MAP_PRIVATE: i32 = 2;
    const MAP_FAILED: *mut u8 = !0usize as *mut u8;

    /// `CLOCK_MONOTONIC` now, nanoseconds: the clock samply and perf
    /// stamp samples with, and the one `Instant` reads on Linux.
    pub(super) fn monotonic_ns() -> u64 {
        let mut ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a plain libc call writing the struct it is handed
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }

    pub(super) struct Marks {
        file: File,
        path: PathBuf,
        /// `wall_time` 0 on `CLOCK_MONOTONIC`.
        zero_ns: u64,
    }

    impl Marks {
        pub(super) fn open(log: &Path, zero: Instant) -> Result<(Self, String), String> {
            let dir = log.parent().filter(|p| !p.as_os_str().is_empty());
            let dir = dir.unwrap_or(Path::new("."));
            let path = dir.join(format!("marker-{}.txt", std::process::id()));
            // `wall_time` 0 on the monotonic clock: now, less the time since
            let since = Instant::now().saturating_duration_since(zero);
            let zero_ns = monotonic_ns().saturating_sub(since.as_nanos() as u64);
            // Readable too: a mapping needs a descriptor open for reading
            // (`File::create` alone is write-only, and mmap says EACCES)
            #[allow(clippy::disallowed_methods)] // a write: the run's output, beside the log
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            // A mapping of an empty file fails: the zero's line comes first
            writeln!(file, "{zero_ns} {zero_ns} flint wall_time 0")
                .and_then(|_| file.flush())
                .map_err(|e| format!("{}: {e}", path.display()))?;
            map_once(&file).map_err(|e| format!("{}: mmap: {e}", path.display()))?;
            let said = format!("samply marker file {}", path.display());
            Ok((
                Self {
                    file,
                    path,
                    zero_ns,
                },
                said,
            ))
        }

        pub(super) fn hitch(&mut self, h: &Hitch) {
            let start = self.zero_ns + (h.at * 1e9) as u64;
            let end = start + (h.ms as f64 * 1e6) as u64;
            let name = mark_name(h.class(), h.ms, h.index, h.at, &h.cause_line());
            let wrote = writeln!(self.file, "{start} {end} {name}").and_then(|_| self.file.flush());
            if let Err(e) = wrote {
                tracing::warn!(target: "perf", "{}: {e}", self.path.display());
            }
        }
    }

    /// Map the file once, so the sampler hears its name (samply records
    /// every mapping; perf records executable ones, hence `PROT_EXEC`
    /// where the mount allows it). The mapping is left in place: it is a
    /// page, and unmapping it would tell the sampler nothing.
    fn map_once(file: &File) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        for prot in [PROT_READ | PROT_EXEC, PROT_READ] {
            // SAFETY: a fresh private read-only mapping of a file we hold
            // open; nothing reads through it
            let at = unsafe { mmap(std::ptr::null_mut(), 4096, prot, MAP_PRIVATE, fd, 0) };
            if at != MAP_FAILED {
                return Ok(());
            }
        }
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
mod platform {
    use super::{mark_name, Hitch, PROVIDER};
    use std::path::Path;
    use std::sync::Once;
    use tracelogging as tlg;
    use web_time::Instant;

    tlg::define_provider!(FLINT_PLAYER, "Flint.Player");

    static REGISTER: Once = Once::new();

    pub(super) struct Marks {
        zero: Instant,
    }

    impl Marks {
        pub(super) fn open(_log: &Path, zero: Instant) -> Result<(Self, String), String> {
            let mut status = 0;
            // SAFETY: an exe's provider, registered once and never
            // unloaded with a DLL
            REGISTER.call_once(|| status = unsafe { FLINT_PLAYER.register() });
            if status != 0 {
                return Err(format!("EventRegister: error {status}"));
            }
            let marks = Self { zero };
            let now_us = marks.now_us();
            tlg::write_event!(
                FLINT_PLAYER,
                "WallTimeZero",
                level(Informational),
                u64("now_us", &now_us)
            );
            let said = format!("ETW provider {PROVIDER} ({})", super::PROVIDER_GUID);
            Ok((marks, said))
        }

        /// `wall_time` now, whole microseconds.
        fn now_us(&self) -> u64 {
            Instant::now()
                .saturating_duration_since(self.zero)
                .as_micros() as u64
        }

        /// Fields of the types every reader decodes: samply's ETW reader
        /// shows integers and NUL-terminated strings, and calls a double
        /// or a counted string "Unknown InType" (measured, 0.13.1), so
        /// times go as whole microseconds and text as `cstr8`.
        pub(super) fn hitch(&mut self, h: &Hitch) {
            if !FLINT_PLAYER.enabled(tlg::Level::Informational, 0) {
                return;
            }
            let now_us = self.now_us();
            let cause = h.cause_line();
            let name = mark_name(h.class(), h.ms, h.index, h.at, &cause);
            let wall_time_us = (h.at * 1e6).round() as u64;
            let duration_us = (h.ms as f64 * 1e3).round() as u64;
            tlg::write_event!(
                FLINT_PLAYER,
                "Hitch",
                level(Informational),
                cstr8("name", &name),
                u64("frame", &h.index),
                u64("wall_time_us", &wall_time_us),
                u64("duration_us", &duration_us),
                u64("now_us", &now_us),
                cstr8("class", h.class()),
                cstr8("cause", &cause),
            );
        }
    }

    /// The provider as registered, for the tests.
    #[cfg(test)]
    pub(super) fn provider_guid() -> tlg::Guid {
        *FLINT_PLAYER.id()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::Hitch;
    use std::path::Path;
    use web_time::Instant;

    pub(super) struct Marks;

    impl Marks {
        pub(super) fn open(_log: &Path, _zero: Instant) -> Result<(Self, String), String> {
            Err("this platform has no sampler marks".into())
        }

        pub(super) fn hitch(&mut self, _h: &Hitch) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marks_name_says_what_the_hitch_was_and_where_it_is_in_the_log() {
        assert_eq!(
            mark_name("player", 101.24, 412, 6.1224, "script stall.rhai:on_update 100.1 ms"),
            "hitch 101.2 ms player: script stall.rhai:on_update 100.1 ms (frame 412, wall_time 6.122)"
        );
    }

    /// What a run pays with no sampler listening: the opening, and a
    /// hitch's mark. `cargo test -p flint-player --lib marks_cost --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn marks_cost() {
        let dir = std::env::temp_dir().join(format!("flint-marks-cost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zero = Instant::now();
        let t = Instant::now();
        let mut marks = SamplerMarks::open(&dir.join("hitches.jsonl"), zero);
        let open_us = t.elapsed().as_secs_f64() * 1e6;
        let h = Hitch {
            index: 150,
            at: 2.5,
            game_time: 2.5,
            ms: 100.0,
            judged_ms: 100.0,
            median_ms: 16.7,
            player_ms: 98.0,
            gpu_ms: None,
            refreshes: None,
            cause: super::super::hitch::Cause::Elsewhere {
                acquire_ms: 98.0,
                present_ms: 0.1,
            },
            loading: false,
            notes: Vec::new(),
        };
        const N: u32 = 2000;
        let t = Instant::now();
        for _ in 0..N {
            marks.hitch(&h);
        }
        let mark_us = t.elapsed().as_secs_f64() * 1e6 / N as f64;
        println!(
            "sampler marks ({}): open {open_us:.1} us, a hitch's mark {mark_us:.3} us",
            marks.said()
        );
        drop(marks);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn the_provider_guid_the_book_names_is_the_providers() {
        let named = tracelogging::Guid::try_parse(PROVIDER_GUID).expect("a GUID");
        assert_eq!(platform::provider_guid(), named, "the GUID of {PROVIDER}");
        assert_eq!(tracelogging::Guid::from_name(PROVIDER), named);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_marker_file_holds_the_zero_and_each_hitch_on_the_monotonic_clock() {
        let dir = std::env::temp_dir().join(format!("flint-marks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zero = Instant::now();
        let before = platform::monotonic_ns();
        let mut marks = SamplerMarks::open(&dir.join("hitches.jsonl"), zero);
        let file = dir.join(format!("marker-{}.txt", std::process::id()));
        assert!(
            marks.said().starts_with("samply marker file "),
            "{}",
            marks.said()
        );
        let h = Hitch {
            index: 150,
            at: 2.5,
            game_time: 2.5,
            ms: 100.0,
            judged_ms: 100.0,
            median_ms: 16.7,
            player_ms: 2.0,
            gpu_ms: None,
            refreshes: None,
            cause: crate::player_app::hitch::Cause::Elsewhere {
                acquire_ms: 98.0,
                present_ms: 0.1,
            },
            loading: false,
            notes: Vec::new(),
        };
        marks.hitch(&h);
        #[allow(clippy::disallowed_methods)] // a test reading back the marker file it wrote
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let nums = |l: &str| -> (u64, u64) {
            let mut w = l.split(' ');
            (
                w.next().unwrap().parse().unwrap(),
                w.next().unwrap().parse().unwrap(),
            )
        };
        let (z0, z1) = nums(lines[0]);
        assert_eq!(z0, z1);
        assert!(
            z0 <= before && before - z0 < 50_000_000,
            "zero {z0} against {before}"
        );
        let (s, e) = nums(lines[1]);
        assert_eq!(s - z0, 2_500_000_000);
        assert_eq!(e - s, 100_000_000);
        assert!(lines[1].contains("hitch 100.0 ms"), "{}", lines[1]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
