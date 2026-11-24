//! Flint Render - wgpu-based renderer for visualizing scenes
//!
//! This crate provides a PBR 3D renderer for previewing Flint scenes,
//! rendering entities as colored shapes or imported glTF models with
//! physically-based Cook-Torrance shading. Supports GPU vertex skinning
//! for skeletal animation.

// The lib's GPU unit tests share one device through the harness the
// integration binaries use (#1275): a device per test hung this binary in
// device creation (#598's loader-lock deadlock). The harness names the
// crate as they do, hence the alias.
#[cfg(test)]
extern crate self as flint_render;
#[cfg(test)]
#[path = "../tests/shared_gpu/mod.rs"]
mod shared_gpu;

pub mod atlas;
pub(crate) mod billboard_pipeline;
pub mod bind_cache;
pub mod bitmap_font;
mod camera;
mod context;
mod debug;
pub mod entity_style;
mod file_watch;
pub mod frame_capture;
#[cfg(not(target_arch = "wasm32"))]
mod frame_capture_write;
pub mod frame_replay;
mod frame_taps;
pub mod frustum;
pub mod glyph_atlas;
pub mod gpu_counts;
pub mod gpu_labels;
mod gpu_mesh;
pub mod gpu_select;
pub mod gpu_timer;
mod gpu_wait;
pub(crate) mod grass_pipeline;
pub mod ground_fog;
mod headless;
pub mod look;
pub mod look_fade;
pub mod model_loader;
pub mod motion_hold;
pub(crate) mod ocean_pipeline;
pub mod orbit_controller;
pub mod palette;
pub mod panels;
pub(crate) mod particle_pipeline;
pub mod pass_knobs;
mod pipeline;
pub mod pipeline_cache;
pub mod pipeline_twins;
pub mod post_style;
pub mod postprocess;
mod primitives;
pub mod quality;
pub mod render_mode;
pub mod render_scale;
pub mod render_stats;
mod scene_renderer;
pub(crate) mod selection_outline;
pub(crate) mod shadow;
pub(crate) mod skinned_pipeline;
pub(crate) mod sky_pipeline;
pub(crate) mod skybox_pipeline;
pub mod split_view;
pub(crate) mod sprite2d_pipeline;
pub(crate) mod style_albedo;
pub mod style_mask;
pub mod stylize;
mod surface_frame;
pub(crate) mod terrain_pipeline;
mod texture_cache;
mod trusted_shader;
pub mod water_body;
mod wgsl_sources;
mod wooze_pipeline;
pub(crate) mod world_line_pipeline;

pub use atlas::{AtlasRegistry, TextureAtlas};
pub use bind_cache::{DepthKey, DepthTarget};
pub use bitmap_font::{anchor_origin, apply_fill, BitmapFont};
pub use camera::{Camera, CameraMode};
pub use context::{resolve_present_mode, PresentOptions, RenderContext, RenderError, Vsync};
pub use debug::{DebugMode, DebugState};
pub use frustum::Frustum;
pub use glyph_atlas::{
    EdgeGlyphs, GlyphAtlas, GlyphError, GlyphRamp, GlyphSet, GlyphSetCache, GpuGlyphAtlas, RampMap,
};
pub use gpu_mesh::{GpuMesh, GpuSkinnedMesh, MeshCache, KEPT_GEOMETRY_LABEL};
pub use gpu_select::{list_adapters, no_adapter_hint, set_gpu_override};
pub use gpu_timer::{
    GpuFrameTime, GpuPassTime, GpuScope, GpuSpread, GpuTimer, GpuTimings, PassSpread,
    TimestampHealth, TracedGpuFrame, TracedPass, TIMESTAMPS_UNAVAILABLE,
};
pub use gpu_wait::{gpu_wait_timeout, wait_for_gpu};
pub use grass_pipeline::{GrassEntityPosition, GrassGround, GrassHeight, GrassInstanceGpu};
pub use ground_fog::GroundFog;
pub use headless::HeadlessContext;
pub use ocean_pipeline::{OceanTint, OceanVisuals};
pub use orbit_controller::OrbitCameraController;
pub use particle_pipeline::{ParticleBlendMode, ParticleDrawData, ParticleInstance};
pub use pass_knobs::PassKnobs;
pub use pipeline::LightUniforms;
pub use postprocess::{post_process_config_from_def, PostProcessConfig, HDR_FORMAT};
pub use primitives::{
    create_box_mesh, create_plane_mesh, generate_normal_arrows, generate_skeleton_lines,
    generate_skeleton_lines_colored, srgb_to_linear, triangles_to_wireframe_indices, Mesh,
    SkinnedMesh, SkinnedVertex, Vertex,
};
pub use quality::{gpu_info, GpuInfo, QualitySettings, QualityValue};
pub use render_scale::{clamp_render_scale, scaled_size, MIN_RENDER_SCALE};
pub use render_stats::{format_count, RenderStats};
pub use scene_renderer::{
    load_grass_ground_in, load_scene_grass_ground, load_scene_particle_textures,
    load_scene_splines, BoundGrass, BoundScene, DrawControls, DrawLimit, DrawTimesRead, FrameHost,
    FrameInputs, HeldFrameCapture, LightingLevers, LoadedGrassGround, LoadedSplines, LoadedTerrain,
    LodSettings, RendererConfig, SceneBinding, SceneRenderer, SplineCollider, SteppedPass, View2d,
    DEFAULT_CLEAR_COLOR,
};
/// The most bones one skinned mesh's buffer holds.
pub use skinned_pipeline::MAX_BONES;
pub use sky_pipeline::SkyParams;
pub use split_view::SplitView;

