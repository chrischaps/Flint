//! Golden-fixture tests. The fixtures live in the *game* repository (which
//! vendors this engine at `engine/`), so this test resolves them relative to
//! the crate and skips silently when the engine is built standalone.
//!
//! Fixture contract (see the game repo's `fixtures/README.md`):
//! - `fixtures/valid/*.suite.toml` / `*.chart.toml` must produce zero issues
//!   from the structural pass.
//! - `fixtures/invalid/*` each carry a `# expect-error: <Code>` header naming
//!   the exact issue code the validator must emit.

use flint_music::{validate_chart, validate_manifest, Chart, SuiteManifest};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures");
    dir.canonicalize().ok().filter(|d| d.join("valid").exists())
}

fn expected_code(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        l.strip_prefix("# expect-error:")
            .map(|c| c.trim().to_string())
    })
}

fn reference_manifest(fixtures: &Path) -> SuiteManifest {
    let path = fixtures.join("valid/prototype.suite.toml");
    SuiteManifest::load(&path).expect("reference manifest must parse")
}

#[test]
fn valid_fixtures_produce_no_issues() {
    let Some(fixtures) = fixtures_dir() else {
        eprintln!("skipping: no game-repo fixtures directory");
        return;
    };
    let manifest = reference_manifest(&fixtures);
    for entry in std::fs::read_dir(fixtures.join("valid")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with(".suite.toml") {
            let m = SuiteManifest::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let issues = validate_manifest(&m);
            assert!(issues.is_empty(), "{name} produced issues: {issues:?}");
        } else if name.ends_with(".chart.toml") {
            let c = Chart::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let issues = validate_chart(&c, &manifest);
            assert!(issues.is_empty(), "{name} produced issues: {issues:?}");
        }
    }
}

#[test]
fn invalid_fixtures_produce_their_pinned_error() {
    let Some(fixtures) = fixtures_dir() else {
        eprintln!("skipping: no game-repo fixtures directory");
        return;
    };
    let manifest = reference_manifest(&fixtures);
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures.join("invalid")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let expected = expected_code(&text)
            .unwrap_or_else(|| panic!("{name}: missing `# expect-error:` header"));

        let issues = if name.ends_with(".suite.toml") {
            let m = SuiteManifest::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            validate_manifest(&m)
        } else if name.ends_with(".chart.toml") {
            let c = Chart::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            validate_chart(&c, &manifest)
        } else {
            continue;
        };

        assert!(
            issues.iter().any(|i| i.code == expected),
            "{name}: expected code '{expected}', got {:?}",
            issues.iter().map(|i| i.code).collect::<Vec<_>>()
        );
        checked += 1;
    }
    assert!(checked >= 8, "suspiciously few invalid fixtures ({checked})");
}
