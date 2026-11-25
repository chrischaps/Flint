//! Scene-declared fields through the player (#1014): the subsystem comes
//! with a `[field]` table, a script samples a key by place, changes the
//! field and is told what it got wrong.

use crate::player_app::scene_entry::SceneEntry;
use crate::player_app::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// Two keys and a zone, the issue's example: windier around x = 10.
const WEATHER: &str = r#"
[field.weather]
keys = { wind = { min = 0, default = 1 }, fog = { min = 0, max = 1, default = 0 } }

[[field.weather.zone]]
name = "storm"
shape = "sphere"
center = [10, 0, 0]
radius = 3
wind = 6
"#;

fn scene(fields: &str, script: &str) -> String {
    format!(
        "[scene]\nname = \"Weather\"\n{fields}\n[entities.walker]\n\
         [entities.walker.transform]\nposition = [0, 0, 0]\n\
         [entities.walker.probe]\nwind = -1.0\nfog = -1.0\nnote = \"\"\n\
         [entities.walker.script]\nsource = \"{script}\"\n"
    )
}

fn app_in(scene: &str) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, scene.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
            packages: None,
        },
    );
    app.core.clock.frame_step = Some(0.25);
    app.enter_scene(SceneEntry::Start).expect("the scene starts");
    app
}

fn probe(app: &PlayerApp, field: &str) -> Option<toml::Value> {
    let id = app.core.world.get_id("walker").expect("the walker");
    app.core
        .world
        .get_components(id)
        .and_then(|c| c.get_field("probe", field))
        .cloned()
}

fn number(app: &PlayerApp, field: &str) -> f64 {
    probe(app, field)
        .as_ref()
        .and_then(flint_core::toml_util::toml_f64)
        .expect("a number")
}

fn errors(app: &PlayerApp) -> Vec<String> {
    app.core
        .script
        .errors()
        .all()
        .iter()
        .map(|e| e.message.clone())
        .collect()
}

/// #1014's first box: a scene declares a field with two keys and a zone, and
/// a script reads a key by place that changes as it walks into the zone.
#[test]
fn a_script_reads_a_key_that_changes_as_it_walks_into_the_zone() {
    const WALKER: &str = r#"
fn on_update() {
    let me = self_entity();
    let p = get_position(me);
    set_position(me, p.x + 1.0, p.y, p.z);
    let here = get_position(me);
    set_field(me, "probe", "wind", field_sample("weather", "wind", here));
    set_field(me, "probe", "fog", field_sample("weather", "fog", here.x, here.y, here.z));
}
"#;
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/weather.scene.toml", scene(WEATHER, "walker.rhai"));
    files.insert("game/scripts/walker.rhai", WALKER);
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("game/scenes/weather.scene.toml");
        assert_eq!(app.core.script.enabled_modules(), ["field"]);
        let mut walked = Vec::new();
        for _ in 0..16 {
            app.tick();
            walked.push(number(&app, "wind"));
            assert_eq!(number(&app, "fog"), 0.0, "no zone sets fog");
        }
        assert_eq!(errors(&app), Vec::<String>::new());
        // x = 1, 2, ... 16: the storm reaches from 7 to 13
        let expected: Vec<f64> = (1..=16)
            .map(|x| if (7..=13).contains(&x) { 6.0 } else { 1.0 })
            .collect();
        assert_eq!(walked, expected);
    });
}

