//! Links `flint` with an 8 MB main-thread stack on Windows (#990), and
//! compiles in the package manifests (#1553, [`package_manifests`]).
//!
//! Windows gives a program's main thread the stack its exe asks for, 1 MB
//! unless the linker is told otherwise, where Linux and macOS shells give
//! 8 MB. A debug build's frames are large (the derived clap parser's
//! `augment_args` is about 8 KB a flag), and `flint --help` once overflowed
//! 1 MB (#708, #982). `.cargo/config.toml` asks for 8 MB too, but cargo reads
//! that file only from the directory it runs in and that directory's
//! parents: a game that builds the engine from its own root
//! (`cargo build --manifest-path engine/Cargo.toml`) never reads it. A build
//! script travels with the crate however it is built.
//!
//! `rustc-link-arg` reaches every target of this package that links (the
//! binary, its tests), and no other package's: a game binary of its own
//! asks for the stack itself (guides/player-extensions.md). flint-player's
//! build script says the same; keep the two alike.

fn main() {
    main_thread_stack();
    package_manifests();
    moved_packages();
}

/// What the CLI launcher lists and knows of the packages without naming
/// one in `src/` (#1557):
///
/// - `OUT_DIR/moved_cli.rs`: `cli()`, the `cli()` of each package whose
///   `[rust] cli` crate this build has (its `[build] features` all on);
/// - `OUT_DIR/built_features.rs`: `BUILT_FEATURES`, each package feature
///   (this crate's `packages` list, in its order) and device feature (the
///   rest of `default`) with whether this build turns it on, after
///   flint-player's `debug-hud` and `frame-history`, which are always on.
#[allow(clippy::disallowed_methods)] // a build script, on the host: not the vfs's
fn moved_packages() {
    use std::fmt::Write as _;
    let here = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).to_path_buf();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let on = |feature: &str| {
        let var = format!("CARGO_FEATURE_{}", feature.to_uppercase().replace('-', "_"));
        std::env::var_os(var).is_some()
    };
    let strings = |table: &toml::Table, at: &str, key: &str| -> Vec<String> {
        table
            .get(at)
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut manifests: Vec<(String, toml::Table)> = std::fs::read_dir(here.join("../../packages"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("package.toml"))
        .filter(|file| file.is_file())
        .filter_map(|file| {
            let table: toml::Table = std::fs::read_to_string(&file).ok()?.parse().ok()?;
            let name = table.get("package")?.get("name")?.as_str()?.to_string();
            Some((name, table))
        })
        .collect();
    manifests.sort_by(|a, b| a.0.cmp(&b.0));
    let mut code = String::from(
        "/// The moved packages' CLI extensions this build has (#1557).\n\
         #[allow(clippy::vec_init_then_push, unused_mut)]\n\
         pub(crate) fn cli() -> Vec<Box<dyn flint_package::cli::CliExtension>> {\n    \
         let mut list: Vec<Box<dyn flint_package::cli::CliExtension>> = Vec::new();\n",
    );
    for (_, table) in &manifests {
        let Some(cli) = table
            .get("rust")
            .and_then(|r| r.get("cli"))
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        let krate = cli.split('/').next().unwrap_or(cli).replace('-', "_");
        let features = strings(table, "build", "features");
        if !features.is_empty() && features.iter().all(|f| on(f)) {
            writeln!(code, "    list.extend({krate}::cli());").unwrap();
        }
    }
    code.push_str("    list\n}\n");
    std::fs::write(out.join("moved_cli.rs"), code).unwrap();

    let cargo: toml::Table = std::fs::read_to_string(here.join("Cargo.toml"))
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let features = cargo.get("features").and_then(|f| f.as_table());
    let list = |name: &str| -> Vec<String> {
        features
            .and_then(|f| f.get(name))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let packages = list("packages");
    let devices: Vec<String> = list("default")
        .into_iter()
        .filter(|f| f != "packages")
        .collect();
    let mut table =
        String::from("&[\n    (\"debug-hud\", true),\n    (\"frame-history\", true),\n");
    for feature in packages.iter().chain(&devices) {
        writeln!(table, "    ({feature:?}, {}),", on(feature)).unwrap();
    }
    table.push(']');
    std::fs::write(out.join("built_features.rs"), table).unwrap();
}

/// Every `packages/<name>/package.toml` of the tree this crate is built
/// from, compiled in (#1553): a command line asking for a command this
/// build has no package for is answered by the manifest that provides it
/// (`commands::absent::missing_command`), wherever the binary is copied,
/// and flint-cli's source names no package to do it. Written into
/// `OUT_DIR/package_manifests.rs` as `(file, text)` pairs, by name.
fn package_manifests() {
    use std::fmt::Write as _;
    let dir =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../packages");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut found: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("package.toml"))
        .filter(|file| file.is_file())
        .map(|file| {
            let name = file.parent().unwrap().file_name().unwrap();
            (
                format!("packages/{}/package.toml", name.to_string_lossy()),
                file,
            )
        })
        .collect();
    found.sort();
    let mut code = String::from("&[\n");
    for (shown, file) in &found {
        println!("cargo:rerun-if-changed={}", file.display());
        writeln!(code, "    ({shown:?}, include_str!({:?})),", file.display()).unwrap();
    }
    code.push(']');
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("package_manifests.rs");
    std::fs::write(out, code).unwrap();
}

/// The stack `link.exe` (`/STACK`) or MinGW's `ld` (`--stack`) reserves for
/// the main thread: what `.cargo/config.toml` gives the engine's own builds.
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
