// crates/flint-render/src/grass_render.wgsl
// Grass instanced rendering — stylized cross-quads with wind and bending

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct GrassRenderUniforms {
    wind_direction: vec3<f32>,
    wind_speed: f32,
    wind_strength: f32,
    time: f32,
    bend_radius: f32,
    bend_strength: f32,
    color_base: vec3<f32>,
    blade_width: f32,
    color_tip: vec3<f32>,
    blade_height: f32,
    color_dry: vec3<f32>,
    dry_amount: f32,
    entity_count: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

struct GrassInstance {
    position: vec3<f32>,
    rotation: f32,
    height: f32,
    tint: u32,
};

struct EntityPosition {
    position: vec3<f32>,
    _pad: f32,
};

// Must match LightUniforms layout in terrain_shader.wgsl and shader.wgsl
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
};

struct SpotLight {
    position: vec3<f32>,
    radius: f32,
    direction: vec3<f32>,
    inner_angle: f32,
    color: vec3<f32>,
    outer_angle: f32,
    intensity: f32,
    _pad0: f32,
    _pad1: f32,
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
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
};

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>,
};

// Bind group 0: Transform (shared)
@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Bind group 1: Grass render uniforms
@group(1) @binding(0)
var<uniform> grass: GrassRenderUniforms;

// Bind group 2: Lights (shared with terrain/PBR)
@group(2) @binding(0)
var<uniform> lights: LightUniforms;
@group(2) @binding(1)
var shadow_depth: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;
@group(2) @binding(3)
var<uniform> shadow_uniforms: ShadowUniforms;

// Bind group 3: Instance data + entity positions
@group(3) @binding(0)
var<storage, read> instances: array<GrassInstance>;
@group(3) @binding(1)
var<storage, read> entities: array<EntityPosition>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) world_normal: vec3<f32>,
};

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

