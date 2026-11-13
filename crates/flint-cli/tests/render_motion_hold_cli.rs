//! Motion on twos (#405), end to end: `flint render` steps the frozen YBot's
//! idle at 48 frames a second, held at 12 drawings a second and sketched in
//! pencil with a boil of 12, beside a box a script turns smoothly. The held
//! half of the frame changes only every fourth frame, the pose and the lines
//! together; the box, untagged, changes on the others too. They skip
//! themselves on a machine without a GPU.
//!
//! Physics is not run by `flint render`; that a held prop still falls at
//! 60 Hz is by construction (the hold is the renderer's, the world is never
//! written) and was checked in the player for the ticket.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// The YBot on the left, held and sketched; the box on the right, smooth.
const HELD: &str = r#"
[scene]
name = "Held"

[camera]
position = [0.0, 1.2, 4.2]
target = [0.0, 0.9, 0.0]
fov = 40.0

[entities.sun.light]
type = "directional"
direction = [0.3, 0.7, 0.4]
intensity = 2.5

[entities.ybot.transform]
position = [-0.7, 0.0, 0.0]

[entities.ybot.model]
asset = "YBotWithIdle"

[entities.ybot.skeleton]
skin = "Armature"

[entities.ybot.animator]
clip = "mixamo.com.001"
autoplay = true
loop = true
playing = true

[entities.ybot.motion_hold]
rate = 12.0

[entities.ybot.pencil_style]
boil = 12.0

[entities.spinner.transform]
position = [0.8, 0.9, 0.0]
scale = [0.6, 0.6, 0.6]

[entities.spinner.model]
asset = "Box"

[entities.spinner.script]
source = "spin.rhai"
"#;

const SPIN: &str =
    "fn on_update() { set_rotation(self_entity(), 20.0, total_time() * 120.0, 0.0); }\n";
const HOLD_FROM_A_SCRIPT: &str = "fn on_init() { set_motion_hold(self_entity(), 12.0); }\n";

const HOLD: &str = "[entities.ybot.motion_hold]\nrate = 12.0\n";
const PENCIL: &str = "[entities.ybot.pencil_style]\nboil = 12.0\n";

const WIDTH: u32 = 240;
/// Frames 0..0.5 s at 48 a second; a 12 Hz tick every fourth.
const FRAMES: usize = 24;
const EVERY: usize = 4;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project of its own holding the scenes, the script and the two models.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_motion_hold_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let models = repo().join("tests/fixtures/scenes/models");
    for model in ["YBotWithIdle.glb", "Box.glb"] {
        std::fs::copy(models.join(model), dir.join("models").join(model)).unwrap();
    }
    std::fs::write(dir.join("scripts/spin.rhai"), SPIN).unwrap();
    std::fs::write(dir.join("scripts/hold.rhai"), HOLD_FROM_A_SCRIPT).unwrap();
    std::fs::write(dir.join("held.scene.toml"), HELD).unwrap();
    let unheld = HELD.replace(HOLD, "");
    assert_ne!(unheld, HELD);
    std::fs::write(dir.join("unheld.scene.toml"), &unheld).unwrap();
    let plain = unheld.replace(PENCIL, "");
    assert_ne!(plain, unheld);
    std::fs::write(dir.join("plain.scene.toml"), &plain).unwrap();
    let scripted = format!("{plain}\n[entities.ybot.script]\nsource = \"hold.rhai\"\n");
    std::fs::write(dir.join("scripted.scene.toml"), scripted).unwrap();
    dir
}

/// Half a second at 48 fps into `<out>/f-000.png`...
fn frames(dir: &Path, scene: &str, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join(scene))
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args([
            "--width",
            &WIDTH.to_string(),
            "--height",
            "150",
            "--no-grid",
        ])
        .args([
            "--scripts",
            "--anim-time",
            "0",
            "--time",
            "0..0.5",
            "--fps",
            "48",
        ])
        .arg("-o")
        .arg(dir.join(out).join("f-%03d.png"))
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

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

fn frame(dir: &Path, out: &str, i: usize) -> image::RgbaImage {
    image::open(dir.join(out).join(format!("f-{i:03}.png")))
        .unwrap()
        .to_rgba8()
}

/// For each frame from 1, the pixels that changed from the one before in the
/// left half (the YBot) and the right half (the box).
fn changes(dir: &Path, out: &str) -> Vec<(usize, usize)> {
    let all: Vec<_> = (0..FRAMES).map(|i| frame(dir, out, i)).collect();
    all.windows(2)
        .map(|w| {
            let (mut left, mut right) = (0, 0);
            for (x, y, p) in w[1].enumerate_pixels() {
                if w[0].get_pixel(x, y) != p {
                    if x < WIDTH / 2 {
                        left += 1;
                    } else {
                        right += 1;
                    }
                }
            }
            (left, right)
        })
        .collect()
}

/// Frames (from 1) whose half changed.
fn turned(changes: &[(usize, usize)], left: bool) -> Vec<usize> {
    changes
        .iter()
        .enumerate()
        .filter(|(_, (l, r))| if left { *l > 0 } else { *r > 0 })
        .map(|(i, _)| i + 1)
        .collect()
}

fn ticks() -> Vec<usize> {
    (1..FRAMES).filter(|i| i % EVERY == 0).collect()
}

