//! A game's Cargo workspace names the engine's `[patch.crates-io]` (#982).
//!
//! cargo applies `[patch]` only from the root of the workspace it builds.
//! In the engine's checkout that root is the engine's own `Cargo.toml`, so
//! its patches hold. A game with a Rust binary of its own builds the engine
//! crates from its own workspace, the engine at `engine/` reached by path,
//! and none of the engine's patches apply there unless the game's root
//! names them too:
//!
//! - `mach-dxcompiler-rs` is Flint's static DXC (#947). flint-render asks
//!   for exactly vendor/static-dxc's version, which crates.io doesn't have,
//!   so without the line cargo's resolver stops before anything builds.
//! - `egui-winit` adds the function the web build needs (#945); a native
//!   build compiles without it.
//!
//! [`patches`] reads the project's `Cargo.toml` (in its folder or the
//! nearest above it) and the root of the workspace it belongs to, finds each engine checkout its path dependencies lead to,
//! and warns once for each patch of that engine's root the game's root
//! lacks or points somewhere else, with the line to add. A project with no
//! `Cargo.toml`, or the engine's own checkout, gets nothing.
//!
//! Read from the disk: a bundle packs no `Cargo.toml`, so `flint bundle`
//! never asks.

use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::vfs;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// The dependency tables a manifest can name a path in, as dotted keys.
const DEPENDENCY_TABLES: &[&str] = &[
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
    "workspace.dependencies",
];

/// What is wrong with the `[patch.crates-io]` of the workspace that builds
/// the project at `project_root`, as warnings on its root `Cargo.toml`.
pub fn patches(project_root: &Path) -> Vec<Diagnostic> {
    let Some(start) = absolute(project_root) else {
        return Vec::new();
    };
    // The package the project is in: its folder's Cargo.toml, or the
    // nearest one above (a project in a crate's subfolder)
    let Some((project_root, manifest)) = start
        .ancestors()
        .find_map(|dir| read_toml(&dir.join("Cargo.toml")).map(|m| (dir.to_path_buf(), m)))
    else {
        return Vec::new();
    };
    let (root_dir, root) = workspace_root(&project_root, manifest.clone());

    // Every path dependency the workspace names: in its root, the project's
    // own manifest, and the members the root lists by name
    let mut deps = BTreeSet::new();
    path_dependencies(&root, &root_dir, &mut deps);
    path_dependencies(&manifest, &project_root, &mut deps);
    for member in members(&root) {
        let dir = clean(&root_dir.join(member));
        if let Some(member) = read_toml(&dir.join("Cargo.toml")) {
            path_dependencies(&member, &dir, &mut deps);
        }
    }

    // The checkouts outside this workspace those dependencies belong to
    let engines: BTreeSet<PathBuf> = deps
        .iter()
        .filter_map(|dep| workspace_of(dep))
        .filter(|engine| !same_dir(engine, &root_dir))
        .collect();

    let ours = patch_table(&root);
    let root_manifest = root_dir.join("Cargo.toml");
    let mut found = Vec::new();
    for engine in &engines {
        let Some(theirs) = read_toml(&engine.join("Cargo.toml")).map(|m| patch_table(&m)) else {
            continue;
        };
        for (name, path) in theirs {
            let wanted = clean(&engine.join(&path));
            let line = format!(
                "{name} = {{ path = \"{}\" }}",
                spelled_from(&wanted, &root_dir)
            );
            let hint = format!(
                "add `{line}` under [patch.crates-io] in this workspace's root Cargo.toml \
                 (book: Upgrading the Engine, Cargo)"
            );
            // What building without it does
            let costs = match name.as_str() {
                "mach-dxcompiler-rs" => {
                    "so cargo stops on flint-render's `mach-dxcompiler-rs = \"=0.1.947\"`, \
                     Flint's own DXC, which crates.io doesn't have"
                        .to_string()
                }
                _ => format!("so the engine's crates get crates.io's `{name}` here"),
            };
            let message = match ours.iter().find(|(n, _)| *n == name) {
                None => format!(
                    "the engine at `{}` patches `{name}` and this workspace doesn't: cargo \
                     applies [patch] only from the workspace root, {costs}",
                    spelled_from(engine, &root_dir)
                ),
                Some((_, mine)) if same_dir(&clean(&root_dir.join(mine)), &wanted) => continue,
                Some((_, mine)) => format!(
                    "`{name}` is patched with `{mine}`, not the engine's `{}`",
                    spelled_from(&wanted, &root_dir)
                ),
            };
            found.push(
                Diagnostic::warning(codes::MISSING_CARGO_PATCH, message)
                    .file(&root_manifest)
                    .field(format!("patch.crates-io.{name}"))
                    .hint(hint),
            );
        }
    }
    found
}

