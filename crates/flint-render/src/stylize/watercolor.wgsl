// Watercolor paint pass (#272). Reads the finished picture, its wash (the
// picture diffused by watercolor_wash.wgsl), the entity style mask and the
// depth, and lays transparent pigment on paper:
//
//   1. wobble   every read is displaced by a smooth noise field in points,
//               so wash borders (and a painted entity's silhouette) wander
//   2. bleed    the colour is mixed toward the wash
//   3. density  pigment pools at colour edges (difference of picture and
//               wash) and on the near side of silhouettes (depth), thins
//               and thickens in low-frequency turbulence, and settles into
//               the paper's valleys (granulation)
//   4. pigment  the colour as a transmittance raised to the density, C^d
//               (Beer-Lambert): d > 1 deepens, d < 1 dilutes, white stays
//   5. paper    the result times the paper's colour and its relief
//
// Everything happens in display values; on an sRGB target samples are
// decoded on the way in and the result encoded on the way out.

struct Row {
    wobble: f32,
    wobble_scale: f32,
    bleed: f32,
    edge: f32,
    edge_depth: f32,
    edge_width: f32,
    turbulence: f32,
    granulation: f32,
    grain: f32,
    grain_scale: f32,
    pigment: f32,
    flags: u32,
    paper: vec3<f32>,
    layers: f32,
    // The paper image's layer plus one (#390, #547); 0 = procedural
    image: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

struct PaintUniforms {
    size: vec2<f32>,
    pixels_per_point: f32,
    srgb: u32,
    row_count: u32,
    passthrough: u32,
    near: f32,
    far: f32,
    // Each layer of the paper stack (#547): measures and size
    papers: array<StyleImageLayer, 4>,
};

@group(0) @binding(0) var<uniform> params: PaintUniforms;
// The variant table (#740, the framework's): a row a variant, and the row
// of each mask id, as long as the frame needs
@group(0) @binding(1) var<storage, read> paint_rows: array<Row>;
@group(0) @binding(2) var<storage, read> flint_slots: array<u32>;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var wash_texture: texture_2d<f32>;
@group(2) @binding(1) var wash_sampler: sampler;
@group(3) @binding(0) var flint_mask_texture: texture_2d<u32>;
@group(3) @binding(1) var depth_texture: texture_depth_2d;
// The paper stack (#390, #547), a blank texel while none is named
@group(3) @binding(2) var paper_texture: texture_2d_array<f32>;

const PASS_THROUGH: u32 = 1u;

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

// --- noise: stylize/noise.wgsl's value noise (#1256), under the paint's seed ---

const PAINT_SEED: u32 = 0u;

fn vnoise(p: vec2<f32>) -> f32 {
    return flint_vnoise(p, PAINT_SEED);
}

// Two octaves, -1..1 on each axis: where a read wanders to
fn wobble_field(p: vec2<f32>) -> vec2<f32> {
    let o = vec2<f32>(37.2, 91.7);
    let x = vnoise(p) * 0.67 + vnoise(p * 2.03 + o) * 0.33;
    let y = vnoise(p + o * 1.7) * 0.67 + vnoise(p * 2.03 + o * 2.9) * 0.33;
    return vec2<f32>(x, y) * 2.0 - 1.0;
}

// The paper's height, 0 (valley) to 1 (tooth), from a position in points:
// a cold-press tooth of three octaves and long faint fibres
fn paper_height(pt: vec2<f32>, scale: f32) -> f32 {
    let p = pt / scale;
    var h = vnoise(p / 9.0) * 0.45 + vnoise(p / 4.0 + vec2<f32>(11.0, 3.0)) * 0.35
        + vnoise(p / 1.8 + vec2<f32>(5.0, 17.0)) * 0.20;
    // Fibres: a few short strands, noise stretched along a slanted axis
    let f = vec2<f32>(p.x * 0.5 + p.y * 0.2, p.y * 0.12 - p.x * 0.05);
    let fibre = vnoise(f);
    h = h * 0.92 + smoothstep(0.78, 0.97, fibre) * 0.08;
    return clamp((h - 0.5) * 1.6 + 0.5, 0.0, 1.0);
}

// Low-frequency blotches, 0..1
fn turbulence(pt: vec2<f32>) -> f32 {
    return vnoise(pt / 170.0) * 0.65 + vnoise(pt / 70.0 + vec2<f32>(23.0, 7.0)) * 0.35;
}

// --- the mask and the depth, read by UV (both at scene resolution) ---

// The mask is read through mask_read.wgsl, the slots through mask_slot.wgsl
// (#1256); a slot past the rows is the scene's
fn slot_of(id: u32) -> u32 {
    let slot = flint_slot(id);
    return select(0u, slot, slot < params.row_count);
}

fn view_depth(uv: vec2<f32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(depth_texture));
    let c = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1);
    let d = textureLoad(depth_texture, c, 0);
    return params.near * params.far / (params.far - d * (params.far - params.near));
}

fn painted(slot: u32) -> bool {
    let off = (paint_rows[slot].flags & PASS_THROUGH) != 0u;
    return !off && !(slot == 0u && params.passthrough == 1u);
}

fn sample_display(t: texture_2d<f32>, s: sampler, uv: vec2<f32>) -> vec4<f32> {
    let c = textureSampleLevel(t, s, uv, 0.0);
    let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(select(rgb, flint_to_display(rgb), params.srgb == 1u), c.a);
}

