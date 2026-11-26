//! Links `flint` with an 8 MB main-thread stack on Windows (#990),
//! compiles in the package manifests (#1553, [`package_manifests`]) and
//! writes the CLI launcher's list from them (#1554, [`launcher_list`]).
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
    let manifests = package_manifests();
    launcher_list(&manifests);
}

/// A manifest's `[build] features` and `[rust] cli`, read as the build
/// needs them (`flint_core::package` holds the rest of the format).
struct Built {
    features: Vec<String>,
    cli: Option<String>,
}

fn built(file: &std::path::Path) -> Built {
    let text = std::fs::read_to_string(file).unwrap();
    let table: toml::Table = text
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let strings = |v: Option<&toml::Value>| -> Vec<String> {
        v.and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Built {
        features: strings(table.get("build").and_then(|b| b.get("features"))),
        cli: table
            .get("rust")
            .and_then(|r| r.get("cli"))
            .and_then(toml::Value::as_str)
            .map(String::from),
    }
}

/// Whether this build of flint-cli has Cargo feature `name`.
fn has_feature(name: &str) -> bool {
    let key = format!("CARGO_FEATURE_{}", name.to_uppercase().replace('-', "_"));
    std::env::var_os(key).is_some()
}

/// The CLI launcher's list (ADR 0137 section 5, #1554), from the manifests,
/// so flint-cli's source names no package to list one:
///
/// - `OUT_DIR/stock_extensions.rs`: `stock()`'s body, the `cli::extensions()`
///   of each package whose `[rust] cli` names a crate and whose
///   `[build] features` this build has, by package folder.
/// - `OUT_DIR/package_features.rs`: `(feature, on)` for each feature of
///   this crate that a manifest's `[build] features` names, which is what
///   `flint package list --built` reads.
fn launcher_list(manifests: &[(String, std::path::PathBuf)]) {
    use std::fmt::Write as _;
    let own: toml::Table = std::fs::read_to_string("Cargo.toml")
        .unwrap()
        .parse()
        .unwrap();
    let own_features = own.get("features").and_then(toml::Value::as_table);
    let mut list = String::from(
        "{\n    #[allow(unused_mut)] // a build with no package that adds a command\n    \
         let mut list: Vec<Box<dyn CliExtension>> = Vec::new();\n",
    );
    let mut features = String::from("&[\n");
    let mut seen: Vec<String> = Vec::new();
    for (_, file) in manifests {
        let b = built(file);
        for f in &b.features {
            let ours = own_features.is_some_and(|t| t.contains_key(f));
            if ours && !seen.contains(f) {
                seen.push(f.clone());
                writeln!(features, "    ({f:?}, {}),", has_feature(f)).unwrap();
            }
        }
        let on = !b.features.is_empty() && b.features.iter().all(|f| has_feature(f));
        if let (Some(krate), true) = (&b.cli, on) {
            let ident = krate.replace('-', "_");
            writeln!(list, "    list.extend(::{ident}::cli::extensions());").unwrap();
        }
    }
    list.push_str("    list\n}\n");
    features.push(']');
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("stock_extensions.rs"), list).unwrap();
    std::fs::write(out.join("package_features.rs"), features).unwrap();
    println!("cargo:rerun-if-changed=Cargo.toml");
}

/// Every `packages/<name>/package.toml` of the tree this crate is built
/// from, compiled in (#1553): a command line asking for a command this
/// build has no package for is answered by the manifest that provides it
/// (`commands::absent::missing_command`), wherever the binary is copied,
/// and flint-cli's source names no package to do it. Written into
/// `OUT_DIR/package_manifests.rs` as `(file, text)` pairs, by name.
fn package_manifests() -> Vec<(String, std::path::PathBuf)> {
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
    found
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
