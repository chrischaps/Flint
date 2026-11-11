//! Scripts setting and clearing an entity's style (#269): the Rhai calls
//! queue commands, a host applies them to the world through the one
//! dispatcher (#697), and a set or a clear fires `style_changed` to
//! scripts' `on_event`.

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_player::{Easing, StyleApplied, StyleTweens, STYLE_CHANGED, STYLE_TWEEN_DONE};
use flint_render::PostProcessConfig;
use flint_runtime::GameEvent;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;
use flint_script::ScriptHost;

/// The style commands as the player's and a capture's hosts do them: each
/// `StyleTweens` method, from its `ScriptHost` method, with what it did.
struct Styles<'a> {
    world: &'a mut FlintWorld,
    tweens: &'a mut StyleTweens,
    config: &'a PostProcessConfig,
    out: Vec<StyleApplied>,
}

impl ScriptHost for Styles<'_> {
    fn set_entity_style(&mut self, entity: EntityId, style: String) {
        let r = self.tweens.set_entity_style(self.world, entity, &style);
        self.out.push(r);
    }
    fn set_style_param(&mut self, entity: EntityId, param: String, value: toml::Value) {
        let r = self
            .tweens
            .set_style_param(self.world, entity, &param, value);
        self.out.push(r);
    }
    fn tween_style_param(
        &mut self,
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        easing: String,
    ) {
        let r = self.tweens.tween(
            self.world,
            self.config,
            entity,
            &param,
            &target,
            seconds,
            &easing,
        );
        self.out.push(r);
    }
    fn clear_entity_style(&mut self, entity: EntityId) {
        let r = self.tweens.clear_entity_style(self.world, entity);
        self.out.push(r);
    }
}

/// `commands` through the dispatcher to the style methods: what each did.
fn run(
    world: &mut FlintWorld,
    tweens: &mut StyleTweens,
    config: &PostProcessConfig,
    commands: Vec<ScriptCommand>,
) -> Vec<StyleApplied> {
    let mut host = Styles {
        world,
        tweens,
        config,
        out: Vec::new(),
    };
    flint_script::host::apply_all(&mut host, commands);
    host.out
}

/// One style command: what it did.
fn one(
    tweens: &mut StyleTweens,
    world: &mut FlintWorld,
    config: &PostProcessConfig,
    command: ScriptCommand,
) -> StyleApplied {
    run(world, tweens, config, vec![command])
        .pop()
        .expect("a style command says what it did")
}

/// One frame as the player runs it: scripts' updates, then their commands.
/// Returns the events the commands fired and the failures they logged.
fn frame(engine: &mut ScriptEngine, world: &mut FlintWorld) -> (Vec<GameEvent>, Vec<String>) {
    engine.call_updates(world);
    let mut events = Vec::new();
    let mut failures = Vec::new();
    let commands = engine.drain_commands();
    let mut tweens = StyleTweens::default();
    for applied in run(world, &mut tweens, &PostProcessConfig::default(), commands) {
        match applied {
            StyleApplied::Done(Some(e)) => events.push(e),
            StyleApplied::Failed(f) => failures.push(f),
            _ => {}
        }
    }
    (events, failures)
}

/// The one `style_changed` event in `events`, as (entity, style).
fn style_changed(events: &[GameEvent]) -> (i64, String) {
    match events {
        [GameEvent::Custom { name, data }] if name == STYLE_CHANGED => (
            data.get("entity").and_then(|v| v.as_integer()).unwrap(),
            data.get("style")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string(),
        ),
        other => panic!("one style_changed event expected, got {other:?}"),
    }
}

fn director(engine: &mut ScriptEngine, world: &mut FlintWorld, src: &str) {
    let id = world.spawn("director").unwrap();
    let mut story = toml::Table::new();
    story.insert("frame".into(), toml::Value::Integer(0));
    world.set_component(id, "story", story.into()).unwrap();
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, "director.rhai".into());
    engine.call_inits(world);
}

