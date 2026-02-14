// Skybox shader — equirectangular panorama
//
// Draws a fullscreen triangle at the far plane and samples a panoramic
// environment texture.  The inverse view-projection (rotation only)
// converts clip-space corners into world-space ray directions which are
// then mapped to equirectangular UV coordinates via atan2 / asin.

struct SkyboxUniforms {
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: SkyboxUniforms;
@group(1) @binding(0) var panorama: texture_2d<f32>;
@group(1) @binding(1) var panorama_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ray_dir: vec3<f32>,
};

@vertex
fn vs_skybox(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle: 3 vertices cover the whole screen
    //   0 → (-1, -1)   1 → ( 3, -1)   2 → (-1,  3)
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    let clip = vec4<f32>(x, y, 1.0, 1.0);    // depth = 1.0 (far plane)
    let world = uniforms.inv_view_proj * clip;
    let dir = world.xyz / world.w;

    var out: VsOut;
    out.position = vec4<f32>(x, y, 1.0, 1.0); // stays at far plane
    out.ray_dir = dir;
    return out;
}

const PI: f32 = 3.14159265359;

@fragment
fn fs_skybox(in: VsOut) -> @location(0) vec4<f32> {
    let dir = normalize(in.ray_dir);

    // Equirectangular mapping: direction → UV
    let u = atan2(dir.x, -dir.z) / (2.0 * PI) + 0.5;
    let v = 0.5 - asin(dir.y) / PI;

    return textureSample(panorama, panorama_sampler, vec2<f32>(u, v));
}
