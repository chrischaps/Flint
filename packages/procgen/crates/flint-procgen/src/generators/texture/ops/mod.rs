//! Composable texture operations for the pipeline pattern.
//!
//! Each [`TextureOp`] is a small, reusable stage that reads and writes named
//! channels on a [`TextureField`]. Ops are chained sequentially to build up
//! PBR texture maps from simple building blocks.
//!
//! # Op families
//!
//! One module per family; every op is re-exported here, so `ops::BrickGridOp`
//! and `ops::BRICK_GRID` name it wherever it lives.
//!
//! - [`layout`]: cells and coordinates (`brick_grid`, `voronoi_grid`, `domain_warp`)
//! - [`cells`]: per-cell height, colour and roughness, and the mortar between
//! - [`patterns`]: noise and pattern generators
//! - [`blend`]: one channel into another
//! - [`math`]: scalar math, range mapping, colour ramps
//! - [`adjust`]: colour and value adjustment
//! - [`filters`]: neighbourhood filters, normals from height

use crate::SeededRng;

use super::descriptor::OpDescriptor;
use super::field::TextureField;

/// Linear interpolation helper.
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ─── Op Port Metadata ────────────────────────────────────────────────────

/// Describes which channels an op reads, writes, and modifies.
///
/// Used by the node editor to draw typed ports and validate connections.
#[derive(Debug, Clone)]
pub struct OpPortInfo {
    /// The op type identifier (e.g. `"brick_grid"`).
    pub op_type: &'static str,
    /// Human-readable label for the node (e.g. `"Brick Grid"`).
    pub label: &'static str,
    /// Channels this op reads (must exist from a prior op).
    pub reads: &'static [&'static str],
    /// Channels this op creates/overwrites.
    pub writes: &'static [&'static str],
    /// Channels this op reads AND writes (in-place modification).
    pub modifies: &'static [&'static str],
}

// ─── TextureOp trait ──────────────────────────────────────────────────────

/// A composable texture generation stage.
///
/// Each op reads channels from the field, performs some computation, and
/// writes results back. Ops are applied sequentially; later ops can build
/// on earlier ops' output.
pub trait TextureOp: Send + Sync {
    /// Apply this operation to the field, using `rng` for any randomness.
    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng);

    /// What the op is: its type, channels and parameters (#720).
    fn descriptor(&self) -> &'static OpDescriptor;

    /// Port metadata describing this op's channel I/O, from its descriptor.
    fn port_info(&self) -> OpPortInfo {
        self.descriptor().port_info()
    }
}

pub mod adjust;
pub mod blend;
pub mod cells;
pub mod filters;
pub mod layout;
pub mod math;
pub mod patterns;

pub use adjust::*;
pub use blend::*;
pub use cells::*;
pub use filters::*;
pub use layout::*;
pub use math::*;
pub use patterns::*;

#[cfg(test)]
mod tests;
