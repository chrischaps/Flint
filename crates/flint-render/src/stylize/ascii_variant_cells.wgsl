// ASCII variant cell pass (#313): each per-entity variant cell's colour,
// once a cell instead of once a pixel. ascii_common.wgsl comes first.
//
// The glyph pass draws a per-entity variant's pixel from the mean of the
// lattice samples of the pixel's own variant in its cell (#266). Each of a
// cell's 64 or more pixels used to load those 16 samples and their 16
// mask texels for itself; this pass does it once for the cell, and the
// glyph pass reads one texel (two at most).
//
// Variants on one grid (the same cell, origin and footprint) share a band
// of rows: `u.band_rows[b]` = (first row, rows, columns, footprint bits),
// `u.bands[b]` = (cell, origin). A variant's `cell_row` is its band's first
// row + 1 (0 = no band: the glyph pass samples for itself). A cell is a
// texel in each of two targets, the first two of the band's variants met
// in its lattice (in sample order), each holding
//
//   (sum.r, sum.g, sum.b as f32 bits, slot | count << 8 | more << 16)
//
// where `more` says a third of the band's variants sampled the cell too:
// a pixel of that one samples for itself. A slot of 0 is an empty entry.
// Each sum is added in the glyph pass's own order and divided there, so
// the picture is the same to the bit.

@group(1) @binding(0) var cell_input: texture_2d<f32>;
@group(1) @binding(1) var cell_mask: texture_2d<u32>;

// As the glyph pass's `shown_entity`
fn cell_shown(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let c = clamp(textureLoad(cell_input, clamp(p, vec2<i32>(0), max_px), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

// As the glyph pass's `slot_at`
fn cell_slot(p: vec2<f32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(cell_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    let id = textureLoad(cell_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r;
    return slot_of(id);
}

struct Entries {
    @location(0) first: vec4<u32>,
    @location(1) second: vec4<u32>,
};

fn entry(sum: vec3<f32>, slot: u32, count: u32, more: u32) -> vec4<u32> {
    if slot == 0u {
        return vec4<u32>(0u, 0u, 0u, more << 16u);
    }
    return vec4<u32>(bitcast<vec3<u32>>(sum), slot | (count << 8u) | (more << 16u));
}

@fragment
fn fs_main(in: VertexOutput) -> Entries {
    let texel = vec2<i32>(floor(in.position.xy));
    var out: Entries;
    out.first = vec4<u32>(0u);
    out.second = vec4<u32>(0u);
    var band = -1;
    for (var i = 0u; i < min(u.variant_meta.z, MAX_BANDS); i++) {
        let r = u.band_rows[i];
        if texel.y >= i32(r.x) && texel.y < i32(r.x + r.y) && texel.x < i32(r.z) {
            band = i32(i);
        }
    }
    if band < 0 {
        return out;
    }
    let rows = u.band_rows[band];
    let cell_size = u.bands[band].xy;
    let origin = u.bands[band].zw;
    let footprint = bitcast<f32>(rows.w);
    let member = rows.x + 1u;

    // The lattice, as `variant_pixel` lays it
    let cell = vec2<f32>(vec2<i32>(texel.x, texel.y - i32(rows.x)) - vec2<i32>(CELL_OFFSET));
    let cell_min = cell * cell_size + origin;
    let foot = cell_size * max(footprint, 1.0);
    let centre = cell_min + 0.5 * cell_size;

    // The band's first two variants in the cell, in sample order, and
    // which samples are each one's
    var first = 0u;
    var second = 0u;
    var first_bits = 0u;
    var second_bits = 0u;
    var more = 0u;
    for (var k = 0u; k < 16u; k++) {
        let q = floor(centre + ((vec2<f32>(f32(k & 3u), f32(k >> 2u)) + 0.5) / 4.0 - 0.5) * foot);
        let s = cell_slot(q);
        // Pass-through variants have no band, so no row of theirs matches
        if s == 0u || variants[s].cell_row != member {
            continue;
        }
        if first == 0u || first == s {
            first = s;
            first_bits |= 1u << k;
        } else if second == 0u || second == s {
            second = s;
            second_bits |= 1u << k;
        } else {
            more = 1u;
        }
    }
    if first == 0u {
        return out;
    }
    // Each sum in sample order, as the glyph pass adds them
    var sum_a = vec3<f32>(0.0);
    var sum_b = vec3<f32>(0.0);
    for (var k = 0u; k < 16u; k++) {
        let bit = 1u << k;
        if ((first_bits | second_bits) & bit) == 0u {
            continue;
        }
        let q = floor(centre + ((vec2<f32>(f32(k & 3u), f32(k >> 2u)) + 0.5) / 4.0 - 0.5) * foot);
        let c = cell_shown(vec2<i32>(q)) * 1.0;
        if (first_bits & bit) != 0u {
            sum_a += c;
        } else {
            sum_b += c;
        }
    }
    out.first = entry(sum_a, first, countOneBits(first_bits), more);
    out.second = entry(sum_b, second, countOneBits(second_bits), more);
    return out;
}
