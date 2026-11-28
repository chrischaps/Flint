//! The kernel's point for a scene's particles (#1622, ADR 0138's table):
//! the particles package fills it, and the hosts that draw particles ask it,
//! so none of them names the package's crate.
//!
//! The renderer's particle pipeline and its plain draw stay the kernel's
//! until Phase 4 (`flint_render::particles`); the simulation is the
//! package's, a [`ParticleSource`]. The player's frame packs the run's
//! system and hands its draws to `prepare` through [`Particles::of`] /
//! [`Particles::of_mut`] (the system is a run resource the package puts
//! in); `flint render`'s captures step the capture's own through the same
//! two, and the scene viewer and a still make a system of their own with
//! [`Particles::new`]. A host with no point registered draws none.
//!
//! The point is registered twice by the one package: on the player's
//! [`Registry`](crate::Registry) (`insert_point`) and on the command line's
//! [`Points`](crate::cli::Points), where the viewer and `flint render` find
//! it.

use crate::Resources;
use flint_render::ParticleSource;

/// What the particles package lends a host that draws particles.
pub struct Particles {
    /// A system of the caller's own, empty, on the package's seed.
    pub new: fn() -> Box<dyn ParticleSource>,
    /// The run's system, among the resources.
    pub of: fn(&Resources) -> Option<&dyn ParticleSource>,
    /// The run's system, to step.
    pub of_mut: fn(&mut Resources) -> Option<&mut dyn ParticleSource>,
    /// Whether a capture draws particles (`flint render --time` or
    /// `--particle-time`), so its scripts' particle calls reach the system:
    /// a still that draws none drops them, saying so once.
    pub set_drawn: fn(&mut Resources, bool),
    /// An inline `particle_emitter` table held to the emitter's own reader:
    /// each problem's key, its message, and whether it is a key the reader
    /// doesn't know (`flint check`'s `bad-emitter`).
    pub emitter_problems: fn(&toml::value::Table) -> Vec<EmitterIssue>,
}

/// One problem with an inline emitter, as [`Particles::emitter_problems`]
/// says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitterIssue {
    /// The key it is about (`bursts[1]`, `lifetime`); `None` when the
    /// reader couldn't say.
    pub key: Option<String>,
    /// What is wrong, in the reader's words.
    pub message: String,
    /// A key the reader doesn't know: the emitter plays without it.
    pub unknown: bool,
    /// A suggestion, when there is one.
    pub hint: Option<String>,
}
