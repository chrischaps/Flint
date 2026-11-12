//! End-to-end tests for `flint render --scripts` and `--expression-timeline`
//! (#8): a scene's Rhai scripts run headless on the fixed frame step, hearing a
//! timeline, and the capture shows what they did. Run the real binary on a lit
//! floor whose light a script tints by `expression().state`. Those that need a GPU
//! skip themselves on a machine without one.

use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Tinted Floor"

[camera]
position = [0.0, 3.0, 4.0]
target = [0.0, 0.0, 0.0]
fov = 50.0

[entities.key.transform]
position = [0.0, 2.0, 0.0]
[entities.key.light]
type = "point"
color = [1.0, 1.0, 1.0]
intensity = 30.0
range = 10.0

[entities.floor]
archetype = "floor"
[entities.floor.bounds]
min = [-5.0, -0.1, -5.0]
max = [5.0, 0.0, 5.0]
[entities.floor.material]
color = [0.8, 0.8, 0.8, 1.0]
roughness = 0.9

[entities.director.script]
source = "tint.rhai"
"#;

/// Each state its own colour of light, and a flicker from `random()` so the
/// seeded sequence is in the pixels too.
const TINT: &str = r#"
fn on_update() {
    let key = get_entity("key");
    let e = expression();
    let c = if e.len() == 0 { [1.0, 1.0, 1.0] }
        else if e.state == "listening" { [0.1, 0.2, 1.0] }
        else if e.state == "thinking" { [1.0, 0.1, 0.1] }
        else if e.state == "speaking" { [0.1, 1.0, 0.1] }
        else { [1.0, 1.0, 1.0] };
    set_field(key, "light", "color", c);
    set_field(key, "light", "intensity", 28.0 + 4.0 * random());
}
"#;

const TIMELINE: &str = r#"{"events": [
  {"state": "listening", "t": 0.0},
  {"state": "thinking", "t": 1.0},
  {"state": "speaking", "t": 2.0}
]}"#;

/// A project holding the scene, its script and a timeline, in a directory of its own.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_scripts_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("floor.scene.toml"), SCENE).unwrap();
    std::fs::write(dir.join("scripts/tint.rhai"), TINT).unwrap();
    std::fs::write(dir.join("one-turn.timeline.json"), TIMELINE).unwrap();
    dir
}

/// The engine's set. The scenes here are written in the examples'
/// room-builder archetypes (`floor`), so each render also names
/// `schemas/examples`: a `--schemas` flag is the whole list (#681).
fn schemas() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

fn render(dir: &Path, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("floor.scene.toml"))
        .arg("--schemas")
        .arg(schemas())
        .arg("--schemas")
        .arg(schemas().join("examples"))
        .arg("--width")
        .arg("64")
        .arg("--height")
        .arg("48")
        .arg("-o")
        .arg(dir.join(out))
        .args(args)
        .output()
        .expect("run flint")
}

fn timeline(dir: &Path) -> String {
    dir.join("one-turn.timeline.json")
        .to_string_lossy()
        .into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
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

fn gif_frames(path: &Path) -> Vec<image::RgbaImage> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    GifDecoder::new(file)
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap()
        .into_iter()
        .map(|frame| frame.into_buffer())
        .collect()
}

/// The image's summed red, green and blue.
fn sums(image: &image::RgbaImage) -> [u64; 3] {
    let mut sum = [0u64; 3];
    for pixel in image.pixels() {
        for (s, c) in sum.iter_mut().zip(pixel.0) {
            *s += u64::from(c);
        }
    }
    sum
}

/// Which of red, green and blue the image has most of.
fn strongest(image: &image::RgbaImage) -> usize {
    let sum = sums(image);
    (0..3).max_by_key(|&i| sum[i]).unwrap()
}

const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;

