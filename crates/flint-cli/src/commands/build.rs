//! `flint build` (#1335, ADR 0136 decision 10): the project's player and
//! CLI, built with the packages its `flint.toml` lists and nothing else.
//!
//! ```bash
//! flint build                 # a debug flint-player and flint for this project
//! flint build --release       # an optimized pair
//! flint build --dry-run       # the cargo command(s) and where the binaries land
//! flint build --engine-path ../Flint   # another engine checkout
//! ```
//!
//! It reads `[packages]` (#1334) against the engine's manifests
//! (`packages/<name>/package.toml`), turns each package that is in into
//! the Cargo features its `[build] features` names (on flint-player,
//! flint-cli, or both: whichever crate has the feature), keeps each crate's
//! build options (the default features that are no package's: `audio`,
//! `gamepad`, `frame-history`, `dialogs`), and runs `cargo build
//! --no-default-features --features ...` in the engine workspace. With no
//! `[packages]` table every package is in, which is the default build.
//!
//! **One cargo run, or two.** Cargo unifies a crate's features across what
//! one run builds, and flint-cli turns on flint-player's `frame-history`
//! in every build. A project whose player leaves out a feature the CLI
//! forces would get it back in a shared run, so its player is built alone
//! first, then the CLI. `debug-hud` was one until #1583; it is a package
//! feature of both crates now, so `debug-hud = false` is one run.
//!
//! **A game's own binary** (#1532). A game that embeds `PlayerApp` in a
//! crate of its own (a `[package]`, or a workspace's member, at the project
//! root that depends on flint-player or flint-cli) gets one more cargo run,
//! in the project root: `cargo build -p <game> --no-default-features
//! --features flint-player/<f>,...` with the stock player's features from
//! the same plan (and flint-cli's, when it depends on that too), plus the
//! game's own defaults. `cargo metadata` finds the crate. What would bring a
//! left-out package back is refused before anything builds, naming the
//! line: a launcher dependency taking its defaults (every package) or
//! pinning one in `features`, or a default feature of the game's that does.
//!
//! **A game's own packages** (#1562): a package in the project's
//! `packages/` (`tier = "game"`) that the list lets in and that has Rust
//! (`[rust] player` or `cli`) is built into a launcher crate `flint build`
//! writes, `<engine>/launch/<id>/`, in place of the stock binaries' run
//! (`build_launch.rs`).
//!
//! **A binary that is running.** Windows refuses to overwrite a running
//! `.exe` (`flint.exe` doing this build, a `flint-player.exe` the game is
//! open in), but lets it be renamed in its folder. So before cargo runs,
//! each output binary that exists is renamed to `<name>.old.exe`; a failed
//! build renames them back, and the next start of each binary deletes its
//! old copy (`flint_player::packages::sweep_set_aside`). Elsewhere nothing
//! is renamed.
//!
//! The engine: `--engine-path`, else the project's `engine/` subtree, else
//! the engine its schemas come from, else the project itself when it is an
//! engine checkout, else the tree this flint was built from. The first
//! build of a game, before any flint exists, is `cargo run -p flint-cli --
//! build` from the engine folder.

use crate::output::{self, say, Answer};
use anyhow::{bail, Context, Result};
use flint_core::package::{InUse, Packages, Tier};
use flint_core::project::{self, Project};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "build_launch.rs"]
mod launch;

/// The two binaries, by package and target name.
const PLAYER: &str = "flint-player";
const CLI: &str = "flint-cli";
const CLI_BIN: &str = "flint";

#[derive(clap::Args, Debug)]
pub struct BuildArgs {
    /// Build optimized binaries (cargo's `--release`)
    #[arg(long)]
    pub release: bool,

    /// The target triple to build for (cargo's `--target`), e.g.
    /// x86_64-unknown-linux-gnu
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Print the cargo command(s) and where the binaries would land, and
    /// build nothing
    #[arg(long)]
    pub dry_run: bool,

    /// The engine workspace to build in. Without it: the project's
    /// `engine/`, else the engine its schemas come from, else the tree this
    /// flint was built from
    #[arg(long, value_name = "DIR")]
    pub engine_path: Option<PathBuf>,
}

/// A package in the build, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Included {
    name: String,
    tier: Tier,
    why: String,
}

/// One `cargo build` and the binaries it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    /// The words after `cargo`.
    args: Vec<String>,
    /// Where cargo runs: the engine workspace, or a game's root.
    dir: PathBuf,
    /// The output binaries.
    bins: Vec<Bin>,
}

/// A binary a run writes, by its target name, and where it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bin {
    name: String,
    path: PathBuf,
}

/// A game's own binary crate at the project root (#1532).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Game {
    /// Its `[package] name`.
    package: String,
    /// Its Cargo.toml.
    manifest: PathBuf,
    /// Its bin targets.
    bins: Vec<String>,
    /// The launcher crates it depends on (`flint-player`, `flint-cli`), each
    /// with the name its Cargo.toml gives the dependency.
    launchers: Vec<(&'static str, String)>,
    /// Every `--features` word its run passes, its own defaults last.
    features: Vec<String>,
}

