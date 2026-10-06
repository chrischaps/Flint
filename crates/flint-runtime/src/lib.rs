//! Flint Runtime - Game loop infrastructure
//!
//! Provides the core game loop building blocks:
//! - `GameClock` — fixed-timestep accumulator for deterministic physics
//! - `InputState` — keyboard and mouse input tracking with action bindings
//! - `GameEvent` / `EventBus` — typed event queue for inter-system communication
//! - `RuntimeSystem` — trait for systems ticked by the game loop

mod clock;
mod event;
mod event_bus;
mod input;
pub mod persist;
pub mod state;
mod system;

pub use clock::GameClock;
pub use event::GameEvent;
pub use event_bus::EventBus;
pub use input::{
    ActionConfig, ActionKind, AxisDirection, Binding, GamepadSelector, InputConfig, InputDevice,
    InputState, RebindMode, DEVICE_AXIS_DEADZONE,
};
pub use input::parse_key_code;
pub use persist::{PersistentStore, SaveDebounce};
pub use state::{GameState, GameStateMachine, StateConfig, SystemPolicy};
pub use system::RuntimeSystem;
