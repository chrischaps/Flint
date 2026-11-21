//! A style's asset fields are checked (#1063): `flint validate` with the
//! engine's own schemas calls `ascii_style.palette = "ambr"` missing, naming
//! the entity and field and suggesting `amber`, while a built-in palette, a
//! palette file beside the scene and a paper image pass, and `"none"` passes
//! for `pencil_style.paper_image` / `stroke_image` and
//! `watercolor_style.paper_image`. A misspelt paper is missing too.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn engine_schemas() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_string()
}

const PALETTE: &str = "match = \"nearest\"\ncolors = [\"#1b998b\", \"#ed217c\"]\n";

/// Each entity gets one component, so every mistake is its own line.
fn scene(entries: &[(&str, &str, &str, &str)]) -> String {
    let mut text = String::from("[scene]\nname = \"Styles\"\n");
    for (entity, component, field, value) in entries {
        text.push_str(&format!(
            "\n[entities.{entity}.{component}]\n{field} = \"{value}\"\n"
        ));
    }
    text
}

fn project(name: &str, scene_text: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("style_asset_builtins_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("palettes")).unwrap();
    std::fs::create_dir_all(dir.join("papers")).unwrap();
    std::fs::write(
        dir.join("flint.toml"),
        format!(
            "[project]\nid = \"styles\"\nschemas = [\"{}\"]\n",
            engine_schemas()
        ),
    )
    .unwrap();
    std::fs::write(dir.join("palettes/tidepool.palette.toml"), PALETTE).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 190, 170, 255]))
        .save(dir.join("papers/cold_press.png"))
        .unwrap();
    std::fs::write(dir.join("styles.scene.toml"), scene_text).unwrap();
    dir
}

fn validate(dir: &Path) -> (Output, serde_json::Value) {
    let out = Command::new(FLINT)
        .current_dir(dir)
        .args(["validate", "styles.scene.toml", "--format", "json"])
        .output()
        .expect("flint runs");
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, json)
}

/// (entity, component.field, message, hint) of each error.
fn errors(json: &serde_json::Value) -> Vec<(String, String, String, String)> {
    let text = |d: &serde_json::Value, key: &str| d[key].as_str().unwrap_or("").to_string();
    json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .map(|d| {
            (
                text(d, "entity"),
                format!("{}.{}", text(d, "component"), text(d, "field")),
                text(d, "message"),
                text(d, "hint"),
            )
        })
        .collect()
}

#[test]
fn built_in_names_none_and_files_pass() {
    let dir = project(
        "pass",
        &scene(&[
            ("amber", "ascii_style", "palette", "amber"),
            ("paper", "ascii_style", "palette", "paper"),
            ("tide", "ascii_style", "palette", "tidepool"),
            ("glyphs", "ascii_style", "font", "builtin"),
            ("plain", "pencil_style", "paper_image", "none"),
            ("bars", "pencil_style", "stroke_image", "none"),
            ("sheet", "pencil_style", "paper_image", "cold_press"),
            (
                "by_path",
                "pencil_style",
                "paper_image",
                "papers/cold_press.png",
            ),
            ("wash", "watercolor_style", "paper_image", "none"),
            ("washed", "watercolor_style", "paper_image", "cold_press"),
            ("label", "ui_text", "font", "builtin"),
        ]),
    );
    let (out, json) = validate(&dir);
    assert_eq!(errors(&json), [], "{json:#}");
    assert!(out.status.success(), "{json:#}");
}

#[test]
fn a_misspelt_palette_or_paper_is_missing_and_named() {
    let dir = project(
        "fail",
        &scene(&[
            ("lamp", "ascii_style", "palette", "ambr"),
            ("sketch", "pencil_style", "paper_image", "cold_pres"),
            ("wash", "watercolor_style", "paper_image", "nnoe"),
        ]),
    );
    let (out, json) = validate(&dir);
    assert!(!out.status.success(), "{json:#}");
    let found = errors(&json);
    let fields: Vec<(&str, &str, &str)> = found
        .iter()
        .map(|(e, f, m, _)| (e.as_str(), f.as_str(), m.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("lamp", "ascii_style.palette", "palette 'ambr' not found"),
            (
                "sketch",
                "pencil_style.paper_image",
                "paper image 'cold_pres' not found"
            ),
            (
                "wash",
                "watercolor_style.paper_image",
                "paper image 'nnoe' not found"
            ),
        ],
        "{json:#}"
    );
    let hint = &found[0].3;
    assert!(
        hint.starts_with(
            "did you mean `amber`? built in: cga, cga-warm, cga16, phosphor, amber, mono, paper; \
             tried "
        ),
        "{hint}"
    );
    assert!(hint.contains("palettes/ambr.palette.toml"), "{hint}");
    assert!(
        found[2].3.starts_with("did you mean `none`?"),
        "{}",
        found[2].3
    );
}

/// The renderer takes no palette called "none" (it warns and draws cga), so
/// validate doesn't pass one either.
#[test]
fn a_palette_called_none_is_missing() {
    let dir = project(
        "none_palette",
        &scene(&[("lamp", "ascii_style", "palette", "none")]),
    );
    let (_, json) = validate(&dir);
    let found = errors(&json);
    assert_eq!(found.len(), 1, "{json:#}");
    assert_eq!(found[0].2, "palette 'none' not found");
}
