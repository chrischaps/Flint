//! #707's done-when, proved with the real binary: `flint init` into a
//! scratch directory, then the commands it prints.
//!
//! - `flint init x`, then the commands it prints, renders a lit frame, with
//!   no edits and no `--schemas` (every template). `flint play` opens a
//!   window, so here its scene is resolved the way it resolves it and the
//!   scripts are stepped headless; the window itself is run by hand.
//! - The agent file names only paths and commands that exist (every
//!   template, every `--engine` mode).
//! - No engine schema is copied into the game.
//! - `--engine subtree` and `none` say what they don't do, and nothing is
//!   overwritten or deleted.

use flint_core::project::Project;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");
const TEMPLATES: [&str; 3] = ["walk", "blank", "2d"];
const SCENE: &str = "scenes/main.scene.toml";

/// The engine checkout: what `--engine path` names by default.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An empty directory for one test to make projects in.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("init_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .env_remove("WGPU_ADAPTER_NAME")
        .output()
        .expect("flint runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "failed:\n{}\n{}", stdout(o), stderr(o));
    stdout(o)
}

/// `flint init game <more>` in `dir`: the project's directory and what
/// the command printed.
fn init(dir: &Path, more: &[&str]) -> (PathBuf, String) {
    let mut args = vec!["init", "game"];
    args.extend_from_slice(more);
    let said = ok(&flint(dir, &args));
    (dir.join("game"), said)
}

/// A command line split as a shell would: spaces, and double quotes.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// The commands under "Next steps:" in what `flint init` printed.
fn next_steps(said: &str) -> Vec<String> {
    let after = said.split("Next steps:").nth(1).expect("a Next steps list");
    after
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .map(|l| l.trim().to_string())
        .collect()
}

/// Whether a failed render failed for want of a GPU (then the test is
/// skipped, or fails where one is required).
fn no_gpu(o: &Output) -> bool {
    if !o.status.success() && stderr(o).contains("headless render context") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return true;
    }
    false
}

/// A PNG's mean luma, and the widest spread (standard deviation) of its
/// red, green and blue, each 0..1. A frame of one colour has no spread in
/// any channel.
fn luma(png: &Path) -> (f64, f64) {
    let img = image::open(png).expect("a PNG").to_rgb8();
    let n = (img.width() * img.height()) as f64;
    let mut mean = [0.0f64; 3];
    for px in img.pixels() {
        for c in 0..3 {
            mean[c] += px[c] as f64 / n;
        }
    }
    let mut var = [0.0f64; 3];
    for px in img.pixels() {
        for c in 0..3 {
            var[c] += (px[c] as f64 - mean[c]).powi(2) / n;
        }
    }
    let luma = 0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2];
    let spread = var.iter().fold(0.0f64, |a, v| a.max(v.sqrt()));
    (luma / 255.0, spread / 255.0)
}

/// Runs a printed or documented `flint ...` line in `dir`. `play` and
/// `edit` open windows, so they only have to parse. `None`: no GPU.
fn run_line(dir: &Path, line: &str) -> Option<()> {
    let words = words(line);
    assert_eq!(words[0], "flint", "{line}");
    let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    assert!(
        !args.contains(&"--schemas"),
        "a project with a manifest needs no --schemas: {line}"
    );
    if matches!(args[0], "play" | "edit") {
        ok(&flint(dir, &[args[0], "--help"]));
        return Some(());
    }
    let o = flint(dir, &args);
    if args[0] == "render" && no_gpu(&o) {
        return None;
    }
    assert!(
        o.status.success(),
        "`{line}` failed in {}:\n{}\n{}",
        dir.display(),
        stdout(&o),
        stderr(&o)
    );
    Some(())
}

// ── The pre-flight (#687): what init writes has nothing wrong with it ──

