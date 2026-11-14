//! `PlayerExtension` wiring through a real `PlayerApp`: registration at
//! `add_extension`, and `pre_update` running in add order before the
//! scripts' `on_update` in the same frame. Headless: no window, so
//! `ExtensionCtx::gpu` is `None`.

use super::PlayerApp;
use crate::extension::{ExtensionCtx, PlayerExtension};
use flint_debug_ui::run_headless_frame;
use flint_ecs::FlintWorld;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

/// Registers `<name>_frame()`, which returns how many times its
/// `pre_update` has run, and logs every hook call.
struct Counter {
    name: &'static str,
    frames: Arc<AtomicI64>,
    log: Log,
}

impl Counter {
    fn new(name: &'static str, log: &Log) -> Self {
        Self {
            name,
            frames: Arc::default(),
            log: log.clone(),
        }
    }
}

impl PlayerExtension for Counter {
    fn register_script_api(&mut self, engine: &mut rhai::Engine) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} register", self.name));
        let frames = self.frames.clone();
        engine.register_fn(format!("{}_frame", self.name), move || {
            frames.load(Ordering::SeqCst)
        });
    }

    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        assert!(ctx.gpu.is_none(), "headless test has no renderer");
        self.frames.fetch_add(1, Ordering::SeqCst);
        self.log
            .lock()
            .unwrap()
            .push(format!("{} pre_update", self.name));
    }
}

fn headless_app() -> PlayerApp {
    PlayerApp::new(
        FlintWorld::new(),
        "extension_test.scene.toml".into(),
        false,
        None,
        None,
    )
}

#[test]
fn add_extension_registers_script_api_in_order() {
    let log = Log::default();
    let mut app = headless_app();
    app.add_extension(Box::new(Counter::new("a", &log)));
    app.add_extension(Box::new(Counter::new("b", &log)));
    assert_eq!(*log.lock().unwrap(), ["a register", "b register"]);

    let n: i64 = app
        .core
        .script
        .rhai_engine_mut()
        .eval("a_frame() + b_frame()")
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn pre_update_runs_in_add_order_before_on_update() {
    let log = Log::default();
    let mut app = headless_app();
    app.add_extension(Box::new(Counter::new("a", &log)));
    app.add_extension(Box::new(Counter::new("b", &log)));

    // A script that records what the extensions' counters read during its
    // on_update. If pre_update runs first, frame N sees N.
    let seen = Arc::new(Mutex::new(Vec::<(i64, i64)>::new()));
    {
        let seen = seen.clone();
        app.core
            .script
            .rhai_engine_mut()
            .register_fn("record", move |a: i64, b: i64| {
                seen.lock().unwrap().push((a, b))
            });
    }
    let dir = std::env::temp_dir().join(format!("flint_ext_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reader.rhai");
    std::fs::write(&path, "fn on_update() { record(a_frame(), b_frame()); }").unwrap();
    let id = app.core.world.spawn("reader").unwrap();
    app.core.script.load_script_for_entity(id, &path);
    let _ = std::fs::remove_dir_all(&dir);

    app.tick();
    app.tick();

    assert_eq!(*seen.lock().unwrap(), [(1, 1), (2, 2)]);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "a register",
            "b register",
            "a pre_update",
            "b pre_update",
            "a pre_update",
            "b pre_update"
        ]
    );
}

/// Declares `kart_speed(id)` through the builder (ADR 0116).
struct Declarer;

impl PlayerExtension for Declarer {
    fn declare_script_api(&mut self, api: &mut flint_script::api::ScriptApi) {
        use flint_script::api::builder::kind::{Entity, Num};
        let mut m = api.module("kart");
        m.func("kart_speed", "The kart's speed times a factor.")
            .arg("kart", Entity)
            .arg("factor", Num)
            .returns(Num, "m/s; 0 for no kart")
            .run(|call, (kart, factor)| {
                let world = call.world()?;
                Ok(world.get_name(kart).map_or(0.0, |n| n.len() as f64) * factor)
            });
    }
}

