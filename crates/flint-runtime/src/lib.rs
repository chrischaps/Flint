//! Flint Runtime - Game loop infrastructure
//!
//! Provides the core game loop building blocks:
//! - `GameClock` — fixed-timestep accumulator for deterministic physics
//! - `InputState` — keyboard and mouse input tracking with action bindings
//! - `GameEvent` / `EventBus` — the frame's event queue, owned by the player
//! - `RuntimeSystem` — what the player's per-frame systems share (called by type, no registry)
//! - `pad` — the gamepad backend (gilrs, or a stand-in without `gamepad`)
//! - `rumble` — a general pad rumble, `PadRumble` (#748)

pub mod atomic_file;
mod clock;
mod event;
mod event_bus;
mod input;
pub mod pad;
pub mod persist;
pub mod rumble;
pub mod state;
mod system;
pub mod touch_controls;

pub use clock::GameClock;
pub use event::GameEvent;
pub use event_bus::EventBus;
pub use input::parse_key_code;
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
pub use touch_controls::{TouchControls, TouchControlsConfig, TouchControlsMode, TouchWidget};
