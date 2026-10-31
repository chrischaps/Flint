//! `flint gen` — run a procedural generation spec and write the output.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use flint_asset_gen::StyleGuide;
use flint_core::ContentHash;
use flint_procgen::{
    export_glb, Generator, GeneratorOutput, GeneratorRegistry, OutputKind, ProcGenSpec, SeedConfig,
    SeedMode, ValidationConstraints,
};

use super::gen_refine;
use super::gen_store::{self, WrittenFile};

#[derive(Args)]
pub struct GenArgs {
    /// Path to a .procgen.toml spec file
    pub spec: String,

    /// Output path (file or directory). Derived from spec name if omitted.
    #[arg(short, long)]
    pub output: Option<String>,

    /// Override the spec's seed with a fixed value (a batch takes --seed-start)
    #[arg(long, conflicts_with = "batch")]
    pub seed: Option<u64>,

    /// Print estimated cost without generating
    #[arg(long)]
    pub dry_run: bool,

    /// Force output format (auto-detected from extension by default)
    #[arg(long, value_parser = parse_format)]
    pub format: Option<String>,

    /// Number of variants to generate, one after another, with sequential
    /// seeds; each is written as `<stem>_<seed>.<ext>` (or fills `{seed}`)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub batch: Option<u32>,

    /// First seed of a batch: seeds S, S+1, ..., S+N-1 (default: 0)
    #[arg(long, requires = "batch")]
    pub seed_start: Option<u64>,

    /// Store output(s) in the content store (.flint/assets), write a
    /// provenance sidecar beside each file, and reuse an earlier registered
    /// result for the same spec + seed instead of generating
    #[arg(long)]
    pub register: bool,

    /// With --register, generate even if this spec + seed is already stored
    #[arg(long, requires = "register")]
    pub force: bool,

    /// Validate output after generation
    #[arg(long)]
    pub validate: bool,

    /// Treat warnings as failures (exit code 1)
    #[arg(long)]
    pub strict: bool,

    /// Style guide TOML path for additional validation constraints
    /// (triangle ceiling, roughness/metallic ranges, palette, require_uvs)
    #[arg(long, requires = "validate")]
    pub style_guide: Option<String>,

    /// Refine the spec toward this reference image with Claude before
    /// generating (needs ANTHROPIC_API_KEY). Claude sees a render of what
    /// the spec builds; each round is rendered and measured, and the loop
    /// keeps going while the distance falls. The closest spec is written
    /// beside the original as `<name>.refined.procgen.toml`.
    #[arg(long, conflicts_with_all = ["batch", "dry_run"])]
    pub refine: Option<String>,

    /// Most measured rounds for --refine
    #[arg(long, requires = "refine", default_value = "3",
          value_parser = clap::value_parser!(u32).range(1..=8))]
    pub refine_rounds: u32,
}

fn parse_format(s: &str) -> std::result::Result<String, String> {
    match s {
        "glb" | "png" | "wav" => Ok(s.to_string()),
        _ => Err(format!(
            "unsupported format '{s}'; valid values: glb, png, wav"
        )),
    }
}

