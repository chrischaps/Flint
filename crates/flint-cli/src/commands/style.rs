//! `flint style new | check | prelude | list` (#416): start a user style
//! from a template, check one without a GPU, see the code it is compiled
//! inside, and list what a scene can switch on.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use flint_render::stylize::{catalog, custom, StyleStage};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum StyleCommands {
    /// Write `<name>.shader.toml` and `<name>.wgsl` from a template: a
    /// commented passthrough listing every field and helper its inputs
    /// give, with one example parameter
    New(NewArgs),
    /// Check user styles the way a host loads them, without a GPU: print
    /// each error and warning, exit 1 if any style has an error
    Check(CheckArgs),
    /// Print the generated code a style's WGSL is compiled inside
    Prelude(PreludeArgs),
    /// List every style a scene can use, built-in and its own (the same
    /// list as `flint render --list-styles`)
    List(ListArgs),
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Stage {
    /// The finished picture, after tonemapping, at output resolution
    Display,
    /// Linear light before fog, bloom and the grade, at scene resolution
    Hdr,
}

#[derive(Args)]
pub struct NewArgs {
    /// The style's name: lowercase letters, digits and _
    pub name: String,
    /// Where it draws
    #[arg(long, value_enum, default_value = "display")]
    pub stage: Stage,
    /// The buffers it reads besides colour: depth, mask, edges
    #[arg(long, value_delimiter = ',')]
    pub inputs: Vec<String>,
    /// The folder to write into (a game's styles/, beside levels/)
    #[arg(long, default_value = "styles")]
    pub dir: PathBuf,
    /// Overwrite files that are already there
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct CheckArgs {
    /// `.shader.toml` files, or folders to check every one in
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
}

#[derive(Args)]
pub struct PreludeArgs {
    /// The style's `.shader.toml`
    pub file: PathBuf,
    /// Print the whole module wgpu compiles: prelude, the style's WGSL and
    /// the entry points
    #[arg(long)]
    pub composed: bool,
    /// For a style of several `[[passes]]` (#412), the pass to print, by
    /// name (`pass1`, `pass2`... for unnamed ones); the last by default
    #[arg(long)]
    pub pass: Option<String>,
}

#[derive(Args)]
pub struct ListArgs {
    /// A scene (or the folder holding it) whose styles/ to include
    pub scene: Option<PathBuf>,
    /// Print the list as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(cmd: StyleCommands) -> Result<()> {
    match cmd {
        StyleCommands::New(args) => new(args),
        StyleCommands::Check(args) => check(args),
        StyleCommands::Prelude(args) => prelude(args),
        StyleCommands::List(args) => list(args),
    }
}

fn parse_inputs(names: &[String]) -> Result<custom::Inputs> {
    let mut inputs = custom::Inputs::default();
    for name in names {
        match name.trim() {
            "" | "color" => {}
            "depth" => inputs.depth = true,
            "mask" => inputs.mask = true,
            "edges" => inputs.edges = true,
            other => bail!("unknown input \"{other}\"; inputs are depth, mask, edges"),
        }
    }
    Ok(inputs)
}

fn new(args: NewArgs) -> Result<()> {
    let stage = match args.stage {
        Stage::Display => StyleStage::Display,
        Stage::Hdr => StyleStage::Hdr,
    };
    let inputs = parse_inputs(&args.inputs)?;
    let name = &args.name;
    let toml_path = args.dir.join(format!("{name}{}", custom::SUFFIX));
    let wgsl_path = args.dir.join(format!("{name}.wgsl"));
    let files = custom::template(name, stage, inputs);
    // The description's own checks (name, built-ins) before anything lands
    custom::parse_source(&files.toml, &toml_path).map_err(anyhow::Error::msg)?;
    if !args.force {
        for path in [&toml_path, &wgsl_path] {
            if path.exists() {
                bail!(
                    "{} is already there (--force overwrites it)",
                    path.display()
                );
            }
        }
    }
    std::fs::create_dir_all(&args.dir)
        .with_context(|| format!("creating {}", args.dir.display()))?;
    std::fs::write(&toml_path, &files.toml)
        .with_context(|| format!("writing {}", toml_path.display()))?;
    std::fs::write(&wgsl_path, &files.wgsl)
        .with_context(|| format!("writing {}", wgsl_path.display()))?;
    println!("wrote {}", toml_path.display());
    println!("wrote {}", wgsl_path.display());
    // What it is now: the template checks, so this is the first green light
    custom::check_file(&toml_path).map_err(anyhow::Error::msg)?;
    println!("ok: {name} checks. Next:");
    println!("  flint style check {}", toml_path.display());
    println!("  flint render <scene> --style {name}:amount=1");
    Ok(())
}

/// The `.shader.toml` files `paths` names: files as given, folders' own
/// (not their subfolders'), sorted.
fn style_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .with_context(|| format!("reading {}", path.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .is_some_and(|f| f.ends_with(custom::SUFFIX))
                })
                .collect();
            if found.is_empty() {
                bail!("{}: no *{} files", path.display(), custom::SUFFIX);
            }
            found.sort();
            files.extend(found);
        } else {
            files.push(path.clone());
        }
    }
    Ok(files)
}

fn check(args: CheckArgs) -> Result<()> {
    let files = style_files(&args.paths)?;
    let mut failed = 0;
    for file in &files {
        match custom::check_file(file) {
            Ok(report) => {
                for warning in &report.warnings {
                    eprintln!("{warning}");
                }
                println!("ok: {}", file.display());
            }
            Err(e) => {
                eprintln!("error: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} of {} styles failed", files.len());
        std::process::exit(1);
    }
    Ok(())
}

fn prelude(args: PreludeArgs) -> Result<()> {
    let source = custom::read_source(&args.file).map_err(anyhow::Error::msg)?;
    let pass = match &args.pass {
        None => source.last_pass(),
        Some(name) => source
            .passes
            .iter()
            .position(|p| &p.name == name)
            .ok_or_else(|| {
                let names: Vec<&str> = source.passes.iter().map(|p| p.name.as_str()).collect();
                anyhow::anyhow!(
                    "{}: no pass \"{name}\" (its passes: {})",
                    args.file.display(),
                    names.join(", ")
                )
            })?,
    };
    if args.composed {
        let path = &source.passes[pass].wgsl_path;
        let user =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        print!("{}", custom::compose_pass(&source, pass, &user).text);
    } else {
        print!("{}", custom::prelude_pass(&source, pass));
    }
    Ok(())
}

fn list(args: ListArgs) -> Result<()> {
    let scene_dir: Option<&Path> = args.scene.as_deref().map(|p| {
        if p.is_dir() {
            p
        } else {
            p.parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        }
    });
    if let Some(dir) = scene_dir {
        for message in custom::load_dir(dir) {
            eprintln!("warning: {message}");
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&catalog::catalog_for(scene_dir))?
        );
    } else {
        print!("{}", catalog::catalog_text_for(scene_dir));
    }
    Ok(())
}
