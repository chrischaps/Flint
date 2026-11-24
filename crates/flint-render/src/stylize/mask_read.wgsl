// The style mask's readers (#1256), for every style that reads it and for
// the prelude a user style is compiled with (`custom/compose.rs`, the
// `flint_mask_*` functions ADR 0112 documents).
//
// The mask (`style_mask.rs`) is `Rg16Uint` at the scene's resolution: .r
// the entity's mask id (0 = no style), .g its blend 0..65535 (#278). Each
// shader that includes this declares it, at its own group and binding,
// under this name:
//
//     @group(G) @binding(B) var flint_mask_texture: texture_2d<u32>;
//
// WGSL finds a module's declarations in any order, so this file can come
// before or after that line. `mask_slot.wgsl` is the slot table's reader.

fn flint_mask_size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(flint_mask_texture));
}

// The mask at texel `p`, clamped to its edge: x = the id, y = the blend
// 0..65535
fn flint_mask_load(p: vec2<i32>) -> vec2<u32> {
    return textureLoad(flint_mask_texture, clamp(p, vec2<i32>(0), flint_mask_size() - 1), 0).rg;
}

// The mask texel under `uv` (0..1, y down), clamped to the mask
fn flint_mask_texel(uv: vec2<f32>) -> vec2<i32> {
    let dims = flint_mask_size();
    return clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1);
}

// The mask texel under pixel `p` of a picture `size` pixels big (the
// output, which a render scale makes larger than the mask). Not clamped:
// flint_mask_load clamps
fn flint_mask_texel_of(p: vec2<f32>, size: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(floor(p * vec2<f32>(flint_mask_size()) / size));
}

// The style mask id at uv (0 = no style; #263)
fn flint_mask_id(uv: vec2<f32>) -> u32 {
    return flint_mask_load(flint_mask_texel(uv)).x;
}

// A mask texel's blend, 0..1
fn flint_mask_blend(m: vec2<u32>) -> f32 {
    return f32(m.y) / 65535.0;
}
