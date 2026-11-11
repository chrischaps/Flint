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
//! component fewer).
//!
//! The repository's own tree must have no problem, and a tree with the #266
//! mistake in it must have exactly that one, naming the file.

use flint_constraint::ConstraintRegistry;
use flint_schema::SchemaRegistry;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The schema layers of the repository, each with the stack it is loaded
/// in: the one its `flint.toml` names (`flint.toml` for the first two,
/// `showcase_game/flint.toml` for the third). A directory of schema files
/// that is in none of them fails [`problems`] as unread.
const STACKS: &[&[&str]] = &[
    &["schemas"],
    &["schemas", "schemas/examples"],
    &["schemas", "schemas/examples", "showcase_game/schemas"],
];

/// The roots walked for `.toml` files: every file under them must be a
/// schema file of one of the [`STACKS`]' layers.
const ROOTS: &[&str] = &["schemas", "showcase_game/schemas"];

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
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_schema_file_in_the_repository_loads() {
    let found = problems(&repo(), ROOTS, STACKS);
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
    for dir in ROOTS {
        copy(&repo().join(dir), &root.join(dir));
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

    let found = problems(&root, ROOTS, STACKS);
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

    let found = problems(&root, ROOTS, STACKS);
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
