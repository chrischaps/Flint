//! `flint render --load-report` (#1092), end to end: a scene with a model
//! says where its load went, on the still path as a table and under
//! `--report json` as the report's `load`, whose stages and "not named"
//! add up to its total and whose slowest assets name the model. Without
//! the flag the report has no `load`. They skip themselves on a machine
//! without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Load"

[entities.card.transform]
position = [0, 0.5, 0]

[entities.card.model]
asset = "card"

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
        .join("render_load_report_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::write(dir.join("load.scene.toml"), SCENE).unwrap();
    std::fs::copy(
        repo().join("tests/fixtures/models/DoubleSidedCard.glb"),
        dir.join("models/card.glb"),
    )
    .unwrap();
    dir
}

fn render(dir: &Path, extra: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("load.scene.toml"))
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args(["--width", "160", "--height", "90", "--no-grid", "-o"])
        .arg(dir.join("out.png"))
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

fn stage<'a>(load: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    load["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["stage"] == name)
}

#[test]
fn the_report_carries_the_loads_stages_and_its_slowest_assets() {
    let dir = dir("json");
    let o = render(&dir, &["--load-report", "--report", "json"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("one JSON document");
    let load = &report["load"];
    assert_eq!(load["kind"], "render", "{load:#}");
    assert!(load["name"].as_str().unwrap().ends_with("load.scene.toml"));
    for name in [
        "scene read",
        "gpu device",
        "renderer",
        "models",
        "pipelines",
        "first frame",
    ] {
        assert!(stage(load, name).is_some(), "a {name} row: {load:#}");
    }
    assert_eq!(stage(load, "models").unwrap()["count"], 1);
    let total = load["total_ms"].as_f64().unwrap();
    let named: f64 = load["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["ms"].as_f64().unwrap())
        .sum();
    let unnamed = load["unnamed_ms"].as_f64().unwrap();
    assert!(
        (named + unnamed - total).abs() < 0.2,
        "the rows and what nothing named add up to the total: {load:#}"
    );
    let slowest = load["slowest"].as_array().unwrap();
    assert_eq!(slowest.len(), 5, "{load:#}");
    let ms: Vec<f64> = slowest.iter().map(|a| a["ms"].as_f64().unwrap()).collect();
    assert!(
        ms.windows(2).all(|w| w[0] >= w[1]),
        "slowest first: {load:#}"
    );
    assert!(slowest
        .iter()
        .all(|a| a["kind"].is_string() && a["name"].is_string()));
    // The card is named among them, or each of them took longer than it
    let card = stage(load, "models").unwrap()["ms"].as_f64().unwrap();
    let named = slowest
        .iter()
        .any(|a| a["kind"] == "model" && a["name"].as_str().unwrap().ends_with("card.glb"));
    assert!(named || ms[4] >= card, "{load:#}");
    // The table goes to stderr beside the JSON, never into it
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("load (render) "), "{stderr}");
}

#[test]
fn a_still_prints_the_table_and_no_flag_means_no_load() {
    let dir = dir("text");
    let o = render(&dir, &["--load-report"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("ms to the first capture"), "{stdout}");
    assert!(stdout.contains("not named"), "{stdout}");
    assert!(stdout.contains("slowest:"), "{stdout}");

    let o = render(&dir, &["--report", "json"]);
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("one JSON document");
    assert!(report.get("load").is_none(), "{report:#}");
    assert!(!String::from_utf8_lossy(&o.stderr).contains("load (render)"));
}
