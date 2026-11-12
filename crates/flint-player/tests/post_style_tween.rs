//! `tween_post_style_param` (#406): the engine eases a whole-frame style's
//! number, here the `reveal` that tears the live frame open into a style,
//! so one script call drives the whole transition.

use flint_ecs::FlintWorld;
use flint_player::{StyleApplied, StyleTweens, POST_STYLE_TWEEN_DONE};
use flint_render::PostProcessConfig;
use flint_runtime::GameEvent;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;

fn reveal(config: &PostProcessConfig, style: &str) -> Option<f32> {
    config.styles.reveal(style).and_then(|r| r.amount)
}

fn done(events: &[GameEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            GameEvent::Custom { name, data } if name == POST_STYLE_TWEEN_DONE => Some((
                data["style"].as_str().unwrap().to_string(),
                data["param"].as_str().unwrap().to_string(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn a_reveal_tween_opens_from_the_live_frame_to_the_style() {
    let mut config = PostProcessConfig::default();
    config.styles.set_enabled("pencil", true);
    assert!(config.styles.shows("pencil"), "no reveal: all of it");
    let mut tweens = StyleTweens::default();

    let applied = tweens.tween_post(&mut config, "pencil", "reveal", 1.0, 1.0, "linear");
    assert!(matches!(applied, StyleApplied::Done(None)), "{applied:?}");
    // It starts where the style had none: at 0, so the first frame is live
    assert_eq!(reveal(&config, "pencil"), Some(0.0));
    assert!(!config.styles.shows("pencil"));
    assert!(tweens.is_tweening_post("pencil", "reveal"));

    let mut events = Vec::new();
    for i in 1..=60 {
        let (step, moved) = tweens.step_post(&mut config, 1.0 / 60.0);
        assert!(moved);
        assert!(step.failures.is_empty(), "{:?}", step.failures);
        events.extend(step.events);
        if i == 30 {
            let half = reveal(&config, "pencil").unwrap();
            assert!((half - 0.5).abs() < 1e-3, "{half}");
        }
    }
    assert_eq!(reveal(&config, "pencil"), Some(1.0), "the target, exactly");
    assert_eq!(done(&events), [("pencil".into(), "reveal".into())]);
    assert_eq!(tweens.post_len(), 0);
    let (_, moved) = tweens.step_post(&mut config, 1.0 / 60.0);
    assert!(!moved, "nothing running: the config is left alone");
}

#[test]
fn a_new_tween_replaces_the_old_and_a_set_stops_it() {
    let mut config = PostProcessConfig::default();
    let mut tweens = StyleTweens::default();
    tweens.tween_post(&mut config, "pencil", "reveal", 1.0, 2.0, "smooth");
    tweens.step_post(&mut config, 1.0);
    let midway = reveal(&config, "pencil").unwrap();
    assert!(midway > 0.4 && midway < 0.6, "{midway}");
    // Back out from wherever it had got to
    tweens.tween_post(&mut config, "pencil", "reveal", 0.0, 1.0, "linear");
    assert_eq!(tweens.post_len(), 1);
    assert_eq!(reveal(&config, "pencil"), Some(midway));
    tweens.step_post(&mut config, 0.5);
    let back = reveal(&config, "pencil").unwrap();
    assert!((back - midway / 2.0).abs() < 1e-3, "{back}");
    // Blend and reveal ease side by side; stopping one leaves the other
    tweens.tween_post(&mut config, "pencil", "blend", 0.5, 1.0, "linear");
    tweens.stop_post("pencil", Some("reveal"));
    assert!(!tweens.is_tweening_post("pencil", "reveal"));
    assert!(tweens.is_tweening_post("pencil", "blend"));
    tweens.clear();
    assert_eq!(tweens.post_len(), 0);
}

#[test]
fn no_time_sets_it_now_and_mistakes_are_refused() {
    let mut config = PostProcessConfig::default();
    let mut tweens = StyleTweens::default();
    let applied = tweens.tween_post(&mut config, "pencil", "reveal_edge", 5.0, 0.0, "linear");
    let StyleApplied::Done(Some(event)) = applied else {
        panic!("{applied:?}")
    };
    assert_eq!(done(&[event]), [("pencil".into(), "reveal_edge".into())]);
    assert_eq!(
        config
            .styles
            .get("pencil")
            .unwrap()
            .reveal
            .number("reveal_edge"),
        5.0
    );
    let before = config.clone();
    for (style, param, easing) in [
        ("pencil", "reveal_shape", "linear"),
        ("pencil", "direction", "linear"),
        ("pencil", "reveal", "bouncy"),
        ("pencl", "reveal", "linear"),
    ] {
        let applied = tweens.tween_post(&mut config, style, param, 1.0, 1.0, easing);
        assert!(
            matches!(applied, StyleApplied::Failed(_)),
            "{style}.{param}"
        );
    }
    assert_eq!(
        config.styles, before.styles,
        "nothing refused changed anything"
    );
    assert_eq!(tweens.post_len(), 0);
}

#[test]
fn one_script_call_queues_the_tween() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("director").unwrap();
    let ast = engine
        .compile(r#"fn on_update() { tween_post_style_param("pencil", "reveal", 1, 4.0, "ease_in_out"); }"#)
        .unwrap();
    engine.add_script(id, ast, "director.rhai".into());
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    let commands = engine.drain_commands();
    assert!(
        commands.iter().any(|c| matches!(
            c,
            ScriptCommand::TweenPostStyleParam { style, param, target, seconds, easing }
                if style == "pencil" && param == "reveal" && target == &1.0
                    && seconds == &4.0 && easing == "ease_in_out"
        )),
        "{commands:?}"
    );
}
