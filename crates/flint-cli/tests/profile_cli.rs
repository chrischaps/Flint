//! `flint profile` (#1098), end to end on the hand-made traces in
//! `tests/fixtures/traces/` (`make_traces.py` writes them): `explain` names
//! each of the stall trace's three staged causes in its first line (a
//! script's stall, a model load that ran only there, a wait no scope
//! covers), `top`, `frame` and `diff` print tables whose numbers `--json`
//! carries, and bad input is refused with what was wanted.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/traces")
        .join(name)
        .display()
        .to_string()
}

fn stall() -> String {
    fixture("stall.trace.json")
}

fn calm() -> String {
    fixture("calm.trace.json")
}

fn flint(args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .args(args)
        .output()
        .expect("run flint")
}

fn text(args: &[&str]) -> String {
    let o = flint(args);
    assert!(
        o.status.success(),
        "`flint {}` failed:\n{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn json(args: &[&str]) -> Value {
    let mut words = vec!["--json"];
    words.extend_from_slice(args);
    let out = text(&words);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("not one JSON document ({e}):\n{out}"))
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

/// The row of a text table whose first cell is `name`, split on spaces.
fn row<'a>(table: &'a str, name: &str) -> Vec<&'a str> {
    table
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
        .unwrap_or_else(|| panic!("no row `{name}` in:\n{table}"))[name.len()..]
        .split_whitespace()
        .collect()
}

fn ms(v: &Value) -> String {
    let v = v.as_f64().unwrap();
    if v != 0.0 && v.abs() < 0.1 {
        format!("{v:.3}")
    } else {
        format!("{v:.2}")
    }
}

#[test]
fn explain_names_the_stalling_script_in_its_first_line() {
    // The worst frame is the script's stall: the default, and `hitch` (the last one)
    for which in [None, Some("hitch"), Some("113")] {
        let mut args = vec!["profile", "explain"];
        let s = stall();
        args.push(&s);
        if let Some(w) = which {
            args.push(w);
        }
        let out = text(&args);
        let first = first_line(&out);
        assert!(
            first.starts_with(
                "frame 113 took 104.29 ms (median 4.29): stall.rhai:on_update 100.05 ms"
            ),
            "{which:?}: {out}"
        );
        assert!(
            first.contains("+100.00 of the frame's +100.00 ms excess"),
            "{out}"
        );
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("tick > run_scripts > script::call (scripts/stall.rhai)"),
            "the path to it is the second line:\n{out}"
        );
    }
    let doc = json(&["profile", "explain", &stall()]);
    assert_eq!(doc["culprit"]["scope"], "stall.rhai:on_update");
    assert_eq!(doc["culprit"]["only_here"], false);
    assert_eq!(doc["frame"]["frame"], 113);
    assert_eq!(doc["frame"]["excess_ms"], 100.0);
    assert_eq!(doc["culprit"]["path"][1], "run_scripts");
    assert!(doc["answer"]
        .as_str()
        .unwrap()
        .starts_with("frame 113 took"));
    // The excess table's first row is the script, the same numbers in both
    let out = text(&["profile", "explain", &stall()]);
    let r = row(&out, "stall.rhai:on_update");
    let first = &doc["scopes"][0];
    assert_eq!(first["scope"], "stall.rhai:on_update");
    assert_eq!(r[0], ms(&first["ms"]));
    assert_eq!(r[1], ms(&first["median_ms"]));
    assert_eq!(r[2], format!("+{}", ms(&first["excess_ms"])));
    // 16 frames less the two edges and the three hitches
    assert_eq!(doc["frame"]["normal_frames"], 11);
}

