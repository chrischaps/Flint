//! Musical suite manifests, beatmap charts, tempo maps, and validation.
//!
//! This crate is the data-contract layer for rhythm-driven Flint games
//! ("linear composition, adaptive playback"): a per-suite TOML manifest
//! (tempo map, sections, re-entry points, fixed six-bus stem inventory) and a
//! per-suite beatmap chart (continuous input curves, discrete pulses, scene
//! cues), plus the validator that cross-checks both against each other and
//! against the stem files on disk.
//!
//! The schema is pinned by golden fixtures in the game repository; when this
//! code and those fixtures disagree, the fixtures are correct. Schema doc:
//! `docs/schema/manifest-schema-v0.md` in the game repo.

pub mod analysis;
pub mod chart;
pub mod chart_eval;
pub mod chart_session;
pub mod clock_bridge;
pub mod coherence;
pub mod conductor;
pub(crate) mod config_toml;
pub mod event_script;
pub mod gradient;
pub mod haptics;
pub mod input_stream;
pub mod judgment;
pub mod ladder;
pub mod manifest;
pub mod mixer;
pub mod offline;
pub mod probe;
pub mod reintegration;
pub mod replay;
pub mod scheduler;
pub mod session;
pub mod status;
pub mod tempo;
pub mod timeline;
pub mod validate;

pub use chart::Chart;
pub use chart_eval::{ChannelValue, ChartEval, PulseWindow};
pub use chart_session::{
    judgment_offset_samples, latest_calibration_ms, latest_latency_ms, lean_mode_name,
    parse_lean_mode, resolve_coherence_config, ChartCore, ChartSession, ChartSessionConfig,
    CoherenceSource, ConductedFrame, ConductedPulse, FinishSummary, PulseKind, Tick, TickOutcome,
    VisualFrame,
};
pub use clock_bridge::{BridgeStats, ClockBridge};
pub use coherence::{Coherence, CoherenceConfig};
pub use conductor::{Conductor, Grid, MusicalPosition};
pub use event_script::EventScript;
pub use gradient::{GradientConfig, GradientDriver, GradientOffsets};
pub use haptics::{HapticEvent, HapticsConfig, HapticsDriver, TickGrid};
pub use input_stream::{InputEvent, LeanSample, PulseEvent};
pub use judgment::{Judge, JudgmentConfig, JudgmentRecord};
pub use ladder::{Ladder, LadderConfig, LadderDriver, LadderParams};
pub use manifest::SuiteManifest;
pub use mixer::{BusMixer, BusState, StemBus};
pub use offline::{render_offline, render_offline_with, write_wav, OfflineRenderConfig};
pub use reintegration::{ReintegrationEvent, Reintegrator, SeqPhase, SeqTick};
pub use replay::{read_session, synthesize, SessionHeader, SessionWriter, SyntheticProfile};
pub use scheduler::{BusAction, EventTime, FiredEvent, ScheduledEvent, Scheduler};
pub use session::{RealtimePlayer, StemResolver, SuiteSession};
pub use tempo::TempoMap;
pub use validate::{validate_chart, validate_manifest, validate_manifest_assets, Issue};

/// The one schema version this build understands.
pub const SCHEMA_VERSION: i64 = 0;

/// The fixed bus layout. Order matters for display; the set is the contract.
pub const BUSES: [&str; 6] = [
    "foundation",
    "harmony",
    "world_voice",
    "home_theme",
    "child_motif",
    "texture",
];

/// Buses that must stay isolated (never share a file with any other bus).
pub const MOTIF_BUSES: [&str; 2] = ["home_theme", "child_motif"];

/// Continuous chart channels: (name, value arity). Vec2 channels range [-1,1],
/// scalar channels range [0,1].
pub const CHANNELS: [(&str, usize); 4] = [
    ("lean", 2),
    ("sway", 2),
    ("pressure_l", 1),
    ("pressure_r", 1),
];

/// Discrete pulse kinds.
pub const PULSE_KINDS: [&str; 3] = ["pulse", "press", "flick"];

/// Curve interpolation modes.
pub const INTERPS: [&str; 3] = ["linear", "hold", "smooth"];
