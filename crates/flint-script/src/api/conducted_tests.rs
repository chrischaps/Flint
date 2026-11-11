//! The `conducted` module (ADR 0020): the music session's snapshot as
//! scripts read it, its neutral defaults, and a bad script beside it.

use crate::context::InputSnapshot;
use crate::engine::test_support::*;
use crate::engine::ScriptEngine;
use flint_ecs::FlintWorld;

#[test]
fn test_conducted_getters_round_trip() {
    use crate::context::{ConductedPulse, ConductedSnapshot};
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("binder").unwrap();
    probe_component(&mut world, id);

    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            set_field(me, "probe", "coh", conducted_coherence());
            set_field(me, "probe", "lx", conducted_lean().x);
            set_field(me, "probe", "ty", conducted_target().y);
            let nt = conducted_next_target();
            set_field(me, "probe", "ntx", nt.x);
            set_field(me, "probe", "nty", nt.y);
            set_field(me, "probe", "ntb", nt.beats);
            set_field(me, "probe", "bar", conducted_bar());
            let pulses = conducted_pulses();
            set_field(me, "probe", "npulses", pulses.len().to_float());
            if pulses.len() >= 2 {
                set_field(me, "probe", "kind_ok",
                    if pulses[0].kind == "hit" && pulses[1].kind == "miss" { 1.0 } else { 0.0 });
                set_field(me, "probe", "age", pulses[0].age);
                set_field(me, "probe", "err", pulses[0].err_ms);
            }
            set_field(me, "probe", "sec_ok",
                if conducted_section() == "verse" { 1.0 } else { 0.0 });
            set_field(me, "probe", "reassembly", conducted_reassembly());
            set_field(me, "probe", "preroll",
                if conducted_preroll() { 1.0 } else { 0.0 });
            let sw = conducted_sway();
            set_field(me, "probe", "swx", sw.x);
            set_field(me, "probe", "pl", conducted_pressure_l());
            set_field(me, "probe", "pr", conducted_pressure_r());
            let np = conducted_next_pulse();
            set_field(me, "probe", "npb", np.beats);
            set_field(me, "probe", "np_open", if np.open { 1.0 } else { 0.0 });
            let cues = conducted_cues();
            set_field(me, "probe", "ncues", cues.len().to_float());
            if cues.len() > 0 {
                set_field(me, "probe", "cue_ok",
                    if cues[0].name == "surge" { 1.0 } else { 0.0 });
                set_field(me, "probe", "cdepth", cues[0].params.depth);
                set_field(me, "probe", "cmode_ok",
                    if cues[0].params.mode == "bank" { 1.0 } else { 0.0 });
            }
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "binder.rhai".into());

    {
        let mut c = engine.ctx.lock().unwrap();
        c.conducted = ConductedSnapshot {
            lean: [0.25, -0.5],
            sway: [0.6, -0.2],
            pressure_l: 0.15,
            pressure_r: 0.85,
            cues: vec![crate::context::ConductedCue {
                name: "surge".into(),
                age: 0.004,
                params: vec![
                    ("depth".into(), crate::context::CueParam::Number(0.7)),
                    ("mode".into(), crate::context::CueParam::Text("bank".into())),
                ],
            }],
            target: [0.1, 0.75],
            next_target: [0.3, -0.4],
            next_target_beats: 2.5,
            next_pulse_beats: 1.75,
            pulse_window_open: true,
            coherence: 0.625,
            beat_phase: 0.5,
            bar_phase: 0.125,
            bar: 7,
            beat: 30.5,
            section: "verse".into(),
            pulses: vec![
                ConductedPulse {
                    age: 0.031,
                    err_ms: -12.5,
                    kind: "hit".into(),
                },
                ConductedPulse {
                    age: 0.002,
                    err_ms: 0.0,
                    kind: "miss".into(),
                },
            ],
            desaturate: 0.3,
            blur: 0.2,
            chromatic: 0.1,
            reassembly: 0.4,
            rewind: 0.0,
            no_input: false,
            preroll: false,
        };
    }
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);

    assert_eq!(probe_float(&world, id, "coh"), 0.625);
    assert_eq!(probe_float(&world, id, "lx"), 0.25);
    assert_eq!(probe_float(&world, id, "ty"), 0.75);
    assert_eq!(probe_float(&world, id, "ntx"), 0.3);
    assert_eq!(probe_float(&world, id, "nty"), -0.4);
    assert_eq!(probe_float(&world, id, "ntb"), 2.5);
    let bar = world
        .get_components(id)
        .unwrap()
        .get_field("probe", "bar")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(bar, 7);
    assert_eq!(probe_float(&world, id, "npulses"), 2.0);
    assert_eq!(probe_float(&world, id, "kind_ok"), 1.0);
    assert_eq!(probe_float(&world, id, "age"), 0.031);
    assert_eq!(probe_float(&world, id, "err"), -12.5);
    assert_eq!(probe_float(&world, id, "sec_ok"), 1.0);
    assert_eq!(probe_float(&world, id, "reassembly"), 0.4);
    assert_eq!(probe_float(&world, id, "preroll"), 0.0);
    assert_eq!(probe_float(&world, id, "swx"), 0.6);
    assert_eq!(probe_float(&world, id, "pl"), 0.15);
    assert_eq!(probe_float(&world, id, "pr"), 0.85);
    assert_eq!(probe_float(&world, id, "npb"), 1.75);
    assert_eq!(probe_float(&world, id, "np_open"), 1.0);
    assert_eq!(probe_float(&world, id, "ncues"), 1.0);
    assert_eq!(probe_float(&world, id, "cue_ok"), 1.0);
    assert_eq!(probe_float(&world, id, "cdepth"), 0.7);
    assert_eq!(probe_float(&world, id, "cmode_ok"), 1.0);
}

