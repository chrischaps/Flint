//! `terrain_v1`: a terrain spec's heightmap and splat map, as a procgen
//! generator (#1168).
//!
//! The terrain is a package of its own (#1623), and the generator wraps its
//! bake, so the body ([`baked`]) is this crate's `terrain` feature: the
//! procgen package's `[with.terrain]` part, which the launchers turn on when
//! a game lists both. Without it a `.terrain.toml` is still known by its
//! name, and loading one says which package it needs.

/// The generator's registered name.
pub const TERRAIN_GENERATOR: &str = "terrain_v1";

/// The ending of a terrain editor's spec file.
pub const TERRAIN_SUFFIX: &str = ".terrain.toml";

/// Whether `path` names a terrain editor's spec (`x.terrain.toml`).
pub fn is_terrain_spec(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(TERRAIN_SUFFIX)
}

#[cfg(feature = "terrain")]
mod baked;
#[cfg(feature = "terrain")]
pub use baked::*;

/// What loading a terrain spec says in a build without the terrain package.
#[cfg(not(feature = "terrain"))]
fn without() -> crate::ProcGenError {
    crate::ProcGenError::SpecParseError(format!(
        "a {TERRAIN_SUFFIX} spec is baked by the terrain package's `{TERRAIN_GENERATOR}`,          which this build doesn't have"
    ))
}

/// A `.terrain.toml`'s text as a `terrain_v1` spec: in this build, the
/// error naming the package it needs.
#[cfg(not(feature = "terrain"))]
pub fn from_terrain_toml(_text: &str) -> crate::Result<crate::ProcGenSpec> {
    Err(without())
}

/// What `flint gen` would say of a `.terrain.toml`: in this build, the error
/// naming the package it needs.
#[cfg(not(feature = "terrain"))]
pub fn check_terrain_toml(_text: &str) -> crate::Result<Vec<crate::ParamIssue>> {
    Err(without())
}
