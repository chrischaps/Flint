// The scene's lights and shadow cascades as the GPU sees them: the one WGSL
// declaration of `LightUniforms` and `ShadowUniforms` (#738). Every lit
// shader is assembled with this text in front of it (`wgsl_sources.rs`), so
// the layout is written here once and held to the Rust structs
// (`pipeline::LightUniforms`, `shadow::ShadowUniforms`) by the
// `wgsl_layouts_match_rust` test. Declares no bindings: `light_bindings.wgsl`.

struct DirectionalLight {
    direction: vec3<f32>,
    volumetric_intensity: f32,
    color: vec3<f32>,
    intensity: f32,
    volumetric_color: vec3<f32>,
    _pad1: f32,
};

struct PointLight {
    position: vec3<f32>,
    radius: f32,
    color: vec3<f32>,
    intensity: f32,
    // Physical source radius (world units); 0 = punctual (ADR 0056).
    source_radius: f32,
    // 1.0 on the point light whose cube shadow map is drawn (#1112)
    shadow: f32,
    _pad1: f32,
    _pad2: f32,
};

struct SpotLight {
    position: vec3<f32>,
    radius: f32,
    direction: vec3<f32>,
    inner_angle: f32,
    color: vec3<f32>,
    outer_angle: f32,
    intensity: f32,
    // Physical source radius (world units); 0 = punctual (ADR 0056).
    source_radius: f32,
    // 1.0 on the spot whose shadow map is drawn (#504)
    shadow: f32,
    _pad2: f32,
};

struct LightUniforms {
    directional_lights: array<DirectionalLight, 4>,
    point_lights: array<PointLight, 16>,
    spot_lights: array<SpotLight, 8>,
    directional_count: u32,
    point_count: u32,
    spot_count: u32,
    _pad: u32,
    // w = 1 + diffuse wrap (ADR 0044), so a legacy write of 1.0 is off
    ambient_sky: vec4<f32>,
    // w = 1 + Oren-Nayar blend (ADR 0048), the same encoding
    ambient_ground: vec4<f32>,
    // rgb = sheen tint, w = strength; zero = off (ADR 0048)
    sheen_color_strength: vec4<f32>,
};

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    // x, y = the cascades' split depths, z = the shadow far plane (0 = no
    // shadows), w = one texel of the map in UV (0 = unset -> 1/2048)
    cascade_splits: vec4<f32>,
    // PCSS (ADR 0057): xyz = per-cascade light-ortho depth range (world
    // units), w = tan(sun angular size); w = 0 -> legacy 3x3 PCF verbatim.
    pcss: vec4<f32>,
    // The spot's shadow (#504): world to layer 3's clip space, and
    // (near, far, tan of the half-angle, 1 when drawn); w = 0 -> no spot shadow.
    spot_view_proj: mat4x4<f32>,
    spot: vec4<f32>,
    // The point light's cube (#1112): world to each face's clip space (+X,
    // -X, +Y, -Y, +Z, -Z; face f at column f % 3, row f / 3 of layer 4),
    // (near, far, tan of a face's half-angle, 1 when drawn), and (a face's
    // width as a share of the layer, its width in texels, -, -).
    point_view_proj: array<mat4x4<f32>, 6>,
    point: vec4<f32>,
    point_atlas: vec4<f32>,
};
