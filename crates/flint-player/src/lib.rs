//! Flint Player — standalone game player library
//!
//! This crate provides the `PlayerApp` application handler
//! for running Flint scenes with physics and first-person controls.

pub mod extension;
mod player_app;
pub mod spline_gen;

pub use extension::{ExtensionAudio, ExtensionCtx, ExtensionGpu, PlayerExtension, SoundOptions};
pub use flint_audio::Bus;
pub use flint_render::{
    river_points, GrassGround, GrassHeight, RiverPoint, WaterBodyLook, WaterBodyMesh, WaterVertex,
};
pub use flint_script::{Projector, ScreenPoint, View};
pub use flint_terrain::GrassConfig;
pub use player_app::fonts::project_font_definitions;
pub use player_app::input_script::{Applied, InputScript, PointerEvent};
pub use player_app::scene_loading::{load_scene_grass_ground, post_process_config_from_def};
pub use player_app::{CaptureConfig, PlayerApp};