// Frame 0 styles the barrel, 1 to 3 grow its cells, 4 clears it. Integer
// literals are what a web script (i64 only) hands over.
const STORY: &str = r#"
fn on_update() {
    let me = self_entity();
    let n = get_field(me, "story", "frame");
    let barrel = get_entity("barrel");
    if n == 0 {
        set_entity_style(barrel, "ascii");
        set_style_param(barrel, "ramp", "blocks");
        set_style_param(barrel, "tint", [1.0, 0.6, 0.2]);
    } else if n < 4 {
        set_style_param(barrel, "cell", 6 + 2 * n);
        set_style_param(barrel, "levels", 4);
    } else if n == 4 {
        clear_entity_style(barrel);
    }
    set_field(me, "story", "frame", n + 1);
}
"#;

#[test]
fn a_script_sets_animates_and_clears_a_style() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut engine = ScriptEngine::new();
    director(&mut engine, &mut world, STORY);

    let (events, failures) = frame(&mut engine, &mut world);
    assert!(failures.is_empty(), "{failures:?}");
    let style = world.get_component(barrel, "ascii_style").expect("styled");
    assert_eq!(style.get("ramp").and_then(|v| v.as_str()), Some("blocks"));
    assert_eq!(
        style
            .get("tint")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(3)
    );
    assert_eq!(
        style_changed(&events),
        (barrel.0 as i64, "ascii".to_string())
    );

    for expect in [8.0, 10.0, 12.0] {
        let (events, failures) = frame(&mut engine, &mut world);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(events.is_empty(), "a param change fires nothing");
        let style = world.get_component(barrel, "ascii_style").unwrap();
        assert_eq!(style.get("cell").and_then(|v| v.as_float()), Some(expect));
        assert_eq!(style.get("levels").and_then(|v| v.as_float()), Some(4.0));
    }

    let (events, _) = frame(&mut engine, &mut world);
    assert!(world.get_component(barrel, "ascii_style").is_none());
    assert!(world.entities_with_component("ascii_style").is_empty());
    assert_eq!(style_changed(&events), (barrel.0 as i64, String::new()));
}

// ─── Tweens (#370) ──────────────────────────────────────

/// One frame with tweens, as the player runs it: tweens step by `dt`, then
/// scripts' updates, then their commands.
fn tween_frame(
    engine: &mut ScriptEngine,
    world: &mut FlintWorld,
    tweens: &mut StyleTweens,
    config: &PostProcessConfig,
    dt: f64,
) -> (Vec<GameEvent>, Vec<String>) {
    let step = tweens.step(world, dt);
    let (mut events, mut failures) = (step.events, step.failures);
    engine.call_updates(world);
    let commands = engine.drain_commands();
    for applied in run(world, tweens, config, commands) {
        match applied {
            StyleApplied::Done(Some(e)) => events.push(e),
            StyleApplied::Failed(f) => failures.push(f),
            _ => {}
        }
    }
    (events, failures)
}

fn number(world: &FlintWorld, id: flint_core::EntityId, field: &str) -> f64 {
    let c = world.get_component(id, "ascii_style").expect("styled");
    let v = c
        .get(field)
        .unwrap_or_else(|| panic!("no {field} in {c:?}"));
    v.as_float()
        .or_else(|| v.as_integer().map(|i| i as f64))
        .unwrap()
}

fn tween_done(events: &[GameEvent]) -> Vec<(i64, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            GameEvent::Custom { name, data } if name == STYLE_TWEEN_DONE => Some((
                data.get("entity").and_then(|v| v.as_integer()).unwrap(),
                data.get("param")
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_string(),
            )),
            _ => None,
        })
        .collect()
}

// Frame 0 styles the barrel at cell 8 and asks for three tweens: cell to
// 16 linearly over a second, blend 1 -> 0 smoothly over half a second (an
// integer seconds and no easing: what a web script hands over), and the
// tint to orange over a second.
const TWEENS: &str = r##"
fn on_update() {
    let me = self_entity();
    let n = get_field(me, "story", "frame");
    let barrel = get_entity("barrel");
    if n == 0 {
        set_entity_style(barrel, "ascii");
        set_style_param(barrel, "cell", 8);
        set_style_param(barrel, "tint", [0.0, 0.0, 1.0]);
        tween_style_param(barrel, "cell", 16, 1, "linear");
        tween_style_param(barrel, "blend", 0.0, 0.5);
        tween_style_param(barrel, "tint", "#ff8000", 1.0, "linear");
    }
    set_field(me, "story", "frame", n + 1);
}
"##;

