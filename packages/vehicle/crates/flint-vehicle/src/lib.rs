//! The vehicle package (ADR 0140, #1628, epic #1626): a vehicle is a
//! dynamic body on Rapier's raycast wheels, with arcade assists as tuning,
//! driven once each fixed physics step. The first genre package, and the
//! first written **against** the public Package API (ADR 0137) rather than
//! moved onto it: it builds on `flint-package` and the kernel alone.
//!
//! - A `vehicle` on an entity with a dynamic `rigidbody` and a `collider`,
//!   its wheels the children with `vehicle_wheel`.
//! - `vehicle_input`, which a player's bindings (`vehicle.driver =
//!   "player"`) and an AI's script both write: driving, modifiers, a push.
//! - `vehicle_state`, written for scripts at the frame's `PreScripts`.
//! - [`player`]: the package's player part, [`player::PACKAGE`]: the
//!   subsystem (its hand in each fixed step), the `vehicle` script module
//!   (`vehicle_place`, `vehicle_wheels`, `vehicle_ids`) and the Vehicles
//!   panel where the player has the debug HUD.
//!
//! The handling model itself, Rapier's raycast wheels and the assists, is
//! [`player::model`]; the measurements that chose it are in ADR 0140.

/// The vehicle's tuning: `schemas/components/vehicle.toml`.
pub const VEHICLE: &str = "vehicle";
/// A wheel, on a child entity of the vehicle: `vehicle_wheel.toml`.
pub const VEHICLE_WHEEL: &str = "vehicle_wheel";
/// What drives it each step: `vehicle_input.toml`.
pub const VEHICLE_INPUT: &str = "vehicle_input";
/// What it is doing, for scripts: `vehicle_state.toml`.
pub const VEHICLE_STATE: &str = "vehicle_state";

/// The package's name, as its manifest (`packages/vehicle/package.toml`)
/// says it.
pub const PACKAGE_NAME: &str = "vehicle";

#[cfg(feature = "player")]
pub mod player;