/// The root of the workspace the package at `dir` builds in, and its
/// manifest: the nearest manifest from `dir` up with a `[workspace]` that
/// doesn't exclude it, else the package's own.
fn workspace_root(dir: &Path, own: toml::Value) -> (PathBuf, toml::Value) {
    if own.get("workspace").is_some() {
        return (dir.to_path_buf(), own);
    }
    for above in dir.ancestors().skip(1) {
        let Some(manifest) = read_toml(&above.join("Cargo.toml")) else {
            continue;
        };
        let Some(workspace) = manifest.get("workspace") else {
            continue;
        };
        let excluded = workspace
            .get("exclude")
            .and_then(|e| e.as_array())
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str())
            .any(|e| same_dir(&clean(&above.join(e)), dir));
        if excluded {
            break;
        }
        return (above.to_path_buf(), manifest);
    }
    (dir.to_path_buf(), own)
}

/// The members a root lists by name (a glob is skipped: its packages are
/// found through the root's own tables or not at all).
fn members(root: &toml::Value) -> Vec<String> {
    root.get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str())
        .filter(|m| !m.contains('*') && !m.contains('?') && !m.contains('['))
        .map(str::to_string)
        .collect()
}

/// Every `path = ".."` dependency of `manifest` (in `dir`), absolute, in
/// its dependency tables and their `[target.<cfg>]` forms.
fn path_dependencies(manifest: &toml::Value, dir: &Path, into: &mut BTreeSet<PathBuf>) {
    let mut tables: Vec<&toml::Value> = DEPENDENCY_TABLES
        .iter()
        .filter_map(|key| dotted(manifest, key))
        .collect();
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        for target in targets.values() {
            tables.extend(DEPENDENCY_TABLES[..3].iter().filter_map(|k| target.get(k)));
        }
    }
    for table in tables.into_iter().filter_map(|t| t.as_table()) {
        for dep in table.values() {
            if let Some(path) = dep.get("path").and_then(|p| p.as_str()) {
                into.insert(clean(&dir.join(path)));
            }
        }
    }
}

/// The root of the workspace the package at `dir` belongs to: the nearest
/// manifest from `dir` up with a `[workspace]`. `None` for a package in
/// none.
fn workspace_of(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|above| {
            read_toml(&above.join("Cargo.toml")).is_some_and(|m| m.get("workspace").is_some())
        })
        .map(Path::to_path_buf)
}

/// A manifest's `[patch.crates-io]` entries that name a `path`.
fn patch_table(manifest: &toml::Value) -> Vec<(String, String)> {
    dotted(manifest, "patch.crates-io")
        .and_then(|t| t.as_table())
        .into_iter()
        .flatten()
        .filter_map(|(name, v)| {
            let path = v.get("path")?.as_str()?;
            Some((name.clone(), path.to_string()))
        })
        .collect()
}

fn dotted<'a>(value: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    key.split('.').try_fold(value, |v, part| v.get(part))
}

fn read_toml(path: &Path) -> Option<toml::Value> {
    if !vfs::is_file(path) {
        return None;
    }
    vfs::read_to_string(path).ok()?.parse().ok()
}

fn absolute(path: &Path) -> Option<PathBuf> {
    std::path::absolute(path).ok().map(|p| clean(&p))
}

