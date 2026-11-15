//! `flint init`: start a game project (#707).
//!
//! The project is the game-repo pattern, written from the files under
//! `crates/flint-cli/templates/`, which are compiled into the binary:
//! a `flint.toml` manifest (ADR 0114) naming the schemas, the start scene
//! and the input config; a lit scene with a `[camera]`; a script-drawn HUD;
//! an input layer; packing rules; an empty game `schemas/` (the engine's
//! are named, never copied); and an agent guide that names only what the
//! project and this binary hold (`tests/init_cli.rs` checks that it does).
//!
//! `--engine` says where the engine's schemas come from:
//!
//! - `path` (the default): the Flint checkout this binary was built from, or
//!   `--engine-path`, named by absolute path. The printed commands work at
//!   once, with no git and no network.
//! - `subtree`: `engine/schemas`, the pattern the games use. This command
//!   runs no git, so it prints the `git subtree add` to run, and says that
//!   nothing loads until it has been.
//! - `none`: only the game's own `schemas/`. Scenes validate and render
//!   (a component is a table with or without a schema), but an engine
//!   archetype resolves to nothing, so the output doesn't offer `play`.
//!
//! Nothing is ever deleted or overwritten: a directory with files in it is
//! refused unless `--force`, and with `--force` a file already there is kept.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct InitArgs {
    /// The directory to create. Its last part is the project's name
    pub name: String,

    /// What the first scene is: `walk` (a first-person player in a lit
    /// 3D scene), `blank` (a lit ground, one box, no player) or `2d`
    /// (sprites under an orthographic camera, one of them steered)
    #[arg(long, value_enum, default_value_t = Template::Walk)]
    pub template: Template,

    /// Where the engine's schemas come from: `path` (a Flint checkout on
    /// this machine, named by path; works at once), `subtree` (`engine/`
    /// inside the game; prints the `git subtree add` to run first) or
    /// `none` (no engine named)
    #[arg(long, value_enum, default_value_t = EngineMode::Path)]
    pub engine: EngineMode,

    /// With `--engine path`: the Flint checkout to name. Without it, the
    /// one this binary was built from
    #[arg(long, value_name = "DIR")]
    pub engine_path: Option<PathBuf>,

    /// Write into a directory that already holds files. A file already
    /// there is kept as it is; nothing is overwritten or deleted
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Template {
    Walk,
    Blank,
    #[value(name = "2d")]
    TwoD,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum EngineMode {
    Subtree,
    Path,
    None,
}

/// A template file: where it goes in the project, and its text.
type File = (&'static str, &'static str);

macro_rules! template {
    ($path:literal) => {
        include_str!(concat!("../../templates/", $path))
    };
}

/// In every project, whatever the template.
const SHARED: &[File] = &[
    ("flint.toml", template!("shared/flint.toml")),
    ("CLAUDE.md", template!("shared/guide.md")),
    ("AGENTS.md", template!("shared/agents.md")),
    ("bundle.toml", template!("shared/bundle.toml")),
    ("config/input.toml", template!("shared/config/input.toml")),
    (".gitignore", template!("shared/gitignore")),
    // Folders git would not keep empty. The game's schemas start empty:
    // the engine's are named by flint.toml, not copied.
    ("schemas/components/.gitkeep", ""),
    ("schemas/archetypes/.gitkeep", ""),
    ("renders/.gitignore", "*\n!.gitignore\n"),
];

impl Template {
    fn name(self) -> &'static str {
        match self {
            Template::Walk => "walk",
            Template::Blank => "blank",
            Template::TwoD => "2d",
        }
    }

    fn files(self) -> &'static [File] {
        match self {
            Template::Walk => &[
                (
                    "scenes/main.scene.toml",
                    template!("walk/scenes/main.scene.toml"),
                ),
                ("scripts/hud.rhai", template!("walk/scripts/hud.rhai")),
            ],
            Template::Blank => &[
                (
                    "scenes/main.scene.toml",
                    template!("blank/scenes/main.scene.toml"),
                ),
                ("scripts/hud.rhai", template!("blank/scripts/hud.rhai")),
            ],
            Template::TwoD => &[
                (
                    "scenes/main.scene.toml",
                    template!("2d/scenes/main.scene.toml"),
                ),
                ("scripts/hud.rhai", template!("2d/scripts/hud.rhai")),
                ("scripts/player.rhai", template!("2d/scripts/player.rhai")),
            ],
        }
    }

    /// One sentence for the agent guide's opening.
    fn blurb(self) -> &'static str {
        match self {
            Template::Walk => {
                "The first scene is a ring of standing stones round a lit plinth, with a first-person player who starts outside the ring, facing it."
            }
            Template::Blank => {
                "The first scene is a lit ground and one box, with a camera and no player: a clean start."
            }
            Template::TwoD => {
                "The first scene is 2D: flat sprites under an orthographic camera, one of them steered with WASD."
            }
        }
    }

    /// What the guide's file table adds about `scripts/`.
    fn scripts_note(self) -> &'static str {
        match self {
            Template::TwoD => "`scripts/player.rhai` steers the player sprite.",
            _ => "",
        }
    }

    /// The web page's controls line.
    fn controls(self) -> &'static str {
        match self {
            Template::Walk => "WASD to walk, mouse to look, Space to jump, H for help",
            Template::Blank => "H for help",
            Template::TwoD => "WASD to move, H for help",
        }
    }

    /// What the scene shows, for the printed summary.
    fn scene_line(self) -> &'static str {
        match self {
            Template::Walk => "a lit ring of stones, a [camera], and a player facing it",
            Template::Blank => "a lit ground, one box and a [camera]",
            Template::TwoD => "sprites under an orthographic [camera], one steered with WASD",
        }
    }
}

