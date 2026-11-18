// Style images (#390, stylize/images.rs): papers or stroke strips as
// layers of an Rgba8Unorm texture_2d_array of display values (#547), each
// at its own size in its layer's corner with its own CPU-made mip chain,
// read with textureLoad, so no style needs a sampler of its own and a
// paper tiles by integer wrap. A layer is described by a StyleImageLayer
// (style_image_layer.wgsl). Concatenated with the shader that uses it.

fn img_fetch(t: texture_2d_array<f32>, layer: i32, q: vec2<i32>, size: vec2<i32>, level: i32, wrap: bool, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
    var c: vec2<i32>;
    if wrap {
        c = ((q % size) + size) % size;
    } else {
        c = clamp(q, lo, hi);
    }
    return textureLoad(t, c, layer, level);
}

// A layer's level-0 size, in texels.
fn img_size(info: StyleImageLayer) -> vec2<f32> {
    return vec2<f32>(info.size.xy);
}

// A bilinear read of layer `layer` (described by `info`) at `p`, in its
// level-0 texels, from the level nearest `lod` (log2 of texels per pixel).
// `wrap` tiles the image; otherwise reads stay inside the level-0
// rectangle lo0..hi0 (a row of a stroke strip). Level k of a layer is its
// level 0 halved k times, as the GPU sizes a texture's mips.
fn img_read(t: texture_2d_array<f32>, layer: i32, info: StyleImageLayer, p: vec2<f32>, lod: f32, wrap: bool, lo0: vec2<f32>, hi0: vec2<f32>) -> vec4<f32> {
    let levels = i32(max(info.size.z, 1u));
    let level = clamp(i32(round(lod)), 0, levels - 1);
    let size = vec2<i32>(max(info.size.xy >> vec2<u32>(u32(level)), vec2<u32>(1u)));
    let scale = vec2<f32>(size) / vec2<f32>(info.size.xy);
    let q = p * scale - 0.5;
    let i = floor(q);
    let f = q - i;
    let i0 = vec2<i32>(i);
    let lo = vec2<i32>(floor(lo0 * scale));
    let hi = max(vec2<i32>(ceil(hi0 * scale)) - 1, lo);
    let a = img_fetch(t, layer, i0, size, level, wrap, lo, hi);
    let b = img_fetch(t, layer, i0 + vec2<i32>(1, 0), size, level, wrap, lo, hi);
    let c = img_fetch(t, layer, i0 + vec2<i32>(0, 1), size, level, wrap, lo, hi);
    let d = img_fetch(t, layer, i0 + vec2<i32>(1, 1), size, level, wrap, lo, hi);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// The layer a row's image lane names (its layer plus one, > 0.5), in
// range of a stack's MAX_LAYERS (4) descriptions.
fn img_layer(lane: f32) -> i32 {
    return clamp(i32(lane + 0.5) - 1, 0, 3);
}

// A paper's height, 0 (valley) to 1 (tooth), from its luma measured
// against the image's own mean and spread: any scan comes out at the same
// strength, and the style's grain alone says how much of it shows.
fn img_paper_height(c: vec4<f32>, mean: f32, spread: f32) -> f32 {
    let l = flint_luma(c.rgb);
    return clamp(0.5 + 0.2 * (l - mean) / max(spread, 1e-3), 0.0, 1.0);
}

// A paper's hue, at the brightness of its mean: the scan's tint without
// its shading (the height carries that).
fn img_paper_tint(c: vec4<f32>) -> vec3<f32> {
    let l = max(flint_luma(c.rgb), 0.02);
    return clamp(c.rgb / l, vec3<f32>(0.0), vec3<f32>(1.5));
}
