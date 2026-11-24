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
    // paper_scale (pt a texel), stroke_rows, then the paper's and the
    // stroke strip's layer plus one (#390, #547; 0 = procedural)
    h: vec4<f32>,
    // grazing (#404), tone (#869: 0 picture, 1 light), transition (#879:
    // 0 fade, 1 draw), the seed without the boil's steps (the draw order's)
    i: vec4<f32>,
    // clutter (#883), unused x3
    j: vec4<f32>,
    // World to the styled root's own space (#544, `grid = "local"`): the
    // inverse world matrix's top three rows, local.i = dot(row, (p, 1));
    // the identity on any other row
    k: vec4<f32>,
    l: vec4<f32>,
    m: vec4<f32>,
};

struct PencilUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    pixels_per_point: f32,
    size: vec2<f32>,
    srgb: f32,
    // The whole frame's draw front (#879): its table's blend while the
    // table draws on, else 1
    front: f32,
    // The camera's forward axis, for view depth
    forward: vec3<f32>,
    _pad1: f32,
    // x: rows in use; y: 1 when slot 0 passes through (only entities
    // asked); z: 1 when the mask is bound
    info: vec4<u32>,
    // The shared edges' thresholds (#743): edge_depth, edge_normal, unused x2
    edge: vec4<f32>,
    // Each layer of the paper and stroke stacks (#547): measures and size
    papers: array<StyleImageLayer, 4>,
    strokes: array<StyleImageLayer, 4>,
};

@group(0) @binding(0) var<uniform> u: PencilUniforms;
// The variant table (#740, the framework's): a row a variant, and the row
// of each mask id, as long as the frame needs
@group(0) @binding(1) var<storage, read> pencil_rows: array<PencilRow>;
@group(0) @binding(2) var<storage, read> flint_slots: array<u32>;

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

// The picture's colour as the eye sees it (sRGB targets sample linear)
fn shown(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(x, flint_to_display(x), u.srgb > 0.5);
}

// --- hashes: stylize/noise.wgsl (flint_hash1, flint_hash3, flint_vnoise, #1256) ---

// --- settings by pixel -------------------------------------------------------

@group(2) @binding(0) var scene_depth: texture_depth_2d;
@group(2) @binding(1) var flint_mask_texture: texture_2d<u32>;
// The paper and stroke stacks (#390, #547), a blank texel while none is named
@group(2) @binding(2) var paper_tex: texture_2d_array<f32>;
@group(2) @binding(3) var stroke_tex: texture_2d_array<f32>;
// The style albedo (#869): each opaque mesh's own colour, alpha 0 where none
// was drawn; a blank texel while no row reads the light
@group(2) @binding(4) var albedo_tex: texture_2d<f32>;
// The shared geometry edges (#743, `edges.wgsl`), at the depth's size; a
// blank texel if the edge pass couldn't be built
@group(2) @binding(5) var edge_tex: texture_2d<f32>;

// The mask is read through mask_read.wgsl, the slots through mask_slot.wgsl
// (#1256); a row's slot only while the frame has per-entity rows
fn slot_of(id: u32) -> u32 {
    if u.info.z == 0u {
        return 0u;
    }
    return flint_slot(id);
}

// Whether the pixel in this row is left as it is: an opt-out variant, or
// slot 0 when the scene's table doesn't cover the whole frame
// A mask id another style owns (an ASCII prop in a pencil frame) gets this
// slot: its pixels are that style's, so they pass through
const FOREIGN: u32 = 0xffffffffu;

fn passes_through(slot: u32) -> bool {
    if slot == FOREIGN {
        return true;
    }
    if slot == 0u {
        return u.info.y == 1u;
    }
    return (u32(pencil_rows[slot].g.z + 0.5) & 1u) != 0u;
}

// Whether the row's hatching is laid on the world's surfaces (#389)
fn world_grid(row: PencilRow) -> bool {
    return (u32(row.g.z + 0.5) & 2u) != 0u;
}

// Whether a world grid's row lays its strokes in its root's own space (#544)
fn local_grid(row: PencilRow) -> bool {
    return (u32(row.g.z + 0.5) & 4u) != 0u;
}

// A world point in the row's local space
fn to_local(row: PencilRow, p: vec3<f32>) -> vec3<f32> {
    let h = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(row.k, h), dot(row.l, h), dot(row.m, h));
}

// A world direction (a derivative of a position) in the row's local space
fn dir_to_local(row: PencilRow, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(row.k.xyz, d), dot(row.l.xyz, d), dot(row.m.xyz, d));
}

// A world normal in the row's local space: through the inverse transpose of
// the rows' 3x3 A, which is A's cofactor matrix (rows r1 x r2, r2 x r0,
// r0 x r1) over its determinant. The determinant's scale and sign go with
// the normalize and the triplanar weights' abs
fn normal_to_local(row: PencilRow, n: vec3<f32>) -> vec3<f32> {
    let r0 = row.k.xyz;
    let r1 = row.l.xyz;
    let r2 = row.m.xyz;
    let c = vec3<f32>(dot(cross(r1, r2), n), dot(cross(r2, r0), n), dot(cross(r0, r1), n));
    return c / max(length(c), 1e-12);
}

// --- depth --------------------------------------------------------------------

fn depth_size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(scene_depth));
}

fn world_at(q: vec2<i32>) -> vec3<f32> {
    let size = depth_size();
    let p = clamp(q, vec2<i32>(0), size - 1);
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
    return flint_unproject(u.inv_view_proj, uv, textureLoad(scene_depth, p, 0));
}

// The world position under an exact uv (not a texel centre), from the
// depth texel it falls in: w = 0 where nothing was drawn (the sky)
fn world_uv(uv: vec2<f32>) -> vec4<f32> {
    let size = depth_size();
    let q = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let d = textureLoad(scene_depth, q, 0);
    return vec4<f32>(flint_unproject(u.inv_view_proj, uv, d), select(1.0, 0.0, d >= 1.0));
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
