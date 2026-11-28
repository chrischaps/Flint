//! The rig package (ADR 0139, #1621): what a clip drives when it drives a
//! pose of named parts, on the animation package's clock.
//!
//! - **Skeletons** from glTF skins ([`skeleton`]), their clips
//!   ([`skeletal_clip`], sampled by [`skeletal_sampler`]) and the joint
//!   tracks node clips share (until #1664)
//! - **Layers and crossfades** composed joint by joint ([`blend`],
//!   [`skeletal_sync`])
//! - **Node clips** on a model's named child nodes ([`node_clip`],
//!   [`node_sync`])
//! - **Two-bone IK** ([`ik_pass`], ADR 0070) and bone probes
//! - **The clip kinds** ([`kind`]): [`SkeletalKind`] and [`NodeKind`] on
//!   `flint_animation`'s seam, and [`Rig`], the rig's calls on an
//!   `AnimationSystem`
//!
//! Its `player` feature is the package's player part (`player::PACKAGE`):
//! both kinds on animation's `ClipKinds` point, a spawned model's skeleton,
//! and the still's IK. Its `cli` feature is the model previewer, `flint
//! edit <model>.glb` (`cli::extensions()`). GPU skinning stays in
//! flint-render until Phase 4, fed bone matrices as plain data.

pub mod blend;
#[cfg(test)]
mod clip_rules_tests;
pub mod ik_pass;
pub mod kind;
pub mod node_clip;
pub mod node_sync;
pub mod skeletal_clip;
pub mod skeletal_sampler;
pub mod skeletal_sync;
pub mod skeleton;

#[cfg(feature = "player")]
pub mod player;

#[cfg(feature = "cli")]
pub mod cli;

pub use ik_pass::IkPass;
pub use kind::{publish_bone_probes, NodeKind, Rig, SkeletalKind};
pub use skeletal_sync::{LayerContribution, WRITER_BASE, WRITER_REST};

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "rig";
