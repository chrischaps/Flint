// The editor's selection outline (#602), drawn over the finished image.
//
// Reads the selection mask (selection_mask.wgsl: .r covered, .g visible) at
// scene resolution and inks a band `width` output pixels wide:
//
//   * round the selection's silhouette, outside it, at full strength where
//     the selection beside it can be seen;
//   * at `hidden_alpha` where it is hidden behind something, so a selection
//     behind a wall still shows where it is.
//
// The band is measured in output pixels from the nearest mask texel, so it
// is the same width across and down the screen, at any render scale, and
// the caller sizes it in points. Drawn after tonemapping, bloom, fog and
// every style, the colour on screen is the colour asked for.

struct OutlineUniforms {
    // Premultiplied by nothing; alpha-blended. Linear when the target is sRGB.
    color: vec4<f32>,
    // Mask texels per output pixel, x and y
    mask_per_px: vec2<f32>,
    // Band width in output pixels
    width: f32,
    hidden_alpha: f32,
    // Mask size in texels
    mask_size: vec2<i32>,
    // Texels searched each side of the pixel
    reach: i32,
    _pad: i32,
};

@group(0) @binding(0) var<uniform> u: OutlineUniforms;
@group(0) @binding(1) var mask: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// How much of a band `width` wide reaches a pixel whose centre is `d`
// output pixels from the shape's edge: 1 inside it, a one-pixel ramp out.
fn band(d: f32) -> f32 {
    return clamp(u.width + 0.5 - d, 0.0, 1.0);
}

@fragment
fn fs_outline(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let at = frag.xy * u.mask_per_px;
    let here = vec2<i32>(floor(at));
    let own = textureLoad(mask, clamp(here, vec2<i32>(0), u.mask_size - 1), 0);
    // The selection's own pixels, seen or hidden, are never inked
    if own.r > 0.5 {
        discard;
    }

    // Nearest covered and nearest visible texel, in output pixels from the
    // pixel's centre to the texel's edge (half a texel short of its centre)
    let half_texel = 0.5 / max(u.mask_per_px.x, u.mask_per_px.y);
    var near_covered = 1e6;
    var near_visible = 1e6;
    for (var y = -u.reach; y <= u.reach; y++) {
        for (var x = -u.reach; x <= u.reach; x++) {
            let t = here + vec2<i32>(x, y);
            if any(t < vec2<i32>(0)) || any(t >= u.mask_size) {
                continue;
            }
            let m = textureLoad(mask, t, 0);
            if m.r < 0.5 {
                continue;
            }
            let centre = (vec2<f32>(t) + 0.5) / u.mask_per_px;
            let d = max(length(centre - frag.xy) - half_texel, 0.0);
            near_covered = min(near_covered, d);
            if m.g > 0.5 {
                near_visible = min(near_visible, d);
            }
        }
    }

    // The band goes round the whole silhouette and only outside it: full
    // where the selection beside it can be seen, faint where it is hidden.
    // Its own pixels were left alone above, so an occluder in front of it
    // (or everything standing on a selected floor) is not outlined.
    let a = max(band(near_visible), band(near_covered) * u.hidden_alpha);
    if a <= 0.0 {
        discard;
    }
    return vec4<f32>(u.color.rgb, u.color.a * a);
}
