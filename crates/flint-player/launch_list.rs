//! The player launcher's list of package crates (ADR 0137 decision 5,
//! #1555), for flint-player's build script, which includes this file by
//! `#[path]`. Not a module of the crate: it runs on the host, at compile
//! time. flint-cli's build script lists the CLI's the same way from
//! `[rust] cli` (#1554).
//!
//! A package that has moved into its folder brings its crates at
//! `packages/<name>/crates/<crate>/`, and its manifest's `[rust] player`
//! names the one whose `player::PACKAGE` (a `flint_package::Package`) is its
//! player part. flint-player's feature named for the package turns that
//! crate's `player` feature on (`music = ["dep:flint-music",
//! "flint-music?/player"]`), so a package is in this binary when every one of
//! its `[build] features` is on (`CARGO_FEATURE_<NAME>`), and the generated
//! list names it only then: the player's source names no moved package (the
//! epic's gate, #1560).
//!
//! The list is in the order packages register: each after the ones it
//! `[requires]`, and otherwise by tier (kernel, standard, feature, game)
//! and then by name, which is the order the stock list had by hand.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One package's manifest, as far as a launcher needs it.
pub struct Manifest {
    pub name: String,
    pub tier: u8,
    pub requires: Vec<String>,
    pub features: Vec<String>,
    /// `[rust] player`: its crate's Rust name.
    pub player: Option<String>,
    /// `[provides] components`, `scene_tables` and `archetypes`: what a
    /// scene uses of it (#1585; archetypes since #1618, first-person's
    /// `player`).
    pub components: Vec<String>,
    pub scene_tables: Vec<String>,
    pub archetypes: Vec<String>,
    /// `[without] does` and `policies`: what a build without it still says
    /// and takes (#1585).
    pub does: Option<String>,
    pub policies: Vec<String>,
    /// The manifest, for `cargo:rerun-if-changed`.
    pub file: PathBuf,
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
            player: table
                .get("rust")
                .and_then(|r| r.get("player"))
                .and_then(|p| p.as_str())
                .map(|krate| krate.replace('-', "_")),
            components: strings(table.get("provides"), "components"),
            scene_tables: strings(table.get("provides"), "scene_tables"),
            archetypes: strings(table.get("provides"), "archetypes"),
            does: table
                .get("without")
                .and_then(|w| w.get("does"))
                .and_then(|d| d.as_str())
                .map(String::from),
            policies: strings(table.get("without"), "policies"),
            file,
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

/// The player crate of every package this build has that names one, in
/// registration order.
pub fn players(manifests: &[Manifest]) -> Vec<String> {
    manifests
        .iter()
        .filter(|m| built(&m.features))
        .filter_map(|m| m.player.clone())
        .collect()
}

/// `cargo:rerun-if-changed` for every manifest the list was read from.
pub fn watch(manifests: &[Manifest]) {
    for m in manifests {
        println!("cargo:rerun-if-changed={}", m.file.display());
    }
}
