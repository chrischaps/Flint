//! End-to-end tests for `flint render --anim-time` (#307): run the real binary
//! on a scene holding the frozen YBot with its idle clip, and hold what it
//! writes to the ticket. They skip themselves on a machine without a GPU.
//!
//! What this can't test is that the pose is the player's at that time, since
//! the player needs a window. The book's headless guide says how that was
//! measured.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Idle"

[camera]
position = [1.2, 1.4, 3.0]
target = [0.0, 1.0, 0.0]
fov = 40.0

[entities.sun.light]
type = "directional"
direction = [0.3, 0.7, 0.4]
intensity = 2.5

[entities.ybot.transform]
position = [0.0, 0.0, 0.0]

[entities.ybot.model]
asset = "YBotWithIdle"

[entities.ybot.skeleton]
skin = "Armature"

[entities.ybot.animator]
clip = "mixamo.com.001"
autoplay = true
loop = true
playing = true
"#;

const STILL: &str = r#"
[scene]
name = "Still"

[entities.sun.light]
type = "directional"
direction = [0.3, 0.7, 0.4]

[entities.box.transform]
position = [0.0, 0.5, 0.0]

[entities.box.model]
asset = "Box"
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project holding the scene and the model it loads, in a directory of its own.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_anim_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models")).unwrap();
    let models = repo().join("tests/fixtures/scenes/models");
    for model in ["YBotWithIdle.glb", "Box.glb"] {
        std::fs::copy(models.join(model), dir.join("models").join(model)).unwrap();
    }
    std::fs::write(dir.join("idle.scene.toml"), SCENE).unwrap();
    std::fs::write(dir.join("still.scene.toml"), STILL).unwrap();
    let no_skeleton = SCENE.replace(
        "[entities.ybot.skeleton]
skin = \"Armature\"
",
        "",
    );
    assert_ne!(no_skeleton, SCENE);
    std::fs::write(dir.join("unskeletoned.scene.toml"), no_skeleton).unwrap();
    dir
}

fn render(dir: &Path, scene: &str, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join(scene))
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .arg("--width")
        .arg("96")
        .arg("--height")
        .arg("128")
        .arg("--no-grid")
        .arg("-o")
        .arg(dir.join(out))
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Whether this run had no GPU to draw with, which isn't this test's business.
fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success() && stderr(o).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(o.status.success(), "flint failed: {}", stderr(o));
}

fn bytes(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap()
}

/// Pixels that differ between two renders.
fn changed(dir: &Path, a: &str, b: &str) -> usize {
    let a = image::open(dir.join(a)).unwrap().to_rgba8();
    let b = image::open(dir.join(b)).unwrap().to_rgba8();
    a.pixels().zip(b.pixels()).filter(|(x, y)| x != y).count()
}

#[test]
fn a_time_poses_the_character_the_same_every_run() {
    let dir = project("same");
    let rest = render(&dir, "idle.scene.toml", "rest.png", &[]);
    if no_gpu(&rest) {
        return;
    }
    ok(&rest);
    let first = render(&dir, "idle.scene.toml", "a.png", &["--anim-time", "1.5"]);
    ok(&first);
    assert!(
        stdout(&first).contains("Animation: ybot at t = 1.500s (1 skinned, 90 step(s)"),
        "{}",
        stdout(&first)
    );
    ok(&render(
        &dir,
        "idle.scene.toml",
        "b.png",
        &["--anim-time", "1.5"],
    ));
    assert_eq!(
        bytes(&dir, "a.png"),
        bytes(&dir, "b.png"),
        "same time, same pixels"
    );
    // The bind pose is a T: arms out where the idle has them down.
    assert!(changed(&dir, "rest.png", "a.png") > 200);
    // And the clip moves: another time is another pose.
    ok(&render(
        &dir,
        "idle.scene.toml",
        "c.png",
        &["--anim-time", "0.5"],
    ));
    assert!(changed(&dir, "a.png", "c.png") > 0);
}

