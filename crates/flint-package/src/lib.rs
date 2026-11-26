//! The Package API (ADR 0137): what a package's crate builds against, so it
//! depends on this small kernel crate and never on `flint-player` or
//! `flint-cli`.
//!
//! - [`Subsystem`]: a package's running part, with a hook for each moment
//!   of the run (a scene coming and going, each [`Stage`] of the frame, a
//!   script's command, a debug key, a click, the frame's renderer inputs,
//!   the HUD, the exit report).
//! - The contexts each hook is lent: [`SceneCtx`], [`StageCtx`],
//!   [`FixedCtx`] (inside each physics step, #1551), [`FrameCtx`],
//!   [`EncodeCtx`], [`ReportCtx`]. Every field is a kernel type.
//! - [`Resources`]: the run's typed values, one per Rust type, through which
//!   one package lends state to another (procgen's resolver to living)
//!   without the kernel naming either.
//!
//! - [`Package`] and [`Registry`]: a launcher lists its packages, and each
//!   one registers its scene subsystems, policy names, script modules and
//!   debug keys once a run (#1552), so the kernel finds them by what was
//!   registered, never by a package's name.
//!
//! - [`cli::CliExtension`]: a package's commands on the `flint` command
//!   line, and the one way a command answers (`--json`, #710), so a
//!   package's CLI side never depends on flint-cli (#1553).
//!
//! The player keeps the ordered list (`Subsystems`) and lends each context
//! from its own fields. This crate stays below 1.0 until Phase 5 (ADR 0136
//! decision 5).

pub mod cli;
mod context;
pub mod note;
mod registry;
mod resources;
mod stage;
mod subsystem;

pub use context::{
    Button, Capture, EncodeCtx, ExitReport, FixedCtx, FrameCtx, Lasts, Pads, PlayerInput,
    ReportCtx, SceneCtx, StageCtx,
};
pub use note::Note;
pub use registry::{BuildForScene, LeftOutLine, Package, Registry};
pub use resources::Resources;
pub use stage::Stage;
pub use subsystem::Subsystem;

/// The engine's egui, so a package's `ui` builds against the same version
/// (ADR 0137 decision 6).
pub use egui;

/// The engine's physics, which [`FixedCtx::physics`] lends, with Rapier as
/// `physics::rapier` (ADR 0137 section 3a), so a package's forces and rays
/// build against the same version.
pub use flint_physics as physics;
