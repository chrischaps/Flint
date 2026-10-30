//! `PlayerExtension` wiring through a real `PlayerApp`: registration at
//! `add_extension`, and `pre_update` running in add order before the
//! scripts' `on_update` in the same frame. Headless: no window, so
//! `ExtensionCtx::gpu` is `None`.

use super::PlayerApp;
use crate::extension::{ExtensionCtx, PlayerExtension};
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
        app.script
            .rhai_engine_mut()
            .register_fn("record", move |a: i64, b: i64| {
                seen.lock().unwrap().push((a, b))
            });
    }
    let dir = std::env::temp_dir().join(format!("flint_ext_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reader.rhai");
    std::fs::write(&path, "fn on_update() { record(a_frame(), b_frame()); }").unwrap();
    let id = app.world.spawn("reader").unwrap();
    app.script.load_script_for_entity(id, &path);
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
fn attach_recording_script(app: &mut PlayerApp, test: &str, src: &str) -> Log {
    let log = Log::default();
    {
        let log = log.clone();
        app.script
            .rhai_engine_mut()
            .register_fn("record", move |s: &str| log.lock().unwrap().push(s.into()));
    }
    let dir = std::env::temp_dir().join(format!("flint_ext_{test}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("listener.rhai");
    std::fs::write(&path, src).unwrap();
    let id = app.world.spawn("listener").unwrap();
    app.script.load_script_for_entity(id, &path);
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
    let id = app.world.spawn("plot").unwrap();
    let mut model = toml::Table::new();
    model.insert("asset".into(), "placeholder".into());
    model.insert("note".into(), "kept".into());
    app.world.set_component(id, "model", model.into()).unwrap();
    app.add_extension(Box::new(Attacher));
    app.tick();

    let model = app.world.get_component(id, "model").unwrap();
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
    assert!(app.ui_fonts.contains("game"), "scripts can draw in it");

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
    assert!(app.ui_fonts.contains("game"));

    // egui has the family on the next pass, so laying text out in it from
    // `ui` doesn't panic.
    let _ = app.egui_ctx.run(egui::RawInput::default(), |ctx| {
        ctx.fonts(|f| {
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
    assert!(!app.persist_debounce.is_pending());

    app.tick();

    // Dirty, and waiting out the debounce: no script wrote anything, so
    // the extension's write alone is what the next save will carry.
    assert!(app.persistent_store.is_dirty());
    assert!(app.persist_debounce.is_pending());
    assert_eq!(
        app.persistent_store
            .get("greeting")
            .and_then(|v| v.as_str()),
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
    app.clock.frame_step = Some(0.25);

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
    let _ = ctx.run(input, |ctx| {
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