#[test]
fn the_clock_carries_the_animation_on() {
    let dir = project("clock");
    let posed = render(
        &dir,
        "idle.scene.toml",
        "posed.png",
        &["--time", "0", "--anim-time", "1"],
    );
    if no_gpu(&posed) {
        return;
    }
    ok(&posed);
    // Started at 0 and run on with the clock to 1: the same 60 steps.
    ok(&render(
        &dir,
        "idle.scene.toml",
        "clocked.png",
        &["--time", "1", "--anim-time", "0"],
    ));
    assert_eq!(bytes(&dir, "posed.png"), bytes(&dir, "clocked.png"));
    // Without --anim-time the clock leaves the clip alone, and says so.
    let still = render(&dir, "idle.scene.toml", "still.png", &["--time", "1"]);
    ok(&still);
    let said = stdout(&still) + &stderr(&still);
    assert!(
        said.contains("animation clips don't run without --anim-time"),
        "{said}"
    );
}

#[test]
fn frames_step_the_animation() {
    let dir = project("frames");
    let run = render(
        &dir,
        "idle.scene.toml",
        "seq.png",
        &["--frames", "2", "--frame-dt", "0.5", "--anim-time", "1"],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    // Frame 1 is half a second on from frame 0, which is --anim-time 1 itself.
    ok(&render(
        &dir,
        "idle.scene.toml",
        "one.png",
        &["--anim-time", "1"],
    ));
    assert_eq!(bytes(&dir, "seq_0000.png"), bytes(&dir, "one.png"));
    assert!(changed(&dir, "seq_0000.png", "seq_0001.png") > 0);
}

/// #872: the idle's keys start at 1/30 s (Blender writes frame 1 there), and
/// the clip used to clamp to its first key until then, so frames 0 and 1 of a
/// 48 fps span drew one pose while physics moved. A clip starts at its first
/// key now. Before the fix frame 1 changed 0 pixels here, after it 183.
#[test]
fn the_first_step_of_a_span_moves() {
    let dir = project("first-step");
    let run = render(
        &dir,
        "idle.scene.toml",
        "span-%04d.png",
        &["--anim-time", "0", "--time", "0..0.0625", "--fps", "48"],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    // Frame 0 is the clip at 0, as a still at --anim-time 0 draws it...
    ok(&render(
        &dir,
        "idle.scene.toml",
        "zero.png",
        &["--anim-time", "0"],
    ));
    assert_eq!(bytes(&dir, "span-0000.png"), bytes(&dir, "zero.png"));
    // ...and frame 1 has moved on from it, by about as much as frame 2 does
    // from frame 1.
    let first = changed(&dir, "span-0000.png", "span-0001.png");
    let second = changed(&dir, "span-0001.png", "span-0002.png");
    assert!(
        first * 2 > second,
        "frame 1 changed {first} pixels, frame 2 {second}: the first step held"
    );
}

#[test]
fn a_scene_with_nothing_to_animate_is_told() {
    let dir = project("still");
    let run = render(&dir, "still.scene.toml", "box.png", &["--anim-time", "2"]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    assert!(
        stderr(&run).contains("--anim-time: nothing in this scene animates"),
        "{}",
        stderr(&run)
    );
    // And the picture is the one without the flag.
    ok(&render(&dir, "still.scene.toml", "plain.png", &[]));
    assert_eq!(bytes(&dir, "box.png"), bytes(&dir, "plain.png"));
}

#[test]
fn an_animator_with_no_skeleton_is_named() {
    let dir = project("no_skeleton");
    let run = render(
        &dir,
        "unskeletoned.scene.toml",
        "t.png",
        &["--anim-time", "1"],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    assert!(
        stderr(&run).contains("ybot has an animator and a skinned model but no `skeleton`"),
        "{}",
        stderr(&run)
    );
}