#[test]
fn an_extension_declares_functions_that_are_listed_and_called() {
    let mut app = headless_app();
    app.add_extension(Box::new(Declarer));
    let listing = app.core.script.api_listing();
    let entry = listing
        .declared
        .iter()
        .find(|e| e.name == "kart_speed")
        .expect("listed");
    assert_eq!(entry.module, "kart");
    assert_eq!(
        entry.signature(),
        "kart_speed(kart: entity, factor: num) -> num"
    );
    assert!(listing.declared.iter().any(|e| e.name == "set_position"));

    let log = attach_recording_script(
        &mut app,
        "declared",
        r#"fn on_update() { record(`${kart_speed(get_entity("listener"), 2)}`); }"#,
    );
    app.tick();
    assert_eq!(*log.lock().unwrap(), ["16.0"], "an int for a num, too");
}

/// Fires `pinged` with the frame count in every `pre_update`.
struct Pinger {
    frame: i64,
}

impl PlayerExtension for Pinger {
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        self.frame += 1;
        let mut data = toml::Table::new();
        data.insert("frame".into(), self.frame.into());
        ctx.fire_event("pinged", data);
    }
}

/// Registers `record(s)` on the app's script engine and attaches `src` to a
/// new entity. Returns the log `record` appends to.
pub(super) fn attach_recording_script(app: &mut PlayerApp, test: &str, src: &str) -> Log {
    let log = Log::default();
    {
        let log = log.clone();
        app.core
            .script
            .rhai_engine_mut()
            .register_fn("record", move |s: &str| log.lock().unwrap().push(s.into()));
    }
    let dir = std::env::temp_dir().join(format!("flint_ext_{test}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("listener.rhai");
    std::fs::write(&path, src).unwrap();
    let id = app.core.world.spawn("listener").unwrap();
    app.core.script.load_script_for_entity(id, &path);
    let _ = std::fs::remove_dir_all(&dir);
    log
}

#[test]
fn extension_events_reach_on_event_before_on_update_same_frame() {
    let mut app = headless_app();
    app.add_extension(Box::new(Pinger { frame: 0 }));
    let log = attach_recording_script(
        &mut app,
        "fire",
        r#"
fn on_event(name, data) { record(`${name} ${data.frame}`); }
fn on_update() { record("update"); }
"#,
    );

    app.tick();
    app.tick();

    assert_eq!(
        *log.lock().unwrap(),
        ["pinged 1", "update", "pinged 2", "update"]
    );
}

#[test]
fn script_fired_events_reach_on_event_next_frame() {
    let mut app = headless_app();
    let log = attach_recording_script(
        &mut app,
        "script_fire",
        r#"
fn on_update() {
    record("update");
    fire_event_data("counted", #{ by: "script" });
}
fn on_event(name, data) { record(`${name} by ${data.by}`); }
"#,
    );

    app.tick();
    app.tick();

    // Commands drain after on_update, so the event lands in the next frame.
    assert_eq!(
        *log.lock().unwrap(),
        ["update", "counted by script", "update"]
    );
}

/// Attaches the runtime mesh `ground` to an existing entity in `pre_update`.
struct Attacher;

impl PlayerExtension for Attacher {
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        let id = ctx.world.get_id("plot").unwrap();
        ctx.attach_mesh(id, "ground").unwrap();
    }
}

#[test]
fn attach_mesh_names_a_runtime_model_and_keeps_other_fields() {
    let mut app = headless_app();
    let id = app.core.world.spawn("plot").unwrap();
    let mut model = toml::Table::new();
    model.insert("asset".into(), "placeholder".into());
    model.insert("note".into(), "kept".into());
    app.core
        .world
        .set_component(id, "model", model.into())
        .unwrap();
    app.add_extension(Box::new(Attacher));
    app.tick();

    let model = app.core.world.get_component(id, "model").unwrap();
    assert_eq!(model.get("asset").unwrap().as_str(), Some("ground"));
    assert_eq!(model.get("runtime").unwrap().as_bool(), Some(true));
    assert_eq!(model.get("note").unwrap().as_str(), Some("kept"));
    assert!(flint_render::model_loader::is_runtime_model(model));
}

/// Adds a named family and makes it egui's default, as a game with its own
/// embedded font would.
struct Faces {
    calls: Arc<AtomicI64>,
}

impl PlayerExtension for Faces {
    fn fonts(&mut self, defs: &mut egui::FontDefinitions) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let face = defs.families[&egui::FontFamily::Proportional][0].clone();
        defs.families
            .insert(egui::FontFamily::Name("game".into()), vec![face]);
    }
}

#[test]
fn the_fonts_hook_runs_on_every_install_and_its_families_reach_scripts() {
    let calls = Arc::new(AtomicI64::new(0));
    let mut app = headless_app();
    app.add_extension(Box::new(Faces {
        calls: calls.clone(),
    }));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no fonts before initialize"
    );

    app.refresh_project_fonts();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(app.hud.fonts.contains("game"), "scripts can draw in it");

    // Same project root: nothing to reload.
    app.refresh_project_fonts();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // An extension added late gets its hook, and the earlier one keeps its
    // family through the reload.
    let late = Arc::new(AtomicI64::new(0));
    app.add_extension(Box::new(Faces {
        calls: late.clone(),
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(late.load(Ordering::SeqCst), 1);
    assert!(app.hud.fonts.contains("game"));

    // egui has the family on the next pass, so laying text out in it from
    // `ui` doesn't panic.
    let _ = run_headless_frame(&app.hud.egui_ctx, egui::RawInput::default(), |ui| {
        let ctx = &ui.ctx().clone();
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                "Capitol".into(),
                egui::FontId::new(14.0, egui::FontFamily::Name("game".into())),
                egui::Color32::WHITE,
            )
        });
    });
}

