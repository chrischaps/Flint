//! A game's extension with the engine's own hooks (ADR 0117 decision 5,
//! #754): `Laps`, a lap counter of the kind a racing game would write. It
//! declares a script module (`laps()` reads what the extension set at
//! `Stage::PreScripts`, `mark_lap(n)` queues a command it takes), hears
//! every stage after the scene's subsystems, and hears its scenes come and
//! go. Headless: no window.

use super::scene_entry::SceneEntry;
use super::subsystem::{Stage, StageCtx, Subsystem};
use super::PlayerApp;
use crate::extension::{ExtensionCtx, PlayerExtension};
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_script::api::builder::kind::Int;
use flint_script::api::Module;
use flint_script::context::ScriptCommand;
use flint_script::{ModuleCommand, ScriptModule};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

const A: &str = "game/scenes/a.scene.toml";

/// A scene whose script reads the module every frame and marks a lap.
const SCENE_A: &str = r#"
[scene]
name = "A"

[entities.counter.script]
source = "lap.rhai"
"#;

const SCENE_B: &str = r#"
[scene]
name = "B"

[entities.counter.script]
source = "lap.rhai"
"#;

const LAP_SCRIPT: &str = r#"
fn on_update() {
    record(`laps ${laps()}`);
    mark_lap(1);
}
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(A, SCENE_A);
    files.insert("game/scenes/b.scene.toml", SCENE_B);
    files.insert("game/scenes/scripts/lap.rhai", LAP_SCRIPT);
    files
}

/// What `laps()` reads: the extension's count when the frame's scripts
/// began.
#[derive(Debug, Default, Clone, Copy)]
struct LapState {
    laps: i64,
}

/// `mark_lap(n)`'s command, which only `Laps` knows.
#[derive(Debug)]
struct MarkLap(i64);

/// The game's `laps` module.
struct LapModule;

static LAPS: LapModule = LapModule;

/// What `Laps::script_modules` hands over: a const, so the slice is
/// `'static` (an array literal in the method would be a temporary).
const MODULES: &[&dyn ScriptModule] = &[&LAPS];

impl ScriptModule for LapModule {
    fn name(&self) -> &'static str {
        "laps"
    }

    fn enabled_when(&self) -> &'static str {
        "the game's lap counter is installed"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func("laps", "The laps counted when this frame's scripts began.")
            .returns(Int, "laps so far")
            .run(|call, ()| call.ctx().module_state.get_mut::<LapState>().laps);
        m.func(
            "mark_lap",
            "Count `n` more laps, at the frame's next command drain.",
        )
        .arg("n", Int)
        .run(|call, (n,)| {
            let function = call.name();
            call.ctx()
                .commands
                .push(ScriptCommand::Module(ModuleCommand::new(
                    function,
                    MarkLap(n),
                )));
        });
    }
}

/// The test extension.
struct Laps {
    laps: i64,
    log: Log,
}

impl Laps {
    fn say(&self, what: String) {
        self.log.lock().unwrap().push(format!("ext {what}"));
    }
}

impl PlayerExtension for Laps {
    fn script_modules(&self) -> &[&'static dyn ScriptModule] {
        MODULES
    }

    fn scene_loaded(&mut self, ctx: &mut ExtensionCtx) {
        let counters = ctx.world.entities_with_component("script").len();
        self.say(format!("loaded with {counters} script"));
    }

    fn scene_unloading(&mut self, _ctx: &mut ExtensionCtx) {
        self.say("unloading".into());
    }

    fn update(&mut self, stage: Stage, ctx: &mut ExtensionCtx) {
        self.say(format!("{stage:?}"));
        if stage == Stage::PreScripts {
            ctx.set_module_state(LapState { laps: self.laps });
        }
    }

    fn script_command(&mut self, command: &ScriptCommand, _ctx: &mut ExtensionCtx) -> bool {
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(MarkLap(n)) = command.payload::<MarkLap>() else {
            return false;
        };
        self.laps += n;
        self.say(format!("took mark_lap -> {}", self.laps));
        true
    }
}

/// An engine subsystem that says each stage, to see who goes first.
struct Probe(Log);

impl Subsystem for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn update(&mut self, stage: Stage, _ctx: &mut StageCtx) {
        self.0.lock().unwrap().push(format!("sub {stage:?}"));
    }
}

/// The player in scene A with `Laps` installed before it starts, as a game
/// binary does, and `record(text)` for its scripts.
fn started_with_laps(log: &Log) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(A, &registry).expect("scene A loads");
    let mut app = PlayerApp::new(world, A.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    {
        let log = log.clone();
        app.core
            .script
            .rhai_engine_mut()
            .register_fn("record", move |s: &str| {
                log.lock().unwrap().push(format!("script {s}"))
            });
    }
    app.add_extension(Box::new(Laps {
        laps: 0,
        log: log.clone(),
    }));
    app.core.clock.frame_step = Some(0.25);
    app.enter_scene(SceneEntry::Start).expect("scene A starts");
    app
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().unwrap())
}

#[test]
fn a_game_module_reads_the_extension_and_its_command_comes_back_to_it() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_with_laps(&log);
        assert!(app.core.script.enabled_modules().contains(&"laps"));
        take(&log);

        app.tick();
        app.tick();
        app.tick();

        let script_and_takes: Vec<String> = take(&log)
            .into_iter()
            .filter(|l| l.starts_with("script") || l.contains("took"))
            .collect();
        assert_eq!(
            script_and_takes,
            [
                "script laps 0",
                "ext took mark_lap -> 1",
                "script laps 1",
                "ext took mark_lap -> 2",
                "script laps 2",
                "ext took mark_lap -> 3",
            ]
        );
    });
}

#[test]
fn a_game_extension_hears_every_stage_after_the_subsystems() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_with_laps(&log);
        app.core.subsystems.push(Box::new(Probe(log.clone())));
        take(&log);

        app.tick();

        let heard: Vec<String> = take(&log)
            .into_iter()
            .filter(|l| !l.starts_with("script") && !l.contains("took"))
            .collect();
        let mut want = Vec::new();
        for stage in Stage::UPDATES {
            want.push(format!("sub {stage:?}"));
            want.push(format!("ext {stage:?}"));
        }
        assert_eq!(heard, want);
    });
}

#[test]
fn a_transition_keeps_the_game_module_and_says_the_scene_came_and_went() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_with_laps(&log);
        assert_eq!(take(&log), ["ext loaded with 1 script"]);
        app.tick();
        take(&log);

        // The subsystems' modules are set again at every scene's entry;
        // the game's stay enabled through it (#754).
        assert!(app.execute_scene_transition("b.scene.toml"));
        assert!(app.core.script.enabled_modules().contains(&"laps"));
        let scene_hooks: Vec<String> = take(&log)
            .into_iter()
            .filter(|l| l.contains("loaded") || l.contains("unloading"))
            .collect();
        assert_eq!(scene_hooks, ["ext unloading", "ext loaded with 1 script"]);

        app.tick();
        assert!(
            take(&log).contains(&"script laps 1".to_string()),
            "scene B's script reads the count scene A's left"
        );
    });
}