#[test]
fn explain_says_a_scope_ran_only_here_and_a_wait_no_scope_covers() {
    let out = text(&["profile", "explain", &stall(), "109"]);
    assert!(
        first_line(&out).contains(": import_model (models/whale.glb) 30.00 ms ran only here"),
        "{out}"
    );
    assert!(
        out.contains("ran in this frame and in no normal one:"),
        "{out}"
    );
    assert!(
        out.contains("model load models/whale.glb 30.00 ms"),
        "the frame's note:\n{out}"
    );
    let doc = json(&["profile", "explain", &stall(), "109"]);
    assert_eq!(doc["culprit"]["only_here"], true);
    assert_eq!(
        doc["only_here"][0]["scope"],
        "import_model (models/whale.glb)"
    );

    let out = text(&["profile", "explain", &stall(), "105"]);
    assert!(
        first_line(&out)
            .contains("no scope covers the +40.00 ms excess; the acquire phase took 40.50 ms"),
        "{out}"
    );
    assert!(out.contains("a wait, not work"), "{out}");
    let doc = json(&["profile", "explain", &stall(), "105"]);
    assert!(doc["culprit"].is_null());
    assert_eq!(doc["phases"][0]["phase"], "acquire");

    let out = text(&["profile", "explain", &stall(), "102"]);
    assert!(first_line(&out).ends_with("nothing to explain"), "{out}");
}

