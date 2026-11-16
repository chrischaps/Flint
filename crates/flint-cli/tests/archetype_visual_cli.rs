//! #793: a game's archetype states its placeholder box's colour in its own
//! schema file, and `flint render` draws it, with nothing in the engine
//! naming the archetype. The real binary in a small game project whose
//! `flint.toml` layers the game's `schemas/` over this checkout's engine
//! schemas (no `--schemas` flag, as a game with the engine as a subtree
//! runs): a `crate` archetype with an `[archetype.crate.visual]` draws its
//! colour, the same file saying another colour draws that one, and a
//! `barrel` with no `visual` draws the one neutral grey. The renderer side
//! (`SceneBinding::with_archetypes`) is `flint-render`'s
//! `a_games_archetype_colours_its_box_from_its_schema`; this is the host.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SIZE: u32 = 96;

fn engine_schemas() -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let dir = dir.canonicalize().unwrap();
    // Forward slashes, and no `\\?\` prefix: the manifest is TOML text
    let text = dir.display().to_string().replace('\\', "/");
    text.trim_start_matches("//?/").to_string()
}

/// A scene of one entity of `archetype` with a 2 m box at the origin.
fn scene(archetype: &str) -> String {
    format!(
        "[scene]\nname = \"Yard\"\nversion = \"1.0\"\n\n\
         [entities.thing]\narchetype = \"{archetype}\"\n\n\
         [entities.thing.transform]\nposition = [0, 0, 0]\n\n\
         [entities.thing.bounds]\nmin = [-1, 0, -1]\nmax = [1, 2, 1]\n"
    )
}

/// The game, written fresh: its manifest, `crate` (whose visual is
/// `crate_visual`, a TOML table body, or none) and `barrel` (none).
fn project(name: &str, crate_visual: Option<&str>) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("archetype_visual_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let mut crate_toml = String::from(
        "[archetype.crate]\ndescription = \"A crate the game invented\"\n\
         components = [\"transform\", \"bounds\"]\n",
    );
    if let Some(visual) = crate_visual {
        crate_toml.push_str(&format!("\n[archetype.crate.visual]\n{visual}\n"));
    }
    let files = [
        (
            "flint.toml",
            format!(
                "[project]\nid = \"yard\"\nschemas = [\"{}\", \"schemas\"]\n",
                engine_schemas()
            ),
        ),
        ("schemas/archetypes/crate.toml", crate_toml),
        (
            "schemas/archetypes/barrel.toml",
            "[archetype.barrel]\ndescription = \"A barrel with no visual\"\n\
             components = [\"transform\", \"bounds\"]\n"
                .to_string(),
        ),
        ("scenes/crate.scene.toml", scene("crate")),
        ("scenes/barrel.scene.toml", scene("barrel")),
    ];
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

/// The centre pixel of `flint render <scene>` in `dir`, or `None` with no GPU.
fn centre(dir: &Path, scene: &str) -> Option<[u8; 3]> {
    let out = format!("{scene}.png");
    let o: Output = Command::new(FLINT)
        .current_dir(dir)
        .args(["render", &format!("scenes/{scene}.scene.toml")])
        .args(["--output", &out, "--no-grid"])
        .args(["--width", &SIZE.to_string(), "--height", &SIZE.to_string()])
        .args(["--distance", "6", "--pitch", "20", "--yaw", "30"])
        .args(["--target", "0,1,0"])
        .output()
        .expect("run flint");
    let stderr = String::from_utf8_lossy(&o.stderr);
    if !o.status.success() && stderr.contains("headless render context") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return None;
    }
    assert!(
        o.status.success(),
        "flint render failed: {}{stderr}",
        String::from_utf8_lossy(&o.stdout)
    );
    let image = image::open(dir.join(&out)).unwrap().into_rgb8();
    Some(image.get_pixel(SIZE / 2, SIZE / 2).0)
}

#[test]
fn a_games_archetype_colours_its_box_from_its_own_schema_file() {
    let red = project("red", Some("color = [0.9, 0.1, 0.1]"));
    let Some([r, g, b]) = centre(&red, "crate") else {
        return;
    };
    assert!(
        r > 2 * g.max(b) && r > 64,
        "the crate is not its schema's red: {:?}",
        [r, g, b]
    );

    // The same archetype, its file saying blue: the colour is the file's
    let blue = project("blue", Some("color = [0.1, 0.2, 0.9]"));
    let [r, g, b] = centre(&blue, "crate").unwrap();
    assert!(
        b > 2 * r.max(g) && b > 64,
        "the crate is not its schema's blue: {:?}",
        [r, g, b]
    );

    // An archetype with no visual draws the one neutral grey
    let grey = centre(&red, "barrel").unwrap();
    let spread = grey.iter().max().unwrap() - grey.iter().min().unwrap();
    assert!(
        spread <= 8 && grey[0] > 32,
        "a barrel with no visual is not grey: {grey:?}"
    );
}