/// Counts its frames into the shared store as `visits`, and records what
/// the scripts last left under `from_script`.
struct Keeper {
    seen: Log,
}

impl PlayerExtension for Keeper {
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        let visits = ctx
            .persist
            .get("visits")
            .and_then(|v| v.as_integer())
            .unwrap_or(0);
        ctx.persist.set("visits", (visits + 1).into());
        let from_script = ctx
            .persist
            .get("from_script")
            .and_then(|v| v.as_str())
            .unwrap_or("nothing");
        self.seen.lock().unwrap().push(from_script.into());
    }
}

#[test]
fn extensions_and_scripts_share_the_persistent_store() {
    let mut app = headless_app();
    let seen = Log::default();
    app.add_extension(Box::new(Keeper { seen: seen.clone() }));
    let log = attach_recording_script(
        &mut app,
        "persist",
        r#"
fn on_update() {
    let visits = persist_get("visits");
    record(`visits ${visits}`);
    persist_set("from_script", `after ${visits}`);
}
"#,
    );

    app.tick();
    app.tick();

    // The script reads the extension's write in the same frame, and the
    // extension reads the script's in the next.
    assert_eq!(*log.lock().unwrap(), ["visits 1", "visits 2"]);
    assert_eq!(*seen.lock().unwrap(), ["nothing", "after 1"]);
}

/// Sets `greeting` once, in its first `pre_update`.
struct Greeter {
    done: bool,
}

impl PlayerExtension for Greeter {
    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        if !std::mem::replace(&mut self.done, true) {
            ctx.persist.set("greeting", "hello".into());
        }
    }
}

#[test]
fn an_extension_write_starts_the_save_debounce() {
    let mut app = headless_app();
    app.add_extension(Box::new(Greeter { done: false }));
    assert!(!app.saves.debounce.is_pending());

    app.tick();

    // Dirty, and waiting out the debounce: no script wrote anything, so
    // the extension's write alone is what the next save will carry.
    assert!(app.core.store.is_dirty());
    assert!(app.saves.debounce.is_pending());
    assert_eq!(
        app.core.store.get("greeting").and_then(|v| v.as_str()),
        Some("hello")
    );
}

/// Logs every key `on_key` sees, and takes the ones that stand for `?`.
struct QuestionTaker {
    log: Log,
}

impl PlayerExtension for QuestionTaker {
    fn on_key(&mut self, key: &crate::extension::KeyInput) -> bool {
        self.log.lock().unwrap().push(format!(
            "{:?} {:?} {:?} shift={} scripted={}",
            key.key,
            key.state,
            key.character,
            key.modifiers.shift_key(),
            key.scripted
        ));
        key.character == Some("?")
    }
}

