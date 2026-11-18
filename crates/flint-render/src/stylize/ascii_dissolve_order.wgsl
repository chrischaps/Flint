// ASCII dissolve order pass (#398): ascii_common.wgsl and
// ascii_dissolve.wgsl come first.

// Binding 0 is the picture, unread here
@group(1) @binding(1) var order_mask: texture_2d<u32>;

fn order_slot(p: vec2<f32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(order_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    return slot_of(textureLoad(order_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r);
}

// Pixels a side a cell's scan reads at most (#554): a cell is at most 32
// points, so this covers 5 pixels a point
const MAX_SCAN: i32 = 160;

// Whether any pixel of the cell from `cell_min`, `size` across, is slot
// `slot`'s (#554). Read only where none of the 16 lattice samples was: an
// outline sliver or a thin edge between them. Each pixel is read as the
// glyph pass reads it, by its centre, and only inside the variant's bounds
// (#507), past which the mask never holds its id.
fn holds_a_pixel(slot: u32, cell_min: vec2<f32>, size: vec2<f32>, bounds: vec4<f32>) -> bool {
    // The pixels whose centres fall in the cell
    let first = max(ceil(cell_min - 0.5), max(floor(bounds.xy), vec2<f32>(0.0)));
    let past = min(ceil(cell_min + size - 0.5), min(ceil(bounds.zw), u.size));
    let lo = vec2<i32>(first);
    let hi = min(vec2<i32>(past), lo + vec2<i32>(MAX_SCAN));
    for (var y = lo.y; y < hi.y; y++) {
        for (var x = lo.x; x < hi.x; x++) {
            if order_slot(vec2<f32>(f32(x), f32(y)) + 0.5) == slot {
                return true;
            }
        }
    }
    return false;
}

// The order of each cell of the band's variant, NOT_MEMBER where the cell
// isn't the variant's: no pixel of it is (#554; the 16 lattice samples
// first, as the glyph pass lays them, then every pixel where they all
// missed), or, for a variant whose silhouette snaps (#338), fewer than its
// `cell_share` of the samples, since those cells are drawn whole or not.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(in.position.xy));
    let band = dissolve_band(texel);
    if band < 0 {
        return vec4<f32>(NOT_MEMBER);
    }
    let d = u.dissolve[band];
    let v = variants[d.x];
    let cell = vec2<f32>(vec2<i32>(texel.x, texel.y - i32(d.y)) - vec2<i32>(CELL_OFFSET));
    let cell_min = cell * v.cell + v.origin;
    // A cell clear of the variant's bounds (a pixel's slack each side) holds
    // no pixel of it: most of the band, spared the mask reads
    if any(cell_min + v.cell < v.bounds.xy - 1.0) || any(cell_min > v.bounds.zw + 1.0) {
        return vec4<f32>(NOT_MEMBER);
    }
    var count = 0u;
    for (var k = 0; k < 16; k++) {
        let q = floor(cell_min + (vec2<f32>(f32(k & 3), f32(k >> 2u)) + 0.5) / 4.0 * v.cell);
        // A sample off the frame would read the clamped edge texel: a cell
        // past the edge, never drawn, would count as the prop's
        let inside = all(q >= vec2<f32>(0.0)) && all(q < u.size);
        count += select(0u, 1u, inside && order_slot(q) == d.x);
    }
    var member = count > 0u;
    if (v.flags & 4u) != 0u {
        member = f32(count) + 1e-3 >= v.cell_share * 16.0;
    } else if !member {
        member = holds_a_pixel(d.x, cell_min, v.cell, v.bounds);
    }
    if !member {
        return vec4<f32>(NOT_MEMBER);
    }
    return vec4<f32>(dissolve_order(cell), 0.0, 0.0, 0.0);
}
