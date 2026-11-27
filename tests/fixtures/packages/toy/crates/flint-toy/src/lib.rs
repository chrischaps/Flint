//! The toy package (#1560, epic #1548): the gate of Phase 2a, which says a
//! package is a folder. It lives in `packages/toy/` only while CI's
//! `scripts/package-toy.py` has copied it there, and the stock `flint-player`
//! and `flint` pick it up from its manifest with no edit to either: the
//! launchers' lists come from `packages/*/package.toml` (ADR 0137 section 5)
//! and its Cargo lines from `scripts/package-sync.py`.
//!
//! It is shaped like the `vehicle` package that starts after the gate (ADR
//! 0136 decision 7): a component on an entity with a rigid body, and a force
//! applied inside each fixed physics step, before Rapier integrates it.
//!
//! - [`player`]: the `toy` package's player part, [`player::PACKAGE`]. A
//!   scene with any `toy_spring` gets a subsystem that pulls each such body
//!   towards its `anchor` once a step, and the `toy` script module
//!   (`toy_steps()`, `toy_springs()`).
//! - [`cli`]: `flint toy springs <scene>`, the springs a scene file declares.

/// The component's name, as its schema (`schemas/components/toy_spring.toml`)
/// declares it.
pub const TOY_SPRING: &str = "toy_spring";

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "player")]
pub mod player;
