//! `flint frame` (#1087, ADR 0126 decision 7), end to end on the frozen
//! captures in `tests/fixtures/frames/` (a crate and a lantern; the same
//! scene with the lantern taken out): the summary and every view in text
//! and JSON with the same numbers, the diff naming the lantern's draws and
//! the pass images that differ, a pass's image copied out viewable, and a
//! newer manifest refused. One test makes a fresh capture with today's
//! writer and reads it, skipping itself without a GPU.

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

fn two() -> PathBuf {
    fixture("two_crates.flintframe")
}

fn one() -> PathBuf {
    fixture("one_crate.flintframe")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("frame_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Output {
    let mut cmd = std::process::Command::new(FLINT);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("run flint")
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

fn manifest(capture: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(capture.join("manifest.json")).unwrap()).unwrap()
}

fn close(a: &Value, b: &Value) -> bool {
    (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-6
}

/// Every pass of a manifest, sub-passes after their pass.
fn all_passes(m: &Value) -> Vec<Value> {
    fn walk(passes: &Value, out: &mut Vec<Value>) {
        for p in passes.as_array().into_iter().flatten() {
            out.push(p.clone());
            walk(&p["passes"], out);
        }
    }
    let mut out = Vec::new();
    for panel in m["panels"].as_array().into_iter().flatten() {
        walk(&panel["passes"], &mut out);
    }
    walk(&m["passes"], &mut out);
    out
}

#[test]
fn show_prints_the_summary_and_its_json_carries_the_same_numbers() {
    let m = manifest(&two());
    let text = ok(&flint(&[&"frame", &"show", &two()]));
    for want in [
        "Crate and Lantern",
        "tests/fixtures/frames/two_crates.scene.toml",
        "RTX 5070",
        "64x36",
        "3 draws, 36 triangles; shadows 9 draws, 108 triangles",
        "Most expensive passes",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    let doc = json(&flint(&[&"--json", &"frame", &"show", &two()]));
    let s = &doc["summary"];
    assert_eq!(s["draws"]["draw_calls"], m["stats"]["draw_calls"]);
    assert_eq!(s["draws"]["triangles"], m["stats"]["triangles"]);
    assert_eq!(
        s["draws"]["shadow_draw_calls"],
        m["stats"]["shadow_draw_calls"]
    );
    assert_eq!(s["memory"]["target_bytes"], m["memory"]["target_bytes"]);
    assert_eq!(s["frame"]["adapter"], m["frame"]["adapter"]);
    assert_eq!(s["scene"]["name"], "Crate and Lantern");
    assert!(close(
        &s["time"]["gpu_frame_ms"],
        &m["frame"]["gpu_frame_ms"]
    ));
    // The top pass is the manifest's most expensive, and the text shows it
    let passes = all_passes(&m);
    let slowest = passes
        .iter()
        .filter(|p| p["gpu_ms"].is_number())
        .max_by(|a, b| {
            a["gpu_ms"]
                .as_f64()
                .partial_cmp(&b["gpu_ms"].as_f64())
                .unwrap()
        })
        .unwrap();
    assert_eq!(s["top_passes"][0]["pass"], slowest["name"]);
    assert!(close(&s["top_passes"][0]["gpu_ms"], &slowest["gpu_ms"]));
    let shown = format!("{:.3}", slowest["gpu_ms"].as_f64().unwrap());
    assert!(text.contains(&shown), "{shown} missing from:\n{text}");
    assert_eq!(
        s["passes"]["total"].as_u64().unwrap() as usize,
        passes.len()
    );
}

#[test]
fn each_view_answers_in_text_and_json() {
    let m = manifest(&two());
    // Passes: every one, in order, skipped ones with why
    let text = ok(&flint(&[&"frame", &"show", &two(), &"--passes"]));
    assert!(text.contains("Shadow cascade 2"), "{text}");
    assert!(text.contains("skipped: no grass in the scene"), "{text}");
    let doc = json(&flint(&[&"--json", &"frame", &"show", &two(), &"--passes"]));
    let rows = doc["passes"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["pass"].as_str().unwrap()).collect();
    let want: Vec<String> = all_passes(&m)
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, want);

    // Draws: by entity, by pass, by triangles, and the culls
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"show",
        &two(),
        &"--draws",
        &"--entity",
        &"lantern",
    ]));
    let draws = doc["draws"].as_array().unwrap();
    assert_eq!(
        draws.len(),
        4,
        "the scene's draw and three cascades': {draws:#?}"
    );
    assert!(draws.iter().all(|d| d["entity"] == "lantern"));
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"show",
        &two(),
        &"--pass",
        &"Scene",
    ]));
    let draws = doc["draws"].as_array().unwrap();
    assert_eq!(
        draws.len() as u64,
        m["stats"]["draw_calls"].as_u64().unwrap()
    );
    assert!(draws.iter().all(|d| d["pass"] == "Scene"));
    let text = ok(&flint(&[
        &"frame",
        &"show",
        &two(),
        &"--pass",
        &"Scene",
        &"--sort",
        &"tris",
    ]));
    assert!(text.contains("crate") && text.contains("pbr"), "{text}");
    let missing = flint(&[&"frame", &"show", &two(), &"--entity", &"lanter"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("did you mean `lantern`"));

    // Targets: each with its memory and the passes that wrote it
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"show",
        &two(),
        &"--targets",
    ]));
    let targets = doc["targets"].as_array().unwrap();
    assert_eq!(targets.len(), m["targets"].as_array().unwrap().len());
    let hdr = targets.iter().find(|t| t["id"] == "hdr_scene").unwrap();
    assert_eq!(hdr["written_by"], serde_json::json!(["Scene"]));
    let text = ok(&flint(&[&"frame", &"show", &two(), &"--targets"]));
    assert!(text.contains("Rgba16Float"), "{text}");

    // The hitch window: flint render keeps no frame history, and says so
    let text = ok(&flint(&[&"frame", &"show", &two(), &"--hitch"]));
    assert!(text.contains("no frame history"), "{text}");
    let doc = json(&flint(&[&"--json", &"frame", &"show", &two(), &"--hitch"]));
    assert!(doc["hitch"]["hitch"].is_null());
}

