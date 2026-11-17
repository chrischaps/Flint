//! `flint frame replay` and the window's replay (#1089, ADR 0126 decisions
//! 4 and 6), end to end: `flint render --frame-capture` writes `replay/`,
//! the frame is drawn again byte for byte, the scene pass steps a draw at a
//! time to the captured image, hiding the lantern is the scene with the
//! lantern taken out, a shadow layer, the style mask and the albedo step
//! their own draws to their captured images (#1204), a model that moved is
//! named rather than crashed on, and a capture with no replay inputs is
//! refused saying why. Each test that draws skips itself without a GPU.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    repo().join("tests/fixtures/frames").join(name)
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("frame_replay_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint_in(dir: Option<&Path>, args: &[&dyn AsRef<std::ffi::OsStr>]) -> Output {
    let mut cmd = std::process::Command::new(FLINT);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("run flint")
}

fn flint(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Output {
    flint_in(None, args)
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn json(o: &Output) -> Value {
    let text = ok(o);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not one JSON document ({e}):\n{text}"))
}

fn schemas() -> PathBuf {
    repo().join("schemas")
}

/// Capture `scene` at the fixtures' size into `out`; false without a GPU.
fn capture_in(dir: Option<&Path>, scene: &Path, out: &Path, schemas: &Path) -> bool {
    let o = flint_in(
        dir,
        &[
            &"render",
            &scene,
            &"--schemas",
            &schemas,
            &"--width",
            &"64",
            &"--height",
            &"36",
            &"--no-grid",
            &"--shadow-resolution",
            &"128",
            &"-o",
            &out.with_extension("png"),
            &"--frame-capture",
            &out,
        ],
    );
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("headless render context")
    {
        eprintln!("no GPU adapter: skipped");
        return false;
    }
    ok(&o);
    true
}

fn capture(scene: &Path, out: &Path) -> bool {
    capture_in(None, scene, out, &schemas())
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap()
}

fn scene_draws(m: &Value) -> Vec<String> {
    let pass = m["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Scene")
        .unwrap();
    pass["draws"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["entity"].as_str().unwrap_or("").to_string())
        .collect()
}

fn replay(capture: &Path, out: &Path, more: &[&str]) -> Value {
    let s = schemas();
    let mut args: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![
        &"--json",
        &"frame",
        &"replay",
        &capture,
        &"--schemas",
        &s,
        &"--out",
        &out,
    ];
    for m in more {
        args.push(m);
    }
    json(&flint(&args))
}

#[test]
fn a_capture_replays_byte_for_byte_and_steps_its_scene_pass_to_the_captured_image() {
    let dir = tmp("steps");
    let cap = dir.join("two.flintframe");
    if !capture(&fixture("two_crates.scene.toml"), &cap) {
        return;
    }
    // The capture holds its replay inputs, and names them
    for f in ["world.toml", "inputs.json", "skinning.bin", "particles.bin"] {
        assert!(cap.join("replay").join(f).is_file(), "no replay/{f}");
    }
    let m = manifest(&cap);
    assert_eq!(m["replay"]["available"], true);
    let captured = scene_draws(&m);
    assert_eq!(captured, ["crate", "ground", "lantern"]);

    // Whole: byte-identical, final image and scene pass alike
    let whole = replay(&cap, &dir.join("whole.flintframe"), &[]);
    assert_eq!(whole["identical"], true, "{whole:#}");
    assert_eq!(whole["final"]["differing"], 0);
    assert_eq!(whole["scene"]["differing"], 0);
    assert_eq!(whole["scene_draws"], 3);
    let written = manifest(&dir.join("whole.flintframe"));
    assert_eq!(written["written_by"], "flint frame replay");

    // A draw at a time: each step draws the first N, the last is the frame
    for n in 0..=3usize {
        let out = dir.join(format!("d{n}.flintframe"));
        let r = replay(&cap, &out, &["--draws", &n.to_string()]);
        assert_eq!(scene_draws(&manifest(&out)), captured[..n], "step {n}");
        assert_eq!(r["identical"], n == 3, "step {n}: {r:#}");
    }
    // The steps before the last are the scene pass being built
    let d2 = manifest(&dir.join("d2.flintframe"));
    let culled = d2["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Scene")
        .unwrap()["culled"]
        .clone();
    assert_eq!(culled[0]["entity"], "lantern");
    assert_eq!(culled[0]["culled"], "past the draw limit");

    // An entity's pixels marked: the selection outline's pass draws them
    let out = dir.join("outline.flintframe");
    let r = replay(&cap, &out, &["--outline", "crate"]);
    assert_eq!(r["controls"]["outline"], "crate");
    assert_eq!(
        r["scene"]["differing"], 0,
        "the outline drew into the scene"
    );
    assert!(r["final"]["differing"].as_u64().unwrap() > 0);
    let outline = manifest(&out)["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Selection outline")
        .cloned()
        .unwrap();
    assert_eq!(outline["ran"], true);

    // The window steps it too: a screenshot of the replay at one draw
    let shot = dir.join("open.png");
    let o = flint(&[
        &"frame",
        &"open",
        &cap,
        &"--schemas",
        &schemas(),
        &"--draws",
        &"1",
        &"--screenshot",
        &shot,
        &"--width",
        &"1280",
        &"--height",
        &"800",
    ]);
    ok(&o);
    assert!(std::fs::metadata(&shot).unwrap().len() > 10_000);
}

#[test]
fn hiding_the_lantern_draws_the_scene_written_without_it_and_its_times_are_its_own() {
    let dir = tmp("hide");
    let two = dir.join("two.flintframe");
    let one = dir.join("one.flintframe");
    if !capture(&fixture("two_crates.scene.toml"), &two) {
        return;
    }
    assert!(capture(&fixture("one_crate.scene.toml"), &one));
    let hidden = dir.join("hidden.flintframe");
    let r = replay(&two, &hidden, &["--hide", "lantern"]);
    assert_eq!(r["identical"], false);
    assert_eq!(r["controls"]["hidden"][0], "lantern");
    let m = manifest(&hidden);
    assert_eq!(scene_draws(&m), ["crate", "ground"]);
    // Its passes carry the replay's own GPU times: what the window's bars show
    let timed = m["passes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["gpu_ms"].is_number())
        .count();
    let adapter_times = manifest(&two)["passes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["gpu_ms"].is_number());
    if adapter_times {
        assert!(timed > 0, "the replay has no GPU times");
    }

    // Pass for pass, the frame the scene without the lantern draws
    let diff = json(&flint(&[&"--json", &"frame", &"diff", &hidden, &one]));
    let images = diff["images"].as_array().unwrap();
    assert!(!images.is_empty());
    for image in images {
        assert_eq!(image["status"], "same", "{image:#}");
    }
}

#[test]
fn a_capture_whose_model_moved_is_replayed_with_the_model_named() {
    let dir = tmp("moved");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::copy(
        repo().join("tests/fixtures/models/DoubleSidedCard.glb"),
        dir.join("models/card.glb"),
    )
    .unwrap();
    let scene = std::fs::read_to_string(fixture("two_crates.scene.toml"))
        .unwrap()
        .replace(
            "[entities.lantern.bounds]",
            "[entities.lantern.model]\nasset = \"card\"\n\n[entities.lantern.bounds]",
        );
    std::fs::write(dir.join("card.scene.toml"), scene).unwrap();
    let cap = dir.join("cap.flintframe");
    if !capture_in(Some(&dir), Path::new("card.scene.toml"), &cap, &schemas()) {
        return;
    }
    let assets = &manifest(&cap)["replay"]["assets"];
    assert_eq!(assets[0]["name"], "card");

    // In place: as captured, the model's split parts and all
    let o = flint_in(
        Some(&dir),
        &[
            &"--json",
            &"frame",
            &"replay",
            &cap,
            &"--schemas",
            &schemas(),
        ],
    );
    let r = json(&o);
    assert_eq!(r["identical"], true, "{r:#}");
    assert_eq!(r["assets"][0]["state"], "same");

    // Moved: named, and the frame still drawn
    std::fs::rename(dir.join("models/card.glb"), dir.join("models/card_v2.glb")).unwrap();
    let o = flint_in(
        Some(&dir),
        &[
            &"--json",
            &"frame",
            &"replay",
            &cap,
            &"--schemas",
            &schemas(),
        ],
    );
    let r = json(&o);
    assert_eq!(r["assets"][0]["state"], "missing");
    let notes = r["notes"].to_string();
    assert!(notes.contains("model card is missing"), "{notes}");
    assert_eq!(r["identical"], false);
}

#[test]
fn a_capture_without_replay_inputs_is_refused_saying_why() {
    // The frozen fixture predates replay/: its static views still read
    let o = flint(&[&"frame", &"replay", &fixture("two_crates.flintframe")]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("holds no replay inputs"), "{err}");
    assert!(err.contains("--frame-capture"), "{err}");
}

/// The pass named `name`, a shadow layer under `Shadows` included.
fn pass_named<'a>(m: &'a Value, name: &str) -> &'a Value {
    fn find<'a>(passes: &'a Value, name: &str) -> Option<&'a Value> {
        passes.as_array()?.iter().find_map(|p| {
            if p["name"] == name {
                Some(p)
            } else {
                find(&p["passes"], name)
            }
        })
    }
    find(&m["passes"], name).unwrap_or_else(|| panic!("no {name} pass"))
}

