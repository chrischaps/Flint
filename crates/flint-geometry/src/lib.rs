//! Geometry above the core (#754): what was `flint_core::ocean`,
//! `flint_core::spline` and `flint_core::ray_mesh`, and the component names
//! of the features built on them.
//!
//! - [`ocean`] - the Gerstner wave spectrum, the one source of the ocean's
//!   surface for the renderer and for scripts' buoyancy
//! - [`spline`] - Catmull-Rom path sampling with twist
//! - [`point_path`] - a Catmull-Rom path through a list of points, read by
//!   distance, with a bank into its turns: a camera's flight (#1158)
//! - [`spline_file`] - a `.spline.toml` read with its mistakes placed by line
//!   and column (#750)
//! - [`sweep`] - a profile polygon swept along a sampled spline: a road, a
//!   wall, a rail, a pipe (#750)
//! - [`ray_mesh`] - rays against the triangles a game draws (ADR 0105)
//! - [`components`] - the `ocean`, `water_body` and `spline` component names

pub mod components;
pub mod ocean;
pub mod point_path;
pub mod ray_mesh;
pub mod spline;
pub mod spline_file;
pub mod sweep;
