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

pub mod bundle;
pub mod callbacks;
pub mod components;
pub mod diagnostic;
mod error;
pub mod events;
pub mod file_watch;
mod hash;
mod id;
pub mod ik;
pub mod project;
pub mod projection;
pub mod quality;
pub mod quat;
pub mod toml_util;
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