/// A pass's draws, as `entity / mesh`.
fn draws_of(m: &Value, pass: &str) -> Vec<String> {
    pass_named(m, pass)["draws"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| format!("{} / {}", d["entity"], d["mesh"]))
        .collect()
}

#[test]
fn a_shadow_layer_steps_a_caster_at_a_time_to_the_captured_layer_and_no_other_moves() {
    let dir = tmp("shadow_steps");
    let cap = dir.join("two.flintframe");
    if !capture(&fixture("two_crates.scene.toml"), &cap) {
        return;
    }
    let casters = draws_of(&manifest(&cap), "Shadow cascade 0");
    assert_eq!(casters.len(), 3, "{casters:?}");
    for n in 0..=casters.len() {
        let out = dir.join(format!("c{n}.flintframe"));
        let r = replay(
            &cap,
            &out,
            &["--pass", "Shadow cascade 0", "--draws", &n.to_string()],
        );
        let m = manifest(&out);
        assert_eq!(draws_of(&m, "Shadow cascade 0"), casters[..n], "step {n}");
        assert_eq!(r["controls"]["pass"], "Shadow cascade 0");
        assert_eq!(r["pass_draws"], 3);
        // The layer's own image: the captured one at the last step
        assert_eq!(
            r["pass"]["differing"] == 0,
            n == casters.len(),
            "step {n}: {r:#}"
        );
        assert_eq!(r["identical"], n == casters.len(), "step {n}");
        // The scene pass draws every draw, and the other layers are whole
        assert_eq!(scene_draws(&m), scene_draws(&manifest(&cap)), "step {n}");
        let diff = json(&flint(&[&"--json", &"frame", &"diff", &cap, &out]));
        for image in diff["images"].as_array().unwrap() {
            let name = image["image"].as_str().unwrap();
            if name.starts_with("Shadow cascade 1") || name.starts_with("Shadow cascade 2") {
                assert_eq!(image["status"], "same", "step {n}: {image:#}");
            }
        }
        if n > 0 && n < casters.len() {
            let culled = &pass_named(&m, "Shadow cascade 0")["culled"];
            assert!(
                culled
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["culled"] == "past the draw limit"),
                "step {n}: {culled:#}"
            );
        }
    }

    // A pass that can't be stepped is refused, naming those that can
    let o = flint(&[
        &"frame", &"replay", &cap, &"--pass", &"Bloom", &"--draws", &"1",
    ]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("Shadow cascade 0") && err.contains("Style mask"),
        "{err}"
    );
}

