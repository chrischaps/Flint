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
//!   without the kernel naming either. A standard package's state is one
//!   too (ADR 0138 decision 1, #1616): physics' `PhysicsSystem`, the
//!   particles' `ParticleSystem`, and the sound's `AudioEngine`, lent for
//!   each hook. This crate names none of them.
//!
//! - [`Package`] and [`Registry`]: a launcher lists its packages, and each
//!   one registers its scene subsystems, policy names, script modules and
//!   debug keys once a run (#1552), so the kernel finds them by what was
//!   registered, never by a package's name.
//!
//! - [`assets::AssetMaker`]: a package that makes assets by name (procgen's
//!   specs) before the model loader looks for a file, asked at a scene's
//!   load, for spawned entities and each frame (#1557).
//!
//! - [`panels::DebugPanel`] and [`hud::DebugHud`]: a panel any package (or
//!   the kernel) adds to F3, and the debug HUD the debug-hud package
//!   brings, which makes the engine's panels, lays them out and drives F3
//!   and F4 (#1559).
//!
//! - [`cli::CliExtension`]: a package's commands on the `flint` command
//!   line, and the one way a command answers (`--json`, #710), so a
//!   package's CLI side never depends on flint-cli (#1553).
//!
//! The player keeps the ordered list (`Subsystems`) and lends each context
//! from its own fields. This crate stays below 1.0 until Phase 5 (ADR 0136
//! decision 5).

pub mod assets;
pub mod bind;
pub mod cli;
mod context;
pub mod hud;
mod launch;
pub mod note;
pub mod panels;
pub mod particles;
mod registry;
mod stage;
mod subsystem;
pub mod surfaces;

pub use context::{
    find, find_mut, Button, Capture, CaptureGame, CaptureGameMut, CapturePose, EncodeCtx,
    ExitReport, FixedCtx, FrameCtx, Lasts, Pads, PlayerInput, ReportCtx, SceneCtx, StageCtx,
};
/// The run's typed values: flint-core's since #1616, so flint-script can
/// lend them to a script call (ADR 0138 decision 1).
pub use flint_core::resources::Resources;
pub use hud::DebugHud;
pub use launch::PackageFlags;
pub use note::Note;
pub use panels::{DebugPanel, DebugPanels};
pub use registry::{
    left_out_line, BuildForScene, LeftOutLine, Package, Registry, RunResources, SceneRead,
    SettleScene,
};
pub use stage::{Stage, StepStage};
pub use subsystem::Subsystem;

/// The engine's egui, so a package's `ui` builds against the same version
/// (ADR 0137 decision 6).
pub use egui;
