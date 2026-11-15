//! The `conducted` script module (ADR 0020, ADR 0116, #698): a rhythm
//! session's per-frame state for scene bindings: lean, target, coherence,
//! the musical position, the pulses judged and the ladder and
//! reintegration visual ramps. The music subsystem declares it
//! (`player_app/subsystems/music.rs`) and enables it in a scene whose
//! `music_session` has a chart; anywhere else a call is a "not enabled"
//! error. Until a session runs (its count-in, waiting for a device, a
//! capture, a player built without `rhythm`), every getter reads the
//! neutral state, a clean, settled world, so a charted scene's script
//! never branches on there being one.
//!
//! The snapshot is the module's state ([`flint_script::ScriptSystem::set_module_state`]):
//! the rhythm subsystem sets it at `Stage::PreScripts` each frame.

// Without `rhythm` only the neutral snapshot is ever made
#![cfg_attr(not(feature = "rhythm"), allow(dead_code))]

use flint_script::api::builder::kind::{Array as ArrayKind, Bool, Int, Map as MapKind, Num, Str};
use flint_script::api::{Call, Module};
use flint_script::ScriptModule;
use rhai::{Array, Dynamic, Map};

/// One judged pulse event from the music session, for scripts. A plain POD
/// mirror of flint-rhythm's, so this module needs no rhythm (ADR 0020).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConductedPulse {
    /// Seconds before "now" the event was stamped (suite-time arithmetic).
    pub(crate) age: f64,
    /// Timing error in ms (hits only; 0.0 for miss/spurious).
    pub(crate) err_ms: f64,
    /// "hit" | "miss" | "spurious".
    pub(crate) kind: String,
}

/// One typed value from a chart cue's free-form `params` table (ADR 0033).
/// A plain POD, so scripts see no toml shapes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CueParam {
    Number(f64),
    Text(String),
    Flag(bool),
}

/// One chart cue fired this frame (ADR 0033), for scene bindings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConductedCue {
    pub(crate) name: String,
    /// Seconds since the cue's authored beat (suite-time arithmetic).
    pub(crate) age: f64,
    /// The chart's `params` table, flattened to primitives (nested tables
    /// and arrays are dropped by the host — author flat params).
    pub(crate) params: Vec<(String, CueParam)>,
}

/// Per-frame conducted-parameters snapshot (Phase 4 decision 4 / ADR 0020).
/// Filled by the rhythm subsystem from the session every frame;
/// neutral defaults when no session so scripts never branch on existence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConductedSnapshot {
    /// Player lean and the chart's lean target, both in [-1,1]².
    pub(crate) lean: [f64; 2],
    pub(crate) target: [f64; 2],
    /// Player sway (right stick; zeros under the prototype input map).
    pub(crate) sway: [f64; 2],
    /// Trigger depths in [0,1] (zeros under the prototype input map).
    pub(crate) pressure_l: f64,
    pub(crate) pressure_r: f64,
    /// Cues fired this frame (empty most frames — ADR 0033).
    pub(crate) cues: Vec<ConductedCue>,
    /// The next authored lean key's value (ADR 0023); `target` when nothing
    /// is upcoming.
    pub(crate) next_target: [f64; 2],
    /// Suite beats until that key's anchor; 1e6 = nothing upcoming.
    pub(crate) next_target_beats: f64,
    /// Suite beats until the next unconsumed judgment window's anchor beat
    /// (any kind); 1e6 = nothing upcoming. Lets the world telegraph that
    /// input is expected (P5 immediate-feedback pass).
    pub(crate) next_pulse_beats: f64,
    /// True while that window is open right now (a press would land).
    pub(crate) pulse_window_open: bool,
    pub(crate) coherence: f64,
    /// 0..1 within the current beat / bar (0 at the boundary).
    pub(crate) beat_phase: f64,
    pub(crate) bar_phase: f64,
    pub(crate) bar: i64,
    /// Suite beats from zero, accumulated across tempo changes.
    pub(crate) beat: f64,
    /// Current section name; "" when none.
    pub(crate) section: String,
    /// Pulses judged this frame (empty most frames).
    pub(crate) pulses: Vec<ConductedPulse>,
    /// Ladder visual params (0 = clean).
    pub(crate) desaturate: f64,
    pub(crate) blur: f64,
    pub(crate) chromatic: f64,
    /// Reassembly progress: 1 = normal play, 0→1 while re-gathering.
    pub(crate) reassembly: f64,
    /// Rewind interlude progress: 0 = not rewinding.
    pub(crate) rewind: f64,
    pub(crate) no_input: bool,
    pub(crate) preroll: bool,
}