/// `path` with its `.` and `..` worked out, as cargo joins a path
/// dependency to its manifest's folder.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether two folders are one: the same spelling (ignoring case on
/// Windows), or the same folder once the file system resolves both.
fn same_dir(a: &Path, b: &Path) -> bool {
    let spelled = |p: &Path| {
        let s = vfs::normalize(p);
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    if spelled(a) == spelled(b) {
        return true;
    }
    // The CLI's own file system call: a junction or symlink spells one
    // folder two ways, and only the disk knows
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `path` as the root's manifest would write it: relative when it is under
/// `root` (`engine/vendor/static-dxc`), else absolute, with `/`.
fn spelled_from(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => vfs::normalize(rel),
        Err(_) => vfs::normalize(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch folder for one test, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint-cargo-patch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// An engine at `dir`: a workspace with one crate and two patches.
    fn engine(dir: &Path) {
        write(
            &dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/flint-render\"]\n\n\
             [patch.crates-io]\n\
             egui-winit = { path = \"vendor/egui-winit\" }\n\
             mach-dxcompiler-rs = { path = \"vendor/static-dxc\" }\n",
        );
        write(
            &dir.join("crates/flint-render/Cargo.toml"),
            "[package]\nname = \"flint-render\"\n",
        );
        for vendored in ["vendor/egui-winit", "vendor/static-dxc"] {
            fs::create_dir_all(dir.join(vendored)).unwrap();
        }
    }

    const GAME: &str = "[workspace]\nexclude = [\"engine\"]\n\n\
         [package]\nname = \"game\"\n\n\
         [dependencies]\nflint-render = { path = \"engine/crates/flint-render\" }\n";

    fn fields(found: &[Diagnostic]) -> Vec<String> {
        found.iter().map(|d| d.field.clone().unwrap()).collect()
    }

    /// The case #982 found: a game with the engine at engine/ and no
    /// [patch], told each line to add.
    #[test]
    fn a_game_without_the_engines_patches_is_told_each_line() {
        let game = scratch("missing");
        engine(&game.join("engine"));
        write(&game.join("Cargo.toml"), GAME);
        let found = patches(&game);
        assert_eq!(
            fields(&found),
            [
                "patch.crates-io.egui-winit",
                "patch.crates-io.mach-dxcompiler-rs"
            ]
        );
        let dxc = &found[1];
        assert_eq!(dxc.code, codes::MISSING_CARGO_PATCH);
        assert!(
            dxc.hint.as_deref().unwrap().contains(
                "add `mach-dxcompiler-rs = { path = \"engine/vendor/static-dxc\" }` under [patch.crates-io]"
            ),
            "{:?}",
            dxc.hint
        );
        assert!(dxc
            .file
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .ends_with("Cargo.toml"));
        let _ = fs::remove_dir_all(&game);
    }

    #[test]
    fn a_game_with_them_is_clean_and_a_wrong_one_is_named() {
        let game = scratch("patched");
        engine(&game.join("engine"));
        write(
            &game.join("Cargo.toml"),
            &format!(
                "{GAME}\n[patch.crates-io]\n\
                 mach-dxcompiler-rs = {{ path = \"./engine/vendor/static-dxc\" }}\n\
                 egui-winit = {{ path = \"vendor/my-egui-winit\" }}\n"
            ),
        );
        let found = patches(&game);
        assert_eq!(fields(&found), ["patch.crates-io.egui-winit"]);
        assert!(
            found[0].message.contains("vendor/my-egui-winit"),
            "{}",
            found[0].message
        );
        let _ = fs::remove_dir_all(&game);
    }

    /// The dependencies a Capitol-style root names in
    /// `[workspace.dependencies]`, and an engine outside the game's folder.
    #[test]
    fn workspace_dependencies_and_an_engine_elsewhere_count() {
        let dir = scratch("elsewhere");
        engine(&dir.join("flint"));
        let game = dir.join("game");
        write(
            &game.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/client\"]\n\n\
             [workspace.dependencies]\nflint-render = { path = \"../flint/crates/flint-render\" }\n",
        );
        write(
            &game.join("crates/client/Cargo.toml"),
            "[package]\nname = \"client\"\n[dependencies]\nflint-render.workspace = true\n",
        );
        let found = patches(&game);
        assert_eq!(found.len(), 2);
        let hint = found[1].hint.as_deref().unwrap();
        assert!(hint.contains("/flint/vendor/static-dxc\" }"), "{hint}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The engine's own checkout, and a project with no Cargo.toml.
    #[test]
    fn the_engine_itself_and_a_scripts_only_game_get_nothing() {
        let dir = scratch("itself");
        engine(&dir);
        assert!(patches(&dir).is_empty());
        let scripts_only = scratch("scripts-only");
        write(&scripts_only.join("flint.toml"), "[project]\n");
        assert!(patches(&scripts_only).is_empty());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&scripts_only);
    }
}
