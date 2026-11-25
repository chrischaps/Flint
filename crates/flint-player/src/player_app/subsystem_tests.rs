//! The subsystem seam (ADR 0117, #724 step 3): a toy subsystem hears every
//! stage of a frame in order, a scene come up and go, and is torn down last
//! added first; the expression feed is one only when the run has a feed, and
//! its states reach scripts at `Stage::Input` as before. Headless: no window.

use super::expression_feed::{ExpressionFeed, ExpressionTimeline};
use super::scene_entry::SceneEntry;
use super::subsystem::{ExitReport, Lasts, ReportCtx, SceneCtx, Stage, StageCtx, Subsystem};
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
            packages: None,
        },
    );
    app.launch.expression_feed = feed;
    app.core.clock.frame_step = Some(0.25);
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
    /// Toy "a" declares `ping` on F8; the others none.
    fn debug_keys(&self) -> &'static [flint_runtime::DebugKey] {
        const PING: &[flint_runtime::DebugKey] = &[flint_runtime::DebugKey {
            action: "ping",
            key: "F8",
            does: "Ping",
        }];
        if self.name == "a" {
            PING
        } else {
            &[]
        }
    }
    fn on_debug_key(&mut self, action: &str, key: &str, _ctx: &mut StageCtx) {
        self.say(format!("key {action} {key}"));
    }
    fn exit_report(&self, _ctx: &ReportCtx, report: &mut ExitReport) {
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
        assert!(
            app.core.subsystems.is_empty(),
            "{:?}",
            app.core.subsystems.names()
        );
    });
}

#[test]
fn a_toy_hears_every_stage_in_order() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_in_a(None);
        app.core.subsystems.push(Toy::boxed("toy", &log, false));
        app.tick();
        let heard = take(&log);
        let stages: Vec<String> = Stage::UPDATES
            .iter()
            .map(|s| {
                let policy = !matches!(s, Stage::Clock | Stage::Input);
                format!("toy {s:?} policy={policy}")
            })
            .collect();
        assert_eq!(heard, stages);
    });
}

#[test]
fn a_transition_unloads_last_added_first_and_drops_the_scenes_own() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_in_a(None);
        app.core.subsystems.push(Toy::boxed("run", &log, false));
        app.core.subsystems.push(Toy::boxed("scene", &log, true));

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
        assert_eq!(app.core.subsystems.names(), ["run"]);
    });
}

#[test]
fn subsystems_take_debug_keys_and_add_to_the_exit_report() {
    vfs::scoped(Arc::new(game()), || {
        let log = Log::default();
        let mut app = started_in_a(None);
        app.core.subsystems.push(Toy::boxed("a", &log, false));
        app.core.subsystems.push(Toy::boxed("b", &log, false));
        // A key only the subsystem that declares it hears, and one it
        // doesn't declare is no debug key of theirs
        assert!(app.handle_debug_key(KeyCode::F8), "a's ping");
        assert_eq!(take(&log), ["a key ping F8"], "b declares none");
        assert!(!app.handle_debug_key(KeyCode::F9));
        assert!(take(&log).is_empty());

        assert_eq!(app.subsystem_report(), ["[a] done", "[b] done"]);

        let list = std::mem::take(&mut app.core.subsystems);
        drop(list);
        assert_eq!(take(&log), ["b dropped", "a dropped"], "last added first");
    });
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
        assert_eq!(app.core.subsystems.names(), ["expression feed"]);
        assert!(app.launch.expression_feed.is_none(), "the subsystem has it");

        // Its states reach scripts at Stage::Input, as the feed's did
        app.tick();
        assert_eq!(
            app.core
                .script
                .expression()
                .get("state")
                .map(|s| s.to_string()),
            Some("hello".into())
        );
        app.tick();
        app.tick();
        assert_eq!(
            app.core
                .script
                .expression()
                .get("state")
                .map(|s| s.to_string()),
            Some("later".into())
        );

        // A transition keeps it: it belongs to the run, not the scene
        assert!(app.execute_scene_transition("b.scene.toml"));
        assert_eq!(app.core.subsystems.names(), ["expression feed"]);
    });
}

