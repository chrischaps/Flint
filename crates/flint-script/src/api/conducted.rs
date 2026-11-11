//! The `conducted` module: a music session's per-frame state for scene
//! bindings (ADR 0020, ADR 0116): lean, target, coherence, the musical
//! position, the pulses judged and the ladder and reintegration visual
//! ramps. With no session active every getter reads the neutral state (a
//! clean, settled world), so a script never branches on there being one.
//! A subsystem's module: #698 gates it behind the music session.

use super::builder::kind::{Array as ArrayKind, Bool, Int, Map as MapKind, Num, Str};
use super::builder::{Module, ScriptApi};
use crate::context::{ConductedSnapshot, CueParam};
use rhai::{Array, Dynamic, Map};

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
        .run(move |call, ()| field(&call.ctx_ref().conducted));
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
        .run(move |call, ()| xy(field(&call.ctx_ref().conducted)));
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("conducted");

    pair(
        &mut m,
        "conducted_lean",
        "The player's lean.",
        "#{x, y}, each -1..1",
        |c| c.lean,
    );
    pair(
        &mut m,
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
        let c = &call.ctx_ref().conducted;
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
        let c = &call.ctx_ref().conducted;
        let mut map = Map::new();
        map.insert("beats".into(), Dynamic::from(c.next_pulse_beats));
        map.insert("open".into(), Dynamic::from(c.pulse_window_open));
        map
    });

    scalar(
        &mut m,
        "conducted_coherence",
        "How well the player follows the chart.",
        "0..1; 1 with no session",
        |c| c.coherence,
    );
    scalar(
        &mut m,
        "conducted_beat_phase",
        "How far through the current beat the music is.",
        "0..1, 0 at the beat",
        |c| c.beat_phase,
    );
    scalar(
        &mut m,
        "conducted_bar_phase",
        "How far through the current bar the music is.",
        "0..1, 0 at the bar line",
        |c| c.bar_phase,
    );
    scalar(
        &mut m,
        "conducted_beat",
        "Suite beats from zero, accumulated across tempo changes.",
        "beats",
        |c| c.beat,
    );
    scalar(
        &mut m,
        "conducted_desaturate",
        "The ladder's desaturation ramp.",
        "0..1, 0 clean",
        |c| c.desaturate,
    );
    scalar(
        &mut m,
        "conducted_blur",
        "The ladder's blur ramp.",
        "0..1, 0 clean",
        |c| c.blur,
    );
    scalar(
        &mut m,
        "conducted_chromatic",
        "The ladder's chromatic-aberration ramp.",
        "0..1, 0 clean",
        |c| c.chromatic,
    );
    scalar(
        &mut m,
        "conducted_reassembly",
        "Reassembly progress after a fall.",
        "1 in normal play, 0 to 1 while re-gathering",
        |c| c.reassembly,
    );
    scalar(
        &mut m,
        "conducted_rewind",
        "The rewind interlude's progress.",
        "0..1, 0 when not rewinding",
        |c| c.rewind,
    );

    m.func("conducted_bar", "The current bar, from zero.")
        .returns(Int, "the bar index")
        .run(|call, ()| call.ctx_ref().conducted.bar);

    m.func("conducted_section", "The current section of the chart.")
        .returns(Str, "its name; \"\" when none")
        .run(|call, ()| call.ctx_ref().conducted.section.clone());

    m.func(
        "conducted_pulses",
        "The pulses judged this frame; empty most frames.",
    )
    .returns(ArrayKind, "[#{age, err_ms, kind}]")
    .run(|call, ()| -> Array {
        call.ctx_ref()
            .conducted
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
        &mut m,
        "conducted_sway",
        "The player's sway (the right stick).",
        "#{x, y}; zeros under the prototype input map",
        |c| c.sway,
    );
    scalar(
        &mut m,
        "conducted_pressure_l",
        "The left trigger's depth.",
        "0..1; 0 under the prototype input map",
        |c| c.pressure_l,
    );
    scalar(
        &mut m,
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
        call.ctx_ref()
            .conducted
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
    .run(|call, ()| call.ctx_ref().conducted.no_input);
    m.func(
        "conducted_preroll",
        "Whether the session is still in its count-in, when the world should stay untouched.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().conducted.preroll);
}
