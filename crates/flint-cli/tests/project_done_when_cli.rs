//! #706's own done-when (ADR 0114: a project manifest and one resolver),
//! proved with the real binary in a fixture game project: a `flint.toml`
//! naming `schemas = ["engine/schemas", "schemas"]`, a default scene and an
//! input config; the engine's real schemas under `engine/`; a scene at
//! `levels/x.scene.toml` and the same scene two folders deeper; and a file
//! of every asset kind at the project root.
//!
//! - In a project with `flint.toml`, `flint render levels/x.scene.toml` and
//!   `flint query` need no `--schemas` (and neither do validate, entity,
//!   schema, the viewer's loader or the player's).
//! - A not-found warning lists the paths tried.
//! - A scene two folders deep finds the same assets as one at `levels/`.
//! - `flint bundle` and `flint play` read the manifest.
//!
//! The other two boxes are not a program's to prove: the grep for
//! `scene_dir.parent()` is in the PR, and the table is in the book
//! (concepts/assets.md).

use flint_core::project::{AssetKind, Found, Project};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const MANIFEST: &str = "[project]\nid = \"lanterns\"\n\
    schemas = [\"engine/schemas\", \"schemas\"]\n\
    scene = \"levels/x.scene.toml\"\ninput = \"config/input.toml\"\n";

/// The scene, written at two depths. It names a model, a texture, an
/// effect and a script by the names a scene uses, an engine archetype
/// (`floor`) and the game's own (`lantern_post`, which only `schemas/`
/// knows and which draws a box).
const SCENE_BODY: &str = r#"
[camera]
position = [0, 2.5, 7]
target = [0, 0.6, 0]
fov = 50.0

[entities.sun]
[entities.sun.transform]
position = [0, 20, 0]
[entities.sun.light]
type = "directional"
direction = [0.4, 0.8, 0.3]
intensity = 2.5

[entities.crate]
[entities.crate.transform]
position = [-1.5, 0.5, 0]
[entities.crate.model]
asset = "crate"

[entities.ground]
archetype = "floor"
[entities.ground.transform]
position = [0, 0, 0]
[entities.ground.bounds]
min = [-6, -0.1, -6]
max = [6, 0, 6]
[entities.ground.material]
texture = "textures/wall.png"

[entities.post]
archetype = "lantern_post"
[entities.post.transform]
position = [1.5, 0, 0]

[entities.fire]
[entities.fire.transform]
position = [0, 0.1, 1]
[entities.fire.particle_effect]
effect = "fire"

[entities.spinner]
[entities.spinner.script]
source = "spin.rhai"
"#;

/// A scene whose every name is missing.
const MISSING_SCENE: &str = r#"[scene]
name = "Missing"
version = "1.0"

[entities.ghost]
[entities.ghost.transform]
position = [0, 0.5, 0]
[entities.ghost.model]
asset = "ghost"

[entities.ground]
archetype = "floor"
[entities.ground.bounds]
min = [-2, -0.1, -2]
max = [2, 0, 2]
[entities.ground.material]
texture = "textures/ghost.png"

[entities.fire]
[entities.fire.transform]
position = [0, 0.1, 1]
[entities.fire.particle_effect]
effect = "ghost"

[entities.spinner]
[entities.spinner.script]
source = "ghost.rhai"
"#;

/// A 1x1 white PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F,
    0x00, 0x05, 0xFE, 0x02, 0xFE, 0x0D, 0xEF, 0x46, 0xB8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// One file of every kind at the project root: the kind, the name a scene
