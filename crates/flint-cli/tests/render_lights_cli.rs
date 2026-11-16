//! Lights that used to fail in silence (#682), end to end through
//! `flint render`: a typo'd `type` says so by name, a light past the
//! shader's cap is named and the nearest the camera are the ones drawn, and
//! a spot light with no `direction` turns with its parent. They skip
//! themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A dim grey floor, no shadows, and nothing lit but what the scene adds.
const FLOOR: &str = r#"
[scene]
name = "Lights"

[environment]
ambient_sky = [0.03, 0.03, 0.04]
ambient_ground = [0.02, 0.02, 0.02]
shadows = false

[entities.ground.transform]
position = [0, -0.1, 0]

[entities.ground.bounds]
min = [-14, 0, -6]
max = [14, 0.1, 6]

[entities.ground.material]
color = [0.8, 0.8, 0.8, 1.0]
roughness = 0.9
"#;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_lights_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn render(dir: &Path, name: &str, scene: &str) -> (Output, PathBuf) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{FLOOR}\n{scene}")).unwrap();
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

/// Mean luma of the left and right halves of the image's lower two thirds
/// (the floor).
fn halves(png: &Path) -> (f64, f64) {
    let image = image::open(png).unwrap().to_luma8();
    let (mut left, mut right, mut n) = (0u64, 0u64, 0u64);
    for (x, y, p) in image.enumerate_pixels() {
        if y < HEIGHT / 3 {
            continue;
        }
        if x < WIDTH / 2 {
            left += p.0[0] as u64;
            n += 1;
        } else {
            right += p.0[0] as u64;
        }
    }
    (left as f64 / n as f64, right as f64 / n as f64)
}

#[test]
fn a_light_of_no_known_type_is_named_and_off() {
    let dir = dir("typo");
    let (run, _) = render(
        &dir,
        "typo",
        r#"
[camera]
position = [0, 6, 9]
target = [0, 0, 0]

[entities.lamp.transform]
position = [0, 3, 0]

[entities.lamp.light]
type = "spott"
intensity = 30
"#,
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let said = stderr(&run);
    assert!(
        said.contains(
            "entity 'lamp' light.type: \"spott\" is not a light type, so this light is off"
        ),
        "{said}"
    );
    assert!(
        said.contains("none the renderer can use: the built-in key and fill light the scene"),
        "{said}"
    );
}

/// Seventeen point lamps in a row, the camera at `lamp_17`'s end.
fn lamps(from: usize) -> String {
    let mut scene = String::from("[camera]\nposition = [10, 5, 7]\ntarget = [10, 0, 0]\n");
    for i in from..=17 {
        let x = -12.0 + (i - 1) as f32 * 1.5;
        scene.push_str(&format!(
            "\n[entities.lamp_{i:02}.transform]\nposition = [{x}, 1.0, 0]\n\n\
             [entities.lamp_{i:02}.light]\ntype = \"point\"\ncolor = [1.0, 0.75, 0.4]\n\
             intensity = 6\nrange = 2.2\n"
        ));
    }
    scene
}

#[test]
fn past_the_cap_the_lamps_nearest_the_camera_are_drawn() {
    let dir = dir("cap");
    let (seventeen, all) = render(&dir, "seventeen", &lamps(1));
    if no_gpu(&seventeen) {
        return;
    }
    ok(&seventeen);
    // lamp_01, the farthest, is the one left out, and said by name; by name
    // order it was lamp_17, the nearest, in silence
    let said = stderr(&seventeen);
    assert!(
        said.contains("warning[light-cap] entity 'lamp_01' component 'light': the shader takes 16 point lights, the nearest the camera"),
        "{said}"
    );
    assert!(!said.contains("entity 'lamp_17'"), "{said}");

    // So the frame is the frame of the sixteen it kept, byte for byte
    let (sixteen, kept) = render(&dir, "sixteen", &lamps(2));
    ok(&sixteen);
    assert!(!stderr(&sixteen).contains("light-cap"));
    assert_eq!(
        image::open(&all).unwrap().to_rgba8(),
        image::open(&kept).unwrap().to_rgba8(),
        "seventeen lamps should draw as the sixteen nearest the camera"
    );
}

/// A spot light with no `direction` on an arm turned `degrees` about z.
fn torch(degrees: f32) -> String {
    format!(
        r#"
[camera]
position = [0, 7, 11]
target = [0, 0, 0]

[entities.arm.transform]
position = [0, 4, 0]
rotation = [0, 0, {degrees}]

[entities.torch]
parent = "arm"

[entities.torch.transform]
position = [0, 0, 0]

[entities.torch.light]
type = "spot"
color = [1.0, 0.9, 0.7]
intensity = 40
range = 14
inner_angle_deg = 14
outer_angle_deg = 23
"#
    )
}

#[test]
fn a_spot_with_no_direction_turns_with_its_parent() {
    let dir = dir("spot");
    let (level, straight) = render(&dir, "level", &torch(0.0));
    if no_gpu(&level) {
        return;
    }
    ok(&level);
    let (left_run, left) = render(&dir, "left", &torch(-40.0));
    ok(&left_run);
    let (right_run, right) = render(&dir, "right", &torch(40.0));
    ok(&right_run);

    let (off_run, off) = render(
        &dir,
        "off",
        &torch(0.0).replace("intensity = 40", "intensity = 0"),
    );
    ok(&off_run);

    let (l, r) = halves(&straight);
    let (unlit, _) = halves(&off);
    // Straight down: a pool in the middle, the halves alike, and lit at all
    // (the mesh shaders took a spot pointing down for one pointing up, and
    // drew this floor as dark as the torch turned off)
    assert!((l - r).abs() < 1.0, "unturned: {l} | {r}");
    assert!(l > unlit + 2.0, "the pool is lit: {l} against {unlit}");

    let (ll, lr) = halves(&left);
    let (rl, rr) = halves(&right);
    // A turn of -40 degrees about z swings its -Y toward -x, +40 toward +x
    assert!(ll > lr + 5.0, "turned left: {ll} | {lr}");
    assert!(rr > rl + 5.0, "turned right: {rl} | {rr}");
}
