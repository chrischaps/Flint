// Style images (#390, stylize/images.rs): a paper or a stroke strip as an
// Rgba8Unorm texture of display values with a CPU-made mip chain, read
// with textureLoad, so no style needs a sampler of its own and a paper
// tiles by integer wrap. Concatenated ahead of the shader that uses it.

fn img_fetch(t: texture_2d<f32>, q: vec2<i32>, size: vec2<i32>, level: i32, wrap: bool, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
    var c: vec2<i32>;
    if wrap {
        c = ((q % size) + size) % size;
    } else {
        c = clamp(q, lo, hi);
    }
    return textureLoad(t, c, level);
}

// A bilinear read at `p`, in level-0 texels, from the level nearest `lod`
// (log2 of texels per pixel). `wrap` tiles the image; otherwise reads
// stay inside the level-0 rectangle lo0..hi0 (a row of a stroke strip).
fn img_read(t: texture_2d<f32>, p: vec2<f32>, lod: f32, wrap: bool, lo0: vec2<f32>, hi0: vec2<f32>) -> vec4<f32> {
    let levels = i32(textureNumLevels(t));
    let level = clamp(i32(round(lod)), 0, levels - 1);
    let size = vec2<i32>(textureDimensions(t, level));
    let scale = vec2<f32>(size) / vec2<f32>(textureDimensions(t, 0));
    let q = p * scale - 0.5;
    let i = floor(q);
    let f = q - i;
    let i0 = vec2<i32>(i);
    let lo = vec2<i32>(floor(lo0 * scale));
    let hi = max(vec2<i32>(ceil(hi0 * scale)) - 1, lo);
    let a = img_fetch(t, i0, size, level, wrap, lo, hi);
    let b = img_fetch(t, i0 + vec2<i32>(1, 0), size, level, wrap, lo, hi);
    let c = img_fetch(t, i0 + vec2<i32>(0, 1), size, level, wrap, lo, hi);
    let d = img_fetch(t, i0 + vec2<i32>(1, 1), size, level, wrap, lo, hi);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// A paper's height, 0 (valley) to 1 (tooth), from its luma measured
// against the image's own mean and spread: any scan comes out at the same
// strength, and the style's grain alone says how much of it shows.
fn img_paper_height(c: vec4<f32>, mean: f32, spread: f32) -> f32 {
    let l = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    return clamp(0.5 + 0.2 * (l - mean) / max(spread, 1e-3), 0.0, 1.0);
}

// A paper's hue, at the brightness of its mean: the scan's tint without
// its shading (the height carries that).
fn img_paper_tint(c: vec4<f32>) -> vec3<f32> {
    let l = max(dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.02);
    return clamp(c.rgb / l, vec3<f32>(0.0), vec3<f32>(1.5));
}
