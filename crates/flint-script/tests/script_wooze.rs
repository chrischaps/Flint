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

/// The panel's knobs by another door (#218): a script's table is a `.wooze.toml`
/// by the time it arrives, whatever kinds of number it was written with.
#[test]
fn tune_living_takes_what_a_preset_file_holds() {
    let commands = commands_of(
        r#"
        fn on_init() {
            tune_living(#{
                wooze: #{ step: 0.25, glide: 1 },
                living: #{
                    stagger: "per_branch",
                    echoes: #{ count: 3, spread: 1.5 },
                    variance: #{ amount: 1, keep_size: false },
                },
                boil: #{ enabled: true, contour: 0.5, on: 3 },
            });
        }
        "#,
    );
    let [ScriptCommand::TuneLiving { knobs }] = &commands[..] else {
        panic!("{commands:?}");
    };
    let tune: flint_procgen::WoozePreset = knobs.clone().try_into().unwrap();
    assert_eq!(tune.wooze.step, Some(0.25));
    assert_eq!(tune.wooze.glide, Some(1.0));
    assert_eq!(tune.wooze.amplitude, None);
    assert_eq!(tune.living.stagger.as_deref(), Some("per_branch"));
    let echoes = tune.living.echoes.unwrap();
    assert_eq!((echoes.count, echoes.spread), (Some(3), Some(1.5)));
    let variance = tune.living.variance.unwrap();
    assert_eq!(variance.amount, Some(1.0));
    assert_eq!(variance.keep_size, Some(false));
    let boil = tune.boil.unwrap();
    assert_eq!((boil.contour, boil.on), (Some(0.5), Some(3)));
}

#[test]
fn a_script_can_freeze_step_save_and_load() {
    let commands = commands_of(
        r#"
        fn on_init() {
            freeze_living(true);
            step_living();
            save_living_preset("misty");
            load_living_preset("calm");
            freeze_living(false);
        }
        "#,
    );
    assert!(matches!(
        commands[..],
        [
            ScriptCommand::FreezeLiving { frozen: true },
            ScriptCommand::StepLiving,
            ScriptCommand::SaveLivingPreset { .. },
            ScriptCommand::LoadLivingPreset { .. },
            ScriptCommand::FreezeLiving { frozen: false },
        ]
    ));
    assert!(commands
        .iter()
        .any(|c| matches!(c, ScriptCommand::SaveLivingPreset { name } if name == "misty")));
}

/// One species' own values (#246): the flat form, with a species named, arrives
/// as it was written, and clear_living takes a species and any keys.
#[test]
fn a_script_can_tune_and_clear_one_species() {
    let commands = commands_of(
        r#"
        fn on_init() {
            tune_living(#{ species: "grove_thorn", step: 0.25, echoes: #{ count: 1 } });
            clear_living("grove_thorn", ["echoes", "step"]);
            clear_living("grove_oak");
        }
        "#,
    );
    let [ScriptCommand::TuneLiving { knobs }, ScriptCommand::ClearLiving {
        species: thorn,
        keys,
    }, ScriptCommand::ClearLiving {
        species: oak,
        keys: none,
    }] = &commands[..]
    else {
        panic!("{commands:?}");
    };
    assert_eq!(knobs["species"].as_str(), Some("grove_thorn"));
    assert_eq!(knobs["echoes"]["count"].as_integer(), Some(1));
    assert_eq!(thorn, "grove_thorn");
    assert_eq!(keys, &["echoes".to_string(), "step".to_string()]);
    assert_eq!(oak, "grove_oak");
    assert!(none.is_empty());
}