#[test]
fn diff_names_the_entity_taken_out_and_the_images_that_differ() {
    let out = tmp("diff");
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"diff",
        &two(),
        &one(),
        &"--out",
        &out,
    ]));
    let vanished: Vec<(String, String)> = doc["draws"]["vanished"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["pass"].as_str().unwrap().into(),
                v["entity"].as_str().unwrap().into(),
            )
        })
        .collect();
    for pass in [
        "Scene",
        "Shadow cascade 0",
        "Shadow cascade 1",
        "Shadow cascade 2",
    ] {
        assert!(
            vanished.contains(&(pass.to_string(), "lantern".to_string())),
            "{vanished:?}"
        );
    }
    assert!(
        vanished.iter().all(|(_, e)| e == "lantern"),
        "only the lantern left: {vanished:?}"
    );
    assert!(doc["draws"]["appeared"].as_array().unwrap().is_empty());
    assert_eq!(doc["frame"]["draw_calls"]["delta"], -1.0);
    assert!(doc["passes"]["added"].as_array().unwrap().is_empty());
    let images = doc["images"].as_array().unwrap();
    let differs = |key: &str| {
        images
            .iter()
            .find(|i| i["image"] == key)
            .unwrap_or_else(|| panic!("{key} not in {images:#?}"))
    };
    for key in ["Scene / hdr_scene", "Scene / depth", "Composite / surface"] {
        let image = differs(key);
        assert_eq!(image["status"], "differs", "{image}");
        assert!(image["differing"].as_u64().unwrap() > 0);
        let diff = PathBuf::from(image["diff"].as_str().unwrap());
        let png = image::open(&diff).unwrap();
        assert_eq!((png.width(), png.height()), (64, 36));
    }
    // A pass the lantern never reached is the same
    assert!(images.iter().any(|i| i["status"] == "same"));

    let text = ok(&flint(&[&"frame", &"diff", &two(), &one()]));
    assert!(
        text.contains("removed  lantern in Scene: 1 draw: pbr"),
        "{text}"
    );
    assert!(text.contains("Scene / hdr_scene"), "{text}");
    // A capture against itself is the same everywhere
    let text = ok(&flint(&[&"frame", &"diff", &two(), &two()]));
    assert!(text.contains("every entity draws as it did"), "{text}");
    assert!(text.contains("every pass image is the same"), "{text}");
}

/// Copy a capture folder (manifest and images) to `to`.
fn copy_capture(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.join("targets")).unwrap();
    std::fs::copy(from.join("manifest.json"), to.join("manifest.json")).unwrap();
    for f in std::fs::read_dir(from.join("targets")).unwrap() {
        let f = f.unwrap();
        std::fs::copy(f.path(), to.join("targets").join(f.file_name())).unwrap();
    }
}

