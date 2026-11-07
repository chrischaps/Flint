// Mask view (#263): the entity style mask made visible. Each style id gets
// a hue (style_mask::debug_color), laid over the scene's own shading so the
// shapes stay readable; unstyled pixels fade to a dim grey; where two ids
// meet, a white line marks the silhouette a per-entity style will cut.
//
// It runs at output resolution and reads the mask (scene resolution) by
// UV, so it shows the mask a display-stage style would read under a
// render scale.

struct MaskViewUniforms {
    // 0..1: how much of the scene's shading shows through a style's hue
    shading: f32,
    // 0..1: brightness of the unstyled scene
    dim: f32,
    // Silhouette line width in mask pixels (0 = none)
    outline: f32,
    srgb: f32,
};

@group(0) @binding(0) var<uniform> params: MaskViewUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var style_mask: texture_2d<u32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Must match style_mask::debug_color
fn id_color(id: u32) -> vec3<f32> {
    let h = fract(f32(id) * 0.618034) * 6.0;
    let x = 1.0 - abs((h % 2.0) - 1.0);
    var c: vec3<f32>;
    switch u32(h) {
        case 0u: { c = vec3<f32>(1.0, x, 0.0); }
        case 1u: { c = vec3<f32>(x, 1.0, 0.0); }
        case 2u: { c = vec3<f32>(0.0, 1.0, x); }
        case 3u: { c = vec3<f32>(0.0, x, 1.0); }
        case 4u: { c = vec3<f32>(x, 0.0, 1.0); }
        default: { c = vec3<f32>(1.0, 0.0, x); }
    }
    return vec3<f32>(0.25) + 0.75 * c;
}

fn mask_at(p: vec2<i32>, size: vec2<i32>) -> u32 {
    return textureLoad(style_mask, clamp(p, vec2<i32>(0), size - 1), 0).r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(input_texture, input_sampler, in.uv);
    let rgb = clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let shown = select(rgb, to_display(rgb), params.srgb > 0.5);
    let luma = dot(shown, vec3<f32>(0.2126, 0.7152, 0.0722));

    let size = vec2<i32>(textureDimensions(style_mask));
    let p = vec2<i32>(in.uv * vec2<f32>(size));
    let id = mask_at(p, size);

    var out: vec3<f32>;
    if id == 0u {
        out = vec3<f32>(luma * params.dim);
    } else {
        // The hue, shaded by the scene so form still reads, and as strong
        // as the entity's blend (#278): a half-faded prop is half its hue
        let shade = mix(1.0, 0.35 + 0.9 * luma, params.shading);
        let blend = f32(textureLoad(style_mask, clamp(p, vec2<i32>(0), size - 1), 0).g) / 65535.0;
        out = mix(vec3<f32>(luma * params.dim), id_color(id) * shade, blend);
    }

    // A silhouette wherever a neighbour within `outline` mask pixels has
    // another id
    let r = i32(round(params.outline));
    if r > 0 {
        var edge = false;
        for (var dy = -r; dy <= r; dy++) {
            for (var dx = -r; dx <= r; dx++) {
                if mask_at(p + vec2<i32>(dx, dy), size) != id {
                    edge = true;
                }
            }
        }
        if edge {
            out = vec3<f32>(1.0);
        }
    }

    let encoded = select(out, to_linear(out), params.srgb > 0.5);
    return vec4<f32>(encoded, src.a);
}