pub fn run(args: GenArgs) -> Result<()> {
    let spec_path = Path::new(&args.spec);
    let mut spec = ProcGenSpec::from_file(spec_path)
        .with_context(|| format!("failed to load spec from {}", spec_path.display()))?;

    // Build registry
    let mut registry = GeneratorRegistry::new();
    flint_procgen::register_built_in_generators(&mut registry);

    // Everything that can be wrong with the request is checked here, before
    // a (possibly seconds-long) generation runs: the generator must exist,
    // the spec's params must be ones it reads, and the output path /
    // --format must be able to hold what it makes.
    let generator = resolve_generator(&registry, &spec, spec_path)?;

    // Keys the generator won't read: a typo, or a knob for another pattern.
    // Silently ignored params are the worst kind of spec bug, so say so.
    let issues = flint_procgen::lint_params(&generator.param_schema(), &spec.params);
    for issue in &issues {
        eprintln!("warning: {}: {issue}", spec_path.display());
    }
    if args.strict && !issues.is_empty() {
        bail!(
            "{} spec parameter(s) would be ignored (--strict)",
            issues.len()
        );
    }

    let kind = generator.output_kind();
    let format = check_output_format(
        &spec.generator,
        kind,
        args.output.as_deref(),
        args.format.as_deref(),
    )?;

    if let Some(seed_val) = args.seed {
        spec = spec_with_seed(&spec, seed_val);
    }

    // Refine first, then generate what the refine kept.
    if let Some(reference) = &args.refine {
        spec = gen_refine::refine(&spec, Path::new(reference), args.refine_rounds)?;
        let refined_path = gen_refine::refined_spec_path(spec_path);
        let text = toml::to_string(&spec).context("failed to serialize the refined spec")?;
        std::fs::write(&refined_path, text)
            .with_context(|| format!("failed to write {}", refined_path.display()))?;
        println!("Refined:    {}", refined_path.display());
        println!();
    }

    // Dry-run: print cost estimate and return
    if args.dry_run {
        let cost = generator.estimate_cost(&spec);
        println!("Spec:       {}", spec.meta.name);
        println!("Generator:  {}", spec.generator);
        println!("Seed:       {}", describe_seed(&spec));
        println!(
            "Output:     {} ({})",
            kind_noun(kind),
            format.to_uppercase()
        );
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

    // Every variant this run makes, as (seed, output path): one for a plain
    // run, N for `--batch N`. Seeds are resolved here, before generating, so
    // a random or derived spec records the seed it really used (a plain
    // `--register` used to record 0 whatever the spec said).
    let variants: Vec<(u64, PathBuf)> = match args.batch {
        Some(n) => {
            let start = args.seed_start.unwrap_or(0);
            (0..u64::from(n))
                .map(|i| {
                    let seed = start.checked_add(i).ok_or_else(|| {
                        anyhow!("--seed-start {start} with --batch {n} runs past the largest seed")
                    })?;
                    Ok((
                        seed,
                        resolve_output_path_for_seed(&args, &spec, seed, format),
                    ))
                })
                .collect::<Result<_>>()?
        }
        None => {
            let seed = spec
                .resolve_seed()
                .with_context(|| format!("{}: invalid [seed]", spec_path.display()))?;
            vec![(seed, single_output_path(&args, &spec, seed, format))]
        }
    };
    let batch_total = args.batch;

    let store = args.register.then(gen_store::Store::open_default);
    let spec_hash = if args.register {
        Some(
            ContentHash::from_file(spec_path)
                .with_context(|| format!("failed to hash spec file {}", spec_path.display()))?,
        )
    } else {
        None
    };

    let (mut generated, mut cached) = (0u32, 0u32);
    let mut validation_failed = false;
    for (i, (seed, out_path)) in variants.iter().enumerate() {
        let seed = *seed;
        let tag = batch_total
            .map(|n| format!("[{}/{}] ", i + 1, n))
            .unwrap_or_default();
        let seed_spec = spec_with_seed(&spec, seed);

        // The cache: this spec + seed was registered before, and the store
        // still holds every file it made.
        if let (Some(store), Some(sh), false) = (&store, &spec_hash, args.force) {
            if let Some(prov) = store.lookup(sh, seed) {
                let targets = planned_files(out_path, &prov);
                if let Some(reuse) = store.reuse(&prov, &targets)? {
                    store.write_sidecars(&seed_spec, kind, &prov, &targets)?;
                    match reuse {
                        gen_store::Reuse::Present => println!(
                            "{tag}Cached: {} (seed {seed}, spec unchanged)",
                            out_path.display()
                        ),
                        gen_store::Reuse::Restored(k) => println!(
                            "{tag}Cached: {} (seed {seed}, spec unchanged; restored {k} missing \
                             or changed file{} from the content store)",
                            out_path.display(),
                            if k == 1 { "" } else { "s" }
                        ),
                    }
                    if args.validate {
                        println!("  validation skipped for a cached result (--force regenerates)");
                    }
                    cached += 1;
                    continue;
                }
            }
        }

        if batch_total.is_some() {
            println!("{tag}Generating seed {seed}...");
        } else {
            println!("Seed:       {}", describe_resolved_seed(&spec, seed));
        }

        let (output, files) = generate_one(&seed_spec, &registry, out_path)?;
        generated += 1;

        if let (Some(store), Some(sh)) = (&store, &spec_hash) {
            let prov = store.register(&seed_spec, sh, seed, kind, &files)?;
            for (f, (_, hash)) in files.iter().zip(&prov.files) {
                println!("Registered: {} -> {hash}", f.path.display());
            }
        }

        if args.validate
            && !run_validation(
                &output,
                &seed_spec,
                args.style_guide.as_deref(),
                args.strict,
            )?
        {
            validation_failed = true;
        }
    }

    if batch_total.is_some() {
        println!();
        println!("Batch complete: {generated} generated, {cached} cached");
    }
    if validation_failed {
        std::process::exit(1);
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

/// Look up the spec's generator, or explain which spec named an unknown one
/// and what the choices are.
fn resolve_generator<'a>(
    registry: &'a GeneratorRegistry,
    spec: &ProcGenSpec,
    spec_path: &Path,
) -> Result<&'a dyn Generator> {
    registry.get(&spec.generator).ok_or_else(|| {
        let mut names = registry.generator_names();
        names.sort_unstable();
        anyhow!(
            "{} names generator '{}', which isn't registered\n  known generators: {}",
            spec_path.display(),
            spec.generator,
            names.join(", ")
        )
    })
}

/// File formats `flint gen` recognises by extension. An output path ending in
/// one of these must match what the generator makes; any other extension is
/// taken as the caller's choice of name.
const KNOWN_FORMATS: [&str; 4] = ["glb", "gltf", "png", "wav"];

/// The one format each kind of output is written in.
fn kind_format(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Mesh => "glb",
        OutputKind::Image => "png",
        OutputKind::Sound => "wav",
    }
}