#[test]
fn a_tween_steps_on_game_time_and_says_when_it_is_done() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut engine = ScriptEngine::new();
    director(&mut engine, &mut world, TWEENS);
    let mut tweens = StyleTweens::default();
    let config = PostProcessConfig::default();
    let dt = 0.125;

    // Frame 0: asked for; nothing moves yet
    let (_, failures) = tween_frame(&mut engine, &mut world, &mut tweens, &config, dt);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(tweens.len(), 3);
    assert_eq!(number(&world, barrel, "cell"), 8.0);

    // Eight steps of 1/8 s: cell climbs one point a step; blend follows
    // smoothstep and is done after four
    let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
    let mut done = Vec::new();
    for step in 1..=8 {
        let (events, failures) = tween_frame(&mut engine, &mut world, &mut tweens, &config, dt);
        assert!(failures.is_empty(), "{failures:?}");
        let t = step as f64 * dt;
        assert!((number(&world, barrel, "cell") - (8.0 + 8.0 * t)).abs() < 1e-9);
        let blend = 1.0 - smooth((t / 0.5).min(1.0));
        assert!(
            (number(&world, barrel, "blend") - blend).abs() < 1e-9,
            "blend at {t}"
        );
        for d in tween_done(&events) {
            done.push((step, d.1));
        }
    }
    assert_eq!(
        done,
        vec![
            (4, "blend".to_string()),
            (8, "cell".to_string()),
            (8, "tint".to_string())
        ]
    );
    assert!(tweens.is_empty());
    assert_eq!(number(&world, barrel, "cell"), 16.0);
    assert_eq!(number(&world, barrel, "blend"), 0.0);
    let tint = world.get_component(barrel, "ascii_style").unwrap()["tint"].clone();
    let tint: Vec<f64> = tint
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect();
    assert!((tint[0] - 1.0).abs() < 1e-6);
    assert!((tint[1] - 128.0 / 255.0).abs() < 1e-3, "{tint:?}");
    assert!(tint[2].abs() < 1e-6);
}

#[test]
fn a_tween_starts_from_the_scene_table_and_gives_way_to_the_script() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let id = barrel.0 as i64;
    let mut tweens = StyleTweens::default();
    let mut config = PostProcessConfig::default();
    config.styles.set_param("ascii", "cell", 10.0);
    let tween = |param: &str, target: f64, seconds: f64| ScriptCommand::TweenStyleParam {
        entity: barrel,
        param: param.into(),
        target: target.into(),
        seconds,
        easing: "linear".into(),
    };
    let apply = |tweens: &mut StyleTweens, world: &mut FlintWorld, c: ScriptCommand| {
        one(tweens, world, &config, c)
    };

    apply(
        &mut tweens,
        &mut world,
        ScriptCommand::SetEntityStyle {
            entity: barrel,
            style: "ascii".into(),
        },
    );
    // No cell of its own: the tween starts at the scene's 10
    apply(&mut tweens, &mut world, tween("cell", 20.0, 1.0));
    tweens.step(&mut world, 0.5);
    assert_eq!(number(&world, barrel, "cell"), 15.0);

    // A second tween of the same setting takes over from 15
    apply(&mut tweens, &mut world, tween("cell", 5.0, 1.0));
    assert_eq!(tweens.len(), 1);
    tweens.step(&mut world, 0.5);
    assert_eq!(number(&world, barrel, "cell"), 10.0);

    // A set_style_param of it stops the tween where the script put it
    apply(
        &mut tweens,
        &mut world,
        ScriptCommand::SetStyleParam {
            entity: barrel,
            param: "cell".into(),
            value: 30.0.into(),
        },
    );
    assert!(!tweens.is_tweening(barrel, "cell"));
    tweens.step(&mut world, 0.5);
    assert_eq!(number(&world, barrel, "cell"), 30.0);

    // Zero seconds is there at once, and done at once
    match apply(&mut tweens, &mut world, tween("glow", 0.5, 0.0)) {
        StyleApplied::Done(Some(e)) => assert_eq!(tween_done(&[e]), [(id, "glow".into())]),
        other => panic!("{other:?}"),
    }
    assert_eq!(number(&world, barrel, "glow"), 0.5);

    // clear_entity_style drops the entity's tweens
    apply(&mut tweens, &mut world, tween("glow", 1.0, 2.0));
    apply(&mut tweens, &mut world, tween("blend", 0.0, 2.0));
    assert_eq!(tweens.len(), 2);
    apply(
        &mut tweens,
        &mut world,
        ScriptCommand::ClearEntityStyle { entity: barrel },
    );
    assert!(tweens.is_empty());
}

