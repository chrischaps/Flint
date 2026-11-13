// ASCII edge view (#349): what the edge passes found and what each glyph
// cell made of it. `flint render --debug-mode edges` (and F4's "Style
// Edges") draw this in place of the glyphs, over the scene in dim grey:
//   - each edge pixel (ascii_edges.wgsl) in its source's colour: cyan a
//     depth step, amber a crease, magenta a styled entity's outline;
//   - each glyph cell's decision (ascii_edge_cells.wgsl, then the
//     thinning and corners of ascii_edge_thin.wgsl and
//     ascii_edge_corners.wgsl, #910, the texel the glyph pass reads): a
//     green line in the
//     glyph's direction where it draws one, a dim dashed one where it
//     yields the line to a neighbour, a red wash where the cell holds more
//     than `edge_max` (texture, keeps its tone glyph), nothing where it
//     holds less than `edge_threshold`; a yellow corner or tee, arms from
//     the cell's centre, where the cell draws one (#348);
//   - along each cell's foot, a bar of its edge length over `edge_max`,
//     with a white tick at `edge_threshold`; in cells 12 px or wider the
//     length in cell widths is written in the top left.
// The colours are ascii.rs's EDGE_VIEW_LEGEND; keep them together.

struct EdgeViewUniforms {
    cell: vec2<f32>,
    origin: vec2<f32>,
    size: vec2<f32>,
    // The edge texture's size: the depth's
    edge_size: vec2<f32>,
    threshold: f32,
    max_len: f32,
    srgb: f32,
    dim: f32,
    // Line width in pixels (a point)
    thick: f32,
    // How many corner glyphs the frame's font has (#348)
    corners: u32,
    _pad1: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: EdgeViewUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var edges: texture_2d<f32>;
@group(3) @binding(0) var edge_cells: texture_2d<f32>;
// The edge slot pass's packed decision per cell (#910)
@group(3) @binding(1) var edge_slots: texture_2d<u32>;

// The edge cells are offset by one, so the part cell at the top left is
// texel 0, 0 (ascii_common.wgsl's CELL_OFFSET)
const VIEW_CELL_OFFSET: i32 = 1;

const DEPTH_COLOR: vec3<f32> = vec3<f32>(0.25, 0.85, 1.0);
const CREASE_COLOR: vec3<f32> = vec3<f32>(1.0, 0.68, 0.2);
const OUTLINE_COLOR: vec3<f32> = vec3<f32>(1.0, 0.35, 0.85);
const LINE_COLOR: vec3<f32> = vec3<f32>(0.4, 1.0, 0.45);
const YIELD_COLOR: vec3<f32> = vec3<f32>(0.25, 0.55, 0.3);
const SHORT_COLOR: vec3<f32> = vec3<f32>(0.55, 0.55, 0.55);
const TEXTURE_COLOR: vec3<f32> = vec3<f32>(1.0, 0.25, 0.2);
const CORNER_COLOR: vec3<f32> = vec3<f32>(1.0, 0.9, 0.3);

// A corner glyph slot's arms (#348), right 1, left 2, down 4, up 8: the
// inverse of ascii_edge_corners.wgsl's ARM_GLYPHS, slots 4..12
const SLOT_ARMS = array<u32, 9>(5u, 6u, 9u, 10u, 13u, 14u, 7u, 11u, 15u);

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

// 3x5 digits, bit (row * 3 + column) set where the digit inks
fn digit_on(d: u32, x: i32, y: i32) -> bool {
    var glyphs = array<u32, 10>(31599u, 29850u, 29671u, 31207u, 18925u, 31183u, 31695u, 9383u, 31727u, 31215u);
    if x < 0 || x > 2 || y < 0 || y > 4 {
        return false;
    }
    return ((glyphs[min(d, 9u)] >> u32(y * 3 + x)) & 1u) == 1u;
}

// Whether `len` (0.0 .. 9.9), written `d.d` from `at` in font pixels of
// `scale` screen pixels, inks the pixel `p`
fn number_on(len: f32, p: vec2<f32>, at: vec2<f32>, scale: f32) -> bool {
    let tenths = u32(round(clamp(len, 0.0, 9.9) * 10.0));
    let q = vec2<i32>(floor((p - at) / scale));
    // `d` `.` `d`: columns 0-2, 4, 6-8
    if q.x >= 0 && q.x <= 2 {
        return digit_on(tenths / 10u, q.x, q.y);
    }
    if q.x == 4 {
        return q.y == 4;
    }
    return digit_on(tenths % 10u, q.x - 6, q.y);
}

// The direction a line glyph runs across a cell (screen y down), corner to
// corner for the diagonals
fn glyph_direction(slot: u32) -> vec2<f32> {
    if slot == 0u {
        return vec2<f32>(1.0, 0.0);
    }
    if slot == 1u {
        return vec2<f32>(0.0, 1.0);
    }
    if slot == 2u {
        return normalize(vec2<f32>(u.cell.x, -u.cell.y));
    }
    return normalize(vec2<f32>(u.cell.x, u.cell.y));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(input_texture, input_sampler, in.uv);
    let rgb = clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let shown = select(rgb, to_display(rgb), u.srgb > 0.5);
    let luma = dot(shown, vec3<f32>(0.2126, 0.7152, 0.0722));
    var out = vec3<f32>(luma * u.dim);

    let p = in.position.xy;
    let index = floor((p - u.origin) / u.cell);
    let local = p - u.origin - index * u.cell;
    let c = vec2<i32>(index) + vec2<i32>(VIEW_CELL_OFFSET);
    // A faint grid, so the cells read
    if local.x < 1.0 || local.y < 1.0 {
        out += vec3<f32>(0.05);
    }

    let e = edge_cell_at(c);
    let len = e.y;
    let has_line = e.x > 0.5;
    let cmax = vec2<i32>(textureDimensions(edge_slots)) - vec2<i32>(1);
    let slots = textureLoad(edge_slots, clamp(c, vec2<i32>(0), cmax), 0).x;
    let drawn = has_line && slot_for(slots, 0u) != NO_EDGE;
    let shape = slot_for(slots, u.corners);
    // A corner in a gap (#637) draws no line of its own, so it isn't `drawn`
    let corner = shape >= 4u && shape != NO_EDGE;
    var decision = SHORT_COLOR;
    if corner {
        decision = CORNER_COLOR;
    } else if drawn {
        decision = LINE_COLOR;
    } else if has_line {
        decision = YIELD_COLOR;
    } else if len > u.max_len && len >= u.threshold {
        decision = TEXTURE_COLOR;
        out = mix(out, TEXTURE_COLOR, 0.28);
    }

    // The edge pixel under this one, by its source
    let q = min(
        vec2<i32>(p * u.edge_size / u.size),
        vec2<i32>(u.edge_size) - vec2<i32>(1),
    );
    let kind = textureLoad(edges, q, 0).w;
    if kind > 2.5 {
        out = OUTLINE_COLOR;
    } else if kind > 1.5 {
        out = DEPTH_COLOR;
    } else if kind > 0.5 {
        out = CREASE_COLOR;
    }

    // The corner the cell draws (#348): its arms, from the centre out
    if corner {
        var table = SLOT_ARMS;
        let arms = table[shape - 4u];
        let d = local - 0.5 * u.cell;
        let half = 0.5 * u.thick;
        let on_row = abs(d.y) <= half;
        let on_col = abs(d.x) <= half;
        if (on_row && ((d.x >= -half && (arms & 1u) != 0u) || (d.x <= half && (arms & 2u) != 0u)))
            || (on_col && ((d.y >= -half && (arms & 4u) != 0u) || (d.y <= half && (arms & 8u) != 0u))) {
            out = mix(out, decision, 0.85);
        }
    } else if has_line {
    // The line glyph the cell draws (or yields), through its centre
        let slot = min(u32(e.x - 0.5), 3u);
        let d = local - 0.5 * u.cell;
        let dir = glyph_direction(slot);
        let off = abs(d.x * dir.y - d.y * dir.x);
        let along = dot(d, dir);
        let dashed = !drawn && (u32(floor(along / max(u.thick * 2.0, 2.0) + 64.0)) & 1u) == 1u;
        if off <= 0.5 * u.thick && !dashed {
            out = mix(out, decision, 0.85);
        }
    }

    // The length bar along the cell's foot, and the threshold's tick
    if len > 0.0 {
        let span = max(u.max_len, u.threshold);
        if local.y >= u.cell.y - u.thick {
            let tick = u.cell.x * clamp(u.threshold / span, 0.0, 1.0);
            if abs(local.x - tick) < 0.5 * max(u.thick, 1.0) {
                out = vec3<f32>(1.0);
            } else if local.x <= u.cell.x * clamp(len / span, 0.0, 1.0) {
                out = decision;
            }
        }
        // The length, in cells wide enough to write it
        if u.cell.x >= 12.0 && u.cell.y >= 9.0 {
            let scale = max(1.0, floor((u.cell.x - 2.0) / 12.0));
            if number_on(len, local, vec2<f32>(1.0 + scale), scale) {
                out = mix(decision, vec3<f32>(1.0), 0.35);
            }
        }
    }

    let encoded = select(out, to_linear(out), u.srgb > 0.5);
    return vec4<f32>(encoded, src.a);
}
