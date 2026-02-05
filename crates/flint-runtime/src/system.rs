//! Runtime system trait

use flint_core::Result;
use flint_ecs::FlintWorld;

/// A system that can be ticked by the game loop
///
/// Systems are updated in registration order. Fixed update runs at a constant
/// rate (physics), while update runs once per frame (rendering, input).
pub trait RuntimeSystem {
    /// Called once when the system is first registered
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()>;

    /// Called at a fixed rate (e.g. 60Hz) for deterministic simulation
    fn fixed_update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()>;

    /// Called once per frame for variable-rate logic
    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()>;

    /// Called when the system is being shut down
    fn shutdown(&mut self) -> Result<()>;

    /// Human-readable name for this system
    fn name(&self) -> &str;
}