#[test]
fn a_tween_refuses_what_cannot_ease() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut tweens = StyleTweens::default();
    let config = PostProcessConfig::default();
    let tween = |param: &str, target: toml::Value, easing: &str| ScriptCommand::TweenStyleParam {
        entity: barrel,
        param: param.into(),
        target,
        seconds: 1.0,
        easing: easing.into(),
    };
    let failed = |applied: StyleApplied| match applied {
        StyleApplied::Failed(f) => f,
        other => panic!("refusal expected, got {other:?}"),
    };
    let unstyled = failed(one(
        &mut tweens,
        &mut world,
        &config,
        tween("cell", 8.0.into(), ""),
    ));
    assert!(unstyled.contains("set_entity_style first"), "{unstyled}");
    one(
        &mut tweens,
        &mut world,
        &config,
        ScriptCommand::SetEntityStyle {
            entity: barrel,
            style: "ascii".into(),
        },
    );
    let option = failed(one(
        &mut tweens,
        &mut world,
        &config,
        tween("ramp", "blocks".into(), ""),
    ));
    assert!(option.contains("can't ease"), "{option}");
    let easing = failed(one(
        &mut tweens,
        &mut world,
        &config,
        tween("cell", 8.0.into(), "bouncy"),
    ));
    assert!(easing.contains("ease_in_out"), "{easing}");
    let kind = failed(one(
        &mut tweens,
        &mut world,
        &config,
        tween("cell", "big".into(), ""),
    ));
    assert!(kind.contains("a number"), "{kind}");
    assert!(tweens.is_empty());
}

#[test]
fn easings_run_from_zero_to_one() {
    for name in Easing::NAMES {
        let e = Easing::from_name(name).unwrap();
        assert_eq!(e.apply(0.0), 0.0, "{name}");
        assert!((e.apply(1.0) - 1.0).abs() < 1e-12, "{name}");
        assert!(e.apply(0.25) <= e.apply(0.5) && e.apply(0.5) <= e.apply(0.75));
    }
    assert_eq!(Easing::from_name("smooth").unwrap().apply(0.5), 0.5);
    assert_eq!(Easing::from_name("ease_in").unwrap().apply(0.5), 0.125);
    assert_eq!(Easing::from_name("ease_out").unwrap().apply(0.5), 0.875);
}

#[test]
fn mistakes_reach_the_log_and_change_nothing() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut engine = ScriptEngine::new();
    director(
        &mut engine,
        &mut world,
        r#"
fn on_update() {
    let barrel = get_entity("barrel");
    set_style_param(barrel, "cell", 12.0);
    set_entity_style(barrel, "kuwahara");
}
"#,
    );
    let (events, failures) = frame(&mut engine, &mut world);
    assert!(events.is_empty());
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures[0].contains("set_entity_style first"),
        "{failures:?}"
    );
    assert!(failures[1].contains("whole-frame"), "{failures:?}");
    assert!(world.get_component(barrel, "ascii_style").is_none());
}

