//! What hot reload knows about the disk, asked without waiting on it (#633).
//!
//! Every hot-reload poll (scripts and `scripts/lib`, UI documents, user
//! styles, glyph sets, palettes, style images) asks two questions over and
//! over: what is this file's stamp (modified time and length), and what is in
//! this folder. Asked on the frame thread, either can wait: a stat or a read
//! of a file another process is writing (or a virus scanner is holding) took
//! seconds on Windows, and that frame took seconds with it.
//!
//! This module answers both, in one of three [`Mode`]s:
//!
//! - [`Mode::Direct`], the default: each call goes to the disk then and
//!   there, as the polls always did. Tools and tests that poll once and expect
//!   the answer at once keep working unchanged.
//! - [`Mode::Background`], from [`start`] (the player calls it): the first
//!   call for a path stats it then (at load, when its file is being read
//!   anyway) and from then on a watcher thread keeps the answer fresh, so a
//!   poll only takes a lock and copies a value. The thread publishes a change
//!   only once the file has held still for two of its polls and it has read
//!   the whole file once, so the reload that follows reads a file nobody is
//!   still writing, and one already in the OS cache. A change arrives a
//!   quarter to half a second after the last write.
//! - [`Mode::Off`], from [`disable`] (`--no-hot-reload`): nothing is stamped
//!   or listed, so nothing ever looks changed and the disk is never polled.
//!
//! Native only: hot reload reads the disk with `std::fs`, never the vfs, and
//! a web bundle never changes, so on wasm every stamp is `None` and every
//! folder empty, as the polls' own `cfg`s already had it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file's modified time and length (a coarse clock can miss a quick edit's
/// time; its length often differs).
pub type Stamp = (SystemTime, u64);

/// One folder entry: its path (the folder joined with its name, as
/// `std::fs::read_dir(..).path()` gives it) and whether it is a folder.
pub type Entry = (PathBuf, bool);

/// How the answers are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Stat and list on the caller's thread, every call (the default).
    Direct,
    /// Answer from what the watcher thread last settled on.
    Background,
    /// Hot reload is off: nothing is stamped, every folder is empty.
    Off,
}

/// The current mode.
pub fn mode() -> Mode {
    imp::mode()
}

/// Answer from the watcher thread from now on, unless hot reload is off.
/// Natively only; on wasm this does nothing.
pub fn start() {
    imp::start()
}

/// Turn hot reload off for the rest of the process (`--no-hot-reload`):
/// every poll then sees nothing change, and [`start`] no longer turns it on.
pub fn disable() {
    imp::disable()
}

/// `path`'s stamp, `None` when it doesn't exist (or hot reload is off).
pub fn stamp(path: &Path) -> Option<Stamp> {
    imp::stamp(path)
}

/// `path`'s modified time, `None` when it doesn't exist.
pub fn modified(path: &Path) -> Option<SystemTime> {
    stamp(path).map(|(at, _)| at)
}

/// `path`'s stamp from the disk now, whatever the mode: for a thread of
/// hot reload's own (never a frame) checking that a file held still while
/// it read it.
pub fn stamp_now(path: &Path) -> Option<Stamp> {
    imp::stamp_now(path)
}