#[test]
fn each_state_tints_the_light_at_its_time() {
    let dir = project("tints");
    let tl = timeline(&dir);
    // Frames at 0, 0.5, 1, ... 2.5 seconds.
    let o = render(
        &dir,
        "turn.gif",
        &["--expression-timeline", &tl, "--time", "0..3", "--fps", "2"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let frames = gif_frames(&dir.join("turn.gif"));
    assert_eq!(frames.len(), 6);
    let colours: Vec<usize> = frames.iter().map(strongest).collect();
    // Listening from the first frame, held until thinking at 1 s, then speaking at 2.
    assert_eq!(colours, [BLUE, BLUE, RED, RED, GREEN, GREEN]);
}

#[test]
fn the_same_timeline_gives_the_same_bytes() {
    let dir = project("same_bytes");
    let tl = timeline(&dir);
    let args = [
        "--expression-timeline",
        &tl,
        "--time",
        "0..2.5",
        "--fps",
        "4",
    ];
    let first = render(&dir, "a.gif", &args);
    if no_gpu(&first) {
        return;
    }
    ok(&first);
    ok(&render(&dir, "b.gif", &args));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    // Not assert_eq!, which would print both files.
    assert!(
        read("a.gif") == read("b.gif"),
        "two runs drew different pixels"
    );

    // And the scripts are what made the picture: without them the light is white.
    ok(&render(
        &dir,
        "plain.gif",
        &["--time", "0..2.5", "--fps", "4"],
    ));
    assert!(read("a.gif") != read("plain.gif"));
    let plain = gif_frames(&dir.join("plain.gif"));
    let [r, g, b] = sums(&plain[0]);
    assert!(r.abs_diff(b) * 20 < r, "white light, so grey: {r} {g} {b}");
}

#[test]
fn a_still_at_a_time_hears_everything_due_by_then() {
    let dir = project("still");
    let tl = timeline(&dir);
    let o = render(
        &dir,
        "t.png",
        &["--expression-timeline", &tl, "--time", "1.5"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let image = image::open(dir.join("t.png")).unwrap().into_rgba8();
    assert_eq!(strongest(&image), RED);
}

#[test]
fn a_list_of_moments_is_one_run_with_a_still_each() {
    let dir = project("list");
    let tl = timeline(&dir);
    let o = render(
        &dir,
        "at.png",
        &["--expression-timeline", &tl, "--time", "0.5,1.5,2.5"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    for (i, (moment, colour)) in [("0.5", BLUE), ("1.5", RED), ("2.5", GREEN)]
        .into_iter()
        .enumerate()
    {
        let still = format!("at_{i:04}.png");
        let image = image::open(dir.join(&still)).unwrap().into_rgba8();
        assert_eq!(strongest(&image), colour, "{still} at {moment} s");
        // The same bytes as the moment rendered alone, seeded flicker and all.
        let alone = format!("alone_{i}.png");
        ok(&render(
            &dir,
            &alone,
            &["--expression-timeline", &tl, "--time", moment],
        ));
        assert!(
            read(&still) == read(&alone),
            "{still} differs from --time {moment} alone"
        );
    }
    assert!(
        !dir.join("at.png").exists(),
        "a list writes only its numbered stills"
    );
}

#[test]
fn without_scripts_a_scene_with_one_says_so() {
    let dir = project("warns");
    let o = render(&dir, "t.png", &["--time", "0.5"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    assert!(stderr(&o).contains("--scripts"), "{}", stderr(&o));
}

const SAPLING: &str = r#"
generator = "tree_v1"
[meta]
name = "sapling"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 7
pose_ring = 4
"#;

const GROVE: &str = r#"
[scene]
name = "A Sapling"

[wooze]
step = 0.5
glide = 0.7

[camera]
position = [0.0, 4.5, 11.0]
target = [0.0, 4.0, 0.0]
fov = 50.0

[entities.sun.light]
type = "directional"
direction = [0.6, 0.5, 0.4]
color = [1.0, 0.9, 0.8]
intensity = 2.5

[entities.tree.transform]
position = [0.0, 0.0, 0.0]
[entities.tree.model]
asset = "sapling"
[entities.tree.living_prop]
lineage_seed = 3

[entities.director.script]
source = "director.rhai"
"#;

/// A living prop and a director script, as the Embodiment's behaviors drive one.
fn grove(name: &str, director: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_scripts_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::create_dir_all(dir.join("specs")).unwrap();
    std::fs::write(dir.join("specs/sapling.procgen.toml"), SAPLING).unwrap();
    std::fs::write(dir.join("grove.scene.toml"), GROVE).unwrap();
    std::fs::write(dir.join("scripts/director.rhai"), director).unwrap();
    dir
}

fn render_grove(dir: &Path, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("grove.scene.toml"))
        .arg("--schemas")
        .arg(schemas())
        .arg("--schemas")
        .arg(schemas().join("examples"))
        .args([
            "--width", "96", "--height", "72", "--time", "0..2", "--fps", "4",
        ])
        .arg("-o")
        .arg(dir.join(out))
        .args(args)
        .output()
        .expect("run flint")
}

#[test]
fn a_script_tunes_living_props_and_freezes_them() {
    // Quick boil: a new pose every 0.1 s instead of the scene's 0.5.
    let quick = grove(
        "tune",
        "fn on_init() { tune_living(#{ wooze: #{ step: 0.1 } }); }",
    );
    let first = render_grove(&quick, "quick.gif", &["--scripts"]);
    if no_gpu(&first) {
        return;
    }
    ok(&first);
    assert!(
        !stderr(&first).contains("tune_living:"),
        "{}",
        stderr(&first)
    );
    ok(&render_grove(&quick, "again.gif", &["--scripts"]));
    ok(&render_grove(&quick, "plain.gif", &[]));
    let read = |dir: &Path, name: &str| std::fs::read(dir.join(name)).unwrap();
    // Not assert_eq!, which would print both files.
    assert!(
        read(&quick, "quick.gif") == read(&quick, "again.gif"),
        "two runs of a tune drew different pixels"
    );
    assert!(
        read(&quick, "quick.gif") != read(&quick, "plain.gif"),
        "the tune changed nothing"
    );

    // Frozen from the first frame: every frame is the first.
    let held = grove("freeze", "fn on_init() { freeze_living(true); }");
    ok(&render_grove(&held, "held.gif", &["--scripts"]));
    let frames = gif_frames(&held.join("held.gif"));
    assert!(frames.len() > 1);
    assert!(
        frames.iter().all(|frame| frame == &frames[0]),
        "a frozen grove moved"
    );
    ok(&render_grove(&held, "moving.gif", &[]));
    let moving = gif_frames(&held.join("moving.gif"));
    assert!(
        moving.iter().any(|frame| frame != &moving[0]),
        "the grove never moves"
    );
}

#[test]
fn the_report_says_what_each_capture_heard_and_what_didnt_run() {
    let dir = project("report");
    // A script that also asks for what a capture can't do.
    std::fs::write(
        dir.join("scripts/tint.rhai"),
        format!("{TINT}\nfn on_init() {{ load_scene(\"elsewhere.scene.toml\"); }}\n"),
    )
    .unwrap();
    let tl = timeline(&dir);
    let o = render(
        &dir,
        "r.gif",
        &[
            "--expression-timeline",
            &tl,
            "--time",
            "0..3",
            "--fps",
            "1",
            "--report",
            "json",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value =
        serde_json::from_slice(&o.stdout).expect("stdout is only the report");
    let heard: Vec<&str> = report["captures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["expression"].as_str().unwrap())
        .collect();
    assert_eq!(heard, ["listening", "thinking", "speaking"]);
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("load_scene"), "{warnings}");
    assert!(warnings.contains("one scene"), "says why: {warnings}");
}

// #697: a capture is a script host like the player, so the style commands
// the old headless dispatcher dropped land in the picture.

/// The floor drawn in ASCII from `on_init`, its cells easing from 4 to 16
/// points over two seconds.
const STYLER: &str = r#"
fn on_init() {
    let floor = get_entity("floor");
    set_entity_style(floor, "ascii");
    set_style_param(floor, "cell", 4);
    tween_style_param(floor, "cell", 16, 2.0, "linear");
}
"#;

#[test]
fn entity_styles_and_their_tweens_reach_the_capture() {
    let dir = project("styles");
    std::fs::write(dir.join("scripts/tint.rhai"), STYLER).unwrap();
    let o = render(&dir, "plain.png", &["--time", "0"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let o = render(&dir, "styled0.png", &["--scripts", "--time", "0"]);
    ok(&o);
    let said = stderr(&o);
    assert!(!said.contains("doesn't do it"), "{said}");
    ok(&render(&dir, "styled2.png", &["--scripts", "--time", "2"]));
    ok(&render(&dir, "again2.png", &["--scripts", "--time", "2"]));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert!(
        read("styled0.png") != read("plain.png"),
        "set_entity_style changed nothing"
    );
    assert!(
        read("styled2.png") != read("styled0.png"),
        "the tween didn't move the cells"
    );
    assert!(
        read("styled2.png") == read("again2.png"),
        "two runs differed"
    );
}

// #579: the boil, step_living and particles from scripts.

/// A block on a floor under the shading boil, which nothing else moves: what
/// changes from frame to frame is the paint and ink re-deciding themselves on the
/// field's beat. `{boil}` is both passes' amount.
const BOILING: &str = r#"
[scene]
name = "A Boiling Block"

[wooze]
step = 0.25

[camera]
position = [2.5, 2.0, 4.0]
target = [0.0, 0.5, 0.0]
fov = 45.0

[post_process]
boil_on = 1

[post_process.styles.kuwahara]
radius = 4
boil = {boil}

[post_process.styles.contour]
width = 1.5
boil = {boil}

[entities.sun.light]
type = "directional"
direction = [0.5, 0.8, 0.3]
color = [1.0, 0.95, 0.9]
intensity = 3.0

[entities.floor]
archetype = "floor"
[entities.floor.bounds]
min = [-5.0, -0.1, -5.0]
max = [5.0, 0.0, 5.0]
[entities.floor.material]
color = [0.7, 0.7, 0.65, 1.0]
roughness = 0.9

[entities.block]
archetype = "floor"
[entities.block.bounds]
min = [-0.5, 0.0, -0.5]
max = [0.5, 1.0, 0.5]
[entities.block.material]
color = [0.8, 0.3, 0.2, 1.0]
roughness = 0.6

[entities.director.script]
source = "director.rhai"
"#;

fn boiling(name: &str, boil: f32, director: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_scripts_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let scene = BOILING.replace("{boil}", &format!("{boil:.1}"));
    std::fs::write(dir.join("block.scene.toml"), scene).unwrap();
    std::fs::write(dir.join("scripts/director.rhai"), director).unwrap();
    dir
}

fn render_block(dir: &Path, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("block.scene.toml"))
        .arg("--schemas")
        .arg(schemas())
        .arg("--schemas")
        .arg(schemas().join("examples"))
        .args([
            "--width", "96", "--height", "72", "--time", "0..2", "--fps", "4",
        ])
        .arg("-o")
        .arg(dir.join(out))
        .args(args)
        .output()
        .expect("run flint")
}

fn all_alike(frames: &[image::RgbaImage]) -> bool {
    frames.iter().all(|frame| frame == &frames[0])
}

#[test]
fn a_script_switches_the_boil_off_and_on() {
    // The scene boils, and a script switches it off from the first frame.
    let off = boiling(
        "boil_off",
        1.0,
        "fn on_init() { tune_living(#{ boil: #{ enabled: false } }); }",
    );
    let o = render_block(&off, "off.gif", &["--scripts"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = stderr(&o);
    assert!(!said.contains("tune_living"), "{said}");
    let held = gif_frames(&off.join("off.gif"));
    assert!(held.len() > 1);
    assert!(
        all_alike(&held),
        "the boil was switched off and still moved"
    );
    ok(&render_block(&off, "boiling.gif", &[]));
    let boiling_frames = gif_frames(&off.join("boiling.gif"));
    assert!(
        !all_alike(&boiling_frames),
        "the scene's own boil never moves"
    );

    // A still scene, and a script gives it a boil.
    let on = boiling(
        "boil_on",
        0.0,
        "fn on_init() { tune_living(#{ boil: #{ kuwahara: 1.0, contour: 1.0, on: 2 } }); }",
    );
    ok(&render_block(&on, "plain.gif", &[]));
    assert!(all_alike(&gif_frames(&on.join("plain.gif"))));
    ok(&render_block(&on, "a.gif", &["--scripts"]));
    ok(&render_block(&on, "b.gif", &["--scripts"]));
    let read = |name: &str| std::fs::read(on.join(name)).unwrap();
    assert!(
        read("a.gif") == read("b.gif"),
        "two runs boiled differently"
    );
    assert!(
        !all_alike(&gif_frames(&on.join("a.gif"))),
        "the script's boil never moved"
    );
}

#[test]
fn step_living_moves_a_frozen_grove_a_beat_at_a_time() {
    // Frozen from the first frame, and stepped a beat each second.
    let stepped = grove(
        "step",
        r#"
let frames = 0;
fn on_init() { freeze_living(true); }
fn on_update() {
    frames += 1;
    if frames % 60 == 0 { step_living(); }
}
"#,
    );
    let o = render_grove(&stepped, "a.gif", &["--scripts"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = stderr(&o);
    assert!(!said.contains("StepLiving"), "{said}");
    ok(&render_grove(&stepped, "b.gif", &["--scripts"]));
    let read = |name: &str| std::fs::read(stepped.join(name)).unwrap();
    assert!(
        read("a.gif") == read("b.gif"),
        "two runs stepped differently"
    );
    // 0, 0.25 ... 1.75 s: held for the first second, a beat on from then.
    let frames = gif_frames(&stepped.join("a.gif"));
    assert_eq!(frames.len(), 8);
    assert!(
        all_alike(&frames[..4]),
        "a frozen grove moved before its step"
    );
    assert!(frames[4] != frames[0], "step_living didn't move the grove");
    assert!(all_alike(&frames[4..]), "the grove moved on after one step");
}

const SPARKS: &str = r#"
name = "sparks"
seed = 3

[[emitters]]
name = "sparks"
emission_rate = 80.0
max_particles = 200
lifetime = [0.6, 1.0]
speed = [1.5, 2.5]
direction = [0, 1, 0]
spread = 30.0
gravity = [0, -1.0, 0]
blend_mode = "additive"
size = { start = [0.12, 0.12], end = [0.04, 0.04] }
color = { start = [1.0, 0.8, 0.2, 1.0], end = [1.0, 0.2, 0.0, 0.0] }
"#;

#[test]
fn a_script_plays_a_particle_effect() {
    let dir = project("sparks");
    std::fs::create_dir_all(dir.join("particles")).unwrap();
    std::fs::write(dir.join("particles/sparks.particles.toml"), SPARKS).unwrap();
    // Played half a second in, and stopped a second later.
    std::fs::write(
        dir.join("scripts/tint.rhai"),
        r#"
let fx = -1;
let frames = 0;
fn on_update() {
    frames += 1;
    if frames == 30 { fx = play_effect("sparks", 0.0, 0.5, 0.0); }
    if frames == 90 { stop_effect(fx); }
}
"#,
    )
    .unwrap();
    let at = |t: &str, out: &str, scripts: bool| {
        let mut args = vec!["--time", t];
        if scripts {
            args.push("--scripts");
        }
        render(&dir, out, &args)
    };
    let o = at("1", "a.png", true);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = stderr(&o);
    assert!(!said.contains("PlayEffect"), "{said}");
    assert!(!said.contains("play_effect:"), "{said}");
    ok(&at("1", "b.png", true));
    ok(&at("1", "plain.png", false));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert!(
        read("a.png") == read("b.png"),
        "two runs sparked differently"
    );
    assert!(
        read("a.png") != read("plain.png"),
        "the effect drew nothing"
    );
    // Before it's played the picture is the plain one, and well after it's
    // stopped, with every spark spent, it is again.
    ok(&at("0.25", "early.png", true));
    ok(&at("0.25", "early_plain.png", false));
    assert!(read("early.png") == read("early_plain.png"));
    ok(&at("3", "late.png", true));
    ok(&at("3", "late_plain.png", false));
    assert!(
        read("late.png") == read("late_plain.png"),
        "sparks outlived their stop"
    );

    // A still with no clock draws no particles, and says so.
    std::fs::write(
        dir.join("scripts/tint.rhai"),
        r#"fn on_init() { play_effect("sparks", 0.0, 0.5, 0.0); }"#,
    )
    .unwrap();
    let o = render(&dir, "still.png", &["--scripts"]);
    ok(&o);
    assert!(stderr(&o).contains("--particle-time"), "{}", stderr(&o));
}

#[test]
fn a_script_loads_a_living_preset() {
    let dir = grove("preset", r#"fn on_init() { load_living_preset("quick"); }"#);
    std::fs::create_dir_all(dir.join("wooze")).unwrap();
    std::fs::write(dir.join("wooze/quick.wooze.toml"), "[wooze]\nstep = 0.1\n").unwrap();
    let o = render_grove(&dir, "a.gif", &["--scripts"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = stderr(&o);
    assert!(!said.contains("LoadLivingPreset"), "{said}");
    assert!(!said.contains("load_living_preset:"), "{said}");
    ok(&render_grove(&dir, "b.gif", &["--scripts"]));
    ok(&render_grove(&dir, "plain.gif", &[]));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert!(read("a.gif") == read("b.gif"), "two runs differed");
    assert!(
        read("a.gif") != read("plain.gif"),
        "the preset changed nothing"
    );

    // The same as the tune it holds.
    std::fs::write(
        dir.join("scripts/director.rhai"),
        "fn on_init() { tune_living(#{ wooze: #{ step: 0.1 } }); }",
    )
    .unwrap();
    ok(&render_grove(&dir, "tuned.gif", &["--scripts"]));
    assert!(
        read("a.gif") == read("tuned.gif"),
        "the preset isn't its tune"
    );
}

/// A marker drawn in ASCII at the scene's cell size, which a script grows
/// mid-span and reads back each frame (#725).
const READS_SCENE: &str = r#"
[scene]
name = "A Style Read Back"

[camera]
position = [0.0, 3.0, 4.0]
target = [0.0, 0.0, 0.0]

[post_process.styles.ascii]
enabled = false
cell = 8.0

[entities.marker]
archetype = "floor"
[entities.marker.bounds]
min = [-0.5, 0.0, -0.5]
max = [0.5, 1.0, 0.5]

[entities.director.script]
source = "reads.rhai"
"#;

const READS: &str = r#"
let frames = 0;
let said = false;
fn on_init() { set_entity_style(get_entity("marker"), "ascii"); }
fn on_update() {
    frames += 1;
    if frames == 30 { set_post_style_param("ascii", "cell", 20.0); }
    let cell = get_style_param(get_entity("marker"), "cell");
    if type_of(cell) != "()" && cell > 19.0 && !said {
        said = true;
        log_warn("the read caught up on frame " + frames);
    }
}
"#;

#[test]
fn a_style_read_sees_a_change_the_step_after_it_as_the_player_does() {
    // Before #725 a capture handed scripts the styles it drew with only at a
    // capture, so a read between two pictures was the last picture's.
    let dir = project("style_reads");
    std::fs::write(dir.join("floor.scene.toml"), READS_SCENE).unwrap();
    std::fs::write(dir.join("scripts/reads.rhai"), READS).unwrap();
    let o = render(&dir, "reads.png", &["--scripts", "--time", "1"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = stderr(&o);
    assert!(said.contains("the read caught up on frame 31"), "{said}");
}

#[test]
fn a_state_that_pauses_living_stills_the_grove_as_in_the_player() {
    // #727's policies by name, read by the player's living subsystem, which
    // a capture runs too (#725): a state that pauses `living` holds the
    // grove's clock, and one that pauses only scripts doesn't.
    let held = grove(
        "policy_living",
        r#"fn on_init() { register_state("still", #{ living: "pause" }); push_state("still"); }"#,
    );
    let o = render_grove(&held, "held.gif", &["--scripts"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let frames = gif_frames(&held.join("held.gif"));
    assert!(frames.len() > 1);
    assert!(
        all_alike(&frames),
        "a state pausing `living` let the grove move"
    );

    let moving = grove(
        "policy_scripts",
        r#"fn on_init() { register_state("quiet", #{ scripts: "pause" }); push_state("quiet"); }"#,
    );
    ok(&render_grove(&moving, "moving.gif", &["--scripts"]));
    assert!(
        !all_alike(&gif_frames(&moving.join("moving.gif"))),
        "pausing scripts stilled the grove"
    );
}