/// would write, and the file. A kind added to `AssetKind::ALL` with no row
/// here fails `a_scene_two_folders_deep_finds_the_same_file_of_every_kind`.
const ASSETS: &[(AssetKind, &str, &str)] = &[
    (
        AssetKind::Particles,
        "fire",
        "particles/fire.particles.toml",
    ),
    (AssetKind::Models, "crate", "models/crate.glb"),
    (
        AssetKind::Textures,
        "textures/wall.png",
        "textures/wall.png",
    ),
    (AssetKind::Sprites, "logo.png", "sprites/logo.png"),
    (AssetKind::Animations, "bob", "animations/bob.anim.toml"),
    (
        AssetKind::SpriteAnimations,
        "walk",
        "animations/walk.sprite.toml",
    ),
    (
        AssetKind::Sequences,
        "intro",
        "animations/intro.sequence.toml",
    ),
    (AssetKind::Audio, "hit.ogg", "audio/hit.ogg"),
    (AssetKind::Scripts, "spin.rhai", "scripts/spin.rhai"),
    (AssetKind::Specs, "oak", "specs/oak.procgen.toml"),
    (AssetKind::Prefabs, "lamp", "prefabs/lamp.prefab.toml"),
    (AssetKind::WoozePresets, "misty", "wooze/misty.wooze.toml"),
    (AssetKind::Files, "config/input.toml", "config/input.toml"),
    (
        AssetKind::ParticleTextures,
        "spark.png",
        "particles/spark.png",
    ),
    (
        AssetKind::Scenes,
        "levels/x.scene.toml",
        "levels/x.scene.toml",
    ),
    (AssetKind::Palettes, "dusk", "palettes/dusk.palette.toml"),
    (
        AssetKind::GlyphSets,
        "terminal",
        "fonts/terminal.glyphs.toml",
    ),
    (AssetKind::Fonts, "ui", "fonts/ui.font.toml"),
    (AssetKind::FontFaces, "Mono.ttf", "fonts/Mono.ttf"),
    (AssetKind::Looks, "noir", "styles/noir.look.toml"),
    (AssetKind::Papers, "cotton", "papers/cotton.png"),
    (AssetKind::Strokes, "hb", "strokes/hb.png"),
    (AssetKind::Styles, "glow", "styles/glow.shader.toml"),
];

const X: &str = "levels/x.scene.toml";
const DEEP: &str = "levels/act1/deep/y.scene.toml";
const MISSING: &str = "levels/act1/deep/missing.scene.toml";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn write(dir: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

/// The fixture game, written fresh under the test's own folder.
fn game(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("project_done_when_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "flint.toml", MANIFEST);
    // The engine as a game holds it: its real schemas under engine/
    copy_tree(&repo().join("schemas"), &dir.join("engine/schemas"));
    // The game's own: a component and an archetype the engine has not
    write(
        &dir,
        "schemas/components/lantern.toml",
        "[component.lantern]\ndescription = \"Only the game has lanterns\"\n\n\
         [component.lantern.fields]\noil = { type = \"f32\", default = 3.0 }\n",
    );
    write(
        &dir,
        "schemas/archetypes/lantern_post.toml",
        "[archetype.lantern_post]\ndescription = \"A post with a lantern\"\n\
         components = [\"transform\", \"bounds\", \"lantern\"]\n\n\
         [archetype.lantern_post.defaults.bounds]\nmin = [-0.2, 0, -0.2]\nmax = [0.2, 2, 0.2]\n\n\
         [archetype.lantern_post.defaults.lantern]\noil = 3.0\n",
    );
    for (_, _, file) in ASSETS {
        write(&dir, file, "");
    }
    // Real files where a render reads them
    std::fs::copy(
        repo().join("examples/models/Box.glb"),
        dir.join("models/crate.glb"),
    )
    .unwrap();
    // An effect is known by the name it gives itself
    let campfire =
        std::fs::read_to_string(repo().join("examples/particles/campfire.particles.toml")).unwrap();
    assert!(campfire.contains("name = \"campfire\""));
    write(
        &dir,
        "particles/fire.particles.toml",
        campfire.replace("name = \"campfire\"", "name = \"fire\""),
    );
    for png in [
        "textures/wall.png",
        "sprites/logo.png",
        "particles/spark.png",
    ] {
        write(&dir, png, PNG);
    }
    write(
        &dir,
        "scripts/spin.rhai",
        "fn on_init() { log(\"spin: found\"); }\n",
    );
    write(&dir, "specs/oak.procgen.toml", "[meta]\nname = \"oak\"\n");
    write(
        &dir,
        "config/input.toml",
        "version = 1\ngame_id = \"lanterns\"\n",
    );
    write(
        &dir,
        "styles/glow.shader.toml",
        "version = 1\nname = \"glow\"\nstage = \"display\"\nshader = \"glow.wgsl\"\n",
    );
    write(
        &dir,
        "styles/glow.wgsl",
        "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n",
    );
    let scene =
        |title: &str| format!("[scene]\nname = \"{title}\"\nversion = \"1.0\"\n{SCENE_BODY}");
    write(&dir, X, scene("X"));
    write(&dir, DEEP, scene("X"));
    write(&dir, MISSING, MISSING_SCENE);
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
    String::from_utf8_lossy(&o.stdout).replace('\\', "/")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).replace('\\', "/")
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "flint failed: {}{}",
        stdout(o),
        stderr(o)
    );
    stdout(o)
}

