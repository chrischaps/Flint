//! `flint render --list-styles [--json]` (#303): the real binary lists the
//! registered styles with their stages and parameter ranges, and the JSON
//! parses. No GPU: the list comes from the descriptors alone.

use std::path::Path;
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn run(args: &[&str]) -> String {
    let out = Command::new(FLINT)
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "flint {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn text_lists_kuwahara_and_invert_with_stages_and_ranges() {
    let text = run(&["render", "--list-styles"]);
    assert!(
        text.contains("kuwahara (Kuwahara) [hdr, builtin]"),
        "{text}"
    );
    assert!(text.contains("4  [1 .. 8, integer]"), "{text}");
    assert!(
        text.contains("invert (Invert) [display, builtin]"),
        "{text}"
    );
    assert!(text.contains("strength  1  [0 .. 1]"), "{text}");
}

/// #741: `--list-effects` is the post keys' sibling list: every built-in
/// effect and each `[post_process]` key with its kind, default and range,
/// as text and as JSON.
#[test]
fn list_effects_names_every_post_key_with_its_range() {
    let text = run(&["render", "--list-effects"]);
    assert!(text.contains("bloom  (Bloom)"), "{text}");
    assert!(text.contains("vignette_smoothness"), "{text}");
    assert!(text.contains("0.25..5"), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&run(&["render", "--list-effects", "--json"])).unwrap();
    let effects = json.as_array().unwrap();
    let keys: Vec<&str> = effects
        .iter()
        .flat_map(|e| e["params"].as_array().unwrap())
        .map(|p| p["key"].as_str().unwrap())
        .collect();
    for key in [
        "exposure",
        "bloom_intensity",
        "fog_color",
        "ssao_samples",
        "fxaa",
    ] {
        assert!(keys.contains(&key), "{key} in {keys:?}");
    }
    let bloom = effects.iter().find(|e| e["name"] == "bloom").unwrap();
    let intensity = &bloom["params"][1];
    assert_eq!(intensity["key"], "bloom_intensity");
    assert_eq!(intensity["kind"], "f32");
    assert_eq!(intensity["max"], 1.0);
    assert!((intensity["default"].as_f64().unwrap() - 0.04).abs() < 1e-6);
    // --json goes with one list or the other, and the two don't mix
    let both = Command::new(FLINT)
        .args(["render", "--list-styles", "--list-effects"])
        .output()
        .unwrap();
    assert!(!both.status.success());
}

#[test]
fn json_parses_and_has_ranges() {
    let json: serde_json::Value =
        serde_json::from_str(&run(&["render", "--list-styles", "--json"])).unwrap();
    let styles = json.as_array().unwrap();
    let style = |name: &str| {
        styles
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let kuwahara = style("kuwahara");
    assert_eq!(kuwahara["stage"], "hdr");
    let radius = &kuwahara["params"][0];
    assert_eq!(radius["key"], "radius");
    assert_eq!(radius["min"], 1.0);
    assert_eq!(radius["max"], 8.0);
    let invert = style("invert");
    assert_eq!(invert["stage"], "display");
    assert_eq!(invert["params"][0]["name"], "strength");
    assert_eq!(invert["params"][0]["max"], 1.0);
    assert!(styles.iter().all(|s| s["source"] == "builtin"));
}

#[test]
fn a_scene_adds_its_user_styles() {
    let json: serde_json::Value = serde_json::from_str(&run(&[
        "render",
        "tests/fixtures/scenes/user_styles.scene.toml",
        "--list-styles",
        "--json",
    ]))
    .unwrap();
    let template = json
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "template")
        .expect("tests/fixtures/scenes/styles/template.shader.toml is listed");
    assert_eq!(template["source"], "user");
}