fn kind_noun(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Mesh => "a mesh",
        OutputKind::Image => "images",
        OutputKind::Sound => "a sound",
    }
}

/// Check that `--format` and the output path's extension agree with what the
/// generator produces, and return the format that will be written.
///
/// Without this a mesh asked for as `-o tree.png` was written as GLB bytes
/// under a `.png` name, and `--format` was accepted and ignored.
fn check_output_format(
    generator: &str,
    kind: OutputKind,
    output: Option<&str>,
    format: Option<&str>,
) -> Result<&'static str> {
    let native = kind_format(kind);
    if let Some(f) = format {
        if f != native {
            bail!(
                "generator '{generator}' makes {}, written as {}; --format {f} can't hold it",
                kind_noun(kind),
                native.to_uppercase()
            );
        }
    }
    if let Some(out) = output {
        let path = Path::new(out);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if let Some(ext) = ext {
            if KNOWN_FORMATS.contains(&ext.as_str()) && ext != native {
                bail!(
                    "output '{out}' asks for .{ext}, but generator '{generator}' makes {}, \
                     written as {}\n  try: -o {}",
                    kind_noun(kind),
                    native.to_uppercase(),
                    path.with_extension(native).display()
                );
            }
        }
    }
    Ok(native)
}

/// The seed a run will use, as a person would want to read it.
fn describe_seed(spec: &ProcGenSpec) -> String {
    match spec.seed.mode {
        SeedMode::Random => "random (a new seed each run; pass --seed to pin one)".to_string(),
        SeedMode::Fixed => match spec.resolve_seed() {
            Ok(s) => format!("{s} (fixed)"),
            Err(e) => format!("invalid: {e}"),
        },
        SeedMode::Derived => match spec.resolve_seed() {
            Ok(s) => format!(
                "{s} (derived from '{}')",
                spec.seed.derive_from.as_deref().unwrap_or("")
            ),
            Err(e) => format!("invalid: {e}"),
        },
    }
}

/// Generate a single output from a spec and write it to disk at the given
/// path, returning the output and every file written.
fn generate_one(
    spec: &ProcGenSpec,
    registry: &GeneratorRegistry,
    out_path: &Path,
) -> Result<(GeneratorOutput, Vec<WrittenFile>)> {
    let (output, gen_time) = generate_and_get_output(spec, registry)?;
    let files = write_output(&output, out_path, gen_time)?;
    Ok((output, files))
}

/// Where a plain (non-batch) run writes: `-o` as given (with any `{seed}`
/// filled in), or the spec's name in the generator's format.
fn single_output_path(args: &GenArgs, spec: &ProcGenSpec, seed: u64, ext: &str) -> PathBuf {
    match &args.output {
        Some(out) => PathBuf::from(out.replace("{seed}", &seed.to_string())),
        None => PathBuf::from(format!("{}.{ext}", gen_store::slug(&spec.meta.name))),
    }
}

