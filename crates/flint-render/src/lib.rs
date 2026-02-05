//! Flint Render - wgpu-based renderer for visualizing scenes
//!
//! This crate provides a PBR 3D renderer for previewing Flint scenes,
//! rendering entities as colored shapes or imported glTF models with
//! physically-based Cook-Torrance shading. Supports GPU vertex skinning
//! for skeletal animation.

mod camera;
mod context;
mod debug;
mod gpu_mesh;
mod headless;
mod pipeline;
mod primitives;
mod scene_renderer;
pub mod shadow;
pub mod skinned_pipeline;
mod texture_cache;

pub use camera::{Camera, CameraMode};
pub use context::{RenderContext, RenderError};
pub use debug::{DebugMode, DebugState};
pub use gpu_mesh::{GpuMesh, GpuSkinnedMesh, MeshCache};
pub use headless::HeadlessContext;
pub use pipeline::{
    DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline, SpotLight,
    TransformUniforms,
};
pub use primitives::{
    create_box_mesh, generate_normal_arrows, triangles_to_wireframe_indices, Mesh, SkinnedMesh,
    SkinnedVertex, Vertex,
};
pub use scene_renderer::{ArchetypeVisual, SceneRenderer};
pub use skinned_pipeline::SkinnedPipeline;
pub use texture_cache::TextureCache;

#[cfg(test)]
mod tests {
    #[test]
    fn shader_wgsl_parses() {
        let source = include_str!("shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("shader.wgsl failed to parse");
    }

    #[test]
    fn shadow_shader_wgsl_parses() {
        let source = include_str!("shadow_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("shadow_shader.wgsl failed to parse");
    }

    #[test]
    fn skinned_shader_wgsl_parses() {
        let source = include_str!("skinned_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("skinned_shader.wgsl failed to parse");
    }
}