#[test]
fn a_vanished_entity_of_many_draws_counts_its_kinds() {
    // The lantern drawn as 40 parts, as a skinned body is (#1208: Float's
    // `body`, 91 draws a pass, printed its kind 91 times), and in the
    // scene pass one part drawn by another pipeline
    let lantern = tmp("many-parts").join("lantern.flintframe");
    copy_capture(&two(), &lantern);
    let mut m = manifest(&lantern);
    fn repeat(passes: &mut Value) {
        for p in passes.as_array_mut().into_iter().flatten() {
            let scene = p["name"] == "Scene";
            if let Some(draws) = p["draws"].as_array_mut() {
                if let Some(d) = draws.iter().find(|d| d["entity"] == "lantern").cloned() {
                    for _ in 1..40 {
                        draws.push(d.clone());
                    }
                    if scene {
                        draws.last_mut().unwrap()["pipeline"] = "unlit".into();
                    }
                }
            }
            repeat(&mut p["passes"]);
        }
    }
    repeat(&mut m["passes"]);
    std::fs::write(lantern.join("manifest.json"), m.to_string()).unwrap();

    let text = ok(&flint(&[&"frame", &"diff", &lantern, &one()]));
    assert!(
        text.contains("removed  lantern in Shadow cascade 0: 40 draws: shadow\n"),
        "one kind is named once: {text}"
    );
    assert!(
        text.contains("removed  lantern in Scene: 40 draws (39 pbr, 1 unlit)\n"),
        "several are counted, most first: {text}"
    );
    assert!(!text.contains("pbr, pbr"), "{text}");
    // and an entity that appears reads the same way
    let text = ok(&flint(&[&"frame", &"diff", &one(), &lantern]));
    assert!(
        text.contains("added    lantern in Shadow cascade 1: 40 draws: shadow\n"),
        "{text}"
    );
    // The JSON keeps every draw's pipeline, as before
    let doc = json(&flint(&[&"--json", &"frame", &"diff", &lantern, &one()]));
    let scene = doc["draws"]["vanished"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["pass"] == "Scene")
        .unwrap();
    assert_eq!(scene["draws"], 40);
    assert_eq!(scene["pipelines"].as_array().unwrap().len(), 40);
}

#[test]
fn image_copies_a_pass_out_as_a_png_to_look_at() {
    let out = tmp("image");
    // HDR, tonemapped
    let hdr = out.join("scene.png");
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"image",
        &two(),
        &"Scene",
        &"-o",
        &hdr,
    ]));
    assert_eq!(doc["written"][0]["target"], "hdr_scene");
    assert!(doc["written"][0]["view"]
        .as_str()
        .unwrap()
        .contains("tonemapped"));
    let png = image::open(&hdr).unwrap().to_rgba8();
    assert_eq!(png.dimensions(), (64, 36));
    assert!(png.pixels().any(|p| p.0[0] > 40), "not black");
    // The same pass's depth, by --target
    let depth = out.join("depth.png");
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"image",
        &two(),
        &"Scene",
        &"--target",
        &"depth",
        &"-o",
        &depth,
    ]));
    // The frozen capture is version 1 and names no camera (#1176): its
    // depth keeps the stretch over the stored values
    let view = doc["written"][0]["view"].as_str().unwrap();
    assert!(
        view.starts_with("depth") && view.contains("stretched") && !view.contains(" m"),
        "{view}"
    );
    assert!(manifest(&two()).get("camera").is_none());
    // An 8-bit image is copied byte for byte, and --raw keeps an EXR
    let surface = out.join("surface.png");
    ok(&flint(&[
        &"frame",
        &"image",
        &two(),
        &"Composite",
        &"-o",
        &surface,
    ]));
    assert_eq!(
        std::fs::read(&surface).unwrap(),
        std::fs::read(two().join("targets/13-surface.png")).unwrap()
    );
    let raw = out.join("hdr.exr");
    ok(&flint(&[
        &"frame",
        &"image",
        &two(),
        &"05-hdr_scene.exr",
        &"--raw",
        &"-o",
        &raw,
    ]));
    assert_eq!(
        std::fs::read(&raw).unwrap(),
        std::fs::read(two().join("targets/05-hdr_scene.exr")).unwrap()
    );
    // Every image at once
    let all = out.join("all");
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"image",
        &two(),
        &"--all",
        &"-o",
        &all,
    ]));
    let m = manifest(&two());
    assert_eq!(
        doc["written"].as_array().unwrap().len() as u64,
        m["capture"]["images"].as_u64().unwrap()
    );
    // A pass that drew nothing says why
    let none = flint(&[
        &"frame",
        &"image",
        &two(),
        &"FXAA",
        &"-o",
        &out.join("x.png"),
    ]);
    assert!(!none.status.success());
    assert!(String::from_utf8_lossy(&none.stderr).contains("skipped"));
}

