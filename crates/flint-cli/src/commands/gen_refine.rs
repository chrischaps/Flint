//! `flint gen <spec> --refine <reference>`: ask Claude to move a spec toward
//! a reference image, showing it what the spec builds and measuring every
//! round (#183).
//!
//! `flint-procgen-ai` can't draw a mesh (it depends on `flint-procgen`
//! only), so this module is its [`Previewer`]: generate the spec, export a
//! GLB, and render it with the same headless model render as
//! `flint edit model.glb --render`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flint_procgen::{export_glb, GeneratorOutput, GeneratorRegistry, ProcGenSpec};
use flint_procgen_ai::{AgentConfig, AnthropicAgent, Distance, Previewer, Report};

use super::preview;

/// Side of the square render Claude sees and the loop measures. Small
/// enough to be cheap in tokens, big enough to show a silhouette.
pub const PREVIEW_SIZE: u32 = 512;

/// Renders mesh specs through the headless model renderer.
pub struct MeshRenderPreviewer {
    size: u32,
    scratch: PathBuf,
}

impl MeshRenderPreviewer {
    /// A previewer rendering `size` x `size` images, using `scratch` for the
    /// GLB it renders from.
    pub fn new(size: u32, scratch: PathBuf) -> Self {
        Self { size, scratch }
    }

    /// The PNG `spec` renders to, or why it couldn't.
    pub fn render(&self, spec: &ProcGenSpec) -> Result<Vec<u8>> {
        let mut registry = GeneratorRegistry::new();
        flint_procgen::register_built_in_generators(&mut registry);
        let glb = match registry.generate_from_spec(spec)? {
            GeneratorOutput::Mesh(m) => export_glb::mesh_to_glb(&m)?,
            // Only the full-detail level: LODs would stack on one another.
            GeneratorOutput::MeshWithLods(l) => {
                export_glb::mesh_to_glb(l.first().context("no LOD levels")?)?
            }
            GeneratorOutput::SkinnedMesh(m) => export_glb::skinned_mesh_to_glb(&m)?,
            _ => anyhow::bail!("not a mesh"),
        };
        std::fs::create_dir_all(&self.scratch)?;
        let path = self.scratch.join(format!(
            "refine-preview-{}-{}.glb",
            std::process::id(),
            unique()
        ));
        std::fs::write(&path, glb)?;
        let rendered = preview::render_model(&path, self.size, self.size);
        let _ = std::fs::remove_file(&path);
        let mut png = std::io::Cursor::new(Vec::new());
        rendered?.write_to(&mut png, image::ImageFormat::Png)?;
        Ok(png.into_inner())
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Previewer for MeshRenderPreviewer {
    fn preview(&self, spec: &ProcGenSpec) -> Option<Vec<u8>> {
        match self.render(spec) {
            Ok(png) => Some(png),
            Err(e) => {
                // A texture lands here by design; the agent shows its pixels.
                if !e.to_string().contains("not a mesh") {
                    eprintln!("warning: couldn't render a preview: {e:#}");
                }
                None
            }
        }
    }
}

/// Refine `spec` toward the image at `reference`, up to `rounds` measured
/// rounds. Prints each round's distance and returns the closest spec.
pub fn refine(spec: &ProcGenSpec, reference: &Path, rounds: u32) -> Result<ProcGenSpec> {
    let bytes = std::fs::read(reference)
        .with_context(|| format!("failed to read reference {}", reference.display()))?;
    let scratch = std::env::temp_dir().join("flint-refine");
    let agent = AnthropicAgent::new(AgentConfig::anthropic())
        .context("--refine asks Claude")?
        .with_previewer(MeshRenderPreviewer::new(PREVIEW_SIZE, scratch))
        .with_refine_rounds(rounds);
    println!(
        "Refining {} toward {} ({}, up to {rounds} round(s))...",
        spec.meta.name,
        reference.display(),
        agent.config().model
    );
    let (refined, report) = agent
        .refine_spec_reported(spec, Some(&bytes))
        .context("refine failed")?;
    print_report(&report);
    Ok(refined)
}

fn print_report(report: &Report) {
    if report.distances.is_empty() {
        println!("Unmeasured: no picture of what the spec builds.");
    }
    for (i, d) in report.distances.iter().enumerate() {
        let label = if i == 0 {
            "start".to_string()
        } else {
            format!("round {i}")
        };
        let kept = if i == report.kept { "  <- kept" } else { "" };
        println!("  {label:<8} {}{kept}", describe(d));
    }
    for note in &report.notes {
        println!("  - {note}");
    }
    let u = report.usage();
    println!(
        "Calls:      {} ({} input + {} output tokens, {} cached)",
        report.rounds.len(),
        u.input_tokens,
        u.output_tokens,
        u.cache_read_input_tokens
    );
}

fn describe(d: &Distance) -> String {
    format!(
        "distance {:5.1}  (colour {:5.1}, histogram {:.2}, layout {:.2})",
        d.score, d.mean_colour, d.histogram, d.layout
    )
}

/// Where a refined spec is written: `tree.procgen.toml` ->
/// `tree.refined.procgen.toml`, beside it.
pub fn refined_spec_path(spec_path: &Path) -> PathBuf {
    let name = spec_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .strip_suffix(".procgen.toml")
        .or_else(|| name.strip_suffix(".toml"))
        .unwrap_or(&name);
    spec_path.with_file_name(format!("{stem}.refined.procgen.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refined_spec_lands_beside_the_spec() {
        assert_eq!(
            refined_spec_path(Path::new("specs/oak.procgen.toml")),
            PathBuf::from("specs/oak.refined.procgen.toml")
        );
        assert_eq!(
            refined_spec_path(Path::new("oak.toml")),
            PathBuf::from("oak.refined.procgen.toml")
        );
    }
}
