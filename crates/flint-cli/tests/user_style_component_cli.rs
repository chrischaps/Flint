//! #1320: a game's per-entity user style, with no schema file for its
//! component, the way the book says to write one. A drawing host loads
//! the scene before it knows its user styles, and its loader called
//! `hologram_style` an unknown component, as an error, asking for
//! `components/hologram_style.toml`. `flint check` was right. Now a host
//! says nothing of the component itself and the field mistakes once, as
//! `flint check` does. A style that reads no mask has no component, so its
//! `<name>_style` is one warning, worded the same in both.
//!
//! The project names only the engine's schemas. The examples' layer
//! declared `hologram_style` open until #1320, and that file is what hid
//! the problem in the engine's own scenes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"[scene]
name = "Dusk"

[entities.box]
[entities.box.hologram_style]
glwo = 2.0

[entities.barrel]
[entities.barrel.hologram_style]
glow = 1.0

[entities.crate]
[entities.crate.wash_style]
amount = 1.0
"#;

/// A whole-frame style: it reads no mask, so it has no component.
const WASH: &str = r#"version = 1
name = "wash"
stage = "display"
shader = "wash.wgsl"
inputs = []

[params.amount]
default = 0.0
min = 0.0
max = 1.0
"#;

const WASH_WGSL: &str = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project naming the engine's schemas alone, with the hologram style
/// (which reads the mask), the wash style (which doesn't) and the scene:
/// one folder a test, since they run side by side.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("user_style_component_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = std::fs::canonicalize(repo().join("schemas")).unwrap();
    let schemas = schemas
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    let manifest = format!("[project]\nid = \"dusk\"\nschemas = [\"{schemas}\"]\n");
    let style = |name: &str| {
        std::fs::read_to_string(repo().join("tests/fixtures/scenes/styles").join(name)).unwrap()
    };
    let files = [
        ("flint.toml".to_string(), manifest),
        ("scenes/main.scene.toml".into(), SCENE.into()),
        (
            "scenes/styles/hologram.shader.toml".into(),
            style("hologram.shader.toml"),
        ),
        ("scenes/styles/hologram.wgsl".into(), style("hologram.wgsl")),
        ("scenes/styles/wash.shader.toml".into(), WASH.into()),
        ("scenes/styles/wash.wgsl".into(), WASH_WGSL.into()),
    ];
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    assert!(
        !dir.join("schemas").exists(),
        "the project must have no schema of its own for the styles"
    );
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("run flint")
}

/// The lines of `text` that contain `needle`.
fn lines_with<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines().filter(|l| l.contains(needle)).collect()
}

/// The issue's first box: `flint render` (the loader every drawing host
/// shares, the player's too) calls neither style's component unknown, and
/// says the misspelt field once and the mask-less style's table once.
#[test]
fn a_drawing_host_says_a_user_styles_field_mistakes_once_and_no_unknown_component() {
    let dir = project("render");
    let run = flint(
        &dir,
        &[
            "render",
            "scenes/main.scene.toml",
            "-o",
            "dusk.png",
            "--width",
            "64",
            "--height",
            "64",
            "--no-grid",
        ],
    );
    let err = String::from_utf8_lossy(&run.stderr);
    if !run.status.success() && err.contains("headless render context") {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    assert!(run.status.success(), "{err}");
    assert!(lines_with(&err, "unknown-component").is_empty(), "{err}");
    assert!(
        lines_with(&err, "the full list: flint validate").is_empty(),
        "{err}"
    );
    assert_eq!(
        lines_with(
            &err,
            "warning[unknown-field] entity 'box' hologram_style.glwo"
        )
        .len(),
        1,
        "{err}"
    );
    assert!(lines_with(&err, "'barrel'").is_empty(), "{err}");
    let wash = lines_with(
        &err,
        "warning[bad-style-setting] entity 'crate' component 'wash_style'",
    );
    assert_eq!(wash.len(), 1, "{err}");
    assert!(
        wash[0].contains("the user style 'wash' reads no mask"),
        "{}",
        wash[0]
    );
}

/// `flint check` and `flint validate` say the same as the host: the
/// misspelt field, the mask-less style's table as the host's warning (it
/// was an `unknown-component` error asking for a schema file), nothing
/// else, so both pass.
#[test]
fn check_and_validate_say_what_the_drawing_host_says() {
    let dir = project("check");
    let run = flint(&dir, &["check", "scenes/main.scene.toml", "--json"]);
    let text = String::from_utf8_lossy(&run.stdout);
    let doc: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{text}"));
    let mut said: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            format!(
                "{} {} {} {}",
                d["severity"].as_str().unwrap_or("?"),
                d["code"].as_str().unwrap_or("?"),
                d["entity"].as_str().unwrap_or("?"),
                d["component"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    said.sort();
    assert_eq!(
        said,
        [
            "warning bad-style-setting crate wash_style",
            "warning unknown-field box hologram_style",
        ],
        "{text}"
    );
    assert!(run.status.success(), "{text}");

    let run = flint(&dir, &["validate", "scenes/main.scene.toml"]);
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.status.success(), "{out}");
    assert!(lines_with(&out, "unknown-component").is_empty(), "{out}");
    assert_eq!(lines_with(&out, "hologram_style.glwo").len(), 1, "{out}");
}
