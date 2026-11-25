//! `flint package` (#1331, ADR 0136): the engine's packages, from their
//! manifests (`packages/<name>/package.toml`, read by
//! `flint_core::package`).
//!
//! ```bash
//! flint package list            # each package: tier, version, what it owns
//! flint package list --json     # the same as one JSON document
//! flint package list --dir d    # the manifests under another folder
//! ```
//!
//! The folder is the project's engine: `engine/packages` in a game that
//! carries the engine as a subtree, `packages/` in an engine checkout, else
//! the tree this binary was built from. Nothing is compiled or built: the
//! answer is the manifests'. A manifest with a mistake is listed among the
//! problems, each naming its file and key, and the command exits 1.

use crate::output::{self, Answer};
use anyhow::Result;
use flint_core::diagnostic::Diagnostic;
use flint_core::package::{self, Kind, Package, Packages};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod owners;
mod used;

#[derive(clap::Subcommand)]
pub enum PackageCommands {
    /// Every package: its tier, version, requirements, the features that
    /// build it, and the crates, components, archetypes, scene tables,
    /// script modules and commands it owns
    List(ListArgs),
    /// The packages the project's scenes, prefabs and scripts use, from
    /// their components, scene tables, script calls and procgen specs; with
    /// `--write`, the `[packages]` table of flint.toml that lists them
    Used(used::UsedArgs),
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// The folder holding a folder a package (`<name>/package.toml`).
    /// Without it: the project's `engine/packages`, an engine checkout's
    /// `packages`, else the tree this binary was built from
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Only the packages this flint was compiled with: each whose `[build]
    /// features` the binary has (the kernel's always). A command of one
    /// left out exits 2 naming it
    #[arg(long)]
    pub built: bool,
}

pub fn run(cmd: PackageCommands) -> Result<()> {
    match cmd {
        PackageCommands::List(args) => list(args),
        PackageCommands::Used(args) => used::run(args),
    }
}

fn list(args: ListArgs) -> Result<()> {
    let dir = match args.dir {
        Some(dir) => dir,
        None => default_dir()?,
    };
    // Absolute, so each file prints from the working directory
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    let mut read = Packages::read(&dir);
    if !args.built {
        return output::emit(&answer(&read));
    }
    let of = read.packages.len();
    read.packages.retain(is_built);
    let mut answer = answer(&read);
    answer.text = built_text(&read, of);
    answer.json["built"] = json!(true);
    answer.json["features"] = json!(built_features());
    output::emit(&answer)
}

/// Whether this binary was built with `p`: every feature its manifest
/// builds it with is on (#1333).
fn is_built(p: &Package) -> bool {
    p.features
        .iter()
        .all(|f| crate::commands::absent::built_with(f))
}

/// Every package of the tree this flint was built from, each with whether
/// the binary has it (#1336: `flint examples --check` passes over an
/// example whose package is out). Where that tree has gone (a binary
/// copied to another machine), each package feature stands for the package
/// of its name, as every manifest's `[build] features` does today.
pub(crate) fn packages_built() -> Vec<(String, bool)> {
    let read = Packages::read(&built_from().join(package::PACKAGES_DIR));
    if read.packages.is_empty() {
        return crate::commands::absent::BUILT_FEATURES
            .iter()
            .map(|(f, on)| (f.to_string(), *on))
            .collect();
    }
    read.packages
        .iter()
        .map(|p| (p.name.clone(), is_built(p)))
        .collect()
}

/// The Cargo features this binary was compiled with, of those a package
/// can name.
fn built_features() -> Vec<&'static str> {
    crate::commands::absent::BUILT_FEATURES
        .iter()
        .filter(|(_, on)| *on)
        .map(|(f, _)| *f)
        .collect()
}

/// `--built`'s text: one line a package, then what it was built with.
fn built_text(read: &Packages, of: usize) -> String {
    let width = read
        .packages
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for p in &read.packages {
        out.push_str(&format!(
            "{:width$}  {:<8}  {}  {}\n",
            p.name,
            p.tier.as_str(),
            p.version,
            p.summary
        ));
    }
    out.push_str(&format!(
        "{} of {} packages built into this flint (features: {})\n",
        read.packages.len(),
        of,
        built_features().join(", ")
    ));
    for d in &read.problems {
        out.push_str(&format!("  {}\n", d.one_line()));
    }
    out
}

/// Whether this flint lacks `feature`: one of the package features it
/// can be built with, compiled out (#1333's `BUILT_FEATURES`).
pub fn lacks(feature: &str) -> bool {
    crate::commands::absent::BUILT_FEATURES
        .iter()
        .any(|(f, on)| *f == feature && !on)
}

/// The packages the project at `at` (a scene, or a project's flint.toml)
/// lists that this flint was built without (#1335): what `flint render`
/// refuses and `flint check` reports, naming `flint build`. `None` with no
/// `[packages]` table, or schemas that don't resolve (that command says
/// so itself).
pub fn unbuilt_at(at: &Path, flags: &[String]) -> Option<flint_player::packages::Unbuilt> {
    let dirs = flint_core::project::schema_dirs(Some(at), flags).ok()?;
    flint_player::packages::unbuilt(&dirs, &flint_player::packages::this_binary(), lacks)
}

