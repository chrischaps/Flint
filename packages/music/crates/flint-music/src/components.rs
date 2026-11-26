//! The music's component names (#1330; it was in `flint_core::components`).
//! A build without music links none of this crate, and the player says so by
//! the name it writes itself (`missing_features.rs`).

/// A scene's suite (#745): its stems play on their buses, sample-locked, and
/// scripts read the beat; with a `chart`, the rhythm layer judges it too.
/// Read by flint-player's music subsystem.
pub const MUSIC_SESSION: &str = "music_session";
