// ASCII edges (#267), pass 2 of 3: which glyph cells draw a line glyph.
//
// One fragment per cell of the ASCII grid (the target is the grid, one
// texel a cell, offset by one so the part cell at the top left is 0, 0).
// It reads every edge pixel of its cell, not a sparse lattice, so a
// one-pixel outline can't slip between samples and leave a gap.
//
// The edge pixels' structure tensors are summed: its dominant orientation
// is the line's direction, smoothed over the whole cell, with no voting
// bins (Kuwahara's tensor, but over geometry edges, not colour). How much
// edge the cell holds is measured in cell widths, so a line straight
// across counts 1 at any cell size:
//   - at least `threshold` of edge and the cell draws a line glyph;
//   - more than `max_len` and it's texture-like geometry (grass, foliage),
//     not an outline, and keeps its tone glyph;
//   - a styled entity's outline needs only `threshold` and ignores
//     `max_len`, so it is always drawn.
// Output: x = the edge glyph's slot + 1 (0 or below = no edge; slots in
// `EdgeGlyphs::all()` order: horizontal, vertical, diagonal,
// antidiagonal) plus the line's direction as a fraction of a half turn
// halved, [0, 0.5) (a cell with edge pixels but no line glyph writes -0.25
// minus it), y = the edge length in cell widths that decided it (the
// outline's, for an outline), z = 1 for an outline, plus 2 when the line is
// nearer vertical (its cross axis is x, not y), w = where the line crosses
// the cell's centre on that cross axis, in cells from the centre (#352):
// the edge pixels' centroid is on the line, and running it along the line
// to the centre row or column finds the line's offset there. A line
// belongs to the cell it passes within half a cell of; the glyph pass
// (ascii_edge_thin.wgsl) keeps it only there. z and w are written for a
// cell holding any edge pixels, so a sliver of a line its neighbour holds
// most of still says where it is.
// z also carries the near side's hue (#351) above those two bits:
// z = outline + 2 * cross_x + 4 * hue, the hue being the mean display
// colour of the picture under the edge pixels that decided the cell, at
// full value, 7 bits a channel (r | g << 7 | b << 14). Edge pixels lie on
// the near side of a step, so it is the object's own colour, not the
// cell's mix of it and what's behind. z stays a whole number under 2^23,
// exact in the Rgba32Float target. x, y and w are rounded to f16, as the
// Rgba16Float target stored them before.
// Corners (#348): z's bit 23 (CORNER_BIT, 2^23; z stays under 2^24, still
// exact) marks a cell that draws a line glyph but whose edge pixels run in
// no one direction: the coherence of its summed tensor, (l1 - l2) /
// (l1 + l2), is under `corner`. Two arms of a corner at right angles give
// (a - b) / (a + b) for arm lengths a and b, 0 when the corner sits in the
// cell's middle; a straight line gives nearly 1. The glyph pass picks which
// corner from the neighbours' line glyphs (ascii_edge_corners.wgsl).

struct CellUniforms {
    cell: vec2<f32>,
    origin: vec2<f32>,
    size: vec2<f32>,
    threshold: f32,
    max_len: f32,
    // The edge texture's size: the depth's, smaller than `size` under a
    // render scale
    edge_size: vec2<f32>,
    // 1: the picture arrives linear (an sRGB target)
    srgb: u32,
    // Coherence under which a drawn cell is marked a corner (#348; 0 off)
    corner: f32,
};

