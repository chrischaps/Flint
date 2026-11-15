//! Geometry above the core (#754): what was `flint_core::ocean`,
//! `flint_core::spline` and `flint_core::ray_mesh`, and the component names
//! of the features built on them.
//!
//! - [`ocean`] - the Gerstner wave spectrum, the one source of the ocean's
//!   surface for the renderer and for scripts' buoyancy
//! - [`spline`] - Catmull-Rom path sampling with twist
//! - [`ray_mesh`] - rays against the triangles a game draws (ADR 0105)
//! - [`components`] - the `ocean`, `water_body` and `spline` component names

pub mod components;
pub mod ocean;
pub mod ray_mesh;
pub mod spline;