#[test]
fn a_newer_manifest_is_refused_naming_both_versions() {
    let dir = tmp("newer").join("later.flintframe");
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = manifest(&two());
    m["version"] = 2.into();
    std::fs::write(dir.join("manifest.json"), m.to_string()).unwrap();
    let o = flint(&[&"frame", &"show", &dir]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("version 2 capture"), "{err}");
    assert!(err.contains("version 1 and older"), "{err}");
    // Under --json the refusal is the one document
    let o = flint(&[&"--json", &"frame", &"show", &dir]);
    assert!(!o.status.success());
    let doc: Value = serde_json::from_slice(&o.stdout).unwrap();
    let message = format!("{}", doc["error"]);
    assert!(message.contains("version 2"), "{message}");
    // and a folder that is no capture says what it is missing
    let empty = tmp("empty");
    let o = flint(&[&"frame", &"show", &empty]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no manifest.json"));
}

#[test]
fn a_fresh_capture_from_todays_writer_reads() {
    let dir = tmp("fresh");
    let capture = dir.join("fresh.flintframe");
    let o = flint(&[
        &"render",
        &fixture("two_crates.scene.toml"),
        &"--schemas",
        &repo().join("schemas"),
        &"--width",
        &"64",
        &"--height",
        &"36",
        &"--no-grid",
        &"--shadow-resolution",
        &"128",
        &"-o",
        &dir.join("fresh.png"),
        &"--frame-capture",
        &capture,
    ]);
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("headless render context")
    {
        eprintln!("no GPU adapter: skipped");
        return;
    }
    ok(&o);
    let doc = json(&flint(&[&"--json", &"frame", &"show", &capture]));
    let s = &doc["summary"];
    assert_eq!(s["written_by"], "flint render");
    // The camera it drew with (#1176), in the JSON and as a line
    let camera = &s["camera"];
    assert_eq!(camera["orthographic"], false, "{camera}");
    let (near, far) = (
        camera["near"].as_f64().unwrap(),
        camera["far"].as_f64().unwrap(),
    );
    assert!(near > 0.0 && far > near, "{camera}");
    let text = ok(&flint(&[&"frame", &"show", &capture]));
    assert!(
        text.lines()
            .any(|l| l.starts_with("Camera") && l.contains("perspective") && l.contains("near")),
        "{text}"
    );
    // so its depth is shown in metres, the range named in the view line;
    // a shadow cascade (a light's view) keeps the stretch
    let depth = dir.join("depth.png");
    let doc = json(&flint(&[
        &"--json",
        &"frame",
        &"image",
        &capture,
        &"Scene",
        &"--target",
        &"depth",
        &"-o",
        &depth,
    ]));
    let view = doc["written"][0]["view"].as_str().unwrap();
    assert!(
        view.starts_with("depth ") && view.contains(" m, log scale"),
        "{view}"
    );
    let range: Vec<f64> = view["depth ".len()..view.find(" m").unwrap()]
        .split("..")
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(
        near <= range[0] && range[0] < range[1] && range[1] <= far,
        "{view} lies between the planes {near}..{far}"
    );
    let png = image::open(&depth).unwrap().to_luma8();
    assert_eq!(
        png.pixels().map(|p| p.0[0]).max(),
        Some(255),
        "the nearest is white"
    );
    let shadow = json(&flint(&[
        &"--json",
        &"frame",
        &"image",
        &capture,
        &"Shadow cascade 0",
        &"-o",
        &dir.join("shadow.png"),
    ]));
    let view = shadow["written"][0]["view"].as_str().unwrap();
    assert!(view.contains("stretched"), "{view}");
    // The replay inputs carry no camera of their own: the manifest's is it
    let inputs: Value =
        serde_json::from_str(&std::fs::read_to_string(capture.join("replay/inputs.json")).unwrap())
            .unwrap();
    assert!(
        inputs.get("camera").is_none(),
        "written once, in the manifest"
    );
    assert_eq!(s["scene"]["name"], "Crate and Lantern");
    assert!(s["captured_at"].as_str().unwrap().ends_with('Z'));
    // It names the engine's commit, as the player's do (#1208): the one
    // the binary was built from (#1290), this checkout's HEAD
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(repo())
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if let Some(head) = head {
        let engine = &manifest(&capture)["engine"];
        assert_eq!(engine["commit"], head.as_str(), "{engine}");
        assert!(engine["dirty"].is_boolean(), "{engine}");
    }
    // The same scene as the frozen capture draws the same draws, and
    // the frozen one's images still match today's where nothing moves
    let doc = json(&flint(&[&"--json", &"frame", &"diff", &two(), &capture]));
    assert!(
        doc["draws"]["vanished"].as_array().unwrap().is_empty(),
        "{doc:#}"
    );
    assert!(
        doc["draws"]["appeared"].as_array().unwrap().is_empty(),
        "{doc:#}"
    );
    assert!(
        doc["passes"]["added"].as_array().unwrap().is_empty(),
        "{doc:#}"
    );
    assert!(
        doc["passes"]["removed"].as_array().unwrap().is_empty(),
        "{doc:#}"
    );
}
