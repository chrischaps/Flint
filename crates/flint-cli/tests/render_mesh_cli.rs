//! `flint render --mesh` (#222): a frame a mesh file on one renderer. These run
//! the real binary on `demo/mesh_sequence.scene.toml` with models from
//! `demo/models`; those that draw skip themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scene() -> PathBuf {
    repo_root().join("demo/mesh_sequence.scene.toml")
}

/// A folder of mesh files, named so that only a natural sort orders them right.
fn meshes(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flint-mesh-cli-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("seq")).unwrap();
    let models = repo_root().join("demo/models");
    for (from, to) in [
        ("Box.glb", "m2.glb"),
        ("Sphere.glb", "m10.glb"),
        ("Lantern.glb", "m1.glb"),
    ] {
        std::fs::copy(models.join(from), dir.join("seq").join(to)).unwrap();
    }
    dir
}

fn render(args: &[&str]) -> Output {
    Command::new(FLINT)
        .arg("render")
        .arg(scene())
        .args(["--schemas"])
        .arg(repo_root().join("schemas"))
        .args(["--no-grid", "--width", "160", "--height", "90"])
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
    assert!(
        o.status.success(),
        "flint failed: {}{}",
        String::from_utf8_lossy(&o.stdout),
        stderr(o)
    );
}

fn pixels(path: &Path) -> Vec<u8> {
    image::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .into_rgba8()
        .into_raw()
}

#[test]
fn a_frame_of_a_sequence_is_the_render_of_its_file_alone() {
    let dir = meshes("alone");
    let seq = dir.join("seq");
    let out = render(&[
        "--mesh",
        &format!("tree_a={}", seq.display()),
        "-o",
        &dir.join("f-%02d.png").to_string_lossy(),
    ]);
    if no_gpu(&out) {
        return;
    }
    ok(&out);
    // Natural order: m1 (lantern), m2 (box), m10 (sphere)
    let frames: Vec<Vec<u8>> = (0..3)
        .map(|i| pixels(&dir.join(format!("f-{i:02}.png"))))
        .collect();
    assert_ne!(frames[0], frames[1], "the lantern and the box drew alike");
    assert_ne!(frames[1], frames[2], "the box and the sphere drew alike");
    // The last frame, after two swaps, is exactly the sphere drawn on its own
    let alone = render(&[
        "--mesh",
        &format!("tree_a={}", seq.join("m10.glb").display()),
        "-o",
        &dir.join("alone.png").to_string_lossy(),
    ]);
    ok(&alone);
    assert_eq!(
        frames[2],
        pixels(&dir.join("alone_0000.png")),
        "a swapped-in mesh kept something of the ones before it"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sheet_is_one_labelled_image_and_a_gif_has_a_frame_a_file() {
    let dir = meshes("sheet");
    let pattern = format!("tree_a={}", dir.join("seq").join("m*.glb").display());
    let sheet = dir.join("sheet.png");
    let out = render(&[
        "--mesh",
        &pattern,
        "--sheet",
        "-o",
        &sheet.to_string_lossy(),
    ]);
    if no_gpu(&out) {
        return;
    }
    ok(&out);
    let (w, _) = image::image_dimensions(&sheet).unwrap();
    assert!(w > 3 * 160, "three cells across, got {w} px");

    let gif = dir.join("seq.gif");
    ok(&render(&[
        "--mesh",
        &pattern,
        "--fps",
        "2",
        "-o",
        &gif.to_string_lossy(),
    ]));
    let file = std::fs::File::open(&gif).unwrap();
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).unwrap();
    let frames = image::AnimationDecoder::into_frames(decoder)
        .collect_frames()
        .unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].delay().numer_denom_ms(), (500, 1));
    std::fs::remove_dir_all(&dir).ok();
}

/// These fail before a GPU is asked for.
#[test]
fn what_cant_be_drawn_is_refused_and_says_why() {
    let dir = meshes("refused");
    let seq = format!("tree_a={}", dir.join("seq").display());
    let png = dir.join("x.png").to_string_lossy().into_owned();
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec![
                "--mesh".into(),
                seq.clone(),
                "--mesh".into(),
                format!("tree_b={}", dir.join("seq").join("m1*.glb").display()),
            ],
            "differ in length",
        ),
        (
            vec!["--mesh".into(), seq.replace("tree_a", "nope")],
            "no entity named 'nope'",
        ),
        (
            vec!["--mesh".into(), seq.clone(), "--frames".into(), "2".into()],
            "drop --frames",
        ),
        (
            vec![
                "--mesh".into(),
                format!("tree_a={}", dir.join("none*.glb").display()),
            ],
            "matches no files",
        ),
        (vec!["--mesh".into(), "tree_a".into()], "names no entity"),
    ];
    for (args, says) in cases {
        let mut all: Vec<&str> = args.iter().map(String::as_str).collect();
        all.extend(["-o", &png]);
        let out = render(&all);
        assert!(!out.status.success(), "{all:?} rendered");
        assert!(stderr(&out).contains(says), "{all:?}: {}", stderr(&out));
    }
    std::fs::remove_dir_all(&dir).ok();
}
