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
// Whether this build has a project's packages, and `flint build`'s
// set-aside binaries (#1335)
pub mod packages;
// `--skip-pass` / `--repeat-pass` (#1317)
pub mod pass_knob_args;
pub mod present_args;
// `--perf-report FILE` (#1093)
pub mod perf_report_args;
// What a launch's options mean, whichever host read them (#1142)
pub mod play_options;
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
pub use play_options::{PlayOptions, Query};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::expression_feed::ExpressionListener;
pub use player_app::expression_feed::{ExpressionFeed, ExpressionTimeline, TimedState};
pub use player_app::fonts::project_font_definitions;
pub use player_app::frame_rate::FrameRateSetting;
pub use player_app::headless_animation::HeadlessAnimation;
pub use player_app::headless_game::{
    apply_look, CaptureStart, HeadlessGame, LookRequest, ScriptLook, Skipped, PRESENTATION_ONLY,
    SCRIPT_SEED,
};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::headless_hud::HeadlessHud;
pub use player_app::input_checks::CheckOutcome;
pub use player_app::input_script::{Applied, InputScript, PointerEvent, ScriptEvent};
#[cfg(not(target_arch = "wasm32"))]
pub use player_app::inspect::InspectListener;
pub use player_app::playtest::{
    run as playtest, Ended as PlaytestEnded, Failure as PlaytestFailure, PlaytestOptions,
    PlaytestReport, SaveReport as PlaytestSave, DEFAULT_STEP as PLAYTEST_STEP,
};
pub use player_app::playtest_warnings::warning_layer as playtest_warning_layer;
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
pub use present_args::PresentArgs;
#[cfg(not(target_arch = "wasm32"))]
pub use provenance::git_state;
pub use provenance::Provenance;
pub use record_args::RecordArgs;
pub use trace_args::TraceArgs;
pub use window_args::WindowArgs;

/// Settle a scene file just loaded from `path`, as every host does before
/// anything reads it (#686, #1558): each package of the run checks and
/// fills in its own tables (living's `[living]` and `[wooze]`, a preset
/// named laid in, `Registry::settle_scene`). An error refuses the scene.
pub fn settle_scene(
    scene: &mut flint_scene::SceneFile,
    path: &std::path::Path,
) -> flint_core::Result<()> {
    packages::registry().settle(scene, path)
}

/// A script system with the player's whole API: flint-script's core and
/// the modules the run's packages registered (`living`, `wooze`, `debug`,
/// `conducted`; #698, #1552), declared and not yet enabled. A host enables
/// a module where the subsystem that owns it runs (`GameCore` does, for the
/// player and `flint render --scripts`); `flint script api` and `flint
/// script check` list and check against this one.
pub fn script_system() -> flint_script::ScriptSystem {
    let mut system = flint_script::ScriptSystem::new();
    for module in packages::registry().modules() {
        system.declare_module(module);
    }
    system
}
