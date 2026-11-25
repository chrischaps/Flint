//! Embeds the engine's schema TOML for the web entry (`src/bin/flint_web.rs`).
//!
//! A browser can't list a directory over HTTP, so the web player carries
//! `schemas/components`, `schemas/archetypes` and `schemas/scene` (the
//! scene-level tables, #686), and the same folders of every
//! `packages/<name>/schemas` (#1332), in the binary (about 140 KB of text). A game bundle's own schemas (#98) load over them. Writes
//! `$OUT_DIR/engine_schemas.rs`; the desktop player never includes it.
//!
//! Also lists the player's scene subsystems (ADR 0117, #724): every file in
//! `src/player_app/subsystems/` is a module with a `for_scene` builder, and
//! `$OUT_DIR/subsystems.rs` declares each and lists its builder and its
//! `POLICY` (the name a game state pauses it by, or `None`; #727) and its
//! `SCRIPT_MODULES` (the script modules it owns, #698) and its `DEBUG_KEYS`
//! (a `flint_runtime::DebugKeyGroup`, #717), so a new subsystem
//! is one new file and no other edit.
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
    subsystems();
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

/// `$OUT_DIR/subsystems.rs`: a module for each `src/player_app/subsystems/*.rs`,
/// in file-name order, `for_scene_builders()` listing their `for_scene`
/// in that order, and `subsystem_policies()` their `POLICY` names. A file that opens with `#![cfg(test)]` is a test's own and
/// is listed only in test builds.
#[allow(clippy::disallowed_methods)]
fn subsystems() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/player_app/subsystems");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "rs"));
    files.sort();
    let mut mods = String::new();
    let mut pushes = String::new();
    let mut policies = String::new();
    let mut modules = String::new();
    let mut keys = String::new();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let cfg = if text.lines().any(|l| l.trim() == "#![cfg(test)]") {
            "#[cfg(test)] "
        } else {
            ""
        };
        let path = file.display().to_string();
        writeln!(mods, "#[path = {path:?}]").unwrap();
        writeln!(mods, "pub(crate) mod {name};").unwrap();
        writeln!(pushes, "    {cfg}builders.push({name}::for_scene);").unwrap();
        writeln!(policies, "    {cfg}names.extend({name}::POLICY);").unwrap();
        writeln!(
            modules,
            "    {cfg}modules.extend({name}::SCRIPT_MODULES.iter().copied());"
        )
        .unwrap();
        writeln!(keys, "    {cfg}groups.push({name}::DEBUG_KEYS);").unwrap();
    }
    let mut out =
        String::from("// Generated by build.rs from src/player_app/subsystems/ (#724).\n");
    out.push_str(&mods);
    out.push_str("\n/// Every scene subsystem's builder, in file-name order.\n");
    out.push_str("#[allow(clippy::vec_init_then_push)]\n");
    out.push_str("pub(crate) fn for_scene_builders() -> Vec<BuildForScene> {\n");
    out.push_str("    let mut builders: Vec<BuildForScene> = Vec::new();\n");
    out.push_str(&pushes);
    out.push_str("    builders\n}\n");
    out.push_str("\n/// The names a game state's policy may give the scene subsystems.\n");
    out.push_str("pub(crate) fn subsystem_policies() -> Vec<&'static str> {\n");
    out.push_str("    let mut names: Vec<&'static str> = Vec::new();\n");
    out.push_str(&policies);
    out.push_str("    names\n}\n");
    out.push_str("\n/// The script modules the scene subsystems own, in file-name order (#698).\n");
    out.push_str(
        "pub(crate) fn subsystem_script_modules() -> Vec<&'static dyn flint_script::ScriptModule> {\n",
    );
    out.push_str(
        "    let mut modules: Vec<&'static dyn flint_script::ScriptModule> = Vec::new();\n",
    );
    out.push_str(&modules);
    out.push_str("    modules\n}\n");
    out.push_str("\n/// Each file's `DEBUG_KEYS`: its debug keys and when they are live (#717).\n");
    out.push_str("#[allow(clippy::vec_init_then_push)]\n");
    out.push_str("pub(crate) fn subsystem_debug_keys() -> Vec<flint_runtime::DebugKeyGroup> {\n");
    out.push_str("    let mut groups: Vec<flint_runtime::DebugKeyGroup> = Vec::new();\n");
    out.push_str(&keys);
    out.push_str("    groups\n}\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("subsystems.rs");
    std::fs::write(dest, out).unwrap();
}
