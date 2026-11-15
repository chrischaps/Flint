// Cyanotype (#419): a game's own Rust style, in two passes.
//
// A cyanotype is a sun print. Paper brushed with iron salts turns Prussian
// blue where light reaches it and stays white where something lies on it;
// Anna Atkins printed her algae and ferns this way in the 1840s. The print
// is soft, because light creeps under a leaf's edge, and the coating ends
// where the brush did.
//
//   fs_dose   the light the paper received: the picture's luminance, sharp
//             (r) and spread by the creep under the edges (g)
//   fs_print  the print: the dose read through the iron's tone curve, an
//             uneven coat, paper fibres and the brushed border
//
// `process` picks the reading: "photogram" (light turns blue, the things in
// front of it stay paper, as Atkins' plants do) or "print" (from a negative:
// light stays paper, shadow turns blue).

struct Uniforms {
    texel: vec2<f32>,
    aspect: f32,
    srgb: f32,
    exposure: f32,
    bleed: f32,
    coating: f32,
    grain: f32,
    strength: f32,
    // 1 = photogram, 0 = print
    photogram: f32,
    // Pixels per point: the creep is measured in points, so it reads the
    // same on any display
    ppp: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var dose_texture: texture_2d<f32>;
@group(2) @binding(1) var dose_sampler: sampler;

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

// What the viewer sees at this pixel, as display values
fn shown(uv: vec2<f32>) -> vec3<f32> {
    let c = clamp(textureSample(input_texture, input_sampler, uv).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb > 0.5);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// --- pass 1: the dose ---------------------------------------------------

@fragment
fn fs_dose(in: VertexOutput) -> @location(0) vec4<f32> {
    let sharp = luma(shown(in.uv));
    // Light creeping under an edge: a disc of 16 taps on a golden-angle
    // spiral, up to 9 points across at bleed 1
    let radius = u.bleed * 9.0 * u.ppp;
    var sum = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let f = f32(i);
        let r = radius * sqrt((f + 0.5) / 16.0);
        let a = f * 2.39996323;
        let offset = vec2<f32>(cos(a), sin(a)) * r * u.texel;
        sum = sum + luma(shown(in.uv + offset));
    }
    return vec4<f32>(sharp, sum / 16.0, 0.0, 1.0);
}

// --- pass 2: the print --------------------------------------------------

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.1031, 0.1030));
    let r = q + dot(q, q.yx + 33.33);
    return fract((r.x + r.y) * r.x);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0u; i < 4u; i = i + 1u) {
        total = total + amp * noise(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        amp = amp * 0.5;
    }
    return total;
}

// Prussian blue at full dose, through the paler blues, to the paper
const DEEP: vec3<f32> = vec3<f32>(0.035, 0.105, 0.265);
const BLUE: vec3<f32> = vec3<f32>(0.090, 0.270, 0.520);
const PALE: vec3<f32> = vec3<f32>(0.520, 0.680, 0.800);
const PAPER: vec3<f32> = vec3<f32>(0.955, 0.940, 0.890);
// The paper past the coating: a touch warmer, unwashed
const BARE: vec3<f32> = vec3<f32>(0.940, 0.915, 0.850);

// `blue` 0 = untouched paper, 1 = the deepest blue the iron gives
fn tone(blue: f32) -> vec3<f32> {
    let t = clamp(blue, 0.0, 1.0);
    if (t < 0.35) {
        return mix(PAPER, PALE, smoothstep(0.0, 0.35, t));
    }
    if (t < 0.7) {
        return mix(PALE, BLUE, smoothstep(0.35, 0.7, t));
    }
    return mix(BLUE, DEEP, smoothstep(0.7, 1.0, t));
}

@fragment
fn fs_print(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(input_texture, input_sampler, in.uv);
    let original = shown(in.uv);
    let dose = textureSample(dose_texture, dose_sampler, in.uv).rg;

    // Light finds its way under edges: the brighter of the sharp and the
    // spread dose, by how much it creeps
    let light = mix(dose.x, max(dose.x, dose.y), u.bleed);
    // Exposure: longer in the sun, deeper the blue for the same light. The
    // iron's response has a toe and a shoulder: little light leaves the
    // paper white, and past a point it can't get bluer
    let reached = smoothstep(0.22, 0.86, pow(clamp(light, 0.0, 1.0), 1.0 / u.exposure));
    var blue = select(1.0 - reached, reached, u.photogram > 0.5);

    // The coat is never even: the brush laid more iron in places
    let p = vec2<f32>(in.uv.x * u.aspect, in.uv.y);
    let coat = 0.86 + 0.24 * fbm(p * 3.1 + vec2<f32>(4.0, 1.0));
    blue = blue * coat;

    // Paper fibres: long, thin, mostly across the sheet, paper showing
    // through the blue where they lie
    let fibre = noise(p * vec2<f32>(46.0, 520.0)) * noise(p * vec2<f32>(210.0, 31.0));
    let tooth = hash(floor(in.position.xy / max(u.ppp, 1.0)));
    blue = blue - u.grain * (0.16 * fibre + 0.05 * (tooth - 0.5));

    var color = tone(blue);

    // The brushed border: the coating stops short of each edge by
    // `coating` of the sheet's height, unevenly, in bristle streaks
    if (u.coating > 0.0) {
        let edge_x = min(p.x, u.aspect - p.x);
        let edge_y = min(p.y, 1.0 - p.y);
        // Along each edge, its own wander
        let wander_x = fbm(vec2<f32>(p.y * 7.0, 3.0 + step(u.aspect * 0.5, p.x) * 11.0));
        let wander_y = fbm(vec2<f32>(p.x * 7.0, 23.0 + step(0.5, p.y) * 11.0));
        let bristle_x = noise(vec2<f32>(p.y * 70.0, 5.0));
        let bristle_y = noise(vec2<f32>(p.x * 70.0, 7.0));
        let reach_x = u.coating * (0.6 + 0.75 * wander_x + 0.1 * bristle_x);
        let reach_y = u.coating * (0.6 + 0.75 * wander_y + 0.1 * bristle_y);
        // The coat thins toward its end before it stops
        let band = u.coating * 0.3;
        let inside = smoothstep(reach_x - band, reach_x + band, edge_x)
            * smoothstep(reach_y - band, reach_y + band, edge_y);
        // Where the bristles ran dry, a faint wash on the bare paper, in
        // strokes along the edge it belongs to
        let along = select(p.x, p.y, edge_x < edge_y);
        let across = select(edge_y, edge_x, edge_x < edge_y);
        let dry = fbm(vec2<f32>(along * 34.0, across * 5.0)) * 0.12 * (1.0 - inside)
            * smoothstep(reach_x * 0.3, reach_x, min(edge_x, edge_y) + band);
        color = mix(mix(BARE, PALE, dry), color, inside);
    }

    let out = mix(original, color, u.strength);
    return vec4<f32>(select(out, to_linear(out), u.srgb > 0.5), src.a);
}
