//! What the player's per-frame systems share

use flint_core::Result;
use flint_ecs::FlintWorld;

/// A system the player updates once a frame: scripts, audio, animation and
/// particles. Set up from the world when a scene is entered
/// ([`initialize`](Self::initialize)), then [`update`](Self::update)d once a
/// frame while the game state lets it run.
///
/// Nothing holds a list of these or calls them through `dyn RuntimeSystem`:
/// `PlayerApp` (and the CLI's headless commands) calls each by its concrete
/// type, in the order `PlayerApp::tick` sets out, and asks the top game
/// state whether it runs by the name in [`crate::state`] (`SCRIPTS`,
/// `AUDIO`, ...). Physics isn't one: it steps at the fixed rate
/// (`PhysicsSystem::fixed_update`, which pushes collisions into the player's
/// [`EventBus`](crate::EventBus)). The player's optional parts are its
/// `Subsystem`s (ADR 0117), not this trait.
pub trait RuntimeSystem {
    /// Read the scene's world when it is entered (startup and each
    /// transition, after the world is loaded).
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()>;

    /// Once a frame, with the frame's game time step.
    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()>;
}