/// Every template passes `flint check` as written: no error and no
/// warning, for the project and for its scene alone. The check reads the
/// scene, its script, its input config and the manifest.
#[test]
fn each_template_passes_flint_check() {
    for template in TEMPLATES {
        let dir = scratch(&format!("check_{template}"));
        let (game, _) = init(&dir, &["--template", template]);
        for args in [vec!["check", "--json"], vec!["check", SCENE, "--json"]] {
            let o = flint(&game, &args);
            let out = stdout(&o);
            assert!(
                o.status.success(),
                "{template} {args:?}:\n{out}{}",
                stderr(&o)
            );
            let doc: serde_json::Value = serde_json::from_str(&out).expect("one JSON document");
            assert_eq!(doc["valid"], true, "{template}: {out}");
            assert_eq!(
                doc["diagnostics"].as_array().map(Vec::len),
                Some(0),
                "{template} {args:?}: {out}"
            );
            assert_eq!(doc["scenes"].as_array().map(Vec::len), Some(1), "{out}");
            assert!(
                doc["files"].as_u64().unwrap() >= 2,
                "scene and script: {out}"
            );
        }
        // ...and it is not passing by looking at nothing: break the scene
        let scene = game.join(SCENE);
        let text = std::fs::read_to_string(&scene).unwrap();
        std::fs::write(&scene, text.replace("hud.rhai", "hdu.rhai")).unwrap();
        let o = flint(&game, &["check"]);
        assert_eq!(o.status.code(), Some(1), "{template}: {}", stdout(&o));
        assert!(
            stdout(&o).contains("error[missing-asset]"),
            "{}",
            stdout(&o)
        );
        assert!(stdout(&o).contains("scripts/hdu.rhai"), "{}", stdout(&o));
    }
}

// ── Box 1: the printed commands render a lit frame and reach play ───

#[test]
fn each_template_runs_the_commands_it_prints() {
    for template in TEMPLATES {
        let dir = scratch(&format!("printed_{template}"));
        let (game, said) = init(&dir, &["--template", template]);
        assert!(!said.contains("--schemas"), "{said}");
        assert!(!said.contains("serve"), "the legacy alias is gone: {said}");

        // The steps, as printed, from the directory they say to be in
        let steps = next_steps(&said);
        assert_eq!(steps[0], "cd game", "{said}");
        assert!(steps.contains(&"flint check".to_string()), "{said}");
        assert!(steps.contains(&"flint play".to_string()), "{said}");
        let mut rendered = true;
        for step in &steps[1..] {
            assert!(step.starts_with("flint "), "{template}: {step}");
            rendered &= run_line(&game, step).is_some();
        }

        // `flint play`, as far as a test can take it: the scene it opens
        // with no argument is the one init wrote
        let project = Project::discover(&game).unwrap();
        assert_eq!(project.id.as_deref(), Some("game"));
        assert_eq!(project.default_scene().unwrap(), game.join(SCENE));
        assert!(game.join(project.input.as_ref().unwrap()).is_file());

        // The other commands the loop leans on, with no flags
        let entities = ok(&flint(&game, &["query", "entities", "--scene", SCENE]));
        assert!(entities.contains("hud_controller"), "{entities}");
        ok(&flint(&game, &["schema", "transform"]));
        ok(&flint(&game, &["schema", "player"]));
        let where_ = ok(&flint(
            &game,
            &["asset", "where", "scripts", "hud.rhai", "--scene", SCENE],
        ));
        assert!(where_.contains("scripts/hud.rhai"), "{where_}");
        let packed = ok(&flint(&game, &["bundle", ".", "--dry-run"]));
        assert!(packed.contains("game.flintpack"), "{packed}");
        assert!(packed.contains("engine/schemas"), "{packed}");

        if !rendered {
            continue;
        }
        // The printed render wrote its file. Then a lit frame: the same
        // scene without the editor grid (whose lines alone spread a frame's
        // colours), not black and not flat. The old scaffold's room and
        // door measured a spread of 0.029 here; these measure 0.076 and up.
        assert!(game.join("renders/main.png").is_file(), "{template}");
        let plain = [
            "render",
            SCENE,
            "--output",
            "renders/plain.png",
            "--no-grid",
            "--width",
            "640",
            "--height",
            "360",
        ];
        ok(&flint(&game, &plain));
        let (mean, spread) = luma(&game.join("renders/plain.png"));
        assert!(mean > 0.08, "{template}: a dark frame, mean luma {mean:.3}");
        assert!(
            spread > 0.05,
            "{template}: a flat frame, spread {spread:.3}"
        );

        // The scripts run: half a second of on_update, with no error
        let o = flint(
            &game,
            &[
                "render",
                SCENE,
                "--output",
                "renders/stepped.png",
                "--width",
                "320",
                "--height",
                "180",
                "--scripts",
                "--time",
                "0.5",
            ],
        );
        let said = format!("{}{}", ok(&o), stderr(&o));
        assert!(said.contains("hud_controller"), "{template}: {said}");
        assert!(!said.contains(" error "), "{template}: {said}");
    }
}

