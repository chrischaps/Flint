//! `flint package used [dir] [--write]` (#1334, ADR 0136 decision 9): the
//! packages a project's scenes, prefabs and scripts reach, read from the
//! files, and with `--write` the `[packages]` table that lists them.
//!
//! What counts as a use, each mapped to its owner by the manifests:
//!
//! - an entity's component and archetype, in a scene, a chunk or a prefab;
//! - a field it writes that a package other than the component's gives
//!   its meaning (`[provides] fields`: `music_session.chart` is rhythm's,
//!   #1581);
//! - a scene's top-level table (`[living]`, `[wooze]`);
//! - a script's call of a function in a package's script module (the
//!   script checker's walk, so a comment or a string is no call);
//! - a model or texture named by a spec (a string that names one of the
//!   project's spec files, or a path one of them is spelled by): the
//!   package whose `[provides] specs` names the file (`*.procgen.toml`,
//!   #1595).
//!
//! Then what those packages require comes along, said as `required by`. The
//! kernel is every game's and is never listed.
//!
//! A standard package is in unless a game writes `name = false`, so a list
//! of what was found still gives the game every standard package. One that
//! the scan could have seen used and didn't is named with the line that
//! leaves it out (#1536): `procgen = false   # nothing scanned uses it`.
//! It is printed, never written: writing it changes what `flint build`
//! gives a game whose table is empty today. `debug-hud` is never named:
//! its use is the F3/F4 panels, which no file says.

use super::{packages_dir_for, Packages};
use crate::output::{self, Answer};
use anyhow::{Context, Result};
use flint_core::package::{spec_ending, Kind, Package, PackageList, Tier, Wanted};
use flint_core::project::{shown, Project};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct UsedArgs {
    /// The project to scan; the one the working directory is in by default
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Write what is found to `[packages]` in the project's flint.toml:
    /// each package found as `name = "*"` (a version already written is
    /// kept), every other line kept as it is
    #[arg(long)]
    pub write: bool,
}

/// One place a package is used.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Use {
    /// The file, from the project root.
    file: String,
    /// `component`, `archetype`, `scene table`, `script module`, `spec`.
    kind: &'static str,
    item: String,
}

/// What the scan found for one package.
#[derive(Debug, Default)]
struct Found {
    uses: BTreeSet<Use>,
    /// The found packages that require it.
    required_by: BTreeSet<String>,
}

pub fn run(args: UsedArgs) -> Result<()> {
    let project = match &args.dir {
        Some(dir) => Project::at(dir)?,
        None => Project::current()?,
    };
    let root = project.root.clone();
    let packages = Packages::read(&packages_dir_for(&project)?);
    let files = crate::commands::bundle::project_files(&root)?;
    let found = scan(&root, &files, &packages);
    let written = if args.write {
        Some(write(&project, &found, &packages)?)
    } else {
        None
    };
    output::emit(&answer(&project, &packages, &found, written.as_ref()))
}