const BOTH: [&str; 4] = ["--schemas", "engine/schemas", "--schemas", "schemas"];

fn with_flags(dir: &Path, args: &[&str]) -> Output {
    let all: Vec<&str> = args.iter().chain(&BOTH).copied().collect();
    flint(dir, &all)
}

/// A command's JSON with entity ids dropped and lists sorted (scenes load
/// from a hash map).
fn json(text: &str) -> serde_json::Value {
    fn settle(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(settle);
                items.sort_by_key(|item| item.to_string());
            }
            serde_json::Value::Object(map) => {
                map.remove("id");
                map.values_mut().for_each(settle);
            }
            _ => {}
        }
    }
    let mut value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"));
    settle(&mut value);
    value
}

/// `flint render` of `scene` to `out`, or `None` with no GPU.
fn render(dir: &Path, scene: &str, out: &str, more: &[&str]) -> Option<Output> {
    let mut args = vec![
        "render",
        scene,
        "--output",
        out,
        "--no-grid",
        "--width",
        "320",
        "--height",
        "180",
        "--particle-time",
        "1.0",
        "--scripts",
    ];
    args.extend_from_slice(more);
    let o = flint(dir, &args);
    if !o.status.success() && stderr(&o).contains("headless render context") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return None;
    }
    Some(o)
}

// ── Box 1: no --schemas ─────────────────────────────────────────────

#[test]
fn query_validate_entity_and_schema_need_no_schemas_flag() {
    let dir = game("no_flags");
    for scene in [X, DEEP] {
        // The game's archetype and the engine's, in one answer
        let posts = [
            "query",
            "entities where archetype == 'lantern_post'",
            "--scene",
            scene,
        ];
        let found = ok(&flint(&dir, &posts));
        assert!(found.contains("post"), "{scene}: {found}");
        assert_eq!(json(&found), json(&ok(&with_flags(&dir, &posts))));
        let oil = ["query", "entities where lantern.oil > 2", "--scene", scene];
        assert!(ok(&flint(&dir, &oil)).contains("post"), "{scene}");
        let floors = [
            "query",
            "entities where archetype == 'floor'",
            "--scene",
            scene,
        ];
        assert!(ok(&flint(&dir, &floors)).contains("ground"), "{scene}");

        let show = ["entity", "show", "post", "--scene", scene];
        let shown = ok(&flint(&dir, &show));
        assert!(shown.contains("\"oil\": 3.0"), "{scene}: {shown}");
        assert_eq!(json(&shown), json(&ok(&with_flags(&dir, &show))));

        let plain = flint(&dir, &["validate", scene]);
        let flagged = with_flags(&dir, &["validate", scene]);
        assert_eq!(stdout(&plain), stdout(&flagged), "{scene}");
        assert_eq!(plain.status.code(), flagged.status.code(), "{scene}");
        assert!(
            !stdout(&plain).contains("No schemas directories found"),
            "{scene}: {}",
            stdout(&plain)
        );
    }
    assert!(ok(&flint(&dir, &["schema", "lantern"])).contains("Only the game"));
    // An engine component: `schemas/` in the working directory alone, all
    // that discovery found from a game's root, never knew it
    ok(&flint(&dir, &["schema", "transform"]));
}

