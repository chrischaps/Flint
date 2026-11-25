//! Flint Core - Foundational types for the Flint engine
//!
//! This crate provides the core types that all other Flint crates depend on:
//! - `EntityId` - Stable entity identifiers
//! - `ContentHash` - SHA-256 based content hashing
//! - `Transform`, `Vec3` - Spatial types
//! - Error types and Result alias
//! - `diagnostic` - `Diagnostic`, the one shape for what is wrong with a scene (ADR 0115)
//! - `vfs` - The read-only file layer every runtime read goes through
//! - `bundle` - Game bundles: a game's files in one fetchable blob for the vfs
//! - `project` - A project's root and its `flint.toml` manifest, found through the vfs
//! - `package` - A package's `package.toml`: what one feature owns (ADR 0136)
//! - `step_clock` - The stepped hold-and-glide clock everything drawn on ticks reads (#747)
//! - `frame_notes` - What the engine did on a frame (loads, builds, reloads), for the hitch detector (#1077)
//! - `profile` - `scope!`, one profiler scope reaching Tracy and Flint's own recorder (ADR 0128)
//! - `env` - Every `FLINT_*` environment switch, in one table with typed getters (#715)
//! - `style_field` - The screen field a style's blend can follow (#859)
//! - `collision_layers` - Collision layer names, and what a collider naming none is told (#1220)
//! - `field` - Named numbers as one function of place and time: zones, falloff, ramps (ADR 0123)
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

pub mod bundle;
pub mod callbacks;
pub mod collision_layers;
pub mod components;
pub mod diagnostic;
pub mod env;
mod error;
pub mod events;
pub mod field;
pub mod field_frame;
pub mod file_watch;
pub mod frame_notes;
mod hash;
mod id;
pub mod ik;
pub mod load_report;
pub mod package;
pub mod profile;
pub mod project;
pub mod projection;
pub mod quality;
pub mod quat;
pub mod step_clock;
pub mod style_field;
pub mod toml_util;
pub mod tween;
mod types;
pub mod vfs;
pub mod world_line;
pub mod world_text;

pub use diagnostic::{Diagnostic, Severity};
pub use error::{FlintError, Result};
pub use hash::ContentHash;
pub use id::EntityId;
pub use ik::{solve_two_bone, TwoBoneSolution};
pub use quat::{
    euler_deg_to_quat, mat4_scale, mat4_skew, mat4_to_quat, mat4_to_rigid, quat_conjugate,
    quat_from_axis_angle, quat_from_two_vectors, quat_mul, quat_nlerp, quat_normalize,
    quat_rotate_vec3, rigid_inverse_apply,
};
pub use types::{mat4_mul, Color, Shear, Transform, Vec3};
