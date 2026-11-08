// The scissored pass's copy (#385): each pixel of the input, unchanged,
// read with textureLoad as a style's passthrough reads it, so a pixel the
// blit writes is the one the style would have returned. Group 0 is a
// `kit::sampled_layout` (the style's own input bind group); the sampler
// is unused.

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(input_texture, vec2<i32>(pos.xy), 0);
}
