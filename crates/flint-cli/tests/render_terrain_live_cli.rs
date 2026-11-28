//! A script's change to a terrain's grass and material reaches the frame
//! (#1698). Until then the bind read them once: `set_field` on `grass.*`
//! was stored and drawn nowhere. Each test renders a small meadow with
//! `--scripts`, once with a script changing a field at run time and once
//! with the scene already saying what the script set, and holds the two to
//! the same bytes (or, for a colour, to the colour in the frame). Skips
//! without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

/// A meadow: a gentle 32 m slope, grass painted everywhere (the splat
/// map's R is layer 0, the grass layer), under a low sun.
fn scene(grass_enabled: bool, tile: f32, roughness: f32, script: Option<&str>) -> String {
    let director = match script {
        Some(s) => format!("[entities.director.script]\nsource = \"{s}.rhai\"\n"),
        None => String::new(),
    };
    format!(
        r#"
[scene]
name = "Live Meadow"

[camera]
position = [0.0, 3.0, 7.0]
target = [0.0, 0.0, 0.0]
fov = 55.0

[environment]
default_lights = false

[entities.sun.light]
type = "directional"
direction = [0.4, 0.8, 0.3]
color = [1.0, 0.95, 0.85]
intensity = 3.0

[entities.island.transform]
position = [-16.0, -1.0, -16.0]
[entities.island.terrain]
heightmap = "height.png"
splat_map = "splat.png"
layer0_texture = "ground.png"
layer1_texture = "ground.png"
layer2_texture = "ground.png"
layer3_texture = "ground.png"
width = 32.0
depth = 32.0
height_scale = 2.0
chunk_resolution = 16
texture_tile = {tile:?}
roughness = {roughness:?}
"grass.enabled" = {grass_enabled}
"grass.density" = 60.0
"grass.max_distance" = 30.0
"grass.fade_start" = 25.0
"grass.blade_height" = 0.6
"grass.wind_strength" = 0.1

{director}"#
    )
}

/// The scripts: each sets a field on the terrain every frame.
const SCRIPTS: &[(&str, &str)] = &[
    (
        "red",
        r#"fn on_update() {
    let island = get_entity("island");
    set_field(island, "terrain", "grass.color_tip", [1.0, 0.05, 0.05]);
    set_field(island, "terrain", "grass.color_base", [0.8, 0.02, 0.02]);
}"#,
    ),
    (
        "off",
        r#"fn on_update() { set_field(get_entity("island"), "terrain", "grass.enabled", false); }"#,
    ),
    (
        "on",
        r#"fn on_update() { set_field(get_entity("island"), "terrain", "grass.enabled", true); }"#,
    ),
    (
        "material",
        r#"fn on_update() {
    let island = get_entity("island");
    set_field(island, "terrain", "texture_tile", 3.0);
    set_field(island, "terrain", "roughness", 0.3);
}"#,
    ),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project of its own: the maps, a striped ground texture (so a tile
/// change shows) and the scripts.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_terrain_live_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    image::GrayImage::from_fn(33, 33, |x, _| image::Luma([96 + x as u8 * 2]))
        .save(dir.join("height.png"))
        .unwrap();
    image::RgbaImage::from_pixel(33, 33, image::Rgba([255, 0, 0, 0]))
        .save(dir.join("splat.png"))
        .unwrap();
    image::RgbaImage::from_fn(16, 16, |x, y| {
        let v = if (x / 4 + y / 4) % 2 == 0 { 200 } else { 60 };
        image::Rgba([v, v, v, 255])
    })
    .save(dir.join("ground.png"))
    .unwrap();
    for (name, text) in SCRIPTS {
        std::fs::write(dir.join(format!("scripts/{name}.rhai")), text).unwrap();
    }
    dir
}

/// Render `scene` at half a second of its scripts' time.
fn render(dir: &Path, name: &str, scene: &str) -> (Output, PathBuf) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, scene).unwrap();
    let out = dir.join(format!("{name}.png"));
    let run = std::process::Command::new(FLINT)
        .arg("render")
        .arg(&path)
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .arg("--schemas")
        .arg(repo().join("packages/terrain/schemas"))
        .args(["--width", &WIDTH.to_string()])
        .args(["--height", &HEIGHT.to_string()])
        .args(["--scripts", "--time", "0.5", "--no-grid"])
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

/// Render, or `None` without a GPU; a failed render fails the test.
fn drawn(dir: &Path, name: &str, scene: &str) -> Option<image::RgbaImage> {
    let (run, out) = render(dir, name, scene);
    if no_gpu(&run) {
        return None;
    }
    assert!(run.status.success(), "{name}: {}", stderr(&run));
    assert!(
        !stderr(&run).contains("script error"),
        "{name}: {}",
        stderr(&run)
    );
    Some(image::open(out).unwrap().to_rgba8())
}

/// How many pixels are plainly red: red over both others by 40 levels.
fn red_pixels(image: &image::RgbaImage) -> usize {
    image
        .pixels()
        .filter(|p| {
            let [r, g, b, _] = p.0;
            i32::from(r) > i32::from(g) + 40 && i32::from(r) > i32::from(b) + 40
        })
        .count()
}

fn differing(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    a.pixels().zip(b.pixels()).filter(|(x, y)| x != y).count()
}

#[test]
fn a_scripts_grass_colour_is_in_the_frame() {
    let dir = project("colour");
    let Some(plain) = drawn(&dir, "plain", &scene(true, 6.0, 0.85, None)) else {
        return;
    };
    let red = drawn(&dir, "red", &scene(true, 6.0, 0.85, Some("red"))).unwrap();
    let area = (WIDTH * HEIGHT) as usize;
    assert_eq!(red_pixels(&plain), 0, "the meadow starts green");
    let reds = red_pixels(&red);
    assert!(
        reds > area / 5,
        "the script's red grass covers {reds} of {area} pixels"
    );
}

#[test]
fn a_script_turns_the_grass_off_as_a_scene_without_it_draws() {
    let dir = project("off");
    let Some(without) = drawn(&dir, "without", &scene(false, 6.0, 0.85, None)) else {
        return;
    };
    let with = drawn(&dir, "with", &scene(true, 6.0, 0.85, None)).unwrap();
    let off = drawn(&dir, "off", &scene(true, 6.0, 0.85, Some("off"))).unwrap();
    assert!(
        differing(&with, &without) > (WIDTH * HEIGHT) as usize / 5,
        "the grass is in the frame"
    );
    assert_eq!(differing(&off, &without), 0, "the script's off is no grass");
}

#[test]
fn a_script_grows_grass_a_scene_loaded_without_as_a_scene_with_it_draws() {
    let dir = project("on");
    let Some(with) = drawn(&dir, "with", &scene(true, 6.0, 0.85, None)) else {
        return;
    };
    let on = drawn(&dir, "on", &scene(false, 6.0, 0.85, Some("on"))).unwrap();
    assert_eq!(
        differing(&on, &with),
        0,
        "grown at run time, drawn as bound"
    );
}

#[test]
fn a_scripts_material_is_drawn_as_the_scene_saying_it() {
    let dir = project("material");
    let Some(authored) = drawn(&dir, "authored", &scene(false, 3.0, 0.3, None)) else {
        return;
    };
    let before = drawn(&dir, "before", &scene(false, 6.0, 0.85, None)).unwrap();
    let scripted = drawn(&dir, "scripted", &scene(false, 6.0, 0.85, Some("material"))).unwrap();
    assert!(differing(&authored, &before) > 0, "the tile shows");
    assert_eq!(differing(&scripted, &authored), 0);
}
