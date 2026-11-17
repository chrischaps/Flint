//! `flint render --frame-dump` (#1084), end to end: the file holds the
//! rendered frame's passes and draws by entity, a `--report json` run names
//! it, and a render of many frames is refused. They skip themselves on a
//! machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Dump"

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
        .join("render_frame_dump_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dump.scene.toml"), SCENE).unwrap();
    dir
}

fn render(dir: &Path, extra: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("dump.scene.toml"))
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args(["--width", "160", "--height", "90", "--no-grid", "-o"])
        .arg(dir.join("dump.png"))
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

fn read(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the dump was written")).unwrap()
}

#[test]
fn a_still_is_described_pass_by_pass_with_its_draws_by_name() {
    let dir = dir("still");
    let dump = dir.join("frame.json");
    let run = render(&dir, &["--frame-dump", dump.to_str().unwrap()]);
    if no_gpu(&run) {
        return;
    }
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let d = read(&dump);
    assert_eq!(d["format"], "flintframe");
    assert_eq!(d["version"], 1);
    let passes = d["passes"].as_array().unwrap();
    let names: Vec<&str> = passes.iter().map(|p| p["name"].as_str().unwrap()).collect();
    for name in ["Shadows", "Scene", "Composite"] {
        assert!(names.contains(&name), "no {name} in {names:?}");
    }
    let scene = passes.iter().find(|p| p["name"] == "Scene").unwrap();
    let drawn: Vec<&str> = scene["draws"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dr| dr["entity"].as_str().unwrap())
        .collect();
    assert_eq!(drawn, ["crate", "ground"]);
    assert_eq!(
        scene["draws"].as_array().unwrap().len() as u64,
        d["stats"]["draw_calls"].as_u64().unwrap()
    );
    let grass = passes.iter().find(|p| p["name"] == "Grass").unwrap();
    assert_eq!(grass["ran"], false);
    assert_eq!(grass["skipped"], "no grass in the scene");
}

#[test]
fn a_report_names_the_dump_of_a_spans_last_frame() {
    let dir = dir("report");
    let dump = dir.join("frame.json");
    let run = render(
        &dir,
        &[
            "--time",
            "0.5",
            "--report",
            "json",
            "--frame-dump",
            dump.to_str().unwrap(),
        ],
    );
    if no_gpu(&run) {
        return;
    }
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["frame_dump"], dump.to_str().unwrap());
    assert!(read(&dump)["passes"]
        .as_array()
        .is_some_and(|p| !p.is_empty()));
}

#[test]
fn many_frames_are_refused() {
    let dir = dir("frames");
    let run = render(&dir, &["--frames", "3", "--frame-dump", "f.json"]);
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("--frame-capture record one frame"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
