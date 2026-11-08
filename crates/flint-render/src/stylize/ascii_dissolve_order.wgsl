// ASCII dissolve order pass (#398): ascii_common.wgsl and
// ascii_dissolve.wgsl come first.

// Binding 0 is the picture, unread here
@group(1) @binding(1) var order_mask: texture_2d<u32>;

fn order_slot(p: vec2<f32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(order_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    return slot_of(textureLoad(order_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r);
}

// The order of each cell of the band's variant, NOT_MEMBER where the cell
// isn't the variant's: none of its 16 lattice samples (as the glyph pass
// lays them), or, for a variant whose silhouette snaps (#338), fewer than
// its `cell_share` of them.
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
    }
    if !member {
        return vec4<f32>(NOT_MEMBER);
    }
    return vec4<f32>(dissolve_order(cell), 0.0, 0.0, 0.0);
}
