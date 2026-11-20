//! `flint asset resolve` (#968): the real binary. What it reads is every
//! field the loaded schemas type `asset`, not a list in the CLI:
//!
//! - a scene with one value in each `asset` field of the engine's schemas
//!   (components and scene tables) reports each of them, and only them;
//! - a small harbour, with references in several components, a prefab's
//!   entity and a scene table, finds each where the engine would load it
//!   (the file, by the project's search) or says it is missing and why;
//!   `strict` fails on a missing one, `placeholder` does not;
//! - `human_task` makes a task for each missing model, texture or sound and
//!   names the ones no provider makes (a heightmap's layer, a skybox).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flint_schema::{FieldType, SchemaRegistry};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn engine_schemas() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .canonicalize()
        .expect("the engine's schemas")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("asset_resolve_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    let schemas = engine_schemas();
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .arg("--schemas")
        .arg(&schemas)
        .output()
        .expect("run flint")
}

fn json(o: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&o.stdout);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{e}: {text}\nstderr: {}",
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

/// A value for a field of `field_type` naming `name` in each `asset` inside
/// it, and the paths of those, from `path`: `None` when it holds no asset.
fn sample(field_type: &FieldType, path: &str, name: &str) -> Option<(toml::Value, Vec<String>)> {
    match field_type {
        FieldType::Asset { kinds, .. } if !kinds.is_empty() => Some((
            toml::Value::String(name.to_string()),
            vec![path.to_string()],
        )),
        FieldType::Array { element } => {
            let (value, paths) = sample(element, &format!("{path}[0]"), name)?;
            Some((toml::Value::Array(vec![value]), paths))
        }
        FieldType::Fields { fields } => {
            let mut table = toml::Table::new();
            let mut paths = Vec::new();
            for (field, schema) in fields {
                let inner = format!("{path}.{field}");
                if let Some((value, found)) =
                    sample(&schema.field_type, &inner, &format!("{name}-{field}"))
                {
                    table.insert(field.clone(), value);
                    paths.extend(found);
                }
            }
            (!paths.is_empty()).then_some((toml::Value::Table(table), paths))
        }
        _ => None,
    }
}

/// Each component's and table's asset fields, filled in: the scene's tables
/// and the `component.path` list `resolve` should report.
fn every_asset_field(registry: &SchemaRegistry) -> (toml::Table, BTreeSet<String>) {
    let mut entities = toml::Table::new();
    let mut tables = toml::Table::new();
    let mut expected = BTreeSet::new();
    let mut fill = |owner: &str, schema: &flint_schema::ComponentSchema| {
        let mut data = toml::Table::new();
        for (field, f) in &schema.fields {
            if let Some((value, paths)) = sample(&f.field_type, field, &format!("{owner}-{field}"))
            {
                data.insert(field.clone(), value);
                expected.extend(paths.into_iter().map(|p| format!("{owner}.{p}")));
            }
        }
        (!data.is_empty()).then_some(data)
    };
    for component in registry.component_names() {
        let schema = registry.get_component(component).unwrap();
        if let Some(data) = fill(component, schema) {
            let mut entity = toml::Table::new();
            entity.insert(component.to_string(), toml::Value::Table(data));
            entities.insert(format!("e_{component}"), toml::Value::Table(entity));
        }
    }
    for table in registry.table_names() {
        let schema = registry.get_table(table).unwrap();
        if let Some(data) = fill(table, schema) {
            tables.insert(table.to_string(), toml::Value::Table(data));
        }
    }
    let mut scene = toml::Table::new();
    scene.insert(
        "scene".into(),
        toml::Value::Table(toml::toml! { name = "every" version = "1.0" }),
    );
    scene.extend(tables);
    scene.insert("entities".into(), toml::Value::Table(entities));
    (scene, expected)
}

