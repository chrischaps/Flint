// The entity style mask (#263): redraws styled entities after the scene
// pass, writing each one's style id (.r) and blend (.g, #278: 65535 = the
// style fully on) into an Rg16Uint target.
//
// No depth attachment: a fragment tests itself against the scene's
// single-sample depth (the resolve under MSAA, the scaled depth under a
// render scale), with a tolerance of its own depth slope. The vertex
// stages repeat shader.wgsl's and skinned_shader.wgsl's products in the
// same order, so the depths match wherever the rasterisers agree.
//
// A transparent or a particle in front of a styled entity (glass, fire)
// unmasks it by its coverage (#290): the veil pass drew that coverage just
// before, and the blend written here is the entity's times what's left.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct MaskDraw {
    id: u32,
    // How far behind the scene's depth a fragment may be, in depth slopes
    slope_tolerance: f32,
    blend: u32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> transform: TransformUniforms;
@group(1) @binding(0) var<uniform> draw: MaskDraw;
@group(1) @binding(1) var scene_depth: texture_depth_2d;
// The style veil (#290): how much of each pixel transparents and particles
// in front of the scene cover, 0..1. That share of the pixel is theirs, not
// the styled entity's, so the entity's blend is scaled down by it.
@group(1) @binding(2) var veil: texture_2d<f32>;
// Only the skinned entry point reads it; the static layout leaves it out
@group(2) @binding(0) var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct StaticVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct SkinnedVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) joint_indices: vec4<u32>,
    @location(5) joint_weights: vec4<f32>,
};

@vertex
fn vs_static(in: StaticVertex) -> @builtin(position) vec4<f32> {
    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    return transform.view_proj * world_pos;
}

@vertex
fn vs_skinned(in: SkinnedVertex) -> @builtin(position) vec4<f32> {
    let j = in.joint_indices;
    let w = in.joint_weights;
    let skin = bone_matrices[j.x] * w.x
             + bone_matrices[j.y] * w.y
             + bone_matrices[j.z] * w.z
             + bone_matrices[j.w] * w.w;
    let world_pos = transform.model * (skin * vec4<f32>(in.position, 1.0));
    return transform.view_proj * world_pos;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec2<u32> {
    let slope = fwidth(frag.z);
    let scene = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    if frag.z > scene + slope * draw.slope_tolerance + 1e-6 {
        discard;
    }
    let covered = clamp(textureLoad(veil, vec2<i32>(frag.xy), 0).r, 0.0, 1.0);
    let blend = u32(round(f32(draw.blend) * (1.0 - covered)));
    return vec2<u32>(draw.id, blend);
}

// Under MSAA (#306) the mask is drawn 4x, into a target with the scene's
// sample pattern, and each covered sample tests itself against that
// sample of the scene's own multisampled depth. One invocation a pixel:
// the rasteriser's coverage (`covered`) says which samples the triangle
// reaches, and the output mask keeps the ones that pass. `frag.z` is the
// pixel centre's depth, at most half a slope from any sample's, which
// the tolerance absorbs. style_mask_resolve.wgsl then folds the samples
// into the single-sample mask the styles read. The veil (binding 2) stays
// single-sample: every sample of a pixel is scaled by the same coverage.
@group(1) @binding(3) var scene_depth_ms: texture_depth_multisampled_2d;

struct MsaaOut {
    @location(0) value: vec2<u32>,
    @builtin(sample_mask) samples: u32,
};

@fragment
fn fs_msaa(
    @builtin(position) frag: vec4<f32>,
    @builtin(sample_mask) covered: u32,
) -> MsaaOut {
    let slope = fwidth(frag.z);
    let px = vec2<i32>(frag.xy);
    let limit = frag.z - slope * draw.slope_tolerance - 1e-6;
    var keep = 0u;
    for (var i = 0u; i < 4u; i++) {
        let bit = 1u << i;
        if (covered & bit) != 0u && textureLoad(scene_depth_ms, px, i32(i)) >= limit {
            keep |= bit;
        }
    }
    if keep == 0u {
        discard;
    }
    let veiled = clamp(textureLoad(veil, px, 0).r, 0.0, 1.0);
    var out: MsaaOut;
    out.value = vec2<u32>(draw.id, u32(round(f32(draw.blend) * (1.0 - veiled))));
    out.samples = keep;
    return out;
}