/// What the scene has to hold to be "a lit, playable scene": read from the
/// file, so a template edit that drops one fails here with its name.
#[test]
fn each_scene_has_a_camera_a_typed_light_or_sprites_and_a_hud() {
    for template in TEMPLATES {
        let dir = scratch(&format!("scene_{template}"));
        let (game, _) = init(&dir, &["--template", template]);
        let scene: toml::Table = std::fs::read_to_string(game.join(SCENE))
            .unwrap()
            .parse()
            .unwrap();
        assert!(scene.contains_key("camera"), "{template}: no [camera]");
        let entities = scene["entities"].as_table().unwrap();
        let has = |entity: &toml::Value, component: &str| entity.get(component).is_some();

        let hud = &entities["hud_controller"];
        assert_eq!(hud["script"]["source"].as_str(), Some("hud.rhai"));
        assert!(game.join("scripts/hud.rhai").is_file());
        for (name, entity) in entities {
            assert_ne!(
                entity.get("archetype").and_then(|a| a.as_str()),
                Some("furniture"),
                "{template}: {name}"
            );
            if let Some(script) = entity.get("script") {
                let source = script["source"].as_str().unwrap();
                assert!(game.join("scripts").join(source).is_file(), "{source}");
            }
        }

        if template == "2d" {
            assert_eq!(scene["camera"]["projection"].as_str(), Some("orthographic"));
            assert!(entities.values().filter(|e| has(e, "sprite")).count() > 2);
            continue;
        }
        // A light is a `light` table with a `type`; a `directional_light`
        // table is read by nothing
        let lights: Vec<_> = entities.values().filter(|e| has(e, "light")).collect();
        assert!(!lights.is_empty(), "{template}: no light");
        for light in lights {
            assert!(light["light"].get("type").is_some(), "{template}");
        }
        assert!(entities.values().all(|e| !has(e, "directional_light")));
        assert!(has(&entities["ground"], "collider"), "{template}");

        if template == "walk" {
            // A character starts facing +Z: what there is to see lies there
            let player = &entities["player"];
            assert!(has(player, "character_controller"));
            let z = |e: &toml::Value| e["transform"]["position"][2].as_float().unwrap();
            let player_z = z(player);
            for (name, entity) in entities {
                if name.starts_with("stone") || name == "plinth" {
                    assert!(z(entity) > player_z, "{name} is behind the player");
                }
            }
        }
    }
}

// ── Box 2: the agent file names only what exists ────────────────────

/// A Markdown file's inline code spans (outside fences), its fenced
/// lines, and all of its text.
struct Guide {
    spans: Vec<String>,
    fenced: Vec<String>,
    text: String,
}

fn guide(file: &Path) -> Guide {
    let text = std::fs::read_to_string(file).unwrap();
    let mut spans = Vec::new();
    let mut fenced = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            fenced.push(line.trim().to_string());
            continue;
        }
        for (i, part) in line.split('`').enumerate() {
            if i % 2 == 1 {
                spans.push(part.to_string());
            }
        }
    }
    assert!(!in_fence, "{}: an unclosed fence", file.display());
    Guide {
        spans,
        fenced,
        text,
    }
}

/// Whether a code span is a path: one word, with a slash or a file
/// extension, and none of the marks of a pattern, a flag or a key.
fn is_path(span: &str) -> bool {
    let plain = !span.contains(|c: char| c.is_whitespace() || "<>*=\"()[]{},".contains(c));
    let file = [".toml", ".rhai", ".md", ".rs", ".png"]
        .iter()
        .any(|ext| span.ends_with(ext));
    plain && !span.starts_with('-') && (span.contains('/') || file)
}