#[test]
fn every_asset_field_the_engine_declares_is_a_reference_resolve_reports() {
    let registry = SchemaRegistry::load_from_directories(&[engine_schemas()]).unwrap();
    let (scene, expected) = every_asset_field(&registry);
    // Today's engine schemas: well past the four fields resolve once read
    assert!(expected.len() >= 19, "{expected:?}");
    for field in [
        "terrain.heightmap",
        "particle_emitter.texture",
        "spline_mesh.texture",
        "script.source",
        "audio_source.file",
        "environment.skybox",
    ] {
        assert!(expected.contains(field), "{field} in {expected:?}");
    }

    let dir = dir("every");
    write(
        &dir,
        &[
            ("flint.toml", "[project]\nid = \"every\"\n"),
            ("every.scene.toml", &toml::to_string(&scene).unwrap()),
        ],
    );
    let out = flint(
        &dir,
        &[
            "--json",
            "asset",
            "resolve",
            "every.scene.toml",
            "--strategy",
            "placeholder",
        ],
    );
    assert!(out.status.success(), "placeholder never fails: {out:?}");
    let report = json(&out);
    let reported: BTreeSet<String> = report["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["field"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(reported, expected);
    assert_eq!(report["found"], 0, "no file is there");
    assert_eq!(report["missing"].as_array().unwrap().len(), expected.len());
}

/// A small harbour: a model, a script and a sound that are there, a terrain
/// with one layer missing, an emitter's texture, a crane an `asset_def`
/// declares, a prefab's lamp, and a skybox in `[environment]`.
const HARBOUR: &[(&str, &str)] = &[
    ("flint.toml", "[project]\nid = \"harbour\"\n"),
    ("models/boat.glb", "glTF"),
    ("scripts/lantern.rhai", "fn on_init() {}\n"),
    ("audio/gulls.ogg", "OggS"),
    ("terrain/shore.png", "png"),
    (
        "prefabs/lamp.prefab.toml",
        "[prefab]\nname = \"lamp\"\n\n[entities.post]\nmodel = { asset = \"lamp_post\" }\n\
         script = { source = \"lantern.rhai\" }\n",
    ),
    (
        "harbour.scene.toml",
        "[scene]\nname = \"Harbour\"\nversion = \"1.0\"\n\n\
         [environment]\nskybox = \"sky/dusk.hdr\"\n\n\
         [prefabs.west]\ntemplate = \"lamp\"\nprefix = \"west\"\n\n\
         [entities.boat]\nmodel = { asset = \"boat\" }\n\n\
         [entities.gulls]\naudio_source = { file = \"audio/gulls.ogg\" }\n\n\
         [entities.shore]\nterrain = { heightmap = \"terrain/shore.png\", layer0_texture = \"terrain/sand.png\" }\n\n\
         [entities.spray]\nparticle_emitter = { texture = \"spray.png\" }\n\n\
         [entities.crane]\nmodel = { asset = \"crane\" }\n\
         asset_def = { name = \"crane\", description = \"A harbour crane\", type = \"model\" }\n",
    ),
];

fn harbour(name: &str) -> PathBuf {
    let dir = dir(name);
    write(&dir, HARBOUR);
    dir
}

#[test]
fn nested_references_are_found_where_the_engine_loads_them() {
    let dir = harbour("nested");
    let out = flint(&dir, &["--json", "asset", "resolve", "harbour.scene.toml"]);
    assert_eq!(out.status.code(), Some(1), "strict fails on a missing one");
    let report = json(&out);
    assert_eq!(report["valid"], false);

    // Entities in name order (the prefab's as <prefix>_<name>), then tables
    let rows: Vec<(String, String, String, Option<String>)> = report["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["entity"].as_str().unwrap_or("-").to_string(),
                r["field"].as_str().unwrap().to_string(),
                r["name"].as_str().unwrap().to_string(),
                r["found"].as_str().map(|p| p.replace('\\', "/")),
            )
        })
        .collect();
    let row = |e: &str, f: &str, n: &str, found: Option<&str>| {
        (
            e.to_string(),
            f.to_string(),
            n.to_string(),
            found.map(str::to_string),
        )
    };
    let ends =
        |found: &Option<String>, tail: &str| found.as_deref().is_some_and(|p| p.ends_with(tail));
    assert_eq!(rows.len(), 9, "{rows:#?}");
    assert!(ends(&rows[0].3, "models/boat.glb"), "{rows:#?}");
    assert_eq!(rows[0].1, "model.asset");
    assert_eq!(rows[1], row("crane", "model.asset", "crane", None));
    assert!(ends(&rows[2].3, "audio/gulls.ogg"), "{rows:#?}");
    assert!(ends(&rows[3].3, "terrain/shore.png"), "{rows:#?}");
    assert_eq!(rows[3].1, "terrain.heightmap");
    assert_eq!(
        rows[4],
        row("shore", "terrain.layer0_texture", "terrain/sand.png", None)
    );
    assert_eq!(
        rows[5],
        row("spray", "particle_emitter.texture", "spray.png", None)
    );
    assert_eq!(rows[6], row("west_post", "model.asset", "lamp_post", None));
    assert!(ends(&rows[7].3, "scripts/lantern.rhai"), "{rows:#?}");
    assert_eq!(rows[7].0, "west_post");
    assert_eq!(
        rows[8],
        row("-", "environment.skybox", "sky/dusk.hdr", None)
    );

    assert_eq!(report["found"], 4);
    let missing = report["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 5);
    let crane = &missing[0];
    assert_eq!(crane["entity"], "crane");
    assert!(
        crane["missing"].as_str().unwrap().contains("asset_def"),
        "a declared asset not made yet says so: {crane}"
    );
    let lamp = missing.iter().find(|m| m["name"] == "lamp_post").unwrap();
    assert_eq!(lamp["kinds"], serde_json::json!(["models", "specs"]));
    assert!(
        lamp["missing"].as_str().unwrap().contains("lamp_post.glb"),
        "a miss lists the paths tried: {lamp}"
    );

    // The text says the same, one line a reference
    let text = flint(&dir, &["asset", "resolve", "harbour.scene.toml"]);
    let text = String::from_utf8_lossy(&text.stdout).replace('\\', "/");
    assert!(
        text.contains("Found: gulls.audio_source.file -> \"audio/gulls.ogg\""),
        "{text}"
    );
    assert!(
        text.contains("Missing: [environment].skybox -> \"sky/dusk.hdr\""),
        "{text}"
    );
    assert!(text.contains("Resolution: 4 found, 5 missing"), "{text}");

    // Placeholder reports the same and passes
    let placeholder = flint(
        &dir,
        &[
            "--json",
            "asset",
            "resolve",
            "harbour.scene.toml",
            "--strategy",
            "placeholder",
        ],
    );
    assert!(placeholder.status.success(), "{placeholder:?}");
    assert_eq!(json(&placeholder)["references"], report["references"]);
}

