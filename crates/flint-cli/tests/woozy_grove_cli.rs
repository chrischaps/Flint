//! Woozy Grove (#217) is the place every Living Props ticket after it gets judged, so
//! it has to keep loading. These run the real binary on the scene as committed, with no
//! GPU: the frame rate and the look still need someone to walk it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scene_path() -> PathBuf {
    repo_root().join("examples/woozy_grove.scene.toml")
}

fn scene() -> toml::Value {
    let text = std::fs::read_to_string(scene_path()).expect("read the grove");
    toml::from_str(&text).expect("the grove parses")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The entities that have `component`, by name.
fn with_component(scene: &toml::Value, component: &str) -> Vec<(String, toml::Value)> {
    scene["entities"]
        .as_table()
        .expect("entities")
        .iter()
        .filter(|(_, e)| e.get(component).is_some())
        .map(|(name, e)| (name.clone(), e.clone()))
        .collect()
}

#[test]
fn the_grove_passes_flint_validate() {
    let out = Command::new(FLINT)
        .arg("validate")
        .arg(scene_path())
        .arg("--schemas")
        .arg(repo_root().join("schemas"))
        // the grove's ground is a `furniture` box: the examples' layer (#681)
        .arg("--schemas")
        .arg(repo_root().join("schemas/examples"))
        .output()
        .expect("run flint");
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn the_grove_is_a_few_dozen_trees_of_three_or_four_species() {
    let scene = scene();
    let trees = with_component(&scene, "living_prop");
    assert!(
        (20..=40).contains(&trees.len()),
        "{} living trees",
        trees.len()
    );
    let species: BTreeSet<String> = trees
        .iter()
        .map(|(_, tree)| tree["model"]["asset"].as_str().unwrap().to_string())
        .collect();
    assert!(
        (3..=4).contains(&species.len()),
        "{} species: {species:?}",
        species.len()
    );
}

/// `[[lod]]` in a living spec draws every level stacked once it's baked (#193), and a
/// spec that can't separate structure from shape won't live, so check what the tool says.
#[test]
fn every_species_is_a_living_spec_with_no_lods() {
    let scene = scene();
    let species: BTreeSet<String> = with_component(&scene, "living_prop")
        .iter()
        .map(|(_, tree)| tree["model"]["asset"].as_str().unwrap().to_string())
        .collect();
    for name in species {
        let path = repo_root()
            .join("examples/specs")
            .join(format!("{name}.procgen.toml"));
        let spec: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).expect("read the spec")).unwrap();
        assert!(spec.get("lod").is_none(), "{name} has [[lod]]");

        let out = Command::new(FLINT)
            .arg("gen")
            .arg(&path)
            .arg("--dry-run")
            .output()
            .expect("run flint");
        assert!(out.status.success(), "{name}: {}", text(&out));
        assert!(
            text(&out).contains("Living:"),
            "{name} isn't living: {}",
            text(&out)
        );
    }
}

#[test]
fn a_bench_and_a_lantern_share_shallow_wooze() {
    let scene = scene();
    let groups = with_component(&scene, "shallow_wooze");
    assert_eq!(groups.len(), 1, "one group, so one clock");
    let group = &groups[0].0;
    for prop in ["bench", "lantern"] {
        assert_eq!(
            scene["entities"][prop]["parent"].as_str(),
            Some(group.as_str()),
            "{prop} isn't under {group}"
        );
    }
}

#[test]
fn the_grove_has_two_or_three_named_zones_and_takes_the_preset() {
    let scene = scene();
    assert_eq!(scene["wooze"]["preset"].as_str(), Some("linklater"));
    let zones = scene["wooze"]["zone"].as_array().expect("zones");
    assert!((2..=3).contains(&zones.len()), "{} zones", zones.len());
    assert!(zones.iter().all(|zone| zone.get("name").is_some()));
}

/// Nothing reads a `directional_light` table, so a scene that uses one lights itself
/// with defaults and says nothing about it. A character needs something to walk in, and
/// a floor to walk on.
#[test]
fn the_grove_is_lit_and_can_be_walked() {
    let scene = scene();
    assert!(with_component(&scene, "directional_light").is_empty());
    let lights = with_component(&scene, "light");
    assert!(lights
        .iter()
        .any(|(_, e)| e["light"]["type"].as_str() == Some("directional")));
    assert_eq!(with_component(&scene, "character_controller").len(), 1);
    assert_eq!(
        scene["entities"]["ground"]["rigidbody"]["body_type"].as_str(),
        Some("static")
    );
}

/// #677: saving the grove untouched leaves it byte for byte as it was. The
/// viewer's Ctrl+S with nothing edited writes nothing at all; this holds the two
/// writers that do run, on the scene as committed: `validate --fix`'s world diff
/// with no change, and the viewer's patch of every field the file writes, each to
/// the value the world loaded (an edit undone before saving).
#[test]
fn saving_the_untouched_grove_leaves_it_byte_identical() {
    let path = scene_path();
    let original = std::fs::read_to_string(&path).unwrap();
    let (registry, _) =
        flint_schema::SchemaRegistry::resolve(Some(&path), &[repo_root().join("schemas")]).unwrap();
    let (world, _) = flint_scene::load_scene(&path, &registry).unwrap();
    let (again, _) = flint_scene::load_scene(&path, &registry).unwrap();

    let mut doc = flint_scene::SceneDocument::from_str(&original).unwrap();
    let problems = doc.apply_world_diff(&world, &again);
    assert!(problems.is_empty(), "{problems:?}");
    same_text(&doc.to_string(), &original, "validate --fix's writer");

    let written = scene();
    let mut patched = 0;
    for (name, entity) in written["entities"].as_table().unwrap() {
        let id = world.get_id(name).unwrap();
        let components = world.get_components(id).unwrap();
        for (component, fields) in entity.as_table().unwrap() {
            let Some(fields) = fields.as_table() else {
                continue;
            };
            for field in fields.keys() {
                let value = components.get_field(component, field).unwrap();
                doc.patch_field(name, component, field, value).unwrap();
                patched += 1;
            }
        }
    }
    assert!(patched > 100, "only {patched} fields");
    same_text(&doc.to_string(), &original, "the viewer's writer");
}

/// `got` is `want` byte for byte; if not, the first line that differs.
fn same_text(got: &str, want: &str, writer: &str) {
    if got == want {
        return;
    }
    let lines = |s: &str| s.split('\n').map(str::to_string).collect::<Vec<_>>();
    let (got, want) = (lines(got), lines(want));
    let n = (0..got.len().max(want.len()))
        .find(|&i| got.get(i) != want.get(i))
        .expect("texts differ");
    panic!(
        "{writer} changed the grove at line {}:\n  was: {:?}\n  now: {:?}",
        n + 1,
        want.get(n),
        got.get(n)
    );
}