/// Read `path` whole once it holds still: its stamp the same before and
/// after the read, tried a few times a tenth of a second apart. The text and
/// the stamp it was read at, or why not. For hot reload's own threads; on a
/// frame thread it would wait on the disk, which is what this module avoids.
pub fn read_settled(path: &Path) -> Result<(String, Option<Stamp>), String> {
    let mut tries = 0;
    loop {
        let before = stamp_now(path);
        let text = crate::vfs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        if stamp_now(path) == before {
            return Ok((text, before));
        }
        tries += 1;
        if tries >= 5 {
            return Err(format!("{} is still being written", path.display()));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The entries directly inside `dir`, sorted by path; `None` when it can't
/// be read (or hot reload is off).
pub fn list(dir: &Path) -> Option<Vec<Entry>> {
    imp::list(dir)
}

/// Every file under `dir` (recursively) whose extension is `ext`, with its
/// stamp: a snapshot to compare against the last one. Empty when the folder
/// doesn't exist.
pub fn tree(dir: &Path, ext: &str) -> std::collections::BTreeMap<PathBuf, Stamp> {
    fn walk(dir: &Path, ext: &str, out: &mut std::collections::BTreeMap<PathBuf, Stamp>) {
        for (path, is_dir) in list(dir).unwrap_or_default() {
            if is_dir {
                walk(&path, ext, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                if let Some(stamp) = stamp(&path) {
                    out.insert(path, stamp);
                }
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, ext, &mut out);
    out
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;

    pub fn mode() -> Mode {
        Mode::Off
    }
    pub fn start() {}
    pub fn disable() {}
    pub fn stamp(_: &Path) -> Option<Stamp> {
        None
    }
    pub fn stamp_now(_: &Path) -> Option<Stamp> {
        None
    }
    pub fn list(_: &Path) -> Option<Vec<Entry>> {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::OnceLock;

    static MODE: AtomicU8 = AtomicU8::new(DIRECT);
    const DIRECT: u8 = 0;
    const BACKGROUND: u8 = 1;
    const OFF: u8 = 2;

    /// How often the process's watcher looks at the disk.
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

    fn global() -> &'static Watcher {
        static GLOBAL: OnceLock<Watcher> = OnceLock::new();
        GLOBAL.get_or_init(|| Watcher::spawn(INTERVAL))
    }

    pub fn mode() -> Mode {
        match MODE.load(Ordering::Relaxed) {
            BACKGROUND => Mode::Background,
            OFF => Mode::Off,
            _ => Mode::Direct,
        }
    }

    pub fn start() {
        let _ = MODE.compare_exchange(DIRECT, BACKGROUND, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub fn disable() {
        MODE.store(OFF, Ordering::Relaxed);
    }

    pub fn stamp(path: &Path) -> Option<Stamp> {
        match mode() {
            Mode::Direct => disk::stamp(path),
            Mode::Background => global().stamp(path),
            Mode::Off => None,
        }
    }

    pub fn stamp_now(path: &Path) -> Option<Stamp> {
        disk::stamp(path)
    }

    pub fn list(dir: &Path) -> Option<Vec<Entry>> {
        match mode() {
            Mode::Direct => disk::list(dir),
            Mode::Background => global().list(dir),
            Mode::Off => None,
        }
    }

    /// The disk itself, the only place this module touches it.
    #[allow(clippy::disallowed_methods)] // hot reload polls the disk; native only (#97)
    pub(super) mod disk {
        use super::*;

        pub fn stamp(path: &Path) -> Option<Stamp> {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        }

        pub fn list(dir: &Path) -> Option<Vec<Entry>> {
            let mut entries: Vec<Entry> = std::fs::read_dir(dir)
                .ok()?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let path = e.path();
                    // A link to a folder is a folder, as `Path::is_dir` says
                    let is_dir = match e.file_type() {
                        Ok(t) if t.is_symlink() => path.is_dir(),
                        Ok(t) => t.is_dir(),
                        Err(_) => false,
                    };
                    (path, is_dir)
                })
                .collect();
            entries.sort();
            Some(entries)
        }

        /// Read the whole file and throw it away: whatever holds it (a
        /// writer, a scanner) is waited out here, and the reload that follows
        /// reads it from the OS cache.
        pub fn read_through(path: &Path) {
            use std::io::Read;
            if let Ok(mut file) = std::fs::File::open(path) {
                let mut buf = vec![0u8; 64 * 1024];
                while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
            }
        }
    }

    pub(super) use watcher::Watcher;

    #[cfg(test)]
    pub(super) fn watcher_pending() -> watcher::Pending {
        Default::default()
    }

    mod watcher {
        use super::*;
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        /// A path asked about, and how: its stamp, or its folder's entries.
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub(crate) enum Key {
            Stamp(PathBuf),
            List(PathBuf),
        }

        #[derive(Clone, PartialEq)]
        pub(crate) enum Answer {
            Stamp(Option<Stamp>),
            List(Option<Vec<Entry>>),
        }

        impl Key {
            fn ask_disk(&self) -> Answer {
                match self {
                    Key::Stamp(p) => Answer::Stamp(disk::stamp(p)),
                    Key::List(p) => Answer::List(disk::list(p)),
                }
            }
        }

        /// Answers seen once but not yet settled on, the thread's own.
        pub(crate) type Pending = HashMap<Key, (Answer, u32)>;

        struct Known {
            answer: Answer,
            asked: Instant,
        }

        /// A path nobody has asked about for this long stops being watched
        /// (a scene that's gone, a style turned off). Asked again, it is
        /// stat'd once more on the asker's thread.
        const FORGET_AFTER: Duration = Duration::from_secs(120);

        /// How many polls in a row must agree on a new answer before it is
        /// published: the file has held still for at least one interval.
        const SETTLE_POLLS: u32 = 2;

        /// What the watcher thread has settled on, shared with the askers.
        /// The thread holds the lock only to copy keys out and answers in,
        /// never across a disk call, so an asker never waits on the disk.
        pub struct Watcher {
            known: Arc<Mutex<HashMap<Key, Known>>>,
        }

        impl Watcher {
            /// A watcher with its own thread, looking every `interval`.
            pub fn spawn(interval: Duration) -> Self {
                let known: Arc<Mutex<HashMap<Key, Known>>> = Arc::default();
                let shared = Arc::downgrade(&known);
                let spawned = std::thread::Builder::new()
                    .name("flint-file-watch".into())
                    .spawn(move || {
                        let mut pending = Pending::new();
                        loop {
                            std::thread::sleep(interval);
                            // Gone with its watcher (tests make their own)
                            let Some(known) = shared.upgrade() else {
                                return;
                            };
                            look_once(&known, &mut pending);
                        }
                    });
                if let Err(e) = spawned {
                    // Nothing is watched, so nothing reloads; the game runs
                    eprintln!("[hot reload] couldn't start the file watcher: {e}");
                }
                Self { known }
            }

            pub fn stamp(&self, path: &Path) -> Option<Stamp> {
                match self.ask(Key::Stamp(path.to_path_buf())) {
                    Answer::Stamp(s) => s,
                    Answer::List(_) => None,
                }
            }

            pub fn list(&self, dir: &Path) -> Option<Vec<Entry>> {
                match self.ask(Key::List(dir.to_path_buf())) {
                    Answer::List(l) => l,
                    Answer::Stamp(_) => None,
                }
            }

            fn ask(&self, key: Key) -> Answer {
                let now = Instant::now();
                {
                    let mut known = lock(&self.known);
                    if let Some(k) = known.get_mut(&key) {
                        k.asked = now;
                        return k.answer.clone();
                    }
                }
                // First time: from the disk, here, once (a load reading the
                // same file anyway), and watched from then on
                let answer = key.ask_disk();
                lock(&self.known).entry(key).or_insert(Known {
                    answer: answer.clone(),
                    asked: now,
                });
                answer
            }
        }

        fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
            m.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// One look at everything watched: forget what's unasked, publish
        /// what has settled on a new answer.
        fn look_once(known: &Mutex<HashMap<Key, Known>>, pending: &mut Pending) {
            let watched: Vec<(Key, Answer)> = {
                let mut known = lock(known);
                known.retain(|_, k| k.asked.elapsed() < FORGET_AFTER);
                known
                    .iter()
                    .map(|(key, k)| (key.clone(), k.answer.clone()))
                    .collect()
            };
            pending.retain(|key, _| watched.iter().any(|(k, _)| k == key));
            for (key, published) in watched {
                let fresh = key.ask_disk();
                if fresh == published {
                    pending.remove(&key);
                    continue;
                }
                let seen = match pending.get_mut(&key) {
                    Some((answer, n)) if *answer == fresh => {
                        *n += 1;
                        *n
                    }
                    _ => {
                        pending.insert(key.clone(), (fresh.clone(), 1));
                        1
                    }
                };
                if seen < SETTLE_POLLS {
                    continue;
                }
                pending.remove(&key);
                if let (Key::Stamp(path), Answer::Stamp(Some(_))) = (&key, &fresh) {
                    disk::read_through(path);
                    // Written again while it was read: settle again
                    if key.ask_disk() != fresh {
                        continue;
                    }
                }
                if let Some(k) = lock(known).get_mut(&key) {
                    k.answer = fresh;
                }
            }
        }

        #[cfg(test)]
        impl Watcher {
            /// One look, on the caller's thread.
            pub fn look_now(&self, pending: &mut Pending) {
                look_once(&self.known, pending);
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::imp::{disk, Watcher};
    use std::time::Duration;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint-file-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A watcher whose thread never looks in a test's lifetime, so the test
    /// steps it with `look_now`.
    fn stepped() -> Watcher {
        Watcher::spawn(Duration::from_secs(3600))
    }

    #[test]
    fn a_change_is_published_once_it_holds_still() {
        let dir = temp_dir("settle");
        let file = dir.join("a.rhai");
        std::fs::write(&file, "let a = 1;").unwrap();
        let w = stepped();
        let mut pending = super::imp::watcher_pending();
        let first = w.stamp(&file);
        assert_eq!(first, disk::stamp(&file));

        std::fs::write(&file, "let a = 22;").unwrap();
        assert_eq!(w.stamp(&file), first, "polls never read the disk");
        w.look_now(&mut pending);
        assert_eq!(
            w.stamp(&file),
            first,
            "one look: still being written, maybe"
        );
        w.look_now(&mut pending);
        assert_eq!(w.stamp(&file), disk::stamp(&file), "two alike: settled");
        assert_ne!(w.stamp(&file), first);
    }

    #[test]
    fn a_file_still_being_written_is_not_published() {
        let dir = temp_dir("writing");
        let file = dir.join("b.ui.toml");
        std::fs::write(&file, "x").unwrap();
        let w = stepped();
        let mut pending = super::imp::watcher_pending();
        let first = w.stamp(&file);
        for i in 0..6 {
            // a write between every look: never two alike
            std::fs::write(&file, "y".repeat(i + 2)).unwrap();
            w.look_now(&mut pending);
            assert_eq!(w.stamp(&file), first);
        }
        w.look_now(&mut pending);
        assert_eq!(w.stamp(&file), disk::stamp(&file));
    }

    #[test]
    fn appearing_vanishing_and_folders() {
        let dir = temp_dir("list");
        let w = stepped();
        let mut pending = super::imp::watcher_pending();
        let file = dir.join("new.rhai");
        assert_eq!(w.stamp(&file), None);
        assert_eq!(w.list(&dir), Some(vec![]));

        std::fs::write(&file, "1").unwrap();
        std::fs::create_dir(dir.join("lib")).unwrap();
        w.look_now(&mut pending);
        w.look_now(&mut pending);
        assert!(w.stamp(&file).is_some(), "appeared");
        assert_eq!(
            w.list(&dir),
            Some(vec![(dir.join("lib"), true), (file.clone(), false)])
        );

        std::fs::remove_file(&file).unwrap();
        w.look_now(&mut pending);
        w.look_now(&mut pending);
        assert_eq!(w.stamp(&file), None, "vanished");
    }

    #[test]
    fn the_thread_publishes_on_its_own() {
        let dir = temp_dir("thread");
        let file = dir.join("c.wgsl");
        std::fs::write(&file, "a").unwrap();
        let w = Watcher::spawn(Duration::from_millis(10));
        let first = w.stamp(&file);
        std::fs::write(&file, "abc").unwrap();
        let want = disk::stamp(&file);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while w.stamp(&file) == first && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(w.stamp(&file), want);
    }

    #[test]
    fn tree_walks_folders_for_one_extension() {
        let dir = temp_dir("tree");
        std::fs::create_dir_all(dir.join("lib/deep")).unwrap();
        std::fs::write(dir.join("lib/a.rhai"), "1").unwrap();
        std::fs::write(dir.join("lib/deep/b.rhai"), "2").unwrap();
        std::fs::write(dir.join("lib/notes.txt"), "3").unwrap();
        // Direct mode in this test binary unless a test turns it on (none do)
        let tree = super::tree(&dir.join("lib"), "rhai");
        let names: Vec<_> = tree.keys().cloned().collect();
        assert_eq!(
            names,
            vec![dir.join("lib/a.rhai"), dir.join("lib/deep/b.rhai")]
        );
        assert!(super::tree(&dir.join("nope"), "rhai").is_empty());
    }
}
