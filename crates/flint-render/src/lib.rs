//! Flint Render - wgpu-based renderer for visualizing scenes
//!
//! This crate provides a PBR 3D renderer for previewing Flint scenes,
//! rendering entities as colored shapes or imported glTF models with
//! physically-based Cook-Torrance shading. Supports GPU vertex skinning
//! for skeletal animation.

pub mod billboard_pipeline;
mod camera;
mod context;
mod debug;
mod gpu_mesh;
mod headless;
pub mod model_loader;
pub mod particle_pipeline;
mod pipeline;
pub mod postprocess;
mod primitives;
mod scene_renderer;
pub mod shadow;
pub mod skybox_pipeline;
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
pub use postprocess::{PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT};
pub use scene_renderer::{ArchetypeVisual, RendererConfig, SceneRenderer};
pub use billboard_pipeline::BillboardPipeline;
pub use particle_pipeline::{
    ParticleDrawCall, ParticleDrawData, ParticleInstanceGpu, ParticlePipeline, ParticleUniforms,
};
pub use skybox_pipeline::SkyboxPipeline;
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

    #[test]
    fn billboard_shader_wgsl_parses() {
        let source = include_str!("billboard_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("billboard_shader.wgsl failed to parse");
    }

    #[test]
    fn particle_shader_wgsl_parses() {
        let source = include_str!("particle_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("particle_shader.wgsl failed to parse");
    }

    #[test]
    fn outline_shader_wgsl_parses() {
        let source = include_str!("outline_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("outline_shader.wgsl failed to parse");
    }

    #[test]
    fn skybox_shader_wgsl_parses() {
        let source = include_str!("skybox_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("skybox_shader.wgsl failed to parse");
    }

    #[test]
    fn composite_shader_wgsl_parses() {
        let source = include_str!("composite_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("composite_shader.wgsl failed to parse");
    }

    #[test]
    fn bloom_shader_wgsl_parses() {
        let source = include_str!("bloom_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("bloom_shader.wgsl failed to parse");
    }

    #[test]
    fn ssao_shader_wgsl_parses() {
        let source = include_str!("ssao_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("ssao_shader.wgsl failed to parse");
    }

    #[test]
    fn ssao_blur_shader_wgsl_parses() {
        let source = include_str!("ssao_blur_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("ssao_blur_shader.wgsl failed to parse");
    }
}