/// The seed a run is about to use, and where it came from.
fn describe_resolved_seed(spec: &ProcGenSpec, seed: u64) -> String {
    match spec.seed.mode {
        SeedMode::Random => format!("{seed} (random; --seed {seed} makes this one again)"),
        SeedMode::Fixed => format!("{seed} (fixed)"),
        SeedMode::Derived => format!(
            "{seed} (derived from '{}')",
            spec.seed.derive_from.as_deref().unwrap_or("")
        ),
    }
}

/// An image set is written as `<stem>_<map>.png` files: inside `out_path`
/// when it names a directory, else beside it with its stem.
fn image_set_dir_and_stem(out_path: &Path) -> (PathBuf, String) {
    let is_dir = out_path.is_dir()
        || out_path
            .to_str()
            .is_some_and(|s| s.ends_with('/') || s.ends_with('\\'));
    if is_dir {
        (out_path.to_path_buf(), "output".to_string())
    } else {
        (
            out_path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            out_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output")
                .to_string(),
        )
    }
}

/// The file a run writes for one role: the output path itself, or one map of
/// an image set.
fn output_file_path(out_path: &Path, role: Option<&str>) -> PathBuf {
    match role {
        None => out_path.to_path_buf(),
        Some(role) => {
            let (dir, stem) = image_set_dir_and_stem(out_path);
            dir.join(format!("{stem}_{role}.png"))
        }
    }
}

/// The files a registered run wrote, placed at this run's output path.
fn planned_files(out_path: &Path, prov: &gen_store::Provenance) -> Vec<WrittenFile> {
    prov.files
        .iter()
        .map(|(role, _)| WrittenFile {
            role: role.clone(),
            path: output_file_path(out_path, role.as_deref()),
        })
        .collect()
}

/// Write a GeneratorOutput to disk, print summary info, and return the files
/// written (one, or one per map of an image set).
fn write_output(
    output: &GeneratorOutput,
    out_path: &Path,
    gen_time: std::time::Duration,
) -> Result<Vec<WrittenFile>> {
    let mut written = Vec::new();
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
            let (dir, _) = image_set_dir_and_stem(out_path);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create directory {}", dir.display()))?;
            let mut total_size: u64 = 0;
            for img in images {
                let role = format!("{:?}", img.channel_semantics).to_lowercase();
                let img_path = output_file_path(out_path, Some(&role));
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
                written.push(WrittenFile {
                    role: Some(role),
                    path: img_path,
                });
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

    if written.is_empty() {
        written.push(WrittenFile {
            role: None,
            path: out_path.to_path_buf(),
        });
    }
    Ok(written)
}

/// Resolve output path for batch mode, inserting the seed into the filename.
fn resolve_output_path_for_seed(
    args: &GenArgs,
    spec: &ProcGenSpec,
    seed: u64,
    ext: &str,
) -> PathBuf {
    // Named after the spec, in the generator's own format (a texture batch
    // used to default to `.glb`).
    let name = gen_store::slug(&spec.meta.name);
    match &args.output {
        Some(out) => resolve_batch_output_path(Path::new(out), seed, &name, ext),
        None => PathBuf::from(format!("{name}_{seed}.{ext}")),
    }
}

/// Insert a seed into a file path: `tree.glb` → `tree_42.glb`.
/// If the path contains `{seed}`, substitute it directly. A directory gets
/// `<name>_<seed>.<ext>` inside it (it used to become a sibling `.glb`).
fn resolve_batch_output_path(base: &Path, seed: u64, name: &str, ext: &str) -> PathBuf {
    let base_str = base.to_string_lossy();
    if base_str.contains("{seed}") {
        return PathBuf::from(base_str.replace("{seed}", &seed.to_string()));
    }
    if base.is_dir() || base_str.ends_with('/') || base_str.ends_with('\\') {
        return base.join(format!("{name}_{seed}.{ext}"));
    }

    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = base.extension().and_then(|e| e.to_str()).unwrap_or(ext);
    let parent = base.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}_{seed}.{ext}"))
}

