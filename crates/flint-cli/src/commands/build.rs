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
//! one run builds, and flint-cli turns on flint-player's `debug-hud` and
//! `frame-history` (its windows draw the panels). A project that drops one
//! of those (`debug-hud = false`) would get it back in a shared run, so its
//! player is built alone first, then the CLI.
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
    /// The output binaries' names (`flint-player`, `flint`).
    bins: Vec<&'static str>,
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
    /// Each crate's package features, then its build options.
    player: Features,
    cli: Features,
    runs: Vec<Run>,
    /// Why the player builds in a run of its own, when it does.
    split: Option<String>,
    out_dir: PathBuf,
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
fn find_engine(project: &Project, flag: Option<&Path>) -> Result<(PathBuf, &'static str)> {
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
fn normal(path: &Path) -> PathBuf {
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
    let packages = Packages::read(&engine.join(flint_core::package::PACKAGES_DIR));
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
    for inc in &included {
        let p = packages
            .get(&inc.name)
            .expect("included from the manifests");
        for f in &p.features {
            let on_player = has_feature(&player_toml, f);
            let on_cli = has_feature(&cli_toml, f);
            if !on_player && !on_cli {
                bail!(
                    "{}'s [build] features names `{f}`, which neither flint-player nor \
                     flint-cli has",
                    project::shown(&p.file)
                );
            }
            if on_player && !player.packages.contains(f) {
                player.packages.push(f.clone());
            }
            if on_cli && !cli.packages.contains(f) {
                cli.packages.push(f.clone());
            }
        }
    }
    player.options = build_options(&player_toml, &package_features);
    cli.options = build_options(&cli_toml, &package_features);

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
    let words = |crates: &[(&str, &str)], features: Vec<String>| -> Vec<String> {
        let mut run = vec!["build".to_string()];
        for (krate, _) in crates {
            run.push("-p".into());
            run.push(krate.to_string());
        }
        for (_, bin) in crates {
            run.push("--bin".into());
            run.push(bin.to_string());
        }
        run.push("--no-default-features".into());
        if !features.is_empty() {
            run.push("--features".into());
            run.push(features.join(","));
        }
        run
    };
    let mut runs = Vec::new();
    if split.is_some() {
        let mut a = words(&[(PLAYER, PLAYER)], qualified(PLAYER, &player));
        tail(&mut a);
        runs.push(Run {
            args: a,
            bins: vec![PLAYER],
        });
        let mut b = words(&[(CLI, CLI_BIN)], qualified(CLI, &cli));
        tail(&mut b);
        runs.push(Run {
            args: b,
            bins: vec![CLI_BIN],
        });
    } else {
        let mut features = qualified(PLAYER, &player);
        features.extend(qualified(CLI, &cli));
        let mut a = words(&[(PLAYER, PLAYER), (CLI, CLI_BIN)], features);
        tail(&mut a);
        runs.push(Run {
            args: a,
            bins: vec![PLAYER, CLI_BIN],
        });
    }

    let mut out_dir = target_dir(&engine);
    if let Some(t) = &args.target {
        out_dir.push(t);
    }
    out_dir.push(if args.release { "release" } else { "debug" });

    Ok(Plan {
        project: project.root.clone(),
        engine,
        engine_from,
        manifest: in_use.map(|u| u.manifest),
        included,
        left_out,
        player,
        cli,
        runs,
        split,
        out_dir,
    })
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
fn target_dir(engine: &Path) -> PathBuf {
    let asked = Command::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(engine)
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = asked {
        if out.status.success() {
            if let Some(dir) = serde_json::from_slice::<Value>(&out.stdout)
                .ok()
                .and_then(|v| v["target_directory"].as_str().map(PathBuf::from))
            {
                return dir;
            }
        }
    }
    engine.join("target")
}

/// The cargo to run: the one running this flint (`cargo run` sets
/// `CARGO`), else the one on the path.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

impl Plan {
    /// Where `bin` lands.
    fn binary(&self, bin: &str) -> PathBuf {
        let windows = match &self.runs_target() {
            Some(t) => t.contains("windows"),
            None => cfg!(windows),
        };
        self.out_dir
            .join(format!("{bin}{}", if windows { ".exe" } else { "" }))
    }

    fn runs_target(&self) -> Option<String> {
        let args = &self.runs.first()?.args;
        let at = args.iter().position(|a| a == "--target")?;
        args.get(at + 1).cloned()
    }

    fn bins(&self) -> Vec<&'static str> {
        self.runs.iter().flat_map(|r| r.bins.clone()).collect()
    }
}

// ── Building ─────────────────────────────────────────────────────────

/// Run the plan's cargo runs in order, setting aside each binary about to
/// be replaced. The set-aside copies that are still there after, deleted
/// where they can be.
fn build(plan: &Plan) -> Result<Vec<PathBuf>> {
    let mut kept = Vec::new();
    for run in &plan.runs {
        let aside: Vec<(PathBuf, PathBuf)> = if cfg!(windows) {
            run.bins
                .iter()
                .map(|bin| plan.binary(bin))
                .filter(|path| path.is_file())
                .map(|path| set_aside(&path).map(|old| (path, old)))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let mut cmd = Command::new(cargo());
        cmd.args(&run.args).current_dir(&plan.engine);
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
    out.push_str(&format!("player    {}\n", features_line(&plan.player)));
    out.push_str(&format!("flint     {}\n", features_line(&plan.cli)));
    if let Some(why) = &plan.split {
        out.push_str(&format!("\nTwo runs: {why}.\n"));
    }
    out.push('\n');
    for run in &plan.runs {
        out.push_str(&format!("{}\n", command_line(&run.args)));
    }
    out.push_str(&format!("  in {}\n", project::shown(&plan.engine)));
    out
}

fn binaries_text(plan: &Plan) -> String {
    let width = plan.bins().iter().map(|b| b.len()).max().unwrap_or(0);
    plan.bins()
        .iter()
        .map(|b| format!("  {b:width$}  {}\n", project::shown(&plan.binary(b))))
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
        .map(|b| (b.to_string(), json!(project::shown(&plan.binary(b)))))
        .collect();
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
        "features": {
            PLAYER: { "packages": plan.player.packages, "options": plan.player.options },
            CLI: { "packages": plan.cli.packages, "options": plan.cli.options },
        },
        "split": plan.split,
        "commands": plan.runs.iter().map(|r| command_line(&r.args)).collect::<Vec<_>>(),
        "argv": plan.runs.iter().map(|r| {
            let mut words = vec!["cargo".to_string()];
            words.extend(r.args.iter().cloned());
            words
        }).collect::<Vec<_>>(),
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
        assert_eq!(plan.player.options, ["audio", "gamepad", "frame-history"]);
        assert_eq!(plan.cli.options, ["audio", "gamepad", "dialogs"]);
        assert_eq!(plan.left_out, ["ai", "living", "music", "rhythm"]);
    }

    #[test]
    fn no_packages_table_is_every_package_as_the_default_build() {
        let plan = plan_for(None, "none");
        assert!(plan.left_out.is_empty(), "{plan:#?}");
        let mut player = plan.player.packages.clone();
        player.sort();
        assert_eq!(
            player,
            ["debug-hud", "living", "music", "procgen", "rhythm"]
        );
        let mut cli = plan.cli.packages.clone();
        cli.sort();
        assert_eq!(cli, ["ai", "living", "music", "procgen", "rhythm"]);
        assert!(plan.split.is_none());
    }

    #[test]
    fn a_requirement_comes_in_and_says_who_needs_it() {
        let plan = plan_for(Some("rhythm = \"*\""), "rhythm");
        let music = plan.included.iter().find(|i| i.name == "music").unwrap();
        assert_eq!(music.why, "required by rhythm");
        assert!(plan.player.packages.contains(&"music".to_string()));
    }

    #[test]
    fn a_player_without_the_debug_hud_is_built_in_a_run_of_its_own() {
        let plan = plan_for(Some("debug-hud = false"), "no_hud");
        assert_eq!(plan.runs.len(), 2, "{plan:#?}");
        let player = command_line(&plan.runs[0].args);
        assert!(
            player.starts_with("cargo build -p flint-player --bin flint-player"),
            "{player}"
        );
        assert!(!player.contains("debug-hud"), "{player}");
        assert!(!player.contains("flint-cli"), "{player}");
        let cli = command_line(&plan.runs[1].args);
        assert!(
            cli.starts_with("cargo build -p flint-cli --bin flint"),
            "{cli}"
        );
        assert!(plan.split.as_deref().unwrap().contains("`debug-hud`"));
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