#[test]
fn a_human_task_is_made_for_what_a_provider_makes_and_the_rest_is_named() {
    let dir = harbour("tasks");
    let out = flint(
        &dir,
        &[
            "--json",
            "asset",
            "resolve",
            "harbour.scene.toml",
            "--strategy",
            "human_task",
            "--output-dir",
            "tasks",
        ],
    );
    let report = json(&out);
    // The crane and the lamp post (models) and the spray (a texture) are
    // tasks; a terrain layer and a skybox are files no provider makes
    assert_eq!(report["references"], 9, "{report}");
    assert_eq!(report["found"], 4, "{report}");
    assert_eq!(report["tasks_created"], 3, "{report}");
    for task in ["crane", "lamp_post", "spray.png"] {
        assert!(
            dir.join("tasks")
                .join(format!("{task}.task.toml"))
                .is_file(),
            "{task}: {report}"
        );
    }
    let no_provider: Vec<&str> = report["no_provider"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["field"].as_str().unwrap())
        .collect();
    assert_eq!(
        no_provider,
        ["terrain.layer0_texture", "environment.skybox"]
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a reference left unresolved fails the run"
    );
}

/// With no schema, nothing is known to name an asset: the run says so and
/// fails, rather than reporting a scene with nothing in it to make.
#[test]
fn with_no_schemas_resolve_refuses_rather_than_finding_nothing() {
    let dir = dir("no_schemas");
    write(
        &dir,
        &[("flint.toml", "[project]\nid = \"bare\"\n"), HARBOUR[6]],
    );
    let out = Command::new(FLINT)
        .current_dir(&dir)
        .args(["--json", "asset", "resolve", "harbour.scene.toml"])
        .output()
        .expect("run flint");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let message = json(&out)["error"]["message"].to_string();
    assert!(message.contains("--schemas"), "{message}");
}
