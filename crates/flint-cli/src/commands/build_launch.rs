//! A game's launcher (#1562, ADR 0137 decision 6): when a package the game
//! lists is the game's own (`packages/<name>/`, `tier = "game"`) and has
//! Rust, `flint build` writes `<engine>/launch/<id>/`, a crate of the
//! engine's workspace (the `launch/*` members glob, gitignored) whose bins
//! are the game's `flint-player` and `flint`: the stock list of the build's
//! features, then each game package's `player::PACKAGE` and
//! `cli::extensions()`. Cargo runs in the engine, so its crates stay
//! compiled and its `.cargo/config.toml` applies (the 8 MB stack), and
//! `Cargo.lock` is put back byte for byte after, so a subtree stays clean.

use super::*;
use flint_core::package::Package;

/// The generated crate, worked out before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Launch {
    /// `<engine>/launch/<id>`.
    pub dir: PathBuf,
    /// Its `[package] name`, `flint-launch-<id>`.
    pub krate: String,
    /// The game packages it builds in, by name.
    pub packages: Vec<String>,
    /// Each file, by its name in `dir`, and its text.
    pub files: Vec<(&'static str, String)>,
}

/// The id a launcher is named by: the project's `[project] id`, else its
/// folder's name, in lowercase letters, digits and `-`.
fn id_of(project: &Project) -> String {
    let named = project.id.clone().unwrap_or_else(|| {
        let name = project.root.file_name().unwrap_or_default();
        name.to_string_lossy().into_owned()
    });
    let mut id = String::new();
    for c in named.chars() {
        match c.to_ascii_lowercase() {
            c if c.is_ascii_alphanumeric() => id.push(c),
            _ if !id.is_empty() && !id.ends_with('-') => id.push('-'),
            _ => {}
        }
    }
    let id = id.trim_end_matches('-');
    if id.is_empty() { "game" } else { id }.to_string()
}

/// A path as a manifest takes it: forward slashes.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The launcher for `own`, the game packages with Rust the build lists.
pub(super) fn plan(
    project: &Project,
    engine: &Path,
    own: &[&Package],
    player: &Features,
    cli: &Features,
) -> Result<Launch> {
    let id = id_of(project);
    let krate = format!("flint-launch-{id}");
    let quoted = |f: &Features| -> String {
        let all: Vec<String> = f.all().iter().map(|f| format!("{f:?}")).collect();
        all.join(", ")
    };
    let mut deps = format!(
        "flint-player = {{ path = \"../../crates/{PLAYER}\", default-features = false, \
         features = [{}] }}\nflint-cli = {{ path = \"../../crates/{CLI}\", \
         default-features = false, features = [{}] }}\nanyhow = {{ workspace = true }}\n",
        quoted(player),
        quoted(cli)
    );
    let (mut players, mut commands) = (String::new(), String::new());
    for p in own {
        let crates: BTreeSet<&String> = p.player.iter().chain(&p.cli).collect();
        for name in crates {
            let dir = normal(&p.dir().join("crates").join(name));
            if !dir.join("Cargo.toml").is_file() {
                bail!(
                    "{}'s [rust] names `{name}`, and {} has no Cargo.toml: a package's crate \
                     is in its folder's crates/<crate>/",
                    project::shown(&p.file),
                    project::shown(&dir)
                );
            }
            let parts: Vec<&str> = [("player", &p.player), ("cli", &p.cli)]
                .into_iter()
                .filter(|(_, c)| c.as_ref() == Some(name))
                .map(|(part, _)| part)
                .collect();
            deps.push_str(&format!(
                "{name} = {{ path = \"{}\", features = {parts:?} }}\n",
                slashed(&dir)
            ));
        }
        let lib = |c: &String| c.replace('-', "_");
        if let Some(c) = &p.player {
            players.push_str(&format!(
                "    packages.push(&::{}::player::PACKAGE);\n",
                lib(c)
            ));
        }
        if let Some(c) = &p.cli {
            commands.push_str(&format!(
                "    commands.extend(::{}::cli::extensions());\n",
                lib(c)
            ));
        }
    }
    let head = format!(
        "// Written by `flint build` for {id} (#1562); written again each build.\n\
         // The stock list of this build's features, then the game's own packages.\n"
    );
    let list = format!(
        "    let mut packages = flint_player::packages::stock();\n{players}    \
         flint_player::packages::launch(&packages);\n"
    );
    let files = vec![
        (
            "Cargo.toml",
            format!(
                "{MARK}{id} (#1562): the game's flint-player and\n\
                 # flint, with the packages its flint.toml lists. Gitignored.\n\
                 [package]\nname = \"{krate}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 publish = false\n\n[[bin]]\nname = \"{PLAYER}\"\npath = \"player.rs\"\n\n\
                 [[bin]]\nname = \"{CLI_BIN}\"\npath = \"flint.rs\"\n\n[lints]\nworkspace = true\n\n\
                 [dependencies]\n{deps}"
            ),
        ),
        (
            "player.rs",
            format!(
                "{head}fn main() -> anyhow::Result<()> {{\n{list}    \
                 flint_player::packages::sweep_set_aside();\n    \
                 let cli = flint_player::PlayerCli::parse_launched();\n    \
                 flint_player::logging::init(&cli.args.log);\n    cli.run()\n}}\n"
            ),
        ),
        (
            "flint.rs",
            format!(
                "{head}fn main() -> std::process::ExitCode {{\n{list}    \
                 #[allow(unused_mut)]\n    let mut commands = Vec::new();\n{commands}    \
                 flint_cli::run_with(commands)\n}}\n"
            ),
        ),
    ];
    Ok(Launch {
        dir: dir_for(project, engine),
        krate,
        packages: own.iter().map(|p| p.name.clone()).collect(),
        files,
    })
}

/// Where the game's launcher goes: `<engine>/launch/<id>`.
pub(super) fn dir_for(project: &Project, engine: &Path) -> PathBuf {
    engine.join("launch").join(id_of(project))
}

/// Take away the launcher an earlier build wrote at `dir`, once the game
/// lists no package with Rust: a member naming a crate that is gone would
/// fail every cargo command in the engine. Only a folder this command
/// wrote, by its manifest's first line.
pub(super) fn remove_stale(dir: &Path) -> Result<()> {
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    if manifest.starts_with(MARK) {
        std::fs::remove_dir_all(dir)
            .with_context(|| format!("removing the old launcher {}", project::shown(dir)))?;
    }
    Ok(())
}

/// [`remove_stale`] when a crate the launcher names is gone (a game
/// package's folder deleted): until then every cargo command in the
/// engine fails, `cargo metadata --no-deps` too, which planning asks
/// first. A dry run takes it away as well.
pub(super) fn remove_broken(dir: &Path) -> Result<()> {
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    let gone = manifest
        .lines()
        .filter_map(|line| line.split_once("{ path = \"")?.1.split_once('"'))
        .any(|(path, _)| !dir.join(path).join("Cargo.toml").is_file());
    if gone {
        remove_stale(dir)?;
    }
    Ok(())
}

/// How a launcher's manifest begins.
const MARK: &str = "# Written by `flint build` for ";

/// Write the launcher's files that differ from what is there.
pub(super) fn write(launch: &Launch) -> Result<()> {
    std::fs::create_dir_all(&launch.dir)
        .with_context(|| format!("creating {}", project::shown(&launch.dir)))?;
    for (name, text) in &launch.files {
        let file = launch.dir.join(name);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text) {
            std::fs::write(&file, text)
                .with_context(|| format!("writing {}", project::shown(&file)))?;
        }
    }
    Ok(())
}
