//! Engine event name constants.
//!
//! Sentinel values used for cross-crate event communication.

/// Internal event fired by `complete_transition()` in scripts,
/// intercepted by `PlayerApp` to advance the scene transition state machine.
pub const TRANSITION_COMPLETE: &str = "__transition_complete";
