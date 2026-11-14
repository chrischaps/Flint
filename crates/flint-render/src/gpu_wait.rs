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
/// or not a number.
pub fn gpu_wait_timeout() -> Option<Duration> {
    static TIMEOUT: OnceLock<Option<Duration>> = OnceLock::new();
    *TIMEOUT.get_or_init(|| parse_timeout(std::env::var("FLINT_GPU_WAIT_TIMEOUT").ok().as_deref()))
}

fn parse_timeout(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(Duration::from_secs_f64)
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
        assert_eq!(parse_timeout(None), None);
        assert_eq!(parse_timeout(Some("")), None);
        assert_eq!(parse_timeout(Some("0")), None);
        assert_eq!(parse_timeout(Some("-3")), None);
        assert_eq!(parse_timeout(Some("soon")), None);
        assert_eq!(parse_timeout(Some("inf")), None);
        assert_eq!(parse_timeout(Some("240")), Some(Duration::from_secs(240)));
        assert_eq!(
            parse_timeout(Some(" 1.5 ")),
            Some(Duration::from_millis(1500))
        );
    }
}
