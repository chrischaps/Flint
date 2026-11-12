//! The `conducted` module (ADR 0020, #698): the rhythm session's snapshot
//! as scripts read it, its neutral defaults, and the error in a scene with
//! no chart.

use super::{ConductedCue, ConductedPulse, ConductedSnapshot, CueParam};
use flint_script::ScriptSystem;

/// The player's script system, with the conducted module enabled or not.
fn system(enabled: bool) -> ScriptSystem {
    let mut system = crate::script_system();
    if enabled {
        system.set_enabled_modules(vec!["conducted"]);
    }
    system
}

fn eval<T: Clone + Send + Sync + 'static>(system: &mut ScriptSystem, src: &str) -> T {
    system
        .rhai_engine_mut()
        .eval::<T>(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
}

#[test]
fn the_getters_read_the_snapshot_the_session_set() {
    let mut s = system(true);
    s.set_module_state(ConductedSnapshot {
        lean: [0.25, -0.5],
        sway: [0.6, -0.2],
        pressure_l: 0.15,
        pressure_r: 0.85,
        cues: vec![ConductedCue {
            name: "surge".into(),
            age: 0.004,
            params: vec![
                ("depth".into(), CueParam::Number(0.7)),
                ("mode".into(), CueParam::Text("bank".into())),
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
    });
    assert_eq!(eval::<f64>(&mut s, "conducted_coherence()"), 0.625);
    assert_eq!(eval::<f64>(&mut s, "conducted_lean().x"), 0.25);
    assert_eq!(eval::<f64>(&mut s, "conducted_target().y"), 0.75);
    assert_eq!(eval::<f64>(&mut s, "conducted_next_target().x"), 0.3);
    assert_eq!(eval::<f64>(&mut s, "conducted_next_target().beats"), 2.5);
    assert_eq!(eval::<i64>(&mut s, "conducted_bar()"), 7);
    assert_eq!(eval::<i64>(&mut s, "conducted_pulses().len()"), 2);
    assert_eq!(eval::<f64>(&mut s, "conducted_pulses()[0].err_ms"), -12.5);
    assert!(eval::<bool>(
        &mut s,
        "conducted_pulses()[1].kind == \"miss\""
    ));
    assert!(eval::<bool>(&mut s, "conducted_section() == \"verse\""));
    assert_eq!(eval::<f64>(&mut s, "conducted_reassembly()"), 0.4);
    assert_eq!(eval::<f64>(&mut s, "conducted_sway().x"), 0.6);
    assert_eq!(eval::<f64>(&mut s, "conducted_pressure_r()"), 0.85);
    assert_eq!(eval::<f64>(&mut s, "conducted_next_pulse().beats"), 1.75);
    assert!(eval::<bool>(&mut s, "conducted_next_pulse().open"));
    assert_eq!(eval::<f64>(&mut s, "conducted_cues()[0].params.depth"), 0.7);
    assert!(eval::<bool>(
        &mut s,
        "conducted_cues()[0].params.mode == \"bank\""
    ));
}

#[test]
fn with_no_session_the_getters_read_a_settled_world() {
    // Enabled (a charted scene) before its session starts: never set, the
    // state is the neutral one, so bindings never branch on a session
    let mut s = system(true);
    assert_eq!(eval::<f64>(&mut s, "conducted_coherence()"), 1.0);
    assert_eq!(eval::<f64>(&mut s, "conducted_reassembly()"), 1.0);
    assert_eq!(eval::<f64>(&mut s, "conducted_lean().x"), 0.0);
    assert_eq!(eval::<f64>(&mut s, "conducted_next_target().beats"), 1e6);
    assert_eq!(eval::<f64>(&mut s, "conducted_next_pulse().beats"), 1e6);
    assert_eq!(eval::<i64>(&mut s, "conducted_pulses().len()"), 0);
    assert!(eval::<bool>(&mut s, "conducted_section() == \"\""));
    assert!(!eval::<bool>(
        &mut s,
        "conducted_preroll() || conducted_no_input()"
    ));
    assert_eq!(
        eval::<f64>(
            &mut s,
            "conducted_desaturate() + conducted_blur() + conducted_chromatic() \
             + conducted_rewind() + conducted_beat_phase()"
        ),
        0.0
    );
}

#[test]
fn a_scene_with_no_chart_is_told_the_module_is_not_enabled() {
    let mut s = system(false);
    let err = s
        .rhai_engine_mut()
        .eval::<f64>("let x = 1;\nconducted_coherence()")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(
            "conducted_coherence: the conducted module is not enabled here; it is enabled \
             when the scene's music_session has a chart"
        ),
        "{err}"
    );
    assert!(err.contains("line 2"), "at the call: {err}");
    // music_beat() is core: any scene asks it
    assert_eq!(eval::<f64>(&mut s, "music_beat()"), -1.0);
}

#[test]
fn the_module_is_listed_under_its_name_with_when_it_is_enabled() {
    let listing = crate::script_system().api_listing();
    let conducted: Vec<_> = listing
        .declared
        .iter()
        .filter(|e| e.module == "conducted")
        .map(|e| e.name)
        .collect();
    assert_eq!(conducted.len(), 22, "{conducted:?}");
    assert!(conducted.iter().all(|n| n.starts_with("conducted_")));
    assert_eq!(
        listing.enabled_when("conducted"),
        Some("the scene's music_session has a chart")
    );
    assert_eq!(listing.enabled_when("music"), None, "music is core");
}
