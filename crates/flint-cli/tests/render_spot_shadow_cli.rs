//! A spot light's shadow (#504), end to end through `flint render`: a spot
//! with `cast_shadows = true` darkens the floor under what stands in its
//! beam and nothing else, the opt-in leaves a scene without it as it was,
//! `[environment] shadows = false` still wins, and a second spot asking is
//! named. They skip themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A grey floor, a block held above it, and no light but the spot the test
/// adds.
const STAGE: &str = r#"
[scene]
name = "Spot shadow"

[environment]
ambient_sky = [0.02, 0.02, 0.02]
ambient_ground = [0.01, 0.01, 0.01]

[camera]
position = [0, 7, 7]
target = [0, 0, 0]

[entities.ground.transform]
position = [0, -0.1, 0]

[entities.ground.bounds]
min = [-8, 0, -8]
max = [8, 0.1, 8]

[entities.ground.material]
color = [0.8, 0.8, 0.8, 1.0]
roughness = 0.9

[entities.block.transform]
position = [0.8, 1.2, 0.0]

[entities.block.bounds]
min = [-0.5, -0.5, -0.5]
max = [0.5, 0.5, 0.5]

[entities.block.material]
color = [0.6, 0.6, 0.6, 1.0]
"#;

/// The key: up and to the left, so the block's shadow falls to its right
/// where the camera sees it.
fn key(name: &str, extra: &str) -> String {
    format!(
        r#"
[entities.{name}.transform]
position = [-2.0, 5.0, 0.0]

[entities.{name}.light]
type = "spot"
direction = [0.45, -1.0, 0.0]
intensity = 40
range = 12
outer_angle_deg = 40
{extra}
"#
    )
}

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_spot_shadow_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn render(dir: &Path, name: &str, scene: &str) -> (Output, PathBuf) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{STAGE}\n{scene}")).unwrap();
    let out = dir.join(format!("{name}.png"));
    let run = std::process::Command::new(FLINT)
        .arg("render")
        .arg(&path)
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args(["--width", &WIDTH.to_string()])
        .args(["--height", &HEIGHT.to_string()])
        .arg("--no-grid")
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run flint");
    (run, out)
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

fn luma(png: &Path) -> image::GrayImage {
    image::open(png).unwrap().to_luma8()
}

#[test]
fn a_spot_that_casts_darkens_only_what_its_beam_cannot_reach() {
    let dir = dir("casts");
    let (run, lit) = render(&dir, "lit", &key("key", ""));
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let (run, cast) = render(&dir, "cast", &key("key", "cast_shadows = true"));
    ok(&run);
    let (lit, cast) = (luma(&lit), luma(&cast));

    let (mut darker, mut brighter) = (0u32, 0u32);
    for (a, b) in lit.pixels().zip(cast.pixels()) {
        let (a, b) = (a.0[0] as i32, b.0[0] as i32);
        if b + 8 < a {
            darker += 1;
        }
        if b > a + 1 {
            brighter += 1;
        }
    }
    // The block's shadow is a few percent of the frame
    let frame = WIDTH * HEIGHT;
    assert!(
        darker > frame / 100,
        "the shadow darkens {darker} of {frame} pixels"
    );
    assert_eq!(brighter, 0, "a shadow only takes light away");
}

#[test]
fn shadows_off_for_the_scene_win_over_a_spot_that_asks() {
    let dir = dir("scene_off");
    // A replace that holds whichever line endings the checkout gave the file
    let stage = STAGE.replace("[environment]", "[environment]\nshadows = false");
    assert!(stage.contains("shadows = false"));
    let write = |name: &str, extra: &str| {
        let path = dir.join(format!("{name}.scene.toml"));
        std::fs::write(&path, format!("{stage}\n{}", key("key", extra))).unwrap();
        let out = dir.join(format!("{name}.png"));
        let run = std::process::Command::new(FLINT)
            .arg("render")
            .arg(&path)
            .arg("--schemas")
            .arg(repo().join("schemas"))
            .args(["--width", &WIDTH.to_string()])
            .args(["--height", &HEIGHT.to_string()])
            .arg("--no-grid")
            .arg("-o")
            .arg(&out)
            .output()
            .expect("run flint");
        (run, out)
    };
    let (run, plain) = write("plain", "");
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let (run, asked) = write("asked", "cast_shadows = true");
    ok(&run);
    assert_eq!(
        std::fs::read(plain).unwrap(),
        std::fs::read(asked).unwrap(),
        "with the scene's shadows off, asking draws the same bytes"
    );
}

#[test]
fn a_second_spot_that_asks_is_named_and_lights_without_one() {
    let dir = dir("two");
    let two = format!(
        "{}\n{}",
        key("a_key", "cast_shadows = true"),
        key("b_fill", "cast_shadows = true"),
    );
    let (run, _) = render(&dir, "two", &two);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let err = stderr(&run);
    assert!(
        err.contains("warning[shadow-cap] entity 'b_fill' light.cast_shadows"),
        "{err}"
    );
    assert!(err.contains("'a_key' does"), "{err}");
    assert!(!err.contains("entity 'a_key' light.cast_shadows"), "{err}");
}
