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
