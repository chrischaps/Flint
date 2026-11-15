//! Scripts switching whole-frame styles (#369): `set_post_style`,
//! `set_post_style_param` and `reset_post_style` queue commands, the
//! player applies them to the renderer's post-process config, and a set or
//! a reset fires `post_style_changed` to scripts' `on_event`.

use flint_ecs::FlintWorld;
use flint_player::{
    reset_post_style, set_post_style, set_post_style_param, StyleApplied, POST_STYLE_CHANGED,
};
use flint_render::{stylize, PostProcessConfig};
use flint_runtime::GameEvent;
use flint_script::engine::ScriptEngine;
use flint_script::ScriptHost;

/// The whole-frame style commands as the player's and a capture's hosts do
/// them (#697): from their `ScriptHost` methods, against a config.
struct PostStyles<'a> {
    config: &'a mut PostProcessConfig,
    authored: &'a PostProcessConfig,
    out: Vec<StyleApplied>,
}

impl ScriptHost for PostStyles<'_> {
    fn set_post_style(&mut self, style: String, enabled: bool) {
        let r = set_post_style(self.config, &style, enabled);
        self.out.push(r);
    }
    fn set_post_style_param(&mut self, style: String, param: String, value: toml::Value) {
        let r = set_post_style_param(self.config, &style, &param, value);
        self.out.push(r);
    }
    fn reset_post_style(&mut self, style: String) {
        let r = reset_post_style(self.config, self.authored, &style);
        self.out.push(r);
    }
}

/// One frame as the player runs it: scripts' updates, then their commands
/// against `config` (the renderer's), with `authored` as the scene's own.
fn frame(
    engine: &mut ScriptEngine,
    world: &mut FlintWorld,
    config: &mut PostProcessConfig,
    authored: &PostProcessConfig,
) -> (Vec<GameEvent>, Vec<String>) {
    engine.call_updates(world);
    let mut events = Vec::new();
    let mut failures = Vec::new();
    let mut host = PostStyles {
        config,
        authored,
        out: Vec::new(),
    };
    flint_script::host::apply_all(&mut host, engine.drain_commands());
    for applied in host.out {
        match applied {
            StyleApplied::Done(Some(e)) => events.push(e),
            StyleApplied::Failed(f) => failures.push(f),
            _ => {}
        }
    }
    (events, failures)
}

/// The one `post_style_changed` event in `events`, as (style, enabled).
fn changed(events: &[GameEvent]) -> (String, bool) {
    match events {
        [GameEvent::Custom { name, data }] if name == POST_STYLE_CHANGED => (
            data.get("style").and_then(|v| v.as_str()).unwrap().into(),
            data.get("enabled").and_then(|v| v.as_bool()).unwrap(),
        ),
        other => panic!("one post_style_changed event expected, got {other:?}"),
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

// Frame 0 turns Kuwahara on unseen, 1 to 4 fade it in, 5 switches it off,
// 6 puts the scene's invert back. Integers, as a web script hands over.
const DREAM: &str = r#"
fn on_update() {
    let me = self_entity();
    let n = get_field(me, "story", "frame");
    if n == 0 {
        set_post_style("kuwahara", true);
        set_post_style_param("kuwahara", "blend", 0);
        set_post_style_param("kuwahara", "radius", 5);
        set_post_style_param("invert", "strength", 1.0);
    } else if n < 5 {
        set_post_style_param("kuwahara", "blend", n / 4.0);
    } else if n == 5 {
        set_post_style("kuwahara", false);
    } else if n == 6 {
        reset_post_style("invert");
    }
    set_field(me, "story", "frame", n + 1);
}
"#;

#[test]
fn a_script_sets_fades_and_clears_a_whole_frame_style() {
    let mut world = FlintWorld::new();
    let mut engine = ScriptEngine::new();
    director(&mut engine, &mut world, DREAM);
    let mut authored = PostProcessConfig::default();
    authored.styles.set_enabled("invert", true);
    authored.styles.set_param("invert", "strength", 0.3);
    let mut config = authored.clone();
    let kuwahara = stylize::find("kuwahara").unwrap();
    let invert = stylize::find("invert").unwrap();

    let (events, failures) = frame(&mut engine, &mut world, &mut config, &authored);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(changed(&events), ("kuwahara".into(), true));
    assert!(kuwahara.is_enabled(&config));
    assert_eq!(config.styles.blend("kuwahara"), 0.0);
    assert_eq!(kuwahara.param(&config, "radius"), 5.0);
    assert_eq!(invert.param(&config, "strength"), 1.0);

    for expect in [0.25, 0.5, 0.75, 1.0] {
        let (events, failures) = frame(&mut engine, &mut world, &mut config, &authored);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(events.is_empty(), "a param change fires nothing");
        assert_eq!(config.styles.blend("kuwahara"), expect);
    }

    let (events, _) = frame(&mut engine, &mut world, &mut config, &authored);
    assert_eq!(changed(&events), ("kuwahara".into(), false));
    assert!(!kuwahara.is_enabled(&config));

    let (events, _) = frame(&mut engine, &mut world, &mut config, &authored);
    assert_eq!(changed(&events), ("invert".into(), true));
    assert_eq!(invert.param(&config, "strength"), 0.3);
}

#[test]
fn post_style_mistakes_reach_the_log_and_change_nothing() {
    let mut world = FlintWorld::new();
    let mut engine = ScriptEngine::new();
    director(
        &mut engine,
        &mut world,
        r#"
fn on_update() {
    set_post_style("watercolour", true);
    set_post_style_param("kuwahara", "cell", 12.0);
    set_post_style_param("ascii", "scope", "planet");
}
"#,
    );
    let authored = PostProcessConfig::default();
    let mut config = authored.clone();
    let (events, failures) = frame(&mut engine, &mut world, &mut config, &authored);
    assert!(events.is_empty());
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        failures[0].contains("no style named `watercolour`"),
        "{failures:?}"
    );
    assert!(failures[1].contains("radius"), "{failures:?}");
    assert!(failures[2].contains("entities"), "{failures:?}");
    assert_eq!(config.styles, authored.styles);
}