// An unpainted pixel, as fs_main returns one whose wobbled read finds no
// painted entity either (#538): the per-entity paint pass is scissored to
// its entities, and this entry point draws the frame around them, so the
// pixels there are fs_main's own bytes
@fragment
fn fs_passthrough(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSampleLevel(input_texture, input_sampler, in.uv, 0.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let ppp = params.pixels_per_point;
    let pt = in.uv * params.size / ppp;
    let to_uv = ppp / params.size;

    // Where this pixel's paint comes from: the mask read at a wobbled
    // position, so a painted silhouette wanders over its surroundings
    let home = slot_of(flint_mask_id(in.uv));
    let r0 = paint_rows[home];
    let reach = wobble_field(pt / r0.wobble_scale) * r0.wobble * to_uv;
    let slot = slot_of(flint_mask_id(in.uv + reach));
    if (!painted(slot)) {
        // Unpainted. If the pixel itself is a painted entity's, its paint
        // wandered off: show what's beside it, as the wash would
        let at = select(in.uv, in.uv + reach, painted(home));
        return textureSampleLevel(input_texture, input_sampler, at, 0.0);
    }
    let r = paint_rows[slot];
    let wuv = in.uv + wobble_field(pt / r.wobble_scale) * r.wobble * to_uv;

    let src = sample_display(input_texture, input_sampler, wuv);
    let wash = sample_display(wash_texture, wash_sampler, wuv).rgb;
    var base = mix(src.rgb, wash, r.bleed);

    // Glazes: the wash's tone in `layers` steps, as a painter builds it up
    // one transparent layer over another. The steps follow the wash, so
    // their borders are soft curves, and each is edged darker on its
    // darker side, where the upper layer's pigment dried
    var glaze_edge = 0.0;
    if (r.layers >= 1.0) {
        // Steps are even in the square root of the tone, so the darks get
        // as many glazes as the lights instead of one hard jump, and no
        // step moves a colour by more than a glaze's worth
        let lw = max(flint_luma(wash), 1e-3);
        let sw = sqrt(lw);
        let q = sw * r.layers;
        let ss = (floor(q) + smoothstep(0.2, 0.8, fract(q))) / r.layers;
        let ratio = clamp(ss * ss / lw, 0.8, 1.2);
        base = clamp(base * ratio, vec3<f32>(0.0), vec3<f32>(1.0));
        let o = 1.5 * to_uv;
        let qx = flint_luma(sample_display(wash_texture, wash_sampler, wuv + vec2<f32>(o.x, 0.0)).rgb)
            - flint_luma(sample_display(wash_texture, wash_sampler, wuv - vec2<f32>(o.x, 0.0)).rgb);
        let qy = flint_luma(sample_display(wash_texture, wash_sampler, wuv + vec2<f32>(0.0, o.y)).rgb)
            - flint_luma(sample_display(wash_texture, wash_sampler, wuv - vec2<f32>(0.0, o.y)).rgb);
        // q per point (d sqrt(l) = dl / 2 sqrt(l)), so the distance to the
        // step is in points
        let slope = length(vec2<f32>(qx, qy)) * r.layers / (6.0 * sw);
        let below = 0.5 - fract(q);
        let dist = below / max(slope, 1e-4);
        glaze_edge = select(0.0, 1.0 - smoothstep(0.0, 2.5, dist), below >= 0.0);
    }

    // Edge darkening: where the picture leaves its wash (a colour edge)...
    let colour_edge = smoothstep(0.02, 0.3, length(src.rgb - wash));
    // ...and on the near side of a silhouette, where pigment pools against
    // the farther wash behind it
    let w = r.edge_width * to_uv;
    let z = view_depth(wuv);
    let farther = max(
        max(view_depth(wuv + vec2<f32>(w.x, 0.0)), view_depth(wuv - vec2<f32>(w.x, 0.0))),
        max(view_depth(wuv + vec2<f32>(0.0, w.y)), view_depth(wuv - vec2<f32>(0.0, w.y))),
    );
    let depth_edge = smoothstep(0.02, 0.15, (farther - z) / max(z, 1e-3));

    // Density: the pigment's thickness here, 1 = as the picture has it.
    // The paper image (#390), a texel every grain_scale points, gives the
    // height and a tint in place of the noise
    var h = paper_height(pt, r.grain_scale);
    var tint = vec3<f32>(1.0);
    if (r.image > 0.5) {
        let lod = log2(max(1.0 / (r.grain_scale * ppp), 1e-3));
        let layer = img_layer(r.image);
        let info = params.papers[layer];
        let c = img_read(paper_texture, layer, info, pt / r.grain_scale, lod, true, vec2<f32>(0.0), vec2<f32>(0.0));
        h = img_paper_height(c, info.stats.x, info.stats.y);
        tint = img_paper_tint(c);
    }
    let load = 1.0 - flint_luma(base);
    var d = r.pigment;
    // Pooling deepens a dark colour fastest under C^d, so it fades with the
    // load: a pale wash gets a crisp rim, a near-black one no inky outline
    d += r.edge * max(colour_edge * 0.6, glaze_edge * 0.45) * (1.0 - 0.6 * load);
    d += r.edge_depth * depth_edge * 0.8;
    d += r.turbulence * (turbulence(pt) - 0.5) * 1.6;
    d += r.granulation * (0.5 - h) * (0.25 + load) * 1.2;
    d = max(d, 0.0);
    // The colour is the pigment layer's transmittance over white paper;
    // thickening the layer d times raises it to the d-th power
    // (Beer-Lambert). White stays the paper, black stays black, and a
    // mid-tone moves more than under Bousseau's quadratic.
    let pigment = pow(max(base, vec3<f32>(1e-4)), vec3<f32>(d));

    // The paper: its colour, and its relief catching light
    let relief = 1.0 - r.grain * (0.5 - h) * 0.3;
    let out = clamp(pigment * r.paper * tint * relief, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(select(out, flint_to_linear(out), params.srgb == 1u), src.a);
}
