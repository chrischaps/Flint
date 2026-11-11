//! End-to-end tests for `flint render --variant` / `--variants` / `--zoom`
//! (#291): the real binary on a lit floor, each variant's frame held equal to a
//! plain `flint render` of the same flags, and each zoom to its frame's pixels.
//! Those that need a GPU skip themselves on a machine without one.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Variant Floor"

[camera]
position = [0.0, 3.0, 4.0]
target = [0.0, 0.0, 0.0]
fov = 50.0

[entities.key.transform]
position = [0.0, 2.0, 0.0]
[entities.key.light]
type = "point"
color = [1.0, 0.8, 0.6]
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

[entities.crate]
archetype = "furniture"
[entities.crate.transform]
position = [0.5, 0.5, 0.0]
[entities.crate.bounds]
min = [-0.5, -0.5, -0.5]
max = [0.5, 0.5, 0.5]
"#;

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_variants_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("floor.scene.toml"), SCENE).unwrap();
    dir
}

fn render(dir: &Path, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("floor.scene.toml"))
        .arg("--schemas")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas"))
        .arg("--width")
        .arg("64")
        .arg("--height")
        .arg("48")
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

/// Whether this run had no GPU to draw with, which isn't this test's business.
fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success() && stderr(o).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(
        o.status.success(),
        "flint failed: {}{}",
        String::from_utf8_lossy(&o.stdout),
        stderr(o)
    );
}

fn png(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgba8()
}

#[test]
fn each_variant_is_the_render_its_flags_make_alone() {
    let dir = project("alone");
    let o = render(
        &dir,
        "cmp.png",
        &[
            "--exposure",
            "1.5",
            "--variant",
            "base:",
            "--variant",
            "dark: --exposure 0.4",
            "--variant",
            "flat: --no-shadows --no-postprocess",
            "--zoom",
            "8,8,24,20@2",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    // The variant's flags go after the base's, and the later one wins
    let plain = [
        ("base", vec!["--exposure", "1.5"]),
        ("dark", vec!["--exposure", "0.4"]),
        (
            "flat",
            vec!["--exposure", "1.5", "--no-shadows", "--no-postprocess"],
        ),
    ];
    let mut frames = Vec::new();
    for (slug, flags) in &plain {
        let alone = format!("alone-{slug}.png");
        ok(&render(&dir, &alone, flags));
        let variant = png(&dir.join(format!("cmp-{slug}.png")));
        assert_eq!(variant.dimensions(), (64, 48));
        assert!(
            variant == png(&dir.join(&alone)),
            "variant '{slug}' differs from flint render {flags:?}"
        );
        frames.push(variant);
    }
    assert!(
        frames[0] != frames[1],
        "the exposure variant changed nothing"
    );

    // The sheet holds three cells; the zooms are 2x nearest of the region
    let sheet = png(&dir.join("cmp.png"));
    assert!(sheet.width() > 3 * 64 && sheet.height() > 48);
    for (slug, frame) in ["base", "dark", "flat"].iter().zip(&frames) {
        let zoom = png(&dir.join(format!("cmp-{slug}-zoom.png")));
        assert_eq!(zoom.dimensions(), (32, 24));
        for y in 0..24 {
            for x in 0..32 {
                assert_eq!(
                    zoom.get_pixel(x, y),
                    frame.get_pixel(8 + x / 2, 8 + y / 2),
                    "{slug} zoom pixel {x},{y}"
                );
            }
        }
    }
    assert!(dir.join("cmp-zoom.png").is_file());
}

#[test]
fn a_variants_file_and_the_command_line_add_up() {
    let dir = project("file");
    std::fs::write(
        dir.join("v.toml"),
        r#"
title = "Floor"
zoom = ["0,0,16,16@1"]

[[variant]]
label = "Shadows off"
flags = ["--no-shadows"]
"#,
    )
    .unwrap();
    let file = dir.join("v.toml");
    let o = render(
        &dir,
        "out/f.png",
        &[
            "--variants",
            file.to_str().unwrap(),
            "--variant",
            "--exposure 0.5",
            "--zoom",
            "16,16,48,40",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let out = dir.join("out");
    for name in [
        "f.png",
        "f-shadows-off.png",
        "f-exposure-0-5.png",
        "f-zoom.png",
        "f-zoom2.png",
        "f-shadows-off-zoom.png",
        "f-exposure-0-5-zoom2.png",
    ] {
        assert!(out.join(name).is_file(), "{name} missing");
    }
    // No @N: as large as fits 640 across, at most 8
    assert_eq!(
        png(&out.join("f-shadows-off-zoom2.png")).dimensions(),
        (256, 192)
    );
    assert_eq!(
        png(&out.join("f-shadows-off-zoom.png")).dimensions(),
        (16, 16)
    );
}

#[test]
fn a_bad_variant_fails_before_anything_draws() {
    let dir = project("bad");
    let o = render(
        &dir,
        "bad.png",
        &[
            "--variant",
            "ok: --exposure 2",
            "--variant",
            "typo: --exposur 2",
        ],
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("typo"), "{}", stderr(&o));
    assert!(!dir.join("bad-ok.png").exists(), "the first variant drew");
    assert!(!dir.join("bad.png").exists());

    let o = render(&dir, "z.png", &["--zoom", "0,0,8,8"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--variant"), "{}", stderr(&o));

    let o = render(&dir, "far.png", &["--variant", "a:", "--zoom", "0,0,80,8"]);
    if no_gpu(&o) {
        return;
    }
    assert!(!o.status.success());
    assert!(stderr(&o).contains("64x48"), "{}", stderr(&o));
}
