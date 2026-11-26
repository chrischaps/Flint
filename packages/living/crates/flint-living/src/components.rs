//! The living props' component names (#1330; it was in
//! `flint_core::components`, so the bottom crate named a feature's
//! component). A build without living props links none of this crate, and
//! the player says so by the name it writes itself (`missing_features.rs`).

/// A tree, bush or other spec-made prop that holds a pose and glides to the
/// next on the wooze field's clock (#211).
pub const LIVING_PROP: &str = "living_prop";