#[test]
fn scripted_keys_reach_on_key_and_one_taken_stays_out_of_the_game() {
    use super::input_script::InputScript;
    use winit::keyboard::KeyCode;

    let mut app = headless_app();
    let log = Log::default();
    app.add_extension(Box::new(QuestionTaker { log: log.clone() }));
    app.capture.input_script = Some(
        InputScript::parse("0 down ShiftLeft\n0 tap Slash\n0.5 up ShiftLeft\n0.5 tap KeyW\n")
            .unwrap(),
    );
    app.core.clock.frame_step = Some(0.25);

    app.tick();
    assert!(app.input.is_key_down(KeyCode::ShiftLeft));
    assert!(
        !app.input.is_key_down(KeyCode::Slash),
        "the extension took it"
    );
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));

    assert_eq!(
        *log.lock().unwrap(),
        [
            "ShiftLeft Pressed None shift=true scripted=true",
            "Slash Pressed Some(\"?\") shift=true scripted=true",
            "Slash Released Some(\"?\") shift=true scripted=true",
            "ShiftLeft Released None shift=false scripted=true",
            "KeyW Pressed Some(\"w\") shift=false scripted=true",
        ]
    );
}

/// Names two debug panels and logs each draw (ADR 0107).
#[cfg(feature = "debug-hud")]
struct PanelGame {
    log: Log,
    frame: u32,
}

#[cfg(feature = "debug-hud")]
impl PlayerExtension for PanelGame {
    fn ui(&mut self, _ctx: &egui::Context) {
        self.frame += 1;
    }
    fn debug_panels(&self) -> Vec<String> {
        vec!["Net".into(), "Town".into()]
    }
    fn debug_panel_ui(&mut self, name: &str, ui: &mut egui::Ui) {
        ui.label(name);
        self.log
            .lock()
            .unwrap()
            .push(format!("{name} {}", self.frame));
    }
}

#[cfg(feature = "debug-hud")]
#[test]
fn a_game_names_debug_panels_that_f3_opens_and_it_draws() {
    let log = Log::default();
    let mut app = headless_app();
    app.add_extension(Box::new(PanelGame {
        log: log.clone(),
        frame: 0,
    }));
    let names: Vec<&str> = app.debug_panels.iter().map(|p| p.name()).collect();
    assert_eq!(names, ["Net", "Town"], "added with the extension");
    assert!(app.debug_panels.iter().all(|p| !p.is_open()), "closed");

    app.handle_debug_key(winit::keyboard::KeyCode::F3);
    assert!(
        app.debug_panels.iter().all(|p| p.is_open()),
        "F3 opens them"
    );

    // One egui pass, as the frame runs it: the game's ui, then the panels.
    let ctx = egui::Context::default();
    let mut extensions = std::mem::take(&mut app.extensions);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let _ = run_headless_frame(&ctx, input, |ui| {
        let ctx = &ui.ctx().clone();
        for ext in extensions.iter_mut() {
            ext.ui(ctx);
        }
        super::debug_panels::show_debug_panels(ctx, &mut app.debug_panels, &mut |panel, ui| {
            super::debug_panels::draw_panel(panel, &mut extensions, ui)
        });
    });
    app.extensions = extensions;
    assert_eq!(
        *log.lock().unwrap(),
        ["Net 1", "Town 1"],
        "drawn by the extension, after its ui"
    );

    // Building the panels again (a scene load) keeps them, still open.
    app.debug_panels[1].toggle();
    app.create_extension_debug_panels();
    let open: Vec<(&str, bool)> = app
        .debug_panels
        .iter()
        .map(|p| (p.name(), p.is_open()))
        .collect();
    assert_eq!(open, [("Net", true), ("Town", false)]);
}

/// A game's own scene-level setting, end to end (#686): its schema layer
/// declares `[table.weather]`, a scene writes `[weather]`, and an extension
/// reads it from `ExtensionCtx`. A typo in the table, or in its name, is a
/// diagnostic of the load.
mod scene_tables {
    use super::*;
    use flint_core::diagnostic::codes;
    use flint_schema::SchemaRegistry;

    /// What the game's extension keeps of the scene's weather.
    #[derive(Debug, Default, Clone, PartialEq, serde::Deserialize)]
    #[serde(default)]
    struct Weather {
        rain: f32,
        wind: [f32; 3],
        kind: String,
    }

