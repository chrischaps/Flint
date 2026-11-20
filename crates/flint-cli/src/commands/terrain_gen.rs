//! `flint gen x.terrain.toml`: a terrain spec baked to its heightmap and
//! splat map with no window (#719).
//!
//! The maps are the bytes the terrain editor's Ctrl+E export writes: both
//! call `flint_terrain::bake` and its PNG encoders. A spec key nothing reads
//! is a warning (an error with `--strict`), as a procgen spec's are.

use super::gen::GenArgs;
use super::gen_preview_common::random_seed;
use super::gen_store;
use crate::output::say;
use anyhow::{bail, Context, Result};
use flint_core::ContentHash;
use flint_terrain::spec::SeedMode;
use flint_terrain::{SpecIssue, TerrainSpec};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The ending that sends `flint gen` here.
pub const TERRAIN_SUFFIX: &str = ".terrain.toml";

/// Whether `flint gen` was handed a terrain spec rather than a procgen spec.
pub fn is_terrain_spec(spec: &str) -> bool {
    spec.to_ascii_lowercase().ends_with(TERRAIN_SUFFIX)
}

/// Where the two maps go.
#[derive(Debug, PartialEq)]
struct Target {
    heightmap: PathBuf,
    splat: PathBuf,
}

impl Target {
    /// `-o hills.png` is the heightmap, with `hills_splat.png` beside it
    /// (the editor's export names them so); a path ending in a separator, or
    /// a folder, gets `<name>.png` and `<name>_splat.png` inside it; no `-o`
    /// is those two here. `{seed}` in `-o` becomes the seed.
    fn of(output: Option<&str>, name: &str, seed: u64) -> Result<Target> {
        let heightmap = match output.map(|o| o.replace("{seed}", &seed.to_string())) {
            None => PathBuf::from(format!("{name}.png")),
            Some(o) if o.ends_with('/') || o.ends_with('\\') || Path::new(&o).is_dir() => {
                Path::new(&o).join(format!("{name}.png"))
            }
            Some(o) => {
                let path = PathBuf::from(&o);
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !ext.eq_ignore_ascii_case("png") {
                    bail!(
                        "a terrain spec makes two PNGs (a heightmap and its splat map): \
                         -o names the heightmap, like {}, or a folder ending in /",
                        path.with_extension("png").display()
                    );
                }
                path
            }
        };
        let stem = heightmap
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
            .to_string();
        let splat = heightmap.with_file_name(format!("{stem}_splat.png"));
        Ok(Target { heightmap, splat })
    }
}

pub fn run(args: &GenArgs) -> Result<()> {
    let spec_arg = args.spec.as_deref().expect("checked by the caller");
    refuse_procgen_flags(args)?;
    let path = Path::new(spec_arg);
    let (spec, issues) = TerrainSpec::from_file_checked(path)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to load terrain spec {spec_arg}"))?;
    for issue in &issues {
        eprintln!("warning: {spec_arg}: {issue}");
    }
    if args.strict && !issues.is_empty() {
        bail!("{} spec key(s) would be ignored (--strict)", issues.len());
    }

    let (seed, seed_note) = match (args.seed, spec.seed.mode) {
        (Some(seed), _) => (seed, format!("{seed} (--seed)")),
        (None, SeedMode::Fixed) => (spec.seed.value, format!("{} (fixed)", spec.seed.value)),
        (None, SeedMode::Random) => {
            let seed = random_seed();
            (
                seed,
                format!("{seed} (random; --seed {seed} makes this one again)"),
            )
        }
    };
    let name = gen_store::slug(&spec.meta.name);
    let target = Target::of(args.output.as_deref(), &name, seed)?;
    let res = flint_terrain::bake::map_resolution(&spec);
    let geo = &spec.geometry;
    let labels = layer_labels(&spec);

    say!("Terrain:    {spec_arg} ({})", spec.meta.name);
    say!("Seed:       {seed_note}");
    say!(
        "Maps:       {res} x {res} texels over {} x {} m, {} m high",
        geo.width,
        geo.depth,
        geo.height_scale
    );
    say!(
        "Recipe:     {} height layer(s), {} splat rule(s)",
        spec.height_layers.len(),
        spec.splat_rules.len()
    );
    let base = json!({
        "spec": shown(path),
        "name": spec.meta.name,
        "seed": seed,
        "seed_mode": if args.seed.is_some() { "flag" } else {
            match spec.seed.mode { SeedMode::Fixed => "fixed", SeedMode::Random => "random" }
        },
        "resolution": res,
        "issues": issues_json(&issues),
    });

    if args.dry_run {
        say!("Would write {}", shown(&target.heightmap));
        say!("            {}", shown(&target.splat));
        crate::output::when_json(|| {
            let mut doc = base.clone();
            doc["dry_run"] = json!(true);
            doc["files"] = json!([
                { "role": "heightmap", "path": shown(&target.heightmap) },
                { "role": "splat", "path": shown(&target.splat) },
            ]);
            doc
        });
        return Ok(());
    }

    let start = Instant::now();
    let maps = flint_terrain::bake(&spec, seed);
    let gen_ms = start.elapsed().as_secs_f64() * 1000.0;
    let heightmap_png = maps.heightmap_png().map_err(anyhow::Error::msg)?;
    let splat_png = maps.splat_png().map_err(anyhow::Error::msg)?;

    let coverage = maps.splat_coverage();
    let mut shares: Vec<String> = Vec::new();
    for (label, share) in labels.iter().zip(coverage) {
        if share > 0.0005 {
            shares.push(format!("{label} {:.1}%", share * 100.0));
        }
    }
    say!(
        "Coverage:   {}",
        if spec.splat_rules.is_empty() {
            "no splat rules: all layer 0".to_string()
        } else {
            shares.join(", ")
        }
    );

    let mut files = Vec::new();
    for (role, file, bytes, format) in [
        (
            "heightmap",
            &target.heightmap,
            &heightmap_png,
            "16-bit grey",
        ),
        (
            "splat",
            &target.splat,
            &splat_png,
            "RGBA8, a channel a layer",
        ),
    ] {
        if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(file, bytes)
            .with_context(|| format!("failed to write {}", file.display()))?;
        say!(
            "Output:     {} ({format}, {})",
            shown(file),
            size(bytes.len())
        );
        files.push(json!({
            "role": role,
            "path": shown(file),
            "bytes": bytes.len(),
            "sha256": ContentHash::from_bytes(bytes).to_hex(),
        }));
    }
    say!("Gen time:   {gen_ms:.1} ms");

    crate::output::when_json(|| {
        let mut doc = base.clone();
        doc["files"] = json!(files);
        doc["coverage"] = labels
            .iter()
            .zip(coverage)
            .enumerate()
            .map(|(i, (label, share))| json!({ "layer": i, "label": label, "share": share }))
            .collect();
        doc["gen_ms"] = json!(gen_ms);
        doc
    });
    Ok(())
}

