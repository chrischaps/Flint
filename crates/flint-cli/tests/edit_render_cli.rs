//! `flint edit <file> --render out.png` for every kind of file (#712): each
//! editor's still goes through the one editor host, so each kind writes a
//! picture of the size asked for, and a flag the kind doesn't take fails
//! before anything opens. They skip themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A texture generator's spec, small so the test is quick
const TEXTURE: &str = r##"generator = "texture_v1"

[meta]
name = "test_plaster"
description = "A small plaster for the test"
version = "1.0.0"
tags = ["texture"]

[seed]
mode = "fixed"
value = 7

[params]
pattern = "perlin_organic"
width = 64
height = 64
output_maps = ["albedo", "normal", "roughness"]
base_color = "#C8B89A"
"##;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("edit_render_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn edit(file: &Path, out: &Path, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .current_dir(repo())
        .arg("edit")
        .arg(file)
        .args(["--width", "160", "--height", "96", "--render"])
        .arg(out)
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Whether this run had no GPU to draw with, which isn't this test's business.
fn no_gpu(o: &Output) -> bool {
    if !o.status.success() && stderr(o).contains("headless render context") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return true;
    }
    false
}

/// The still's size and the widest spread (standard deviation, 0..1) of
/// its red, green and blue: a frame of one colour has none.
fn picture(path: &Path) -> ((u32, u32), f64) {
    let img = image::open(path).unwrap().to_rgb8();
    let n = (img.width() * img.height()) as f64;
    let spread = (0..3)
        .map(|c| {
            let mean = img.pixels().map(|p| p[c] as f64).sum::<f64>() / n;
            let var = img
                .pixels()
                .map(|p| (p[c] as f64 - mean).powi(2))
                .sum::<f64>()
                / n;
            var.sqrt() / 255.0
        })
        .fold(0.0, f64::max);
    (img.dimensions(), spread)
}

/// #712's done-when: a still of every kind, including the three that had
/// none (a procgen mesh, a texture, a terrain), at the size asked for and
/// not one flat colour.
#[test]
fn every_kind_of_file_renders_a_still() {
    let tmp = dir("every_kind");
    std::fs::write(tmp.join("plaster.procgen.toml"), TEXTURE).unwrap();
    let kinds: [(&str, PathBuf, &[&str]); 8] = [
        (
            "scene",
            repo().join("tests/fixtures/scenes/stylize_props.scene.toml"),
            &["--no-grid"],
        ),
        (
            "model",
            repo().join("tests/fixtures/scenes/models/barrel.glb"),
            &["--yaw", "30"],
        ),
        (
            "particles",
            repo().join("tests/fixtures/scenes/particles/campfire.particles.toml"),
            &["--anim-time", "0.5"],
        ),
        (
            "mesh",
            repo().join("tests/fixtures/scenes/woozy_grove/specs/grove_oak.procgen.toml"),
            &["--seed", "3"],
        ),
        ("texture", tmp.join("plaster.procgen.toml"), &[]),
        (
            "graph",
            repo().join("tests/fixtures/scenes/editors/specs/tavern_plank.procgen.toml"),
            &[],
        ),
        // A missing terrain spec is made from the default one, as the
        // window makes it
        ("terrain", tmp.join("new.terrain.toml"), &["--pitch", "40"]),
        (
            "track",
            repo().join("tests/fixtures/scenes/editors/sweep_garden.scene.toml"),
            &["--spline"],
        ),
    ];
    for (kind, file, args) in kinds {
        let out = tmp.join(format!("{kind}.png"));
        let o = edit(&file, &out, args);
        if no_gpu(&o) {
            return;
        }
        assert!(o.status.success(), "{kind}: {}", stderr(&o));
        let (size, spread) = picture(&out);
        assert_eq!(size, (160, 96), "{kind}: the size asked for");
        assert!(spread > 0.01, "{kind}: one flat colour (spread {spread})");
    }
}

/// The camera flags mean the same in every editor with a view: a yaw
/// turns the procgen mesh as it turns the model.
#[test]
fn a_camera_flag_moves_every_editors_view() {
    let tmp = dir("camera");
    for (kind, file) in [
        (
            "mesh",
            repo().join("tests/fixtures/scenes/woozy_grove/specs/grove_oak.procgen.toml"),
        ),
        (
            "model",
            repo().join("tests/fixtures/scenes/models/barrel.glb"),
        ),
        ("terrain", tmp.join("t.terrain.toml")),
    ] {
        let a = tmp.join(format!("{kind}-a.png"));
        let b = tmp.join(format!("{kind}-b.png"));
        let o = edit(&file, &a, &[]);
        if no_gpu(&o) {
            return;
        }
        assert!(o.status.success(), "{kind}: {}", stderr(&o));
        let o = edit(&file, &b, &["--yaw", "120", "--distance", "5"]);
        assert!(o.status.success(), "{kind}: {}", stderr(&o));
        assert_ne!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "{kind}: --yaw / --distance changed nothing"
        );
    }
}

/// A flag the kind doesn't take is an error naming what it applies to,
/// before any window or GPU (it used to be ignored).
#[test]
fn a_flag_the_editor_doesnt_take_is_an_error() {
    let tmp = dir("flags");
    std::fs::write(tmp.join("plaster.procgen.toml"), TEXTURE).unwrap();
    for (file, args, says) in [
        (
            tmp.join("plaster.procgen.toml"),
            vec!["--yaw", "30"],
            "--yaw doesn't apply to a procgen texture spec",
        ),
        (
            repo().join("tests/fixtures/scenes/models/barrel.glb"),
            vec!["--seed", "4"],
            "--seed doesn't apply to a model",
        ),
        (
            repo().join("tests/fixtures/scenes/editors/specs/tavern_plank.procgen.toml"),
            vec!["--no-grid"],
            "--no-grid doesn't apply to a texture pipeline spec",
        ),
        (
            repo().join("tests/fixtures/scenes/particles/campfire.particles.toml"),
            vec!["--clip", "Walk"],
            "--clip doesn't apply to a particle effect",
        ),
    ] {
        let o = edit(&file, &tmp.join("x.png"), &args);
        assert!(!o.status.success(), "{args:?} passed");
        assert!(stderr(&o).contains(says), "{args:?}: {}", stderr(&o));
        assert!(!tmp.join("x.png").exists(), "{args:?} rendered anyway");
    }
}
