//! Flint Render - wgpu-based renderer for visualizing scenes
//!
//! This crate provides a PBR 3D renderer for previewing Flint scenes,
//! rendering entities as colored shapes or imported glTF models with
//! physically-based Cook-Torrance shading. Supports GPU vertex skinning
//! for skeletal animation.

pub mod atlas;
pub mod billboard_pipeline;
pub mod bitmap_font;
mod camera;
mod context;
mod debug;
pub mod frustum;
mod gpu_mesh;
pub mod render_stats;
pub mod grass_pipeline;
mod headless;
pub mod model_loader;
pub mod orbit_controller;
pub mod particle_pipeline;
mod pipeline;
pub mod postprocess;
mod primitives;
mod scene_renderer;
pub mod shadow;
pub mod skinned_pipeline;
pub mod skybox_pipeline;
pub mod sprite2d_pipeline;
pub mod terrain_pipeline;
mod texture_cache;

pub use atlas::{AtlasRegistry, TextureAtlas};
pub use billboard_pipeline::BillboardPipeline;
pub use bitmap_font::{anchor_origin, apply_fill, BitmapFont};
pub use camera::{Camera, CameraMode};
pub use context::{RenderContext, RenderError};
pub use debug::{DebugMode, DebugState};
pub use frustum::Frustum;
pub use render_stats::{RenderStats, format_count};
pub use gpu_mesh::{GpuMesh, GpuSkinnedMesh, MeshCache};
pub use grass_pipeline::{
    GrassComputeUniforms, GrassEntityPosition, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, GrassVertex, BLADE_INDEX_COUNT, MAX_GRASS_ENTITIES,
};
pub use headless::HeadlessContext;
pub use orbit_controller::OrbitCameraController;
pub use particle_pipeline::{
    ParticleDrawCall, ParticleDrawData, ParticleInstanceGpu, ParticlePipeline, ParticleUniforms,
};
pub use pipeline::{
    DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline, SpotLight,
    TransformUniforms,
};
pub use postprocess::{PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT};
pub use primitives::{
    create_box_mesh, create_plane_mesh, generate_normal_arrows, generate_skeleton_lines,
    triangles_to_wireframe_indices, Mesh, SkinnedMesh, SkinnedVertex, Vertex,
};
pub use scene_renderer::{ArchetypeVisual, RendererConfig, SceneRenderer};
pub use skinned_pipeline::SkinnedPipeline;
pub use skybox_pipeline::SkyboxPipeline;
pub use sprite2d_pipeline::{
    Sprite2dBatch, Sprite2dInstanceGpu, Sprite2dPipeline, Sprite2dUniforms,
};
pub use terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
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
    fn terrain_shader_wgsl_parses() {
        let source = include_str!("terrain_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("terrain_shader.wgsl failed to parse");
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

    #[test]
    fn sprite2d_shader_wgsl_parses() {
        let source = include_str!("sprite2d_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("sprite2d_shader.wgsl failed to parse");
    }

    #[test]
    fn grass_compute_shader_wgsl_parses() {
        let source = include_str!("grass_compute.wgsl");
        naga::front::wgsl::parse_str(source).expect("grass_compute.wgsl failed to parse");
    }

    #[test]
    fn grass_render_shader_wgsl_parses() {
        let source = include_str!("grass_render.wgsl");
        naga::front::wgsl::parse_str(source).expect("grass_render.wgsl failed to parse");
    }
}
