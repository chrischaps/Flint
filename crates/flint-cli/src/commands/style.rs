//! `flint style new | check | prelude | list` (#416): start a user style
//! from a template, check one without a GPU, see the code it is compiled
//! inside, and list what a scene can switch on.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use flint_core::project::shown;
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
    /// A scene (or the folder holding it) whose styles/ to include (the
    /// global --json prints the list as JSON)
    pub scene: Option<PathBuf>,
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
                bail!("{} is already there (--force overwrites it)", shown(path));
            }
        }
    }
    std::fs::create_dir_all(&args.dir).with_context(|| format!("creating {}", shown(&args.dir)))?;
    std::fs::write(&toml_path, &files.toml)
        .with_context(|| format!("writing {}", shown(&toml_path)))?;
    std::fs::write(&wgsl_path, &files.wgsl)
        .with_context(|| format!("writing {}", shown(&wgsl_path)))?;
    // What it is now: the template checks, so this is the first green light
    custom::check_file(&toml_path).map_err(anyhow::Error::msg)?;
    let next = [
        format!("flint style check {}", shown(&toml_path)),
        format!("flint render <scene> --style {name}:amount=1"),
    ];
    let written = [shown(&toml_path), shown(&wgsl_path)];
    let text = format!(
        "wrote {}\nwrote {}\nok: {name} checks. Next:\n  {}\n  {}",
        shown(&toml_path),
        shown(&wgsl_path),
        next[0],
        next[1]
    );
    crate::output::emit(&crate::output::Answer::new(
        text,
        serde_json::json!({ "name": name, "written": written, "next": next }),
    ))
}

/// The `.shader.toml` files `paths` names: files as given, folders' own
/// (not their subfolders'), sorted.
fn style_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .with_context(|| format!("reading {}", shown(path)))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .is_some_and(|f| f.ends_with(custom::SUFFIX))
                })
                .collect();
            if found.is_empty() {
                bail!("{}: no *{} files", shown(path), custom::SUFFIX);
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
    let mut checked = Vec::new();
    for file in &files {
        checked.push(match custom::check_file(file) {
            Ok(report) => {
                for warning in &report.warnings {
                    eprintln!("{warning}");
                }
                crate::output::say!("ok: {}", shown(file));
                checked_json(file, Ok(&report))
            }
            Err(e) => {
                eprintln!("error: {e}");
                failed += 1;
                checked_json(file, Err(&e))
            }
        });
    }
    crate::output::when_json(
        || serde_json::json!({ "valid": failed == 0, "failed": failed, "styles": checked }),
    );
    if failed > 0 {
        eprintln!("{failed} of {} styles failed", files.len());
        return Err(crate::output::failed());
    }
    Ok(())
}

/// One style's entry in `flint style check --json` (#567): what the check
/// counted when it compiles, each error in parts when it doesn't (the
/// text as `error` too, as a host shows it).
fn checked_json(file: &Path, result: Result<&custom::Report, &String>) -> serde_json::Value {
    match result {
        Ok(report) => serde_json::json!({
            "file": shown(file),
            "ok": true,
            "loops": report.loops,
            "bounded_loops": report.bounded,
            "texture_reads": report.texture_reads,
            "warnings": report.warnings,
            "errors": [],
        }),
        Err(e) => serde_json::json!({
            "file": shown(file),
            "ok": false,
            "loops": null,
            "bounded_loops": null,
            "texture_reads": null,
            "warnings": [],
            "errors": custom::error_parts(e, &shown(file)),
            "error": e,
        }),
    }
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
                    shown(&args.file),
                    names.join(", ")
                )
            })?,
    };
    let text = if args.composed {
        let path = &source.passes[pass].wgsl_path;
        let user =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", shown(path)))?;
        custom::compose_pass(&source, pass, &user).text
    } else {
        custom::prelude_pass(&source, pass)
    };
    if crate::output::json() {
        crate::output::document(&serde_json::json!({
            "file": shown(&args.file),
            "pass": source.passes[pass].name,
            "composed": args.composed,
            "wgsl": text,
        }));
    } else {
        print!("{text}");
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
    // The styles and choices a scene there sees, by its project's one
    // asset search (ADR 0114); with no scene, the working directory's
    let assets = flint_core::project::AssetScope::for_dir(scene_dir.unwrap_or(Path::new(".")));
    let mut styles = flint_render::stylize::StyleRegistry::new();
    if scene_dir.is_some() {
        for message in styles.load_for(&assets) {
            eprintln!("warning: {message}");
        }
    }
    if crate::output::json() {
        crate::output::document(&serde_json::to_value(catalog::catalog_in(
            &styles, &assets,
        ))?);
    } else {
        print!("{}", catalog::catalog_text_in(&styles, &assets));
    }
    Ok(())
}