/// Whether something listens at `addr`: a connect within a short wait. (A
/// refused loopback connect takes Windows about 2 s; past the wait it's no.)
#[cfg(not(target_arch = "wasm32"))]
fn accepts(addr: std::net::SocketAddr) -> bool {
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_socket_opens_once_the_first_frame_is_presented() {
    use super::expression_feed::ExpressionSubsystem;
    use std::io::Write;
    use std::time::{Duration, Instant};
    vfs::scoped(Arc::new(game()), || {
        // A port that was free a moment ago
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let feed = ExpressionFeed::open(None, Some(&addr.to_string()))
            .unwrap()
            .unwrap();
        assert!(!accepts(addr), "checked at start, not listening");
        let mut app = started_in_a(Some(feed));
        app.tick();
        assert!(!accepts(addr), "the first frame: still starting");
        app.tick();
        assert!(accepts(addr), "frames are running: it listens");

        // Two states sent apart are heard on frames of their own, not drained
        // together as a slow first frame drained them (#584). The client
        // waits for its reader thread to start (a connection's first lines
        // can land before it does), and the gap is several of this loop's
        // frames (about 10 ms each here), so a busy machine still sees two.
        let client = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            s.write_all(b"{\"state\": \"arriving\"}\n").unwrap();
            std::thread::sleep(Duration::from_millis(60));
            s.write_all(b"{\"state\": \"listening\"}\n").unwrap();
            std::thread::sleep(Duration::from_millis(200)); // hang up after they're read
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let record = loop {
            app.tick();
            let feed = app.core.subsystems.find::<ExpressionSubsystem>().unwrap();
            let view = feed.view(app.core.clock.total_time);
            if view.record.delivered() >= 2 {
                break view.record;
            }
            assert!(Instant::now() < deadline, "{:?}", view.socket);
            std::thread::sleep(Duration::from_millis(2));
        };
        client.join().unwrap();
        let frames: Vec<(u64, &str)> = record
            .newest_first()
            .map(|a| (a.frame, a.state["state"].as_str().unwrap()))
            .collect();
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!((frames[1].1, frames[0].1), ("arriving", "listening"));
        assert!(frames[0].0 > frames[1].0, "two frames: {frames:?}");
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_taken_port_ends_the_run_when_the_socket_would_open() {
    vfs::scoped(Arc::new(game()), || {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = busy.local_addr().unwrap().to_string();
        let feed = ExpressionFeed::open(None, Some(&addr)).unwrap().unwrap();
        let mut app = started_in_a(Some(feed));
        app.tick();
        assert!(!app.outcome.exit_requested && app.run_failure().is_none());
        app.tick();
        assert!(app.outcome.exit_requested, "a launcher sees the player go");
        let why = app.run_failure().expect("the run fails");
        assert!(why.contains(&addr), "{why}");
    });
}

/// The F3 Expression panel the feed added, if any (#525).
#[cfg(feature = "debug-hud")]
fn expression_panel(app: &mut PlayerApp) -> Option<&mut super::expression_panel::ExpressionPanel> {
    app.debug_panels
        .iter_mut()
        .find_map(|p| p.as_any_mut().downcast_mut())
}

#[cfg(feature = "debug-hud")]
#[test]
fn the_expression_panel_shows_each_state_the_frame_it_arrives() {
    use flint_debug_ui::DebugPanel;
    vfs::scoped(Arc::new(game()), || {
        // No feed, no panel
        let mut app = started_in_a(None);
        app.tick();
        assert!(expression_panel(&mut app).is_none());

        // Frames are 0.25 s: "hello" on the first, "later" on the third
        let timeline = ExpressionTimeline::parse(
            r#"{"events": [{"state": "hello", "t": 0.0},
                           {"state": "later", "energy": 0.4, "t": 0.5}]}"#,
        )
        .unwrap();
        let feed = ExpressionFeed {
            timeline: Some(timeline),
            ..Default::default()
        };
        let mut app = started_in_a(Some(feed));
        app.tick();
        let panel = expression_panel(&mut app).expect("a feed gets the panel");
        assert!(!panel.is_open(), "registered closed; F3 opens it");
        assert!(panel.view().is_none(), "a closed panel isn't fed");
        panel.toggle();

        let mut seen = Vec::new();
        for _ in 0..4 {
            app.tick();
            let heard = app.core.script.expression();
            let view = expression_panel(&mut app).unwrap().view().unwrap().clone();
            let latest = view.record.latest().unwrap();
            // What the panel shows is what scripts hear, that same frame
            assert_eq!(
                Some(latest.state["state"].as_str().unwrap().to_string()),
                heard.get("state").map(|s| s.to_string())
            );
            seen.push((view.frame, latest.frame, latest.state["state"].clone()));
        }
        // The first frame showing "later" is the frame it arrived on
        let first = seen.iter().find(|s| s.2 == "later").expect("later came");
        assert_eq!(first.0, first.1, "{seen:?}");
        let view = expression_panel(&mut app).unwrap().view().unwrap().clone();
        let names: Vec<&str> = view
            .record
            .newest_first()
            .map(|a| a.state["state"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["later", "hello"], "newest first");
        let timeline = view.timeline.unwrap();
        assert_eq!((timeline.played, timeline.len, timeline.next), (2, 2, None));

        // A transition drops the scene's panels; the feed's comes back
        assert!(app.execute_scene_transition("b.scene.toml"));
        app.tick();
        assert!(expression_panel(&mut app).is_some());
    });
}

#[cfg(all(feature = "debug-hud", not(target_arch = "wasm32")))]
#[test]
fn a_malformed_socket_line_raises_the_panels_dropped_count() {
    use flint_debug_ui::DebugPanel;
    use std::io::Write;
    use std::time::{Duration, Instant};
    vfs::scoped(Arc::new(game()), || {
        let feed = ExpressionFeed::open(None, Some("127.0.0.1:0"))
            .unwrap()
            .unwrap();
        let mut app = started_in_a(Some(feed));
        app.tick();
        expression_panel(&mut app).unwrap().toggle();
        // The socket opens at the second frame (#584), and the panel says where
        app.tick();
        let socket = expression_panel(&mut app).unwrap().view().unwrap().socket;
        let socket = socket.unwrap();
        assert!(socket.open, "{socket:?}");
        let addr = socket.addr;

        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.write_all(b"{\"state\": \"listening\"}\n{oops\n").unwrap();
        s.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let view = loop {
            app.tick();
            let view = expression_panel(&mut app).unwrap().view().unwrap().clone();
            let socket = view.socket.unwrap();
            if socket.malformed == 1 && view.record.latest().is_some() {
                break view;
            }
            assert!(Instant::now() < deadline, "{socket:?}");
            std::thread::sleep(Duration::from_millis(5));
        };
        let socket = view.socket.unwrap();
        assert_eq!(socket.connections, 1);
        assert_eq!((socket.oversize, socket.flooded), (0, 0));
        assert_eq!(view.record.latest().unwrap().state["state"], "listening");
        assert!(view.timeline.is_none());
        // and the panel is still there, still open
        assert!(expression_panel(&mut app).unwrap().is_open());
    });
}

#[cfg(feature = "living")]
#[test]
fn a_scene_with_a_wooze_field_gets_living_props_and_the_next_scene_drops_it() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(None);
        assert!(app.execute_scene_transition("woozy.scene.toml"));
        assert_eq!(app.core.subsystems.names(), ["living"]);

        assert!(app
            .core
            .subsystems
            .find::<super::living_subsystem::LivingSubsystem>()
            .is_some());

        assert!(app.execute_scene_transition("b.scene.toml"));
        assert!(
            app.core.subsystems.is_empty(),
            "{:?}",
            app.core.subsystems.names()
        );
    });
}

