//! A terrain made from its recipe (#1697), end to end through `flint gen` and
//! `flint render`. The frozen rolling_meadow recipe, its layer textures
//! beside it in `land/`, drawn two ways: from the PNGs `flint gen` bakes,
//! with every setting copied into the scene by hand (how a scene did it
//! before #1697), and from `spec = "land/meadow.terrain.toml"` alone. The
//! two must be the same bytes, and so must the pair with a key written on
//! the component, which wins over the recipe's. They skip themselves on a
//! machine without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// The sky, the sun and a camera looking over the meadow, which is
/// centred on the origin.
const WORLD: &str = r#"
[scene]
name = "Meadow"

[camera]
position = [0.0, 45.0, 150.0]
target = [0.0, 6.0, 20.0]
fov = 55.0

[entities.sun.light]
type = "directional"
direction = [0.4, 0.8, 0.3]
intensity = 2.0

[entities.meadow]
archetype = "terrain"
[entities.meadow.transform]
position = [-192.0, 0.0, -192.0]
"#;

/// The meadow as a scene drew it before #1697: the baked PNGs, and every
/// setting of the recipe's `[geometry]`, `[textures]` and `[grass]` copied.
const BY_HAND: &str = r#"
[entities.meadow.terrain]
heightmap = "./land/meadow_height.png"
splat_map = "./land/meadow_splat.png"
layer0_texture = "./land/grass.png"
layer1_texture = "./land/dirt.png"
layer2_texture = "./land/rock.png"
layer3_texture = "./land/sand.png"
width = 384.0
depth = 384.0
height_scale = 25.0
chunk_resolution = 64
texture_tile = 20.0
roughness = 0.85
metallic = 0.0
"grass.enabled" = true
"grass.density" = 10.0
"grass.max_distance" = 100.0
"grass.fade_start" = 70.0
"grass.blade_width" = 0.09
"grass.blade_height" = 0.45
"grass.height_variation" = 0.35
"grass.color_base" = [0.12, 0.42, 0.08]
"grass.color_tip" = [0.35, 0.72, 0.18]
"grass.color_dry" = [0.50, 0.48, 0.18]
"grass.dry_amount" = 0.2
"grass.wind_direction" = [0.8, 0.0, 0.6]
"grass.wind_speed" = 1.2
"grass.wind_strength" = 0.18
"grass.bend_radius" = 2.5
"grass.bend_strength" = 0.85
"grass.density_threshold" = 0.08
"#;

/// The meadow from its recipe: nothing but the spec.
const FROM_SPEC: &str = r#"
[entities.meadow.terrain]
spec = "land/meadow.terrain.toml"
"#;

/// The recipe's layer textures, named from its own folder as the terrain
/// editor reads them (the frozen spec names none).
const LAYERS: &str = r#"
[textures.layers.layer0]
path = "grass.png"
[textures.layers.layer1]
path = "dirt.png"
[textures.layers.layer2]
path = "rock.png"
[textures.layers.layer3]
path = "sand.png"
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project of its own: the recipe and its textures in `land/`, and the
/// maps `flint gen` bakes from it beside them.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("terrain_spec_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let land = dir.join("land");
    std::fs::create_dir_all(&land).unwrap();
    let fixtures = repo().join("tests/fixtures/scenes/terrain");
    let recipe = std::fs::read_to_string(fixtures.join("rolling_meadow.terrain.toml")).unwrap();
    std::fs::write(
        land.join("meadow.terrain.toml"),
        format!("{recipe}\n{LAYERS}"),
    )
    .unwrap();
    for texture in ["grass.png", "dirt.png", "rock.png", "sand.png"] {
        std::fs::copy(fixtures.join(texture), land.join(texture)).unwrap();
    }
    let gen = std::process::Command::new(FLINT)
        .arg("gen")
        .arg(land.join("meadow.terrain.toml"))
        .arg("-o")
        .arg(land.join("meadow.png"))
        .output()
        .expect("run flint gen");
    ok(&gen);
    dir
}

fn render(dir: &Path, name: &str, terrain: &str) -> (Output, Vec<u8>) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{WORLD}\n{terrain}")).unwrap();
    let out = dir.join(format!("{name}.png"));
    let run = std::process::Command::new(FLINT)
        .arg("render")
        .arg(&path)
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .args(["--width", &WIDTH.to_string()])
        .args(["--height", &HEIGHT.to_string()])
        .arg("--no-grid")
        .arg("-o")
        .arg(&out)
        // The disk tier in the project's own folder
        .env("FLINT_PROCGEN_CACHE", dir.join("cache"))
        .output()
        .expect("run flint render");
    let bytes = std::fs::read(&out).unwrap_or_default();
    (run, bytes)
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success() && stderr(o).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(o.status.success(), "flint failed: {}", stderr(o));
}

