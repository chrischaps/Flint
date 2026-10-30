//! Flint Player — standalone game player library
//!
//! This crate provides the `PlayerApp` application handler
//! for running Flint scenes with physics and first-person controls.

pub mod extension;
mod player_app;
pub mod spline_gen;
pub mod user_files;

pub use extension::{
    ExtensionAudio, ExtensionCtx, ExtensionGpu, KeyInput, MeshOptions, PlayerExtension,
    SoundOptions,
};
pub use flint_audio::Bus;
pub use flint_render::{
    river_points, GrassGround, GrassHeight, GroundFog, RiverPoint, WaterBodyLook, WaterBodyMesh,
    WaterVertex,
};
pub use flint_core::ray_mesh::{RayHit, RayMesh, RayScene};
pub use flint_script::{Projector, ScreenPoint, View};
pub use flint_terrain::{GrassConfig, GroundField, Heightmap};
pub use player_app::fonts::project_font_definitions;
pub use player_app::input_script::{Applied, InputScript, PointerEvent, ScriptEvent};
pub use player_app::scene_loading::{load_scene_grass_ground, post_process_config_from_def};
#[cfg(target_arch = "wasm32")]
pub use player_app::web_startup::WEB_CANVAS_ID;
pub use player_app::{CaptureConfig, PlayerApp};