/// The engine's package folder for the working directory's project.
/// The packages `[packages]` lets into the project the working directory
/// is in (#1334), held to the engine the project's schemas come from: the
/// schema resolver's own answer. `None` with no `[packages]` key, or with
/// schemas that don't resolve for another reason (that command says so).
pub fn project_in_use() -> Result<Option<package::InUse>> {
    use flint_core::project::SchemaDirsError;
    match flint_core::project::schema_dirs(None, &[] as &[&Path]) {
        Ok(dirs) => Ok(dirs.packages),
        Err(e @ SchemaDirsError::Packages { .. }) => Err(e.into()),
        Err(_) => Ok(None),
    }
}

/// The engine package folder of `project`: the one its schema directories
/// come from, else its `engine/packages` (or an engine checkout's own),
/// else the tree this binary was built from.
fn packages_dir_for(project: &flint_core::project::Project) -> Result<PathBuf> {
    let at = project.root.join(flint_core::project::MANIFEST_FILE);
    if let Ok(dirs) = flint_core::project::schema_dirs(Some(&at), &[] as &[&Path]) {
        if let Some(dir) = flint_core::project::engine_packages_for(&dirs.dirs) {
            return Ok(dir);
        }
    }
    if let Some(dir) = package::engine_packages_dir(&project.root) {
        return Ok(dir);
    }
    let built = built_from().join(package::PACKAGES_DIR);
    anyhow::ensure!(
        built.is_dir(),
        "no engine packages for {}: none beside its schemas, no engine/packages, and no {}",
        flint_core::project::shown(&project.root),
        flint_core::project::shown(&built)
    );
    Ok(built)
}

fn default_dir() -> Result<PathBuf> {
    let project = flint_core::project::Project::current()?;
    if let Some(dir) = package::engine_packages_dir(&project.root) {
        return Ok(dir);
    }
    let built = built_from().join(package::PACKAGES_DIR);
    if built.is_dir() {
        return Ok(built);
    }
    anyhow::bail!(
        "no engine packages here: neither {}/engine/packages nor {} exists (pass --dir)",
        flint_core::project::shown(&project.root),
        flint_core::project::shown(&built)
    )
}

/// The Flint tree this binary was built from.
fn built_from() -> PathBuf {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR"));
    cli.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cli.join("../.."))
}

/// A path as the user would type it from here, with forward slashes.
fn shown(path: &Path) -> String {
    let cwd = std::env::current_dir().ok();
    let rel = cwd
        .as_deref()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path);
    match flint_core::project::shown(rel) {
        here if here.is_empty() => ".".to_string(),
        shown => shown,
    }
}

fn answer(read: &Packages) -> Answer {
    Answer::new(text(read), document(read)).failed_if(read.failed())
}

/// One package as the JSON lists it.
fn package_json(p: &Package) -> Value {
    let mut v = json!({
        "name": p.name,
        "tier": p.tier.as_str(),
        "version": p.version,
        "summary": p.summary,
        "engine": p.engine,
        "api": p.api,
        "requires": p.requires,
        "features": p.features,
        "file": shown(&p.file),
    });
    for kind in Kind::ALL {
        v[kind.key()] = json!(p.provides.of(kind));
    }
    v
}

fn problem_json(d: &Diagnostic) -> Value {
    let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
    if let Some(file) = &d.file {
        v["file"] = json!(shown(file));
    }
    v
}

fn document(read: &Packages) -> Value {
    json!({
        "dir": shown(&read.dir),
        "valid": !read.failed(),
        "packages": read.packages.iter().map(package_json).collect::<Vec<_>>(),
        "problems": read.problems.iter().map(problem_json).collect::<Vec<_>>(),
    })
}

fn text(read: &Packages) -> String {
    let mut out = String::new();
    let width = read
        .packages
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(0);
    for p in &read.packages {
        out.push_str(&format!(
            "{:width$}  {:<8}  {}  {}\n",
            p.name,
            p.tier.as_str(),
            p.version,
            p.summary
        ));
        let mut rows: Vec<(&str, String)> = Vec::new();
        match (&p.api, &p.engine) {
            (Some(api), _) => rows.push(("api", api.clone())),
            (None, Some(engine)) => rows.push(("engine", engine.clone())),
            (None, None) => {}
        }
        if !p.requires.is_empty() {
            let reqs: Vec<String> = p
                .requires
                .iter()
                .map(|(name, req)| {
                    if req == "*" {
                        name.clone()
                    } else {
                        format!("{name} {req}")
                    }
                })
                .collect();
            rows.push(("requires", reqs.join(", ")));
        }
        if !p.features.is_empty() {
            rows.push(("features", p.features.join(", ")));
        }
        for kind in Kind::ALL {
            let items = p.provides.of(kind);
            if !items.is_empty() {
                let label = match kind {
                    Kind::SceneTable => "tables",
                    Kind::ScriptModule => "modules",
                    other => other.key(),
                };
                let shown_items: Vec<String> = match kind {
                    Kind::SceneTable => items.iter().map(|t| format!("[{t}]")).collect(),
                    _ => items.to_vec(),
                };
                rows.push((
                    label,
                    format!("{} ({})", shown_items.join(", "), items.len()),
                ));
            }
        }
        for (label, value) in rows {
            let head = format!("{:width$}  {label:<10} ", "");
            out.push_str(&wrapped(&head, &value));
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "{} packages in {}",
        read.packages.len(),
        shown(&read.dir)
    ));
    if read.problems.is_empty() {
        out.push('\n');
    } else {
        out.push_str(&format!(", {} problems:\n", read.problems.len()));
        for d in &read.problems {
            let mut d = d.clone();
            d.file = d.file.as_deref().map(|f| PathBuf::from(shown(f)));
            out.push_str(&format!("  {}\n", d.one_line()));
        }
    }
    out
}

