//! `flint render --frame-capture` (#1085, ADR 0126), end to end: the folder
//! holds a manifest naming an image for every pass that wrote one, each
//! image is there, the last surface image is the render's output byte for
//! byte, a render without the flag draws the same output, and a render of
//! many frames is refused. They skip themselves on a machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Capture"

[post_process]
bloom_enabled = true
ssao_enabled = true

[entities.ground.transform]
position = [0, -0.1, 0]

[entities.ground.bounds]
min = [-4, 0, -4]
max = [4, 0.1, 4]

[entities.crate.transform]
position = [0, 0.5, 0]

[entities.crate.bounds]
min = [-0.5, -0.5, -0.5]
max = [0.5, 0.5, 0.5]

[entities.sun.light]
type = "directional"
direction = [0.3, 1, 0.2]
intensity = 2.0
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_frame_capture_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("capture.scene.toml"), SCENE).unwrap();
    dir
}

fn render(dir: &Path, out: &str, extra: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("capture.scene.toml"))
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args(["--width", "160", "--height", "90", "--no-grid", "-o"])
        .arg(dir.join(out))
        .args(extra)
        .output()
        .expect("run flint")
}

fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success()
        && String::from_utf8_lossy(&o.stderr).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn a_capture_is_every_passs_image_and_ends_on_the_output() {
    let dir = dir("still");
    let capture = dir.join("frame.flintframe");
    let run = render(
        &dir,
        "captured.png",
        &["--frame-capture", capture.to_str().unwrap()],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(capture.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["format"], "flintframe");
    assert_eq!(manifest["frame"]["capturing"], true);

    // Every pass that ran and wrote a texture has an image, and every image
    // the manifest names is in the folder
    let passes = |ps: &serde_json::Value, out: &mut Vec<serde_json::Value>| {
        fn walk(ps: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
            for p in ps.as_array().unwrap() {
                out.push(p.clone());
                walk(&p["passes"], out);
            }
        }
        walk(ps, out)
    };
    let mut all = Vec::new();
    passes(&manifest["passes"], &mut all);
    let images: Vec<(String, String)> = all
        .iter()
        .flat_map(|p| {
            p["images"].as_array().unwrap().iter().map(move |i| {
                (
                    p["name"].as_str().unwrap().to_string(),
                    i["file"].as_str().unwrap().to_string(),
                )
            })
        })
        .collect();
    for (_, file) in &images {
        assert!(capture.join(file).is_file(), "{file} missing");
    }
    for name in ["Scene", "SSAO", "Bloom", "Composite"] {
        assert!(
            images.iter().any(|(p, _)| p == name),
            "no image from {name}: {images:?}"
        );
    }
    assert!(
        images.iter().any(|(p, _)| p.starts_with("Shadow cascade")),
        "{images:?}"
    );
    assert_eq!(
        manifest["capture"]["images"].as_u64().unwrap() as usize,
        images.len()
    );

    // The last surface image is the output, byte for byte
    let last = images
        .iter()
        .rfind(|(_, f)| f.ends_with("-surface.png"))
        .expect("a surface image");
    assert_eq!(
        std::fs::read(capture.join(&last.1)).unwrap(),
        std::fs::read(dir.join("captured.png")).unwrap()
    );
    // and that output is the one a render without the flag writes
    ok(&render(&dir, "plain.png", &[]));
    assert_eq!(
        std::fs::read(dir.join("plain.png")).unwrap(),
        std::fs::read(dir.join("captured.png")).unwrap()
    );
}

#[test]
fn a_report_names_the_capture_of_a_spans_last_frame() {
    let dir = dir("report");
    let capture = dir.join("span.flintframe");
    let run = render(
        &dir,
        "span.png",
        &[
            "--time",
            "0.5",
            "--report",
            "json",
            "--frame-capture",
            capture.to_str().unwrap(),
        ],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["frame_capture"], capture.to_str().unwrap());
    assert!(capture.join("manifest.json").is_file());
}

#[test]
fn a_folder_holding_anything_else_is_refused() {
    let dir = dir("foreign");
    let capture = dir.join("mine");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::write(capture.join("notes.txt"), "keep me").unwrap();
    let run = render(
        &dir,
        "x.png",
        &["--frame-capture", capture.to_str().unwrap()],
    );
    if no_gpu(&run) {
        return;
    }
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("holds more than a frame capture"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(capture.join("notes.txt").is_file());
}

#[test]
fn many_frames_are_refused() {
    let dir = dir("frames");
    let run = render(
        &dir,
        "x.png",
        &["--frames", "3", "--frame-capture", "f.flintframe"],
    );
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("record one frame"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// #1290: a capture made in another repository (a game's, with a
/// `flint.toml` of its own) names the engine this binary was built from as
/// `engine.commit`, and the game's HEAD only as `project.commit`.
#[test]
fn a_capture_in_a_games_repository_names_the_engine_it_was_built_from() {
    let Some(built) = git(&repo(), &["rev-parse", "HEAD"]) else {
        eprintln!("the engine isn't in a git checkout: skipped");
        return;
    };
    // Outside the engine's checkout, so nothing above it is the engine's
    let game = std::env::temp_dir().join(format!("flint_1290_game_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&game);
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("capture.scene.toml"), SCENE).unwrap();
    std::fs::write(game.join("flint.toml"), "[project]\nid = \"g\"\n").unwrap();
    git(&game, &["init", "-q"]).expect("git init");
    git(&game, &["add", "."]).unwrap();
    git(&game, &["commit", "-q", "-m", "a game"]).unwrap();
    let game_head = git(&game, &["rev-parse", "HEAD"]).unwrap();

    let capture = game.join("c.flintframe");
    let run = render(
        &game,
        "x.png",
        &["--frame-capture", capture.to_str().unwrap()],
    );
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(capture.join("manifest.json")).unwrap())
            .unwrap();
    let engine = &manifest["engine"];
    let project = &manifest["project"];
    assert_ne!(engine["commit"], game_head.as_str(), "{manifest:#}");
    // The test binary was built from this checkout; HEAD is that commit
    // unless a commit landed between the build and the run
    assert_eq!(engine["commit"], built.as_str(), "{engine}");
    assert!(engine.get("subdir").is_none(), "{engine}");
    assert_eq!(project["commit"], game_head.as_str(), "{project}");
    assert_eq!(project["dirty"], false, "{project}");

    // And `flint frame show` says both
    let show = std::process::Command::new(FLINT)
        .args(["frame", "show"])
        .arg(&capture)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&show.stdout);
    let short = |c: &str| c.chars().take(10).collect::<String>();
    assert!(text.contains(&format!("at {}", short(&built))), "{text}");
    assert!(
        text.contains(&format!("project at {}", short(&game_head))),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&game);
}
