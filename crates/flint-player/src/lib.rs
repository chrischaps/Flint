//! Flint Player — standalone game player library
//!
//! This crate provides the `PlayerApp` application handler
//! for running Flint scenes with physics and first-person controls.

pub mod capture_args;
pub mod extension;
pub mod hitch_args;
// `--load-report` (#1092)
pub mod load_args;
// Everything the player says, through tracing, as text or JSON lines (#716)
pub mod logging;
#[cfg(feature = "music")]
mod music;
// `--skip-pass` / `--repeat-pass` (#1317)
pub mod pass_knob_args;
// `--perf-report FILE` (#1093)
pub mod perf_report_args;
mod player_app;
// The desktop launch, shared by `flint play` and `flint-player` (#705)
pub mod player_args;
// The engine a binary was built from, the project a run is in (#1290)
pub mod provenance;
// `--record DIR --record-fps N` (#583)
pub mod record_args;
// The scope recorder's flags (#1097)
pub mod trace_args;
pub mod user_files;
pub mod window_args;

pub use capture_args::FrameCaptureArgs;
pub use extension::{
    ExtensionAudio, ExtensionCtx, ExtensionGpu, KeyInput, MeshOptions, PlayerExtension,
    SoundOptions, Stage,
};
pub use flint_audio::{Bus, Buses};
pub use flint_core::quality::{GpuInfo, QualitySettings, QualityValue};
pub use flint_geometry::ray_mesh::{RayHit, RayMesh, RayScene};
pub use flint_render::{
    river_points, GrassGround, GrassHeight, GroundFog, RiverPoint, WaterBodyLook, WaterBodyMesh,
    WaterVertex,
};
pub use flint_script::{Projector, ScreenPoint, View};
pub use flint_terrain::{GrassConfig, GroundField, Heightmap};
pub use hitch_args::{HitchArgs, HitchClass, HitchGate};
pub use load_args::LoadReportArgs;
pub use pass_knob_args::PassKnobArgs;
pub use perf_report_args::PerfReportArgs;
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::expression_feed::ExpressionListener;
pub use player_app::expression_feed::{ExpressionFeed, ExpressionTimeline, TimedState};
pub use player_app::fonts::project_font_definitions;
pub use player_app::frame_rate::FrameRateSetting;
pub use player_app::headless_animation::HeadlessAnimation;
pub use player_app::headless_game::{
    apply_look, HeadlessGame, LookRequest, ScriptLook, Skipped, PRESENTATION_ONLY, SCRIPT_SEED,
};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::headless_hud::HeadlessHud;
pub use player_app::input_checks::CheckOutcome;
pub use player_app::input_script::{Applied, InputScript, PointerEvent, ScriptEvent};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::inspect::InspectListener;
#[cfg(feature = "living")]
pub use player_app::living_props::import_of as living_import;
#[cfg(feature = "living")]
pub use player_app::living_props::{
    apply_preset as apply_living_preset, boil_at, species_problems, wanted_by as living_wanted,
    wooze_of_scene,
};
pub use player_app::playtest::{
    run as playtest, Ended as PlaytestEnded, Failure as PlaytestFailure, PlaytestOptions,
    PlaytestReport, DEFAULT_STEP as PLAYTEST_STEP,
};
pub use player_app::playtest_warnings::warning_layer as playtest_warning_layer;
#[cfg(feature = "procgen")]
pub use player_app::procgen_assets::GpuSink;
pub use player_app::render_scale::{RenderScaleSetting, WEB_DPR_CAP};
pub use player_app::scene_loading::{
    load_scene_grass_ground, load_user_styles, post_process_config_from_def,
};
pub use player_app::script_commands::apply_camera_overrides;
pub use player_app::style_commands::{
    reset_post_style, set_post_style, set_post_style_param, Easing, SceneStyles, StyleApplied,
    StyleTweens, TweenStep, LOOK_FADED, POST_STYLE_CHANGED, POST_STYLE_TWEEN_DONE, STYLE_CHANGED,
    STYLE_TWEEN_DONE,
};
pub use player_app::trace::TraceSetup;
#[cfg(target_arch = "wasm32")]
pub use player_app::web_pacing::coarse_pointer as web_coarse_pointer;
#[cfg(target_arch = "wasm32")]
pub use player_app::web_page::begin as web_begin;
#[cfg(target_arch = "wasm32")]
pub use player_app::web_startup::WEB_CANVAS_ID;
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::world_dump::WorldDump;
pub use player_app::ProcGenMode;
pub use player_app::StatsView;
pub use player_app::{CaptureConfig, GameCore, LaunchOptions, PlayerApp, SceneSettings};
pub use player_args::{launch, launch_bundle, PlayerArgs, PlayerCli, Prepared};
#[cfg(not(target_arch = "wasm32"))]
pub use provenance::git_state;
pub use provenance::Provenance;
pub use record_args::RecordArgs;
pub use trace_args::TraceArgs;
pub use window_args::WindowArgs;

/// A script system with the player's whole API: flint-script's core and
/// the modules its optional features own (`living`, `wooze`, `debug`,
/// `conducted`; #698), declared and not yet enabled. A host enables a
/// module where the subsystem that owns it runs (`GameCore` does, for the
/// player and `flint render --scripts`); `flint script api` and `flint
/// script check` list and check against this one.
pub fn script_system() -> flint_script::ScriptSystem {
    let mut system = flint_script::ScriptSystem::new();
    for module in player_app::subsystem::subsystem_script_modules() {
        system.declare_module(module);
    }
    system
}
