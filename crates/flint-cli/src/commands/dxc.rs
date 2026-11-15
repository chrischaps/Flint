//! `flint dxc`: DX12's shader compiler, linked in (#912, #947).
//!
//! ```bash
//! flint dxc status              # what DX12 compiles shaders with here, and why
//! flint dxc notices --to dist/  # DXC's licence and notices, for a Windows build that ships
//! ```
//!
//! Flint links Microsoft's DXC into its binaries on x86_64 Windows with
//! MSVC (flint-render's default `static-dxc` feature, `vendor/static-dxc`),
//! built from Microsoft's own source by Flint's workflow (`tools/dxc-static`).
//! No `dxcompiler.dll` or `dxil.dll` ships, so there's nothing to install:
//! `flint dxc install` and `FLINT_DXC_DIR` are gone (#947). What a Windows
//! build that carries Flint must still ship is DXC's licence and
//! third-party notices: `flint dxc notices` writes them.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use flint_render::gpu_select;
use std::path::PathBuf;

/// The file `flint dxc notices --to` writes.
pub const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES-DXC.txt";

#[derive(Subcommand)]
pub enum DxcCommands {
    /// What DX12 would compile shaders with here, and why
    Status,
    /// Write DXC's licence and third-party notices (THIRD-PARTY-NOTICES-DXC.txt),
    /// which a Windows build carrying Flint ships beside its executable
    Notices(NoticesArgs),
}

#[derive(Args)]
pub struct NoticesArgs {
    /// The folder to write THIRD-PARTY-NOTICES-DXC.txt into. Default: print it
    #[arg(long, value_name = "DIR")]
    pub to: Option<PathBuf>,
}

pub fn run(cmd: DxcCommands) -> Result<()> {
    match cmd {
        DxcCommands::Status => {
            status();
            Ok(())
        }
        DxcCommands::Notices(args) => notices(args),
    }
}

fn status() {
    match gpu_select::static_dxc() {
        Some(linked) => {
            println!(
                "linked: DXC {} (microsoft/DirectXShaderCompiler {}), from release {}",
                linked.version, linked.commit, linked.release
            );
            for line in linked.build_info.lines().skip(1) {
                println!("  {line}");
            }
        }
        None => println!(
            "linked: no (flint-render's `static-dxc` links DXC on x86_64 Windows with MSVC only)"
        ),
    }
    if let Ok(asked) = std::env::var("WGPU_DX12_COMPILER") {
        println!("WGPU_DX12_COMPILER={asked}");
    }
    println!("DX12 shaders: {}", gpu_select::dx12_shaders());
}

fn notices(args: NoticesArgs) -> Result<()> {
    let Some(linked) = gpu_select::static_dxc() else {
        bail!("this build doesn't link DXC, so it carries no DXC notices to ship");
    };
    let text = notices_text(linked.version, linked.commit, linked.notices);
    match args.to {
        None => print!("{text}"),
        Some(dir) => {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let path = dir.join(NOTICES_FILE);
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

/// The notices file: what it covers, then DXC's own two files.
fn notices_text(version: &str, commit: &str, notices: &str) -> String {
    format!(
        "This program includes the DirectX Shader Compiler (DXC) {version}, linked into\n\
         its executable, built from https://github.com/microsoft/DirectXShaderCompiler\n\
         at {commit}. DXC's licence and third-party notices follow.\n\
         \n\
         ==============================================================================\n\
         \n\
         {notices}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notices_name_the_source_and_carry_the_licence() {
        let text = notices_text("v1.9.2609", "01b62ad4", "University of Illinois/NCSA");
        assert!(text.starts_with("This program includes the DirectX Shader Compiler"));
        assert!(text.contains("v1.9.2609") && text.contains("01b62ad4"));
        assert!(text.ends_with("University of Illinois/NCSA"));
    }

    // A default build on x86_64 Windows with MSVC links DXC, so it has both
    // of DXC's files to ship (#947)
    #[test]
    #[cfg(all(windows, target_arch = "x86_64", target_env = "msvc"))]
    fn a_windows_build_has_dxcs_notices() {
        let linked = gpu_select::static_dxc().expect("flint-render's static-dxc is on");
        assert!(linked.notices.contains("University of Illinois/NCSA"));
        assert!(linked
            .notices
            .contains("THIRD-PARTY SOFTWARE NOTICES AND INFORMATION"));
    }
}
