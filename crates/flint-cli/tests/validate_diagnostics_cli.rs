//! `flint validate` reports everything wrong with a scene in one run (ADR
//! 0115, #683): the real binary on a small project whose scene misspells a
//! field, a component and an archetype and gives an enum a value it doesn't
//! have. Each is named with its entity, component and field and a
//! suggestion, the exit code is non-zero, `--format json` carries the same
//! list, and `--strict` (or `[project] strict = true`) makes the unknown
//! field an error. The project has no constraint files: validate used to
//! return before loading the scene in that case.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const MANIFEST: &str = "[project]\nid = \"lamps\"\nschemas = [\"schemas\"]\n";

const FILES: &[(&str, &str)] = &[
    (
        "schemas/components/lamp.toml",
        "[component.lamp]\ndescription = \"A lamp\"\n\n[component.lamp.fields]\n\
         brightness = { type = \"f32\", default = 1.0, min = 0.0 }\n\
         kind = { type = \"enum\", values = [\"point\", \"spot\"], default = \"point\" }\n",
    ),
    (
        "schemas/components/place.toml",
        "[component.place]\n\n[component.place.fields]\n\
         position = { type = \"vec3\", default = [0, 0, 0] }\n",
    ),
    // Declared by name only: open, for a component only scripts read
    (
        "schemas/components/inventory.toml",
        "[component.inventory]\ndescription = \"Read by scripts\"\n",
    ),
    (
        "schemas/archetypes/lamp.toml",
        "[archetype.lamp]\ndescription = \"A lamp\"\ncomponents = [\"lamp\", \"place\"]\n\n\
         [archetype.lamp.defaults.lamp]\nbrightness = 2.0\n",
    ),
    (
        "scenes/good.scene.toml",
        "[scene]\nname = \"Good\"\n\n[entities.desk_lamp]\narchetype = \"lamp\"\n\n\
         [entities.desk_lamp.lamp]\nkind = \"spot\"\n\n\
         [entities.chest.inventory]\nslots = 4\n",
    ),
    (
        "scenes/broken.scene.toml",
        "[scene]\nname = \"Broken\"\n\n\
         [entities.desk_lamp]\narchetype = \"lmap\"\n\n\
         [entities.desk_lamp.lamp]\nbrightnes = 3.0\nkind = \"spto\"\n\n\
         [entities.floor_lamp.lapm]\nbrightness = 1.0\n\n\
         [entities.floor_lamp.place]\nposition = \"left\"\n",
    ),
    (
        "scenes/typo_only.scene.toml",
        "[scene]\nname = \"Typo\"\n\n[entities.desk_lamp.lamp]\nbrightnes = 3.0\n",
    ),
    (
        "scenes/not_toml.scene.toml",
        "[scene]\nname = \"Not TOML\"\n\n[entities.desk_lamp\n",
    ),
];