/// What `flint build` does, worked out before anything runs.
#[derive(Debug, Clone)]
struct Plan {
    project: PathBuf,
    engine: PathBuf,
    /// How the engine was found, in words.
    engine_from: &'static str,
    /// The `flint.toml` whose `[packages]` decided it, if one did.
    manifest: Option<PathBuf>,
    included: Vec<Included>,
    left_out: Vec<String>,
    /// The packages the player's launcher hands the kernel (#1552): the
    /// kernel, then each package in the build with a part in flint-player.
    launcher: Vec<String>,
    /// Each crate's package features, then its build options.
    player: Features,
    cli: Features,
    /// The game's own binary crates at the project root, each built in a
    /// run of its own after the engine's (#1532).
    games: Vec<Game>,
    runs: Vec<Run>,
    /// Why the player builds in a run of its own, when it does.
    split: Option<String>,
    /// The launcher written for the game's own packages, when it has any
    /// with Rust (#1562).
    launch: Option<launch::Launch>,
    /// Where that launcher goes: a build with none takes away one an
    /// earlier build left there.
    launch_dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Features {
    packages: Vec<String>,
    options: Vec<String>,
}

impl Features {
    fn all(&self) -> Vec<String> {
        self.packages.iter().chain(&self.options).cloned().collect()
    }
}

pub fn run(args: BuildArgs) -> Result<()> {
    let project = Project::current()?;
    let (engine, engine_from) = find_engine(&project, args.engine_path.as_deref())?;
    launch::remove_broken(&launch::dir_for(&project, &engine))?;
    let plan = plan(&project, engine, engine_from, &args)?;
    if args.dry_run {
        return output::emit(&answer(&plan, &Outcome::DryRun));
    }
    say!("{}", plan_text(&plan));
    let began = std::time::Instant::now();
    let outcome = build(&plan);
    let outcome = match outcome {
        Ok(set_aside) => Outcome::Built {
            seconds: began.elapsed().as_secs_f64(),
            set_aside,
        },
        Err(e) => Outcome::Failed(format!("{e:#}")),
    };
    let failed = matches!(outcome, Outcome::Failed(_));
    let mut answer = answer(&plan, &outcome);
    // The plan was said before cargo ran; the text now is how it ended
    answer.text = outcome_text(&plan, &outcome);
    output::emit(&answer.failed_if(failed))
}

/// How a build ended.
enum Outcome {
    DryRun,
    Built {
        seconds: f64,
        set_aside: Vec<PathBuf>,
    },
    Failed(String),
}

// ── The engine ───────────────────────────────────────────────────────

/// Whether `dir` is an engine workspace `flint build` can build in.
fn is_engine_workspace(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file()
        && project::is_engine_checkout(dir)
        && dir.join("crates").join(PLAYER).join("Cargo.toml").is_file()
        && dir.join("crates").join(CLI).join("Cargo.toml").is_file()
}

/// The engine workspace for `project`, and how it was found.
pub(crate) fn find_engine(
    project: &Project,
    flag: Option<&Path>,
) -> Result<(PathBuf, &'static str)> {
    if let Some(dir) = flag {
        let dir = normal(dir);
        if !is_engine_workspace(&dir) {
            bail!(
                "--engine-path {}: not a Flint engine workspace (no Cargo.toml with \
                 crates/flint-player and crates/flint-cli there)",
                project::shown(&dir)
            );
        }
        return Ok((dir, "--engine-path"));
    }
    let subtree = project.root.join("engine");
    if is_engine_workspace(&subtree) {
        return Ok((normal(&subtree), "the project's engine/"));
    }
    let manifest = project.root.join(project::MANIFEST_FILE);
    if let Ok(dirs) = project::schema_dirs(Some(&manifest), &[] as &[&Path]) {
        if let Some(engine) = project::engine_packages_for(&dirs.dirs)
            .as_deref()
            .and_then(Path::parent)
            .filter(|dir| is_engine_workspace(dir))
        {
            return Ok((normal(engine), "the engine its schemas come from"));
        }
    }
    if is_engine_workspace(&project.root) {
        return Ok((normal(&project.root), "the project, an engine checkout"));
    }
    let built = built_from();
    if is_engine_workspace(&built) {
        return Ok((built, "the tree this flint was built from"));
    }
    bail!(
        "no engine to build for {}: no engine/ there, none behind its schemas, and \
         this flint's own tree ({}) is gone (pass --engine-path)",
        project::shown(&project.root),
        project::shown(&built)
    )
}

/// The Flint tree this binary was built from.
fn built_from() -> PathBuf {
    normal(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// `path` absolute, with `.` and `..` folded away by its words alone.
pub(crate) fn normal(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

// ── The plan ─────────────────────────────────────────────────────────

fn plan(
    project: &Project,
    engine: PathBuf,
    engine_from: &'static str,
    args: &BuildArgs,
) -> Result<Plan> {
    // The engine's packages, then the game's own (#1562)
    let game_packages = project::game_packages_beside(&project.root.join("schemas"));
    let packages = Packages::read_with_game(
        &engine.join(flint_core::package::PACKAGES_DIR),
        game_packages.as_deref(),
    );
    if packages.failed() {
        let lines: Vec<String> = packages.problems.iter().map(|d| d.one_line()).collect();
        bail!(
            "the engine's package manifests don't read:\n  {}",
            lines.join("\n  ")
        );
    }
    let in_use = match &project.packages {
        Some(list) => Some(InUse::resolve(list, &packages).map_err(|problems| {
            let lines: Vec<String> = problems.iter().map(|d| d.one_line()).collect();
            anyhow::anyhow!("[packages] doesn't resolve:\n  {}", lines.join("\n  "))
        })?),
        None => None,
    };
    let (included, left_out) = included(in_use.as_ref(), project.packages.as_ref(), &packages);

    let player_toml = cargo_toml(&engine.join("crates").join(PLAYER))?;
    let cli_toml = cargo_toml(&engine.join("crates").join(CLI))?;
    let package_features: BTreeSet<String> = packages
        .packages
        .iter()
        .flat_map(|p| p.features.iter().cloned())
        .collect();
    let mut player = Features::default();
    let mut cli = Features::default();
    let mut launcher = Vec::new();
    for inc in &included {
        let p = packages
            .get(&inc.name)
            .expect("included from the manifests");
        // The kernel registers first, in every launcher
        if p.tier == Tier::Kernel {
            launcher.insert(0, p.name.clone());
        }
        for f in &p.features {
            let on_player = has_feature(&player_toml, f);
            let on_cli = has_feature(&cli_toml, f);
            // A folder newer than the launchers' generated features (#1587)
            if !on_player && !on_cli {
                bail!(
                    "{}'s [build] features names `{f}`, which neither flint-player nor \
                     flint-cli has: the package's folder is newer than the launchers' \
                     features. Run `{}` in {}, then `flint build` again",
                    project::shown(&p.file),
                    flint_package::cli::SYNC,
                    project::shown(&engine)
                );
            }
            if on_player && !player.packages.contains(f) {
                player.packages.push(f.clone());
            }
            if on_player && !launcher.contains(&p.name) {
                launcher.push(p.name.clone());
            }
            if on_cli && !cli.packages.contains(f) {
                cli.packages.push(f.clone());
            }
        }
    }
    player.options = build_options(&player_toml, &package_features);
    cli.options = build_options(&cli_toml, &package_features);
    // The game's own packages with Rust, which a generated launcher lists
    let own: Vec<&flint_core::package::Package> = included
        .iter()
        .filter_map(|i| packages.get(&i.name))
        .filter(|p| p.tier == Tier::Game && (p.player.is_some() || p.cli.is_some()))
        .collect();
    launcher.extend(
        own.iter()
            .filter(|p| p.player.is_some())
            .map(|p| p.name.clone()),
    );
    let launch = if own.is_empty() {
        None
    } else {
        Some(launch::plan(project, &engine, &own, &player, &cli)?)
    };

    // What flint-cli turns on in flint-player, which a shared run gives
    // the player too
    let forced = forced_player_features(&cli_toml, &cli.all());
    let player_has = closure(&player_toml, &player.all());
    let missing: Vec<&String> = forced.iter().filter(|f| !player_has.contains(*f)).collect();
    let split = (!missing.is_empty()).then(|| {
        let names: Vec<String> = missing.iter().map(|f| format!("`{f}`")).collect();
        format!(
            "flint-cli turns on flint-player's {}, which this player leaves out: one \
             cargo run would build it in",
            names.join(" and ")
        )
    });

    let tail = |run: &mut Vec<String>| {
        if args.release {
            run.push("--release".into());
        }
        if let Some(t) = &args.target {
            run.push("--target".into());
            run.push(t.clone());
        }
    };
    let qualified = |krate: &str, f: &Features| -> Vec<String> {
        f.all().iter().map(|f| format!("{krate}/{f}")).collect()
    };
    let words = |crates: &[(&str, Vec<String>)], features: Vec<String>| -> Vec<String> {
        let mut run = vec!["build".to_string()];
        for (krate, _) in crates {
            run.push("-p".into());
            run.push(krate.to_string());
        }
        for (_, bins) in crates {
            for bin in bins {
                run.push("--bin".into());
                run.push(bin.clone());
            }
        }
        run.push("--no-default-features".into());
        if !features.is_empty() {
            run.push("--features".into());
            run.push(features.join(","));
        }
        tail(&mut run);
        run
    };
    let engine_out = out_dir(&target_dir(&engine), args);
    let bins_in = |dir: &Path, names: &[&str]| -> Vec<Bin> {
        names
            .iter()
            .map(|name| Bin {
                name: name.to_string(),
                path: dir.join(exe_name(name, args.target.as_deref())),
            })
            .collect()
    };
    let one = |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
    let mut runs = Vec::new();
    if let Some(launch) = &launch {
        if let Some(why) = &split {
            bail!("a game's launcher builds the player and flint in one run, and {why}");
        }
        let mut args = vec!["build".to_string(), "-p".into(), launch.krate.clone()];
        for bin in [PLAYER, CLI_BIN] {
            args.push("--bin".into());
            args.push(bin.into());
        }
        tail(&mut args);
        runs.push(Run {
            args,
            dir: engine.clone(),
            bins: bins_in(&engine_out, &[PLAYER, CLI_BIN]),
        });
    } else if split.is_some() {
        runs.push(Run {
            args: words(&[(PLAYER, one(&[PLAYER]))], qualified(PLAYER, &player)),
            dir: engine.clone(),
            bins: bins_in(&engine_out, &[PLAYER]),
        });
        runs.push(Run {
            args: words(&[(CLI, one(&[CLI_BIN]))], qualified(CLI, &cli)),
            dir: engine.clone(),
            bins: bins_in(&engine_out, &[CLI_BIN]),
        });
    } else {
        let mut features = qualified(PLAYER, &player);
        features.extend(qualified(CLI, &cli));
        runs.push(Run {
            args: words(
                &[(PLAYER, one(&[PLAYER])), (CLI, one(&[CLI_BIN]))],
                features,
            ),
            dir: engine.clone(),
            bins: bins_in(&engine_out, &[PLAYER, CLI_BIN]),
        });
    }

    // The game's own binaries, each from the same features (#1532)
    let launchers = [(PLAYER, &player_toml, &player), (CLI, &cli_toml, &cli)];
    let (games, game_target) = games(&project.root, &engine, &launchers, &package_features)?;
    if let Some(target) = game_target {
        let game_out = out_dir(&target, args);
        for game in &games {
            let names: Vec<&str> = game.bins.iter().map(String::as_str).collect();
            runs.push(Run {
                args: words(&[(&game.package, game.bins.clone())], game.features.clone()),
                dir: project.root.clone(),
                bins: bins_in(&game_out, &names),
            });
        }
    }

    Ok(Plan {
        project: project.root.clone(),
        launch_dir: launch::dir_for(project, &engine),
        engine,
        engine_from,
        manifest: in_use.map(|u| u.manifest),
        included,
        left_out,
        launcher,
        player,
        cli,
        games,
        runs,
        split,
        launch,
    })
}

/// `target` with `--target`'s triple and the profile's folder.
fn out_dir(target: &Path, args: &BuildArgs) -> PathBuf {
    let mut dir = target.to_path_buf();
    if let Some(t) = &args.target {
        dir.push(t);
    }
    dir.push(if args.release { "release" } else { "debug" });
    dir
}

/// A binary's file name for `target` (this machine's without one).
fn exe_name(bin: &str, target: Option<&str>) -> String {
    let windows = match target {
        Some(t) => t.contains("windows"),
        None => cfg!(windows),
    };
    format!("{bin}{}", if windows { ".exe" } else { "" })
}

// ── A game's own binary (#1532) ──────────────────────────────────────

/// The launcher crates a game's binary may depend on, each with its
/// Cargo.toml and the plan's features for it.
type Launchers<'a> = [(&'static str, &'a toml::Table, &'a Features); 2];

/// The game's own binary crates at `root`, planned, and the target folder
/// cargo writes them into. None at a root with no Cargo.toml, or one that is
/// the engine's own workspace. Refused when a crate's launcher dependency
/// would bring a left-out package back in.
fn games(
    root: &Path,
    engine: &Path,
    launchers: &Launchers,
    package_features: &BTreeSet<String>,
) -> Result<(Vec<Game>, Option<PathBuf>)> {
    let root = normal(root);
    if !root.join("Cargo.toml").is_file() || root == *engine || is_engine_workspace(&root) {
        return Ok((Vec::new(), None));
    }
    let meta = metadata(&root)?;
    let members: BTreeSet<&str> = strs(&meta["workspace_members"]).collect();
    let mut games = Vec::new();
    for package in arr(&meta["packages"]) {
        let manifest = normal(Path::new(package["manifest_path"].as_str().unwrap_or("")));
        // The game's crates, not the engine's (a subtree a workspace counts in)
        let ours = manifest.starts_with(&root) && !manifest.starts_with(engine);
        if ours
            && package["id"]
                .as_str()
                .is_some_and(|id| members.contains(id))
        {
            games.extend(game_of(package, manifest, launchers, package_features)?);
        }
    }
    let target = meta["target_directory"].as_str().map(PathBuf::from);
    Ok((games, target.or_else(|| Some(root.join("target")))))
}

/// `cargo metadata --no-deps` of the workspace at `root`.
fn metadata(root: &Path) -> Result<Value> {
    let out = Command::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .with_context(|| format!("running cargo metadata in {}", project::shown(root)))?;
    if !out.status.success() {
        bail!(
            "{}'s Cargo.toml doesn't read, so its own binary can't be planned \
             (cargo metadata says: {})",
            project::shown(root),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("reading cargo metadata's answer")
}

/// One workspace package, as a game binary to build: `None` when it is no
/// binary or depends on no launcher.
fn game_of(
    package: &Value,
    manifest: PathBuf,
    launchers: &Launchers,
    package_features: &BTreeSet<String>,
) -> Result<Option<Game>> {
    let bins: Vec<String> = arr(&package["targets"])
        .filter(|t| strs(&t["kind"]).any(|k| k == "bin"))
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    let shown = project::shown(&manifest);
    let mut found = Vec::new();
    let mut features = Vec::new();
    // Each launcher by the name the game gives it, with what it would build
    // in that the list leaves out
    let mut off_by_name = Vec::new();
    for (krate, toml, plan) in launchers {
        let Some(dep) =
            arr(&package["dependencies"]).find(|d| d["kind"].is_null() && d["name"] == *krate)
        else {
            continue;
        };
        let named = dep["rename"].as_str().unwrap_or(krate).to_string();
        let off = |f: &str| -> Option<String> {
            closure(toml, &[f.to_string()]).into_iter().find(|p| {
                package_features.contains(p) && has_feature(toml, p) && !plan.packages.contains(p)
            })
        };
        if dep["uses_default_features"].as_bool().unwrap_or(true) {
            bail!(
                "{shown}: its `{krate}` dependency takes the crate's default features, \
                 which build in every package whatever flint.toml lists. Add \
                 `default-features = false` to it (on the `[workspace.dependencies]` \
                 entry when it says `workspace = true`): `flint build` turns on the \
                 packages flint.toml lists"
            );
        }
        if let Some((pin, p)) = strs(&dep["features"]).find_map(|f| Some((f, off(f)?))) {
            bail!(
                "{shown}: its `{krate}` dependency pins `features = [\"{pin}\"]`, which \
                 builds in `{p}`, a package flint.toml leaves out. Take `{pin}` out of \
                 that list: `flint build` turns on each listed package itself"
            );
        }
        features.extend(plan.all().iter().map(|f| format!("{named}/{f}")));
        off_by_name.push((named.clone(), off));
        found.push((*krate, named));
    }
    if found.is_empty() || bins.is_empty() {
        return Ok(None);
    }
    // The game's own default features, refused like a pin when one turns
    // on a left-out package (`trees = ["flint-player/living"]`)
    let own = &package["features"];
    for f in strs(&own["default"]) {
        let mut queue = vec![f];
        let mut seen = BTreeSet::new();
        while let Some(g) = queue.pop() {
            if !seen.insert(g) {
                continue;
            }
            let Some((dep, feat)) = g.split_once('/') else {
                queue.extend(strs(&own[g]));
                continue;
            };
            let dep = dep.trim_end_matches('?');
            if let Some(p) = off_by_name
                .iter()
                .find(|(n, _)| n == dep)
                .and_then(|(_, off)| off(feat))
            {
                bail!(
                    "{shown}: its default feature `{f}` turns on `{g}`, which builds in \
                     `{p}`, a package flint.toml leaves out. Take `{f}` out of \
                     `default`, or list `{p}` in flint.toml"
                );
            }
        }
        features.push(f.to_string());
    }
    Ok(Some(Game {
        package: package["name"].as_str().unwrap_or_default().to_string(),
        manifest,
        bins,
        launchers: found,
        features,
    }))
}

/// A JSON array's items (none for anything else).
fn arr(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

/// A JSON array's strings.
fn strs(v: &Value) -> impl Iterator<Item = &str> {
    arr(v).filter_map(Value::as_str)
}

/// The packages in the build, each with why, and the names left out.
fn included(
    in_use: Option<&InUse>,
    list: Option<&flint_core::package::PackageList>,
    packages: &Packages,
) -> (Vec<Included>, Vec<String>) {
    let mut included = Vec::new();
    let mut left_out = Vec::new();
    for p in &packages.packages {
        let on = in_use.is_none_or(|u| u.is_on(&p.name));
        if !on {
            left_out.push(p.name.clone());
            continue;
        }
        let listed = list.and_then(|l| l.get(&p.name)).is_some();
        let why = if in_use.is_none() {
            "no [packages]: every package".to_string()
        } else if p.tier == Tier::Kernel {
            "the kernel".to_string()
        } else if listed {
            "listed".to_string()
        } else if p.tier == Tier::Standard {
            "standard".to_string()
        } else {
            let by: Vec<&str> = packages
                .packages
                .iter()
                .filter(|q| {
                    q.requires.contains_key(&p.name) && in_use.is_some_and(|u| u.is_on(&q.name))
                })
                .map(|q| q.name.as_str())
                .collect();
            format!("required by {}", by.join(", "))
        };
        included.push(Included {
            name: p.name.clone(),
            tier: p.tier,
            why,
        });
    }
    (included, left_out)
}

/// A crate's Cargo.toml, parsed.
fn cargo_toml(dir: &Path) -> Result<toml::Table> {
    let file = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("reading {}", project::shown(&file)))?;
    text.parse::<toml::Table>()
        .with_context(|| format!("parsing {}", project::shown(&file)))
}

fn features_of(toml: &toml::Table) -> BTreeMap<String, Vec<String>> {
    toml.get("features")
        .and_then(|f| f.as_table())
        .map(|t| {
            t.iter()
                .map(|(k, v)| {
                    let list = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    (k.clone(), list)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn has_feature(toml: &toml::Table, feature: &str) -> bool {
    feature != "default" && features_of(toml).contains_key(feature)
}

/// A crate's build options: its default features that are no package's
/// and turn on more than packages (flint-cli's `packages`, which is all
/// five, is not one).
fn build_options(toml: &toml::Table, package_features: &BTreeSet<String>) -> Vec<String> {
    let table = features_of(toml);
    let Some(default) = table.get("default") else {
        return Vec::new();
    };
    default
        .iter()
        .filter(|f| !package_features.contains(*f))
        .filter(|f| {
            let own = table.get(*f).cloned().unwrap_or_default();
            own.is_empty() || !own.iter().all(|g| package_features.contains(g))
        })
        .cloned()
        .collect()
}

/// `features` and every crate feature they turn on, by the crate's own
/// table (`living` brings `procgen`).
fn closure(toml: &toml::Table, features: &[String]) -> BTreeSet<String> {
    let table = features_of(toml);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = features.to_vec();
    while let Some(f) = queue.pop() {
        if !seen.insert(f.clone()) {
            continue;
        }
        for g in table.get(&f).into_iter().flatten() {
            if !g.contains('/') && !g.starts_with("dep:") {
                queue.push(g.clone());
            }
        }
    }
    seen
}

/// The flint-player features flint-cli turns on with `cli_features`: its
/// dependency's own `features`, and each `flint-player/<f>` its chosen
/// features name.
fn forced_player_features(cli: &toml::Table, cli_features: &[String]) -> BTreeSet<String> {
    let mut forced = BTreeSet::new();
    if let Some(dep) = cli
        .get("dependencies")
        .and_then(|d| d.get(PLAYER))
        .and_then(|d| d.get("features"))
        .and_then(|f| f.as_array())
    {
        forced.extend(dep.iter().filter_map(|f| f.as_str().map(str::to_string)));
    }
    let table = features_of(cli);
    for f in closure(cli, cli_features) {
        for g in table.get(&f).into_iter().flatten() {
            let g = g
                .strip_prefix("flint-player/")
                .or_else(|| g.strip_prefix("flint-player?/"));
            if let Some(g) = g {
                forced.insert(g.to_string());
            }
        }
    }
    forced
}

/// Where cargo writes the engine's builds: `cargo metadata`'s answer (so
/// `CARGO_TARGET_DIR` and a `.cargo/config.toml` count), else `target/`.
fn target_dir(dir: &Path) -> PathBuf {
    metadata(dir)
        .ok()
        .and_then(|v| v["target_directory"].as_str().map(PathBuf::from))
        .unwrap_or_else(|| dir.join("target"))
}

/// The cargo to run: the one running this flint (`cargo run` sets
/// `CARGO`), else the one on the path.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

impl Plan {
    /// Where `bin` lands.
    #[cfg(test)]
    fn binary(&self, bin: &str) -> PathBuf {
        self.bins()
            .into_iter()
            .find(|b| b.name == bin)
            .map(|b| b.path.clone())
            .expect("a binary the plan writes")
    }

    fn bins(&self) -> Vec<&Bin> {
        self.runs.iter().flat_map(|r| &r.bins).collect()
    }
}

// ── Building ─────────────────────────────────────────────────────────

/// Run the plan's cargo runs in order, setting aside each binary about to
/// be replaced. The set-aside copies that are still there after, deleted
/// where they can be.
fn build(plan: &Plan) -> Result<Vec<PathBuf>> {
    let Some(launch) = &plan.launch else {
        launch::remove_stale(&plan.launch_dir)?;
        return build_runs(plan);
    };
    launch::write(launch)?;
    // The launcher joins the engine's lock while cargo runs; the lock goes
    // back as it was, so a game's subtree stays clean (ADR 0137 decision 6)
    let lock = plan.engine.join("Cargo.lock");
    let before = std::fs::read(&lock).ok();
    let built = build_runs(plan);
    if let Some(before) = before {
        if std::fs::read(&lock).ok().as_ref() != Some(&before) {
            std::fs::write(&lock, before)
                .with_context(|| format!("putting {} back", project::shown(&lock)))?;
        }
    }
    built
}

fn build_runs(plan: &Plan) -> Result<Vec<PathBuf>> {
    let mut kept = Vec::new();
    for run in &plan.runs {
        let aside: Vec<(PathBuf, PathBuf)> = if cfg!(windows) {
            run.bins
                .iter()
                .map(|bin| bin.path.clone())
                .filter(|path| path.is_file())
                .map(|path| set_aside(&path).map(|old| (path, old)))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let mut cmd = Command::new(cargo());
        cmd.args(&run.args).current_dir(&run.dir);
        if output::json() {
            // stdout is the document's (#710)
            cmd.stdout(Stdio::from(std::io::stderr()));
        }
        let status = cmd
            .status()
            .with_context(|| format!("running {}", command_line(&run.args)));
        let ok = matches!(&status, Ok(s) if s.success());
        for (path, old) in aside {
            if !ok && !path.exists() {
                // Nothing written in its place: the old binary goes back
                let _ = std::fs::rename(&old, &path);
            } else if std::fs::remove_file(&old).is_err() {
                // Still running: its next start deletes it
                kept.push(old);
            }
        }
        let status = status?;
        if !status.success() {
            bail!("{} failed ({status})", command_line(&run.args));
        }
    }
    Ok(kept)
}

/// Rename `binary` aside (ADR 0136 decision 10), to `<name>.old.exe`, or
/// `<name>.old.<n>.exe` while an earlier copy still runs.
fn set_aside(binary: &Path) -> Result<PathBuf> {
    let first = flint_player::packages::set_aside_name(binary);
    let mut candidates = vec![first.clone()];
    let stem = first.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = first.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 2..100 {
        let name = match (&stem, &ext) {
            (Some(stem), Some(ext)) => format!("{stem}.{n}.{ext}"),
            (Some(stem), None) => format!("{stem}.{n}"),
            _ => break,
        };
        candidates.push(first.with_file_name(name));
    }
    for to in candidates {
        if to.exists() && std::fs::remove_file(&to).is_err() {
            continue;
        }
        std::fs::rename(binary, &to).with_context(|| {
            format!(
                "setting {} aside as {} before building over it",
                project::shown(binary),
                project::shown(&to)
            )
        })?;
        return Ok(to);
    }
    bail!(
        "{}: every set-aside name beside it is held by a running copy",
        project::shown(binary)
    )
}

// ── What it says ─────────────────────────────────────────────────────

/// `args` after `cargo`, as one would type it.
fn command_line(args: &[String]) -> String {
    let mut line = String::from("cargo");
    for a in args {
        line.push(' ');
        if a.contains(' ') {
            line.push_str(&format!("\"{a}\""));
        } else {
            line.push_str(a);
        }
    }
    line
}

fn features_line(f: &Features) -> String {
    let mut line = if f.packages.is_empty() {
        "no package features".to_string()
    } else {
        f.packages.join(", ")
    };
    if !f.options.is_empty() {
        line.push_str(&format!(" + {}", f.options.join(", ")));
    }
    line
}

fn plan_text(plan: &Plan) -> String {
    let mut out = String::new();
    out.push_str(&format!("project   {}\n", project::shown(&plan.project)));
    out.push_str(&format!(
        "engine    {} ({})\n",
        project::shown(&plan.engine),
        plan.engine_from
    ));
    let inc: Vec<String> = plan
        .included
        .iter()
        .map(|i| format!("{} ({})", i.name, i.why))
        .collect();
    out.push_str(&format!("packages  {}\n", inc.join(", ")));
    if !plan.left_out.is_empty() {
        out.push_str(&format!("left out  {}\n", plan.left_out.join(", ")));
    }
    match &plan.launch {
        Some(launch) => out.push_str(&format!(
            "launcher  {} ({}, written by this build): {PLAYER} and {CLI_BIN} with {}\n",
            launch.krate,
            project::shown(&launch.dir),
            plan.launcher.join(", ")
        )),
        None => out.push_str(&format!(
            "launcher  {PLAYER}, its stock list from these features: {}\n",
            plan.launcher.join(", ")
        )),
    }
    out.push_str(&format!("player    {}\n", features_line(&plan.player)));
    out.push_str(&format!("flint     {}\n", features_line(&plan.cli)));
    for game in &plan.games {
        let on: Vec<&str> = game.launchers.iter().map(|(k, _)| *k).collect();
        out.push_str(&format!(
            "game      {} ({}): {} on {}'s features",
            game.bins.join(", "),
            project::shown(&game.manifest),
            if game.bins.len() == 1 {
                "its own binary,"
            } else {
                "its own binaries,"
            },
            on.join(" and ")
        ));
        out.push('\n');
    }
    if let Some(why) = &plan.split {
        out.push_str(&format!("\nTwo runs: {why}.\n"));
    }
    out.push('\n');
    for run in &plan.runs {
        out.push_str(&format!(
            "{}\n  in {}\n",
            command_line(&run.args),
            project::shown(&run.dir)
        ));
    }
    out
}

fn binaries_text(plan: &Plan) -> String {
    let width = plan.bins().iter().map(|b| b.name.len()).max().unwrap_or(0);
    plan.bins()
        .iter()
        .map(|b| format!("  {:width$}  {}\n", b.name, project::shown(&b.path)))
        .collect()
}

fn outcome_text(plan: &Plan, outcome: &Outcome) -> String {
    match outcome {
        Outcome::DryRun => format!(
            "{}\nWould write (--dry-run built nothing):\n{}",
            plan_text(plan),
            binaries_text(plan)
        ),
        Outcome::Built { seconds, set_aside } => {
            let mut out = format!("Built in {seconds:.1} s:\n{}", binaries_text(plan));
            for old in set_aside {
                out.push_str(&format!(
                    "  (set aside {}: it was running, and goes at its next start)\n",
                    project::shown(old)
                ));
            }
            out
        }
        Outcome::Failed(why) => format!("Build failed: {why}\n"),
    }
}

fn answer(plan: &Plan, outcome: &Outcome) -> Answer {
    let binaries: serde_json::Map<String, Value> = plan
        .bins()
        .iter()
        .map(|b| (b.name.clone(), json!(project::shown(&b.path))))
        .collect();
    let argv = |r: &Run| {
        let mut words = vec!["cargo".to_string()];
        words.extend(r.args.iter().cloned());
        words
    };
    let mut doc = json!({
        "project": project::shown(&plan.project),
        "engine": project::shown(&plan.engine),
        "engine_from": plan.engine_from,
        "manifest": plan.manifest.as_deref().map(project::shown),
        "packages": plan.included.iter().map(|i| json!({
            "name": i.name,
            "tier": i.tier.as_str(),
            "why": i.why,
        })).collect::<Vec<_>>(),
        "left_out": plan.left_out,
        "launcher": match &plan.launch {
            Some(launch) => json!({
                "binary": PLAYER,
                "crate": launch.krate,
                "dir": project::shown(&launch.dir),
                "generated": true,
                "game_packages": launch.packages,
                "packages": plan.launcher,
            }),
            None => json!({
                "binary": PLAYER,
                "crate": format!("crates/{PLAYER}"),
                "packages": plan.launcher,
            }),
        },
        "features": {
            PLAYER: { "packages": plan.player.packages, "options": plan.player.options },
            CLI: { "packages": plan.cli.packages, "options": plan.cli.options },
        },
        "games": plan.games.iter().map(|g| json!({
            "package": g.package,
            "manifest": project::shown(&g.manifest),
            "bins": g.bins,
            "launchers": g.launchers.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            "features": g.features,
        })).collect::<Vec<_>>(),
        "split": plan.split,
        "commands": plan.runs.iter().map(|r| command_line(&r.args)).collect::<Vec<_>>(),
        "argv": plan.runs.iter().map(argv).collect::<Vec<_>>(),
        "runs": plan.runs.iter().map(|r| json!({
            "command": command_line(&r.args),
            "argv": argv(r),
            "dir": project::shown(&r.dir),
            "binaries": r.bins.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "dir": project::shown(&plan.engine),
        "binaries": binaries,
        "dry_run": matches!(outcome, Outcome::DryRun),
    });
    match outcome {
        Outcome::DryRun => {}
        Outcome::Built { seconds, set_aside } => {
            doc["built"] = json!(true);
            doc["seconds"] = json!(seconds);
            doc["set_aside"] = json!(set_aside
                .iter()
                .map(|p| project::shown(p))
                .collect::<Vec<_>>());
        }
        Outcome::Failed(why) => {
            doc["built"] = json!(false);
            doc["error"] = json!(why);
        }
    }
    Answer::new(outcome_text(plan, outcome), doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PathBuf {
        built_from()
    }

    fn args() -> BuildArgs {
        BuildArgs {
            release: false,
            target: None,
            dry_run: true,
            engine_path: None,
        }
    }

    /// A project folder with `flint.toml` holding `packages`, or none.
    fn project_with(name: &str, packages: Option<&str>) -> Project {
        let dir = std::env::temp_dir()
            .join(format!("flint-build-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = "[project]\nid = \"t\"\n".to_string();
        if let Some(p) = packages {
            text.push_str(&format!("\n[packages]\n{p}\n"));
        }
        std::fs::write(dir.join("flint.toml"), text).unwrap();
        Project::at(&dir).unwrap()
    }

    fn plan_for(packages: Option<&str>, name: &str) -> Plan {
        plan(&project_with(name, packages), engine(), "test", &args()).unwrap()
    }

    #[test]
    fn a_project_listing_only_procgen_builds_none_of_the_feature_packages() {
        let plan = plan_for(Some("procgen = \"*\""), "procgen");
        assert_eq!(plan.runs.len(), 1, "{plan:#?}");
        let line = command_line(&plan.runs[0].args);
        assert!(
            line.starts_with("cargo build -p flint-player -p flint-cli"),
            "{line}"
        );
        assert!(line.contains("--no-default-features"), "{line}");
        for left in ["living", "music", "rhythm", "/ai"] {
            assert!(!line.contains(left), "{left} in {line}");
        }
        assert!(line.contains("flint-player/procgen"), "{line}");
        assert!(line.contains("flint-player/debug-hud"), "{line}");
        assert!(line.contains("flint-cli/procgen"), "{line}");
        // The build options are kept, the `packages` bundle is not one
        assert_eq!(plan.player.options, ["sound", "gamepad", "frame-history"]);
        assert_eq!(plan.cli.options, ["sound", "gamepad", "dialogs"]);
        assert_eq!(
            plan.left_out,
            ["ai", "living", "music", "rhythm", "vehicle"]
        );
    }

    /// A scratch game (#1532): `flint.toml` listing `packages`, and a crate
    /// of its own at the root whose flint-player dependency adds `dep`
    /// after its path, with `extra` lines after the dependencies.
    fn game_with(name: &str, packages: &str, dep: &str, extra: &str) -> Project {
        let project = project_with(name, Some(packages));
        let player = engine().join("crates").join(PLAYER);
        let player = player.to_string_lossy().replace('\\', "/");
        std::fs::write(
            project.root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"scratch-game\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n\n[dependencies]\nflint-player = {{ path = \"{player}\"{dep} }}\n{extra}"
            ),
        )
        .unwrap();
        std::fs::create_dir_all(project.root.join("src")).unwrap();
        std::fs::write(project.root.join("src/main.rs"), "fn main() {}\n").unwrap();
        project
    }

    fn game_plan(name: &str, packages: &str, dep: &str, extra: &str) -> Result<Plan> {
        plan(
            &game_with(name, packages, dep, extra),
            engine(),
            "test",
            &args(),
        )
    }

    const NO_DEFAULTS: &str = ", default-features = false";

    /// #1532's first box: a game with its own binary and `procgen = "*"`
    /// gets a run naming its binary with none of the other packages.
    #[test]
    fn a_games_own_binary_with_procgen_alone_builds_none_of_the_others() {
        let extra = "[features]\ndefault = [\"hud\"]\nhud = []\n";
        let plan = game_plan("game_procgen", "procgen = \"*\"", NO_DEFAULTS, extra).unwrap();
        assert_eq!(plan.games[0].bins, ["scratch-game"], "{plan:#?}");
        let run = plan.runs.last().unwrap();
        assert_eq!(run.dir, plan.project);
        let line = command_line(&run.args);
        let head = "cargo build -p scratch-game --bin scratch-game --no-default-features";
        assert!(line.starts_with(head) && line.ends_with(",hud"), "{line}");
        for left in ["living", "music", "rhythm", "flint-cli/"] {
            assert!(!line.contains(left), "{left} in {line}");
        }
        // The stock player's features, by the same plan
        for f in plan.player.all() {
            assert!(line.contains(&format!("flint-player/{f}")), "{f}: {line}");
        }
        let bin = plan.binary("scratch-game");
        assert!(
            bin.starts_with(plan.project.join("target/debug")),
            "{bin:?}"
        );
        assert!(plan_text(&plan).contains("game      scratch-game ("));
        let doc = answer(&plan, &Outcome::DryRun).json;
        assert_eq!(doc["runs"][1]["binaries"][0], "scratch-game");
    }

    /// What would bring a left-out package back is refused naming the line:
    /// the player's defaults, a pin, a default feature of the game's own. A
    /// pin of a listed package or a build option is no pin.
    #[test]
    fn a_game_that_would_build_in_a_left_out_package_is_refused() {
        let refused = |name, dep, extra| {
            let e = game_plan(name, "procgen = \"*\"", dep, extra).unwrap_err();
            format!("{e:#}")
        };
        let e = refused("game_defaults", "", "");
        assert!(
            e.contains("Cargo.toml: its `flint-player` dependency takes"),
            "{e}"
        );
        assert!(e.contains("Add `default-features = false`"), "{e}");
        let e = refused(
            "game_pin",
            ", default-features = false, features = [\"living\"]",
            "",
        );
        assert!(
            e.contains("pins `features = [\"living\"]`, which builds in `living`"),
            "{e}"
        );
        let trees = "[features]\ndefault = [\"trees\"]\ntrees = [\"flint-player/living\"]\n";
        let e = refused("game_own", NO_DEFAULTS, trees);
        assert!(
            e.contains("default feature `trees` turns on `flint-player/living`"),
            "{e}"
        );
        let ok = ", default-features = false, features = [\"procgen\", \"audio\"]";
        assert!(game_plan("game_pin_ok", "procgen = \"*\"", ok, "").is_ok());
    }

    /// The engine checkout is no game: its own crates depend on the player.
    #[test]
    fn the_engine_checkout_has_no_game_binary() {
        let (none, f) = (toml::Table::new(), Features::default());
        let launchers = [(PLAYER, &none, &f), (CLI, &none, &f)];
        let (games, _) = games(&engine(), &engine(), &launchers, &BTreeSet::new()).unwrap();
        assert!(games.is_empty());
    }

    #[test]
    fn no_packages_table_is_every_package_as_the_default_build() {
        let plan = plan_for(None, "none");
        assert!(plan.left_out.is_empty(), "{plan:#?}");
        let mut player = plan.player.packages.clone();
        player.sort();
        assert_eq!(
            player,
            [
                "animation",
                "audio",
                "debug-hud",
                "flipbook",
                "living",
                "music",
                "particles",
                "physics",
                "procgen",
                "rhythm",
                "rig",
                "terrain",
                "vehicle"
            ]
        );
        let mut cli = plan.cli.packages.clone();
        cli.sort();
        assert_eq!(
            cli,
            [
                "ai",
                "animation",
                "audio",
                "debug-hud",
                "flipbook",
                "living",
                "music",
                "particles",
                "physics",
                "procgen",
                "rhythm",
                "rig",
                "terrain",
                "vehicle"
            ]
        );
        assert!(plan.split.is_none());
    }

    #[test]
    fn a_requirement_comes_in_and_says_who_needs_it() {
        let plan = plan_for(Some("rhythm = \"*\""), "rhythm");
        let music = plan.included.iter().find(|i| i.name == "music").unwrap();
        assert_eq!(music.why, "required by rhythm");
        assert!(plan.player.packages.contains(&"music".to_string()));
    }

    /// #1583: the CLI's `debug-hud` is its own feature, so a project that
    /// leaves the package out builds it out of both binaries in one run.
    #[test]
    fn a_project_without_the_debug_hud_leaves_it_out_of_both_in_one_run() {
        let plan = plan_for(Some("debug-hud = false"), "no_hud");
        assert_eq!(plan.runs.len(), 1, "{plan:#?}");
        assert!(plan.split.is_none(), "{plan:#?}");
        let line = command_line(&plan.runs[0].args);
        assert!(
            line.starts_with("cargo build -p flint-player -p flint-cli"),
            "{line}"
        );
        assert!(!line.contains("debug-hud"), "{line}");
        assert!(!plan.cli.packages.contains(&"debug-hud".to_string()));
    }

    #[test]
    fn release_and_target_reach_cargo_and_the_binaries_path() {
        let project = project_with("target", Some(""));
        let mut a = args();
        a.release = true;
        a.target = Some("x86_64-unknown-linux-gnu".into());
        let plan = plan(&project, engine(), "test", &a).unwrap();
        let line = command_line(&plan.runs[0].args);
        assert!(
            line.ends_with("--release --target x86_64-unknown-linux-gnu"),
            "{line}"
        );
        let player = plan.binary(PLAYER);
        assert!(
            player.ends_with("x86_64-unknown-linux-gnu/release/flint-player"),
            "{}",
            player.display()
        );
    }

    #[test]
    fn a_list_that_does_not_resolve_is_refused_naming_the_line() {
        let project = project_with("bad", Some("livng = \"*\""));
        let e = plan(&project, engine(), "test", &args()).unwrap_err();
        let e = format!("{e:#}");
        assert!(e.contains("packages.livng"), "{e}");
        assert!(e.contains("living"), "{e}");
    }

    /// #1587: a package folder newer than the launchers' features (the toy
    /// fixture dropped into a copy of the engine's manifests, the sync not
    /// run) is refused naming the sync script and the engine to run it in.
    #[test]
    fn a_package_the_launchers_dont_know_yet_names_the_sync_script() {
        let real = engine();
        let copy = std::env::temp_dir()
            .join(format!("flint-build-{}", std::process::id()))
            .join("unsynced-engine");
        let _ = std::fs::remove_dir_all(&copy);
        for name in [PLAYER, CLI] {
            let to = copy.join("crates").join(name);
            std::fs::create_dir_all(&to).unwrap();
            std::fs::copy(
                real.join("crates").join(name).join("Cargo.toml"),
                to.join("Cargo.toml"),
            )
            .unwrap();
        }
        let packages = real.join(flint_core::package::PACKAGES_DIR);
        for folder in std::fs::read_dir(&packages).unwrap() {
            let folder = folder.unwrap().path();
            let file = folder.join(flint_core::package::MANIFEST_FILE);
            if file.is_file() {
                let to = copy.join("packages").join(folder.file_name().unwrap());
                std::fs::create_dir_all(&to).unwrap();
                std::fs::copy(&file, to.join(flint_core::package::MANIFEST_FILE)).unwrap();
            }
        }
        let toy = copy.join("packages/toy");
        std::fs::create_dir_all(&toy).unwrap();
        std::fs::copy(
            real.join("tests/fixtures/packages/toy/package.toml"),
            toy.join(flint_core::package::MANIFEST_FILE),
        )
        .unwrap();

        let project = project_with("unsynced", Some("toy = \"*\""));
        let e = plan(&project, copy.clone(), "test", &args()).unwrap_err();
        let e = format!("{e:#}");
        assert!(e.contains("names `toy`, which neither flint-player"), "{e}");
        assert!(
            e.contains("Run `python3 scripts/package-sync.py` in"),
            "{e}"
        );
        assert!(e.contains(&project::shown(&copy)), "{e}");
        // A package the launchers have still plans as before
        let project = project_with("synced", Some("procgen = \"*\""));
        assert!(plan(&project, copy.clone(), "test", &args()).is_ok());
        let _ = std::fs::remove_dir_all(&copy);
    }

    #[test]
    fn the_engine_flag_must_name_a_workspace() {
        let project = project_with("flag", None);
        let e = find_engine(&project, Some(&project.root)).unwrap_err();
        assert!(
            e.to_string().contains("not a Flint engine workspace"),
            "{e}"
        );
        let (found, from) = find_engine(&project, Some(&engine())).unwrap();
        assert_eq!(found, engine());
        assert_eq!(from, "--engine-path");
    }

    /// Windows' rule the set-aside is for (ADR 0136 decision 10): a running
    /// image can't be deleted or overwritten, and can be renamed. A copy of
    /// `ping.exe` stands in for a running flint.
    #[cfg(windows)]
    #[test]
    fn a_running_binary_is_set_aside_and_a_running_old_copy_passed_over() {
        let dir = std::env::temp_dir().join(format!("flint-build-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ping = Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32/PING.EXE");
        let exe = dir.join("flint.exe");
        std::fs::copy(&ping, &exe).unwrap();
        let run = |exe: &Path| {
            Command::new(exe)
                .args(["-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut first = run(&exe);
        assert!(
            std::fs::remove_file(&exe).is_err(),
            "a running image can't be deleted"
        );
        let old = set_aside(&exe).unwrap();
        assert_eq!(old, dir.join("flint.old.exe"));
        assert!(!exe.exists());

        // The next build: flint.exe runs again, its old copy still runs
        std::fs::copy(&ping, &exe).unwrap();
        let mut second = run(&exe);
        let next = set_aside(&exe).unwrap();
        assert_eq!(next, dir.join("flint.old.2.exe"));
        for child in [&mut first, &mut second] {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Stopped, an old copy can be deleted, as its next start's sweep does
        std::fs::remove_file(&old).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