pub use style_mask::{StyleIds, STYLE_MASK_FORMAT};
pub use surface_frame::{AcquireFrame, SurfaceError};
pub use texture_cache::{memoize_mip_chains, open_image, TextureCache, KEPT_TEXTURE_LABEL};
pub use trusted_shader::TrustedShader;
pub use water_body::{river_points, RiverPoint, WaterBodyLook, WaterBodyMesh, WaterVertex};
pub use wooze_pipeline::{ShallowWoozeDraw, WAVELENGTH_MIN};
/// How far a world line's ends are pulled toward the eye, as a share of
/// their distance (ADR 0083), so a line or text lying on a surface
/// doesn't fight it for depth. The player's world text uses it too.
pub use world_line_pipeline::DEPTH_SLACK as WORLD_LINE_DEPTH_SLACK;

#[cfg(test)]
mod tests {
    /// The lib's tests take the binary's one device (#1275): only the two
    /// hosts' contexts ask for a device, and nothing here makes a
    /// `HeadlessContext` (each is a device of its own). A device per test
    /// hung this binary in `vkCreateDevice` against a thread's exit (#598).
    #[test]
    #[allow(clippy::disallowed_methods)] // a test reading the crate's own source tree
    fn only_the_hosts_contexts_make_a_device() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        // Spelled in pieces so this file doesn't find itself
        let asks = [
            format!(".{}(", "request_device"),
            format!("HeadlessContext::{}(", "new"),
            format!("HeadlessContext::{}(", "with_features"),
        ];
        let mut found = Vec::new();
        for file in &files {
            let name = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                let host = matches!(name.as_str(), "context.rs" | "headless.rs");
                if asks.iter().any(|a| line.contains(a.as_str())) && !host {
                    found.push(format!("src/{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "a lib test takes `crate::shared_gpu::lease()` (#1275), not a device \
             of its own:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn shader_wgsl_parses_and_validates() {
        let source = crate::wgsl_sources::SCENE;
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
            ("shader.wgsl", crate::wgsl_sources::SCENE),
            ("skinned_shader.wgsl", crate::wgsl_sources::SKINNED),
        ] {
            let module = naga::front::wgsl::parse_str(source).expect(name);
            let mut material_sampler = None;
            for (_, var) in module.global_variables.iter() {
                let Some(binding) = &var.binding else {
                    continue;
                };
                if !matches!(module.types[var.ty].inner, naga::TypeInner::Sampler { .. }) {
                    continue;
                }
                assert_ne!(binding.group, 1, "{name}: a sampler in the material group");
                if var.name.as_deref() == Some("material_sampler") {
                    material_sampler = Some((binding.group, binding.binding));
                }
            }
            assert_eq!(
                material_sampler,
                Some((2, 4)),
                "{name}: material_sampler at group 2, binding 4"
            );
        }
    }

    /// Both of the shading boil's passes, on the validator the player's GPU runs.
    #[test]
    fn shading_boil_shaders_validate() {
        // With the helpers they're built with (#743)
        for source in [
            concat!(
                include_str!("stylize/prelude_core.wgsl"),
                include_str!("stylize/noise.wgsl"),
                include_str!("stylize/edges_read.wgsl"),
                include_str!("stylize/contour.wgsl")
            ),
            concat!(
                include_str!("stylize/noise.wgsl"),
                include_str!("stylize/kuwahara.wgsl")
            ),
        ] {
            let module = naga::front::wgsl::parse_str(source).expect("failed to parse");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("failed validation");
        }
    }

    #[test]
    fn shadow_shader_wgsl_parses() {
        let source = crate::wgsl_sources::SHADOW;
        naga::front::wgsl::parse_str(source).expect("shadow_shader.wgsl failed to parse");
    }

    #[test]
    fn skinned_shader_wgsl_parses() {
        let source = crate::wgsl_sources::SKINNED;
        naga::front::wgsl::parse_str(source).expect("skinned_shader.wgsl failed to parse");
    }

    #[test]
    fn billboard_shader_wgsl_parses() {
        let source = include_str!("billboard_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("billboard_shader.wgsl failed to parse");
    }

    #[test]
    fn particle_shader_wgsl_parses_and_validates() {
        let source = crate::wgsl_sources::PARTICLE;
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
        let source = crate::wgsl_sources::TERRAIN;
        naga::front::wgsl::parse_str(source).expect("terrain_shader.wgsl failed to parse");
    }

    #[test]
    fn ocean_shader_wgsl_parses_and_validates() {
        // Validated too: a failure here would otherwise only show as the
        // pipeline's catch_unwind quietly turning the ocean (and every water
        // body) off.
        let source = crate::wgsl_sources::OCEAN;
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
    fn skinned_wire_shader_wgsl_parses() {
        let source = crate::wgsl_sources::WIRE;
        naga::front::wgsl::parse_str(source).expect("skinned_wire_shader.wgsl failed to parse");
    }

    #[test]
    fn skybox_shader_wgsl_parses() {
        let source = include_str!("skybox_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("skybox_shader.wgsl failed to parse");
    }

    #[test]
    fn composite_shader_wgsl_parses_and_validates() {
        // Both: the plain composite and the fog veil's (#513)
        for source in [
            crate::postprocess::COMPOSITE_WGSL,
            crate::postprocess::COMPOSITE_VEILED_WGSL,
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .expect("composite_shader.wgsl failed to parse");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("composite_shader.wgsl failed validation");
        }
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
        // (strength/focus/range/_pad2) and the post time row (the render
        // modes' two rows until #746 made them styles) were appended after
        // inv_view_proj (208); the color-grade rows (lift+enabled /
        // gamma+pad / gain+pad, ADR 0050) grew it to 256, with film grain
        // reusing the former _pad slot; the fog rows (aerial tint, height fog
        // density, ADR 0101) grew it to 288, and the ground fog rows (ADR
        // 0106) to 352. Any other size means the Rust struct and the WGSL
        // mirror have diverged.
        let size = std::mem::size_of::<crate::postprocess::PostProcessUniforms>();
        assert_eq!(size % 16, 0, "PostProcessUniforms not 16-byte aligned");
        assert_eq!(size, 352, "PostProcessUniforms size changed");
    }

    #[test]
    fn post_process_uniforms_offsets_match_the_wgsl() {
        // Every member of the WGSL struct sits where the Rust one puts it.
        use crate::postprocess::PostProcessUniforms as U;
        use std::mem::offset_of;
        let module = naga::front::wgsl::parse_str(crate::postprocess::COMPOSITE_WGSL).unwrap();
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
        // LightUniforms is declared once in WGSL, lights.wgsl (#738), and
        // `wgsl_sources::tests::wgsl_layouts_match_rust` holds its size to
        // this struct's in every shader; bind groups use min_binding_size:
        // None, so a divergence would surface only at draw time. The
        // sheen vec4 (ADR 0048) grew the struct from 1264 to 1280 bytes;
        // PointLight's source_radius row (ADR 0056) grew it to 1536
        // (16 point lights × 16 extra bytes).
        let size = std::mem::size_of::<crate::pipeline::LightUniforms>();
        assert_eq!(size % 16, 0, "LightUniforms not 16-byte aligned");
        assert_eq!(
            size, 1536,
            "LightUniforms size changed — update lights.wgsl"
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
        // ShadowUniforms is declared once in WGSL, lights.wgsl (#738); the
        // PCSS vec4 (ADR 0057) grew it 208 → 224 bytes, the spot's
        // matrix and vec4 (ADR 0124) to 304, and the point light's six face
        // matrices and two vec4s (ADR 0132) to 720.
        let size = std::mem::size_of::<crate::shadow::ShadowUniforms>();
        assert_eq!(size % 16, 0, "ShadowUniforms not 16-byte aligned");
        assert_eq!(
            size, 720,
            "ShadowUniforms size changed — update lights.wgsl"
        );
        assert_eq!(
            std::mem::offset_of!(crate::shadow::ShadowUniforms, spot_view_proj),
            224,
            "the spot's fields follow `pcss`"
        );
        assert_eq!(
            std::mem::offset_of!(crate::shadow::ShadowUniforms, point_view_proj),
            304,
            "the point light's fields follow the spot's"
        );
    }

    #[test]
    fn shadow_uniforms_default_point_is_off() {
        use bytemuck::Zeroable;
        // point.w = 0 lights every point light as before (#1112)
        assert_eq!(crate::shadow::ShadowUniforms::default().point, [0.0; 4]);
        assert_eq!(crate::shadow::ShadowUniforms::zeroed().point, [0.0; 4]);
    }

    #[test]
    fn point_shadow_mark_rides_the_old_pad() {
        // `PointLight::shadow` is the former `_pad0`: the struct stays 48 bytes
        use crate::pipeline::PointLight;
        assert_eq!(std::mem::size_of::<PointLight>(), 48);
        assert_eq!(std::mem::offset_of!(PointLight, shadow), 36);
        assert_eq!(PointLight::default().shadow, 0.0);
    }

    #[test]
    fn shadow_uniforms_default_spot_is_off() {
        use bytemuck::Zeroable;
        // spot.w = 0 lights every spot as before (#504)
        assert_eq!(crate::shadow::ShadowUniforms::default().spot, [0.0; 4]);
        assert_eq!(crate::shadow::ShadowUniforms::zeroed().spot, [0.0; 4]);
    }

    #[test]
    fn spot_shadow_mark_rides_the_old_pad() {
        // `SpotLight::shadow` is the former `_pad1`: the struct stays 64 bytes
        use crate::pipeline::SpotLight;
        assert_eq!(std::mem::size_of::<SpotLight>(), 64);
        assert_eq!(std::mem::offset_of!(SpotLight, shadow), 56);
        assert_eq!(SpotLight::default().shadow, 0.0);
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
            std::mem::size_of::<crate::grass_pipeline::GrassComputeUniforms>()
        );
    }

    #[test]
    fn grass_render_shader_wgsl_parses() {
        // As grass_pipeline.rs builds it: shallow wooze's noise first, for the
        // comb of woozy grass (#252).
        let source = crate::wgsl_sources::GRASS;
        let module =
            naga::front::wgsl::parse_str(source).expect("grass_render.wgsl failed to parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("grass_render.wgsl failed validation");

        // The uniforms the Rust side writes must match the shader's struct
        let span = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("GrassRenderUniforms"))
            .map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { span, .. } => *span,
                _ => panic!("GrassRenderUniforms is not a struct"),
            })
            .expect("GrassRenderUniforms in grass_render.wgsl");
        assert_eq!(
            span as usize,
            std::mem::size_of::<crate::grass_pipeline::GrassRenderUniforms>()
        );
    }
}