#[test]
fn the_style_mask_and_albedo_step_their_draws_to_the_captured_images() {
    let dir = tmp("style_steps");
    let cap = dir.join("props.flintframe");
    let o = flint(&[
        &"render",
        &repo().join("tests/fixtures/scenes/stylize_props.scene.toml"),
        &"--width",
        &"96",
        &"--height",
        &"54",
        // Pencil's light tone divides by the albedo, so both passes run
        &"--set",
        &"pencil.tone=light",
        &"-o",
        &dir.join("props.png"),
        &"--frame-capture",
        &cap,
    ]);
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("headless render context")
    {
        eprintln!("no GPU adapter: skipped");
        return;
    }
    ok(&o);
    let m = manifest(&cap);
    for pass in ["Style mask", "Style albedo"] {
        let all = draws_of(&m, pass);
        assert!(all.len() >= 4, "{pass}: {all:?}");
        let mut last = None;
        for n in [0, all.len() / 2, all.len()] {
            let out = dir.join(format!("{}-{n}.flintframe", pass.replace(' ', "_")));
            let r = json(&flint(&[
                &"--json",
                &"frame",
                &"replay",
                &cap,
                &"--out",
                &out,
                &"--pass",
                &pass,
                &"--draws",
                &n.to_string(),
            ]));
            assert_eq!(draws_of(&manifest(&out), pass), all[..n], "{pass} step {n}");
            let differing = r["pass"]["differing"].as_u64().unwrap();
            if n == all.len() {
                assert_eq!(
                    differing, 0,
                    "{pass}: the last step isn't the captured image"
                );
            } else {
                assert!(differing > 0, "{pass} step {n} drew the whole pass");
            }
            // Each step is nearer the captured image than the one before
            if let Some(before) = last {
                assert!(
                    differing < before,
                    "{pass} step {n}: {differing} >= {before}"
                );
            }
            last = Some(differing);
        }
    }
}
