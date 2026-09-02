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
pub mod grass_pipeline;
mod headless;
pub mod model_loader;
pub mod ocean_pipeline;
pub mod orbit_controller;
pub mod particle_pipeline;
mod pipeline;
pub mod postprocess;
mod primitives;
pub mod render_stats;
mod scene_renderer;
pub mod shadow;
pub mod skinned_pipeline;
pub mod sky_pipeline;
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
pub use gpu_mesh::{GpuMesh, GpuSkinnedMesh, MeshCache};
pub use grass_pipeline::{
    GrassComputeUniforms, GrassEntityPosition, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, GrassVertex, BLADE_INDEX_COUNT, MAX_GRASS_ENTITIES,
};
pub use headless::HeadlessContext;
pub use ocean_pipeline::{OceanPipeline, OceanUniformsGpu, OceanVisuals};
pub use orbit_controller::OrbitCameraController;
pub use particle_pipeline::{
    ParticleDrawCall, ParticleDrawData, ParticleInstanceGpu, ParticlePipeline, ParticleUniforms,
};
pub use pipeline::{
    DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline, SpotLight,
    TransformUniforms,
};
pub use postprocess::{
    post_process_config_from_def, PostProcessConfig, PostProcessPipeline, PostProcessResources,
    HDR_FORMAT,
};
pub use primitives::{
    create_box_mesh, create_plane_mesh, generate_normal_arrows, generate_skeleton_lines,
    triangles_to_wireframe_indices, Mesh, SkinnedMesh, SkinnedVertex, Vertex,
};
pub use render_stats::{format_count, RenderStats};
pub use scene_renderer::{ArchetypeVisual, LightingLevers, RendererConfig, SceneRenderer};
pub use skinned_pipeline::SkinnedPipeline;
pub use sky_pipeline::{SkyParams, SkyPipeline, SkyUniformsGpu};
pub use skybox_pipeline::SkyboxPipeline;
pub use sprite2d_pipeline::{
    Sprite2dBatch, Sprite2dInstanceGpu, Sprite2dPipeline, Sprite2dUniforms,
};
pub use terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
pub use texture_cache::TextureCache;