/// The epic's line (#672): render, edit, play, validate and query all load
/// the same schemas with no flag. Each command's loader, asked directly:
/// the CLI's (`SchemaRegistry::resolve`, which render, validate, query,
/// entity, schema, play and `flint-player` call), and the viewer's
/// (`flint_viewer::app::load_schemas`, `flint edit`).
#[test]
fn every_loader_names_the_same_two_directories() {
    let dir = game("loaders");
    for scene in [X, DEEP] {
        let scene = dir.join(scene);
        let none: [String; 0] = [];
        let (registry, cli) = flint_schema::SchemaRegistry::resolve(Some(&scene), &none).unwrap();
        let (viewer_registry, _, viewer) =
            flint_viewer::app::load_schemas(scene.to_str().unwrap(), &none).unwrap();
        assert_eq!(cli.source.name(), "manifest");
        assert_eq!(cli, viewer);
        let listed: Vec<String> = cli
            .dirs
            .iter()
            .map(|d| flint_core::project::shown(d.strip_prefix(&dir).expect("under the project")))
            .collect();
        assert_eq!(listed, ["engine/schemas", "schemas"]);
        for registry in [&registry, &viewer_registry] {
            assert!(registry.get_component("lantern").is_some(), "the game's");
            assert!(
                registry.get_component("transform").is_some(),
                "the engine's"
            );
            assert!(registry.get_archetype("lantern_post").is_some());
            assert!(registry.get_archetype("floor").is_some());
        }
    }
}

#[test]
fn render_needs_no_schemas_flag_and_a_deep_scene_draws_the_same_picture() {
    let dir = game("render");
    let Some(plain) = render(&dir, X, "x.png", &[]) else {
        return;
    };
    let said = format!("{}{}", ok(&plain), stderr(&plain));
    assert!(!said.contains("No schemas directories found"), "{said}");
    assert!(!said.contains("not found"), "everything was found: {said}");
    for loaded in [
        "Loaded model: crate",
        "Loaded texture: textures/wall.png",
        "Loaded particle effect: fire",
        "Loaded: spinner",
    ] {
        assert!(said.contains(loaded), "no `{loaded}` in:\n{said}");
    }
    assert!(!said.contains("is not loaded"), "{said}");
    assert!(!said.contains("Particles: 0 alive"), "{said}");
    let x = std::fs::read(dir.join("x.png")).unwrap();

    // The same picture with both flags: the manifest is those flags
    let flagged = render(&dir, X, "flags.png", &BOTH).unwrap();
    ok(&flagged);
    assert_eq!(x, std::fs::read(dir.join("flags.png")).unwrap());

    // The engine's schemas alone: the game's post is not drawn
    let engine = render(&dir, X, "engine.png", &["--schemas", "engine/schemas"]).unwrap();
    assert_ne!(
        x,
        std::fs::read(dir.join("engine.png")).unwrap(),
        "the game's schemas reached the render: {}",
        stderr(&engine)
    );

    // Two folders deep: the same model, texture, effect and script, so the
    // same pixels
    let deep = render(&dir, DEEP, "deep.png", &[]).unwrap();
    let said = format!("{}{}", ok(&deep), stderr(&deep));
    assert!(!said.contains("not found"), "{said}");
    assert_eq!(
        x,
        std::fs::read(dir.join("deep.png")).unwrap(),
        "a scene two folders deep draws what one at levels/ draws"
    );

    // And it is the assets that make the picture: without them it differs
    let bare = render(&dir, MISSING, "missing.png", &[]).unwrap();
    assert!(bare.status.success(), "{}", stderr(&bare));
    assert_ne!(x, std::fs::read(dir.join("missing.png")).unwrap());
}

// ── Box 3: a not-found warning lists the paths tried ────────────────

#[test]
fn a_render_of_missing_assets_lists_every_path_tried() {
    let dir = game("missing");
    let Some(o) = render(&dir, MISSING, "missing.png", &[]) else {
        return;
    };
    assert!(o.status.success(), "a miss is a warning: {}", stderr(&o));
    let said = stderr(&o);
    assert_eq!(stdout(&o).matches("not found").count(), 0, "stderr's alone");
    // Each of the four, from the scene's folder up to the root
    for tried in [
        // the model
        "levels/act1/deep/models/ghost.glb",
        "levels/act1/models/ghost.glb",
        "levels/models/ghost.glb",
        "models/ghost.glb",
        "models/ghost.gltf",
        // the texture
        "levels/act1/deep/textures/ghost.png",
        "levels/textures/ghost.png",
        // the effect
        "levels/act1/deep/particles/ghost.particles.toml",
        "levels/act1/particles/ghost.particles.toml",
        "levels/particles/ghost.particles.toml",
        "particles/ghost.particles.toml",
        // the script
        "levels/act1/deep/scripts/ghost.rhai",
        "levels/act1/scripts/ghost.rhai",
        "levels/scripts/ghost.rhai",
        "scripts/ghost.rhai",
    ] {
        assert!(
            said.lines().any(|line| line.trim().ends_with(tried)),
            "no line for {tried} in:\n{said}"
        );
    }
    for what in ["model", "texture", "particle effect", "script"] {
        assert!(
            said.lines()
                .any(|line| line.contains(what) && line.contains("not found")),
            "no `{what} ... not found` in:\n{said}"
        );
    }
}

