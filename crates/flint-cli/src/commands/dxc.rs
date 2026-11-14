//! `flint dxc`: DX12's fast shader compiler beside the binaries (#912).
//!
//! ```bash
//! flint dxc install                 # the pinned release, beside this flint.exe (and its deps/)
//! flint dxc install --to dist/win   # beside a game's executable, with the licence files
//! flint dxc install --sdk           # the Windows SDK's copy instead of the download
//! flint dxc install --from <dir>    # any folder holding dxcompiler.dll and dxil.dll
//! flint dxc status                  # what DX12 would compile with here, and why
//! ```
//!
//! wgpu loads DXC at run time (`Dx12Compiler::DynamicDxc`, by
//! `dxcompiler.dll`'s path; `dxil.dll` signs what it compiles), and
//! `flint_render::gpu_select` loads both only from the executable's folder
//! (or `FLINT_DXC_DIR`). Without them DX12 falls back to FXC, about 10x
//! slower. This command puts them there:
//!
//! - **The source** is the pinned DirectXShaderCompiler release, [`PINNED`]:
//!   downloaded once into a fresh, empty folder of the cache
//!   (`%LOCALAPPDATA%\flint\dxc\<version>`), the zip's SHA-256 checked, the
//!   two x64 DLLs and the licence files unpacked with Windows' own
//!   `tar.exe`, and each DLL's SHA-256 checked again. `--sdk` takes the
//!   newest Windows SDK's copy; `--from` any folder (neither is checked
//!   against a hash: it's the user's choice of file).
//! - **The destinations** are `--to` folders, else this executable's folder
//!   and its `deps/` when there is one (a cargo profile folder: `flint.exe`
//!   and `flint-player.exe` load from `target/<profile>/`, test binaries from
//!   `target/<profile>/deps/`). A file already there with the same bytes is
//!   left alone, so a running process holding it doesn't stop the copy.
//!
//! The DLLs are never committed. Shipping them in a game is the game's
//! choice: the licence's terms are in the book (GPU Selection).

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The DXC release `flint dxc install` downloads: a fixed version, the
/// zip's SHA-256 (as GitHub's release lists it) and each x64 DLL's.
pub struct Pinned {
    pub version: &'static str,
    pub url: &'static str,
    pub zip_sha256: &'static str,
    /// `(name, path in the zip, SHA-256)`.
    pub dlls: [(&'static str, &'static str, &'static str); 2],
    /// Licence files, at the zip's top level, copied beside the DLLs with a
    /// `dxc-` prefix so a game's folder can tell them from its own.
    pub licences: [&'static str; 3],
}

pub const PINNED: Pinned = Pinned {
    version: "v1.9.2609",
    url: "https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2609/dxc_2026_09_29.zip",
    zip_sha256: "ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1",
    dlls: [
        (
            "dxcompiler.dll",
            "bin/x64/dxcompiler.dll",
            "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
        ),
        (
            "dxil.dll",
            "bin/x64/dxil.dll",
            "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
        ),
    ],
    licences: ["LICENSE-MS.txt", "LICENSE-LLVM.txt", "LICENCE-MIT.txt"],
};

#[derive(Subcommand)]
pub enum DxcCommands {
    /// Put dxcompiler.dll and dxil.dll beside the binaries, so DX12
    /// compiles shaders with DXC instead of FXC
    Install(InstallArgs),
    /// What DX12 would compile shaders with here, and why
    Status,
}

#[derive(Args)]
pub struct InstallArgs {
    /// A folder to install into (repeatable). Default: this flint
    /// executable's folder, and its deps/ when there is one
    #[arg(long, value_name = "DIR")]
    pub to: Vec<PathBuf>,

    /// Take the DLLs from this folder instead of the pinned release
    #[arg(long, value_name = "DIR", conflicts_with = "sdk")]
    pub from: Option<PathBuf>,

    /// Take the DLLs from the newest installed Windows SDK
    /// (Windows Kits\10\bin\<version>\x64) instead of the pinned release
    #[arg(long)]
    pub sdk: bool,
}

pub fn run(cmd: DxcCommands) -> Result<()> {
    match cmd {
        DxcCommands::Install(args) => install(args),
        DxcCommands::Status => {
            status();
            Ok(())
        }
    }
}

fn status() {
    println!("pinned release: {} ({})", PINNED.version, PINNED.url);
    match flint_render::gpu_select::dxc_dir() {
        Some(dir) => {
            let from = if std::env::var_os("FLINT_DXC_DIR").is_some_and(|v| !v.is_empty()) {
                "FLINT_DXC_DIR"
            } else {
                "the executable's folder"
            };
            println!("looks in: {} ({from})", dir.display());
            for (name, _, _) in PINNED.dlls {
                let path = dir.join(name);
                match sha256_file(&path) {
                    Ok(hash) => println!("  {name}: {}", describe_hash(name, &hash)),
                    Err(_) => println!("  {name}: missing"),
                }
            }
        }
        None => println!("looks in: nowhere (the executable's folder is unknown)"),
    }
    println!("DX12 shaders: {}", flint_render::gpu_select::dx12_shaders());
}

/// `pinned v1.9.2609` when `hash` is the pinned DLL's, else the hash.
fn describe_hash(name: &str, hash: &str) -> String {
    match PINNED.dlls.iter().find(|(n, _, _)| *n == name) {
        Some((_, _, pinned)) if *pinned == hash => format!("pinned {}", PINNED.version),
        _ => format!("sha256 {hash} (not the pinned release's)"),
    }
}

fn install(args: InstallArgs) -> Result<()> {
    let (source, what) = if let Some(dir) = &args.from {
        (dir.clone(), format!("{}", dir.display()))
    } else if args.sdk {
        let dir = newest_sdk_dir(&sdk_roots()).context(
            "no Windows SDK with dxcompiler.dll and dxil.dll under Windows Kits\\10\\bin\\<version>\\x64",
        )?;
        let what = format!("the Windows SDK, {}", dir.display());
        (dir, what)
    } else {
        let dir = pinned_release()?;
        let what = format!("release {}, {}", PINNED.version, dir.display());
        (dir, what)
    };
    for (name, _, _) in PINNED.dlls {
        if !source.join(name).is_file() {
            bail!("{name} is not in {}", source.display());
        }
    }

    let targets = if args.to.is_empty() {
        let exe = std::env::current_exe().context("finding this executable's folder")?;
        default_targets(exe.parent().context("this executable has no folder")?)
    } else {
        args.to.clone()
    };

    println!("DXC from {what}");
    let licences: Vec<&str> = PINNED
        .licences
        .iter()
        .copied()
        .filter(|l| source.join(l).is_file())
        .collect();
    if licences.is_empty() {
        println!(
            "  (no licence files there: the release zip has them, `flint dxc install` without --from/--sdk copies them)"
        );
    }
    for dir in &targets {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut copied = Vec::new();
        for (name, _, _) in PINNED.dlls {
            if copy_if_changed(&source.join(name), &dir.join(name))? {
                copied.push(name.to_string());
            }
        }
        for licence in &licences {
            let to = format!("dxc-{licence}");
            if copy_if_changed(&source.join(licence), &dir.join(&to))? {
                copied.push(to);
            }
        }
        if copied.is_empty() {
            println!("  {}: already there", dir.display());
        } else {
            println!("  {}: {}", dir.display(), copied.join(", "));
        }
    }
    Ok(())
}

/// Where a bare `flint dxc install` puts the DLLs: `exe_dir`, and its
/// `deps/` when there is one (test binaries in a cargo profile folder).
fn default_targets(exe_dir: &Path) -> Vec<PathBuf> {
    let mut targets = vec![exe_dir.to_path_buf()];
    let deps = exe_dir.join("deps");
    if deps.is_dir() {
        targets.push(deps);
    }
    targets
}

/// Copy `from` to `to` unless `to` already has the same bytes. True when
/// it copied.
fn copy_if_changed(from: &Path, to: &Path) -> Result<bool> {
    if to.is_file() && sha256_file(from)? == sha256_file(to)? {
        return Ok(false);
    }
    std::fs::copy(from, to).with_context(|| {
        format!(
            "copying {} to {} (is a Flint process running from there?)",
            from.display(),
            to.display()
        )
    })?;
    Ok(true)
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `Windows Kits\10\bin` under both Program Files folders.
fn sdk_roots() -> Vec<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|p| PathBuf::from(p).join("Windows Kits").join("10").join("bin"))
        .collect()
}

/// The newest `<root>/<10.0.x.y>/x64` holding both DLLs.
fn newest_sdk_dir(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut found: Vec<(Vec<u32>, PathBuf)> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(version) = sdk_version(&name) else {
                continue;
            };
            let dir = entry.path().join("x64");
            if PINNED.dlls.iter().all(|(n, _, _)| dir.join(n).is_file()) {
                found.push((version, dir));
            }
        }
    }
    found
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, d)| d)
}

