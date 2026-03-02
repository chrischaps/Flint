//! Texture pattern generators.
//!
//! This module provides the pattern generation layer — "base shape" producers
//! that turn texture parameters into per-pixel fields (cell IDs, heights, edge
//! distances). These fields are consumed by the map derivation stage to produce
//! albedo, normal, and roughness textures.
//!
//! Three pattern families cover the most common procedural material types:
//!
//! - **Voronoi/Brick** ([`VoronoiBrickPattern`]) — stone walls, cobblestone, cracked earth
//! - **Organic** ([`PerlinOrganicPattern`]) — layered rock, dirt, bark, natural surfaces
//! - **Grid** ([`TilingGridPattern`]) — manufactured tiles, bricks, panels

pub mod grid;
pub mod maps;
pub mod organic;
pub mod pattern;
pub mod voronoi;

pub use grid::{TilingGridParams, TilingGridPattern};
pub use maps::{derive_maps, TextureMapParams};
pub use organic::{PerlinOrganicParams, PerlinOrganicPattern};
pub use pattern::{Pattern, PatternCell, PatternField};
pub use voronoi::{VoronoiBrickParams, VoronoiBrickPattern};
