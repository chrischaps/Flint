//! Putting a helper thread behind the game (#1178): a frame capture's
//! writer encodes about 6 MB of PNG and EXR on a quarter of the cores while
//! the game plays on. At the operating system's normal priority it competes
//! with the frame thread for the cores and the disk, and the frames after a
//! capture paid for it (the hot-reload poll's `stat`s took 6-21 ms against
//! under 1 ms, measured in #1086). [`enter`] asks the OS to run the calling
//! thread only when the game's threads leave it room:
//!
//! - **Windows**: `THREAD_MODE_BACKGROUND_BEGIN`, which lowers the thread's
//!   CPU, I/O and memory priorities together (the scheduler's background
//!   mode, meant for exactly this).
//! - **Linux**: nice 10 for the thread (`setpriority` on Linux is per thread).
//! - **macOS**: the background QoS class, which also throttles its I/O.
//!
//! Elsewhere it does nothing. Each thread asks for itself: a thread it then
//! spawns is not lowered on Windows, so every encoder thread calls it too.
//!
//! **What it costs** (measured, release, 1280x720, vsync, 24 captures a
//! scene and priority, on a machine other sessions were building on): the
//! tavern's seconds after a write had no frame over 25 ms (four with a
//! normal-priority writer, two of them 50 ms hitches); the woozy grove's were
//! no better. The writes' medians were the same (about 0.46 s tavern, 0.6-0.7
//! s grove), but their tail grew: the slowest background writes took 7.7 and
//! 7.9 s, against 1.8 and 2.8 s at normal priority, since an idle-class
//! thread with very low I/O priority waits behind every other process. A
//! long write holds its read-back (about 75 MB) and keeps `--capture-on-hitch`
//! passing over hitches until it ends. `THREAD_PRIORITY_LOWEST` (CPU only,
//! within the normal class) is the milder setting not yet measured.

/// Lower the calling thread's priority. Whether the OS took it.
#[cfg(windows)]
pub(crate) fn enter() -> bool {
    // kernel32, which std links on every Windows target
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
    // SAFETY: the pseudo handle of the calling thread, always valid
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) != 0 }
}

/// Lower the calling thread's priority. Whether the OS took it.
#[cfg(target_os = "linux")]
pub(crate) fn enter() -> bool {
    unsafe extern "C" {
        fn setpriority(which: i32, who: u32, prio: i32) -> i32;
    }
    const PRIO_PROCESS: i32 = 0;
    // SAFETY: plain libc call; `who` 0 is the calling thread on Linux
    unsafe { setpriority(PRIO_PROCESS, 0, 10) == 0 }
}

/// Lower the calling thread's priority. Whether the OS took it.
#[cfg(target_os = "macos")]
pub(crate) fn enter() -> bool {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }
    const QOS_CLASS_BACKGROUND: u32 = 0x09;
    // SAFETY: plain libpthread call on the calling thread
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0) == 0 }
}

/// Lower the calling thread's priority: nothing to do on this platform.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn enter() -> bool {
    false
}

#[cfg(test)]
mod tests {
    // macOS's new thread has no QoS class to compare against: only that
    // the call is taken there
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn a_thread_that_enters_the_background_runs_below_the_game() {
        let lowered = std::thread::spawn(|| (super::enter(), priority()))
            .join()
            .expect("the thread ran");
        assert!(lowered.0, "the OS took the request");
        let (normal, below) = (priority_of_new_thread(), lowered.1);
        assert!(
            below < normal,
            "a background thread runs below a new one: {below} against {normal}"
        );
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn priority_of_new_thread() -> i32 {
        std::thread::spawn(priority).join().expect("the thread ran")
    }

    /// The calling thread's priority, higher runs first.
    #[cfg(windows)]
    fn priority() -> i32 {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn GetThreadPriority(thread: isize) -> i32;
        }
        // SAFETY: the calling thread's pseudo handle
        unsafe { GetThreadPriority(GetCurrentThread()) }
    }

    #[cfg(target_os = "linux")]
    fn priority() -> i32 {
        unsafe extern "C" {
            fn getpriority(which: i32, who: u32) -> i32;
        }
        // SAFETY: plain libc call; nice is lower-runs-first, so negated
        -unsafe { getpriority(0, 0) }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_thread_can_enter_the_background() {
        assert!(std::thread::spawn(super::enter)
            .join()
            .expect("the thread ran"));
    }
}