fn project(name: &str, manifest: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES.iter().chain(&[("flint.toml", manifest)]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The ticket's first two boxes, through the binary.
#[test]
fn every_mistake_is_reported_in_one_run_and_fails() {
    let dir = project("text", MANIFEST);
    let o = flint(&dir, &["validate", "scenes/broken.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.starts_with("scenes/broken.scene.toml: 4 errors, 1 warning\n"),
        "{out}"
    );
    for expected in [
        // the archetype
        "  error[unknown-archetype] entity 'desk_lamp' field 'archetype': no archetype named \
         `lmap`, so the entity gets no defaults\n      hint: did you mean `lamp`?\n",
        // the enum value
        "  error[bad-enum-value] entity 'desk_lamp' lamp.kind: \"spto\" is not a value of \
         `kind`\n      hint: did you mean `spot`? allowed values: point, spot\n",
        // the component
        "  error[unknown-component] entity 'floor_lamp' component 'lapm': no component named \
         `lapm`, so nothing checks it\n      hint: did you mean `lamp`?\n",
        // a wrong type
        "  error[bad-type] entity 'floor_lamp' place.position: expected vec3, got string\n",
        // the field: a warning
        "  warning[unknown-field] entity 'desk_lamp' lamp.brightnes: `lamp` has no field \
         `brightnes`\n      hint: did you mean `brightness`?\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }
    // Errors come before warnings
    assert!(out.find("warning[").unwrap() > out.rfind("error[").unwrap());
}

#[test]
fn json_carries_the_same_list() {
    let dir = project("json", MANIFEST);
    let o = flint(
        &dir,
        &["validate", "scenes/broken.scene.toml", "--format", "json"],
    );
    assert_eq!(o.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["errors"], 4);
    assert_eq!(doc["warnings"], 1);
    let found: Vec<(String, String, String, String, String)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let s = |key: &str| d[key].as_str().unwrap_or("").to_string();
            (
                s("severity"),
                s("code"),
                s("entity"),
                s("component"),
                s("field"),
            )
        })
        .collect();
    let row = |a: &str, b: &str, c: &str, d: &str, e: &str| {
        (a.into(), b.into(), c.into(), d.into(), e.into())
    };
    assert_eq!(
        found,
        [
            row("error", "unknown-archetype", "desk_lamp", "", "archetype"),
            row("warning", "unknown-field", "desk_lamp", "lamp", "brightnes"),
            row("error", "bad-enum-value", "desk_lamp", "lamp", "kind"),
            row("error", "unknown-component", "floor_lamp", "lapm", ""),
            row("error", "bad-type", "floor_lamp", "place", "position"),
        ]
    );
    let first = &doc["diagnostics"][0];
    assert_eq!(first["file"], "scenes/broken.scene.toml");
    assert_eq!(first["hint"], "did you mean `lamp`?");
}

/// A clean scene passes with no constraint files, and a component declared
/// by name only (no fields) is known and unchecked.
#[test]
fn a_clean_scene_passes_and_an_open_component_is_known() {
    let dir = project("clean", MANIFEST);
    let o = flint(&dir, &["validate", "scenes/good.scene.toml"]);
    let out = stdout(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.starts_with("No problems found.\n"), "{out}");
    assert!(out.contains("No constraints found"), "{out}");
}

/// Decision 1: an unknown field alone is a warning and passes; `--strict`
/// and `[project] strict = true` make it an error.
#[test]
fn strict_makes_an_unknown_field_an_error() {
    let scene = "scenes/typo_only.scene.toml";
    let dir = project("lenient", MANIFEST);
    let o = flint(&dir, &["validate", scene]);
    let out = stdout(&o);
    assert!(o.status.success(), "a warning alone passes: {out}");
    assert!(out.contains("warning[unknown-field]"), "{out}");
    assert!(out.contains(": 1 warning\n"), "{out}");

    let o = flint(&dir, &["validate", scene, "--strict"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("error[unknown-field] entity 'desk_lamp' lamp.brightnes"),
        "{out}"
    );

    let strict = project(
        "strict_manifest",
        "[project]\nschemas = [\"schemas\"]\nstrict = true\n",
    );
    let o = flint(&strict, &["validate", scene]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("error[unknown-field]"),
        "{}",
        stdout(&o)
    );
}

/// #795: `[project.diagnostics] allow` leaves out what it names, at the
/// place it names, and `flint validate` and `flint check` say how many,
/// in text and JSON. An allowed error no longer fails the scene; an entry
/// that names no code fails every command, naming the key.
#[test]
fn the_manifest_allows_a_code_and_validate_counts_it() {
    let manifest = "[project]\nschemas = [\"schemas\"]\n\n[project.diagnostics]\n\
                    allow = [\"unknown-field:lamp.brightnes\", \"unknown-component:lapm\"]\n";
    let dir = project("allowed", manifest);
    let broken = "scenes/broken.scene.toml";

    let o = flint(&dir, &["validate", broken]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "three errors are left: {out}");
    assert!(
        out.starts_with("scenes/broken.scene.toml: 3 errors (2 allowed by flint.toml)\n"),
        "{out}"
    );
    assert!(!out.contains("lamp.brightnes"), "{out}");
    assert!(!out.contains("lapm"), "{out}");
    assert!(
        out.contains("error[bad-enum-value]"),
        "the rest stays: {out}"
    );

    let o = flint(&dir, &["validate", broken, "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["allowed"], 2, "{doc}");
    assert_eq!(doc["errors"], 3, "{doc}");
    assert_eq!(doc["warnings"], 0, "{doc}");

    let o = flint(&dir, &["check", broken, "--json"]);
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["allowed"], 2, "{doc}");
    let summary = doc["summary"].as_str().unwrap();
    assert!(summary.ends_with("(2 allowed by flint.toml)"), "{summary}");

    // The one warning allowed: nothing left, and still said
    let o = flint(&dir, &["validate", "scenes/typo_only.scene.toml"]);
    let out = stdout(&o);
    assert!(o.status.success(), "{out}");
    assert!(
        out.starts_with("scenes/typo_only.scene.toml: no problems (1 allowed by flint.toml)\n"),
        "{out}"
    );

    let bad = project(
        "allowed_typo",
        "[project.diagnostics]\nallow = [\"unknown-feild\"]\n",
    );
    let o = flint(&bad, &["validate", broken]);
    let said = String::from_utf8_lossy(&o.stderr);
    assert_ne!(o.status.code(), Some(0), "{said}");
    assert!(said.contains("project.diagnostics.allow[0]"), "{said}");
    assert!(said.contains("did you mean `unknown-field`?"), "{said}");
}

/// #339's scene: three mistakes in one component, with a constraint that
/// passes. Before ADR 0115 the loader logged a warning for the first field
/// (`cell`), and validate printed "All constraints passed." and exited 0.
/// Every field is named, and the scene fails although every constraint
/// passes. The schema copies the three fields of the engine's `ascii_style`
/// as they were when #339 was filed, so the engine's file can change under it.
#[test]
fn every_field_of_one_component_is_named_and_fails_when_constraints_pass() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join("one_component");
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        ("flint.toml", MANIFEST),
        (
            "schemas/components/ascii_style.toml",
            "[component.ascii_style]\ndescription = \"#339's copy\"\n\n\
             [component.ascii_style.fields]\n\
             cell = { type = \"f32\", min = 4.0, max = 32.0 }\n\
             color = { type = \"enum\", values = [\"source\", \"tint\", \"raw\", \"mono\", \
             \"palette\"] }\n\
             tint = { type = \"vec3\" }\n",
        ),
        (
            // Passes: the scene has no lamp
            "schemas/constraints/lamps.toml",
            "[[constraint]]\nname = \"lamps_have_transform\"\n\
             query = \"entities where archetype == 'lamp'\"\nseverity = \"error\"\n\
             message = \"Lamp '{name}' has no transform\"\n\n\
             [constraint.kind]\ntype = \"required_component\"\narchetype = \"lamp\"\n\
             component = \"transform\"\n",
        ),
        (
            "bad.scene.toml",
            "[scene]\nname = \"Bad\"\n\n\
             [entities.a.ascii_style]\ncolor = \"plaid\"\ncell = 99\ntint = [1, 2]\n",
        ),
    ] {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let run = |format: &str| flint(&dir, &["validate", "bad.scene.toml", "--format", format]);

    let o = run("text");
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.starts_with("bad.scene.toml: 3 errors\n"), "{out}");
    for expected in [
        "  error[out-of-range] entity 'a' ascii_style.cell: 99 is above the maximum 32\n\
         \x20     hint: the range is 4 to 32\n",
        "  error[bad-enum-value] entity 'a' ascii_style.color: \"plaid\" is not a value of \
         `color`\n      hint: allowed values: mono, palette, raw, source, tint\n",
        "  error[bad-type] entity 'a' ascii_style.tint: a vec3 is 3 numbers, got 2\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }
    // The constraints did load and pass; the scene fails anyway
    assert!(out.contains("All constraints passed."), "{out}");

    let o = run("json");
    assert_eq!(o.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["errors"], 3);
    assert_eq!(doc["violations"].as_array().map(Vec::len), Some(0));
    let found: Vec<(&str, &str, &str, &str)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["code"].as_str().unwrap(),
                d["entity"].as_str().unwrap(),
                d["component"].as_str().unwrap(),
                d["field"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("out-of-range", "a", "ascii_style", "cell"),
            ("bad-enum-value", "a", "ascii_style", "color"),
            ("bad-type", "a", "ascii_style", "tint"),
        ]
    );
}

/// #807: one mistake in `[living]` or `[wooze]` is one diagnostic. The scene
/// pass names the field (an undeclared key is its `unknown-field`, a wrong
/// type its `bad-type`), and the type check that refuses the load made a
/// second, `toml-parse`, diagnostic of it with no field. Now the pass's is
/// the error, with the load's hint, and nothing else is said of the table.
#[test]
fn one_mistake_in_living_or_wooze_is_one_diagnostic_with_its_field() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join("living_once");
    let _ = std::fs::remove_dir_all(&dir);
    // The engine's own table schemas, so `[table.living]` is the real one
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let manifest = format!(
        "[project]\nschemas = [\"{}\"]\n",
        engine.display().to_string().replace('\\', "/")
    );
    for (path, text) in [
        ("flint.toml", manifest.as_str()),
        (
            "typo.scene.toml",
            "[scene]\nname = \"Typo\"\n\n[living]\nshadowz = \"drift\"\n",
        ),
        (
            "wrong_type.scene.toml",
            "[scene]\nname = \"Type\"\n\n[living]\nsoftness = \"soft\"\n",
        ),
        (
            "wooze.scene.toml",
            "[scene]\nname = \"Wooze\"\n\n[wooze]\ngilde = 0.5\n",
        ),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    }
    for (scene, table, code, field, said) in [
        (
            "typo.scene.toml",
            "living",
            "unknown-field",
            "shadowz",
            "error[unknown-field] 5:1 [living] shadowz: `living` has no field `shadowz`\n      \
             hint: did you mean `shadows`?; the scene does not load until the table fits\n",
        ),
        (
            "wrong_type.scene.toml",
            "living",
            "bad-type",
            "softness",
            "error[bad-type] 5:1 [living] softness:",
        ),
        (
            "wooze.scene.toml",
            "wooze",
            "unknown-field",
            "gilde",
            "error[unknown-field] 5:1 [wooze] gilde: `wooze` has no field `gilde`",
        ),
    ] {
        let o = flint(&dir, &["validate", scene]);
        let out = stdout(&o);
        assert_eq!(o.status.code(), Some(1), "{scene} refuses to load: {out}");
        assert!(out.starts_with(&format!("{scene}: 1 error\n")), "{out}");
        assert!(out.contains(said), "missing:\n{said}\nin:\n{out}");
        assert!(!out.contains("toml-parse"), "{out}");

        let o = flint(&dir, &["validate", scene, "--format", "json"]);
        let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
        let found = doc["diagnostics"].as_array().unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let d = &found[0];
        assert_eq!(
            (&d["severity"], &d["code"], &d["table"], &d["field"]),
            (&"error".into(), &code.into(), &table.into(), &field.into()),
            "{d:#?}"
        );
        // #1061: at the key that is wrong
        assert_eq!((&d["line"], &d["column"]), (&5.into(), &1.into()), "{d:#?}");
    }
}

/// #1061: a wrong TOML type in a typed table (`[camera]`, `[environment]`,
/// `[post_process]`) refuses the load, as it did, and is one `bad-type`
/// naming its field and line, from `flint validate` and `flint check` alike:
/// no `toml-parse` beside it. Each such table is named, where the parse
/// stopped at the first.
#[test]
fn a_wrong_type_in_a_typed_table_is_bad_type_by_field() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join("typed_1061");
    let _ = std::fs::remove_dir_all(&dir);
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let manifest = format!(
        "[project]\nschemas = [\"{}\"]\n",
        engine.display().to_string().replace('\\', "/")
    );
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("flint.toml"), manifest).unwrap();
    std::fs::write(
        dir.join("wide.scene.toml"),
        "[scene]\nname = \"Wide\"\n\n[camera]\nposition = [0, 2, 8]\nfov = \"wide\"\n\n\
         [environment]\ndiffuse_wrap = \"soft\"\n\n[entities.rock]\n",
    )
    .unwrap();

    let o = flint(&dir, &["validate", "wide.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "the scene does not load: {out}");
    assert!(out.starts_with("wide.scene.toml: 2 errors\n"), "{out}");
    for said in [
        "  error[bad-type] 6:1 [camera] fov: expected f32, got string\n      \
         hint: the scene does not load until the table fits\n",
        "  error[bad-type] 9:1 [environment] diffuse_wrap: expected f32, got string\n",
    ] {
        assert!(out.contains(said), "missing:\n{said}\nin:\n{out}");
    }
    assert!(!out.contains("toml-parse"), "{out}");

    for args in [
        &["validate", "wide.scene.toml", "--format", "json"][..],
        &["check", "wide.scene.toml", "--json"][..],
    ] {
        let o = flint(&dir, args);
        let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
        let found = doc["diagnostics"].as_array().unwrap();
        assert_eq!(found.len(), 2, "{args:?}: {found:#?}");
        let fov = &found[0];
        assert_eq!(
            (&fov["code"], &fov["table"], &fov["field"]),
            (&"bad-type".into(), &"camera".into(), &"fov".into()),
            "{args:?}: {fov:#?}"
        );
        assert_eq!((&fov["line"], &fov["column"]), (&6.into(), &1.into()));
        assert_eq!(found[1]["table"], "environment", "{args:?}");
    }
}

/// #804's boxes, through the binary: a model registered only in the asset
/// catalog passes; a misspelt particle effect and sequence fail, though a
/// file of the misspelling's stem is there (they are known by the `name`
/// inside); a colour in the old table form fails with the array to write;
/// a collider with no rigidbody is a warning. The schemas copy the engine's
/// field types, so the engine's files can change under the test.
#[test]
fn catalog_assets_pass_and_effect_sequence_colour_and_collider_mistakes_are_named() {
    const HASH: &str = "ab12000000000000000000000000000000000000000000000000000000000000";
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join("references_804");
    let _ = std::fs::remove_dir_all(&dir);
    let sidecar =
        format!("[asset]\nname = \"oak_table\"\ntype = \"mesh\"\nhash = \"sha256:{HASH}\"\n");
    let stored = format!(".flint/assets/ab/{HASH}.glb");
    for (path, text) in [
        ("flint.toml", MANIFEST),
        (
            "schemas/components/model.toml",
            "[component.model.fields]\nasset = { type = \"asset\", kind = [\"models\", \"specs\"] }\n",
        ),
        (
            "schemas/components/particle_effect.toml",
            "[component.particle_effect.fields]\n\
             effect = { type = \"asset\", kind = \"particles\", default = \"\" }\n",
        ),
        (
            "schemas/components/animator.toml",
            "[component.animator.fields]\n\
             sequence = { type = \"asset\", kind = \"sequences\", default = \"\" }\n",
        ),
        (
            "schemas/components/material.toml",
            "[component.material.fields]\ncolor = \"color\"\n",
        ),
        ("schemas/components/collider.toml", "[component.collider]\n"),
        ("schemas/components/rigidbody.toml", "[component.rigidbody]\n"),
        ("assets/oak_table.asset.toml", sidecar.as_str()),
        (stored.as_str(), "glTF"),
        ("particles/fire.particles.toml", "name = \"campfire\"\n"),
        ("animations/greet.sequence.toml", "name = \"wave\"\n"),
        (
            "scenes/props.scene.toml",
            "[scene]\nname = \"Props\"\n\n\
             [entities.table.model]\nasset = \"oak_table\"\n\n\
             [entities.hearth.particle_effect]\neffect = \"campfire\"\n\n\
             [entities.brazier.particle_effect]\neffect = \"fire\"\n\n\
             [entities.greeter.animator]\nsequence = \"wave\"\n\n\
             [entities.waver.animator]\nsequence = \"greet\"\n\n\
             [entities.rug.material]\ncolor = { r = 0.8, g = 0.2, b = 0.1 }\n\n\
             [entities.crate.collider]\n",
        ),
    ] {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let o = flint(&dir, &["validate", "scenes/props.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.starts_with("scenes/props.scene.toml: 3 errors, 1 warning\n"),
        "{out}"
    );
    for expected in [
        "  error[missing-asset] entity 'brazier' particle_effect.effect: particle effect \
         'fire' not found\n      hint: tried scenes/particles, particles\n",
        "  error[bad-type] entity 'rug' material.color: a color is [r, g, b] or [r, g, b, a], \
         and no reader takes a table: write [0.8, 0.2, 0.1]\n",
        "  error[missing-asset] entity 'waver' animator.sequence: animation sequence 'greet' \
         not found\n",
        "  warning[needs-component] entity 'crate' component 'collider': a collider with no \
         rigidbody collides with nothing\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }
    assert!(!out.contains("oak_table"), "the catalog holds it: {out}");

    // `flint check` says the same about the catalog and the effect
    let o = flint(&dir, &["check", "scenes/props.scene.toml"]);
    let out = stdout(&o);
    assert!(!out.contains("oak_table"), "{out}");
    assert!(out.contains("particle effect 'fire' not found"), "{out}");
}

/// The ticket's fourth box: a file that is not TOML is named, by validate
/// and by a command that loads the scene.
#[test]
fn a_toml_parse_error_names_the_file() {
    let dir = project("parse", MANIFEST);
    let scene = "scenes/not_toml.scene.toml";
    let o = flint(&dir, &["validate", scene]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.starts_with("scenes/not_toml.scene.toml: 1 error\n"),
        "{out}"
    );
    assert!(
        out.contains("error[toml-parse]: TOML parse error at line 4"),
        "{out}"
    );

    let o = flint(&dir, &["validate", scene, "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
    assert_eq!(doc["diagnostics"][0]["code"], "toml-parse");
    assert_eq!(doc["diagnostics"][0]["file"], scene);

    // The loader's own error (here through `flint query`) names it too
    let o = flint(&dir, &["query", "entities", "--scene", scene]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        err.contains("scenes/not_toml.scene.toml: TOML parse error at line 4"),
        "{err}"
    );
}
