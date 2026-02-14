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
mod system;

pub use clock::GameClock;
pub use event::GameEvent;
pub use event_bus::EventBus;
pub use input::{
    ActionConfig, ActionKind, AxisDirection, Binding, GamepadSelector, InputConfig, InputState,
    RebindMode,
};
pub use system::RuntimeSystem;