impl Default for ConductedSnapshot {
    /// The neutral no-session state: a clean, settled world (coherence and
    /// reassembly at 1.0) so bindings mapping `1 - coherence` show nothing.
    fn default() -> Self {
        Self {
            lean: [0.0; 2],
            target: [0.0; 2],
            sway: [0.0; 2],
            pressure_l: 0.0,
            pressure_r: 0.0,
            cues: Vec::new(),
            next_target: [0.0; 2],
            next_target_beats: 1e6,
            next_pulse_beats: 1e6,
            pulse_window_open: false,
            coherence: 1.0,
            beat_phase: 0.0,
            bar_phase: 0.0,
            bar: 0,
            beat: 0.0,
            section: String::new(),
            pulses: Vec::new(),
            desaturate: 0.0,
            blur: 0.0,
            chromatic: 0.0,
            reassembly: 1.0,
            rewind: 0.0,
            no_input: false,
            preroll: false,
        }
    }
}

/// The session's snapshot this frame, or the neutral one.
fn snapshot<'a>(call: &'a mut Call<'_>) -> &'a ConductedSnapshot {
    call.ctx().module_state.get_mut::<ConductedSnapshot>()
}

fn xy(v: [f64; 2]) -> Map {
    let mut map = Map::new();
    map.insert("x".into(), Dynamic::from(v[0]));
    map.insert("y".into(), Dynamic::from(v[1]));
    map
}

/// A number the snapshot holds, read by a getter of no arguments.
fn scalar(
    m: &mut Module<'_>,
    name: &'static str,
    doc: &'static str,
    note: &'static str,
    field: fn(&ConductedSnapshot) -> f64,
) {
    m.func(name, doc)
        .returns(Num, note)
        .run(move |call, ()| field(snapshot(call)));
}

/// `#{x, y}` the snapshot holds.
fn pair(
    m: &mut Module<'_>,
    name: &'static str,
    doc: &'static str,
    note: &'static str,
    field: fn(&ConductedSnapshot) -> [f64; 2],
) {
    m.func(name, doc)
        .returns(MapKind, note)
        .run(move |call, ()| xy(field(snapshot(call))));
}