// Reading back (#372): frame 0 styles the barrel; from frame 1 on the
// script asks what the barrel and the hoop (its child) are drawn in and
// writes the answers into its own component for the test to read.
const READBACK: &str = r##"
fn on_update() {
    let me = self_entity();
    let n = get_field(me, "story", "frame");
    let barrel = get_entity("barrel");
    let hoop = get_entity("hoop");
    if n == 0 {
        set_entity_style(barrel, "ascii");
        set_style_param(barrel, "ramp", "blocks");
        set_style_param(barrel, "cell", 12);
        set_style_param(barrel, "tint", "#ff8000");
    }
    set_field(me, "story", "barrel", get_entity_style(barrel));
    set_field(me, "story", "hoop", get_entity_style(hoop));
    set_field(me, "story", "variant", get_entity_style_variant(hoop));
    set_field(me, "story", "cell", get_style_param(hoop, "cell"));
    set_field(me, "story", "ramp", get_style_param(hoop, "ramp"));
    set_field(me, "story", "glow", get_style_param(hoop, "glow"));
    let tint = get_style_param(hoop, "tint");
    set_field(me, "story", "tint_r", if type_of(tint) == "array" { tint[0] } else { -1.0 });
    let none = get_style_param(get_entity("bench"), "cell");
    set_field(me, "story", "bench_unit", type_of(none) == "()");
    set_field(me, "story", "frame", n + 1);
}
"##;

#[test]
fn a_script_reads_back_its_style_and_an_inherited_one() {
    use flint_player::SceneStyles;
    use flint_render::PostProcessConfig;
    use std::sync::Arc;

    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let hoop = world.spawn("hoop").unwrap();
    world.spawn("bench").unwrap();
    world.set_parent(hoop, barrel).unwrap();
    let mut engine = ScriptEngine::new();
    let mut config = PostProcessConfig::default();
    config.styles.set_param("ascii", "glow", 0.4);
    engine.set_style_reader(Arc::new(SceneStyles::new(config)));
    director(&mut engine, &mut world, READBACK);
    let director = world.get_id("director").unwrap();
    let story = |world: &FlintWorld, key: &str| -> toml::Value {
        world
            .get_component(director, "story")
            .and_then(|s| s.get(key).cloned())
            .unwrap_or_else(|| panic!("story.{key} unset"))
    };

    // Frame 0: the set is queued, so the reads still see it unstyled
    let (_, failures) = frame(&mut engine, &mut world);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(story(&world, "barrel").as_str(), Some(""));
    assert_eq!(story(&world, "hoop").as_str(), Some(""));

    // Frame 1: the barrel's own style, and the hoop's inherited one with
    // the barrel's settings, the scene's, and the defaults under them
    frame(&mut engine, &mut world);
    assert_eq!(story(&world, "barrel").as_str(), Some("ascii"));
    assert_eq!(story(&world, "hoop").as_str(), Some("ascii"));
    let variant = story(&world, "variant");
    let variant = variant.as_str().unwrap();
    assert!(
        variant.starts_with("ascii{")
            && variant.contains("ramp=blocks")
            && variant.contains("cell=12"),
        "{variant}"
    );
    assert_eq!(story(&world, "cell").as_float(), Some(12.0), "the entity's");
    assert_eq!(story(&world, "ramp").as_str(), Some("blocks"));
    let glow = story(&world, "glow").as_float().unwrap();
    assert!((glow - 0.4).abs() < 1e-6, "the scene's: {glow}");
    assert_eq!(story(&world, "tint_r").as_float(), Some(1.0));
    assert_eq!(story(&world, "bench_unit").as_bool(), Some(true));
}

#[test]
fn without_a_reader_every_entity_reads_unstyled() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    world
        .set_component(barrel, "ascii_style", toml::Table::new().into())
        .unwrap();
    let mut engine = ScriptEngine::new();
    director(
        &mut engine,
        &mut world,
        r#"
fn on_update() {
    let me = self_entity();
    set_field(me, "story", "style", get_entity_style(get_entity("barrel")));
}
"#,
    );
    frame(&mut engine, &mut world);
    let director = world.get_id("director").unwrap();
    let style = world
        .get_component(director, "story")
        .unwrap()
        .get("style")
        .cloned();
    assert_eq!(
        style.and_then(|v| v.as_str().map(str::to_string)),
        Some(String::new())
    );
}
