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
pub mod gpu_select;
pub mod gpu_timer;
mod gpu_mesh;
pub mod grass_pipeline;
pub mod ground_fog;
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
pub mod water_body;
mod texture_cache;
pub mod world_line_pipeline;

pub use atlas::{AtlasRegistry, TextureAtlas};
pub use billboard_pipeline::BillboardPipeline;
pub use bitmap_font::{anchor_origin, apply_fill, BitmapFont};
pub use camera::{Camera, CameraMode};
pub use context::{RenderContext, RenderError};
pub use gpu_select::{list_adapters, set_gpu_override};
pub use gpu_timer::{GpuPassTime, GpuScope, GpuTimer, GpuTimings};
pub use ground_fog::GroundFog;
pub use debug::{DebugMode, DebugState};
pub use frustum::Frustum;
pub use gpu_mesh::{GpuMesh, GpuSkinnedMesh, MeshCache};
pub use grass_pipeline::{
    check_grass_mask_patch, grass_dispatch_window, GrassComputeUniforms, GrassEntityPosition, GrassGround, GrassHeight,
    GrassInstanceGpu, GrassPipeline, GrassRenderUniforms, GrassVertex, BLADE_INDEX_COUNT,
    MAX_GRASS_ENTITIES,
};
pub use headless::HeadlessContext;
pub use ocean_pipeline::{OceanPipeline, OceanUniformsGpu, OceanVisuals};
pub use orbit_controller::OrbitCameraController;
pub use particle_pipeline::{
    ParticleBlendMode, ParticleDrawCall, ParticleDrawData, ParticleInstance, ParticlePipeline,
    ParticleUniforms,
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
    generate_skeleton_lines_colored, srgb_to_linear, triangles_to_wireframe_indices, Mesh,
    SkinnedMesh, SkinnedVertex, Vertex,
};
pub use render_stats::{format_count, RenderStats};
pub use scene_renderer::{
    load_particle_textures, ArchetypeVisual, LightingLevers, RendererConfig, SceneRenderer,
    DEFAULT_CLEAR_COLOR,
};
pub use skinned_pipeline::SkinnedPipeline;
pub use sky_pipeline::{SkyParams, SkyPipeline, SkyUniformsGpu};
pub use skybox_pipeline::SkyboxPipeline;
pub use sprite2d_pipeline::{
    Sprite2dBatch, Sprite2dInstanceGpu, Sprite2dPipeline, Sprite2dUniforms,
};
pub use terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
pub use water_body::{river_points, RiverPoint, WaterBodyLook, WaterBodyMesh, WaterVertex};
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

    /// The material group (1) holds no samplers in either PBR shader: they
    /// share the light group's one `material_sampler`. wgpu 23's DX12 backend
    /// copies every bind group's samplers into a 2048-slot heap, so samplers
    /// in the material group cap a scene's distinct looks (Capitol #577).
    #[test]
    fn material_group_holds_no_samplers() {
        for (name, source) in [
            ("shader.wgsl", include_str!("shader.wgsl")),
            ("skinned_shader.wgsl", include_str!("skinned_shader.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source).expect(name);
            let mut material_sampler = None;
            for (_, var) in module.global_variables.iter() {
                let Some(binding) = &var.binding else { continue };
                if !matches!(module.types[var.ty].inner, naga::TypeInner::Sampler { .. }) {
                    continue;
                }
                assert_ne!(binding.group, 1, "{name}: a sampler in the material group");
                if var.name.as_deref() == Some("material_sampler") {
                    material_sampler = Some((binding.group, binding.binding));
                }
            }
            assert_eq!(material_sampler, Some((2, 4)), "{name}: material_sampler at group 2, binding 4");
        }
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
    fn particle_shader_wgsl_parses_and_validates() {
        let source = include_str!("particle_shader.wgsl");
        let module =
            naga::front::wgsl::parse_str(source).expect("particle_shader.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("particle_shader.wgsl failed validation");
    }

    #[test]
    fn terrain_shader_wgsl_parses() {
        let source = include_str!("terrain_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("terrain_shader.wgsl failed to parse");
    }

    #[test]
    fn ocean_shader_wgsl_parses_and_validates() {
        // Validated too: a failure here would otherwise only show as the
        // pipeline's catch_unwind quietly turning the ocean (and every water
        // body) off.
        let source = include_str!("ocean_shader.wgsl");
        let module =
            naga::front::wgsl::parse_str(source).expect("ocean_shader.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("ocean_shader.wgsl failed validation");
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
        // with film grain reusing the former _pad slot; the fog rows (aerial
        // tint, height fog density, ADR 0101) grew it to 304, and the ground
        // fog rows (ADR 0106) to 368. Any other size means the Rust struct
        // and the WGSL mirror have diverged.
        let size = std::mem::size_of::<crate::postprocess::PostProcessUniforms>();
        assert_eq!(size % 16, 0, "PostProcessUniforms not 16-byte aligned");
        assert_eq!(size, 368, "PostProcessUniforms size changed");
    }

    #[test]
    fn post_process_uniforms_offsets_match_the_wgsl() {
        // Every member of the WGSL struct sits where the Rust one puts it.
        use crate::postprocess::PostProcessUniforms as U;
        use std::mem::offset_of;
        let module = naga::front::wgsl::parse_str(include_str!("composite_shader.wgsl")).unwrap();
        let members = module
            .types
            .iter()
            .find_map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { members, .. }
                    if ty.name.as_deref() == Some("PostProcessUniforms") =>
                {
                    Some(members.clone())
                }
                _ => None,
            })
            .expect("PostProcessUniforms in the WGSL");
        let offset = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("{name} not in the WGSL"))
                .offset as usize
        };
        for (name, rust) in [
            ("fog_tint_color", offset_of!(U, fog_tint_color)),
            ("fog_height_density", offset_of!(U, fog_height_density)),
            ("ground_fog_color", offset_of!(U, ground_fog_color)),
            ("ground_fog_density", offset_of!(U, ground_fog_density)),
            ("ground_fog_origin", offset_of!(U, ground_fog_origin)),
            ("ground_fog_size", offset_of!(U, ground_fog_size)),
            (
                "ground_fog_floor_base",
                offset_of!(U, ground_fog_floor_base),
            ),
            ("ground_fog_ceiling", offset_of!(U, ground_fog_ceiling)),
            ("ground_fog_falloff", offset_of!(U, ground_fog_falloff)),
            ("ground_fog_steps", offset_of!(U, ground_fog_steps)),
            ("ground_fog_texel", offset_of!(U, ground_fog_texel)),
            ("ground_fog_max_lod", offset_of!(U, ground_fog_max_lod)),
            ("ground_fog_enabled", offset_of!(U, ground_fog_enabled)),
        ] {
            assert_eq!(offset(name), rust, "{name}");
        }
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
        let module =
            naga::front::wgsl::parse_str(source).expect("grass_compute.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("grass_compute.wgsl failed validation");

        // The uniforms the Rust side writes must match the shader's struct
        let span = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("GrassComputeUniforms"))
            .map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { span, .. } => *span,
                _ => panic!("GrassComputeUniforms is not a struct"),
            })
            .expect("GrassComputeUniforms in grass_compute.wgsl");
        assert_eq!(
            span as usize,
            std::mem::size_of::<crate::GrassComputeUniforms>()
        );
    }

    #[test]
    fn grass_render_shader_wgsl_parses() {
        let source = include_str!("grass_render.wgsl");
        naga::front::wgsl::parse_str(source).expect("grass_render.wgsl failed to parse");
    }
}