fn declare_conducted(m: &mut Module<'_>) {
    pair(
        m,
        "conducted_lean",
        "The player's lean.",
        "#{x, y}, each -1..1",
        |c| c.lean,
    );
    pair(
        m,
        "conducted_target",
        "The chart's lean target now.",
        "#{x, y}, each -1..1",
        |c| c.target,
    );
    m.func(
        "conducted_next_target",
        "The next authored lean key and the suite beats until its anchor (ADR 0023).",
    )
    .returns(
        MapKind,
        "#{x, y, beats}; beats 1e6 and x, y the current target when nothing is upcoming",
    )
    .run(|call, ()| {
        let c = snapshot(call);
        let mut map = xy(c.next_target);
        map.insert("beats".into(), Dynamic::from(c.next_target_beats));
        map
    });
    m.func(
        "conducted_next_pulse",
        "Suite beats until the next unconsumed judgment window, and whether it is open now.",
    )
    .returns(
        MapKind,
        "#{beats, open}; beats 1e6 when nothing is upcoming or no session is active",
    )
    .run(|call, ()| {
        let c = snapshot(call);
        let mut map = Map::new();
        map.insert("beats".into(), Dynamic::from(c.next_pulse_beats));
        map.insert("open".into(), Dynamic::from(c.pulse_window_open));
        map
    });

    scalar(
        m,
        "conducted_coherence",
        "How well the player follows the chart.",
        "0..1; 1 with no session",
        |c| c.coherence,
    );
    scalar(
        m,
        "conducted_beat_phase",
        "How far through the current beat the music is.",
        "0..1, 0 at the beat",
        |c| c.beat_phase,
    );
    scalar(
        m,
        "conducted_bar_phase",
        "How far through the current bar the music is.",
        "0..1, 0 at the bar line",
        |c| c.bar_phase,
    );
    scalar(
        m,
        "conducted_beat",
        "Suite beats from zero, accumulated across tempo changes.",
        "beats",
        |c| c.beat,
    );
    scalar(
        m,
        "conducted_desaturate",
        "The ladder's desaturation ramp.",
        "0..1, 0 clean",
        |c| c.desaturate,
    );
    scalar(
        m,
        "conducted_blur",
        "The ladder's blur ramp.",
        "0..1, 0 clean",
        |c| c.blur,
    );
    scalar(
        m,
        "conducted_chromatic",
        "The ladder's chromatic-aberration ramp.",
        "0..1, 0 clean",
        |c| c.chromatic,
    );
    scalar(
        m,
        "conducted_reassembly",
        "Reassembly progress after a fall.",
        "1 in normal play, 0 to 1 while re-gathering",
        |c| c.reassembly,
    );
    scalar(
        m,
        "conducted_rewind",
        "The rewind interlude's progress.",
        "0..1, 0 when not rewinding",
        |c| c.rewind,
    );

    m.func("conducted_bar", "The current bar, from zero.")
        .returns(Int, "the bar index")
        .run(|call, ()| snapshot(call).bar);

    m.func("conducted_section", "The current section of the chart.")
        .returns(Str, "its name; \"\" when none")
        .run(|call, ()| snapshot(call).section.clone());

    m.func(
        "conducted_pulses",
        "The pulses judged this frame; empty most frames.",
    )
    .returns(ArrayKind, "[#{age, err_ms, kind}]")
    .run(|call, ()| -> Array {
        snapshot(call)
            .pulses
            .iter()
            .map(|p| {
                let mut map = Map::new();
                map.insert("age".into(), Dynamic::from(p.age));
                map.insert("err_ms".into(), Dynamic::from(p.err_ms));
                map.insert("kind".into(), Dynamic::from(p.kind.clone()));
                Dynamic::from_map(map)
            })
            .collect()
    });

    pair(
        m,
        "conducted_sway",
        "The player's sway (the right stick).",
        "#{x, y}; zeros under the prototype input map",
        |c| c.sway,
    );
    scalar(
        m,
        "conducted_pressure_l",
        "The left trigger's depth.",
        "0..1; 0 under the prototype input map",
        |c| c.pressure_l,
    );
    scalar(
        m,
        "conducted_pressure_r",
        "The right trigger's depth.",
        "0..1; 0 under the prototype input map",
        |c| c.pressure_r,
    );

    m.func(
        "conducted_cues",
        "The chart cues fired this frame (ADR 0033); empty most frames.",
    )
    .returns(
        ArrayKind,
        "[#{name, age, params}], params the chart's table as numbers, strings and bools",
    )
    .run(|call, ()| -> Array {
        snapshot(call)
            .cues
            .iter()
            .map(|cue| {
                let mut params = Map::new();
                for (k, v) in &cue.params {
                    let d = match v {
                        CueParam::Number(n) => Dynamic::from(*n),
                        CueParam::Text(s) => Dynamic::from(s.clone()),
                        CueParam::Flag(b) => Dynamic::from(*b),
                    };
                    params.insert(k.as_str().into(), d);
                }
                let mut map = Map::new();
                map.insert("name".into(), Dynamic::from(cue.name.clone()));
                map.insert("age".into(), Dynamic::from(cue.age));
                map.insert("params".into(), Dynamic::from_map(params));
                Dynamic::from_map(map)
            })
            .collect()
    });

    m.func(
        "conducted_no_input",
        "Whether input capture has delivered nothing since the bar-2 check (a dead controller).",
    )
    .returns(Bool, "")
    .run(|call, ()| snapshot(call).no_input);
    m.func(
        "conducted_preroll",
        "Whether the session is still in its count-in, when the world should stay untouched.",
    )
    .returns(Bool, "")
    .run(|call, ()| snapshot(call).preroll);
}

/// The `conducted` module.
pub(crate) struct ConductedModule;

pub(crate) static CONDUCTED: ConductedModule = ConductedModule;

impl ScriptModule for ConductedModule {
    fn name(&self) -> &'static str {
        "conducted"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene's music_session has a chart"
    }

    fn declare(&self, m: &mut Module<'_>) {
        declare_conducted(m);
    }
}

#[cfg(test)]
#[path = "conducted_tests.rs"]
mod tests;
