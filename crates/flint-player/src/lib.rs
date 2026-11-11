//! Flint Player — standalone game player library
//!
//! This crate provides the `PlayerApp` application handler
//! for running Flint scenes with physics and first-person controls.

pub mod extension;
mod player_app;
pub mod spline_gen;
pub mod user_files;
pub mod window_args;

pub use extension::{
    ExtensionAudio, ExtensionCtx, ExtensionGpu, KeyInput, MeshOptions, PlayerExtension,
    SoundOptions,
};
pub use flint_audio::Bus;
pub use flint_core::quality::{GpuInfo, QualitySettings, QualityValue};
pub use flint_core::ray_mesh::{RayHit, RayMesh, RayScene};
pub use flint_render::{
    river_points, GrassGround, GrassHeight, GroundFog, RiverPoint, WaterBodyLook, WaterBodyMesh,
    WaterVertex,
};
pub use flint_script::{Projector, ScreenPoint, View};
pub use flint_terrain::{GrassConfig, GroundField, Heightmap};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::expression_feed::ExpressionListener;
pub use player_app::expression_feed::{ExpressionFeed, ExpressionTimeline, TimedState};
pub use player_app::fonts::project_font_definitions;
pub use player_app::frame_rate::FrameRateSetting;
pub use player_app::headless_scripts::{
    apply_look, CameraView, HeadlessScripts, ScriptLook, SCRIPT_SEED,
};
pub use player_app::input_script::{Applied, InputScript, PointerEvent, ScriptEvent};
pub use player_app::living_props::import_of as living_import;
pub use player_app::living_props::{species_problems, LivingRun, WoozeClocks};
pub use player_app::procgen_assets::{GpuSink, ProcGenMode};
pub use player_app::render_scale::{RenderScaleSetting, WEB_DPR_CAP};
pub use player_app::scene_loading::{
    load_scene_grass_ground, load_user_styles, post_process_config_from_def,
};
pub use player_app::style_commands::{
    apply_post_style_command, apply_style_command, is_post_style_command, Easing, SceneStyles,
    StyleApplied, StyleTweens, TweenStep, POST_STYLE_CHANGED, STYLE_CHANGED, STYLE_TWEEN_DONE,
};
#[cfg(target_arch = "wasm32")]
pub use player_app::web_pacing::coarse_pointer as web_coarse_pointer;
#[cfg(target_arch = "wasm32")]
pub use player_app::web_page::begin as web_begin;
#[cfg(target_arch = "wasm32")]
pub use player_app::web_startup::WEB_CANVAS_ID;
pub use player_app::{CaptureConfig, PlayerApp};
pub use window_args::WindowArgs;
