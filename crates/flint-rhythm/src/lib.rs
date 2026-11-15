//! The rhythm-game layer over `flint-music` ("linear composition, adaptive
//! playback"): a beatmap chart (continuous input curves, discrete pulses,
//! scene cues) judged against a suite as it plays, coherence, the
//! degradation ladder, reintegration seams, replay, haptics, and the
//! contract a rhythm suite keeps (the fixed six-bus layout,
//! `[reintegration]`, a judgment window on every section).
//!
//! Music without a game (a tempo map, stems on buses, scheduled bus events,
//! offline renders, latency calibration) is `flint-music`; this crate adds
//! the game's rules on top and never the other way round. A game that only
//! plays music doesn't link it.
//!
//! The contract was pinned by golden fixtures in the rhythm game's
//! repository (`fixtures/`, beside `engine/`); when this code and those
//! fixtures disagree, the fixtures are correct. Schema doc:
//! `docs/schema/manifest-schema-v0.md` in the game repo.

pub mod chart;
pub mod chart_eval;
pub mod chart_session;
pub mod coherence;
pub mod gradient;
pub mod haptics;
pub mod input_stream;
pub mod judgment;
pub mod ladder;
pub mod reintegration;
pub mod replay;
pub mod status;
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
pub use coherence::{Coherence, CoherenceConfig};
pub use gradient::{GradientConfig, GradientDriver, GradientOffsets};
pub use haptics::{HapticEvent, HapticsConfig, HapticsDriver, TickGrid};
pub use input_stream::{InputEvent, LeanSample, PulseEvent};
pub use judgment::{Judge, JudgmentConfig, JudgmentRecord};
pub use ladder::{Ladder, LadderConfig, LadderDriver, LadderParams};
pub use reintegration::{ReintegrationEvent, Reintegrator, SeqPhase, SeqTick};
pub use replay::{read_session, synthesize, SessionHeader, SessionWriter, SyntheticProfile};
pub use validate::{validate_chart, validate_manifest};

/// The rhythm layer's bus layout. Order matters for display; the set is the
/// contract a rhythm suite keeps ([`validate_manifest`]).
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
