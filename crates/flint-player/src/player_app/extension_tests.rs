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
