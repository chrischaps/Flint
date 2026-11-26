//! The `rhythm` package's parts on the Package API (ADR 0137, #1556): what
//! flint-player and flint-cli knew of the rhythm game until it moved into
//! `packages/rhythm/`, so neither names it now.
//!
//! - [`player`] (feature `player`): [`player::PACKAGE`], which fills music's
//!   chart judge with the rhythm session's subsystem (`ChartSession`, the
//!   1 kHz pad capture, the ladder's post values, the guide panels). A
//!   launcher lists it from the manifest's `[rust] player`.
//! - [`cli`] (feature `cli`): `flint music calibrate` and `flint music chart
//!   play|replay`, an extension under music's `music`, listed from the
//!   manifest's `[rust] cli`.
//!
//! The judging itself is flint-rhythm's and the capture flint-input-capture's,
//! the package's other two crates. This one is its own crate because the
//! session needs both, and flint-input-capture depends on flint-rhythm.

#[cfg(feature = "player")]
pub mod player;

#[cfg(feature = "cli")]
pub mod cli;