    #[derive(Default)]
    struct WeatherReader {
        seen: Arc<Mutex<Vec<Option<Weather>>>>,
        raw_rain: Arc<Mutex<Option<f64>>>,
    }

    impl PlayerExtension for WeatherReader {
        fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
            let weather = ctx
                .scene_table_as::<Weather>("weather")
                .map(|read| read.expect("[weather] fits the game's type"));
            self.seen.lock().unwrap().push(weather);
            *self.raw_rain.lock().unwrap() = ctx
                .scene_table("weather")
                .and_then(|weather| weather.get("rain")?.as_float());
            assert!(ctx.scene_table("nothing_here").is_none());
        }
    }

    const WEATHER_SCHEMA: &str = r#"
[table.weather]
description = "What the sky is doing in this scene"

[table.weather.fields]
rain = { type = "f32", min = 0.0, max = 1.0, default = 0.0, description = "How hard it rains" }
wind = { type = "vec3", description = "Metres a second, in world space" }
kind = { type = "enum", values = ["clear", "storm"], default = "clear" }
"#;

    const STORM: &str = r#"
[scene]
name = "Storm"

[weather]
rain = 0.7
wind = [3.0, 0.0, -1.0]
kind = "storm"

[entities.rock.transform]
position = [0, 0, 0]
"#;

    /// A game project in a temp folder: a manifest naming its schema layer,
    /// that layer's `scene/weather.toml`, and `scene` as its one scene.
    struct Game(std::path::PathBuf);

    impl Game {
        fn new(test: &str, scene: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("flint_scene_table_{test}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("schemas/scene")).unwrap();
            std::fs::create_dir_all(dir.join("schemas/components")).unwrap();
            std::fs::create_dir_all(dir.join("scenes")).unwrap();
            std::fs::write(
                dir.join("flint.toml"),
                "[project]\nschemas = [\"schemas\"]\n",
            )
            .unwrap();
            std::fs::write(dir.join("schemas/scene/weather.toml"), WEATHER_SCHEMA).unwrap();
            std::fs::write(
                dir.join("schemas/components/transform.toml"),
                "[component.transform.fields]\nposition = \"vec3\"\n",
            )
            .unwrap();
            std::fs::write(dir.join("scenes/storm.scene.toml"), scene).unwrap();
            Self(dir)
        }

        fn scene(&self) -> std::path::PathBuf {
            self.0.join("scenes/storm.scene.toml")
        }

        /// The scene loaded as the player loads it: schemas by the one
        /// resolver, from the project's manifest.
        fn load(
            &self,
        ) -> (
            FlintWorld,
            flint_scene::SceneFile,
            Vec<flint_core::diagnostic::Diagnostic>,
        ) {
            let none: &[&str] = &[];
            let (registry, dirs) = SchemaRegistry::resolve(Some(&self.scene()), none).unwrap();
            assert_eq!(dirs.source.name(), "manifest");
            assert!(registry.get_table("weather").is_some());
            flint_scene::load_scene_diagnosed(self.scene(), &registry).unwrap()
        }
    }

    impl Drop for Game {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_games_table_is_declared_read_by_an_extension_and_checked() {
        let game = Game::new("read", STORM);
        let (world, scene_file, found) = game.load();
        assert_eq!(found, [], "a scene that fits its schemas says nothing");

        let mut app = PlayerApp::new(
            world,
            game.scene().to_string_lossy().into_owned(),
            false,
            None,
            None,
        );
        app.apply_scene_settings(&scene_file);
        let reader = WeatherReader::default();
        let (seen, raw_rain) = (reader.seen.clone(), reader.raw_rain.clone());
        app.add_extension(Box::new(reader));
        app.tick();

        assert_eq!(
            *seen.lock().unwrap(),
            [Some(Weather {
                rain: 0.7,
                wind: [3.0, 0.0, -1.0],
                kind: "storm".into(),
            })]
        );
        assert_eq!(*raw_rain.lock().unwrap(), Some(0.7));

        // A scene with no [weather] reads as none, and the next scene's
        // tables replace this one's
        app.scene.tables = flint_scene::SceneFile::new("calm").scene_tables();
        app.tick();
        assert_eq!(seen.lock().unwrap()[1], None);
    }

    #[test]
    fn a_typo_in_a_games_table_is_diagnosed() {
        let game = Game::new(
            "typo",
            &STORM
                .replace("rain = 0.7", "rian = 0.7")
                .replace("kind = \"storm\"", "kind = \"strom\""),
        );
        let (_, _, found) = game.load();
        let said: Vec<String> = found
            .iter()
            .map(|d| match &d.hint {
                Some(hint) => format!("{d} ({hint})"),
                None => d.to_string(),
            })
            .collect();
        assert_eq!(
            said,
            [
                "error[bad-enum-value] [weather] kind: \"strom\" is not a value of `kind` \
                 (did you mean `storm`? allowed values: clear, storm)",
                "warning[unknown-field] [weather] rian: `weather` has no field `rian` (did you \
                 mean `rain`?)",
            ]
        );

        // ...and so is a typo in its name
        let game = Game::new("name", &STORM.replace("[weather]", "[wether]"));
        let (_, scene_file, found) = game.load();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, codes::UNKNOWN_TABLE);
        assert_eq!(found[0].hint.as_deref(), Some("did you mean `weather`?"));
        // The table is kept all the same: the load drops nothing
        assert!(scene_file.tables.contains_key("wether"));
    }
}

