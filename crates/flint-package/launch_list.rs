//! The launchers' list of package crates (ADR 0137 decision 5, #1555),
//! shared by flint-player's and flint-cli's build scripts, which include
//! this file by `#[path]`. Not a module of the crate: it runs on the host,
//! at compile time.
//!
//! A package that has moved into its folder brings its crates at
//! `packages/<name>/crates/<crate>/`. A crate with a `player` feature has
//! the package's player part at `<crate>::player::PACKAGE` (a
//! `flint_package::Package`); one with a `cli` feature has its commands at
//! `<crate>::cli::extensions()`. The launcher's crate turns that feature on
//! through its own feature named for the package (`music =
//! ["dep:flint-music", "flint-music/player"]`), so a package is in this
//! binary when every one of its `[build] features` is on
//! (`CARGO_FEATURE_<NAME>`), and the generated list names it only then: the
//! launchers' source names no moved package (the epic's gate, #1560).
//!
//! The list is in the order packages register: each after the ones it
//! `[requires]`, and otherwise by tier (kernel, standard, feature, game)
//! and then by name, which is the order the stock list had by hand.
//!
//! Until #1568 gives the manifest a `[rust]` table, the convention above is
//! the whole contract.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One package's manifest, as far as a launcher needs it.
pub struct Manifest {
    pub name: String,
    pub tier: u8,
    pub requires: Vec<String>,
    pub features: Vec<String>,
    /// Its crates under `crates/`, each by its Rust name, with the parts
    /// its features say it has (`player`, `cli`).
    pub crates: Vec<(String, Vec<String>)>,
    /// Every file read, for `cargo:rerun-if-changed`.
    pub read: Vec<PathBuf>,
}

fn tier(name: &str) -> u8 {
    match name {
        "kernel" => 0,
        "standard" => 1,
        "feature" => 2,
        _ => 3,
    }
}

fn strings(table: Option<&toml::Value>, key: &str) -> Vec<String> {
    table
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `packages/<name>/package.toml` under `engine`, read; a manifest
/// that doesn't read is left out (`flint package list` names its problems).
#[allow(clippy::disallowed_methods)] // a build script reads the host's disk
pub fn read(engine: &Path) -> Vec<Manifest> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(engine.join("packages"))
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs {
        let file = dir.join("package.toml");
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let head = table.get("package");
        let Some(name) = head.and_then(|p| p.get("name")).and_then(|n| n.as_str()) else {
            continue;
        };
        let mut read = vec![file.clone()];
        let mut crates = Vec::new();
        let crates_dir = dir.join("crates");
        if crates_dir.is_dir() {
            read.push(crates_dir.clone());
            let mut found: Vec<PathBuf> = std::fs::read_dir(&crates_dir)
                .map(|rd| {
                    rd.filter_map(|e| Some(e.ok()?.path().join("Cargo.toml")))
                        .collect()
                })
                .unwrap_or_default();
            found.retain(|f| f.is_file());
            found.sort();
            for cargo in found {
                let Ok(manifest) = std::fs::read_to_string(&cargo)
                    .map_err(|_| ())
                    .and_then(|t| t.parse::<toml::Table>().map_err(|_| ()))
                else {
                    continue;
                };
                let Some(krate) = manifest
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                else {
                    continue;
                };
                let features = manifest.get("features").and_then(|f| f.as_table());
                let parts: Vec<String> = ["player", "cli"]
                    .into_iter()
                    .filter(|part| features.is_some_and(|f| f.contains_key(*part)))
                    .map(String::from)
                    .collect();
                read.push(cargo.clone());
                crates.push((krate.replace('-', "_"), parts));
            }
        }
        out.push(Manifest {
            name: name.to_string(),
            tier: tier(
                head.and_then(|p| p.get("tier"))
                    .and_then(|t| t.as_str())
                    .unwrap_or(""),
            ),
            requires: table
                .get("requires")
                .and_then(|r| r.as_table())
                .map(|r| r.keys().cloned().collect())
                .unwrap_or_default(),
            features: strings(table.get("build"), "features"),
            crates,
            read,
        });
    }
    out
}

/// The packages in the order they register: requirements first, then by
/// tier and name.
pub fn in_order(mut manifests: Vec<Manifest>) -> Vec<Manifest> {
    manifests.sort_by(|a, b| (a.tier, &a.name).cmp(&(b.tier, &b.name)));
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    while !manifests.is_empty() {
        let next = manifests
            .iter()
            .position(|m| m.requires.iter().all(|r| placed.contains(r)))
            // A cycle, or a requirement no manifest has: keep the rest as
            // they sort (`flint package list` names the mistake)
            .unwrap_or(0);
        let m = manifests.remove(next);
        placed.insert(m.name.clone());
        out.push(m);
    }
    out
}

/// Whether the crate being built has every one of `features` on.
pub fn built(features: &[String]) -> bool {
    !features.is_empty()
        && features.iter().all(|f| {
            std::env::var_os(format!(
                "CARGO_FEATURE_{}",
                f.to_uppercase().replace('-', "_")
            ))
            .is_some()
        })
}

/// The Rust paths of `part` (`player`, `cli`) of every package this build
/// has, in registration order, each with its package's name.
pub fn parts(manifests: &[Manifest], part: &str) -> Vec<(String, String)> {
    manifests
        .iter()
        .filter(|m| built(&m.features))
        .flat_map(|m| {
            m.crates
                .iter()
                .filter(|(_, parts)| parts.iter().any(|p| p == part))
                .map(|(krate, _)| (m.name.clone(), krate.clone()))
        })
        .collect()
}

/// `cargo:rerun-if-changed` for every file the list was read from.
pub fn watch(manifests: &[Manifest]) {
    for m in manifests {
        for file in &m.read {
            println!("cargo:rerun-if-changed={}", file.display());
        }
    }
}
