//! The completeness test (#1331, ADR 0136 decision 1): every schema file,
//! script module, `flint` command and workspace crate of the tree belongs
//! to exactly one package, and every item a manifest lists is there.
//!
//! What the tree holds is read from the tree itself: the engine's schema
//! files (`schemas/`, less the examples' layer, and each package's
//! `packages/<name>/schemas/`, #1332), the root `Cargo.toml`'s
//! members, the script API the player declares (`flint_player::
//! script_system()`, every module flint-cli's build has, which is all of
//! them: it turns every player feature on), and the `flint` command tree
//! (`Cli::command()`). A failure names the file, crate, module or command
//! and the manifest key to put it under.
//!
//! In a lib test because the command tree is the crate's private `Cli`.
//! CI's headless job runs it (`cargo test -p flint-cli --no-default-features
//! --lib package::`).

use flint_core::package::{Kind, Packages, MANIFEST_FILE, PACKAGES_DIR};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The schema folder no engine package owns, and why.
const NOT_A_PACKAGE: &[(&str, &str)] = &[(
    "examples",
    "the examples' layer (ADR 0136 decision 7, tier game): the examples' project lists the engine's packages like a game, and its own manifest comes with Phase 2's game packages",
)];

/// Workspace members no package owns, and why.
const NO_PACKAGE_CRATES: &[(&str, &str)] = &[(
    "flint-arch-analyzer",
    "repo tooling no game links (ADR 0136 decision 7)",
)];

/// A `[build] features` name that is neither flint-player's nor
/// flint-cli's yet, and the ticket that adds it. Once it is there the
/// entry must go.
const PENDING_FEATURES: &[(&str, &str)] = &[];

/// One schema file and what it declares.
#[derive(Debug, Clone)]
pub struct SchemaFile {
    /// Spelled from the repo root: `schemas/components/light.toml`.
    pub path: String,
    pub items: Vec<(Kind, String)>,
}

/// A workspace member: its folder (from the root) and its name.
#[derive(Debug, Clone)]
pub struct Member {
    pub manifest: String,
    pub name: String,
}

/// A `flint` command: its words below `flint`, and its subcommands'.
#[derive(Debug, Clone)]
pub struct Command {
    pub path: String,
    pub children: Vec<Command>,
}

/// Everything the tree holds that a package must own.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub schemas: Vec<SchemaFile>,
    pub crates: Vec<Member>,
    pub script_modules: Vec<String>,
    pub commands: Vec<Command>,
}

fn repo() -> PathBuf {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR"));
    cli.parent().and_then(Path::parent).unwrap().to_path_buf()
}

