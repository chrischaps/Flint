//! The subsystem seam (ADR 0117, #724 step 3): a toy subsystem hears every
//! stage of a frame in order, a scene come up and go, and is torn down last
//! added first; the expression feed is one only when the run has a feed, and
//! its states reach scripts at `Stage::Input` as before. Headless: no window.

use super::expression_feed::{ExpressionFeed, ExpressionTimeline};
use super::scene_entry::SceneEntry;
use super::subsystem::{ExitReport, Lasts, SceneCtx, Stage, StageCtx, Subsystem, Subsystems};
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::{Arc, Mutex};
use winit::keyboard::KeyCode;

const A: &str = "game/scenes/a.scene.toml";

const SCENE_A: &str = r#"
[scene]
name = "A"
"#;

const SCENE_B: &str = r#"
[scene]
name = "B"
"#;

const WOOZY: &str = r#"
[scene]
name = "Woozy"

[wooze]
step = 0.5

[entities.bench.transform]
position = [0.0, 0.0, 0.0]
[entities.bench.shallow_wooze]
amplitude = 0.05
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(A, SCENE_A);
    files.insert("game/scenes/b.scene.toml", SCENE_B);
    files.insert("game/scenes/woozy.scene.toml", WOOZY);
    files
}

fn started_in_a(feed: Option<ExpressionFeed>) -> PlayerApp {
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
    app.launch.expression_feed = feed;
    app.clock.frame_step = Some(0.25);
    app.enter_scene(SceneEntry::Start).expect("scene A starts");
    app
}

type Log = Arc<Mutex<Vec<String>>>;

/// Says what it heard. `scene_only`: it goes when its scene unloads.
struct Toy {
    name: &'static str,
    log: Log,
    scene_only: bool,
}

impl Toy {
    fn boxed(name: &'static str, log: &Log, scene_only: bool) -> Box<Self> {
        Box::new(Self {
            name,
            log: log.clone(),
            scene_only,
        })
    }
    fn say(&self, what: String) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} {what}", self.name));
    }
}

impl Subsystem for Toy {
    fn name(&self) -> &'static str {
        self.name
    }
    fn lasts(&self) -> Lasts {
        if self.scene_only {
            Lasts::Scene
        } else {
            Lasts::Run
        }
    }
    fn scene_loaded(&mut self, ctx: &mut SceneCtx) {
        let file = std::path::Path::new(ctx.scene_path).file_name().unwrap();
        self.say(format!("loaded {}", file.to_string_lossy()));
    }
    fn scene_unloading(&mut self, _ctx: &mut SceneCtx) {
        self.say("unloading".into());
    }
    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        self.say(format!("{stage:?} policy={}", ctx.policy.is_some()));
    }
    fn on_key(&mut self, key: KeyCode) -> bool {
        self.say(format!("key {key:?}"));
        key == KeyCode::F8
    }
    fn exit_report(&self, report: &mut ExitReport) {
        report.line(format!("[{}] done", self.name));
    }
}

impl Drop for Toy {
    fn drop(&mut self) {
        self.say("dropped".into());
    }
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().unwrap())
}

#[test]
fn a_scene_with_no_subsystem_has_none() {
    vfs::scoped(Arc::new(game()), || {
        let app = started_in_a(None);
        assert!(app.subsystems.is_empty(), "{:?}", app.subsystems.names());
    });
}

#[test]
fn a_toy_hears_every_stage_in_order() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_in_a(None);
        app.subsystems.push(Toy::boxed("toy", &log, false));
        app.tick();
        let heard = take(&log);
        let stages: Vec<String> = Stage::UPDATES
            .iter()
            .map(|s| format!("toy {s:?} policy={}", *s != Stage::Input))
            .collect();
        assert_eq!(heard, stages);
    });
}

#[test]
fn a_transition_unloads_last_added_first_and_drops_the_scenes_own() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_in_a(None);
        app.subsystems.push(Toy::boxed("run", &log, false));
        app.subsystems.push(Toy::boxed("scene", &log, true));

        assert!(app.execute_scene_transition("b.scene.toml"));
        assert_eq!(
            take(&log),
            [
                "scene unloading",
                "run unloading",
                "scene dropped",
                "run loaded b.scene.toml"
            ]
            .map(String::from),
            "the scene's toy goes with A, before B comes up"
        );
        assert_eq!(app.subsystems.names(), ["run"]);
    });
}

#[test]
fn subsystems_take_debug_keys_and_add_to_the_exit_report() {
    let log = Log::default();
    let mut list = Subsystems::default();
    list.push(Toy::boxed("a", &log, false));
    list.push(Toy::boxed("b", &log, false));
    assert!(list.on_key(KeyCode::F8), "a takes F8");
    assert_eq!(take(&log), ["a key F8"], "b never sees a key a took");
    assert!(!list.on_key(KeyCode::F2));
    assert_eq!(take(&log), ["a key F2", "b key F2"]);

    let mut report = ExitReport::default();
    list.exit_report(&mut report);
    assert_eq!(report.lines, ["[a] done", "[b] done"]);

    drop(list);
    assert_eq!(take(&log), ["b dropped", "a dropped"], "last added first");
}

#[test]
fn the_expression_feed_is_a_subsystem_only_when_the_run_has_one() {
    vfs::scoped(Arc::new(game()), || {
        let timeline = ExpressionTimeline::parse(
            r#"{"events": [{"state": "hello", "t": 0.0}, {"state": "later", "t": 0.5}]}"#,
        )
        .unwrap();
        let feed = ExpressionFeed {
            timeline: Some(timeline),
            ..Default::default()
        };
        let mut app = started_in_a(Some(feed));
        assert_eq!(app.subsystems.names(), ["expression feed"]);
        assert!(app.launch.expression_feed.is_none(), "the subsystem has it");

        // Its states reach scripts at Stage::Input, as the feed's did
        app.tick();
        assert_eq!(
            app.script.expression().get("state").map(|s| s.to_string()),
            Some("hello".into())
        );
        app.tick();
        app.tick();
        assert_eq!(
            app.script.expression().get("state").map(|s| s.to_string()),
            Some("later".into())
        );

        // A transition keeps it: it belongs to the run, not the scene
        assert!(app.execute_scene_transition("b.scene.toml"));
        assert_eq!(app.subsystems.names(), ["expression feed"]);
    });
}

#[test]
fn a_scene_with_a_wooze_field_gets_shallow_wooze_and_the_next_scene_drops_it() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(None);
        assert!(app.execute_scene_transition("woozy.scene.toml"));
        assert_eq!(app.subsystems.names(), ["shallow wooze"]);

        assert!(app
            .subsystems
            .find::<super::shallow_wooze::ShallowWoozeSubsystem>()
            .is_some());

        assert!(app.execute_scene_transition("b.scene.toml"));
        assert!(app.subsystems.is_empty(), "{:?}", app.subsystems.names());
    });
}