/// A script's writes are seen by its next read, in the same frame, and ease
/// on the game clock; a new zone fades in, a removed one goes.
#[test]
fn a_script_changes_the_field_and_reads_the_change_back() {
    const CHANGER: &str = r#"
fn on_init() {
    field_set("weather", #{ fog: 0.5 });
    let here = #{ x: 0.0, y: 0.0, z: 0.0 };
    set_field(self_entity(), "probe", "fog", field_sample("weather", "fog", here));
    field_set("weather", #{ wind: 3 }, 1.0);
    field_set_zone("weather", "calm", #{ shape: "box", center: [0, 0, 0], size: [2, 2, 2], wind: 0 });
}
fn on_update() {
    let here = #{ x: 0.0, y: 0.0, z: 0.0 };
    let at = field_at("weather", here);
    set_field(self_entity(), "probe", "wind", at.wind);
    set_field(self_entity(), "probe", "note", `${at.zones}`);
    // Once: a fade asked for again starts again from where it stands
    if get_field(self_entity(), "probe", "fog") != 0.25 && field_remove_zone("weather", "calm", 0.5) {
        set_field(self_entity(), "probe", "fog", 0.25);
    }
}
"#;
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/weather.scene.toml", scene(WEATHER, "changer.rhai"));
    files.insert("game/scripts/changer.rhai", CHANGER);
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("game/scenes/weather.scene.toml");
        assert_eq!(number(&app, "fog"), 0.5, "the write was read back at once");
        app.tick();
        assert_eq!(errors(&app), Vec::<String>::new());
        assert_eq!(number(&app, "wind"), 0.0, "inside the calm zone, whole");
        assert_eq!(number(&app, "fog"), 0.25, "the zone was there to remove");
        let note = probe(&app, "note").unwrap();
        assert_eq!(note.as_str(), Some("#{\"calm\": 1.0}"));
        // Half a second on, the calm has faded half out over a wind of 3
        app.tick();
        app.tick();
        let wind = number(&app, "wind");
        assert!(wind > 0.0 && wind < 3.0, "fading out: {wind}");
        for _ in 0..4 {
            app.tick();
        }
        assert_eq!(number(&app, "wind"), 3.0, "the calm gone, the ramp arrived");
        let note = probe(&app, "note").unwrap();
        assert_eq!(note.as_str(), Some("#{}"));
    });
}

/// A wrong field, key, value or zone is a script error naming it, in the
/// field's own words; in a scene with no field the module is not enabled.
#[test]
fn a_wrong_call_is_an_error_naming_what_is_there() {
    const WRONG: &str = r#"
fn on_init() {
    let said = [];
    try { field_sample("rain", "drops", 0, 0, 0); } catch (e) { said.push(e); }
    try { field_sample("weather", "rain", 0, 0, 0); } catch (e) { said.push(e); }
    try { field_set("weather", #{ fog: 2 }); } catch (e) { said.push(e); }
    try { field_set_zone("weather", "pit", #{ shape: "sphere", center: [0, 0, 0], wind: 1 }); } catch (e) { said.push(e); }
    try { field_sample("weather", "wind", #{ x: 1 }); } catch (e) { said.push(e); }
    set_field(self_entity(), "probe", "note", said.reduce(|all, e| all + e + "\n", ""));
}
"#;
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/weather.scene.toml", scene(WEATHER, "wrong.rhai"));
    files.insert("game/scenes/plain.scene.toml", scene("", "plain.rhai"));
    files.insert("game/scripts/wrong.rhai", WRONG);
    files.insert(
        "game/scripts/plain.rhai",
        "fn on_init() { set_field(self_entity(), \"probe\", \"wind\", field_sample(\"weather\", \"wind\", 0, 0, 0)); }",
    );
    vfs::scoped(Arc::new(files), || {
        let app = app_in("game/scenes/weather.scene.toml");
        assert_eq!(errors(&app), Vec::<String>::new());
        let said = probe(&app, "note").unwrap();
        let said: Vec<&str> = said.as_str().unwrap().lines().collect();
        assert_eq!(
            said,
            [
                "field_sample: there is no field 'rain'; the scene declares weather",
                "field_sample: 'rain' isn't a weather key; it takes fog, wind",
                "field_set: fog = 2 must be from 0 to 1",
                "field_set_zone: zone 'pit': a sphere needs a radius",
                "field_sample: the place must be #{x, y, z}, each a number",
            ]
        );

        let app = app_in("game/scenes/plain.scene.toml");
        assert!(app.core.script.enabled_modules().is_empty());
        assert_eq!(number(&app, "wind"), -1.0, "the call was an error");
        let said = errors(&app).join("\n");
        assert!(said.contains("the field module is not enabled here"), "{said}");
    });
}

/// A table with a mistake is warned at the scene's entry and the rest used:
/// a bad zone is left out, the field kept.
#[test]
fn a_bad_zone_is_left_out_and_the_field_kept() {
    const READER: &str = r#"
fn on_init() {
    let at = field_at("weather", #{ x: 0, y: 0, z: 0 });
    set_field(self_entity(), "probe", "wind", at.wind);
    set_field(self_entity(), "probe", "note", `${at.zones}`);
}
"#;
    let fields = "[field.weather]\nkeys = { wind = 1.5 }\n\n[[field.weather.zone]]\nname = \"pit\"\nshape = \"sphere\"\ncenter = [0, 0, 0]\nwind = 2\n";
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/weather.scene.toml", scene(fields, "reader.rhai"));
    files.insert("game/scripts/reader.rhai", READER);
    vfs::scoped(Arc::new(files), || {
        let app = app_in("game/scenes/weather.scene.toml");
        assert_eq!(errors(&app), Vec::<String>::new());
        assert_eq!(number(&app, "wind"), 1.5, "the field is kept");
        let note = probe(&app, "note").unwrap();
        assert_eq!(note.as_str(), Some("#{}"), "the zone with no radius is not");
    });
}

/// What a script that samples every frame pays (#1014's "pick by
/// measuring"): 1000 `field_sample` calls in one `on_update`, against the
/// same loop calling `get_position` (a declared function of about the same
/// shape: a lookup, a map in or out), each frame's script time from the
/// player's own timing. `cargo test -p flint-player --release --lib
/// sample_cost_in_a_script -- --ignored --nocapture`.
#[test]
#[ignore]
fn sample_cost_in_a_script() {
    let looped = |body: &str| {
        format!(
            "fn on_update() {{\n    let me = self_entity();\n    let here = get_position(me);\n    \
             let sum = 0.0;\n    for i in 0..1000 {{ {body} }}\n    \
             set_field(me, \"probe\", \"wind\", sum);\n}}\n"
        )
    };
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/sample.scene.toml", scene(WEATHER, "sample.rhai"));
    files.insert("game/scenes/control.scene.toml", scene(WEATHER, "control.rhai"));
    files.insert(
        "game/scripts/sample.rhai",
        looped("sum += field_sample(\"weather\", \"wind\", here);"),
    );
    files.insert(
        "game/scripts/control.rhai",
        looped("sum += get_position(me).x;"),
    );
    files.insert("game/scenes/empty.scene.toml", scene(WEATHER, "empty.rhai"));
    files.insert("game/scripts/empty.rhai", looped("sum += 1.0;"));
    vfs::scoped(Arc::new(files), || {
        // The best frame of 200, by wall clock around the whole tick
        let best_frame = |scene: &str| {
            let mut app = app_in(scene);
            for _ in 0..10 {
                app.tick();
            }
            let mut best = std::time::Duration::MAX;
            for _ in 0..200 {
                let start = std::time::Instant::now();
                app.tick();
                best = best.min(start.elapsed());
            }
            assert_eq!(errors(&app), Vec::<String>::new());
            best.as_nanos() as f64
        };
        let empty = best_frame("game/scenes/empty.scene.toml");
        println!("a frame with the empty loop: {:.1} us", empty / 1000.0);
        for (label, scene) in [
            ("field_sample", "game/scenes/sample.scene.toml"),
            ("get_position (control)", "game/scenes/control.scene.toml"),
        ] {
            let ns = (best_frame(scene) - empty) / 1000.0;
            println!("{label}: {ns:.0} ns a call, the loop's own cost taken out");
        }
    });
}