#[test]
fn top_prints_a_table_and_json_carries_its_numbers() {
    let out = text(&["profile", "top", &stall(), "--limit", "6"]);
    assert!(
        out.contains("16 frames (100..115)") && out.contains("scopes on main, io"),
        "{out}"
    );
    let doc = json(&["profile", "top", &stall(), "--limit", "6"]);
    assert_eq!(doc["range"]["frames"], 16);
    assert_eq!(doc["scopes"].as_array().unwrap().len(), 6);
    assert_eq!(
        doc["scopes"][0]["scope"], "tick",
        "by total, tick holds the stall"
    );
    for s in doc["scopes"].as_array().unwrap() {
        let r = row(&out, s["scope"].as_str().unwrap());
        assert_eq!(r[0], s["calls"].to_string(), "{s}");
        assert_eq!(r[1], ms(&s["total_ms"]), "{s}");
        assert_eq!(r[2], ms(&s["self_ms"]), "{s}");
        assert_eq!(r[3], ms(&s["mean_ms_per_frame"]), "{s}");
        assert_eq!(r[4], ms(&s["worst_ms"]), "{s}");
        assert_eq!(r[5], s["worst_frame"].to_string(), "{s}");
    }
    // Its worst call, and where
    let stall_row = doc["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["scope"] == "stall.rhai:on_update")
        .unwrap();
    assert_eq!(stall_row["worst_frame"], 113);
    assert_eq!(stall_row["calls"], 16);

    // --thread, --frames, --sort, --by-detail
    let io = json(&["profile", "top", &stall(), "--thread", "io"]);
    assert_eq!(io["scopes"][0]["scope"], "decode_image");
    assert_eq!(io["scopes"][0]["calls"], 4);
    let few = json(&["profile", "top", &stall(), "--frames", "100..104"]);
    assert_eq!(few["range"]["frames"], 5);
    assert!(few["frame_ms"]["max"].as_f64().unwrap() < 5.0, "{few}");
    let calls = json(&["profile", "top", &stall(), "--sort", "calls", "--by-detail"]);
    assert!(calls["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["scope"] == "script::call (scripts/stall.rhai)"));
    let worst = json(&["profile", "top", &stall(), "--sort", "worst"]);
    assert_eq!(worst["scopes"][0]["scope"], "tick");
    assert_eq!(worst["scopes"][1]["scope"], "run_scripts");
}

#[test]
fn frame_prints_the_tree_and_json_carries_it() {
    let out = text(&["profile", "frame", &stall(), "hitch"]);
    assert!(
        out.contains("frame 113: 104.29 ms (median 4.29 over 11 normal frames)"),
        "{out}"
    );
    // Indented: the stall four levels down, each with its share of the frame
    assert!(
        out.contains("\n        stall.rhai:on_update  ")
            || out.contains("      stall.rhai:on_update"),
        "{out}"
    );
    let r = row(&out, "stall.rhai:on_update");
    assert_eq!(r[0], "100.05");
    assert_eq!(r[1], "95.9%");
    assert!(
        out.contains("phases: input 0.050, scripts 100.39, sync 0.10"),
        "{out}"
    );
    let doc = json(&["profile", "frame", &stall(), "hitch"]);
    let main = &doc["threads"][0];
    assert_eq!(main["thread"], "main");
    let tick = &main["scopes"][0];
    assert_eq!(tick["scope"], "tick");
    let leaf = &tick["children"][0]["children"][1]["children"][0];
    assert_eq!(leaf["scope"], "stall.rhai:on_update");
    assert_eq!(leaf["ms"], 100.05);
    assert_eq!(ms(&leaf["ms"]), r[0]);
    assert_eq!(doc["phases"]["scripts"], 100.39);

    // Pruned below a threshold; worst is the same frame; a frame by number
    let pruned = text(&["profile", "frame", &stall(), "worst", "--min-ms", "1"]);
    assert!(pruned.contains("more under the threshold"), "{pruned}");
    assert!(!pruned.contains("physics::step"), "{pruned}");
    let io = json(&["profile", "frame", &stall(), "100"]);
    assert_eq!(io["threads"][1]["thread"], "io");
    assert_eq!(
        io["threads"][1]["scopes"][0]["scope"],
        "decode_image (textures/moss.png)"
    );
}

#[test]
fn diff_names_what_moved_and_marks_what_the_spread_explains() {
    let out = text(&["profile", "diff", &calm(), &stall()]);
    let r = row(&out, "encode_to");
    assert_eq!(&r[..3], ["0.40", "0.90", "+0.50"], "{out}");
    // One 100 ms stall in 14 frames is a mean of 7 ms a frame, and inside its own spread
    assert!(!out.contains("stall.rhai:on_update"), "{out}");
    assert!(out.contains("--all lists them, marked ~"), "{out}");
    assert!(
        out.contains("the frame time's delta is inside its own spread"),
        "{out}"
    );

    let doc = json(&["profile", "diff", &calm(), &stall()]);
    let enc = &doc["scopes"][0];
    assert_eq!(enc["scope"], "encode_to");
    assert_eq!(enc["delta_mean_ms"], 0.5);
    assert_eq!(enc["delta_p90_ms"], 0.5);
    assert_eq!(enc["within_noise"], false);
    assert_eq!(doc["frame"]["within_noise"], true);

    let all = text(&["profile", "diff", &calm(), &stall(), "--all"]);
    let r = row(&all, "stall.rhai:on_update");
    assert_eq!(r.last(), Some(&"~"), "{all}");
    let r = row(&all, "import_model");
    assert_eq!(r.last(), Some(&"b"), "`only in b`:\n{all}");

    // A trace against itself moves nothing
    let same = json(&["profile", "diff", &stall(), &stall()]);
    assert_eq!(same["scopes"].as_array().unwrap().len(), 0, "{same}");
}

#[test]
fn bad_input_is_refused_with_what_was_wanted() {
    let refused = |args: &[&str], says: &str| {
        let o = flint(args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(says), "{args:?}: {err}");
    };
    refused(&["profile", "frame", &stall(), "9"], "frame 9 is not in");
    refused(
        &["profile", "frame", &stall(), "fast"],
        "give a frame index, `worst` or `hitch`",
    );
    refused(&["profile", "frame", &calm(), "hitch"], "holds no hitch");
    refused(
        &["profile", "top", &stall(), "--thread", "gpu"],
        "it has main, io",
    );
    refused(
        &["profile", "top", &stall(), "--frames", "9..3"],
        "not a range of frames",
    );
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("profile_cli");
    std::fs::create_dir_all(&dir).unwrap();
    let newer: PathBuf = dir.join("newer.json");
    std::fs::write(
        &newer,
        r#"{"traceEvents":[],"otherData":{"format":"flint-trace","version":2}}"#,
    )
    .unwrap();
    refused(
        &["profile", "top", newer.to_str().unwrap()],
        "trace version 2; this flint reads up to version 1",
    );
    // Under --json a refusal is still one document
    let o = flint(&["--json", "profile", "frame", &stall(), "9"]);
    let doc: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(doc["error"]["message"]
        .as_str()
        .unwrap()
        .contains("frame 9"));
}
