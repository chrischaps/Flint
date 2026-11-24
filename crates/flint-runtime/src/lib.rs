//! Flint Runtime - Game loop infrastructure
//!
//! Provides the core game loop building blocks:
//! - `GameClock` — fixed-timestep accumulator for deterministic physics
//! - `InputState` — keyboard and mouse input tracking with action bindings
//! - `GameEvent` / `EventBus` — the frame's event queue, owned by the player
//! - `RuntimeSystem` — what the player's per-frame systems share (called by type, no registry)
//! - `pad` — the gamepad backend (gilrs, or a stand-in without `gamepad`)
//! - `rumble` — a general pad rumble, `PadRumble` (#748)
//! - `debug_keys` — the debug keys as a table, `[debug_keys]` in an input config (#717)
//! - `debug_key_card` — the F1 card that paints it, with `egui` (#1170)
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

pub mod atomic_file;
mod clock;
#[cfg(feature = "egui")]
pub mod debug_key_card;
pub mod debug_keys;
mod event;
mod event_bus;
mod input;
pub mod pad;
pub mod persist;
pub mod rumble;
pub mod state;
mod system;
mod text_entry;
pub mod touch_controls;

pub use clock::GameClock;
pub use debug_keys::{
    Chord, DebugKey, DebugKeyEntry, DebugKeyGroup, DebugKeyTable, DebugKeysConfig, Standing,
};
pub use event::GameEvent;
pub use event_bus::EventBus;
pub use input::{binding_label, parse_key_code};
pub use input::{
    ActionConfig, ActionKind, AxisDirection, Binding, GamepadSelector, InputConfig, InputDevice,
    InputState, RebindMode, DEVICE_AXIS_DEADZONE,
};
pub use persist::{
    check_slot_name, CorruptSave, Loaded, PersistentStore, SaveBackend, SaveDebounce, SaveText,
    DEFAULT_SLOT,
};
pub use rumble::{PadRumble, PadTarget, RumbleError, RumbleRequest, MAX_RUMBLE_SECONDS};
pub use state::{GameState, GameStateMachine, StateConfig, StateError, SystemPolicy};
pub use system::RuntimeSystem;
pub use text_entry::TextEdit;
pub use touch_controls::{TouchControls, TouchControlsConfig, TouchControlsMode, TouchWidget};
