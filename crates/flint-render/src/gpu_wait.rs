//! The one place Flint blocks on the GPU (#948).
//!
//! Every wait for submitted work (a read-back, a timer resolve, the grass
//! compute's count) calls [`wait_for_gpu`] instead of
//! `device.poll(PollType::wait_indefinitely())` (clippy's
//! `disallowed_methods` holds it).
//!
//! wgpu 23 capped every `Maintain::Wait` at wgpu-core's 60 s
//! `CLEANUP_WAIT_MS` and ignored the timeout: a WARP submission past it had
//! its allocators recycled under it and the process died (#910, #912).
//! wgpu 27 removed the cap. `PollType::Wait` now waits for as long as it
//! takes, or for its own `timeout`, and returns `PollError::Timeout` then.
//!
//! So by default a wait has no limit, as a game wants: a slow frame is
//! still a frame. `FLINT_GPU_WAIT_TIMEOUT=<seconds>` gives every wait that
//! limit, and a wait that reaches it panics, naming the variable. CI's GPU
//! job sets it, so a hung GPU fails the step in minutes with that message,
//! not at the job's 60-minute timeout with none. It panics rather than
//! returning the error because the work it waited on is still queued: the
//! next wait on that device would block behind it too.

use std::sync::OnceLock;
use std::time::Duration;

/// `FLINT_GPU_WAIT_TIMEOUT` in seconds; `None` (no limit) when unset, `0`
/// or not a number (which warns).
pub fn gpu_wait_timeout() -> Option<Duration> {
    static TIMEOUT: OnceLock<Option<Duration>> = OnceLock::new();
    *TIMEOUT.get_or_init(|| timeout_of(flint_core::env::GPU_WAIT_TIMEOUT.number()))
}

/// The limit of a timeout the table accepted: `0` is none.
fn timeout_of(secs: Option<f64>) -> Option<Duration> {
    secs.filter(|s| *s > 0.0).map(Duration::from_secs_f64)
}

/// Block until the device has finished the work submitted to it, for at
/// most [`gpu_wait_timeout`]. Any other error (a lost device) is returned
/// for the caller, as `device.poll` returns it.
///
/// # Panics
/// When the wait reaches `FLINT_GPU_WAIT_TIMEOUT`.
pub fn wait_for_gpu(device: &wgpu::Device) -> Result<wgpu::PollStatus, wgpu::PollError> {
    let timeout = gpu_wait_timeout();
    match device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout,
    }) {
        Err(wgpu::PollError::Timeout) => panic!(
            "the GPU didn't finish its work within {:.0} s (FLINT_GPU_WAIT_TIMEOUT): \
             the device is hung (#948)",
            timeout.unwrap_or_default().as_secs_f64()
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeout_is_seconds_and_unset_means_none() {
        let var = &flint_core::env::GPU_WAIT_TIMEOUT;
        let parse = |s: &str| match var.parse(s) {
            Ok(flint_core::env::Value::Number(n)) => timeout_of(Some(n)),
            _ => None,
        };
        assert_eq!(timeout_of(None), None);
        assert_eq!(parse("0"), None);
        for bad in ["-3", "soon", "inf"] {
            assert!(var.parse(bad).is_err(), "{bad} warns");
        }
        assert_eq!(parse("240"), Some(Duration::from_secs(240)));
        assert_eq!(parse(" 1.5 "), Some(Duration::from_millis(1500)));
    }
}