/// `flint asset where`, for every kind there is: a miss exits non-zero,
/// and both the text and the JSON list what was tried, from the scene's
/// folder to the root.
#[test]
fn asset_where_lists_the_paths_tried_for_every_kind() {
    let dir = game("where_missing");
    for &kind in AssetKind::ALL {
        let label = kind.info().label;
        let args = ["asset", "where", label, "no_such_thing", "--scene", DEEP];
        let o = flint(&dir, &args);
        assert!(!o.status.success(), "{label}");
        let text = stdout(&o);
        assert!(text.contains("loads    nothing"), "{label}: {text}");
        let tried: Vec<&str> = text
            .lines()
            .skip_while(|line| !line.contains("Tried:"))
            .skip(1)
            .map(str::trim)
            .collect();
        assert!(!tried.is_empty(), "{label}: nothing tried\n{text}");
        assert!(tried[0].starts_with("levels/act1/deep"), "{label}: {text}");
        assert!(
            tried.iter().any(|t| !t.starts_with("levels/")),
            "{label}: the root was not tried\n{text}"
        );
        assert!(stderr(&o).contains("not found"), "{label}: {}", stderr(&o));

        let o = flint(&dir, &[&args[..], &["--json"]].concat());
        assert!(!o.status.success(), "{label}");
        let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
        assert!(doc["found"].is_null(), "{label}");
        assert_eq!(doc["tried"].as_array().unwrap().len(), tried.len());
    }
}

// ── Box 4: two folders deep, the same assets ────────────────────────

#[test]
fn a_scene_two_folders_deep_finds_the_same_file_of_every_kind() {
    let dir = game("depth");
    for &kind in AssetKind::ALL {
        let label = kind.info().label;
        let &(_, name, file) = ASSETS
            .iter()
            .find(|(k, _, _)| *k == kind)
            .unwrap_or_else(|| panic!("no fixture file for the kind `{label}`: add a row"));
        let found = |scene: &str| -> String {
            let o = flint(
                &dir,
                &["asset", "where", label, name, "--scene", scene, "--json"],
            );
            assert!(
                o.status.success(),
                "{label} {name} from {scene}: {}",
                stdout(&o)
            );
            let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
            assert_eq!(doc["project"]["found"], "flint.toml", "{label}");
            doc["found"]
                .as_str()
                .unwrap_or_else(|| panic!("{label}: {doc}"))
                .to_string()
        };
        let shallow = found(X);
        assert_eq!(shallow, file, "{label} {name} from levels/");
        assert_eq!(found(DEEP), shallow, "{label} {name} from two folders deep");
    }
}

// ── The manifest's other keys: bundle and play ──────────────────────

