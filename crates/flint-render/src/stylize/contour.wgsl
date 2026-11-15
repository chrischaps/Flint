// Contour pass (#214): ink lines over the scene, found in screen space.
//
// One full-screen draw, so its cost doesn't grow with the scene. Lines come from
// two places: silhouettes, as a break in depth, and the edges between painted
// fills, as a break in luminance. It reads whatever the Kuwahara pass made, so
// paint sits under the ink and never smears it.
//
// With boil above zero, each drawing (one value of `seed`) re-decides where the
// line runs, how wide it is, and how hard the pen pressed. The scene under the
// line stays where it is: only the ink moves. With boil at zero none of that
// runs, so the lines hold still.

struct ContourUniforms {
    texel_size: vec2<f32>,
    // Half the line's width in pixels, at this target's resolution.
    width: f32,
    // How strong an edge must be before it starts to ink.
    threshold: f32,
    color: vec3<f32>,
    strength: f32,
    near: f32,
    far: f32,
    // How much edges in luminance count, against edges in depth.
    luma: f32,
    // Boil this frame: the authored amount times the wooze field's jitter.
    boil: f32,
    // Which drawing this is. Changes on the field's beat, never per frame.
    seed: u32,
    // Metres at which a line has faded by half. 0 never fades.
    fade: f32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0)
var<uniform> params: ContourUniforms;

@group(1) @binding(0)
var scene_texture: texture_2d<f32>;
@group(1) @binding(1)
var scene_sampler: sampler;

@group(2) @binding(0)
var depth_texture: texture_2d<f32>;
@group(2) @binding(1)
var depth_sampler: sampler;

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

// Noise features per screen height, so a boil looks the same at any resolution.
const PUSH_CELLS: f32 = 14.0;
const WOBBLE_CELLS: f32 = 55.0;
const WIDTH_CELLS: f32 = 22.0;
const PRESSURE_CELLS: f32 = 9.0;

// A hash of a lattice point and a drawing. Integer, so it reads the same on every
// GPU and doesn't break down far from the origin the way sin() hashes do.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    var h = (u32(x) * 1597334677u) ^ (u32(y) * 3812015801u) ^ (seed * 2798796415u);
    h = (h ^ (h >> 16u)) * 2654435769u;
    h = (h ^ (h >> 15u)) * 2246822519u;
    h = h ^ (h >> 16u);
    return f32(h) / 4294967295.0;
}

// Smooth value noise in 0..1.
fn noise(p: vec2<f32>, seed: u32) -> f32 {
    let cell = floor(p);
    let f = p - cell;
    let u = f * f * (3.0 - 2.0 * f);
    let x = i32(cell.x);
    let y = i32(cell.y);
    let a = hash(x, y, seed);
    let b = hash(x + 1, y, seed);
    let c = hash(x, y + 1, seed);
    let d = hash(x + 1, y + 1, seed);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 1 / view distance. On any flat surface this is linear across the screen, so its
// second difference is zero however steep the view, and a floor seen at a grazing
// angle draws no lines.
fn inverse_depth(uv: vec2<f32>) -> f32 {
    let d = textureSampleLevel(depth_texture, depth_sampler, uv, 0.0).r;
    return (params.far - d * (params.far - params.near)) / (params.near * params.far);
}

// Luminance squeezed into 0..1, so a bright sky doesn't outweigh everything.
fn tone(uv: vec2<f32>) -> f32 {
    let c = textureSampleLevel(scene_texture, scene_sampler, uv, 0.0).rgb;
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return l / (1.0 + l);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;
    let scene = textureSampleLevel(scene_texture, scene_sampler, in.uv, 0.0);

    var uv = in.uv;
    var width = params.width;
    var pressure = 1.0;

    if (params.boil > 0.0) {
        // Noise cells are square whatever the window's shape.
        let q = in.uv * vec2<f32>(tx.y / tx.x, 1.0);
        let seed = params.seed;

        // Push: the line wanders off its edge, far over a long run and a little
        // over a short one, by more when the line is wide.
        let far_push = vec2<f32>(
            noise(q * PUSH_CELLS, seed),
            noise(q * PUSH_CELLS + 37.0, seed),
        ) - 0.5;
        let near_push = vec2<f32>(
            noise(q * WOBBLE_CELLS + 11.0, seed),
            noise(q * WOBBLE_CELLS + 53.0, seed),
        ) - 0.5;
        let push = (far_push * 2.0 + near_push * 0.8) * params.boil * (1.0 + width);
        uv = in.uv + push * tx;

        // Width: the pen swells and thins along the line.
        let swell = noise(q * WIDTH_CELLS + 91.0, seed) * 2.0 - 1.0;
        width = max(width * (1.0 + swell * params.boil * 0.85), 0.75);

        // Pressure: here and there the pen lifts.
        let lift = smoothstep(0.52, 0.78, noise(q * PRESSURE_CELLS + 7.0, seed));
        pressure = 1.0 - lift * min(params.boil, 1.0) * 0.8;
    }

    let d = vec2<f32>(width) * tx;

    // Depth: the second difference of 1 / distance, on both axes and both
    // diagonals, against the distance here.
    let c = inverse_depth(uv);
    let l = inverse_depth(uv - vec2<f32>(d.x, 0.0));
    let r = inverse_depth(uv + vec2<f32>(d.x, 0.0));
    let t = inverse_depth(uv - vec2<f32>(0.0, d.y));
    let b = inverse_depth(uv + vec2<f32>(0.0, d.y));
    let tl = inverse_depth(uv - d);
    let br = inverse_depth(uv + d);
    let tr = inverse_depth(uv + vec2<f32>(d.x, -d.y));
    let bl = inverse_depth(uv + vec2<f32>(-d.x, d.y));
    let across = max(abs(l + r - 2.0 * c), abs(t + b - 2.0 * c));
    let diagonal = max(abs(tl + br - 2.0 * c), abs(tr + bl - 2.0 * c));
    let depth_edge = max(across, diagonal) / max(c, 1e-6);

    // Luminance: a plain gradient over the same reach.
    var luma_edge = 0.0;
    if (params.luma > 0.0) {
        let gx = tone(uv + vec2<f32>(d.x, 0.0)) - tone(uv - vec2<f32>(d.x, 0.0));
        let gy = tone(uv + vec2<f32>(0.0, d.y)) - tone(uv - vec2<f32>(0.0, d.y));
        luma_edge = length(vec2<f32>(gx, gy)) * params.luma;
    }

    let edge = max(depth_edge, luma_edge);
    var ink = smoothstep(params.threshold, params.threshold * 2.0, edge) * pressure * params.strength;

    // Far away the same line covers a whole leaf, and a tree goes solid. A pen
    // draws less of what's further off, so lines thin out with the distance to the
    // nearest surface at this edge, which is the one the line belongs to.
    if (params.fade > 0.0) {
        let nearest = max(max(c, l), max(max(r, t), b));
        let distance = 1.0 / max(nearest, 1e-6);
        let x = distance / params.fade;
        ink *= 1.0 / (1.0 + x * x);
    }
    return vec4<f32>(mix(scene.rgb, params.color, clamp(ink, 0.0, 1.0)), scene.a);
}
