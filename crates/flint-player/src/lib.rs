//! Flint Player — standalone game player library
//!
//! This crate provides the `PlayerApp` application handler
//! for running Flint scenes with physics and first-person controls.

mod player_app;
pub mod spline_gen;

pub use player_app::PlayerApp;