#[test]
fn a_held_pose_and_its_pencil_lines_turn_on_the_same_ticks() {
    let dir = project("held");
    let run = frames(&dir, "held.scene.toml", "held", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let held = changes(&dir, "held");
    assert_eq!(turned(&held, true), ticks(), "the YBot: {held:?}");
    // The box beside it names no hold and turns on the frames between too
    let smooth = turned(&held, false);
    assert!(smooth.len() >= FRAMES - 3, "the box: {held:?}");
    assert!(smooth.iter().any(|i| i % EVERY != 0));

    // Lines and pose each turn on the ticks: with the pencil off the pose
    // alone still holds
    let unsketched = HELD.replace(PENCIL, "");
    std::fs::write(dir.join("unsketched.scene.toml"), unsketched).unwrap();
    ok(&frames(&dir, "unsketched.scene.toml", "unsketched", &[]));
    assert_eq!(turned(&changes(&dir, "unsketched"), true), ticks());
}

#[test]
fn without_the_hold_the_pose_moves_between_ticks() {
    let dir = project("unheld");
    let run = frames(&dir, "unheld.scene.toml", "unheld", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let between: Vec<usize> = turned(&changes(&dir, "unheld"), true)
        .into_iter()
        .filter(|i| i % EVERY != 0)
        .collect();
    assert!(between.len() > 10, "{between:?}");
}

#[test]
fn a_held_capture_is_byte_identical_run_to_run() {
    let dir = project("twice");
    let run = frames(&dir, "held.scene.toml", "a", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    ok(&frames(&dir, "held.scene.toml", "b", &[]));
    for i in 0..FRAMES {
        let name = format!("f-{i:03}.png");
        assert_eq!(
            std::fs::read(dir.join("a").join(&name)).unwrap(),
            std::fs::read(dir.join("b").join(&name)).unwrap(),
            "frame {i}"
        );
    }
}

/// `--motion-hold 12` holds everything, the box included, and the post time:
/// a whole-frame pencil boiling at 24 still redraws once a tick.
#[test]
fn the_whole_frame_hold_holds_every_entity_and_the_post_time() {
    let dir = project("whole");
    let run = frames(
        &dir,
        "plain.scene.toml",
        "whole",
        &["--motion-hold", "12", "--style", "pencil:boil=24"],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let whole = changes(&dir, "whole");
    assert_eq!(turned(&whole, true), ticks(), "{whole:?}");
    assert_eq!(turned(&whole, false), ticks(), "{whole:?}");
}

/// #873: two lanterns sway over a floor, each carrying a point light and
/// nothing drawn, so the only pixels that move are the pools of light. The
/// left lantern is held on twos; the right one isn't.
const LANTERNS: &str = r#"
[scene]
name = "Lanterns"

[camera]
position = [0.0, 6.0, 2.5]
target = [0.0, 0.0, 0.0]
fov = 60.0

[entities.floor.transform]
position = [0.0, -0.1, 0.0]

[entities.floor.bounds]
min = [-5.0, -0.1, -3.0]
max = [5.0, 0.0, 3.0]

[entities.floor.material]
color = [0.6, 0.58, 0.55, 1.0]
roughness = 0.9

[entities.left_lantern.transform]
position = [-2.4, 0.6, 0.0]

[entities.left_lantern.script]
source = "sway_left.rhai"

[entities.left_lantern.motion_hold]
rate = 12.0

[entities.left_light]
parent = "left_lantern"

[entities.left_light.transform]
position = [0.0, 0.0, 0.0]

[entities.left_light.light]
type = "point"
color = [1.0, 0.75, 0.4]
intensity = 6.0
range = 1.0

[entities.right_lantern.transform]
position = [2.4, 0.6, 0.0]

[entities.right_lantern.script]
source = "sway_right.rhai"

[entities.right_light]
parent = "right_lantern"

[entities.right_light.transform]
position = [0.0, 0.0, 0.0]

[entities.right_light.light]
type = "point"
color = [1.0, 0.75, 0.4]
intensity = 6.0
range = 1.0
"#;

fn sway(x: f32) -> String {
    format!(
        "fn on_update() {{ set_position(self_entity(), {x:.1} + 0.3 * sin(total_time() * 9.0), 0.6, 0.0); }}\n"
    )
}

#[test]
fn a_held_entitys_point_light_steps_on_its_ticks() {
    let dir = project("lanterns");
    std::fs::write(dir.join("scripts/sway_left.rhai"), sway(-2.4)).unwrap();
    std::fs::write(dir.join("scripts/sway_right.rhai"), sway(2.4)).unwrap();
    std::fs::write(dir.join("lanterns.scene.toml"), LANTERNS).unwrap();
    let run = frames(&dir, "lanterns.scene.toml", "lanterns", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let pools = changes(&dir, "lanterns");
    // The held lantern's pool moves only when its tick turns...
    assert_eq!(turned(&pools, true), ticks(), "the held light: {pools:?}");
    // ...and the other's glides on every frame
    let smooth = turned(&pools, false);
    assert!(smooth.len() >= FRAMES - 3, "the smooth light: {pools:?}");
    assert!(smooth.iter().any(|i| i % EVERY != 0));
}

#[test]
fn a_script_holds_an_entity_with_set_motion_hold() {
    let dir = project("scripted");
    let run = frames(&dir, "scripted.scene.toml", "scripted", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let scripted = changes(&dir, "scripted");
    assert_eq!(turned(&scripted, true), ticks(), "{scripted:?}");
    assert!(turned(&scripted, false).len() >= FRAMES - 3);
}