/// `10.0.26100.0` -> `[10, 0, 26100, 0]`; anything else (`x64`) -> None.
fn sdk_version(name: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = name.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() == 4)
}

/// The cache folder holding the pinned release's DLLs, filled on first use.
fn pinned_release() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA is not set: use --sdk or --from <dir>")?;
    let dir = base.join("flint").join("dxc").join(PINNED.version);
    if pinned_dlls_ok(&dir) {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    // A fresh, empty folder for the download, and nothing in it is used
    // until its hash matches
    let work = dir.join(format!("download-{}", std::process::id()));
    if work.exists() {
        std::fs::remove_dir_all(&work)?;
    }
    std::fs::create_dir(&work)?;
    let result = download_and_unpack(&work, &dir);
    let _ = std::fs::remove_dir_all(&work);
    result?;
    Ok(dir)
}

fn pinned_dlls_ok(dir: &Path) -> bool {
    PINNED
        .dlls
        .iter()
        .all(|(name, _, hash)| sha256_file(&dir.join(name)).is_ok_and(|h| h == *hash))
}

fn download_and_unpack(work: &Path, dir: &Path) -> Result<()> {
    println!("downloading {}", PINNED.url);
    let zip = work.join("dxc.zip");
    let bytes = ureq::get(PINNED.url)
        .call()
        .with_context(|| format!("downloading {}", PINNED.url))?
        .body_mut()
        .with_config()
        .limit(128 * 1024 * 1024)
        .read_to_vec()
        .context("reading the release zip")?;
    let hash = sha256_hex(&bytes);
    if hash != PINNED.zip_sha256 {
        bail!(
            "{} has SHA-256 {hash}, not the pinned {}: not using it",
            PINNED.url,
            PINNED.zip_sha256
        );
    }
    std::fs::write(&zip, &bytes)?;

    // Windows' own bsdtar reads zips; Git's GNU tar on PATH doesn't
    let tar = std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
        .filter(|t| t.is_file())
        .context("no %SystemRoot%\\System32\\tar.exe to unpack the zip with")?;
    let unpacked = work.join("x");
    std::fs::create_dir(&unpacked)?;
    let mut members: Vec<&str> = PINNED.dlls.iter().map(|(_, m, _)| *m).collect();
    members.extend(PINNED.licences);
    let status = std::process::Command::new(&tar)
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unpacked)
        .args(&members)
        .status()
        .with_context(|| format!("running {}", tar.display()))?;
    if !status.success() {
        bail!("{} couldn't unpack {}", tar.display(), zip.display());
    }
    for (name, member, pinned) in PINNED.dlls {
        let path = unpacked.join(member);
        let hash = sha256_file(&path)?;
        if hash != pinned {
            bail!("{member} in the release has SHA-256 {hash}, not the pinned {pinned}");
        }
        std::fs::copy(&path, dir.join(name))?;
    }
    for licence in PINNED.licences {
        std::fs::copy(unpacked.join(licence), dir.join(licence))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_versions_read_as_numbers_and_the_newest_wins() {
        assert_eq!(sdk_version("10.0.26100.0"), Some(vec![10, 0, 26100, 0]));
        assert_eq!(sdk_version("x64"), None);
        assert_eq!(sdk_version("10.0"), None);
        let root = std::env::temp_dir().join(format!("flint-dxc-sdk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (version, both) in [
            ("10.0.17134.0", true),
            ("10.0.9999.0", true),
            ("10.0.26100.0", false),
        ] {
            let x64 = root.join(version).join("x64");
            std::fs::create_dir_all(&x64).unwrap();
            std::fs::write(x64.join("dxcompiler.dll"), b"x").unwrap();
            if both {
                std::fs::write(x64.join("dxil.dll"), b"x").unwrap();
            }
        }
        // 26100 has only one of the two; 17134 beats 9999 as a number
        assert_eq!(
            newest_sdk_dir(std::slice::from_ref(&root)),
            Some(root.join("10.0.17134.0").join("x64"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_profile_folder_also_gets_its_deps() {
        let root = std::env::temp_dir().join(format!("flint-dxc-targets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(default_targets(&root), vec![root.clone()]);
        std::fs::create_dir(root.join("deps")).unwrap();
        assert_eq!(
            default_targets(&root),
            vec![root.clone(), root.join("deps")]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_pin_names_the_x64_dlls_gpu_select_loads() {
        let names: Vec<&str> = PINNED.dlls.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(names, flint_render::gpu_select::DXC_DLLS);
        for (_, member, hash) in PINNED.dlls {
            assert!(member.starts_with("bin/x64/"));
            assert_eq!(hash.len(), 64);
        }
        assert!(PINNED.url.contains(PINNED.version));
    }
}