#[cfg(feature = "living")]
#[test]
fn a_state_pauses_the_grove_by_its_living_entry_and_not_by_animation() {
    use flint_runtime::{StateConfig, SystemPolicy};
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(None);
        assert!(app.execute_scene_transition("woozy.scene.toml"));
        let time = |app: &PlayerApp| {
            app.core
                .subsystems
                .find::<super::living_subsystem::LivingSubsystem>()
                .expect("the grove")
                .wooze_time
        };
        let states = [
            (
                "still",
                StateConfig::playing().with("living", SystemPolicy::Pause),
            ),
            (
                "cutscene",
                StateConfig::playing().with("animation", SystemPolicy::Pause),
            ),
        ];
        for (name, config) in &states {
            app.core
                .state_machine
                .register_state(name, config.clone())
                .expect("the player's machine knows `living`");
        }

        // The first frame after a transition has no time step yet
        let before = time(&app);
        app.tick();
        app.tick();
        assert!(time(&app) > before, "the grove's clock runs while playing");

        for (state, moves) in [("still", false), ("cutscene", true), ("paused", false)] {
            assert!(app.core.state_machine.push_state(state));
            let before = time(&app);
            app.tick();
            app.tick();
            assert_eq!(time(&app) > before, moves, "under `{state}`");
            app.core.state_machine.pop_state();
        }
    });
}