@vertex
fn vs_main(
    vertex: VertexInput,
    @builtin(instance_index) instance_idx: u32,
) -> VertexOutput {
    let inst = instances[instance_idx];

    // Y-axis rotation matrix
    let cos_r = cos(inst.rotation);
    let sin_r = sin(inst.rotation);

    // Scale blade by instance height and uniform width
    var local_pos = vertex.position;
    local_pos.x *= grass.blade_width;
    local_pos.z *= grass.blade_width;
    local_pos.y *= inst.height;

    // Rotate around Y axis
    let rotated_x = local_pos.x * cos_r - local_pos.z * sin_r;
    let rotated_z = local_pos.x * sin_r + local_pos.z * cos_r;
    local_pos.x = rotated_x;
    local_pos.z = rotated_z;

    // Wind sway — increases with v² (tip moves most)
    let v = vertex.uv.y;
    let v_sq = v * v;
    let phase = hash21(inst.position.xz * 3.7) * 6.28318;
    let wind_offset = grass.wind_strength * v_sq * sin(grass.time * grass.wind_speed + phase);
    let wind_dir = normalize(grass.wind_direction.xz);
    local_pos.x += wind_offset * wind_dir.x;
    local_pos.z += wind_offset * wind_dir.y;

    // Entity bend-on-contact
    for (var i = 0u; i < min(grass.entity_count, 8u); i++) {
        let entity_pos = entities[i].position;
        let to_blade = inst.position.xz - entity_pos.xz;
        let dist = length(to_blade);
        if dist < grass.bend_radius && dist > 0.001 {
            let falloff = pow(1.0 - dist / grass.bend_radius, 2.0);
            let push = normalize(to_blade) * grass.bend_strength * falloff * v_sq;
            local_pos.x += push.x;
            local_pos.z += push.y;
        }
    }

    let world_pos = inst.position + local_pos;

    // Color: base-to-tip gradient with dry variation
    let base_color = mix(grass.color_base, grass.color_tip, v);
    let dry_noise = hash21(inst.position.xz * 5.3);
    let final_color = mix(base_color, grass.color_dry, dry_noise * grass.dry_amount);

    // Approximate normal (pointing mostly up, tilted by wind)
    let normal = normalize(vec3<f32>(-wind_offset * wind_dir.x * 0.3, 1.0, -wind_offset * wind_dir.y * 0.3));

    var out: VertexOutput;
    out.clip_pos = transform.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_pos = world_pos;
    out.uv = vertex.uv;
    out.color = final_color;
    out.world_normal = normal;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Alpha cutoff: blade shape from UV
    // Blade narrows toward tip (v=1). Discard outside blade silhouette.
    let u_centered = abs(in.uv.x - 0.5) * 2.0; // 0 at center, 1 at edge
    let blade_edge = 1.0 - in.uv.y * 0.7; // Edge threshold narrows with height
    if u_centered > blade_edge {
        discard;
    }

    var color = in.color;

    // Simple directional lighting
    if lights.directional_count > 0u {
        let light_dir = normalize(-lights.directional_lights[0].direction);
        let n_dot_l = max(dot(in.world_normal, light_dir), 0.0);

        // Shadow sampling (cascade 0 only for grass)
        let shadow_pos = shadow_uniforms.cascade_view_proj[0] * vec4<f32>(in.world_pos, 1.0);
        let shadow_ndc = shadow_pos.xyz / shadow_pos.w;
        let shadow_uv = shadow_ndc.xy * vec2<f32>(0.5, -0.5) + 0.5;
        var shadow = 1.0;
        if shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0 {
            shadow = textureSampleCompare(shadow_depth, shadow_sampler, shadow_uv, 0, shadow_ndc.z - 0.002);
        }

        let diffuse = n_dot_l * lights.directional_lights[0].intensity * shadow;

        // Subsurface scattering approximation — backlit tips glow
        let view_dir = normalize(transform.camera_pos - in.world_pos);
        let sss = pow(max(dot(view_dir, -light_dir), 0.0), 4.0) * in.uv.y * 0.3;

        let light_color = lights.directional_lights[0].color;
        // Use ambient_sky as ambient color with unit intensity
        let ambient = mix(lights.ambient_ground.rgb, lights.ambient_sky.rgb, 0.5);
        color *= (diffuse + sss) * light_color + ambient;
    } else {
        let ambient = mix(lights.ambient_ground.rgb, lights.ambient_sky.rgb, 0.5);
        color *= ambient;
    }

    return vec4<f32>(color, 1.0);
}

// Shadow pass vertex shader — same positioning, no fragment color
@vertex
fn vs_shadow(
    vertex: VertexInput,
    @builtin(instance_index) instance_idx: u32,
) -> @builtin(position) vec4<f32> {
    let inst = instances[instance_idx];

    var local_pos = vertex.position;
    local_pos.x *= grass.blade_width;
    local_pos.z *= grass.blade_width;
    local_pos.y *= inst.height;

    let cos_r = cos(inst.rotation);
    let sin_r = sin(inst.rotation);
    let rotated_x = local_pos.x * cos_r - local_pos.z * sin_r;
    let rotated_z = local_pos.x * sin_r + local_pos.z * cos_r;
    local_pos.x = rotated_x;
    local_pos.z = rotated_z;

    // Wind (same as main pass for shadow consistency)
    let v_sq = vertex.uv.y * vertex.uv.y;
    let phase = hash21(inst.position.xz * 3.7) * 6.28318;
    let wind_offset = grass.wind_strength * v_sq * sin(grass.time * grass.wind_speed + phase);
    let wind_dir = normalize(grass.wind_direction.xz);
    local_pos.x += wind_offset * wind_dir.x;
    local_pos.z += wind_offset * wind_dir.y;

    let world_pos = inst.position + local_pos;

    // Shadow uses transform.view_proj which will be set to the shadow cascade VP
    return transform.view_proj * vec4<f32>(world_pos, 1.0);
}
