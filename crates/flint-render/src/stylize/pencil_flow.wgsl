// Pencil sketch (#273), the tensor flow blur (#392): between the prep and
// the drawing, two passes of a separable Gaussian over pass 1's stroke
// directions, for the variants that hatch along the picture's edges
// (`direction = "tensor"`). The drawing pass used to blur them itself,
// 25 taps over about 1.5 stroke spacings; round shadow edges and the
// ground's ambient occlusion the tensor flips between neighbouring
// pixels, and a kernel that sparse and that short let the flips through,
// so the hatching frayed into short crossing patches. This one reaches
// about 3 spacings, and the across pass weighs each tap by its contour
// strength: where the scene's depth steps or its surface creases, the
// direction steers the flow; on a flat, noisy patch it barely counts.
//
// fs_across reads the prep target (tone, contour, flow) and writes the
// weighted flow blurred along x; fs_down blurs that along y. Both write
// rg = the blurred double-angle vector. Pixels whose variant is not
// tensor, or passes through, write zero (the drawing pass never reads it).
//
// Both run at half resolution: the flow is smooth over a dozen pixels, so
// a quarter of the pixels lose nothing. A half-resolution pixel's centre
// is the corner of four full ones, and taps step in whole pairs of full
// pixels, so every bilinear read of the prep averages a 2x2 block and the
// reads tile the row with no gaps (up to the reach TAPS steps of 2 cover).

@group(1) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

// Taps either side of the centre. The step between them grows with the
// reach, so the cost is fixed: 2 x TAPS + 1 bilinear reads a pass.
const TAPS: i32 = 9;

// How much a tap on a flat patch counts beside one on a full contour
const FLAT_WEIGHT: f32 = 0.3;

// The pixel's row, or -1 when it isn't drawn with tensor flow
fn tensor_slot(uv: vec2<f32>) -> i32 {
    let slot = slot_of(mask_id(mask_px(uv)));
    if passes_through(slot) || u32(u.rows[slot].e.w + 0.5) != 2u {
        return -1;
    }
    return i32(slot);
}

// The blur's reach in pixels: three of the row's stroke spacings
fn reach_px(slot: i32) -> f32 {
    return u.rows[slot].a.x * u.pixels_per_point * 3.0;
}

fn blur(uv: vec2<f32>, axis: vec2<f32>, slot: i32, weigh: bool) -> vec2<f32> {
    let reach = reach_px(slot);
    // In full pixels, a whole number of pairs (see above)
    let step = 2.0 * max(round(reach / (2.0 * f32(TAPS))), 1.0);
    // Sigma at half the reach: the last tap is two sigmas out
    let inv_two_sigma2 = 1.0 / (2.0 * (0.5 * reach) * (0.5 * reach));
    let texel = 1.0 / u.size;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var k = -TAPS; k <= TAPS; k++) {
        let d = f32(k) * step;
        let g = exp(-d * d * inv_two_sigma2);
        let s = textureSampleLevel(source_texture, source_sampler, uv + axis * d * texel, 0.0);
        var flow = s.xy;
        if weigh {
            // The prep target: contour in y, flow in zw
            flow = s.zw * mix(FLAT_WEIGHT, 1.0, clamp(s.y, 0.0, 1.0));
        }
        sum += flow * g;
        total += g;
    }
    return sum / total;
}

@fragment
fn fs_across(in: VsOut) -> @location(0) vec4<f32> {
    let slot = tensor_slot(in.uv);
    if slot < 0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(blur(in.uv, vec2<f32>(1.0, 0.0), slot, true), 0.0, 0.0);
}

@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    let slot = tensor_slot(in.uv);
    if slot < 0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(blur(in.uv, vec2<f32>(0.0, 1.0), slot, false), 0.0, 0.0);
}
