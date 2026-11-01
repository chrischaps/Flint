//! The mesh [`Previewer`] for `flint gen --ai-refine --reference <image>`:
//! it lets Claude see what a spec builds, and the refine measure every
//! round against the reference (#183).
//!
//! `flint-procgen-ai` can't draw a mesh (it depends on `flint-procgen`
//! only), so this module draws for it: generate the spec, export a GLB, and
//! render it with the same headless model render as
//! `flint edit model.glb --render`. `gen_ai::build_agent` attaches it.

use std::path::PathBuf;

use anyhow::{Context, Result};
use flint_procgen::{export_glb, GeneratorOutput, GeneratorRegistry, ProcGenSpec};
use flint_procgen_ai::Previewer;

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