/// How wide a listed row may run before it wraps.
const WRAP: usize = 100;

/// `head` then `value`, wrapped after a `, ` to [`WRAP`] columns, each
/// line after the first indented under the value.
fn wrapped(head: &str, value: &str) -> String {
    let indent = " ".repeat(head.len());
    let pieces: Vec<&str> = value.split(", ").collect();
    let mut out = String::new();
    let mut line = head.to_string();
    for (i, piece) in pieces.iter().enumerate() {
        let piece = if i + 1 < pieces.len() {
            format!("{piece},")
        } else {
            piece.to_string()
        };
        if i > 0 && line.len() + 1 + piece.len() > WRAP {
            out.push_str(&line);
            out.push('\n');
            line = indent.clone();
        } else if i > 0 {
            line.push(' ');
        }
        line.push_str(&piece);
    }
    out.push_str(&line);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_row_wraps_under_its_value() {
        let head = format!("{:6}  {:<10} ", "", "crates");
        let value = (0..30).map(|i| format!("crate-{i}")).collect::<Vec<_>>();
        let text = wrapped(&head, &value.join(", "));
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.len() <= WRAP), "{text}");
        assert!(lines[1].starts_with(&" ".repeat(head.len())));
        assert!(lines[1].as_bytes()[head.len()] != b' ');
        let joined: String = lines.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        assert_eq!(joined, format!("{}{}", head.trim_start(), value.join(", ")));
    }

    fn repo() -> Packages {
        Packages::read(&built_from().join(package::PACKAGES_DIR))
    }

    #[test]
    fn the_text_lists_each_package_and_what_it_owns() {
        let text = text(&repo());
        assert!(
            text.contains(
                "living     feature   0.1.0  Props that hold and glide, and the wooze field\n"
            ),
            "{text}"
        );
        assert!(text.contains("           requires   procgen\n"), "{text}");
        assert!(
            text.contains("           tables     [living], [wooze] (2)\n"),
            "{text}"
        );
        assert!(
            text.contains("           commands   music calibrate, music chart (2)\n"),
            "{text}"
        );
        assert!(
            text.trim_end().ends_with("7 packages in ") || text.contains("7 packages in "),
            "{text}"
        );
    }

    /// #1335's done-when, in whichever build runs it: a project listing
    /// `living`, held to this flint. CI's headless job runs `package::` on
    /// the build with no package, where it must name `flint build`.
    #[test]
    fn a_flint_without_a_listed_package_names_flint_build() {
        let dir = std::env::temp_dir()
            .join(format!("flint-unbuilt-{}", std::process::id()))
            .join("game");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let schemas = flint_core::project::shown(&built_from().join("schemas"));
        std::fs::write(
            dir.join("flint.toml"),
            format!(
                "[project]
schemas = [\"{schemas}\"]

[packages]
living = \"*\"
"
            ),
        )
        .unwrap();
        let found = unbuilt_at(&dir.join("flint.toml"), &[]);
        if cfg!(feature = "living") {
            assert_eq!(found, None);
        } else {
            let found = found.expect("a flint without living lacks it");
            assert!(found.packages.contains(&"living".to_string()), "{found:?}");
            let said = found.to_string();
            assert!(said.contains("without the `living`"), "{said}");
            assert!(said.contains("run `flint build`"), "{said}");
        }
        // With no list there is nothing to hold the build to
        std::fs::write(
            dir.join("flint.toml"),
            format!(
                "[project]
schemas = [\"{schemas}\"]
"
            ),
        )
        .unwrap();
        assert_eq!(unbuilt_at(&dir.join("flint.toml"), &[]), None);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn the_json_carries_every_list_by_its_manifest_key() {
        let doc = document(&repo());
        assert_eq!(doc["valid"], true);
        let living = doc["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "living")
            .unwrap();
        assert_eq!(living["scene_tables"], json!(["living", "wooze"]));
        assert_eq!(living["requires"], json!({ "procgen": "*" }));
        assert_eq!(living["api"], Value::Null);
    }
}