/// Every `.toml` under `schemas` (spelled `prefix/...`), each with the
/// components, archetypes and scene tables it declares. A folder of
/// [`NOT_A_PACKAGE`] at the top is skipped.
pub fn schema_files(schemas: &Path, prefix: &str) -> Vec<SchemaFile> {
    fn walk(dir: &Path, at: &str, top: bool, out: &mut Vec<SchemaFile>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let spelled = format!("{at}/{name}");
            if path.is_dir() {
                if top && NOT_A_PACKAGE.iter().any(|(n, _)| *n == name) {
                    continue;
                }
                walk(&path, &spelled, false, out);
            } else if name.ends_with(".toml") {
                let text = std::fs::read_to_string(&path).unwrap();
                let table: toml::Table = text
                    .parse()
                    .unwrap_or_else(|e| panic!("{spelled}: not TOML: {e}"));
                let mut items = Vec::new();
                for (key, kind) in [
                    ("component", Kind::Component),
                    ("archetype", Kind::Archetype),
                    ("table", Kind::SceneTable),
                ] {
                    if let Some(toml::Value::Table(t)) = table.get(key) {
                        items.extend(t.keys().map(|n| (kind, n.clone())));
                    }
                }
                out.push(SchemaFile {
                    path: spelled,
                    items,
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(schemas, prefix, true, &mut out);
    out
}

/// The root manifest's `[workspace] members`, each with its package name.
pub fn workspace_crates(root: &Path) -> Vec<Member> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let manifest: toml::Table = text.parse().unwrap();
    let members = manifest["workspace"]["members"].as_array().unwrap();
    members
        .iter()
        .map(|m| {
            let folder = m.as_str().unwrap();
            let file = format!("{folder}/Cargo.toml");
            let text =
                std::fs::read_to_string(root.join(&file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let crate_toml: toml::Table = text.parse().unwrap();
            Member {
                manifest: file,
                name: crate_toml["package"]["name"].as_str().unwrap().to_string(),
            }
        })
        .collect()
}

/// Every module the script API declares, in declaration order.
pub fn script_modules() -> Vec<String> {
    let listing = flint_player::script_system().api_listing();
    let mut out: Vec<String> = Vec::new();
    for entry in &listing.declared {
        if !out.iter().any(|m| m == entry.module) {
            out.push(entry.module.to_string());
        }
    }
    out
}

/// The `flint` command tree, hidden commands included, clap's `help` not.
pub fn commands() -> Vec<Command> {
    fn of(cmd: &clap::Command, at: &str) -> Vec<Command> {
        cmd.get_subcommands()
            .filter(|c| c.get_name() != "help")
            .map(|c| {
                let path = if at.is_empty() {
                    c.get_name().to_string()
                } else {
                    format!("{at} {}", c.get_name())
                };
                Command {
                    children: of(c, &path),
                    path,
                }
            })
            .collect()
    }
    use clap::CommandFactory;
    of(&crate::Cli::command(), "")
}

impl Inventory {
    pub fn of_repo() -> Inventory {
        let root = repo();
        let mut schemas = schema_files(&root.join("schemas"), "schemas");
        for (package, dir) in flint_core::project::package_schema_dirs(&root.join("schemas")) {
            schemas.extend(schema_files(
                &dir,
                &format!("{PACKAGES_DIR}/{package}/schemas"),
            ));
        }
        Inventory {
            schemas,
            crates: workspace_crates(&root),
            script_modules: script_modules(),
            commands: commands(),
        }
    }
}

/// The package whose folder a schema file is in: `packages/<name>/schemas/..`
/// is `<name>`'s, the engine's own `schemas/` the kernel's.
fn folder_of(path: &str) -> &str {
    path.strip_prefix(&format!("{PACKAGES_DIR}/"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(KERNEL)
}

/// The schema folder of a package, as the tree spells it.
fn schemas_folder(package: &str) -> String {
    if package == KERNEL {
        "schemas/".to_string()
    } else {
        format!("{PACKAGES_DIR}/{package}/schemas/")
    }
}

/// The kernel's package, whose schemas are the engine's `schemas/`.
const KERNEL: &str = "engine";

/// Where an item goes: `[provides] components in one packages/*/package.toml`.
fn put_it(kind: Kind) -> String {
    format!(
        "list it under [provides] {} in one {PACKAGES_DIR}/<name>/{MANIFEST_FILE}",
        kind.key()
    )
}

/// Every way the tree and the manifests disagree, one line each.
pub fn problems(packages: &Packages, inv: &Inventory) -> Vec<String> {
    let mut out = Vec::new();
    let owners = |kind: Kind, item: &str| -> Vec<&str> {
        packages
            .packages
            .iter()
            .filter(|p| p.provides.of(kind).iter().any(|i| i == item))
            .map(|p| p.name.as_str())
            .collect()
    };
    let one_owner = |out: &mut Vec<String>, kind: Kind, item: &str, place: &str| {
        let found = owners(kind, item);
        match found.len() {
            0 => out.push(format!(
                "{place}: the {} `{item}` has no package: {}",
                kind.noun(),
                put_it(kind)
            )),
            1 => {}
            _ => out.push(format!(
                "{place}: the {} `{item}` is provided by {}: an item has one owner",
                kind.noun(),
                found.join(" and ")
            )),
        }
    };

    // Schema files: each declares something, each item one owner, and one
    // file is one package's
    let mut declared: BTreeMap<Kind, BTreeSet<&str>> = BTreeMap::new();
    for file in &inv.schemas {
        if file.items.is_empty() {
            out.push(format!(
                "{}: declares no [component.x], [archetype.x] or [table.x], so no package can own it",
                file.path
            ));
        }
        let mut file_owners: BTreeSet<&str> = BTreeSet::new();
        for (kind, item) in &file.items {
            declared.entry(*kind).or_default().insert(item);
            one_owner(&mut out, *kind, item, &file.path);
            file_owners.extend(owners(*kind, item));
        }
        if file_owners.len() > 1 {
            out.push(format!(
                "{}: its items belong to {}: a schema file is one package's",
                file.path,
                file_owners.into_iter().collect::<Vec<_>>().join(" and ")
            ));
        } else if let Some(owner) = file_owners.first() {
            // In its package's folder (#1332): what a game leaves out has
            // to be a folder it can leave out
            let folder = folder_of(&file.path);
            if folder != *owner {
                out.push(format!(
                    "{}: its items are {owner}'s, and the file is in {}: move it to {}",
                    file.path,
                    schemas_folder(folder),
                    schemas_folder(owner),
                ));
            }
        }
    }

    // Crates
    let mut crate_names: BTreeSet<&str> = BTreeSet::new();
    for member in &inv.crates {
        crate_names.insert(&member.name);
        if NO_PACKAGE_CRATES.iter().any(|(n, _)| *n == member.name) {
            if !owners(Kind::Crate, &member.name).is_empty() {
                out.push(format!(
                    "{}: `{}` is listed as no package's crate, and a manifest provides it",
                    member.manifest, member.name
                ));
            }
            continue;
        }
        one_owner(&mut out, Kind::Crate, &member.name, &member.manifest);
    }

    // Script modules
    for module in &inv.script_modules {
        one_owner(&mut out, Kind::ScriptModule, module, "the script API");
    }

    // Commands: a listed path owns everything below it
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    fn walk<'a>(
        cmd: &'a Command,
        claimed_by: Option<(&'a str, String)>,
        owners: &dyn Fn(Kind, &str) -> Vec<&'a str>,
        paths: &mut BTreeSet<&'a str>,
        out: &mut Vec<String>,
    ) {
        paths.insert(&cmd.path);
        let found = owners(Kind::Command, &cmd.path);
        let place = format!("`flint {}`", cmd.path);
        if found.len() > 1 {
            out.push(format!(
                "{place}: provided by {}: a command has one owner",
                found.join(" and ")
            ));
        }
        let claimed_by = match (&claimed_by, found.first()) {
            (Some((pkg, at)), Some(own)) => {
                out.push(format!(
                    "{place}: provided by `{own}`, and `flint {at}` above it by `{pkg}`: a command has one owner"
                ));
                claimed_by
            }
            (Some(_), None) => claimed_by,
            (None, Some(own)) => Some((*own, cmd.path.clone())),
            (None, None) => None,
        };
        if claimed_by.is_none() && cmd.children.is_empty() {
            out.push(format!(
                "{place}: has no package: {}",
                put_it(Kind::Command)
            ));
        }
        for child in &cmd.children {
            walk(child, claimed_by.clone(), owners, paths, out);
        }
    }
    for cmd in &inv.commands {
        walk(cmd, None, &owners, &mut paths, &mut out);
    }

    // What a manifest lists that the tree lacks
    let modules: BTreeSet<&str> = inv.script_modules.iter().map(String::as_str).collect();
    for p in &packages.packages {
        let file = flint_core::project::shown(&p.file);
        for kind in Kind::ALL {
            let there: &dyn Fn(&str) -> bool = match kind {
                Kind::Crate => &|i| crate_names.contains(i),
                Kind::ScriptModule => &|i| modules.contains(i),
                Kind::Command => &|i| paths.contains(i),
                _ => &|i| declared.get(&kind).is_some_and(|s| s.contains(i)),
            };
            for item in p.provides.of(kind) {
                // The script API is this build's: a package the binary was
                // built without has no modules in it to find (#1333)
                let unbuilt = kind == Kind::ScriptModule
                    && !p
                        .features
                        .iter()
                        .all(|f| crate::commands::absent::built_with(f));
                if !unbuilt && !there(item) {
                    out.push(format!(
                        "{file}: provides.{} lists `{item}`, and the tree has no {} of that name",
                        kind.key(),
                        kind.noun()
                    ));
                }
            }
        }
    }
    out
}

/// The `[features]` names of a crate's manifest, by its folder.
fn features_of(krate: &str) -> BTreeMap<String, Vec<String>> {
    let file = repo().join("crates").join(krate).join("Cargo.toml");
    let manifest: toml::Table = std::fs::read_to_string(file).unwrap().parse().unwrap();
    manifest
        .get("features")
        .and_then(|f| f.as_table())
        .map(|t| {
            t.iter()
                .map(|(k, v)| {
                    let on: Vec<String> = v
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_string())
                        .collect();
                    (k.clone(), on)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A fresh folder of its own under the system temp folder (a test project
/// under the engine's `target/` would be "the engine's").
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flint-owners-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn engine_packages() -> Packages {
    let read = Packages::read(&repo().join(PACKAGES_DIR));
    let lines: Vec<String> = read.problems.iter().map(|d| d.one_line()).collect();
    assert!(lines.is_empty(), "{lines:#?}");
    read
}

#[test]
fn every_item_of_the_tree_has_exactly_one_package() {
    let found = problems(&engine_packages(), &Inventory::of_repo());
    assert!(
        found.is_empty(),
        "{} problems:\n{}",
        found.len(),
        found.join("\n")
    );
}

/// The counts ADR 0136 decision 7 surveyed on 2026-11-24, so an inventory
/// that read nothing can't pass.
#[test]
fn the_inventory_sees_the_whole_tree() {
    let inv = Inventory::of_repo();
    let count = |kind: Kind| {
        inv.schemas
            .iter()
            .flat_map(|f| &f.items)
            .filter(|(k, _)| *k == kind)
            .count()
    };
    assert!(count(Kind::Component) >= 46, "{}", count(Kind::Component));
    assert!(count(Kind::Archetype) >= 5);
    assert!(count(Kind::SceneTable) >= 7);
    assert!(inv.crates.len() >= 34, "{}", inv.crates.len());
    assert!(inv.commands.len() >= 26);
    // The script API is this build's player's: a package's modules are
    // there exactly when flint-cli was built with it (#1333). `music` is
    // flint-script's, declared in every build.
    assert!(inv.script_modules.len() >= 38, "{:?}", inv.script_modules);
    for (module, package) in [
        ("living", "living"),
        ("wooze", "living"),
        ("debug", "living"),
        ("conducted", "rhythm"),
        ("music", "engine"),
    ] {
        let built = package == "engine" || crate::commands::absent::built_with(package);
        assert_eq!(
            inv.script_modules.iter().any(|m| m == module),
            built,
            "{module}"
        );
    }
    if cfg!(all(feature = "living", feature = "rhythm")) {
        assert!(inv.script_modules.len() >= 44, "{:?}", inv.script_modules);
    }
}

#[test]
fn a_schema_file_no_package_owns_fails_naming_the_file() {
    let tmp = scratch("schemas");
    let schemas = tmp.join("schemas");
    // The examples' layer, spelled by its parts (examples-check reads a
    // path that names the old examples folder)
    let layer = schemas.join("examples").join("components");
    std::fs::create_dir_all(schemas.join("components")).unwrap();
    std::fs::create_dir_all(&layer).unwrap();
    std::fs::write(
        schemas.join("components/zebra.toml"),
        "[component.zebra]\ndescription = \"stripes\"\n",
    )
    .unwrap();
    std::fs::write(
        schemas.join("components/notes.toml"),
        "# a file with nothing in it\n",
    )
    .unwrap();
    // The examples' layer is no engine package's: never reported
    std::fs::write(layer.join("door.toml"), "[component.door]\n").unwrap();
    let mut inv = Inventory::of_repo();
    inv.schemas
        .extend(schema_files(&schemas, "schemas").into_iter().filter(|f| {
            f.path.contains("zebra") || f.path.contains("notes") || f.path.contains("door")
        }));
    let found = problems(&engine_packages(), &inv);
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(
        found,
        [
            "schemas/components/notes.toml: declares no [component.x], [archetype.x] or [table.x], so no package can own it",
            "schemas/components/zebra.toml: the component `zebra` has no package: list it under [provides] components in one packages/<name>/package.toml",
        ]
    );
}

/// #1332: a package's schemas are its folder's, so leaving the package out
/// leaves them out. A feature's component back in the engine's `schemas/`,
/// or the engine's in a package's folder, fails naming where it goes.
#[test]
fn a_schema_file_outside_its_packages_folder_fails_naming_where_it_goes() {
    let mut inv = Inventory::of_repo();
    let living = inv
        .schemas
        .iter()
        .find(|f| f.path.ends_with("living_prop.toml"))
        .expect("living_prop's schema")
        .clone();
    assert_eq!(
        living.path,
        "packages/living/schemas/components/living_prop.toml"
    );
    inv.schemas.push(SchemaFile {
        path: "schemas/components/living_prop.toml".into(),
        items: living.items.clone(),
    });
    inv.schemas.push(SchemaFile {
        path: "packages/music/schemas/components/transform.toml".into(),
        items: vec![(Kind::Component, "transform".into())],
    });
    let found = problems(&engine_packages(), &inv);
    assert_eq!(
        found,
        [
            "schemas/components/living_prop.toml: its items are living's, and the file is in schemas/: move it to packages/living/schemas/",
            "packages/music/schemas/components/transform.toml: its items are engine's, and the file is in packages/music/schemas/: move it to schemas/",
        ]
    );
}

#[test]
fn a_crate_no_package_owns_fails_naming_its_manifest() {
    let mut inv = Inventory::of_repo();
    inv.crates.push(Member {
        manifest: "crates/flint-zebra/Cargo.toml".into(),
        name: "flint-zebra".into(),
    });
    let found = problems(&engine_packages(), &inv);
    assert_eq!(
        found,
        ["crates/flint-zebra/Cargo.toml: the crate `flint-zebra` has no package: list it under [provides] crates in one packages/<name>/package.toml"]
    );

    // And from a real workspace manifest: a member added to Cargo.toml
    let tmp = scratch("crates");
    std::fs::create_dir_all(tmp.join("crates/flint-zebra")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/flint-zebra\"]\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("crates/flint-zebra/Cargo.toml"),
        "[package]\nname = \"flint-zebra\"\n",
    )
    .unwrap();
    let read = workspace_crates(&tmp);
    assert_eq!(read[0].manifest, "crates/flint-zebra/Cargo.toml");
    assert_eq!(read[0].name, "flint-zebra");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_module_or_command_no_package_owns_fails_naming_it() {
    let mut inv = Inventory::of_repo();
    inv.script_modules.push("zebra".into());
    inv.commands.push(Command {
        path: "zebra".into(),
        children: vec![Command {
            path: "zebra stripe".into(),
            children: Vec::new(),
        }],
    });
    // A new leaf under a command whose leaves are owned one by one
    let asset = inv.commands.iter_mut().find(|c| c.path == "asset").unwrap();
    asset.children.push(Command {
        path: "asset paint".into(),
        children: Vec::new(),
    });
    let found = problems(&engine_packages(), &inv);
    assert_eq!(
        found,
        [
            "the script API: the script module `zebra` has no package: list it under [provides] script_modules in one packages/<name>/package.toml",
            "`flint asset paint`: has no package: list it under [provides] commands in one packages/<name>/package.toml",
            "`flint zebra stripe`: has no package: list it under [provides] commands in one packages/<name>/package.toml",
        ]
    );
}

#[test]
fn an_item_two_packages_claim_or_one_the_tree_lacks_fails() {
    let mut packages = engine_packages();
    let living = packages
        .packages
        .iter_mut()
        .find(|p| p.name == "living")
        .unwrap();
    living.provides.components.push("transform".into());
    living.provides.commands.push("music".into());
    living.provides.crates.push("flint-gone".into());
    let found = problems(&packages, &Inventory::of_repo());
    for want in [
        "schemas/components/transform.toml: the component `transform` is provided by engine and living: an item has one owner",
        "`flint music validate`: provided by `music`, and `flint music` above it by `living`: a command has one owner",
        "packages/living/package.toml: provides.crates lists `flint-gone`, and the tree has no crate of that name",
    ] {
        assert!(
            found.iter().any(|f| f.ends_with(want) || f == want),
            "missing {want:?} in {found:#?}"
        );
    }
}

/// `[build] features` are flint-player's or flint-cli's features, and a
/// package whose player feature turns another package's on requires it.
#[test]
fn the_build_features_are_the_players_and_bring_what_they_require() {
    let player = features_of("flint-player");
    let cli = features_of("flint-cli");
    let packages = engine_packages();
    let by_feature: BTreeMap<&str, &str> = packages
        .packages
        .iter()
        .flat_map(|p| {
            p.features
                .iter()
                .map(move |f| (f.as_str(), p.name.as_str()))
        })
        .collect();
    for p in &packages.packages {
        for feature in &p.features {
            let there = player.contains_key(feature) || cli.contains_key(feature);
            let pending = PENDING_FEATURES.iter().find(|(f, _)| f == feature);
            match (there, pending) {
                (true, Some(_)) => {
                    panic!("`{feature}` is a feature now: take it out of PENDING_FEATURES")
                }
                (false, None) => panic!(
                    "{}: `{feature}` is a feature of neither flint-player nor flint-cli",
                    flint_core::project::shown(&p.file)
                ),
                _ => {}
            }
            for on in player.get(feature).into_iter().flatten() {
                if let Some(needed) = by_feature.get(on.as_str()) {
                    assert!(
                        *needed == p.name || p.requires.contains_key(*needed),
                        "{}: flint-player's `{feature}` turns on `{on}`, so `{}` requires `{needed}`",
                        flint_core::project::shown(&p.file),
                        p.name
                    );
                }
            }
        }
    }
}

/// flint-cli builds each package the manifests name (#1333): its own
/// feature of that name turns flint-player's on, every crate the package
/// provides that flint-cli names is optional and turned on by it, and
/// `flint package list --built` knows the feature. `debug-hud` is the one
/// the CLI always has.
#[test]
fn flint_cli_has_each_packages_feature_and_forwards_it() {
    const ALWAYS: &[&str] = &["debug-hud"];
    // A package's crate flint-cli still links in every build, and the
    // ticket that frees it. The test fails once it's optional, to remove it.
    const LINKED_ANYWAY: &[(&str, &str)] = &[(
        "flint-procgen",
        "#1516: flint-viewer links it for the [living] and [wooze] tables",
    )];
    let player = features_of("flint-player");
    let cli = features_of("flint-cli");
    let manifest: toml::Table = std::fs::read_to_string(repo().join("crates/flint-cli/Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let deps = manifest["dependencies"].as_table().unwrap();
    for p in &engine_packages().packages {
        for feature in &p.features {
            assert!(
                crate::commands::absent::BUILT_FEATURES
                    .iter()
                    .any(|(f, _)| f == feature),
                "absent::BUILT_FEATURES has no `{feature}`"
            );
            if ALWAYS.contains(&feature.as_str()) {
                assert!(crate::commands::absent::built_with(feature));
                continue;
            }
            let on = cli
                .get(feature)
                .unwrap_or_else(|| panic!("flint-cli has no `{feature}` feature"));
            if player.contains_key(feature) {
                let forward = format!("flint-player/{feature}");
                assert!(
                    on.contains(&forward),
                    "flint-cli's `{feature}` lacks {forward}"
                );
            }
            for krate in p.provides.of(Kind::Crate) {
                let Some(dep) = deps.get(krate.as_str()) else {
                    continue;
                };
                let optional = dep.get("optional").and_then(|o| o.as_bool()) == Some(true);
                match LINKED_ANYWAY.iter().find(|(k, _)| k == krate) {
                    Some((_, why)) => {
                        assert!(
                            !optional,
                            "`{krate}` is optional now: take it out of LINKED_ANYWAY ({why})"
                        );
                        continue;
                    }
                    None => assert!(
                        optional,
                        "flint-cli links `{krate}` ({}'s) in every build",
                        p.name
                    ),
                }
                let dep_on = format!("dep:{krate}");
                let reached = std::iter::once(feature.as_str())
                    .chain(on.iter().map(String::as_str))
                    .filter_map(|f| cli.get(f))
                    .flatten()
                    .any(|f| *f == dep_on);
                assert!(reached, "flint-cli's `{feature}` doesn't turn on {dep_on}");
            }
        }
    }
}
