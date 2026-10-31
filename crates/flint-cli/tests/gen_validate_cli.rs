//! End-to-end tests for `flint gen --validate` (#30): run the real binary on
//! the shipped specs and the tavern style guide, and hold the report and the
//! exit code to the ticket's acceptance criteria.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn spec(name: &str) -> String {
    repo_root()
        .join("specs")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

fn tavern() -> String {
    repo_root()
        .join("styles/medieval_tavern.style.toml")
        .to_str()
        .unwrap()
        .to_string()
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gen_validate_cli")
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

/// The report's check lines with this status, e.g. `[ WARN] LOD0 ...`.
fn lines_with<'a>(out: &'a str, status: &str) -> Vec<&'a str> {
    out.lines()
        .filter(|l| l.trim_start().starts_with(&format!("[ {status}]")))
        .collect()
}

#[test]
fn a_well_configured_spec_passes_cleanly() {
    let dir = scratch("pass");
    let out = dir.join("tree.glb");
    let o = gen(&[
        &spec("oak_tree.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("Validation: "), "{t}");
    assert!(t.contains(" 0 warnings, 0 failed"), "{t}");
    // One spec target per LOD, read from the spec's [[lod]] table.
    for lod in 0..3 {
        assert!(t.contains(&format!("LOD{lod} spec triangle target")), "{t}");
    }
}

#[test]
fn without_validate_nothing_is_checked() {
    let dir = scratch("off");
    let out = dir.join("tree.glb");
    let o = gen(&[&spec("oak_tree.procgen.toml"), "-o", out.to_str().unwrap()]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(!t.contains("Validation:"), "{t}");
}

#[test]
fn a_style_guide_needs_validate() {
    let o = gen(&[&spec("oak_tree.procgen.toml"), "--style-guide", &tavern()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("--validate"), "{}", text(&o));
}

/// The oak's LOD0 is ~8000 triangles and the tavern allows 5000: within the
/// 2x tolerance, so a warning, and `--strict` turns it into exit 1.
#[test]
fn over_budget_warns_and_strict_fails() {
    let dir = scratch("strict");
    let out = dir.join("tree.glb");
    let base = [
        spec("oak_tree.procgen.toml"),
        "-o".into(),
        out.to_str().unwrap().into(),
        "--validate".into(),
        "--style-guide".into(),
        tavern(),
    ];
    let args: Vec<&str> = base.iter().map(String::as_str).collect();

    let o = gen(&args);
    let t = text(&o);
    assert!(o.status.success(), "warnings alone must exit 0: {t}");
    let warns = lines_with(&t, "WARN");
    assert!(
        warns
            .iter()
            .any(|l| l.contains("LOD0 style triangle budget")),
        "{t}"
    );
    assert!(lines_with(&t, "FAIL").is_empty(), "{t}");

    let mut strict = args.clone();
    strict.push("--strict");
    let o = gen(&strict);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("Strict mode"), "{t}");
}

/// A ceiling far below the output is a failure, and fails without --strict.
#[test]
fn far_over_budget_fails() {
    let dir = scratch("fail");
    let style = dir.join("tiny.style.toml");
    std::fs::write(
        &style,
        "[style]\nname = \"tiny\"\n[style.geometry]\nmax_triangles = 100\n",
    )
    .unwrap();
    let out = dir.join("tree.glb");
    let o = gen(&[
        &spec("oak_tree.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
        "--style-guide",
        style.to_str().unwrap(),
    ]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        lines_with(&t, "FAIL")
            .iter()
            .any(|l| l.contains("style triangle budget")),
        "{t}"
    );
}

/// The beetle's ellipsoids used to carry 16 zero-area slivers at each pole.
#[test]
fn the_beetle_has_no_degenerate_triangles() {
    let dir = scratch("beetle");
    let out = dir.join("beetle.glb");
    let o = gen(&[
        &spec("beetle.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("[ PASS] skinned degenerate triangles"), "{t}");
}

/// A texture against the tavern: dimensions, palette and roughness-map
/// lines all appear, and the summary counts them.
#[test]
fn textures_are_checked_against_the_style() {
    let dir = scratch("texture");
    let src = std::fs::read_to_string(repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
    let small = src
        .replace("height = 512", "height = 64")
        .replace("width = 512", "width = 64");
    let spec_path = dir.join("wall.procgen.toml");
    std::fs::write(&spec_path, small).unwrap();
    let out = dir.join("wall.png");
    let o = gen(&[
        spec_path.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--validate",
        "--style-guide",
        &tavern(),
    ]);
    let t = text(&o);
    assert!(t.contains("color texture dimensions: 64x64"), "{t}");
    assert!(t.contains("color palette: "), "{t}");
    assert!(t.contains("roughness style range: "), "{t}");
    assert!(t.contains("Validation: "), "{t}");
}

// ── --report json (#185) ────────────────────────────────────────────────────

fn tavern_oak_json(dir: &Path, extra: &[&str]) -> Output {
    let out = dir.join("tree.glb");
    let mut args = vec![
        spec("oak_tree.procgen.toml"),
        "-o".into(),
        out.to_str().unwrap().into(),
        "--validate".into(),
        "--style-guide".into(),
        tavern(),
        "--report".into(),
        "json".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    gen(&args)
}

/// stdout is nothing but the JSON line, it round-trips into a
/// `ValidationReport`, and the over-budget LOD says by how much in numbers.
#[test]
fn json_report_round_trips_and_carries_the_numbers() {
    let dir = scratch("json");
    let o = tavern_oak_json(&dir, &[]);
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success(), "{}", text(&o));

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout must hold only the report: {stdout}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["verdict"], "pass", "{v}");
    assert_eq!(v["spec"], "oak_tree_01", "{v}");
    assert!(v["warnings"].as_u64().unwrap() >= 1, "{v}");

    let report: flint_procgen::ValidationReport = serde_json::from_str(lines[0]).unwrap();
    let budget = report
        .checks
        .iter()
        .find(|c| c.name == "LOD0 style triangle budget")
        .unwrap_or_else(|| panic!("{report:#?}"));
    assert_eq!(budget.status, flint_procgen::CheckStatus::Warn);
    let (actual, limit) = budget.measured().unwrap();
    assert_eq!(limit, 5000.0);
    assert!(actual > limit && actual <= 2.0 * limit, "{actual}");
    // Counts are JSON integers.
    assert!(
        lines[0].contains(&format!(r#""actual":{actual},"limit":5000"#)),
        "{}",
        lines[0]
    );

    // The human report still goes somewhere a person can read it.
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("[ WARN] LOD0 style triangle budget"),
        "{stderr}"
    );
    assert!(stderr.contains("Validation: "), "{stderr}");
}

/// `--strict` turns the warning into a `fail` verdict and exit 1.
#[test]
fn json_report_verdict_follows_strict() {
    let dir = scratch("json_strict");
    let o = tavern_oak_json(&dir, &["--strict"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["verdict"], "fail", "{v}");
    assert_eq!(v["failed"], 0, "{v}");
}

/// A batch writes one line per variant, each naming its seed.
#[test]
fn json_report_is_one_line_per_batch_variant() {
    let dir = scratch("json_batch");
    let src = std::fs::read_to_string(repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
    let small = src
        .replace("height = 512", "height = 32")
        .replace("width = 512", "width = 32");
    let spec_path = dir.join("wall.procgen.toml");
    std::fs::write(&spec_path, small).unwrap();
    let out = dir.join("wall.png");
    let o = gen(&[
        spec_path.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--batch",
        "2",
        "--seed-start",
        "7",
        "--validate",
        "--report",
        "json",
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let seeds: Vec<u64> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["seed"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(seeds, [7, 8]);
}

#[test]
fn report_needs_validate() {
    let o = gen(&[&spec("oak_tree.procgen.toml"), "--report", "json"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("--validate"), "{}", text(&o));
}

/// Without `--report`, stdout is the human report it always was.
#[test]
fn the_human_report_is_the_default() {
    let dir = scratch("human");
    let out = dir.join("tree.glb");
    let o = gen(&[
        &spec("oak_tree.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
    ]);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        stdout.contains("[ PASS] LOD0 spec triangle target"),
        "{stdout}"
    );
    assert!(stdout.contains("Validation: "), "{stdout}");
    assert!(!stdout.contains('{'), "{stdout}");
}