#[test]
fn bundle_takes_its_name_and_scene_from_the_manifest_and_packs_it() {
    let dir = game("bundle");
    // No bundle.toml: two scenes that load are packed, so the start scene
    // can only come from the manifest
    std::fs::remove_file(dir.join(MISSING)).unwrap();
    for junk in [
        "animations/bob.anim.toml",
        "animations/walk.sprite.toml",
        "animations/intro.sequence.toml",
        "prefabs/lamp.prefab.toml",
        "wooze/misty.wooze.toml",
        "palettes/dusk.palette.toml",
        "fonts/terminal.glyphs.toml",
        "fonts/ui.font.toml",
        "fonts/Mono.ttf",
        "styles/noir.look.toml",
        "specs/oak.procgen.toml",
    ] {
        std::fs::remove_file(dir.join(junk)).unwrap();
    }
    let o = flint(&dir, &["bundle", ".", "--out-dir", "dist"]);
    let said = ok(&o);
    assert!(
        said.contains("lanterns: starts levels/x.scene.toml; schemas engine/schemas, schemas"),
        "{said}"
    );
    let pack = dir.join("dist/lanterns.flintpack");
    assert!(pack.is_file(), "named for [project] id: {said}");

    // The bundle, as the player opens one: every read from its files
    let bytes = std::fs::read(&pack).unwrap();
    let bundle = flint_core::bundle::Bundle::parse(bytes).unwrap();
    assert_eq!(bundle.manifest.name, "lanterns");
    assert_eq!(bundle.manifest.scene.as_deref(), Some(X));
    assert_eq!(bundle.manifest.schemas, ["engine/schemas", "schemas"]);
    let source = std::sync::Arc::new(bundle.to_memory_source());
    flint_core::vfs::scoped(source, || {
        // The manifest is in the bundle, and found from a scene two
        // folders deep, through the vfs like everything else
        for scene in [X, DEEP] {
            let project = Project::discover(Path::new(scene)).unwrap();
            assert_eq!(project.found, Found::Manifest, "{scene}");
            assert_eq!(project.id.as_deref(), Some("lanterns"));
            assert_eq!(project.scene.as_deref(), Some(Path::new(X)));
            assert_eq!(
                project.schemas,
                [PathBuf::from("engine/schemas"), PathBuf::from("schemas")]
            );
            // Its schemas, by the one resolver, with no flag and no host list
            let none: [&str; 0] = [];
            let (registry, dirs) =
                flint_schema::SchemaRegistry::resolve(Some(Path::new(scene)), &none).unwrap();
            assert_eq!(dirs.source.name(), "manifest");
            assert!(registry.get_archetype("lantern_post").is_some());
            assert!(registry.get_archetype("floor").is_some());
            // And its assets
            let scene_dir = Path::new(scene).parent().unwrap();
            let model = project.resolve(AssetKind::Models, "crate", scene_dir);
            assert_eq!(model.as_deref(), Ok(Path::new("models/crate.glb")));
            assert_eq!(project.default_scene().as_deref(), Ok(Path::new(X)));
        }
    });

    // bundle.toml still wins where it speaks, and --name over both
    write(
        &dir,
        "bundle.toml",
        "name = \"lamps\"\nscene = \"levels/act1/deep/y.scene.toml\"\n",
    );
    let said = ok(&flint(&dir, &["bundle", ".", "--dry-run"]));
    assert!(
        said.contains("lamps: starts levels/act1/deep/y.scene.toml"),
        "{said}"
    );
    let said = ok(&flint(
        &dir,
        &["bundle", ".", "--dry-run", "--name", "flag"],
    ));
    assert!(said.contains("flag: starts"), "{said}");
}

#[test]
fn play_with_no_scene_opens_the_manifests_or_says_what_to_pass() {
    // No default scene: the error says what to pass and where to name one,
    // before any window
    let dir = game("play_no_default");
    write(&dir, "flint.toml", "[project]\nid = \"lanterns\"\n");
    let o = flint(&dir, &["play"]);
    assert!(!o.status.success());
    let said = stderr(&o);
    assert!(said.contains("no scene given"), "{said}");
    assert!(said.contains("pass one"), "{said}");
    assert!(said.contains("scene = \""), "{said}");
    assert!(said.contains("[project]"), "{said}");
    assert!(said.contains("flint.toml"), "{said}");

    // A default that isn't there is named
    write(
        &dir,
        "flint.toml",
        "[project]\nscene = \"levels/gone.scene.toml\"\n",
    );
    let said = stderr(&flint(&dir, &["play"]));
    assert!(said.contains("levels/gone.scene.toml"), "{said}");
    assert!(said.contains("`[project] scene`"), "{said}");

    // No project at all
    let empty = dir.join("empty_place");
    std::fs::create_dir_all(&empty).unwrap();
    let said = stderr(&flint(&empty, &["--project", ".", "play"]));
    assert!(said.contains("no scene given"), "{said}");
    assert!(said.contains("no such file yet"), "{said}");

    // The default itself, from the root and from a folder inside the
    // project: the same file, spelled from where the command runs
    let dir = game("play_default");
    let project = Project::discover(&dir).unwrap();
    assert_eq!(project.default_scene().unwrap(), dir.join(X));
    let from_levels = Project::discover(&dir.join("levels/act1")).unwrap();
    assert_eq!(from_levels.default_scene().unwrap(), dir.join(X));
}