/// Every package the files reach, by name, the kernel left out.
fn scan(root: &Path, files: &[String], packages: &Packages) -> BTreeMap<String, Found> {
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let add = |kind: Kind, item: &str, file: &str, found: &mut BTreeMap<String, Found>| {
        let Some(p) = packages.owner(kind, item) else {
            return;
        };
        if p.tier == Tier::Kernel {
            return;
        }
        found.entry(p.name.clone()).or_default().uses.insert(Use {
            file: file.to_string(),
            kind: kind.noun(),
            item: item.to_string(),
        });
    };

    // The package a spec file's assets are made by: the one whose
    // `[provides] specs` names the file (#1595)
    let spec_owner = |file: &str| packages.spec_owner(file).filter(|p| p.tier != Tier::Kernel);
    // The project's spec files, by the name each states, else its stem,
    // each with its package
    let specs: BTreeMap<String, String> = files
        .iter()
        .filter_map(|f| {
            let owner = spec_owner(f)?;
            let ending = owner
                .provides
                .specs
                .iter()
                .filter_map(|s| spec_ending(s))
                .find(|e| f.ends_with(e))?;
            let text = std::fs::read_to_string(root.join(f)).ok()?;
            let stated = text
                .parse::<toml::Table>()
                .ok()
                .and_then(|t| t.get("meta")?.get("name")?.as_str().map(str::to_string));
            let name = Path::new(f).file_name()?.to_string_lossy().to_string();
            let stem = name.strip_suffix(ending)?.to_string();
            Some((stated.unwrap_or(stem), owner.name.clone()))
        })
        .collect();

    for file in files {
        let is_scene = file.ends_with(".scene.toml") || file.ends_with(".chunk.toml");
        if !(is_scene || file.ends_with(".prefab.toml")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue;
        };
        if is_scene {
            for key in doc.keys() {
                if !matches!(key.as_str(), "scene" | "prefabs" | "entities") {
                    add(Kind::SceneTable, key, file, &mut found);
                }
            }
        }
        if let Some(entities) = doc.get("entities").and_then(|e| e.as_table()) {
            for entity in entities.values().filter_map(|e| e.as_table()) {
                for (key, value) in entity {
                    match (key.as_str(), value) {
                        ("archetype", toml::Value::String(a)) => {
                            add(Kind::Archetype, a, file, &mut found)
                        }
                        (_, toml::Value::Table(fields)) => {
                            add(Kind::Component, key, file, &mut found);
                            // A field another package gives its meaning:
                            // `music_session.chart` is rhythm's (#1581)
                            for field in fields.keys() {
                                add(Kind::Field, &format!("{key}.{field}"), file, &mut found);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // A string naming a spec: a path of one, or a name one states
        let mut named = BTreeMap::new();
        strings(&toml::Value::Table(doc), &mut |s| {
            let owner = spec_owner(s)
                .map(|p| p.name.clone())
                .or_else(|| specs.get(s).cloned());
            if let Some(owner) = owner {
                named.insert(s.to_string(), owner);
            }
        });
        for (spec, owner) in named {
            found.entry(owner).or_default().uses.insert(Use {
                file: file.clone(),
                kind: Kind::Spec.noun(),
                item: spec,
            });
        }
    }

    // Scripts: the modules their calls reach
    let scripts: Vec<flint_script::check::ScriptFile> = files
        .iter()
        .filter(|f| f.ends_with(".rhai"))
        .map(|f| flint_script::check::ScriptFile::found(root.join(f)))
        .collect();
    if !scripts.is_empty() {
        let api = crate::check::scripts::engine_api().clone();
        let checked = flint_script::check::Checker::new(api).check(scripts);
        for (file, modules) in checked.modules {
            let file = from_root(root, &file);
            for module in modules {
                add(Kind::ScriptModule, &module, &file, &mut found);
            }
        }
    }

    // What the found packages need comes with them
    let mut queue: Vec<String> = found.keys().cloned().collect();
    while let Some(name) = queue.pop() {
        let Some(p) = packages.get(&name) else {
            continue;
        };
        for needed in p.requires.keys() {
            if packages.get(needed).is_some_and(|n| n.tier == Tier::Kernel) {
                continue;
            }
            let entry = found.entry(needed.clone()).or_default();
            let fresh = entry.uses.is_empty() && entry.required_by.is_empty();
            entry.required_by.insert(name.clone());
            if fresh {
                queue.push(needed.clone());
            }
        }
    }
    found
}

/// The comment a line that leaves a package out ends with.
const NOTHING_USES_IT: &str = "# nothing scanned uses it";

/// Whether the scan can see `p` used: it provides a component, archetype,
/// scene table, script module, spec file (procgen's, used by the specs
/// a file names) or a field of another's component. A package of crates and commands alone (`debug-hud`,
/// the F3/F4 panels) is used where no file says so.
fn seen_by_scan(p: &Package) -> bool {
    [
        Kind::Component,
        Kind::Archetype,
        Kind::SceneTable,
        Kind::ScriptModule,
        Kind::Spec,
        Kind::Field,
    ]
    .into_iter()
    .any(|k| !p.provides.of(k).is_empty())
}

/// The standard packages nothing scanned uses that the scan could have
/// seen used, in package order: each can be left out with `name = false`.
/// Passed over: one with no build switch (it can't be dropped yet), one
/// `[packages]` already names (set to `false`, or listed with a version,
/// which `unused` reports), and one a listed package requires, since
/// `false` there is a `bad-manifest`.
fn left_out_lines(
    packages: &Packages,
    found: &BTreeMap<String, Found>,
    listed: Option<&PackageList>,
) -> Vec<String> {
    // What the packages listed with a version bring, as the resolver does
    let mut brought: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = listed
        .map(|l| {
            l.entries
                .iter()
                .filter(|e| matches!(e.wanted, Wanted::Version(_)))
                .map(|e| e.name.clone())
                .collect()
        })
        .unwrap_or_default();
    while let Some(name) = queue.pop() {
        if let Some(p) = packages.get(&name) {
            for needed in p.requires.keys() {
                if brought.insert(needed.clone()) {
                    queue.push(needed.clone());
                }
            }
        }
    }
    packages
        .packages
        .iter()
        .filter(|p| p.tier == Tier::Standard && !p.features.is_empty() && seen_by_scan(p))
        .filter(|p| !found.contains_key(&p.name) && !brought.contains(&p.name))
        .filter(|p| listed.and_then(|l| l.get(&p.name)).is_none())
        .map(|p| p.name.clone())
        .collect()
}

/// `procgen = false   # nothing scanned uses it`
fn left_out_line(name: &str) -> String {
    format!("{name} = false   {NOTHING_USES_IT}")
}

/// Every string in `value`, at any depth.
fn strings(value: &toml::Value, f: &mut impl FnMut(&str)) {
    match value {
        toml::Value::String(s) => f(s),
        toml::Value::Array(items) => items.iter().for_each(|v| strings(v, f)),
        toml::Value::Table(t) => t.values().for_each(|v| strings(v, f)),
        _ => {}
    }
}

/// `path` from the working directory where it lies under it, with `/`.
fn here(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(shown))
        .unwrap_or_else(|| shown(path))
}

/// `path` from `root`, with `/`.
fn from_root(root: &Path, path: &Path) -> String {
    shown(path.strip_prefix(root).unwrap_or(path))
}

/// The `[packages]` lines for what was found, in package order.
fn lines(packages: &Packages, found: &BTreeMap<String, Found>) -> Vec<String> {
    packages
        .packages
        .iter()
        .filter(|p| found.contains_key(&p.name))
        .map(|p| format!("{} = \"*\"", p.name))
        .collect()
}

/// Write the found packages into `[packages]` of the project's flint.toml,
/// keeping every line already there. Returns the file.
fn write(
    project: &Project,
    found: &BTreeMap<String, Found>,
    packages: &Packages,
) -> Result<PathBuf> {
    let file = project.manifest_path();
    let text = if file.is_file() {
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", shown(&file)))?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not TOML", shown(&file)))?;
    if !doc.contains_key("packages") {
        doc["packages"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let table = doc["packages"]
        .as_table_mut()
        .with_context(|| format!("{}: `packages` is not a table", shown(&file)))?;
    for p in &packages.packages {
        if found.contains_key(&p.name) && !table.contains_key(&p.name) {
            table.insert(&p.name, toml_edit::value("*"));
        }
    }
    std::fs::write(&file, doc.to_string()).with_context(|| format!("writing {}", shown(&file)))?;
    Ok(file)
}

fn answer(
    project: &Project,
    packages: &Packages,
    found: &BTreeMap<String, Found>,
    written: Option<&PathBuf>,
) -> Answer {
    let listed = project.packages.as_ref();
    let mut text = String::new();
    let mut json_packages = Vec::new();
    let width = found.keys().map(String::len).max().unwrap_or(0);
    for p in packages
        .packages
        .iter()
        .filter(|p| found.contains_key(&p.name))
    {
        let f = &found[&p.name];
        // One row a kind: its items, then the files they are in
        let mut by_kind: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
        for u in &f.uses {
            let (items, files) = by_kind.entry(u.kind).or_default();
            items.insert(u.item.as_str());
            files.insert(u.file.as_str());
        }
        let mut rows: Vec<String> = Vec::new();
        if !f.required_by.is_empty() {
            let by: Vec<&str> = f.required_by.iter().map(String::as_str).collect();
            rows.push(format!("required by {}", by.join(", ")));
        }
        for (kind, (items, files)) in &by_kind {
            let items: Vec<&str> = items.iter().copied().collect();
            let first = files.iter().next().copied().unwrap_or_default();
            let more = match files.len() {
                1 => String::new(),
                n => format!(" and {} more", n - 1),
            };
            let kinds = if items.len() == 1 {
                kind.to_string()
            } else {
                format!("{kind}s")
            };
            rows.push(format!("{kinds} {} in {first}{more}", items.join(", ")));
        }
        let off = listed.is_some_and(|l| l.is_off(&p.name));
        if off {
            rows.push("flint.toml sets it to false".into());
        }
        let indent = " ".repeat(width + 12);
        text.push_str(&format!(
            "{:width$}  {:<8}  {}\n",
            p.name,
            p.tier.as_str(),
            rows.join(&format!("\n{indent}"))
        ));
        json_packages.push(json!({
            "name": p.name,
            "tier": p.tier.as_str(),
            "uses": f.uses.iter().map(|u| json!({
                "file": u.file, "kind": u.kind, "item": u.item,
            })).collect::<Vec<_>>(),
            "required_by": f.required_by,
            "set_to_false": off,
        }));
    }
    let lines = lines(packages, found);
    if found.is_empty() {
        text.push_str("no package but the kernel is used\n");
    }
    // Listed with a version and found nowhere
    let unused: Vec<String> = listed
        .map(|l| {
            l.entries
                .iter()
                .filter(|e| {
                    matches!(e.wanted, flint_core::package::Wanted::Version(_))
                        && !found.contains_key(&e.name)
                })
                .map(|e| e.name.clone())
                .collect()
        })
        .unwrap_or_default();
    if !unused.is_empty() {
        text.push_str(&format!(
            "listed in [packages] and used by nothing scanned: {}\n",
            unused.join(", ")
        ));
    }
    let leave_out = left_out_lines(packages, found, listed);
    // `--write` never writes one: it would change what `flint build` gives
    // a game whose table is empty today
    const YOURS: &str = "(a `= false` line is yours to add: `--write` never writes one, since \
                         it changes what `flint build` gives the game)\n";
    match written {
        Some(file) => {
            text.push_str(&format!("wrote [packages] in {}\n", here(file)));
            if !leave_out.is_empty() {
                text.push_str("\nnot written, under [packages]:\n");
                for name in &leave_out {
                    text.push_str(&left_out_line(name));
                    text.push('\n');
                }
                text.push('\n');
                text.push_str(YOURS);
            }
        }
        None => {
            text.push_str("\n[packages]\n");
            for line in &lines {
                text.push_str(line);
                text.push('\n');
            }
            for name in &leave_out {
                text.push_str(&left_out_line(name));
                text.push('\n');
            }
            text.push('\n');
            match (lines.is_empty(), leave_out.is_empty()) {
                (_, true) => text.push_str("(`--write` adds these to flint.toml)\n"),
                (false, false) => {
                    text.push_str("(`--write` adds the `\"*\"` lines to flint.toml)\n");
                    text.push_str(YOURS);
                }
                (true, false) => text.push_str(YOURS),
            }
        }
    }
    let json: Value = json!({
        "root": here(&project.root),
        "packages": json_packages,
        "lines": lines,
        "leave_out": leave_out.iter().map(|name| json!({
            "name": name,
            "line": format!("{name} = false"),
        })).collect::<Vec<_>>(),
        "unused": unused,
        "written": written.map(|f| here(f)),
    });
    Answer::new(text, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_packages() -> Packages {
        Packages::read(&super::super::built_from().join("packages"))
    }

    #[test]
    fn the_woozy_grove_uses_living_and_procgen() {
        let root = super::super::built_from().join("tests/fixtures/scenes");
        let files = vec![
            "woozy_grove/woozy_grove.scene.toml".to_string(),
            "woozy_grove/specs/grove_oak.procgen.toml".to_string(),
        ];
        let found = scan(&root, &files, &engine_packages());
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            // Its walker is the first-person package's (#1618)
            ["first-person", "living", "physics", "procgen"]
        );
        let living = &found["living"];
        assert!(living
            .uses
            .iter()
            .any(|u| u.kind == "component" && u.item == "living_prop"));
        assert!(living
            .uses
            .iter()
            .any(|u| u.kind == "scene table" && u.item == "wooze"));
        assert!(found["procgen"].required_by.contains("living"));
        assert_eq!(
            lines(&engine_packages(), &found),
            [
                "first-person = \"*\"",
                "physics = \"*\"",
                "procgen = \"*\"",
                "living = \"*\""
            ]
        );
        // Procgen is used, so it isn't named to leave out; nothing scanned
        // animates (#1620) or emits (#1622), so animation, flipbook and
        // particles are, and the grove has no terrain (#1623)
        assert_eq!(
            left_out_lines(&engine_packages(), &found, None),
            ["animation", "flipbook", "particles", "terrain"]
        );
    }

    /// #1536: a scene that names no procgen spec gives `procgen = false`
    /// as a line to add, and never `debug-hud`, which no file can show used.
    #[test]
    fn a_scene_naming_no_spec_names_procgen_to_leave_out_and_never_debug_hud() {
        let packages = engine_packages();
        let root = super::super::built_from().join("tests/fixtures/scenes");
        let files = vec!["pbr_test.scene.toml".to_string()];
        let found = scan(&root, &files, &packages);
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(
            left_out_lines(&packages, &found, None),
            [
                "animation",
                "first-person",
                "flipbook",
                "particles",
                "physics",
                "procgen",
                "terrain"
            ]
        );
        assert_eq!(
            left_out_line("procgen"),
            "procgen = false   # nothing scanned uses it"
        );
        let debug_hud = packages.get("debug-hud").expect("the engine has debug-hud");
        assert_eq!(debug_hud.tier, Tier::Standard);
        assert!(!seen_by_scan(debug_hud));

        // A line [packages] already has, or a listed package that needs
        // procgen, keeps it from being named; particles, which nothing
        // there needs, is named still (#1622)
        let list = |lines: &[(&str, Wanted)]| PackageList {
            file: PathBuf::from("flint.toml"),
            entries: lines
                .iter()
                .map(|(name, wanted)| flint_core::package::Listed {
                    name: name.to_string(),
                    wanted: wanted.clone(),
                })
                .collect(),
        };
        let any = || Wanted::Version("*".into());
        for listed in [
            list(&[("procgen", Wanted::Off)]),
            list(&[("procgen", any())]),
            list(&[("ai", any())]),
            list(&[("living", any())]),
        ] {
            // Procgen isn't named once listed, or needed by what is
            assert!(
                !left_out_lines(&packages, &found, Some(&listed))
                    .iter()
                    .any(|p| p == "procgen"),
                "{listed:?}"
            );
        }
        // A listed package that needs nothing standard leaves it named
        assert_eq!(
            left_out_lines(&packages, &found, Some(&list(&[("music", any())]))),
            [
                "animation",
                "first-person",
                "flipbook",
                "particles",
                "physics",
                "procgen",
                "terrain"
            ]
        );
    }
}
