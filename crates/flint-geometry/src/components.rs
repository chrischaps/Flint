//! The component names of the features built on this crate's geometry
//! (#754; they were in `flint_core::components`).

// --- Ocean ---
pub const OCEAN: &str = "ocean";
pub const OCEAN_CONTACT: &str = "ocean_contact";
/// Water on a mesh of its own: a lake at its level or a river down its path (ADR 0094).
pub const WATER_BODY: &str = "water_body";

// --- Splines ---
pub const SPLINE: &str = "spline";
pub const SPLINE_DATA: &str = "spline_data";
pub const SPLINE_MESH: &str = "spline_mesh";
pub const SPLINE_CHUNK: &str = "spline_chunk";
