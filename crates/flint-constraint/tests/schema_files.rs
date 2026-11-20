//! Every schema file in the repository loads (#333).
//!
//! While writing #266 an unescaped quote went into
//! `schemas/components/style_mask.toml`'s description. Nothing in `cargo
//! test` noticed: the first sign was every `flint render` failing on
//! `expected newline, found #`, with no file named, behind any script that
//! swallowed its output.
//!
//! [`problems`] walks a tree of schema directories and loads every `.toml`
//! in it with the loader its directory is read by, one file at a time, so
//! one bad file doesn't hide the next:
//!
//! - `components/`, `archetypes/`, `scene/`: [`SchemaRegistry`], and none of
//!   the lints it keeps ([`SchemaRegistry::lints`])
//! - `constraints/`: [`ConstraintRegistry`], and every query parses
//!   ([`ConstraintRegistry::problems`])
//! - anywhere else: nothing reads it, so a file in `component/` or at a
//!   layer's top level is a mistake too
//!
//! Then each layer stack a manifest names loads whole, and every archetype
//! names only components that stack declares (the registry skips an unknown
//! one in silence, so an archetype with a misspelt component just has one
//! component fewer). A `required_component` constraint of the stack names an
//! archetype the stack declares, which lists that component: when #681 made
//! the examples' room "transform-only" while `rooms_have_bounds` still asked
//! every room for `bounds`, the three showcase levels failed `flint check`
//! and nothing in `cargo test` said why (#974).
//!
//! The repository's own tree must have no problem, and a tree with the #266
//! mistake in it must have exactly that one, naming the file.
#![allow(clippy::disallowed_methods)] // a test: reads its files off the disk, not through the vfs

use flint_constraint::ConstraintRegistry;
use flint_schema::SchemaRegistry;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The schema layers of the repository, read from its manifests.
struct Layers {
    /// Each stack a layer is loaded in, as paths from the repository: the
    /// engine's set alone (what a game's `engine/schemas` is), the stack the
    /// root `flint.toml` names, and the one each game kept in the checkout
    /// names (a top-level directory with a `flint.toml`, found by it so the
    /// engine names no game, #760). A directory of schema files that is in
    /// none of them fails [`problems`] as unread.
    stacks: Vec<Vec<String>>,
    /// The roots walked for `.toml` files, the layers no other layer holds:
    /// every file under them must be a schema file of a stack's layer.
    roots: Vec<String>,
}

impl Layers {
    fn of_repo() -> Layers {
        let root = repo();
        let manifest_stack = |dir: &str| -> Option<Vec<String>> {
            let text = std::fs::read_to_string(root.join(dir).join("flint.toml")).ok()?;
            let doc: toml::Table = text
                .parse()
                .unwrap_or_else(|e| panic!("{dir}/flint.toml: {e}"));
            let layers = doc.get("project")?.get("schemas")?.as_array()?;
            Some(
                layers
                    .iter()
                    .map(|l| normalize(&format!("{dir}/{}", l.as_str().unwrap())))
                    .collect(),
            )
        };
        let mut stacks = vec![vec!["schemas".to_string()]];
        stacks.extend(manifest_stack("."));
        let mut games: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("flint.toml").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        games.sort();
        assert!(!games.is_empty(), "no game kept in the checkout");
        stacks.extend(games.iter().filter_map(|g| manifest_stack(g)));

        let mut layers: Vec<String> = stacks.iter().flatten().cloned().collect();
        layers.sort();
        layers.dedup();
        let roots = layers
            .iter()
            .filter(|l| !layers.iter().any(|o| l.starts_with(&format!("{o}/"))))
            .cloned()
            .collect();
        Layers { stacks, roots }
    }

    fn roots(&self) -> Vec<&str> {
        self.roots.iter().map(String::as_str).collect()
    }

    fn problems(&self, root: &Path) -> Vec<String> {
        let stacks: Vec<Vec<&str>> = self
            .stacks
            .iter()
            .map(|s| s.iter().map(String::as_str).collect())
            .collect();
        let stacks: Vec<&[&str]> = stacks.iter().map(Vec::as_slice).collect();
        problems(root, &self.roots(), &stacks)
    }
}

/// `a/./b/../c` as `a/c`, for a layer a game names from its own folder.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// The directories of a schema layer, and which loader reads each.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Component,
    Archetype,
    Table,
    Constraint,
}

impl Kind {
    fn of_dir(dir: &str) -> Option<Kind> {
        match dir {
            "components" => Some(Kind::Component),
            "archetypes" => Some(Kind::Archetype),
            "scene" => Some(Kind::Table),
            "constraints" => Some(Kind::Constraint),
            _ => None,
        }
    }
}

/// Every `.toml` under `dir`, in name order.
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "toml") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

