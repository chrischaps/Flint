//! The flipbook package (ADR 0139 section 6, #1620): sprite-sheet clips
//! (`.sprite.toml`) played as a clip kind ([`kind::FlipbookKind`]) on the
//! animation package's clock, so a sprite follows the `animator` rules every
//! clip follows and gains sequences, cues, `autoplay`, reverse play and
//! resume.
//!
//! Its `player` feature is the package's player part (`player::PACKAGE`):
//! the kind on animation's `ClipKinds` point and the `flipbook` script
//! module (`play_sprite`, `stop_sprite`, `set_sprite_speed`,
//! `is_sprite_playing`).

#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod clip;
pub mod kind;

#[cfg(feature = "player")]
pub mod player;

pub use clip::{load_sprite_clips_from_file, LoopMode, SpriteAnimClip, SpriteAnimFrame};
pub use kind::FlipbookKind;

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "flipbook";
