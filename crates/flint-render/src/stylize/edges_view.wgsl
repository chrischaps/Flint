// Edges view (#301): the shared geometry edge texture made visible. The
// scene fades to a dim grey; each edge pixel takes a hue from its
// direction (orientation, so the two sides of a line agree: a vertical
// silhouette and its neighbour one pixel over read the same colour),
// silhouettes at full strength, creases a little softer, and anything
// under the threshold a faint ghost of its strength, so the margin
// between "almost an edge" and "an edge" is visible while tuning.
//
// It runs at output resolution and reads the edges (the depth's size) by
// UV, as any display-stage style reading them does.

struct EdgesViewUniforms {
    // 0..1: brightness of the scene under the edges
    dim: f32,
    // Strength that counts as an edge (1 = the shared thresholds)
    threshold: f32,
    // 0..1: how much of a sub-threshold strength shows
    ghost: f32,
    srgb: f32,
};

@group(0) @binding(0) var<uniform> params: EdgesViewUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var edges: texture_2d<f32>;

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

// A hue for an orientation: the doubled angle, so d and -d agree.
// Branchless (the usual hsv-to-rgb ramp): the six-case `switch` this was
// compiled wrong under DX12's FXC, which zeroed the red channel of pixels
// that took none of its colour (#314)
fn orientation_color(d: vec2<f32>) -> vec3<f32> {
    let a = atan2(2.0 * d.x * d.y, d.x * d.x - d.y * d.y); // -pi..pi
    let h = a / 6.2831853 + 0.5; // 0..1
    let ramp = abs(fract(vec3<f32>(h) + vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0;
    let c = clamp(ramp, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec3<f32>(0.2) + 0.8 * c;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(input_texture, input_sampler, in.uv);
    let rgb = clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let shown = select(rgb, to_display(rgb), params.srgb > 0.5);
    let luma = dot(shown, vec3<f32>(0.2126, 0.7152, 0.0722));
    var out = vec3<f32>(luma * params.dim);

    let size = vec2<i32>(textureDimensions(edges));
    let p = clamp(vec2<i32>(in.uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let e = textureLoad(edges, p, 0);
    let t = max(params.threshold, 1e-3);
    let silhouette = e.r / t;
    let crease = e.g / t;
    let hue = orientation_color(e.ba);
    if silhouette >= 1.0 {
        out = hue;
    } else if crease >= 1.0 {
        out = mix(out, hue, 0.8);
    } else {
        let s = clamp(max(silhouette, crease), 0.0, 1.0);
        out = mix(out, hue, s * s * params.ghost);
    }

    let encoded = select(out, to_linear(out), params.srgb > 0.5);
    return vec4<f32>(encoded, src.a);
}
