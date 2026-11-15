//! `flint export web` (#106) through the real binary: a game in an empty
//! folder exports with the engine's schemas, a second game joins the same
//! site without disturbing the first, media paths are copied beside the
//! bundle, and a folder that isn't an export is never written into. The
//! player is a stand-in trunk dist (an index.html and a .wasm), so no trunk
//! and no GPU.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn flint(args: &[&str]) -> Output {
    Command::new(FLINT).args(args).output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("export-web-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A game an agent might write in an afternoon: one scene, one box, no
/// engine subtree, no schemas of its own. The box is a plain `bounds`: the
/// examples' `furniture` archetype is no part of the engine's set (#681),
/// and a bundle refuses an archetype in no bundled schema.
fn game(at: &Path, scene_name: &str, bundle_toml: Option<&str>) {
    std::fs::create_dir_all(at.join("scenes")).unwrap();
    std::fs::write(
        at.join("scenes/main.scene.toml"),
        format!(
            "[scene]\nname = \"{scene_name}\"\nversion = \"1.0\"\n\n\
             [entities.crate]\n\n\
             [entities.crate.transform]\nposition = [0, 0, 0]\n\n\
             [entities.crate.bounds]\nmin = [0, 0, 0]\nmax = [1, 1, 1]\n"
        ),
    )
    .unwrap();
    std::fs::write(at.join("README.md"), "not for the web\n").unwrap();
    if let Some(config) = bundle_toml {
        std::fs::write(at.join("bundle.toml"), config).unwrap();
    }
}

fn player(at: &Path) -> String {
    std::fs::create_dir_all(at).unwrap();
    std::fs::write(
        at.join("index.html"),
        "<!doctype html><title>player</title>\n",
    )
    .unwrap();
    std::fs::write(at.join("flint_web-abc_bg.wasm"), b"\0asm\x01\0\0\0").unwrap();
    at.to_string_lossy().into_owned()
}

fn listing(site: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(site.join("games.json")).unwrap()).unwrap()
}

#[test]
fn games_export_into_one_site() {
    let dir = scratch("site");
    let dist = player(&dir.join("dist"));
    let site = dir.join("site");
    let site_arg = site.to_string_lossy().into_owned();

    // One game, from a bare folder: the folder's name is its id and its
    // scene's name its title.
    let first = dir.join("Lantern Walk");
    game(&first, "A Lantern Walk", None);
    let out = flint(&[
        "export",
        "web",
        first.to_str().unwrap(),
        "--out",
        &site_arg,
        "--player",
        &dist,
    ]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("packing the engine's"),
        "{}",
        text(&out)
    );
    assert!(site.join("index.html").is_file());
    assert!(site.join("flint_web-abc_bg.wasm").is_file());
    let pack = site.join("games/lantern-walk/lantern-walk.flintpack");
    assert!(pack.is_file(), "{}", text(&out));
    let bytes = std::fs::read(&pack).unwrap();
    let bundle = flint_core::bundle::Bundle::parse(bytes).unwrap();
    let names: Vec<&str> = bundle.files().map(|(p, _)| p).collect();
    assert!(names.contains(&"scenes/main.scene.toml"), "{names:?}");
    assert!(
        names
            .iter()
            .any(|p| p.starts_with("engine/schemas/archetypes/")),
        "the engine's schemas are packed: {names:?}"
    );
    assert!(!names.contains(&"README.md"));
    assert_eq!(
        listing(&site),
        serde_json::json!({ "games": [{ "id": "lantern-walk", "title": "A Lantern Walk" }] })
    );

    // A second game, with its words in bundle.toml and a poster in the repo.
    let second = dir.join("raft");
    game(
        &second,
        "raft",
        Some("name = \"raft\"\n[web]\ntitle = \"Raft\"\nline = \"At sea.\"\nposter = \"web/poster.jpg\"\n"),
    );
    std::fs::create_dir_all(second.join("web")).unwrap();
    std::fs::write(second.join("web/poster.jpg"), b"jpeg").unwrap();
    let out = flint(&[
        "export",
        "web",
        second.to_str().unwrap(),
        "--out",
        &site_arg,
        "--player",
        &dist,
        "--meta",
        "line=Out at sea.",
        "--meta",
        "orientation=landscape",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(site.join("games/raft/poster.jpg").is_file());
    let games = listing(&site);
    assert_eq!(games["games"].as_array().unwrap().len(), 2);
    assert_eq!(
        games["games"][0]["id"], "lantern-walk",
        "the first game stays first"
    );
    assert_eq!(
        games["games"][1],
        serde_json::json!({
            "id": "raft",
            "title": "Raft",
            "line": "Out at sea.",
            "orientation": "landscape",
            "poster": "games/raft/poster.jpg",
        })
    );
    assert!(pack.is_file(), "the first game's bundle is untouched");

    // Exporting it again replaces it in place, and --clean leaves one game.
    let out = flint(&[
        "export",
        "web",
        second.to_str().unwrap(),
        "--out",
        &site_arg,
        "--player",
        &dist,
        "--clean",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let games = listing(&site);
    assert_eq!(games["games"].as_array().unwrap().len(), 1);
    assert_eq!(games["games"][0]["line"], "At sea.");
    assert!(!pack.exists());
    assert!(!site.read_dir().unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("staging")));
}

#[test]
fn refuses_what_it_should_not_touch() {
    let dir = scratch("refuse");
    let dist = player(&dir.join("dist"));
    let g = dir.join("game");
    game(&g, "g", None);

    // A folder with something in it and no games.json.
    let busy = dir.join("busy");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join("notes.txt"), "mine").unwrap();
    let out = flint(&[
        "export",
        "web",
        g.to_str().unwrap(),
        "--out",
        busy.to_str().unwrap(),
        "--player",
        &dist,
        "--clean",
    ]);
    assert!(!out.status.success());
    assert!(text(&out).contains("isn't an export"), "{}", text(&out));
    assert!(busy.join("notes.txt").is_file());

    // The game folder itself.
    let out = flint(&[
        "export",
        "web",
        g.to_str().unwrap(),
        "--out",
        g.to_str().unwrap(),
        "--player",
        &dist,
    ]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("holds the game itself"),
        "{}",
        text(&out)
    );

    // A player folder without a wasm, and a bad --meta key.
    let half = dir.join("half");
    std::fs::create_dir_all(&half).unwrap();
    std::fs::write(half.join("index.html"), "").unwrap();
    let out = flint(&[
        "export",
        "web",
        g.to_str().unwrap(),
        "--player",
        half.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no .wasm"), "{}", text(&out));
    let out = flint(&[
        "export",
        "web",
        g.to_str().unwrap(),
        "--player",
        &dist,
        "--meta",
        "colour=red",
    ]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no key"), "{}", text(&out));

    // A bundle with a hole writes nothing, not even the output folder.
    std::fs::write(
        g.join("scenes/main.scene.toml"),
        "[scene]\nname = \"g\"\nversion = \"1.0\"\n\n[entities.a.model]\nasset = \"missing.glb\"\n",
    )
    .unwrap();
    let out = flint(&["export", "web", g.to_str().unwrap(), "--player", &dist]);
    assert!(!out.status.success());
    assert!(text(&out).contains("missing.glb"), "{}", text(&out));
    assert!(!g.join("dist/web").exists(), "nothing written");
}
