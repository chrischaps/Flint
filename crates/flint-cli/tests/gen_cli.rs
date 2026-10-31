//! End-to-end tests for `flint gen`: run the real binary against the example
//! specs and check what lands on disk, what is printed, and the exit code.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn oak_spec() -> PathBuf {
    repo_root().join("specs/oak_tree.procgen.toml")
}

/// A fresh, empty scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gen_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn gen(args: &[&str]) -> Output {
    Command::new(FLINT)
        .arg("gen")
        .args(args)
        .output()
        .expect("run flint")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The stone wall spec, shrunk to 64x64 so a debug build makes it quickly.
fn small_wall_spec(dir: &Path) -> PathBuf {
    let src = std::fs::read_to_string(repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
    let small = src
        .replace("height = 512", "height = 64")
        .replace("width = 512", "width = 64");
    assert_ne!(
        src, small,
        "stone wall spec no longer sets width/height = 512"
    );
    let path = dir.join("wall.procgen.toml");
    std::fs::write(&path, small).unwrap();
    path
}

#[test]
fn mesh_spec_writes_a_glb() {
    let dir = scratch("mesh");
    let out = dir.join("tree.glb");
    let o = gen(&[s(&oak_spec()), "-o", s(&out)]);
    assert!(o.status.success(), "{}", text(&o));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..4], b"glTF", "not a GLB");
    // A GLB header records the file's own length.
    let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    assert_eq!(len, bytes.len());
}

#[test]
fn a_seed_override_is_deterministic_and_matters() {
    let dir = scratch("seed");
    let (a, b, c) = (dir.join("a.glb"), dir.join("b.glb"), dir.join("c.glb"));
    for (out, seed) in [(&a, "42"), (&b, "42"), (&c, "43")] {
        let o = gen(&[s(&oak_spec()), "-o", s(out), "--seed", seed]);
        assert!(o.status.success(), "{}", text(&o));
    }
    let (a, b, c) = (
        std::fs::read(a).unwrap(),
        std::fs::read(b).unwrap(),
        std::fs::read(c).unwrap(),
    );
    assert_eq!(a, b, "same seed, different bytes");
    assert_ne!(a, c, "a different seed changed nothing");
}

#[test]
fn texture_spec_writes_one_png_per_map() {
    let dir = scratch("texture");
    let spec = small_wall_spec(&dir);
    let o = gen(&[s(&spec), "-o", s(&dir.join("wall.png"))]);
    assert!(o.status.success(), "{}", text(&o));
    for map in ["color", "normal", "roughness"] {
        let png = dir.join(format!("wall_{map}.png"));
        let bytes = std::fs::read(&png).unwrap_or_else(|_| panic!("missing {}", png.display()));
        assert_eq!(&bytes[1..4], b"PNG");
    }
}

#[test]
fn dry_run_prints_the_estimate_and_writes_nothing() {
    let dir = scratch("dry");
    let out = dir.join("tree.glb");
    let o = gen(&[s(&oak_spec()), "-o", s(&out), "--dry-run", "--seed", "7"]);
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    for want in [
        "Estimated cost",
        "Vertices",
        "Triangles",
        "Texture bytes",
        "Gen time",
    ] {
        assert!(t.contains(want), "missing '{want}' in:\n{t}");
    }
    assert!(
        t.contains("7 (fixed)"),
        "--seed not reflected in dry run:\n{t}"
    );
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "dry run wrote files"
    );
}

#[test]
fn a_missing_spec_fails_naming_the_path() {
    let o = gen(&["no/such/thing.procgen.toml", "-o", "x.glb"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("thing.procgen.toml"), "{}", text(&o));
}

#[test]
fn a_malformed_spec_fails_with_the_parse_error_and_path() {
    let dir = scratch("malformed");
    let spec = dir.join("bad.procgen.toml");
    std::fs::write(&spec, "this is = = not toml").unwrap();
    let o = gen(&[s(&spec), "-o", s(&dir.join("x.glb"))]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(
        t.contains("bad.procgen.toml") && t.contains("parse error"),
        "{t}"
    );
}

#[test]
fn an_unknown_generator_fails_listing_the_known_ones() {
    let dir = scratch("unknown");
    let src = std::fs::read_to_string(oak_spec()).unwrap();
    let spec = dir.join("odd.procgen.toml");
    std::fs::write(&spec, src.replace("tree_v1", "no_such_gen")).unwrap();
    for extra in [&["-o", "x.glb"][..], &["--dry-run"][..]] {
        let mut args = vec![s(&spec)];
        args.extend_from_slice(extra);
        let o = gen(&args);
        assert!(!o.status.success());
        let t = text(&o);
        assert!(t.contains("odd.procgen.toml"), "{t}");
        assert!(t.contains("no_such_gen"), "{t}");
        assert!(t.contains("tree_v1") && t.contains("texture_v1"), "{t}");
    }
}

#[test]
fn an_output_that_cannot_hold_the_result_is_refused_before_generating() {
    let dir = scratch("mismatch");
    let o = gen(&[s(&oak_spec()), "-o", s(&dir.join("tree.png"))]);
    assert!(!o.status.success(), "mesh written as .png: {}", text(&o));
    assert!(text(&o).contains("tree.glb"), "no suggestion: {}", text(&o));

    let o = gen(&[
        s(&oak_spec()),
        "-o",
        s(&dir.join("tree.glb")),
        "--format",
        "png",
    ]);
    assert!(!o.status.success(), "--format png on a mesh: {}", text(&o));

    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "wrote files");
}
