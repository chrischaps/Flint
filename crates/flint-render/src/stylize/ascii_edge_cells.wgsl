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
// outline's, for an outline), z = 1 for an outline, w = the near side's
// hue (#351): the mean display colour of the picture under the edge pixels
// that decided it, at full value, as an exact 24-bit integer
// (r | g << 8 | b << 16). Edge pixels lie on the near side of a step, so
// this is the object's own colour, not the cell's mix of it and what's
// behind. x..z are rounded to f16, as when the target was Rgba16Float.

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
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: CellUniforms;
@group(1) @binding(0) var edges: texture_2d<f32>;
// The style's input picture, at output resolution
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

// `x` rounded to f16, as the old Rgba16Float target stored it, so the
// glyph pass's length comparisons between neighbours tie as they did
fn as_f16(x: f32) -> f32 {
    return unpack2x16float(pack2x16float(vec2<f32>(x, 0.0))).x;
}

// A colour sum's hue at full value, packed 8 bits a channel
fn packed_hue(sum: vec3<f32>) -> f32 {
    let hue = sum / max(max(sum.r, max(sum.g, sum.b)), 1e-6);
    let b = vec3<u32>(round(clamp(hue, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
    return f32(b.r | (b.g << 8u) | (b.b << 16u));
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
                let c = shown(p);
                all_j += e.xyz;
                all_n += weight;
                all_c += c;
                if e.w > 1.5 {
                    out_j += e.xyz;
                    out_n += weight;
                    out_c += c;
                }
            }
        }
    }
    let width = max(u.cell.x, 1.0);
    let all_len = all_n * stride / width;
    let out_len = out_n * stride / width;

    var j = vec3<f32>(0.0);
    var near = vec3<f32>(0.0);
    var outline = 0.0;
    if out_len >= u.threshold {
        j = out_j;
        near = out_c;
        outline = 1.0;
    } else if all_len >= u.threshold && all_len <= u.max_len {
        j = all_j;
        near = all_c;
    } else {
        return vec4<f32>(0.0, as_f16(all_len), 0.0, 0.0);
    }
    // The gradient's dominant angle (screen space, y down); the line runs
    // across it. In y-up terms the gradient's angle is the negation.
    let grad = 0.5 * atan2(2.0 * j.y, j.x - j.z);
    let slot = slot_for(PI * 0.5 - grad);
    // y = the length that decided it, which the glyph pass compares with
    // its neighbours' to thin a line that splits between two cells
    let len = as_f16(select(all_len, out_len, outline > 0.5));
    return vec4<f32>(f32(slot) + 1.0, len, outline, packed_hue(near));
}