fn shown(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A loader's error about `file`, with the file as `at`: the loaders name
/// the file by the path they were given, which here is absolute.
fn said_about(error: &str, file: &Path, at: &str) -> String {
    let full = file.to_string_lossy().replace('\\', "/");
    if error.contains(&full) {
        error.replacen(&full, at, 1)
    } else {
        format!("{at}: {error}")
    }
}

/// Everything wrong with the schema files of `root`: each a line naming
/// the file. `roots` are walked for files; `stacks` are the layers, as paths
/// relative to `root`, in the orders they are loaded in.
fn problems(root: &Path, roots: &[&str], stacks: &[&[&str]]) -> Vec<String> {
    let mut found = Vec::new();
    let layers: Vec<PathBuf> = {
        let mut all: Vec<&str> = stacks.iter().flat_map(|s| s.iter().copied()).collect();
        all.sort();
        all.dedup();
        all.into_iter().map(|l| root.join(l)).collect()
    };
    let mut seen = 0;

    for dir in roots {
        let files = toml_files(&root.join(dir));
        assert!(!files.is_empty(), "{dir} holds no .toml files");
        seen += files.len();
        for file in files {
            let at = shown(&file, root);
            // The directory a file is in names its kind, and the directory
            // above that must be a layer.
            let parent = file.parent().unwrap();
            let kind = parent
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(Kind::of_dir)
                .filter(|_| layers.iter().any(|l| l == parent.parent().unwrap()));
            let Some(kind) = kind else {
                found.push(format!(
                    "{at}: nothing reads this file: a schema layer reads its components/, \
                     archetypes/, scene/ and constraints/"
                ));
                continue;
            };
            match kind {
                Kind::Component | Kind::Archetype | Kind::Table => {
                    let mut registry = SchemaRegistry::new();
                    let loaded = match kind {
                        Kind::Component => registry.load_component_file(&file),
                        Kind::Archetype => registry.load_archetype_file(&file),
                        _ => registry.load_table_file(&file),
                    };
                    match loaded {
                        Err(e) => found.push(said_about(&e.to_string(), &file, &at)),
                        Ok(()) => found.extend(
                            registry
                                .lints()
                                .iter()
                                .map(|d| format!("{at}: [{}] {}", d.code, d.message)),
                        ),
                    }
                }
                Kind::Constraint => {
                    let mut registry = ConstraintRegistry::new();
                    match registry.load_file(&file) {
                        Err(e) => found.push(said_about(&e.to_string(), &file, &at)),
                        Ok(()) if registry.is_empty() => {
                            found.push(format!("{at}: holds no [[constraint]]"))
                        }
                        Ok(()) => found.extend(registry.problems().iter().map(|p| {
                            format!(
                                "{at}: constraint '{}': its {} `{}` can't be evaluated: {}",
                                p.name, p.field, p.query, p.error
                            )
                        })),
                    }
                }
            }
        }
    }
    assert!(seen > 0, "no schema files found under {}", root.display());
    let files_load = found.is_empty();

    for stack in stacks {
        let dirs: Vec<PathBuf> = stack.iter().map(|l| root.join(l)).collect();
        let named = stack.join(" + ");
        // What a command loads. Once a file has failed on its own, this
        // fails on the same file, so it only speaks when they all loaded.
        if files_load {
            if let Err(e) = SchemaRegistry::load_from_directories(&dirs) {
                found.push(format!("{named}: {e}"));
            }
            for layer in &dirs {
                if let Err(e) = ConstraintRegistry::load_from_directory(layer) {
                    found.push(format!("{named}: {e}"));
                }
            }
        }
        // The stack in the loader's order (each layer's tables, components,
        // archetypes), each file on its own so a bad one leaves the rest.
        let mut registry = SchemaRegistry::new();
        for layer in &dirs {
            for dir in ["scene", "components", "archetypes"] {
                let dir = layer.join(dir);
                if !dir.is_dir() {
                    continue;
                }
                let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .unwrap()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    .collect();
                files.sort();
                for file in files {
                    let _ = match dir.file_name().and_then(|n| n.to_str()) {
                        Some("scene") => registry.load_table_file(&file),
                        Some("components") => registry.load_component_file(&file),
                        _ => registry.load_archetype_file(&file),
                    };
                }
            }
        }
        let mut archetypes = registry.archetypes();
        archetypes.sort_by(|a, b| a.name.cmp(&b.name));
        for archetype in archetypes {
            let defaults = archetype.defaults.keys();
            for component in archetype.components.iter().chain(defaults) {
                if registry.get_component(component).is_none() {
                    found.push(format!(
                        "{named}: archetype '{}' names component '{component}', which no \
                         components/ file of the stack declares",
                        archetype.name
                    ));
                }
            }
        }
        // A constraint that asks an archetype for a component its schema
        // doesn't list: the two say different things about what one is.
        for layer in &dirs {
            let Ok(constraints) = ConstraintRegistry::load_from_directory(layer) else {
                continue; // said above, or by the file on its own
            };
            for constraint in constraints.all() {
                let flint_constraint::ConstraintKind::RequiredComponent {
                    archetype,
                    component,
                } = &constraint.kind
                else {
                    continue;
                };
                let lists = registry
                    .get_archetype(archetype)
                    .map(|a| a.has_component(component));
                let problem = match lists {
                    None => {
                        format!("names archetype '{archetype}', which the stack doesn't declare")
                    }
                    Some(false) => format!(
                        "asks every '{archetype}' for '{component}', which the archetype's \
                         components don't list"
                    ),
                    Some(true) => continue,
                };
                found.push(format!(
                    "{named}: constraint '{}' {problem}",
                    constraint.name
                ));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_schema_file_in_the_repository_loads() {
    let found = Layers::of_repo().problems(&repo());
    assert!(
        found.is_empty(),
        "{} problem(s) in the repository's schema files:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}

/// A copy of the repository's schema layers under a scratch directory, to
/// break.
fn scratch_copy(name: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let path = entry.path();
            let target = to.join(entry.file_name());
            if path.is_dir() {
                copy(&path, &target);
            } else {
                std::fs::copy(&path, &target).unwrap();
            }
        }
    }
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("schema_files_{name}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in Layers::of_repo().roots {
        copy(&repo().join(&dir), &root.join(&dir));
    }
    root
}

fn replace_in(file: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.contains(from), "{}: no `{from}`", file.display());
    std::fs::write(file, text.replacen(from, to, 1)).unwrap();
}

/// #266's mistake: a description with a quote in it, unescaped.
#[test]
fn an_unescaped_quote_in_a_component_file_fails_and_names_the_file() {
    let root = scratch_copy("quote");
    let file = root.join("schemas/components/style_mask.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let description = text
        .lines()
        .find(|l| l.trim_start().starts_with("description = \""))
        .expect("style_mask.toml has a description")
        .to_string();
    let broken = description.replacen("= \"", "= \"the \"style\" ", 1);
    std::fs::write(&file, text.replacen(&description, &broken, 1)).unwrap();

    let found = Layers::of_repo().problems(&root);
    println!("{}", found.join("\n"));
    assert_eq!(found.len(), 1, "one mistake, one problem: {found:#?}");
    assert!(
        found[0].starts_with("TOML parse error: schemas/components/style_mask.toml: "),
        "{}",
        found[0]
    );

    // What `flint render` printed: now with the file in it.
    let e = SchemaRegistry::load_from_directories(&[root.join("schemas")]).unwrap_err();
    assert!(
        e.to_string()
            .contains("/schemas/components/style_mask.toml: TOML parse error at line"),
        "{e}"
    );
}

#[test]
fn a_broken_file_of_each_kind_is_found_with_the_others() {
    let root = scratch_copy("each_kind");
    // An archetype that isn't TOML, a scene table that is TOML and isn't a
    // table schema, a constraint whose query doesn't parse, a component file
    // in a directory nothing reads, and an archetype naming a component
    // nobody declares.
    std::fs::write(
        root.join("schemas/archetypes/sprite.toml"),
        "[archetype.sprite\ncomponents = []\n",
    )
    .unwrap();
    std::fs::write(
        root.join("schemas/scene/camera.toml"),
        "[table.camera]\nfields = 3\n",
    )
    .unwrap();
    replace_in(
        &root.join("schemas/examples/constraints/basics.toml"),
        "query = \"entities where archetype == 'door'\"",
        "query = \"entities where archetype ==\"",
    );
    std::fs::create_dir_all(root.join("schemas/component")).unwrap();
    std::fs::copy(
        root.join("schemas/components/sky.toml"),
        root.join("schemas/component/sky.toml"),
    )
    .unwrap();
    replace_in(
        &root.join("schemas/archetypes/character.toml"),
        "\"transform\"",
        "\"transfrom\"",
    );

    let found = Layers::of_repo().problems(&root);
    println!("{}", found.join("\n"));
    let has = |needle: &str| found.iter().any(|l| l.contains(needle));
    assert!(
        has("schemas/archetypes/sprite.toml: TOML parse error"),
        "{found:#?}"
    );
    assert!(
        has("schemas/scene/camera.toml: TOML parse error"),
        "{found:#?}"
    );
    assert!(
        has("schemas/examples/constraints/basics.toml: constraint 'doors_have_transform': its query"),
        "{found:#?}"
    );
    assert!(
        has("schemas/component/sky.toml: nothing reads this file"),
        "{found:#?}"
    );
    assert!(
        has("schemas: archetype 'character' names component 'transfrom'"),
        "{found:#?}"
    );
}

/// #974: the examples' room as #681 left it, "transform-only", while
/// `rooms_have_bounds` asks every room for `bounds`. Found once in each
/// stack that loads the examples' layer: the root's, and the showcase's.
#[test]
fn a_constraint_asking_an_archetype_for_a_component_it_lacks_is_found() {
    let root = scratch_copy("room_without_bounds");
    replace_in(
        &root.join("schemas/examples/archetypes/room.toml"),
        "components = [\"transform\", \"bounds\"]",
        "components = [\"transform\"]",
    );

    let found = Layers::of_repo().problems(&root);
    println!("{}", found.join("\n"));
    let said = ": constraint 'rooms_have_bounds' asks every 'room' for 'bounds', which the \
                archetype's components don't list";
    assert_eq!(
        found.len(),
        2,
        "one mistake, one problem a stack: {found:#?}"
    );
    assert!(found.iter().all(|l| l.ends_with(said)), "{found:#?}");
    assert!(
        found[0].starts_with("schemas + schemas/examples"),
        "{found:#?}"
    );
}
