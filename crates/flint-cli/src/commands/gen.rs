//! `flint gen` — run a procedural generation spec and write the output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;
use flint_asset::{AssetFile, AssetMeta, AssetType, ContentStore};
use flint_core::ContentHash;
use flint_asset_gen::StyleGuide;
use flint_procgen::{
    export_glb, GeneratorOutput, GeneratorRegistry, OutputKind, ProcGenSpec, SeedConfig, SeedMode,
    ValidationConstraints,
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

    /// Number of variants to generate with sequential seeds
    #[arg(long)]
    pub batch: Option<u32>,

    /// Starting seed for batch generation (default: 0)
    #[arg(long)]
    pub seed_start: Option<u64>,

    /// Register output(s) in the content store with provenance metadata
    #[arg(long)]
    pub register: bool,

    /// Regenerate even if cached in content store
    #[arg(long)]
    pub force: bool,

    /// Validate output after generation
    #[arg(long)]
    pub validate: bool,

    /// Treat warnings as failures (exit code 1)
    #[arg(long)]
    pub strict: bool,

    /// Style guide TOML path for additional validation constraints
    #[arg(long)]
    pub style_guide: Option<String>,
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

    // Compute spec hash once if registration is enabled
    let spec_hash = if args.register {
        Some(
            ContentHash::from_file(spec_path)
                .with_context(|| format!("failed to hash spec file {}", spec_path.display()))?,
        )
    } else {
        None
    };

    // Batch mode
    if let Some(batch_count) = args.batch {
        let seed_start = args.seed_start.unwrap_or(0);
        let mut generated = 0u32;
        let mut skipped = 0u32;
        let mut validation_failed = false;

        for i in 0..batch_count {
            let seed = seed_start + i as u64;
            let batch_spec = spec_with_seed(&spec, seed);
            let out_path = resolve_output_path_for_seed(&args, &batch_spec, seed)?;

            // Check dedup cache
            if args.register && !args.force {
                if let Some(ref sh) = spec_hash {
                    if check_cache_hit(&out_path, sh, seed)? {
                        println!(
                            "[{}/{}] Cached: {} (spec+seed unchanged)",
                            i + 1,
                            batch_count,
                            out_path.display()
                        );
                        skipped += 1;
                        continue;
                    }
                }
            }

            println!(
                "[{}/{}] Generating seed {}...",
                i + 1,
                batch_count,
                seed
            );

            let output = generate_one(&batch_spec, &registry, &out_path, args.format.as_deref())?;
            generated += 1;

            // Register in content store
            if args.register {
                if let Some(ref sh) = spec_hash {
                    register_output(&out_path, &batch_spec, sh, seed)?;
                }
            }

            // Validate
            if args.validate {
                let passed = run_validation(
                    &output,
                    &batch_spec,
                    args.style_guide.as_deref(),
                    args.strict,
                )?;
                if !passed {
                    validation_failed = true;
                }
            }
        }

        println!();
        println!(
            "Batch complete: {} generated, {} cached",
            generated, skipped
        );

        if validation_failed {
            std::process::exit(1);
        }
    } else {
        // Single generation
        if let Some(seed_val) = args.seed {
            spec.seed = SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(seed_val),
                derive_from: None,
            };
        }

        let seed_val = args.seed;

        // If output path is explicit, we can do dedup check before generating
        if let Some(ref out) = args.output {
            let out_path = PathBuf::from(out);
            if args.register && !args.force {
                if let Some(ref sh) = spec_hash {
                    if let Some(s) = seed_val {
                        if check_cache_hit(&out_path, sh, s)? {
                            println!("Cached: {} (spec+seed unchanged)", out_path.display());
                            return Ok(());
                        }
                    }
                }
            }

            let output = generate_one(&spec, &registry, &out_path, args.format.as_deref())?;

            if args.register {
                if let Some(ref sh) = spec_hash {
                    let s = seed_val.unwrap_or(0);
                    register_output(&out_path, &spec, sh, s)?;
                }
            }

            if args.validate {
                let passed = run_validation(
                    &output,
                    &spec,
                    args.style_guide.as_deref(),
                    args.strict,
                )?;
                if !passed {
                    std::process::exit(1);
                }
            }
        } else {
            // No explicit output — generate, then determine path from output type
            let (out_path, output) = generate_single_auto_path(&spec, &registry, &args)?;

            if args.register {
                if let Some(ref sh) = spec_hash {
                    let s = seed_val.unwrap_or(0);
                    register_output(&out_path, &spec, sh, s)?;
                }
            }

            if args.validate {
                let passed = run_validation(
                    &output,
                    &spec,
                    args.style_guide.as_deref(),
                    args.strict,
                )?;
                if !passed {
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

/// Create a copy of the spec with a fixed seed.
fn spec_with_seed(spec: &ProcGenSpec, seed: u64) -> ProcGenSpec {
    let mut s = spec.clone();
    s.seed = SeedConfig {
        mode: SeedMode::Fixed,
        value: Some(seed),
        derive_from: None,
    };
    s
}

/// Generate output from a spec, returning the output and generation time.
fn generate_and_get_output(
    spec: &ProcGenSpec,
    registry: &GeneratorRegistry,
) -> Result<(GeneratorOutput, std::time::Duration)> {
    let start = Instant::now();
    let output = registry
        .generate_from_spec(spec)
        .with_context(|| format!("generation failed for '{}'", spec.meta.name))?;
    let gen_time = start.elapsed();
    Ok((output, gen_time))
}

/// Generate a single output from a spec and write it to disk at the given path.
fn generate_one(
    spec: &ProcGenSpec,
    registry: &GeneratorRegistry,
    out_path: &Path,
    _format: Option<&str>,
) -> Result<GeneratorOutput> {
    let (output, gen_time) = generate_and_get_output(spec, registry)?;
    write_output(&output, out_path, gen_time)?;
    Ok(output)
}

/// Generate a single output with auto-detected output path (no explicit -o given).
/// Returns the path and the output.
fn generate_single_auto_path(
    spec: &ProcGenSpec,
    registry: &GeneratorRegistry,
    args: &GenArgs,
) -> Result<(PathBuf, GeneratorOutput)> {
    let (output, gen_time) = generate_and_get_output(spec, registry)?;

    let default_ext = match output.kind() {
        OutputKind::Mesh => "glb",
        OutputKind::Image => "png",
        OutputKind::Sound => "wav",
    };
    let ext = args.format.as_deref().unwrap_or(default_ext);
    let name = spec
        .meta
        .name
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");
    let out_path = PathBuf::from(format!("{name}.{ext}"));

    write_output(&output, &out_path, gen_time)?;
    Ok((out_path, output))
}

/// Write a GeneratorOutput to disk and print summary info.
fn write_output(
    output: &GeneratorOutput,
    out_path: &Path,
    gen_time: std::time::Duration,
) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
    }

    match output {
        GeneratorOutput::Mesh(mesh) => {
            let glb = export_glb::mesh_to_glb(mesh).context("GLB export failed")?;
            let file_size = glb.len();
            std::fs::write(out_path, glb)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            println!("Output:     {}", out_path.display());
            println!("Format:     GLB");
            println!("Vertices:   {}", format_count(mesh.vertex_count() as u64));
            println!("Triangles:  {}", format_count(mesh.triangle_count() as u64));
            println!("File size:  {}", format_bytes(file_size as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::MeshWithLods(lods) => {
            let glb = export_glb::mesh_lods_to_glb(lods).context("GLB export (LODs) failed")?;
            let file_size = glb.len();
            std::fs::write(out_path, glb)
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
            img.save_png(out_path)
                .with_context(|| format!("failed to save PNG to {}", out_path.display()))?;
            let file_size = std::fs::metadata(out_path)?.len();
            println!("Output:     {}", out_path.display());
            println!("Format:     PNG");
            println!("Dimensions: {}x{}", img.width, img.height);
            println!("Channel:    {:?}", img.channel_semantics);
            println!("File size:  {}", format_bytes(file_size));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::ImageSet(images) => {
            let stem = out_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let parent = out_path.parent().unwrap_or(Path::new("."));
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
        GeneratorOutput::SkinnedMesh(mesh) => {
            let glb = export_glb::skinned_mesh_to_glb(mesh).context("Skinned GLB export failed")?;
            let file_size = glb.len();
            std::fs::write(out_path, glb)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            println!("Output:     {}", out_path.display());
            println!("Format:     GLB (skinned)");
            println!("Vertices:   {}", format_count(mesh.vertex_count() as u64));
            println!("Triangles:  {}", format_count(mesh.triangle_count() as u64));
            println!("Bones:      {}", mesh.bone_count());
            println!("File size:  {}", format_bytes(file_size as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
        GeneratorOutput::Sound(bytes) => {
            std::fs::write(out_path, bytes)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            println!("Output:     {}", out_path.display());
            println!("Format:     WAV (raw)");
            println!("File size:  {}", format_bytes(bytes.len() as u64));
            println!("Gen time:   {:.1} ms", gen_time.as_secs_f64() * 1000.0);
        }
    }

    Ok(())
}

/// Register an output file in the content store and write a sidecar .asset.toml.
fn register_output(
    out_path: &Path,
    spec: &ProcGenSpec,
    spec_hash: &ContentHash,
    seed: u64,
) -> Result<()> {
    let store = ContentStore::new(".flint/assets");
    let content_hash = store
        .store(out_path)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to register in content store")?;

    let ext = out_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let asset_type = match ext {
        "glb" | "gltf" => AssetType::Mesh,
        "png" | "jpg" | "jpeg" => AssetType::Texture,
        "wav" | "ogg" | "mp3" => AssetType::Audio,
        _ => AssetType::Mesh,
    };

    let name = format!(
        "{}_{}",
        spec.meta.name.replace(' ', "_").replace(
            |c: char| !c.is_alphanumeric() && c != '_' && c != '-',
            ""
        ),
        seed
    );

    let mut properties = HashMap::new();
    properties.insert(
        "generator".to_string(),
        toml::Value::String(spec.generator.clone()),
    );
    properties.insert(
        "spec_hash".to_string(),
        toml::Value::String(spec_hash.to_prefixed_hex()),
    );
    properties.insert("seed".to_string(), toml::Value::Integer(seed as i64));
    properties.insert(
        "timestamp".to_string(),
        toml::Value::String(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
        ),
    );
    properties.insert(
        "flint_version".to_string(),
        toml::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );

    let meta = AssetMeta {
        name,
        asset_type,
        hash: content_hash.to_prefixed_hex(),
        source_path: Some(out_path.display().to_string()),
        format: Some(ext.to_string()),
        tags: spec.meta.tags.clone(),
        properties,
    };

    let asset_file = AssetFile { asset: meta };
    let sidecar_path = sidecar_path_for(out_path);
    let toml_str =
        toml::to_string_pretty(&asset_file).context("failed to serialize asset metadata")?;
    std::fs::write(&sidecar_path, toml_str)
        .with_context(|| format!("failed to write sidecar {}", sidecar_path.display()))?;

    println!("Registered: {} -> {}", out_path.display(), content_hash.to_prefixed_hex());

    Ok(())
}

/// Check if a cached sidecar exists with matching spec_hash and seed.
fn check_cache_hit(out_path: &Path, spec_hash: &ContentHash, seed: u64) -> Result<bool> {
    let sidecar = sidecar_path_for(out_path);
    if !sidecar.exists() || !out_path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&sidecar)
        .with_context(|| format!("failed to read sidecar {}", sidecar.display()))?;
    let asset_file: AssetFile =
        toml::from_str(&content).with_context(|| format!("failed to parse {}", sidecar.display()))?;

    let cached_spec_hash = asset_file
        .asset
        .properties
        .get("spec_hash")
        .and_then(|v| v.as_str());
    let cached_seed = asset_file
        .asset
        .properties
        .get("seed")
        .and_then(|v| v.as_integer());

    Ok(cached_spec_hash == Some(&spec_hash.to_prefixed_hex())
        && cached_seed == Some(seed as i64))
}

/// Get the .asset.toml sidecar path for an output file.
fn sidecar_path_for(out_path: &Path) -> PathBuf {
    out_path.with_extension(format!(
        "{}.asset.toml",
        out_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
    ))
}

/// Resolve output path for batch mode, inserting the seed into the filename.
fn resolve_output_path_for_seed(args: &GenArgs, spec: &ProcGenSpec, seed: u64) -> Result<PathBuf> {
    if let Some(ref out) = args.output {
        return Ok(resolve_batch_output_path(Path::new(out), seed));
    }

    // Derive from format or spec defaults
    let ext = args.format.as_deref().unwrap_or("glb");
    let name = spec
        .meta
        .name
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");
    Ok(PathBuf::from(format!("{name}_{seed}.{ext}")))
}

/// Insert a seed into a file path: `tree.glb` → `tree_42.glb`.
/// If the path contains `{seed}`, substitute it directly.
fn resolve_batch_output_path(base: &Path, seed: u64) -> PathBuf {
    let base_str = base.to_string_lossy();
    if base_str.contains("{seed}") {
        return PathBuf::from(base_str.replace("{seed}", &seed.to_string()));
    }

    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = base
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("glb");
    let parent = base.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}_{seed}.{ext}"))
}

/// Build validation constraints from spec and optional style guide.
fn build_validation_constraints(
    spec: &ProcGenSpec,
    style_guide: Option<&StyleGuide>,
) -> ValidationConstraints {
    let mut c = ValidationConstraints::default();

    // Extract max_triangles from LOD level 0 target if present
    if let Some(ref lods) = spec.lod {
        if let Some(lod0) = lods.iter().find(|l| l.level == 0) {
            c.max_triangles = Some(lod0.target_triangles);
        }
    }

    // Extract texture dimensions from spec params if present
    if let Some(table) = spec.params.as_table() {
        if let Some(w) = table.get("width").and_then(|v| v.as_integer()) {
            c.expected_texture_width = Some(w as u32);
        }
        if let Some(h) = table.get("height").and_then(|v| v.as_integer()) {
            c.expected_texture_height = Some(h as u32);
        }
    }

    // Overlay style guide constraints
    if let Some(style) = style_guide {
        if let Some(max_tris) = style.geometry.max_triangles {
            c.max_triangles = Some(max_tris);
        }
        c.roughness_range = style.materials.roughness_range;
        c.metallic_range = style.materials.metallic_range;
        c.palette = style.palette.clone();
    }

    c
}

/// Run validation on a generator output, printing results. Returns true if passed.
fn run_validation(
    output: &GeneratorOutput,
    spec: &ProcGenSpec,
    style_guide_path: Option<&str>,
    strict: bool,
) -> Result<bool> {
    let style_guide = if let Some(path) = style_guide_path {
        Some(
            StyleGuide::load(Path::new(path))
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to load style guide from {path}"))?,
        )
    } else {
        None
    };

    let constraints = build_validation_constraints(spec, style_guide.as_ref());
    let report = flint_procgen::validate_output(output, spec, &constraints);

    println!();
    report.print_checks();
    println!();
    report.print_summary();

    if report.has_failures() {
        Ok(false)
    } else if strict && report.has_warnings() {
        println!("Strict mode: warnings treated as failures");
        Ok(false)
    } else {
        Ok(true)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_output_path_default_insertion() {
        let base = Path::new("/tmp/tree.glb");
        let result = resolve_batch_output_path(base, 42);
        assert_eq!(result, PathBuf::from("/tmp/tree_42.glb"));
    }

    #[test]
    fn test_batch_output_path_seed_template() {
        let base = Path::new("/tmp/tree_{seed}.glb");
        let result = resolve_batch_output_path(base, 7);
        assert_eq!(result, PathBuf::from("/tmp/tree_7.glb"));
    }

    #[test]
    fn test_batch_output_path_no_parent() {
        let base = Path::new("tree.glb");
        let result = resolve_batch_output_path(base, 0);
        assert_eq!(result, PathBuf::from("tree_0.glb"));
    }

    #[test]
    fn test_sidecar_path() {
        let out = Path::new("/tmp/tree_42.glb");
        let sidecar = sidecar_path_for(out);
        assert_eq!(sidecar, PathBuf::from("/tmp/tree_42.glb.asset.toml"));
    }

    #[test]
    fn test_sidecar_path_png() {
        let out = Path::new("wall.png");
        let sidecar = sidecar_path_for(out);
        assert_eq!(sidecar, PathBuf::from("wall.png.asset.toml"));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(1_500_000), "1.4 MB");
    }

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1234), "1,234");
        assert_eq!(format_count(1_000_000), "1,000,000");
    }

    #[test]
    fn test_build_validation_constraints_from_spec_lod() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[[lod]]
level = 0
target_triangles = 5000
[[lod]]
level = 1
target_triangles = 1000
"#,
        )
        .unwrap();
        let constraints = build_validation_constraints(&spec, None);
        assert_eq!(constraints.max_triangles, Some(5000));
    }

    #[test]
    fn test_build_validation_constraints_from_style() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#,
        )
        .unwrap();

        // Build a StyleGuide manually for testing
        use flint_asset_gen::style::{GeometryConstraints, MaterialConstraints};
        let style = StyleGuide {
            name: "test_style".into(),
            description: None,
            prompt_prefix: None,
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec!["#FF0000".into()],
            materials: MaterialConstraints {
                roughness_range: Some([0.6, 0.95]),
                metallic_range: Some([0.0, 0.15]),
                preferred_materials: vec![],
            },
            geometry: GeometryConstraints {
                max_triangles: Some(3000),
                require_uvs: None,
                require_normals: None,
            },
        };

        let constraints = build_validation_constraints(&spec, Some(&style));
        assert_eq!(constraints.max_triangles, Some(3000));
        assert_eq!(constraints.roughness_range, Some([0.6, 0.95]));
        assert_eq!(constraints.metallic_range, Some([0.0, 0.15]));
        assert_eq!(constraints.palette, vec!["#FF0000".to_string()]);
    }

    #[test]
    fn test_build_validation_constraints_texture_dims_from_params() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "texture"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 512
height = 512
"#,
        )
        .unwrap();
        let constraints = build_validation_constraints(&spec, None);
        assert_eq!(constraints.expected_texture_width, Some(512));
        assert_eq!(constraints.expected_texture_height, Some(512));
    }
}