@group(0) @binding(0) var<uniform> u: CellUniforms;
@group(1) @binding(0) var edges: texture_2d<f32>;
// The style's input picture, at output resolution (#351)
@group(2) @binding(0) var picture: texture_2d<f32>;

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// The picture at output pixel `p` in display values, 0..1
fn shown(p: vec2<i32>) -> vec3<f32> {
    let c = clamp(textureLoad(picture, p, 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

// A colour sum's hue at full value, packed 7 bits a channel
fn packed_hue(sum: vec3<f32>) -> f32 {
    let hue = sum / max(max(sum.r, max(sum.g, sum.b)), 1e-6);
    let b = vec3<u32>(round(clamp(hue, vec3<f32>(0.0), vec3<f32>(1.0)) * 127.0));
    return f32(b.r | (b.g << 7u) | (b.b << 14u));
}

// `x` rounded to f16, as the old Rgba16Float target stored it, so the
// glyph pass's comparisons between neighbours behave as they did
fn as_f16(x: f32) -> f32 {
    return unpack2x16float(pack2x16float(vec2<f32>(x, 0.0))).x;
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

const PI: f32 = 3.14159265;
// z's corner mark (#348), 2^23: above the hue's 21 bits and the two below
const CORNER_BIT: f32 = 8388608.0;

// The edge glyph slot for a line whose direction is `angle` radians from
// +x with y up, as `EdgeGlyphs::for_angle` picks it.
fn slot_for(angle: f32) -> u32 {
    let a = angle - floor(angle / PI) * PI;
    let eighth = PI / 8.0;
    if a < eighth || a >= 7.0 * eighth {
        return 0u;
    }
    if a < 3.0 * eighth {
        return 2u;
    }
    if a < 5.0 * eighth {
        return 1u;
    }
    return 3u;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let index = floor(in.position.xy) - 1.0;
    let cell_min = index * u.cell + u.origin;
    // Big cells are sampled every `stride` pixels; each sample stands for
    // that many pixels of a line crossing it
    let stride = max(1.0, ceil(max(u.cell.x, u.cell.y) / 16.0));
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let edge_max_px = vec2<i32>(u.edge_size) - vec2<i32>(1);
    let to_edge = u.edge_size / u.size;
    let weight = min(min(to_edge.x, to_edge.y), 1.0);

    var all_j = vec3<f32>(0.0);
    var all_n = 0.0;
    // Position moments of each tensor component: the centroid is weighted
    // by how much each pixel's gradient agrees with the cell's line, which
    // is linear in its tensor, so it can be summed before the line is known
    var all_cx = vec2<f32>(0.0);
    var all_cy = vec2<f32>(0.0);
    var all_cz = vec2<f32>(0.0);
    var out_j = vec3<f32>(0.0);
    var out_n = 0.0;
    var out_cx = vec2<f32>(0.0);
    var out_cy = vec2<f32>(0.0);
    var out_cz = vec2<f32>(0.0);
    var all_c = vec3<f32>(0.0);
    var out_c = vec3<f32>(0.0);
    for (var y = 0.5 * stride; y < u.cell.y; y += stride) {
        for (var x = 0.5 * stride; x < u.cell.x; x += stride) {
            let p = vec2<i32>(floor(cell_min + vec2<f32>(x, y)));
            if any(p < vec2<i32>(0)) || any(p > max_px) {
                continue;
            }
            // The edge pixel under this output pixel. Under a render scale
            // a one-pixel line is 1/scale output pixels wide, so each hit
            // weighs `scale` and lengths stay in output cell widths
            let q = min(vec2<i32>((vec2<f32>(p) + 0.5) * to_edge), edge_max_px);
            let e = textureLoad(edges, q, 0);
            if e.w > 0.5 {
                // The sample's place in the cell, in cells from its centre
                let at = vec2<f32>(x, y) / u.cell - 0.5;
                all_j += e.xyz;
                all_n += weight;
                let c = shown(p);
                all_c += c;
                all_cx += e.x * at;
                all_cy += e.y * at;
                all_cz += e.z * at;
                // w: 1 crease, 2 depth, 3 outline (ascii_edges.wgsl)
                if e.w > 2.5 {
                    out_j += e.xyz;
                    out_n += weight;
                    out_c += c;
                    out_cx += e.x * at;
                    out_cy += e.y * at;
                    out_cz += e.z * at;
                }
            }
        }
    }
    let width = max(u.cell.x, 1.0);
    let all_len = all_n * stride / width;
    let out_len = out_n * stride / width;

    var j = all_j;
    var cx = all_cx;
    var cy = all_cy;
    var cz = all_cz;
    var near = all_c;
    var outline = 0.0;
    var slot_plus = 0.0;
    var len = all_len;
    if out_len >= u.threshold {
        j = out_j;
        cx = out_cx;
        cy = out_cy;
        cz = out_cz;
        near = out_c;
        outline = 1.0;
        len = out_len;
    } else if all_len < u.threshold || all_len > u.max_len {
        // No line glyph, but where its pixels are, for a neighbour that
        // holds the rest of the line (ascii_edge_thin.wgsl)
        slot_plus = -1.0;
    }
    if all_n <= 0.0 {
        return vec4<f32>(0.0);
    }
    // The gradient's dominant angle (screen space, y down); the line runs
    // across it. In y-up terms the gradient's angle is the negation.
    let grad = 0.5 * atan2(2.0 * j.y, j.x - j.z);
    let dir = PI * 0.5 - grad;
    let slot = slot_for(dir);
    // The direction itself, in [0, 0.5) for the fraction of x, so the
    // glyph pass can tell two lines that converge from one band
    let turn = 0.5 * fract(dir / PI);
    // The line's normal in cell units (a normal scales by the cell's size,
    // so it stays across the line in a cell that isn't square)
    // The line's pixels' centroid, each weighted by (gradient . normal)^2:
    // at a corner, the pixels of the other arm (or of a crease crossing)
    // weigh nothing, so the centroid stays on this cell's line
    let g = vec2<f32>(cos(grad), sin(grad));
    let along = vec3<f32>(g.x * g.x, 2.0 * g.x * g.y, g.y * g.y);
    let centroid = (along.x * cx + along.y * cy + along.z * cz) / max(dot(along, j), 1e-6);
    let n = g * u.cell;
    let cross_x = abs(n.x) > abs(n.y);
    // Where the line meets the centre column (cross axis x) or row (y)
    let across = select(n.y, n.x, cross_x);
    let offset = dot(centroid, n) / select(across, 1e-6, abs(across) < 1e-6);
    // How sure the tensor is of one direction (#348): 1 for a straight
    // line, toward 0 where two arms at right angles share the cell
    let spread = sqrt((j.x - j.z) * (j.x - j.z) + 4.0 * j.y * j.y);
    let coherence = spread / max(j.x + j.z, 1e-6);
    let corner = select(0.0, CORNER_BIT, slot_plus >= 0.0 && coherence < u.corner);
    // y = the length that decided it
    return vec4<f32>(
        as_f16(select(f32(slot) + 1.0 + turn, -0.25 - turn, slot_plus < 0.0)),
        as_f16(len),
        outline + select(0.0, 2.0, cross_x) + 4.0 * packed_hue(near) + corner,
        as_f16(offset),
    );
}
