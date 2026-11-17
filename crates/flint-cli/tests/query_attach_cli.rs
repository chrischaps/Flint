//! A query means the same of a file and of a running game (#713): the real
//! `flint` binary asks a scene file with `--scene` and a live world with
//! `--attach`, and prints the same bytes. The live side is the player's own
//! socket (`flint_player::InspectListener`), answered from a thread standing
//! in for the frame, over the world the same scene loads into. Then the
//! world changes under it, as a script would change it, and the next answer
//! says so: the file can't.

use flint_player::InspectListener;
use flint_schema::SchemaRegistry;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = "\
[scene]
name = \"Hall\"

[entities.front_door.door]
locked = false

[entities.front_door.transform]
position = [0, 0, 10]

[entities.cellar_door.door]
locked = true

[entities.player.transform]
position = [0, 1, -2]
";

fn project(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flint_713_{test}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hall.scene.toml"), SCENE).unwrap();
    dir
}

fn flint(root: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(root)
        .output()
        .expect("run flint")
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "flint failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

/// A stand-in for the player's frame: answers the socket from `world` until
/// dropped, at 1.5 s of game time, counting frames.
struct Frames {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Frames {
    fn run(listener: InspectListener, world: Arc<Mutex<flint_ecs::FlintWorld>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut frame = 0;
            while !stopping.load(Ordering::Relaxed) {
                frame += 1;
                let world = world.lock().unwrap();
                listener.answer_from(&world, "hall.scene.toml", 1.5, frame);
                drop(world);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn a_query_prints_the_same_for_a_file_and_a_running_game() {
    let root = project("same");
    let (world, _) =
        flint_scene::load_scene(root.join("hall.scene.toml"), &SchemaRegistry::new()).unwrap();
    let world = Arc::new(Mutex::new(world));
    let listener = InspectListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().port().to_string();
    let _frames = Frames::run(listener, world.clone());

    let asks: [&[&str]; 4] = [
        &["query", "entities where door.locked == true"],
        &["query", "entities", "--data"],
        &["query", "components"],
        &["entity", "show", "front_door"],
    ];
    for ask in asks {
        let file = stdout(&flint(
            &root,
            &[ask, &["--scene", "hall.scene.toml"]].concat(),
        ));
        let live = flint(&root, &[ask, &["--attach", &port]].concat());
        assert_eq!(stdout(&live), file, "{ask:?}");
        let said = String::from_utf8_lossy(&live.stderr);
        assert!(said.contains("live: hall.scene.toml at t=1.500s"), "{said}");
    }

    // A script unlocks the cellar: the game says so, the file still doesn't.
    {
        let mut world = world.lock().unwrap();
        let cellar = world.get_id("cellar_door").unwrap();
        world.get_components_mut(cellar).unwrap().set_field(
            "door",
            "locked",
            toml::Value::Boolean(false),
        );
    }
    let locked = &["query", "entities where door.locked == true"];
    let live: serde_json::Value = serde_json::from_str(&stdout(&flint(
        &root,
        &[locked, &["--attach", &port][..]].concat(),
    )))
    .unwrap();
    assert_eq!(live, serde_json::json!([]), "nothing is locked now");
    let file: serde_json::Value = serde_json::from_str(&stdout(&flint(
        &root,
        &[locked, &["--scene", "hall.scene.toml"][..]].concat(),
    )))
    .unwrap();
    assert_eq!(file[0]["name"], "cellar_door");
}

#[test]
fn a_refusal_from_the_game_fails_the_command_with_its_reason() {
    let root = project("refusal");
    let world = Arc::new(Mutex::new(flint_ecs::FlintWorld::new()));
    let listener = InspectListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().port().to_string();
    let _frames = Frames::run(listener, world);

    let out = flint(
        &root,
        &["--json", "entity", "show", "nobody", "--attach", &port],
    );
    assert_eq!(out.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["message"], "Entity 'nobody' not found");
}
