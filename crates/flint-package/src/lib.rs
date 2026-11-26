//! The Package API (ADR 0137): what a package's crate builds against, so it
//! depends on this small kernel crate and never on `flint-player` or
//! `flint-cli`.
//!
//! - [`Subsystem`]: a package's running part, with a hook for each moment
//!   of the run (a scene coming and going, each [`Stage`] of the frame, a
//!   script's command, a debug key, a click, the frame's renderer inputs,
//!   the HUD, the exit report).
//! - The contexts each hook is lent: [`SceneCtx`], [`StageCtx`],
//!   [`FrameCtx`], [`EncodeCtx`], [`ReportCtx`]. Every field is a kernel
//!   type.
//! - [`Resources`]: the run's typed values, one per Rust type, through which
//!   one package lends state to another (procgen's resolver to living)
//!   without the kernel naming either.
//!
//! The player keeps the ordered list (`Subsystems`) and lends each context
//! from its own fields. Registration (`Package`, `Registry`) and the
//! fixed-step hook come with the launcher and the physics tickets (#1551,
//! #1552). This crate stays below 1.0 until Phase 5 (ADR 0136 decision 5).

mod context;
pub mod note;
mod resources;
mod stage;
mod subsystem;

pub use context::{
    Button, Capture, EncodeCtx, ExitReport, FrameCtx, Lasts, Pads, PlayerInput, ReportCtx,
    SceneCtx, StageCtx,
};
pub use note::Note;
pub use resources::Resources;
pub use stage::Stage;
pub use subsystem::Subsystem;

/// The engine's egui, so a package's `ui` builds against the same version
/// (ADR 0137 decision 6).
pub use egui;