/// A path as a manifest and a shell both take it: forward slashes.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The Flint tree this binary was built from, as `flint bundle` finds it.
fn built_from() -> PathBuf {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR"));
    cli.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cli.join("../.."))
}

/// The engine checkout `--engine path` names, checked to be one.
fn engine_checkout(given: Option<&Path>) -> Result<PathBuf> {
    let (dir, whose) = match given {
        Some(dir) => (
            std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()),
            "--engine-path",
        ),
        None => (built_from(), "the checkout this flint was built from"),
    };
    if !dir.join("schemas/components/transform.toml").is_file() {
        bail!(
            "no Flint checkout at {} ({whose}): it has no schemas/components/transform.toml.\n\
             Name one with --engine-path <dir>, or use --engine subtree to carry the engine \
             inside the game.",
            slashed(&dir)
        );
    }
    Ok(dir)
}

/// The project's name from the directory asked for, and an id made of it.
fn names(dir: &Path) -> Result<(String, String)> {
    let named = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| {
            // `.` or a path ending in `..`: the directory's real name
            let full = std::path::absolute(dir).ok()?;
            let full = fs::canonicalize(&full).unwrap_or(full);
            full.file_name().map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    if named.is_empty() {
        bail!("{}: not a name for a project directory", dir.display());
    }
    // The name is written into TOML strings, a script string and Markdown
    if let Some(bad) = named
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')))
    {
        bail!(
            "project name `{named}`: `{bad}` can't be part of one (letters, digits, spaces, `-`, `_` and `.` can)"
        );
    }
    let mut id = String::new();
    for c in named.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.ends_with('_') && !id.is_empty() {
            id.push('_');
        }
    }
    let id = id.trim_end_matches('_').to_string();
    Ok((named, if id.is_empty() { "game".into() } else { id }))
}

