//! Flint Core - Foundational types for the Flint engine
//!
//! This crate provides the core types that all other Flint crates depend on:
//! - `EntityId` - Stable entity identifiers
//! - `ContentHash` - SHA-256 based content hashing
//! - `Transform`, `Vec3` - Spatial types
//! - Error types and Result alias

pub mod callbacks;
pub mod components;
mod error;
pub mod events;
mod hash;
mod id;
pub mod ik;
pub mod ocean;
pub mod quat;
pub mod spline;
pub mod toml_util;
mod types;

pub use error::{FlintError, Result};
pub use hash::ContentHash;
pub use id::EntityId;
pub use ik::{solve_two_bone, TwoBoneSolution};
pub use quat::{
    euler_deg_to_quat, mat4_scale, mat4_to_quat, mat4_to_rigid, quat_conjugate,
    quat_from_axis_angle, quat_from_two_vectors, quat_mul, quat_nlerp, quat_normalize,
    quat_rotate_vec3, rigid_inverse_apply,
};
pub use types::{mat4_mul, Color, Transform, Vec3};
