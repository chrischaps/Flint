// Pencil sketch (#273): what both passes share. The uniform (one row of
// settings per variant, the whole frame's in row 0, as ASCII's #266
// table), the slot lookup through the entity style mask, the scene's
// depth as world positions, and the integer hashes every random choice is
// made from, so a fixed seed draws the same strokes on every run.

struct PencilRow {
    // spacing (pt), thickness (pt), layers, angle (rad)
    a: vec4<f32>,
    // brightness, contrast, contour, contour_width (pt)
    b: vec4<f32>,
    // depth, crease, detail, wobble (pt)
    c: vec4<f32>,
    // sketchy, grain, color, seed (boil folded in)
    d: vec4<f32>,
    // paper rgb (display values), direction (0 surface, 1 screen, 2 tensor)
    e: vec4<f32>,
    // lead rgb (display values), shade
    f: vec4<f32>,
    // origin xy (px), flags (bits: 1 pass through, 2 world grid), darkness
    g: vec4<f32>,
    // paper_scale (pt a texel), stroke_rows, paper image on, stroke image on (#390)
    h: vec4<f32>,
    // grazing (#404), unused x3
    i: vec4<f32>,
};

struct PencilUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    pixels_per_point: f32,
    size: vec2<f32>,
    srgb: f32,
    _pad0: f32,
    // The camera's forward axis, for view depth
    forward: vec3<f32>,
    _pad1: f32,
    // x: rows in use; y: 1 when slot 0 passes through (only entities
    // asked); z: 1 when the mask is bound
    info: vec4<u32>,
    // The images' measures (#390): paper mean and spread of luma, stroke ink
    img: vec4<f32>,
    rows: array<PencilRow, 64>,
    // The row of each mask id, four ids a vec4
    slots: array<vec4<u32>, 64>,
};

@group(0) @binding(0) var<uniform> u: PencilUniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// The picture's colour as the eye sees it (sRGB targets sample linear)
fn shown(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(x, to_display(x), u.srgb > 0.5);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// --- hashes ----------------------------------------------------------------

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn hash3(a: i32, b: i32, c: u32) -> vec3<f32> {
    let h = pcg(bitcast<u32>(a) ^ pcg(bitcast<u32>(b) ^ pcg(c)));
    let h2 = pcg(h);
    return vec3<f32>(unit(h), unit(h2), unit(pcg(h2)));
}

fn hash1(a: i32, b: i32, c: u32) -> f32 {
    return unit(pcg(bitcast<u32>(a) ^ pcg(bitcast<u32>(b) ^ pcg(c))));
}

// Value noise in [0, 1], smooth between integer lattice points
fn vnoise(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let s = f * f * (3.0 - 2.0 * f);
    let x = i32(i.x);
    let y = i32(i.y);
    let a = hash1(x, y, seed);
    let b = hash1(x + 1, y, seed);
    let c = hash1(x, y + 1, seed);
    let d = hash1(x + 1, y + 1, seed);
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

// --- settings by pixel -------------------------------------------------------

@group(2) @binding(0) var scene_depth: texture_depth_2d;
@group(2) @binding(1) var style_mask: texture_2d<u32>;
// The paper and stroke images (#390), a blank texel while none is named
@group(2) @binding(2) var paper_tex: texture_2d<f32>;
@group(2) @binding(3) var stroke_tex: texture_2d<f32>;

fn mask_id(p: vec2<i32>) -> u32 {
    let size = vec2<i32>(textureDimensions(style_mask));
    return textureLoad(style_mask, clamp(p, vec2<i32>(0), size - 1), 0).r;
}

fn mask_px(uv: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(uv * vec2<f32>(textureDimensions(style_mask)));
}

fn slot_of(id: u32) -> u32 {
    if u.info.z == 0u || id >= 256u {
        return 0u;
    }
    return u.slots[id / 4u][id % 4u];
}

// Whether the pixel in this row is left as it is: an opt-out variant, or
// slot 0 when the scene's table doesn't cover the whole frame
// A mask id another style owns (an ASCII prop in a pencil frame) gets this
// slot: its pixels are that style's, so they pass through
const FOREIGN: u32 = 255u;

fn passes_through(slot: u32) -> bool {
    if slot == FOREIGN {
        return true;
    }
    if slot == 0u {
        return u.info.y == 1u;
    }
    return (u32(u.rows[slot].g.z + 0.5) & 1u) != 0u;
}

// Whether the row's hatching is laid on the world's surfaces (#389)
fn world_grid(row: PencilRow) -> bool {
    return (u32(row.g.z + 0.5) & 2u) != 0u;
}

// --- depth --------------------------------------------------------------------

fn depth_size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(scene_depth));
}

fn world_at(q: vec2<i32>) -> vec3<f32> {
    let size = depth_size();
    let p = clamp(q, vec2<i32>(0), size - 1);
    let d = textureLoad(scene_depth, p, 0);
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = u.inv_view_proj * ndc;
    return w.xyz / w.w;
}

// The world position under an exact uv (not a texel centre), from the
// depth texel it falls in: w = 0 where nothing was drawn (the sky)
fn world_uv(uv: vec2<f32>) -> vec4<f32> {
    let size = depth_size();
    let q = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let d = textureLoad(scene_depth, q, 0);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = u.inv_view_proj * ndc;
    return vec4<f32>(w.xyz / w.w, select(1.0, 0.0, d >= 1.0));
}

// A unit normal folded into two numbers (octahedral), and back
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let m = n / (abs(n.x) + abs(n.y) + abs(n.z));
    if m.z >= 0.0 {
        return m.xy;
    }
    let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), m.xy >= vec2<f32>(0.0));
    return (1.0 - abs(m.yx)) * s;
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
    if n.z < 0.0 {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}
