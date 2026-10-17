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