#[cfg(test)]
mod tests {
    #[test]
    fn shader_wgsl_parses_and_validates() {
        let source = include_str!("shader.wgsl");
        let module = naga::front::wgsl::parse_str(source).expect("shader.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("shader.wgsl failed validation");
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
    fn ocean_shader_wgsl_parses() {
        let source = include_str!("ocean_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("ocean_shader.wgsl failed to parse");
    }

    #[test]
    fn sky_shader_wgsl_parses() {
        let source = include_str!("sky_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("sky_shader.wgsl failed to parse");
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
    fn composite_shader_wgsl_parses_and_validates() {
        let source = include_str!("composite_shader.wgsl");
        let module =
            naga::front::wgsl::parse_str(source).expect("composite_shader.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("composite_shader.wgsl failed validation");
    }

    #[test]
    fn depth_resolve_shader_wgsl_parses_and_validates() {
        let source = include_str!("depth_resolve.wgsl");
        let module =
            naga::front::wgsl::parse_str(source).expect("depth_resolve.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("depth_resolve.wgsl failed validation");
    }

    #[test]
    fn fxaa_shader_wgsl_parses_and_validates() {
        let source = include_str!("fxaa_shader.wgsl");
        let module =
            naga::front::wgsl::parse_str(source).expect("fxaa_shader.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("fxaa_shader.wgsl failed validation");
    }

    #[test]
    fn post_process_uniforms_layout() {
        // Desaturate reused a pre-existing pad slot (176 bytes); the DoF row
        // (strength/focus/range/_pad2) and the render-mode rows
        // (render_mode/mode_mix/mode_time/pad + mode_params vec4) were
        // appended after inv_view_proj (224); the color-grade rows
        // (lift+enabled / gamma+pad / gain+pad, ADR 0050) grew it to 272,
        // with film grain reusing the former _pad slot. Any other size means
        // the Rust struct and the WGSL mirror have diverged.
        let size = std::mem::size_of::<crate::postprocess::PostProcessUniforms>();
        assert_eq!(size % 16, 0, "PostProcessUniforms not 16-byte aligned");
        assert_eq!(size, 272, "PostProcessUniforms size changed");
    }

    #[test]
    fn light_uniforms_layout() {
        // LightUniforms is mirrored field-for-field in SIX WGSL structs
        // (shader / skinned / terrain / grass_render / volumetric / ocean);
        // bind groups use min_binding_size: None, so a divergence surfaces
        // as a draw-time wgpu validation error, not a compile error. The
        // sheen vec4 (ADR 0048) grew the struct from 1264 to 1280 bytes;
        // PointLight's source_radius row (ADR 0056) grew it to 1536
        // (16 point lights × 16 extra bytes).
        let size = std::mem::size_of::<crate::pipeline::LightUniforms>();
        assert_eq!(size % 16, 0, "LightUniforms not 16-byte aligned");
        assert_eq!(
            size, 1536,
            "LightUniforms size changed — update all six WGSL mirrors"
        );
    }

    #[test]
    fn light_uniforms_defaults_are_lever_neutral() {
        use bytemuck::Zeroable;
        // The clay-look levers must decode to "off" from every pre-existing
        // writer: wrap rides ambient_sky.w as (1 + wrap), Oren-Nayar rides
        // ambient_ground.w the same way, sheen is a plain zeroed vec4.
        let defaults = crate::pipeline::LightUniforms::default_scene_lights();
        assert_eq!(defaults.ambient_sky[3], 1.0, "wrap must decode to 0");
        assert_eq!(defaults.ambient_ground[3], 1.0, "oren must decode to 0");
        assert_eq!(defaults.sheen_color_strength, [0.0; 4], "sheen must be off");
        let zeroed = crate::pipeline::LightUniforms::zeroed();
        assert_eq!(zeroed.sheen_color_strength, [0.0; 4]);
        // Area-light levers (ADR 0056): 0 = punctual → verbatim legacy path.
        for d in defaults
            .directional_lights
            .iter()
            .chain(zeroed.directional_lights.iter())
        {
            assert_eq!(d.angular_size, 0.0, "angular_size must default punctual");
        }
        for p in defaults
            .point_lights
            .iter()
            .chain(zeroed.point_lights.iter())
        {
            assert_eq!(p.source_radius, 0.0, "point source_radius must default 0");
        }
        for sl in defaults.spot_lights.iter().chain(zeroed.spot_lights.iter()) {
            assert_eq!(sl.source_radius, 0.0, "spot source_radius must default 0");
        }
        use crate::pipeline::{DirectionalLight, PointLight, SpotLight};
        assert_eq!(DirectionalLight::default().angular_size, 0.0);
        assert_eq!(PointLight::default().source_radius, 0.0);
        assert_eq!(SpotLight::default().source_radius, 0.0);
    }

    #[test]
    fn shadow_uniforms_default_texel_is_unset() {
        // cascade_splits.w = 0 means "unset" — shaders fall back to the
        // legacy hardcoded 1/2048 (ADR 0049), so a default/stale uniform
        // keeps the exact pre-lever behavior.
        let defaults = crate::shadow::ShadowUniforms::default();
        assert_eq!(defaults.cascade_splits[3], 0.0);
    }

    #[test]
    fn shadow_uniforms_layout() {
        // ShadowUniforms is mirrored in six WGSL shaders (same set as
        // LightUniforms); the PCSS vec4 (ADR 0057) grew it 208 → 224 bytes.
        let size = std::mem::size_of::<crate::shadow::ShadowUniforms>();
        assert_eq!(size % 16, 0, "ShadowUniforms not 16-byte aligned");
        assert_eq!(
            size, 224,
            "ShadowUniforms size changed — update all six WGSL mirrors"
        );
    }

    #[test]
    fn shadow_uniforms_default_pcss_is_unset() {
        use bytemuck::Zeroable;
        // pcss.w = 0 means "PCSS off" — shaders take the legacy 3×3 PCF
        // path verbatim (ADR 0057), so default/stale uniforms keep the
        // exact pre-lever behavior.
        assert_eq!(crate::shadow::ShadowUniforms::default().pcss, [0.0; 4]);
        assert_eq!(crate::shadow::ShadowUniforms::zeroed().pcss, [0.0; 4]);
    }

    #[test]
    fn post_process_config_default_desaturate_is_zero() {
        assert_eq!(crate::PostProcessConfig::default().desaturate, 0.0);
    }

    #[test]
    fn post_process_config_default_dof_is_off() {
        let config = crate::PostProcessConfig::default();
        assert_eq!(config.dof_strength, 0.0, "DoF must default off");
        assert!(config.dof_focus_range > 0.0);
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