/// The cached bakes in the project's disk tier.
fn cached(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("cache"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "terrain-maps"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_terrain_from_its_recipe_draws_the_pixels_of_its_baked_maps() {
    let dir = project("same_pixels");
    let (by_hand, hand_png) = render(&dir, "by_hand", BY_HAND);
    if no_gpu(&by_hand) {
        return;
    }
    ok(&by_hand);
    assert_eq!(cached(&dir), 0, "a terrain from its maps bakes nothing");

    let (from_spec, spec_png) = render(&dir, "from_spec", FROM_SPEC);
    ok(&from_spec);
    assert!(!hand_png.is_empty());
    assert!(
        hand_png == spec_png,
        "the meadow from its recipe isn't the meadow from its baked maps"
    );
    assert_eq!(cached(&dir), 1, "the bake went into the disk tier");

    // Again: the disk tier's bake, the same bytes
    let (again, again_png) = render(&dir, "again", FROM_SPEC);
    ok(&again);
    assert!(again_png == spec_png, "the cached bake drew another meadow");
    assert_eq!(cached(&dir), 1);
}

/// `flint check --json` of a scene with `terrain`: the codes and fields it
/// reports, sorted.
fn check(dir: &Path, name: &str, terrain: &str) -> (bool, Vec<(String, String, String)>) {
    let path = dir.join(format!("{name}.scene.toml"));
    std::fs::write(&path, format!("{WORLD}\n{terrain}")).unwrap();
    let run = std::process::Command::new(FLINT)
        .arg("--json")
        .arg("check")
        .arg(&path)
        .arg("--schemas")
        .arg(repo().join("schemas"))
        .output()
        .expect("run flint check");
    let report: serde_json::Value = serde_json::from_slice(&run.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&run.stdout)));
    let mut found: Vec<(String, String, String)> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let text = |k: &str| d[k].as_str().unwrap_or("").to_string();
            (text("code"), text("field"), text("severity"))
        })
        .collect();
    found.sort();
    (run.status.success(), found)
}

#[test]
fn flint_check_holds_a_terrain_to_its_recipe() {
    let dir = project("check");
    let triple = |code: &str, field: &str, severity: &str| {
        (code.to_string(), field.to_string(), severity.to_string())
    };

    // The recipe alone, and a key that says what the recipe says: clean
    let (passed, found) = check(&dir, "plain", &format!("{FROM_SPEC}width = 384\n"));
    assert!(passed, "{found:?}");
    assert_eq!(found, []);

    // Two keys that disagree with it, and a heightmap standing in for the bake
    let (passed, found) = check(
        &dir,
        "drifted",
        &format!(
            "{FROM_SPEC}width = 400.0\nlayer0_texture = \"./land/grass.png\"\n\
             \"grass.density\" = 30.0\nheightmap = \"./land/meadow_height.png\"\n"
        ),
    );
    assert!(passed, "warnings don't fail the check: {found:?}");
    assert_eq!(
        found,
        [
            triple("overrides-spec", "grass.density", "warning"),
            triple("overrides-spec", "heightmap", "warning"),
            triple("overrides-spec", "width", "warning"),
        ],
        "the layer texture names the recipe's own file, so it agrees"
    );

    // A recipe with a typo is warned on its file; one that doesn't parse fails
    let land = dir.join("land");
    let text = std::fs::read_to_string(land.join("meadow.terrain.toml")).unwrap();
    std::fs::write(
        land.join("typo.terrain.toml"),
        text.replace("height_scale = 25.0", "height_scael = 25.0"),
    )
    .unwrap();
    let (passed, found) = check(
        &dir,
        "typo",
        "[entities.meadow.terrain]\nspec = \"land/typo.terrain.toml\"\n",
    );
    assert!(passed, "{found:?}");
    assert_eq!(
        found,
        [triple(
            "bad-terrain-spec",
            "geometry.height_scael",
            "warning"
        )]
    );

    std::fs::write(land.join("broken.terrain.toml"), "[meta\nname = 1").unwrap();
    let (passed, found) = check(
        &dir,
        "broken",
        "[entities.meadow.terrain]\nspec = \"land/broken.terrain.toml\"\n",
    );
    assert!(!passed);
    assert_eq!(found, [triple("bad-terrain-spec", "spec", "error")]);

    // A recipe that isn't there is the asset pass's
    let (passed, found) = check(
        &dir,
        "missing",
        "[entities.meadow.terrain]\nspec = \"land/nowhere.terrain.toml\"\n",
    );
    assert!(!passed);
    assert_eq!(found, [triple("missing-asset", "spec", "error")]);
}

#[test]
fn a_key_on_the_component_wins_over_the_recipes() {
    let dir = project("override");
    let flatter = "height_scale = 8.0\ntexture_tile = 6.0\n\"grass.enabled\" = false\n";
    let by_hand = BY_HAND
        .replace("height_scale = 25.0\n", "")
        .replace("texture_tile = 20.0\n", "")
        .replace("\"grass.enabled\" = true\n", "");
    let (hand, hand_png) = render(&dir, "by_hand", &format!("{by_hand}{flatter}"));
    if no_gpu(&hand) {
        return;
    }
    ok(&hand);
    let (spec, spec_png) = render(&dir, "from_spec", &format!("{FROM_SPEC}{flatter}"));
    ok(&spec);
    assert!(
        hand_png == spec_png,
        "the component's keys didn't win over the recipe's as written ones do"
    );
    let (plain, plain_png) = render(&dir, "plain", FROM_SPEC);
    ok(&plain);
    assert!(plain_png != spec_png, "the overrides changed nothing");
}
