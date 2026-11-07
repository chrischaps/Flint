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
// Output: x = the edge glyph's slot + 1 (0 = no edge; slots in
// `EdgeGlyphs::all()` order: horizontal, vertical, diagonal,
// antidiagonal), y = the edge length in cell widths that decided it (the
// outline's, for an outline), z = 1 for an outline.

struct CellUniforms {
    cell: vec2<f32>,
    origin: vec2<f32>,
    size: vec2<f32>,
    threshold: f32,
    max_len: f32,
    // The edge texture's size: the depth's, smaller than `size` under a
    // render scale
    edge_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: CellUniforms;
@group(1) @binding(0) var edges: texture_2d<f32>;

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
    var out_j = vec3<f32>(0.0);
    var out_n = 0.0;
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
                all_j += e.xyz;
                all_n += weight;
                if e.w > 1.5 {
                    out_j += e.xyz;
                    out_n += weight;
                }
            }
        }
    }
    let width = max(u.cell.x, 1.0);
    let all_len = all_n * stride / width;
    let out_len = out_n * stride / width;

    var j = vec3<f32>(0.0);
    var outline = 0.0;
    if out_len >= u.threshold {
        j = out_j;
        outline = 1.0;
    } else if all_len >= u.threshold && all_len <= u.max_len {
        j = all_j;
    } else {
        return vec4<f32>(0.0, all_len, 0.0, 0.0);
    }
    // The gradient's dominant angle (screen space, y down); the line runs
    // across it. In y-up terms the gradient's angle is the negation.
    let grad = 0.5 * atan2(2.0 * j.y, j.x - j.z);
    let slot = slot_for(PI * 0.5 - grad);
    // y = the length that decided it, which the glyph pass compares with
    // its neighbours' to thin a line that splits between two cells
    return vec4<f32>(f32(slot) + 1.0, select(all_len, out_len, outline > 0.5), outline, 0.0);
}