/// Each splat layer's name: its texture's label, else its file's stem, else
/// `layer<n>`.
fn layer_labels(spec: &TerrainSpec) -> [String; 4] {
    let layers = &spec.textures.layers;
    let slots = [
        &layers.layer0,
        &layers.layer1,
        &layers.layer2,
        &layers.layer3,
    ];
    let mut out: [String; 4] = Default::default();
    for (i, slot) in slots.into_iter().enumerate() {
        out[i] = slot
            .as_ref()
            .and_then(|l| {
                (!l.label.is_empty()).then(|| l.label.clone()).or_else(|| {
                    Path::new(&l.path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::to_string)
                })
            })
            .unwrap_or_else(|| format!("layer{i}"));
    }
    out
}

fn issues_json(issues: &[SpecIssue]) -> serde_json::Value {
    issues
        .iter()
        .map(|i| json!({ "path": i.path, "message": i.message }))
        .collect()
}

fn shown(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// The flags that only mean something to a procgen spec, refused by name
/// rather than ignored.
fn refuse_procgen_flags(args: &GenArgs) -> Result<()> {
    let used: Vec<&str> = [
        (args.ai_assist, "--ai-assist"),
        (args.ai_refine, "--ai-refine"),
        (args.format.is_some(), "--format"),
        (args.batch.is_some(), "--batch"),
        (args.seed_start.is_some(), "--seed-start"),
        (args.register, "--register"),
        (args.strip, "--strip"),
        (args.print_spec, "--print-spec"),
        (args.save_spec.is_some(), "--save-spec"),
        (args.style_guide.is_some(), "--style-guide"),
        (args.validate || args.report.is_some(), "--validate"),
        (args.max_cost.is_some(), "--max-cost"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    if !used.is_empty() {
        bail!(
            "{} {} for .procgen.toml specs; a .terrain.toml takes -o, --seed, --strict and --dry-run",
            used.join(", "),
            if used.len() == 1 { "is" } else { "are" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_terrain_specs_take_this_route() {
        assert!(is_terrain_spec("levels/rolling_meadow.terrain.toml"));
        assert!(is_terrain_spec("X.TERRAIN.TOML"));
        assert!(!is_terrain_spec("specs/oak_tree.procgen.toml"));
        assert!(!is_terrain_spec("terrain.toml"));
    }

    #[test]
    fn the_splat_map_sits_beside_the_heightmap() {
        let t = Target::of(Some("out/hills.png"), "meadow", 7).unwrap();
        assert_eq!(t.heightmap, Path::new("out/hills.png"));
        assert_eq!(t.splat, Path::new("out/hills_splat.png"));

        let t = Target::of(Some("out/"), "meadow", 7).unwrap();
        assert_eq!(t.heightmap, Path::new("out/meadow.png"));
        assert_eq!(t.splat, Path::new("out/meadow_splat.png"));

        let t = Target::of(None, "meadow", 7).unwrap();
        assert_eq!(t.heightmap, Path::new("meadow.png"));
        assert_eq!(t.splat, Path::new("meadow_splat.png"));

        let t = Target::of(Some("h_{seed}.png"), "meadow", 9).unwrap();
        assert_eq!(t.heightmap, Path::new("h_9.png"));
        assert_eq!(t.splat, Path::new("h_9_splat.png"));
    }

    #[test]
    fn an_output_that_is_not_a_png_is_refused() {
        let e = Target::of(Some("out/hills.glb"), "meadow", 7).unwrap_err();
        assert!(e.to_string().contains("out/hills.png"), "{e}");
    }
}
