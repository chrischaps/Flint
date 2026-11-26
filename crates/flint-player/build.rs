//! Embeds the engine's schema TOML for the web entry (`src/bin/flint_web.rs`).
//!
//! A browser can't list a directory over HTTP, so the web player carries
//! `schemas/components`, `schemas/archetypes` and `schemas/scene` (the
//! scene-level tables, #686), and the same folders of every
//! `packages/<name>/schemas` (#1332), in the binary (about 140 KB of text). A game bundle's own schemas (#98) load over them. Writes
//! `$OUT_DIR/engine_schemas.rs`; the desktop player never includes it.
//!
//! (It no longer lists the player's scene subsystems: since #1552 each
//! package registers its own through the run's `flint_package::Registry`,
//! from the list the binary's launcher hands the kernel.)
//!
//! And stamps the commit the engine's sources are at (#1290): a trace's
//! `engine_commit` and a capture's `engine.commit` name the code the binary
//! was built from, not the repository it happens to run in. See
//! `src/provenance.rs`.
//!
//! And links the player's binaries with an 8 MB main-thread stack on
//! Windows, as flint-cli's build script does `flint` (#990): there
//! `.cargo/config.toml` is read only by a build run inside the engine's
//! checkout, never by a game building the engine from its own root.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// A build script runs on the host at compile time, never in the browser, so
// the vfs rule on file reads (the root clippy.toml, #97) doesn't apply.
#[allow(clippy::disallowed_methods)]
fn main() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The engine's schemas, then each package's (`packages/<name>/schemas`,
    // by name: the order the resolver brings them in, #1332)
    let packages = engine.join("packages");
    println!("cargo:rerun-if-changed={}", packages.display());
    let mut package_dirs: Vec<PathBuf> = std::fs::read_dir(&packages)
        .map(|rd| {
            rd.filter_map(|e| Some(e.ok()?.path().join("schemas")))
                .collect()
        })
        .unwrap_or_default();
    package_dirs.retain(|d| d.is_dir());
    package_dirs.sort();
    let roots: Vec<PathBuf> = std::iter::once(engine.join("schemas"))
        .chain(package_dirs)
        .collect();
    let mut out = String::new();
    for kind in ["components", "archetypes", "scene"] {
        let mut files: Vec<PathBuf> = Vec::new();
        for root in &roots {
            let dir = root.join(kind);
            // Only a folder that is there: cargo takes a watched path that
            // is missing as changed, so every build rebuilt this crate and
            // flint-cli (#1338: a package has no `archetypes/`, and git
            // keeps no empty folder). One made later is still seen: the
            // whole `packages/` folder is watched above.
            if dir.is_dir() {
                println!("cargo:rerun-if-changed={}", dir.display());
            }
            let mut here: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
                .unwrap_or_default();
            here.retain(|p| p.extension().is_some_and(|e| e == "toml"));
            here.sort();
            files.extend(here);
        }
        writeln!(out, "pub const {}: &[&str] = &[", kind.to_uppercase()).unwrap();
        for file in files {
            writeln!(out, "    include_str!({:?}),", file.display().to_string()).unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("engine_schemas.rs");
    std::fs::write(dest, out).unwrap();
    commit();
    profile();
    main_thread_stack();
}

/// `ENGINE_BUILD_PROFILE`: the Cargo profile this crate was built under
/// (`dev` is said `debug`, as its folder is; `release`, `profiling`,
/// `web-release`...), and `ENGINE_BUILD_OPT_LEVEL`, for a perf report
/// (#1093): a debug and a release frame time are never comparable. Cargo's
/// `PROFILE` names only `debug` or `release`, so the profile is the folder
/// `OUT_DIR` is under: `<target>/[<triple>/]<profile>/build/<pkg>/out`.
fn profile() {
    let out = std::env::var("OUT_DIR").unwrap_or_default();
    let named = Path::new(&out)
        .ancestors()
        .nth(3)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let profile = named.unwrap_or_else(|| std::env::var("PROFILE").unwrap_or_default());
    let opt = std::env::var("OPT_LEVEL").unwrap_or_default();
    println!("cargo:rustc-env=ENGINE_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=ENGINE_BUILD_OPT_LEVEL={opt}");
}

/// The stack `link.exe` (`/STACK`) or MinGW's `ld` (`--stack`) reserves for
/// the main thread of `flint-player`, its examples and its tests: 8 MB, what
/// `.cargo/config.toml` gives the engine's own builds. flint-cli's build
/// script says the same; keep the two alike. A game binary is another
/// package, which this can't reach (guides/player-extensions.md).
fn main_thread_stack() {
    const BYTES: u32 = 8 << 20;
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") != "windows" {
        return;
    }
    match target("CARGO_CFG_TARGET_ENV").as_str() {
        "msvc" => println!("cargo:rustc-link-arg=/STACK:{BYTES}"),
        "gnu" => println!("cargo:rustc-link-arg=-Wl,--stack,{BYTES}"),
        _ => {}
    }
}

/// `ENGINE_BUILD_COMMIT`: the commit of the checkout the engine's sources
/// are in, and `ENGINE_BUILD_SUBDIR`: where in that checkout they are (empty
/// in the engine's own; `engine/` in a game carrying it as a subtree, whose
/// commit it then is). Both empty when git can't say, or when this crate
/// isn't tracked by the checkout around it (a `git archive` unpacked inside
/// another worktree). Read again when HEAD moves: a commit, a checkout, a
/// reset each write the HEAD reflog.
#[allow(clippy::disallowed_methods)] // a build script, on the host: not the vfs's
fn commit() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(here)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = Path::new(&dir);
        // Each only if it is there (a missing one would rebuild every time,
        // as a missing schema folder did, #1338): a checkout with
        // `core.logAllRefUpdates = false` keeps no reflog
        for file in [dir.join("HEAD"), dir.join("logs/HEAD")] {
            if file.is_file() {
                println!("cargo:rerun-if-changed={}", file.display());
            }
        }
    }
    let tracked = git(&["ls-files", "--error-unmatch", "--", "build.rs"]).is_some();
    let commit = git(&["rev-parse", "HEAD"])
        .filter(|c| tracked && !c.is_empty())
        .unwrap_or_default();
    // `crates/flint-player/` in the engine's checkout, `engine/crates/...`
    // in a game's
    let subdir = git(&["rev-parse", "--show-prefix"])
        .filter(|_| !commit.is_empty())
        .and_then(|p| p.strip_suffix("crates/flint-player/").map(str::to_string))
        .unwrap_or_default();
    println!("cargo:rustc-env=ENGINE_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=ENGINE_BUILD_SUBDIR={subdir}");
}
