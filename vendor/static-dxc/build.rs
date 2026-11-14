//! Find Flint's static DXC archive, check it, unpack it and link it (#947).
//!
//! Where the archive comes from, in order:
//! 1. `FLINT_STATIC_DXC_ARCHIVE`: a path to the `.tar.gz`, used as it is.
//! 2. The machine-wide cache, `FLINT_STATIC_DXC_CACHE` or else
//!    `%LOCALAPPDATA%\flint\static-dxc`, as `<sha256>.tar.gz`. Every target
//!    dir and worktree on the machine shares it.
//! 3. A download into that cache, from the release asset's own URL (no
//!    GitHub API call), with `curl.exe`.
//!
//! Each path checks the pinned SHA-256 (`pin.rs`) before anything is used,
//! so the bytes linked are the ones the workflow published. With no network
//! and nothing cached the build stops, naming the URL and both variables.

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(dead_code)]
mod pin {
    include!("pin.rs");
}
use pin::*;

const ARCHIVE_ENV: &str = "FLINT_STATIC_DXC_ARCHIVE";
const CACHE_ENV: &str = "FLINT_STATIC_DXC_CACHE";
/// The library inside the archive (`flint_dxcompiler.lib`).
const LIB_NAME: &str = "flint_dxcompiler";

fn main() {
    println!("cargo:rerun-if-changed=pin.rs");
    println!("cargo:rerun-if-env-changed={ARCHIVE_ENV}");
    println!("cargo:rerun-if-env-changed={CACHE_ENV}");

    let target = env::var("TARGET").unwrap_or_default();
    if target != "x86_64-pc-windows-msvc" {
        panic!(
            "Flint's static DXC is built for x86_64-pc-windows-msvc only, not {target}. \
             wgpu-hal links it only there; build without flint-render's `static-dxc` \
             feature elsewhere."
        );
    }
    check_msvc(&target);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let archive = archive(&out_dir);
    unpack(&archive, &out_dir);

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // Not bundled into the rlib: the library is ~500 MB, and the final link
    // finds it in OUT_DIR through the search path above
    println!("cargo:rustc-link-lib=static:-bundle={LIB_NAME}");
    // What DXC calls in Windows beyond Rust's std: COM allocation and BSTRs,
    // the registry (DXC's own settings) and file version info
    for lib in ["ole32", "oleaut32", "advapi32", "version"] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}

/// The checked archive: [`ARCHIVE_ENV`]'s file, the cache's copy, or a fresh
/// download into the cache (into `OUT_DIR` when no cache folder is known).
fn archive(out_dir: &Path) -> PathBuf {
    let url = format!(
        "https://github.com/chrischaps/Flint/releases/download/{RELEASE_TAG}/{ARCHIVE_NAME}"
    );
    if let Some(path) = env::var_os(ARCHIVE_ENV).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        match sha256_file(&path) {
            Ok(hash) if hash == ARCHIVE_SHA256 => return path,
            Ok(hash) => panic!(
                "{ARCHIVE_ENV}={} has SHA-256 {hash}, not the pinned {ARCHIVE_SHA256} \
                 ({ARCHIVE_NAME}, {url})",
                path.display()
            ),
            Err(e) => panic!("{ARCHIVE_ENV}={}: {e}", path.display()),
        }
    }

    let cache = cache_dir();
    let dest = match &cache {
        Some(dir) => dir.join(format!("{ARCHIVE_SHA256}.tar.gz")),
        None => out_dir.join(ARCHIVE_NAME),
    };
    if dest.is_file() {
        if sha256_file(&dest).is_ok_and(|h| h == ARCHIVE_SHA256) {
            return dest;
        }
        // A damaged copy: fetch it again
        let _ = fs::remove_file(&dest);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("creating the static DXC cache {}: {e}", parent.display()));
    }

    // Downloaded beside its destination and moved into place only once the
    // hash matches, so a build running alongside never reads half a file
    let partial = dest.with_extension(format!("part-{}", std::process::id()));
    let fetched = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "2",
        ])
        .arg("--output")
        .arg(&partial)
        .arg(&url)
        .status();
    let ok = matches!(fetched, Ok(s) if s.success());
    if !ok {
        let _ = fs::remove_file(&partial);
        let why = match fetched {
            Ok(s) => format!("curl exited with {s}"),
            Err(e) => format!("curl didn't start: {e}"),
        };
        panic!(
            "Flint's static DXC isn't cached and couldn't be downloaded ({why}).\n\
             \n\
             It is {ARCHIVE_NAME}, SHA-256 {ARCHIVE_SHA256}, from\n  {url}\n\
             \n\
             With no network, put that file at\n  {}\n\
             or point {ARCHIVE_ENV} at a copy of it. {CACHE_ENV} moves the cache folder.",
            dest.display()
        );
    }
    let hash = sha256_file(&partial).unwrap_or_else(|e| panic!("{}: {e}", partial.display()));
    if hash != ARCHIVE_SHA256 {
        let _ = fs::remove_file(&partial);
        panic!("{url} has SHA-256 {hash}, not the pinned {ARCHIVE_SHA256}: refusing to link it");
    }
    if fs::rename(&partial, &dest).is_err() {
        // Another build put it there first; both have the pinned bytes
        let _ = fs::remove_file(&partial);
    }
    dest
}

/// [`CACHE_ENV`], else `%LOCALAPPDATA%\flint\static-dxc`.
fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os(CACHE_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("flint").join("static-dxc"))
}

/// Unpack the library and the notices into `out_dir`, once per archive.
fn unpack(archive: &Path, out_dir: &Path) {
    let stamp = out_dir.join("unpacked.sha256");
    let lib = out_dir.join(format!("{LIB_NAME}.lib"));
    if lib.is_file() && fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == ARCHIVE_SHA256) {
        return;
    }
    // Windows' own bsdtar: a GNU tar from Git reads `C:` as a remote host
    let tar = env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
        .filter(|t| t.is_file())
        .unwrap_or_else(|| PathBuf::from("tar"));
    let status = Command::new(&tar)
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(out_dir)
        .status()
        .unwrap_or_else(|e| panic!("running {}: {e}", tar.display()));
    if !status.success() || !lib.is_file() {
        panic!("{} couldn't unpack {}", tar.display(), archive.display());
    }
    fs::write(&stamp, ARCHIVE_SHA256).expect("writing the unpack stamp");
}

/// A static library built by MSVC links only with that MSVC or a newer one.
fn check_msvc(target: &str) {
    let Some(mut cl) = cc::windows_registry::find(target, "cl.exe") else {
        panic!("Flint's static DXC needs MSVC (cl.exe) to link; install Visual Studio's C++ tools");
    };
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let probe = out_dir.join("msc_ver.c");
    fs::write(&probe, "_MSC_VER\n").expect("writing the MSVC probe");
    let output = cl.arg("/nologo").arg("/EP").arg(&probe).output();
    let found = output
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.split_whitespace()
                .last()
                .and_then(|v| v.parse::<u32>().ok())
        });
    match found {
        Some(v) if v >= BUILT_WITH_MSC_VER => {}
        Some(v) => panic!(
            "Flint's static DXC ({DXC_VERSION}) was built with MSVC _MSC_VER {BUILT_WITH_MSC_VER} \
             and links only with that or newer; this machine's cl.exe is {v}. Update Visual \
             Studio's C++ tools."
        ),
        None => println!("cargo:warning=couldn't read cl.exe's _MSC_VER; linking anyway"),
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