#[test]
fn test_conducted_neutral_defaults() {
    // Never call set_conducted: getters must read as a clean, settled
    // world so bindings never branch on session existence.
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("binder").unwrap();
    probe_component(&mut world, id);

    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            set_field(me, "probe", "coh", conducted_coherence());
            set_field(me, "probe", "reassembly", conducted_reassembly());
            set_field(me, "probe", "lx", conducted_lean().x);
            let nt = conducted_next_target();
            set_field(me, "probe", "ntx", nt.x + nt.y);
            set_field(me, "probe", "ntb", nt.beats);
            let np = conducted_next_pulse();
            set_field(me, "probe", "npb", np.beats);
            set_field(me, "probe", "np_open", if np.open { 1.0 } else { 0.0 });
            set_field(me, "probe", "npulses", conducted_pulses().len().to_float());
            set_field(me, "probe", "sec_ok",
                if conducted_section() == "" { 1.0 } else { 0.0 });
            set_field(me, "probe", "preroll",
                if conducted_preroll() || conducted_no_input() { 1.0 } else { 0.0 });
            set_field(me, "probe", "err", conducted_desaturate() + conducted_blur()
                + conducted_chromatic() + conducted_rewind() + conducted_beat_phase());
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "binder.rhai".into());
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);

    assert_eq!(probe_float(&world, id, "coh"), 1.0);
    assert_eq!(probe_float(&world, id, "reassembly"), 1.0);
    assert_eq!(probe_float(&world, id, "lx"), 0.0);
    assert_eq!(probe_float(&world, id, "ntx"), 0.0);
    assert_eq!(
        probe_float(&world, id, "ntb"),
        1e6,
        "sentinel: nothing inbound"
    );
    assert_eq!(
        probe_float(&world, id, "npb"),
        1e6,
        "sentinel: no window inbound"
    );
    assert_eq!(probe_float(&world, id, "np_open"), 0.0);
    assert_eq!(probe_float(&world, id, "npulses"), 0.0);
    assert_eq!(probe_float(&world, id, "sec_ok"), 1.0);
    assert_eq!(probe_float(&world, id, "preroll"), 0.0);
    assert_eq!(probe_float(&world, id, "err"), 0.0);
}

#[test]
fn test_conducted_bad_script_continues() {
    // A binding that throws every frame must not stop other scripts from
    // reading conducted values (log-and-continue, verified not assumed).
    use crate::context::ConductedSnapshot;
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let bad = world.spawn("bad").unwrap();
    let good = world.spawn("good").unwrap();
    probe_component(&mut world, good);

    let bad_ast = engine
        .compile("fn on_update() { this_function_does_not_exist(); }")
        .unwrap();
    engine.add_script(bad, bad_ast, "bad.rhai".into());
    let good_ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            let acc = get_field(me, "probe", "coh");
            set_field(me, "probe", "coh", acc + conducted_coherence());
        }
    "#,
        )
        .unwrap();
    engine.add_script(good, good_ast, "good.rhai".into());

    engine.ctx.lock().unwrap().conducted = ConductedSnapshot {
        coherence: 0.5,
        ..Default::default()
    };
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    // Two frames: the good script must accumulate across both even
    // though the bad one errors every time.
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, good, "coh"), -1.0 + 0.5 + 0.5);
}

// ─── Camera roll override (ADR 0022) ──────────────────────
