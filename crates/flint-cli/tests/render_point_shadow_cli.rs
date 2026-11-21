//! A point light's shadow (#1112), end to end through `flint render`: a
//! lantern on a table with `cast_shadows = true` shadows the floor around
//! the table on every side, the opt-in leaves a scene without it as it was,
//! `[environment] shadows = false` still wins, and a second point light
//! asking is named. They skip themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A grey floor and a table, seen from straight above, and no light but the
/// lantern the test adds on the table.
const ROOM: &str = r#"
[scene]
name = "Point shadow"

[environment]
ambient_sky = [0.02, 0.02, 0.02]
ambient_ground = [0.01, 0.01, 0.01]

[camera]
position = [0, 6, 0.01]
target = [0, 0, 0]
fov = 50

[entities.floor.transform]
position = [0, -0.05, 0]

[entities.floor.bounds]
min = [-8, -0.05, -8]
max = [8, 0.05, 8]

[entities.floor.material]
color = [0.8, 0.8, 0.8, 1.0]
roughness = 0.9

[entities.table.transform]
position = [0, 0.76, 0]

[entities.table.bounds]
min = [-0.7, -0.03, -0.45]
max = [0.7, 0.03, 0.45]

[entities.table.material]
color = [0.6, 0.6, 0.6, 1.0]
"#;

/// The lantern: a point light a little above the table's top.
fn lantern(name: &str, extra: &str) -> String {
    format!(
        r#"
[entities.{name}.transform]
position = [0.0, 0.94, 0.0]

[entities.{name}.light]
type = "point"
intensity = 12
range = 10
source_radius = 0.03
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
        .join("render_point_shadow_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn render_room(dir: &Path, name: &str, room: &str, scene: &str) -> (Output, PathBuf) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{room}\n{scene}")).unwrap();
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

fn render(dir: &Path, name: &str, scene: &str) -> (Output, PathBuf) {
    render_room(dir, name, ROOM, scene)
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

/// The mean luma of a `w` x `h` box whose top-left is `(x, y)`.
fn mean(img: &image::GrayImage, (x, y): (u32, u32), (w, h): (u32, u32)) -> f64 {
    let mut sum = 0u64;
    for py in y..y + h {
        for px in x..x + w {
            sum += img.get_pixel(px, py).0[0] as u64;
        }
    }
    sum as f64 / (w * h) as f64
}

#[test]
fn a_lantern_that_casts_shadows_the_floor_on_every_side_of_its_table() {
    let dir = dir("casts");
    let (run, lit) = render(&dir, "lit", &lantern("lantern", ""));
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let (run, cast) = render(&dir, "cast", &lantern("lantern", "cast_shadows = true"));
    ok(&run);
    let err = stderr(&run);
    assert!(!err.contains("shadow-cap"), "one lantern casts: {err}");
    let (lit, cast) = (luma(&lit), luma(&cast));

    // Seen from above, 6 m up with a 50 degree field, the table's top covers
    // about 45 x 29 pixels at the centre. A box of floor just past each of
    // its four edges: the -Y face of the cube and, to the sides, the faces
    // either way of it all have to shadow it.
    let (cx, cy) = (WIDTH / 2, HEIGHT / 2);
    let sides = [
        ("left", (cx - 50, cy - 8), (20, 16)),
        ("right", (cx + 30, cy - 8), (20, 16)),
        ("top", (cx - 10, cy - 34), (20, 14)),
        ("bottom", (cx - 10, cy + 20), (20, 14)),
    ];
    for (side, at, size) in sides {
        let (a, b) = (mean(&lit, at, size), mean(&cast, at, size));
        eprintln!("{side}: lit {a:.1}, cast {b:.1}");
        assert!(
            b + 20.0 < a,
            "the floor {side} of the table: {a:.1} lit, {b:.1} with the shadow"
        );
    }
    // The table's own top, which faces the lantern, keeps its light
    let top = ((cx - 8, cy - 5), (16, 10));
    let (a, b) = (mean(&lit, top.0, top.1), mean(&cast, top.0, top.1));
    eprintln!("table top: lit {a:.1}, cast {b:.1}");
    assert!(
        (a - b).abs() < 2.0,
        "the table top: {a:.1} lit, {b:.1} cast"
    );

    let brighter = lit
        .pixels()
        .zip(cast.pixels())
        .filter(|(a, b)| b.0[0] as i32 > a.0[0] as i32 + 1)
        .count();
    assert_eq!(brighter, 0, "a shadow only takes light away");
}

#[test]
fn shadows_off_for_the_scene_win_over_a_point_light_that_asks() {
    let dir = dir("scene_off");
    // A replace that holds whichever line endings the checkout gave the file
    let room = ROOM.replace("[environment]", "[environment]\nshadows = false");
    assert!(room.contains("shadows = false"));
    let (run, plain) = render_room(&dir, "plain", &room, &lantern("lantern", ""));
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let (run, asked) = render_room(
        &dir,
        "asked",
        &room,
        &lantern("lantern", "cast_shadows = true"),
    );
    ok(&run);
    assert_eq!(
        std::fs::read(plain).unwrap(),
        std::fs::read(asked).unwrap(),
        "with the scene's shadows off, asking draws the same bytes"
    );
}

#[test]
fn a_second_point_light_that_asks_is_named_and_lights_without_one() {
    let dir = dir("two");
    let two = format!(
        "{}\n{}",
        lantern("a_lantern", "cast_shadows = true"),
        lantern("b_candle", "cast_shadows = true"),
    );
    let (run, _) = render(&dir, "two", &two);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let err = stderr(&run);
    assert!(
        err.contains("warning[shadow-cap] entity 'b_candle' light.cast_shadows"),
        "{err}"
    );
    assert!(err.contains("'a_lantern' does"), "{err}");
    assert!(
        !err.contains("entity 'a_lantern' light.cast_shadows"),
        "{err}"
    );
}
