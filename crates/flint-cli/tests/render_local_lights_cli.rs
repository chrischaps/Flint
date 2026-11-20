//! Point and spot lights on grass and water (#1048), end to end through
//! `flint render`. Before it the grass and ocean shaders declared the lights
//! and never read them, so a lantern's pool stopped at the grass line and the
//! waterline. Each test lights one half of a dark frame and asks that half to
//! brighten and the other to stay as it was, byte for byte. They skip
//! themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// A night with no sun: the moon is a directional at 0, so the ocean's
/// fallback (a white sun overhead when a scene has no directional) stays
/// out, and the camera looks down the middle so x < 0 is the left half.
const NIGHT: &str = r#"
[scene]
name = "Local lights"

[environment]
ambient_sky = [0.03, 0.03, 0.05]
ambient_ground = [0.01, 0.01, 0.01]
shadows = false

[camera]
position = [0, 3, 8]
target = [0, 0, 0]

[entities.moon.light]
type = "directional"
direction = [0.0, 1.0, 0.0]
intensity = 0.0
"#;

/// A meadow over an unlit black floor, so all the light a lamp adds to
/// the frame is light on the blades.
const MEADOW: &str = r#"
[entities.floor.transform]
position = [0, -0.1, 0]

[entities.floor.bounds]
min = [-10, 0, -10]
max = [10, 0.1, 10]

[entities.floor.material]
color = [0.0, 0.0, 0.0, 1.0]
unlit = true

[entities.meadow.transform]
position = [-10, 0, -10]

[entities.meadow.grass_ground]
mask = "white.png"
width = 20.0
depth = 20.0
"grass.density" = 30.0
"grass.blade_height" = 0.5
"grass.wind_strength" = 0.0
"#;

/// A calm sea.
const SEA: &str = r#"
[entities.sea.ocean]
seed = 3
amplitude = 0.12
wavelength_min = 0.8
wavelength_max = 6.0
"#;

/// A lantern over the right half: at 4 m from x = 4, it gives nothing at
/// x = 0, the frame's middle column.
const LANTERN: &str = r#"
[entities.lantern.transform]
position = [4.0, 0.9, 0.0]

[entities.lantern.light]
type = "point"
color = [1.0, 0.65, 0.3]
intensity = 6.0
range = 4.0
"#;

/// A spot straight down on the right half, its cone well inside it.
const SPOT: &str = r#"
[entities.lamp.transform]
position = [4.0, 3.0, 0.0]

[entities.lamp.light]
type = "spot"
direction = [0.0, -1.0, 0.0]
color = [0.8, 0.9, 1.0]
intensity = 20.0
range = 6.0
inner_angle_deg = 20.0
outer_angle_deg = 30.0
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_local_lights_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::GrayImage::from_pixel(8, 8, image::Luma([255]))
        .save(dir.join("white.png"))
        .unwrap();
    dir
}

fn render(dir: &Path, name: &str, scene: &str) -> (Output, PathBuf) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{NIGHT}\n{scene}")).unwrap();
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

/// The left and right halves' pixels below the horizon (the lower two
/// thirds), as luma.
fn halves(png: &Path) -> (Vec<u8>, Vec<u8>) {
    let image = image::open(png).unwrap().to_luma8();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (x, y, p) in image.enumerate_pixels() {
        if y < HEIGHT / 3 {
            continue;
        }
        // Leave the middle columns out: a blade rooted at x = 0 leans
        // across it
        if x < WIDTH / 2 - 4 {
            left.push(p.0[0]);
        } else if x >= WIDTH / 2 + 4 {
            right.push(p.0[0]);
        }
    }
    (left, right)
}

fn mean(v: &[u8]) -> f64 {
    v.iter().map(|&p| p as f64).sum::<f64>() / v.len() as f64
}

/// Renders `ground` dark and then with `lamp`, and holds the lamp's half to
/// brightening by `at_least` levels on average (before #1048 it was 0.00 in
/// all four; with it, 8.5 and 4.2 on grass, 22 and 11 on water) and the other
/// half to its dark bytes.
fn lights_its_half(name: &str, ground: &str, lamp: &str, at_least: f64) {
    let dir = dir(name);
    let (dark, dark_png) = render(&dir, "dark", ground);
    if no_gpu(&dark) {
        return;
    }
    ok(&dark);
    let (lit, lit_png) = render(&dir, "lit", &format!("{ground}\n{lamp}"));
    ok(&lit);
    let (dark_left, dark_right) = halves(&dark_png);
    let (lit_left, lit_right) = halves(&lit_png);
    let gain = mean(&lit_right) - mean(&dark_right);
    assert!(
        gain >= at_least,
        "{name}: the lamp's half brightened by {gain:.2} levels, wanted {at_least} \
         (dark {:.2}, lit {:.2})",
        mean(&dark_right),
        mean(&lit_right)
    );
    assert!(
        dark_left == lit_left,
        "{name}: the half the lamp doesn't reach changed (dark {:.3}, lit {:.3})",
        mean(&dark_left),
        mean(&lit_left)
    );
}

#[test]
fn a_lantern_lights_the_grass_around_it() {
    lights_its_half("grass_point", MEADOW, LANTERN, 2.0);
}

#[test]
fn a_spot_lights_the_grass_in_its_cone() {
    lights_its_half("grass_spot", MEADOW, SPOT, 2.0);
}

#[test]
fn a_lantern_lights_the_water_under_it() {
    lights_its_half("water_point", SEA, LANTERN, 2.0);
}

#[test]
fn a_spot_lights_the_water_in_its_cone() {
    lights_its_half("water_spot", SEA, SPOT, 2.0);
}