/// Whether `dir` holds anything.
fn has_files(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// `text` with each `{{key}}` replaced. Values are filled in the order
/// given, so a value may itself hold a later key.
fn fill(text: &str, values: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (key, value) in values {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

pub fn run(args: InitArgs) -> Result<()> {
    let dir = Path::new(&args.name);
    if args.engine_path.is_some() && args.engine != EngineMode::Path {
        bail!("--engine-path goes with --engine path");
    }
    if dir.is_file() {
        bail!("{} is a file", dir.display());
    }
    if has_files(dir) && !args.force {
        bail!(
            "{} already holds files. Name a new directory, or pass --force to write \
             the project into it (a file already there is kept, never overwritten).",
            dir.display()
        );
    }
    let (name, id) = names(dir)?;

    // Where the engine's schemas are, as flint.toml names them
    let engine_dir = match args.engine {
        EngineMode::Path => Some(slashed(&engine_checkout(args.engine_path.as_deref())?)),
        EngineMode::Subtree => Some("engine".to_string()),
        EngineMode::None => None,
    };
    let schemas = match &engine_dir {
        Some(engine) => format!("[\"{engine}/schemas\", \"schemas\"]"),
        None => "[\"schemas\"]".to_string(),
    };
    // A bundle holds the engine's schemas at engine/schemas wherever they
    // came from, so a path outside the game is mounted there
    let bundle_engine = match (args.engine, &engine_dir) {
        (EngineMode::Path, Some(engine)) => format!(
            "\n# The engine's schemas live outside this project (flint.toml names them by\n\
             # path), so they are packed from there to engine/schemas in the bundle.\n\
             schemas = [\"engine/schemas\", \"schemas\"]\n\n\
             [mount]\n\"engine/schemas\" = \"{engine}/schemas\"\n"
        ),
        _ => String::new(),
    };
    let (engine_section, schemas_section, script_docs) = match args.engine {
        EngineMode::Path => (
            template!("shared/engine-path.md"),
            template!("shared/schemas-engine.md"),
            template!("shared/scripts-engine.md"),
        ),
        EngineMode::Subtree => (
            template!("shared/engine-subtree.md"),
            template!("shared/schemas-engine.md"),
            template!("shared/scripts-engine.md"),
        ),
        EngineMode::None => (
            template!("shared/engine-none.md"),
            template!("shared/schemas-none.md"),
            template!("shared/scripts-none.md"),
        ),
    };
    let template = args.template;
    let values: &[(&str, &str)] = &[
        // Sections first: they hold the keys below
        ("engine_section", engine_section.trim_end()),
        ("schemas_section", schemas_section.trim_end()),
        ("script_docs", script_docs.trim_end()),
        ("bundle_engine", &bundle_engine),
        ("template_blurb", template.blurb()),
        ("scripts_note", template.scripts_note()),
        ("controls", template.controls()),
        ("template", template.name()),
        ("engine", engine_dir.as_deref().unwrap_or("")),
        ("schemas", &schemas),
        ("name", &name),
        ("id", &id),
    ];

    let mut written = Vec::new();
    let mut kept = Vec::new();
    for (path, text) in SHARED.iter().chain(template.files()) {
        let to = dir.join(path);
        if to.exists() {
            kept.push(*path);
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&to, fill(text, values)).with_context(|| format!("writing {}", to.display()))?;
        written.push(*path);
    }

    report(&args, &name, engine_dir.as_deref(), &written, &kept);
    Ok(())
}

/// What a file is, for the printed list.
fn describes(path: &str, template: Template) -> String {
    match path {
        "flint.toml" => "the manifest: id, schemas, start scene, input".into(),
        "CLAUDE.md" => "the guide to this project, for you and for agents".into(),
        "AGENTS.md" => "points other agents at CLAUDE.md".into(),
        "bundle.toml" => "packing rules for the web build".into(),
        "config/input.toml" => "the game's input actions".into(),
        ".gitignore" => "what the engine writes while you work".into(),
        "schemas/components/.gitkeep" | "schemas/archetypes/.gitkeep" => {
            "the game's own schemas: empty, the engine's are not copied".into()
        }
        "renders/.gitignore" => "where renders go".into(),
        "scenes/main.scene.toml" => template.scene_line().into(),
        "scripts/hud.rhai" => "the HUD, drawn by the scene's hud_controller".into(),
        "scripts/player.rhai" => "steers the player sprite".into(),
        _ => String::new(),
    }
}

fn report(args: &InitArgs, name: &str, engine: Option<&str>, written: &[&str], kept: &[&str]) {
    let template = args.template;
    println!(
        "Created Flint project `{name}` ({} template) in {}",
        template.name(),
        slashed(Path::new(&args.name))
    );
    println!();
    let mut paths: Vec<&str> = written.to_vec();
    paths.sort_by_key(|p| (p.matches('/').count() > 0, *p));
    for path in paths {
        // A folder kept by a placeholder file is listed as the folder
        let shown = match path.rsplit_once('/') {
            Some((folder, ".gitkeep" | ".gitignore")) => format!("{folder}/"),
            _ => path.to_string(),
        };
        println!("  {shown:<26} {}", describes(path, template));
    }
    for path in kept {
        println!("  {path:<26} kept: it was already there");
    }
    println!();

    let scene = "scenes/main.scene.toml";
    match (args.engine, engine) {
        (EngineMode::Path, Some(engine)) => {
            println!("Engine: {engine}");
            println!("  flint.toml names its schemas by that path, so the commands below work");
            println!("  now, on this machine. CLAUDE.md says how to carry the engine inside the");
            println!("  game instead (a git subtree at engine/), as Flint's games do.");
        }
        (EngineMode::Subtree, _) => {
            println!("Engine: engine/ (a git subtree; not added yet)");
            println!("  flint.toml names engine/schemas. flint init runs no git, so until the");
            println!("  subtree is added every flint command here stops with `schema directory");
            println!("  not found`.");
        }
        _ => {
            println!("Engine: none named");
            println!("  flint.toml lists only the game's own, empty schemas/. The scene validates");
            println!(
                "  and renders, but an engine archetype resolves to nothing and `flint schema`"
            );
            println!("  finds no engine component. CLAUDE.md says how to name an engine.");
        }
    }
    println!();
    println!("Next steps:");
    if Path::new(&args.name) != Path::new(".") {
        let dir = slashed(Path::new(&args.name));
        if dir.contains(' ') {
            println!("  cd \"{dir}\"");
        } else {
            println!("  cd {dir}");
        }
    }
    if args.engine == EngineMode::Subtree {
        println!("  git init");
        println!("  git add .");
        println!("  git commit -m \"Start {name}\"");
        println!("  git remote add flint-engine https://github.com/chrischaps/Flint.git");
        println!("  git subtree add --prefix=engine flint-engine main --squash");
    }
    println!("  flint check");
    println!("  flint render {scene} --output renders/main.png");
    if args.engine != EngineMode::None {
        println!("  flint play");
    }
    println!();
    println!("CLAUDE.md in the project is the map: the loop, where each file lives, how to");
    println!("find components and script functions, and what trips people up.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_made_of_the_name() {
        let id = |n: &str| names(Path::new(n)).unwrap().1;
        assert_eq!(id("x"), "x");
        assert_eq!(id("games/My Tavern"), "my_tavern");
        assert_eq!(id("lantern-quest.2"), "lantern_quest_2");
        assert_eq!(id("___"), "game");
        assert_eq!(names(Path::new("a/b/Float")).unwrap().0, "Float");
    }

    #[test]
    fn a_name_that_would_break_a_string_is_refused() {
        for bad in ["say \"hi\"", "a{b", "tab\there", "back`tick"] {
            assert!(names(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_template_file_leaves_a_key_unfilled() {
        // Every `{{key}}` a template file uses is one `run` fills
        let known = [
            "engine_section",
            "schemas_section",
            "script_docs",
            "bundle_engine",
            "template_blurb",
            "scripts_note",
            "controls",
            "template",
            "engine",
            "schemas",
            "name",
            "id",
        ];
        let sections = [
            template!("shared/engine-path.md"),
            template!("shared/engine-subtree.md"),
            template!("shared/engine-none.md"),
            template!("shared/schemas-engine.md"),
            template!("shared/schemas-none.md"),
            template!("shared/scripts-engine.md"),
            template!("shared/scripts-none.md"),
        ];
        let files = SHARED
            .iter()
            .chain(Template::Walk.files())
            .chain(Template::Blank.files())
            .chain(Template::TwoD.files())
            .map(|(_, text)| *text)
            .chain(sections);
        for text in files {
            let mut rest = text;
            while let Some(at) = rest.find("{{") {
                let after = &rest[at + 2..];
                let end = after.find("}}").expect("an unclosed {{");
                assert!(
                    known.contains(&&after[..end]),
                    "unknown key {{{{{}}}}}",
                    &after[..end]
                );
                rest = &after[end + 2..];
            }
        }
    }
}