/// Every `flint <subcommand>` in a text: the word after each `flint `.
fn subcommands(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (at, _) in text.match_indices("flint ") {
        let before = text[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/') {
            continue;
        }
        let word: String = text[at + 6..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .collect();
        if !word.is_empty() {
            found.insert(word);
        }
    }
    found
}

#[test]
fn the_agent_file_names_only_paths_and_commands_that_exist() {
    let repo = repo();
    let mut checked_paths = 0;
    let mut ran = 0;
    for template in TEMPLATES {
        for engine in ["path", "subtree", "none"] {
            let dir = scratch(&format!("guide_{template}_{engine}"));
            let (game, _) = init(&dir, &["--template", template, "--engine", engine]);
            for file in ["CLAUDE.md", "AGENTS.md"] {
                let guide = guide(&game.join(file));
                assert!(!guide.text.contains("{{"), "{file}: an unfilled key");
                assert!(!guide.text.contains("furniture` archetype on"), "{file}");

                // Paths: in the project, or (for `engine/...` in a subtree
                // project) in the engine that the subtree would bring
                for span in guide.spans.iter().filter(|s| is_path(s)) {
                    let path = span.trim_end_matches('/');
                    let there = if Path::new(path).is_absolute() {
                        Path::new(path).exists()
                    } else if game.join(path).exists() {
                        true
                    } else if engine == "subtree" {
                        match path.strip_prefix("engine") {
                            Some(rest) => repo.join(rest.trim_start_matches('/')).exists(),
                            None => false,
                        }
                    } else {
                        false
                    };
                    assert!(
                        there,
                        "{template}/{engine} {file} names `{span}`, which isn't there"
                    );
                    checked_paths += 1;
                }
                // A project with no engine names no engine path at all
                if engine == "none" {
                    assert!(
                        !guide.spans.iter().any(|s| s.starts_with("engine/")),
                        "{file}"
                    );
                }

                // Commands: every `flint <word>` is a subcommand
                for sub in subcommands(&guide.text) {
                    let o = flint(&game, &[&sub, "--help"]);
                    assert!(
                        o.status.success(),
                        "{template}/{engine} {file} names `flint {sub}`, which isn't a command:\n{}",
                        stderr(&o)
                    );
                }

                // And where the engine is there to answer, every `flint`
                // line in a code block works as written
                if engine == "path" {
                    for line in guide.fenced.iter().filter(|l| l.starts_with("flint ")) {
                        if run_line(&game, line).is_some() {
                            ran += 1;
                        }
                    }
                }
            }
        }
    }
    // The checks found something to check
    assert!(checked_paths > 60, "{checked_paths} paths");
    assert!(ran > 15, "{ran} commands");
}

#[test]
fn the_guides_checks_can_fail() {
    // The extractors, on text with known contents
    assert!(is_path("scenes/main.scene.toml"));
    assert!(is_path("flint.toml"));
    assert!(is_path("renders/"));
    assert!(!is_path("on_draw_ui"));
    assert!(!is_path("--width"));
    assert!(!is_path("[entities.<name>]"));
    assert!(!is_path("source = \"hud.rhai\""));
    let found =
        subcommands("Run `flint render x`, then flint play. The Flint engine; flint-engine.");
    assert_eq!(found.into_iter().collect::<Vec<_>>(), ["play", "render"]);
    assert_eq!(
        words("flint query \"entities where x == 1\" --scene s"),
        ["flint", "query", "entities where x == 1", "--scene", "s"]
    );
    // A command that doesn't exist is refused by the same call the test makes
    let dir = scratch("can_fail");
    assert!(!flint(&dir, &["serve-forever", "--help"]).status.success());
}

// ── Box 3: no engine schema is copied into the game ─────────────────

/// The names a schema directory defines: `components/*.toml` and
/// `archetypes/*.toml`, by the table each file declares.
fn schema_names(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (folder, table) in [("components", "component"), ("archetypes", "archetype")] {
        let Ok(files) = std::fs::read_dir(dir.join(folder)) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let doc: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
            if let Some(declared) = doc.get(table).and_then(|t| t.as_table()) {
                names.extend(declared.keys().map(|k| format!("{table}.{k}")));
            }
        }
    }
    names
}

#[test]
fn no_engine_schema_is_copied_into_the_game() {
    let engine = schema_names(&repo().join("schemas"));
    assert!(engine.contains("component.transform") && engine.contains("archetype.player"));
    for template in TEMPLATES {
        for mode in ["path", "subtree", "none"] {
            let dir = scratch(&format!("schemas_{template}_{mode}"));
            let (game, _) = init(&dir, &["--template", template, "--engine", mode]);
            // The folders are there, and define nothing the engine does
            assert!(game.join("schemas/components").is_dir());
            assert!(game.join("schemas/archetypes").is_dir());
            let game_names = schema_names(&game.join("schemas"));
            let copied: Vec<_> = game_names.intersection(&engine).collect();
            assert!(copied.is_empty(), "{template}/{mode}: {copied:?}");
            // No engine directory is written either: it is named
            assert!(!game.join("engine").exists(), "{template}/{mode}");
            assert!(!game.join("schemas/constraints").exists());
        }
    }
}

// ── The engine modes, and what init refuses ─────────────────────────

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

