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

/// The living oak example with its `[living]` table replaced, or `None` to
/// keep it. Written to a scratch file so each case can break it its own way.
fn living_spec(dir: &Path, living: Option<&str>) -> PathBuf {
    let src = std::fs::read_to_string(repo_root().join("specs/examples/oak_living.procgen.toml"))
        .unwrap();
    let text = match living {
        None => src,
        Some(table) => {
            // At the start of a line: the header comment names it too.
            let cut = src
                .find("\n[living]")
                .expect("the example has a [living] table");
            format!("{}\n[living]\n{table}\n", &src[..cut])
        }
    };
    let path = dir.join("living.procgen.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn dry_run_counts_a_living_spec_skeleton() {
    let dir = scratch("living_ok");
    let o = gen(&[s(&living_spec(&dir, None)), "--dry-run"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains("lineage 7: 243 segments") && t.contains("a ring of 8 poses"),
        "{t}"
    );
}

#[test]
fn dry_run_refuses_a_malformed_living_table_and_names_the_key() {
    let dir = scratch("living_bad");
    for (table, key) in [
        ("lineage_seed = 7\npose_rng = 8", "pose_rng"),
        ("pose_ring = 8", "lineage_seed"),
        ("lineage_seed = 7\npose_ring = 1", "living.pose_ring"),
        ("lineage_seed = 7\nstagger = \"per_twig\"", "stagger"),
        (
            "lineage_seed = 7\nvariance = { amount = 2.0 }",
            "living.variance.amount",
        ),
        (
            "lineage_seed = 7\ndrift = { params = [\"branch_angel\"] }",
            "branch_angel",
        ),
    ] {
        let o = gen(&[s(&living_spec(&dir, Some(table))), "--dry-run"]);
        let t = text(&o);
        assert!(!o.status.success(), "{table:?} passed:\n{t}");
        assert!(t.contains(key), "{table:?} didn't name {key}:\n{t}");
    }
}

#[test]
fn living_on_a_colonized_tree_warns_and_strict_refuses() {
    let dir = scratch("living_pine");
    let pine =
        std::fs::read_to_string(repo_root().join("specs/examples/pine_tree.procgen.toml")).unwrap();
    let pine = pine.split("[[lod]]").next().unwrap();
    let path = dir.join("pine.procgen.toml");
    std::fs::write(&path, format!("{pine}\n[living]\nlineage_seed = 7\n")).unwrap();

    let o = gen(&[s(&path), "--dry-run"]);
    let t = text(&o);
    assert!(o.status.success(), "the bake is still good:\n{t}");
    assert!(
        t.contains("[living] would be ignored") && t.contains("space colonization"),
        "{t}"
    );
    assert!(!t.contains("Living:"), "{t}");

    let o = gen(&[s(&path), "--dry-run", "--strict"]);
    assert!(!o.status.success(), "{}", text(&o));
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

// ── AI assistance and cost tracking ───────────────────────────────────────

/// `flint gen` run inside `dir`, so `.flint/` lands there, with no API key.
fn gen_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .arg("gen")
        .args(args)
        .current_dir(dir)
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("run flint")
}

#[test]
fn ai_create_with_the_mock_agent_writes_a_glb() {
    let dir = scratch("ai_create");
    let o = gen_in(
        &dir,
        &[
            "--ai-create",
            "twisted dead tree",
            "--ai-provider",
            "mock",
            "-o",
            "tree.glb",
        ],
    );
    assert!(o.status.success(), "{}", text(&o));
    let bytes = std::fs::read(dir.join("tree.glb")).unwrap();
    assert_eq!(&bytes[..4], b"glTF", "not a GLB");
    // The agent's account is on stderr, not mixed into stdout.
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("ai: creating a tree_v1 spec"), "{err}");
    assert!(!String::from_utf8_lossy(&o.stdout).contains("ai:"));
}

#[test]
fn ai_assist_refines_a_spec_then_generates() {
    let dir = scratch("ai_assist");
    let o = gen_in(
        &dir,
        &[
            s(&oak_spec()),
            "--ai-assist",
            "--ai-provider",
            "mock",
            "-o",
            "oak.glb",
            "--save-spec",
            "oak_refined.procgen.toml",
        ],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(dir.join("oak.glb").is_file());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("ai: interpret: confidence"), "{err}");
    assert!(err.contains("ai: refining"), "{err}");
    // The saved spec is the refined one and runs on its own.
    let saved = dir.join("oak_refined.procgen.toml");
    let o = gen_in(&dir, &[s(&saved), "-o", "again.glb"]);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn print_spec_puts_only_toml_on_stdout() {
    let dir = scratch("print_spec");
    let o = gen_in(
        &dir,
        &[
            "--ai-create",
            "old brick wall",
            "--ai-provider",
            "mock",
            "--print-spec",
            "--dry-run",
        ],
    );
    assert!(o.status.success(), "{}", text(&o));
    // A dry run of an AI step sends nothing, so there's no spec yet.
    assert!(String::from_utf8_lossy(&o.stdout).contains("nothing was sent"));

    let o = gen_in(
        &dir,
        &[
            "--ai-create",
            "short wide autumn tree",
            "--ai-provider",
            "mock",
            "--print-spec",
            "-o",
            "t.glb",
        ],
    );
    assert!(o.status.success(), "{}", text(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let value: toml::Value = toml::from_str(&stdout).expect("stdout is TOML");
    assert_eq!(value["generator"].as_str(), Some("tree_v1"));
    // The generation summary moved to stderr.
    assert!(String::from_utf8_lossy(&o.stderr).contains("Output:"));
    assert!(dir.join("t.glb").is_file());
}

#[test]
fn json_report_keeps_stdout_clear_of_the_ai_dry_run_plan() {
    let dir = scratch("report_json_plan");
    let o = gen_in(
        &dir,
        &[
            "--ai-create",
            "old brick wall",
            "--ai-provider",
            "mock",
            "--dry-run",
            "--validate",
            "--report",
            "json",
        ],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(o.stdout.is_empty(), "{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("nothing was sent"));
}

#[test]
fn anthropic_without_a_key_fails_before_any_call() {
    let dir = scratch("no_key");
    let o = gen_in(&dir, &["--ai-create", "tall oak tree", "-o", "tree.glb"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("ANTHROPIC_API_KEY"), "{}", text(&o));
    assert!(!dir.join(".flint/procgen_costs.json").exists());
    assert!(!dir.join("tree.glb").exists());
}

#[test]
fn cost_report_sums_the_ledger() {
    let dir = scratch("cost_report");
    let o = gen_in(&dir, &["--cost-report"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("no AI calls recorded"), "{}", text(&o));

    std::fs::create_dir_all(dir.join(".flint")).unwrap();
    let line = |ts: &str, model: &str, spec: &str, usd: f64| {
        format!(
            r#"{{"timestamp":"{ts}","provider":"anthropic","model":"{model}","input_tokens":1000,"output_tokens":2000,"estimated_usd":{usd},"spec_name":"{spec}"}}"#
        )
    };
    let ledger = [
        line("2023-12-31T10:00:00Z", "claude-opus-5-5", "oak", 1.0),
        line("2024-01-02T10:00:00Z", "claude-opus-5-5", "oak", 0.25),
        line("2024-01-03T10:00:00Z", "claude-sonnet-5-5", "wall", 0.125),
    ]
    .join("\n");
    std::fs::write(dir.join(".flint/procgen_costs.json"), ledger + "\n").unwrap();

    let o = gen_in(&dir, &["--cost-report"]);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("$1.38"), "total: {out}");
    assert!(
        out.contains("By provider") && out.contains("By spec"),
        "{out}"
    );

    let o = gen_in(&dir, &["--cost-report", "--since", "2024-01-01"]);
    let out = text(&o);
    assert!(out.contains("$0.3750") && out.contains("2 calls"), "{out}");
    assert!(out.contains("since 2024-01-01"), "{out}");

    let o = gen_in(&dir, &["--cost-report", "--since", "last tuesday"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("YYYY-MM-DD"), "{}", text(&o));
}

// ── --strip ────────────────────────────────────────────────────────────────

/// A strip small enough to render quickly. `None` when this machine has no
/// GPU adapter, which is every CI runner.
fn small_strip(dir: &Path, name: &str, living: &str) -> Option<Vec<u8>> {
    let out = dir.join(format!("{name}.png"));
    let o = gen(&[
        s(&living_spec(dir, Some(living))),
        "--strip",
        "--frames",
        "4",
        "--columns",
        "2",
        "--cell-size",
        "48",
        "-o",
        s(&out),
    ]);
    if !o.status.success() && text(&o).contains("headless render context") {
        eprintln!("no GPU adapter; skipping");
        return None;
    }
    assert!(o.status.success(), "{}", text(&o));
    Some(std::fs::read(&out).unwrap())
}

#[test]
fn strip_dry_run_says_what_it_would_make_and_writes_nothing() {
    let dir = scratch("strip_dry");
    let out = dir.join("strip.png");
    let o = gen(&[
        s(&living_spec(&dir, None)),
        "--strip",
        "--dry-run",
        "-o",
        s(&out),
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("lineage 7"), "{t}");
    assert!(t.contains("48 frames at 24 fps, 8 x 6 cells"), "{t}");
    assert!(t.contains("glide 0.3"), "{t}");
    assert!(!out.exists(), "a dry run wrote {out:?}");
}

#[test]
fn strip_reads_glide_from_the_spec() {
    let dir = scratch("strip_glide");
    let o = gen(&[
        s(&living_spec(
            &dir,
            Some("lineage_seed = 7\nstep = 0.25\nglide = 0.9"),
        )),
        "--strip",
        "--dry-run",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("step 0.25 s, glide 0.9"), "{t}");
}

#[test]
fn strip_refuses_what_has_no_living_side() {
    let dir = scratch("strip_refuses");
    // No [living] table.
    let o = gen(&[s(&oak_spec()), "--strip", "--dry-run"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("no [living] table"), "{}", text(&o));

    // A [living] table on a tree that can't live: an error, not the warning.
    let pine =
        std::fs::read_to_string(repo_root().join("specs/examples/pine_tree.procgen.toml")).unwrap();
    let path = dir.join("pine.procgen.toml");
    std::fs::write(&path, format!("{pine}\n[living]\nlineage_seed = 7\n")).unwrap();
    let o = gen(&[s(&path), "--strip", "--dry-run"]);
    let t = text(&o);
    assert!(!o.status.success());
    assert!(t.contains("space colonization"), "{t}");
    assert!(!t.contains("would be ignored"), "{t}");
}

#[test]
fn strip_writes_only_a_png_and_takes_no_other_run_mode() {
    let dir = scratch("strip_flags");
    let spec = living_spec(&dir, None);
    let o = gen(&[s(&spec), "--strip", "--dry-run", "-o", "tree.glb"]);
    assert!(!o.status.success());
    assert!(text(&o).contains(".png"), "{}", text(&o));

    for flags in [
        &["--batch", "2"][..],
        &["--seed", "1"],
        &["--validate"],
        &["--register"],
        &["--format", "glb"],
        &["--print-spec"],
    ] {
        let mut args = vec![s(&spec), "--strip", "--dry-run"];
        args.extend_from_slice(flags);
        let o = gen(&args);
        assert!(!o.status.success(), "{flags:?} should conflict");
    }

    // The strip's own flags mean nothing without it.
    for flags in [["--frames", "4"], ["--fps", "12"], ["--cell-size", "64"]] {
        let mut args = vec![s(&spec), "--dry-run"];
        args.extend_from_slice(&flags);
        assert!(!gen(&args).status.success(), "{flags:?} needs --strip");
    }
}

#[test]
fn a_strip_is_byte_identical_across_runs_and_changes_with_glide() {
    let dir = scratch("strip_bytes");
    let table = |glide: f32| format!("lineage_seed = 7\nstagger = \"none\"\nglide = {glide}");
    let Some(first) = small_strip(&dir, "first", &table(1.0)) else {
        return;
    };
    let second = small_strip(&dir, "second", &table(1.0)).unwrap();
    assert!(first == second, "two runs of one spec differ");

    let snapping = small_strip(&dir, "snapping", &table(0.0)).unwrap();
    assert!(first != snapping, "changing glide changed nothing");

    let sheet = image::load_from_memory(&first).unwrap();
    assert_eq!(sheet.width(), 28 * 2 + 48 * 2 + 16);
}