/// A game's Rust styles (#419): `PlayerExtension::styles` registers them at
/// `add_extension`, so a scene's `[post_process.styles.<name>]` and a
/// `--style` word read after it find the style. Headless, so nothing builds
/// the passes; `create` is never called.
mod game_styles {
    use super::*;
    use flint_render::stylize::{
        self, StyleDescriptor, StyleDevice, StyleParam, StylePass, StyleStage,
    };

    fn never_built(_: &StyleDevice) -> Box<dyn StylePass> {
        unreachable!("a headless test builds no style")
    }

    static SEPIA: StyleDescriptor = StyleDescriptor::new(
        "utest_ext_sepia",
        "Sepia (test)",
        StyleStage::Display,
        never_built,
    )
    .params(&[StyleParam::new("warmth", 0.5, 0.0..=1.0)]);

    /// Takes a built-in's name: refused, and the extension's other style
    /// still registers.
    static TAKEN: StyleDescriptor =
        StyleDescriptor::new("invert", "Not invert", StyleStage::Display, never_built);

    struct Sepia;

    impl PlayerExtension for Sepia {
        fn styles(&self) -> Vec<&'static StyleDescriptor> {
            vec![&TAKEN, &SEPIA]
        }
    }

    #[test]
    fn an_extensions_style_is_read_from_the_scene_and_from_style_words() {
        let mut app = headless_app();
        // The scene is read before the extension is added, as the example
        // games do: its tables are kept as written and read against the
        // registry only when the config is made
        let scene = flint_scene::SceneFile::parse(
            "[scene]\nname = \"t\"\n\
             [post_process.styles.utest_ext_sepia]\nwarmth = 0.8\n",
        )
        .unwrap();
        app.apply_scene_settings(&scene);
        app.add_extension(Box::new(Sepia));

        let sepia = stylize::find("utest_ext_sepia").expect("registered by add_extension");
        assert!(std::ptr::eq(sepia, &SEPIA));
        assert!(stylize::is_game_style("utest_ext_sepia"));
        // The built-in kept its place
        assert_eq!(stylize::find("invert").unwrap().label, "Invert");
        assert!(!stylize::is_game_style("invert"));

        let config = app.scene_post_config();
        assert!(config.styles.is_enabled("utest_ext_sepia"));
        assert_eq!(config.styles.param("utest_ext_sepia", "warmth"), Some(0.8));

        // A `--style` word over it
        app.launch.style_specs = vec!["utest_ext_sepia:warmth=0.25".into()];
        let config = app.scene_post_config();
        assert_eq!(config.styles.param("utest_ext_sepia", "warmth"), Some(0.25));

        // A second player in the process, the same extension: no error, one
        // entry
        let mut again = headless_app();
        again.add_extension(Box::new(Sepia));
        let entries = stylize::registry()
            .iter()
            .filter(|d| d.name == "utest_ext_sepia")
            .count();
        assert_eq!(entries, 1);
    }
}
