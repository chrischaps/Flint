//! Integration tests: scripts asking for changes to the wooze field (#213).
//!
//! The commands land in the player, which owns the field; here the script's half
//! is held to what `flint_core::wooze::WoozeField` takes.

use flint_core::wooze::{WoozeField, WoozeSample};
use flint_ecs::FlintWorld;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;

/// What a script asked for in its `on_init`.
fn commands_of(src: &str) -> Vec<ScriptCommand> {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("director").unwrap();
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, "director.rhai".into());
    engine.call_inits(&mut world);
    engine.drain_commands()
}

#[test]
fn set_wooze_takes_a_map_with_seconds_as_either_kind_of_number() {
    let commands = commands_of(
        r#"
        fn on_init() {
            set_wooze(#{ step: 0.25 });
            set_wooze(#{ glide: 1 }, 2.5);
            set_wooze(#{ preset: "boil" }, 3);
        }
        "#,
    );
    let mut field = WoozeField::new(WoozeSample::NEUTRAL);
    let mut seconds_seen = Vec::new();
    for command in &commands {
        let ScriptCommand::SetWooze { knobs, seconds } = command else {
            panic!("{command:?}");
        };
        seconds_seen.push(*seconds);
        field
            .set_global(knobs.as_table().unwrap(), 0.0, 0.0)
            .unwrap();
    }
    assert_eq!(seconds_seen, [0.0, 2.5, 3.0]);
    // The whole way through: a Rhai integer is a number to the field too.
    assert_eq!(
        field.sample([0.0; 3], 0.0).step,
        0.125,
        "the preset came last"
    );
}

#[test]
fn set_wooze_zone_can_make_a_zone_from_a_map() {
    let commands = commands_of(
        r#"
        fn on_init() {
            set_wooze_zone("pocket", #{
                shape: "sphere",
                center: [0.0, 0, 12.0],
                radius: 6,
                falloff: 4.0,
                glide: 0.8,
            }, 1.5);
            set_wooze_zone("pocket", #{ center: [1.0, 2.0, 3.0] });
            set_wooze_zone("pocket", #{ jitter: 2.0 }, 1);
        }
        "#,
    );
    let mut field = WoozeField::new(WoozeSample::NEUTRAL);
    let mut seconds_seen = Vec::new();
    for command in &commands {
        let ScriptCommand::SetWoozeZone {
            name,
            fields,
            seconds,
        } = command
        else {
            panic!("{command:?}");
        };
        assert_eq!(name, "pocket");
        seconds_seen.push(*seconds);
        field
            .set_zone(name, fields.as_table().unwrap(), 0.0, 0.0)
            .unwrap();
    }
    assert_eq!(seconds_seen, [1.5, 0.0, 1.0]);
    assert_eq!(field.zones().len(), 1, "the later calls edit the same zone");
    let inside = field.sample([1.0, 2.0, 3.0], 0.0);
    assert_eq!((inside.glide, inside.jitter), (0.8, 2.0));
}

#[test]
fn remove_wooze_zone_names_it_and_can_fade() {
    let commands = commands_of(
        r#"
        fn on_init() {
            remove_wooze_zone("a");
            remove_wooze_zone("b", 0.5);
            remove_wooze_zone("c", 2);
        }
        "#,
    );
    let found: Vec<(String, f64)> = commands
        .into_iter()
        .map(|command| match command {
            ScriptCommand::RemoveWoozeZone { name, seconds } => (name, seconds),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        found,
        [
            ("a".to_string(), 0.0),
            ("b".to_string(), 0.5),
            ("c".to_string(), 2.0)
        ]
    );
}