/// Build validation constraints from spec and optional style guide.
fn build_validation_constraints(
    spec: &ProcGenSpec,
    style_guide: Option<&StyleGuide>,
) -> ValidationConstraints {
    // The spec's LOD targets are read from the spec by `validate_output`,
    // one per level; `max_triangles` is the style's ceiling on every mesh.
    let mut c = ValidationConstraints::default();

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
        // `require_normals` needs no flag: every mesh's normals are checked.
        c.require_uvs = style.geometry.require_uvs.unwrap_or(false);
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
    fn format_follows_the_generator_when_unspecified() {
        assert_eq!(
            check_output_format("t", OutputKind::Mesh, None, None).unwrap(),
            "glb"
        );
        assert_eq!(
            check_output_format("t", OutputKind::Image, None, None).unwrap(),
            "png"
        );
        assert_eq!(
            check_output_format("t", OutputKind::Sound, None, None).unwrap(),
            "wav"
        );
    }

    #[test]
    fn matching_and_unrecognised_extensions_pass() {
        let ok = |out| check_output_format("t", OutputKind::Mesh, Some(out), None);
        assert!(ok("tree.glb").is_ok());
        assert!(ok("TREE.GLB").is_ok());
        assert!(ok("out/tree_{seed}.glb").is_ok());
        assert!(ok("tree.bin").is_ok());
        assert!(ok("tree").is_ok());
        // ImageSet output may be a directory
        assert!(check_output_format("t", OutputKind::Image, Some("out/"), None).is_ok());
    }

    #[test]
    fn a_mismatched_extension_is_refused_with_a_suggestion() {
        let err = check_output_format("tree_v1", OutputKind::Mesh, Some("out/tree.png"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tree_v1"), "{err}");
        assert!(err.contains("a mesh"), "{err}");
        assert!(err.contains("tree.glb"), "{err}");
        assert!(check_output_format("t", OutputKind::Mesh, Some("tree.gltf"), None).is_err());
        assert!(check_output_format("t", OutputKind::Image, Some("wall.glb"), None).is_err());
    }

    #[test]
    fn a_mismatched_format_flag_is_refused() {
        assert!(check_output_format("t", OutputKind::Mesh, None, Some("png")).is_err());
        assert!(check_output_format("t", OutputKind::Mesh, Some("x.bin"), Some("glb")).is_ok());
    }

    #[test]
    fn test_batch_output_path_default_insertion() {
        let base = Path::new("/tmp/tree.glb");
        let result = resolve_batch_output_path(base, 42, "oak", "glb");
        assert_eq!(result, PathBuf::from("/tmp/tree_42.glb"));
    }

    #[test]
    fn test_batch_output_path_seed_template() {
        let base = Path::new("/tmp/tree_{seed}.glb");
        let result = resolve_batch_output_path(base, 7, "oak", "glb");
        assert_eq!(result, PathBuf::from("/tmp/tree_7.glb"));
    }

    #[test]
    fn test_batch_output_path_no_parent() {
        let base = Path::new("tree.glb");
        let result = resolve_batch_output_path(base, 0, "oak", "glb");
        assert_eq!(result, PathBuf::from("tree_0.glb"));
    }

    #[test]
    fn a_batch_into_a_directory_lands_inside_it_in_the_native_format() {
        let result = resolve_batch_output_path(Path::new("out/"), 3, "wall", "png");
        assert_eq!(result, Path::new("out/").join("wall_3.png"));
        // No extension: the generator's format, not always .glb.
        let result = resolve_batch_output_path(Path::new("out/wall"), 3, "x", "png");
        assert_eq!(result, Path::new("out").join("wall_3.png"));
    }

    #[test]
    fn image_set_maps_sit_beside_the_stem_or_inside_a_directory() {
        assert_eq!(
            output_file_path(Path::new("tex/wall_3.png"), Some("normal")),
            Path::new("tex").join("wall_3_normal.png")
        );
        assert_eq!(
            output_file_path(Path::new("tex/"), Some("color")),
            Path::new("tex/").join("output_color.png")
        );
        assert_eq!(
            output_file_path(Path::new("tree.glb"), None),
            PathBuf::from("tree.glb")
        );
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
        // LOD targets stay in the spec, where `validate_output` reads one
        // per level; only a style guide sets the ceiling.
        let constraints = build_validation_constraints(&spec, None);
        assert_eq!(constraints.max_triangles, None);
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
                require_uvs: Some(true),
                require_normals: None,
            },
        };

        let constraints = build_validation_constraints(&spec, Some(&style));
        assert_eq!(constraints.max_triangles, Some(3000));
        assert_eq!(constraints.roughness_range, Some([0.6, 0.95]));
        assert_eq!(constraints.metallic_range, Some([0.0, 0.15]));
        assert_eq!(constraints.palette, vec!["#FF0000".to_string()]);
        assert!(constraints.require_uvs);
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
