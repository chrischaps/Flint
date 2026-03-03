//! `flint gen` — run a procedural generation spec and write the output.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;
use flint_procgen::{
    export_glb, GeneratorOutput, GeneratorRegistry, ProcGenSpec, SeedConfig, SeedMode,
};

#[derive(Args)]
pub struct GenArgs {
    /// Path to a .procgen.toml spec file
    pub spec: String,

    /// Output path (file or directory). Derived from spec name if omitted.
    #[arg(short, long)]
    pub output: Option<String>,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Print estimated cost without generating
    #[arg(long)]
    pub dry_run: bool,

    /// Force output format (auto-detected from extension by default)
    #[arg(long, value_parser = parse_format)]
    pub format: Option<String>,
}

fn parse_format(s: &str) -> std::result::Result<String, String> {
    match s {
        "glb" | "png" => Ok(s.to_string()),
        _ => Err(format!("unsupported format '{s}'; valid values: glb, png")),
    }
}

pub fn run(args: GenArgs) -> Result<()> {
    let spec_path = Path::new(&args.spec);
    let mut spec = ProcGenSpec::from_file(spec_path)
        .with_context(|| format!("failed to load spec from {}", spec_path.display()))?;

    // Seed override
    if let Some(seed_val) = args.seed {
        spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(seed_val),
            derive_from: None,
        };
    }

    // Build registry
    let mut registry = GeneratorRegistry::new();
    flint_procgen::register_built_in_generators(&mut registry);

    // Dry-run: print cost estimate and return
    if args.dry_run {
        let generator = registry
            .get(&spec.generator)
            .with_context(|| format!("unknown generator '{}'", spec.generator))?;
        let cost = generator.estimate_cost(&spec);
        println!("Spec:       {}", spec.meta.name);
        println!("Generator:  {}", spec.generator);
        println!("Seed:       {:?}", spec.seed.mode);
        println!();
        println!("Estimated cost:");
        println!(
            "  Vertices:      {}",
            format_count(cost.estimated_vertices as u64)
        );
        println!(
            "  Triangles:     {}",
            format_count(cost.estimated_triangles as u64)
        );
        println!(
            "  Texture bytes: {}",
            format_bytes(cost.estimated_texture_bytes)
        );
        println!("  Gen time:      {:.1} ms", cost.estimated_generation_ms);
        return Ok(());
    }

    // Generate
    let start = Instant::now();
    let output = registry
        .generate_from_spec(&spec)
        .with_context(|| format!("generation failed for '{}'", spec.meta.name))?;
    let gen_time = start.elapsed();

    // Route output
    match &output {
        GeneratorOutput::Mesh(mesh) => {
            let out_path = resolve_output_path(&args, &spec, "glb")?;
            let glb = export_glb::mesh_to_glb(mesh).context("GLB export failed")?;
            let file_size = glb.len();
            std::fs::write(&out_path, glb)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            println!("Output:     {}", out_path.display());
            println!("Format:     GLB");
            println!(
                "Vertices:   {}",
                format_count(mesh.vertex_count() as u64)
            );
            println!(
                "Triangles:  {}",
                format_count(mesh.triangle_count() as u64)
            );
            println!("File size:  {}", format_bytes(file_size as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::MeshWithLods(lods) => {
            let out_path = resolve_output_path(&args, &spec, "glb")?;
            let glb =
                export_glb::mesh_lods_to_glb(lods).context("GLB export (LODs) failed")?;
            let file_size = glb.len();
            std::fs::write(&out_path, glb)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            let total_verts: usize = lods.iter().map(|m| m.vertex_count()).sum();
            let total_tris: usize = lods.iter().map(|m| m.triangle_count()).sum();
            println!("Output:     {}", out_path.display());
            println!("Format:     GLB ({} LOD levels)", lods.len());
            println!("Vertices:   {} (total)", format_count(total_verts as u64));
            println!("Triangles:  {} (total)", format_count(total_tris as u64));
            println!("File size:  {}", format_bytes(file_size as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::Image(img) => {
            let out_path = resolve_output_path(&args, &spec, "png")?;
            img.save_png(&out_path)
                .with_context(|| format!("failed to save PNG to {}", out_path.display()))?;
            let file_size = std::fs::metadata(&out_path)?.len();
            println!("Output:     {}", out_path.display());
            println!("Format:     PNG");
            println!("Dimensions: {}x{}", img.width, img.height);
            println!(
                "Channel:    {:?}",
                img.channel_semantics
            );
            println!("File size:  {}", format_bytes(file_size));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::ImageSet(images) => {
            let base_path = resolve_output_path(&args, &spec, "png")?;
            let stem = base_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let parent = base_path.parent().unwrap_or(Path::new("."));
            let mut total_size: u64 = 0;
            for img in images {
                let suffix = format!("{:?}", img.channel_semantics).to_lowercase();
                let file_name = format!("{stem}_{suffix}.png");
                let img_path = parent.join(&file_name);
                img.save_png(&img_path)
                    .with_context(|| format!("failed to save {}", img_path.display()))?;
                let size = std::fs::metadata(&img_path)?.len();
                total_size += size;
                println!(
                    "  {} ({}x{}, {})",
                    img_path.display(),
                    img.width,
                    img.height,
                    format_bytes(size)
                );
            }
            println!();
            println!("Images:     {}", images.len());
            println!("Total size: {}", format_bytes(total_size));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::Sound(bytes) => {
            let out_path = resolve_output_path(&args, &spec, "wav")?;
            std::fs::write(&out_path, bytes)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            println!("Output:     {}", out_path.display());
            println!("Format:     WAV (raw)");
            println!("File size:  {}", format_bytes(bytes.len() as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
    }

    Ok(())
}

/// Determine the output file path from CLI args, spec name, and default extension.
fn resolve_output_path(args: &GenArgs, spec: &ProcGenSpec, default_ext: &str) -> Result<PathBuf> {
    if let Some(ref out) = args.output {
        return Ok(PathBuf::from(out));
    }

    // Derive from format flag or default extension
    let ext = args.format.as_deref().unwrap_or(default_ext);
    let name = spec
        .meta
        .name
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");
    Ok(PathBuf::from(format!("{name}.{ext}")))
}

/// Human-friendly byte size.
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Human-friendly count with commas.
fn format_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
