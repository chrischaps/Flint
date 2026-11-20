//! The frozen fixtures are held to the examples' rule (#796): no error
//! diagnostic. `flint examples --check` fails an example on one; the
//! fixtures had no such check, so they kept tables nothing reads (24
//! `directional_light` / `point_light` errors after #683) and an agent
//! copying a fixture copied them. An error kept on purpose is named below
//! with its reason. No GPU: `flint check` loads scenes only.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// The fixture folders: the scenes the goldens and CLI tests load, and the
/// golden suite's own.
const FOLDERS: &[&str] = &[
    "tests/fixtures/scenes",
    "crates/flint-cli/tests/golden/fixtures",
];

/// Errors kept on purpose: (file, code, why).
const KEPT: &[(&str, &str, &str)] = &[
    (
        "crates/flint-cli/tests/golden/fixtures/styles/broken.wgsl",
        "bad-user-style",
        "the golden `user-style-broken` draws a style whose WGSL fails",
    ),
    (
        "crates/flint-cli/tests/golden/fixtures/styles/broken_hdr.wgsl",
        "bad-user-style",
        "the golden `user-style-broken-hdr` draws an HDR style whose WGSL fails",
    ),
    (
        "crates/flint-cli/tests/golden/fixtures/styles/broken_inputs.wgsl",
        "bad-user-style",
        "the golden `user-style-broken-inputs` draws a style with every input whose WGSL fails",
    ),
];
// `styles/darkroom.shader.toml` was kept here from #1056 until #984: it
// borrowed its images from the examples by a `../` path, which an image
// option refuses. The fixtures hold their own copies now, and it loads;
// the golden `user-style-darkroom` draws it, so CI notices if it stops.

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `flint check <folder> --json`, from the repository root so its
/// `flint.toml` names the schemas (the examples' layer included).
fn check(folder: &str) -> serde_json::Value {
    let out = Command::new(FLINT)
        .args(["check", folder, "--json"])
        .current_dir(repo())
        .output()
        .expect("run flint");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "flint check {folder} --json is not one JSON document ({e}):\n{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn kept(file: &str, code: &str) -> bool {
    KEPT.iter().any(|(f, c, _)| *f == file && *c == code)
}

#[test]
fn no_fixture_has_an_error_diagnostic_but_those_kept_on_purpose() {
    let mut unexpected = Vec::new();
    let mut seen = Vec::new();
    for folder in FOLDERS {
        let doc = check(folder);
        let diagnostics = doc["diagnostics"].as_array().expect("a diagnostics list");
        for d in diagnostics.iter().filter(|d| d["severity"] == "error") {
            let file = d["file"].as_str().unwrap_or("").replace('\\', "/");
            let code = d["code"].as_str().unwrap_or("");
            if kept(&file, code) {
                seen.push((file, code.to_string()));
                continue;
            }
            let hint = d["hint"].as_str().map(|h| format!(" (hint: {h})"));
            unexpected.push(format!(
                "{file}: error[{code}] entity {} component {}: {}{}",
                d["entity"],
                d["component"],
                d["message"],
                hint.unwrap_or_default()
            ));
        }
    }
    assert!(
        unexpected.is_empty(),
        "a fixture has an error diagnostic (fix it, or name it in KEPT with its reason):\n{}",
        unexpected.join("\n")
    );
    // A kept error that has gone stays out of the list
    for (file, code, why) in KEPT {
        assert!(
            seen.iter().any(|(f, c)| f == file && c == code),
            "{file} no longer has error[{code}] ({why}): drop it from KEPT"
        );
    }
}