#[test]
fn subtree_prints_the_git_commands_and_works_once_the_engine_is_there() {
    let dir = scratch("subtree");
    let (game, said) = init(&dir, &["--engine", "subtree"]);
    let manifest = std::fs::read_to_string(game.join("flint.toml")).unwrap();
    assert!(manifest.contains("schemas = [\"engine/schemas\", \"schemas\"]"));
    assert!(!game.join(".git").exists(), "init runs no git");

    // It says what to run, and that nothing loads before it
    let steps = next_steps(&said);
    assert!(
        steps.contains(&"git subtree add --prefix=engine flint-engine main --squash".to_string())
    );
    assert!(said.contains("schema directory"), "{said}");
    let before = flint(&game, &["validate", SCENE]);
    assert!(!before.status.success());
    let why = stderr(&before);
    assert!(why.contains("schema directory not found"), "{why}");
    assert!(why.contains("engine/schemas"), "{why}");

    // What the subtree brings, as far as the schemas: then the printed
    // flint commands work, unedited
    copy_dir(&repo().join("schemas"), &game.join("engine/schemas"));
    for step in steps.iter().filter(|s| s.starts_with("flint ")) {
        let _ = run_line(&game, step);
    }
    // bundle.toml needs no mount: engine/schemas is in the game
    let bundle = std::fs::read_to_string(game.join("bundle.toml")).unwrap();
    assert!(!bundle.contains("[mount]"), "{bundle}");
    ok(&flint(&game, &["bundle", ".", "--dry-run"]));
}

#[test]
fn none_names_no_engine_and_says_what_that_costs() {
    let dir = scratch("none");
    let (game, said) = init(&dir, &["--engine", "none"]);
    let manifest = std::fs::read_to_string(game.join("flint.toml")).unwrap();
    assert!(manifest.contains("schemas = [\"schemas\"]"), "{manifest}");
    assert!(said.contains("Engine: none named"), "{said}");
    // It doesn't offer play: the player archetype resolves to nothing
    let steps = next_steps(&said);
    assert!(!steps.iter().any(|s| s.starts_with("flint play")), "{said}");
    for step in steps.iter().filter(|s| s.starts_with("flint ")) {
        let _ = run_line(&game, step);
    }
    // What it says is so: no engine component is known
    let o = flint(&game, &["schema", "transform"]);
    assert!(stdout(&o).contains("not found") || !o.status.success());
}

#[test]
fn path_names_a_checkout_and_refuses_what_isnt_one() {
    let dir = scratch("path");
    let repo = std::path::absolute(repo()).unwrap();
    let (game, said) = init(&dir, &["--engine-path", repo.to_str().unwrap()]);
    let manifest: toml::Table = std::fs::read_to_string(game.join("flint.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let schemas = manifest["project"]["schemas"].as_array().unwrap();
    assert_eq!(schemas.len(), 2);
    let engine = Path::new(schemas[0].as_str().unwrap());
    assert!(engine.is_absolute() && engine.join("components/transform.toml").is_file());
    assert!(!schemas[0].as_str().unwrap().contains('\\'), "{said}");
    assert_eq!(schemas[1].as_str(), Some("schemas"));

    // A directory that isn't a checkout, and a flag that doesn't apply
    let o = flint(&dir, &["init", "other", "--engine-path", "."]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no Flint checkout"), "{}", stderr(&o));
    assert!(
        !dir.join("other").exists(),
        "nothing is written on a refusal"
    );
    let o = flint(
        &dir,
        &["init", "other", "--engine", "subtree", "--engine-path", "."],
    );
    assert!(!o.status.success());
    assert!(!dir.join("other").exists());
}

#[test]
fn a_directory_with_files_is_refused_and_force_overwrites_nothing() {
    let dir = scratch("force");
    let game = dir.join("game");
    std::fs::create_dir_all(game.join("scripts")).unwrap();
    std::fs::write(game.join("scripts/hud.rhai"), "// mine\n").unwrap();
    std::fs::write(game.join("notes.txt"), "keep me\n").unwrap();

    let o = flint(&dir, &["init", "game"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--force"), "{}", stderr(&o));
    assert!(
        !game.join("flint.toml").exists(),
        "nothing is written on a refusal"
    );

    let said = ok(&flint(&dir, &["init", "game", "--force"]));
    assert!(
        said.contains("scripts/hud.rhai") && said.contains("kept"),
        "{said}"
    );
    let read = |p: &str| std::fs::read_to_string(game.join(p)).unwrap();
    assert_eq!(read("scripts/hud.rhai"), "// mine\n");
    assert_eq!(read("notes.txt"), "keep me\n");
    assert!(game.join("flint.toml").is_file());
    assert!(game.join(SCENE).is_file());

    // An empty directory that already exists is fine, and so is `.`
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let said = ok(&flint(&empty, &["init", "."]));
    assert!(
        !next_steps(&said).iter().any(|s| s.starts_with("cd ")),
        "{said}"
    );
    let manifest = std::fs::read_to_string(empty.join("flint.toml")).unwrap();
    assert!(manifest.contains("id = \"empty\""), "{manifest}");

    // A name that would break the files it is written into
    let o = flint(&dir, &["init", "bad{name"]);
    assert!(!o.status.success());
    assert!(!dir.join("bad{name").exists());
}
