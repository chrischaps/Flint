// Pencil sketch (#273), pass 2: the drawing. From pass 1's tone, contour
// and flow it lays graphite on paper:
//   - hatching: up to four layers of parallel strokes, each switched on
//     as the tone darkens past its threshold and thickening in from
//     nothing, so tone builds the way a hand builds it. Each stroke is a
//     dash of its own length that tapers at both ends, jittered off its
//     row and bent by a slow wobble. The direction comes from the blurred
//     flow (surface or tensor) or the base angle (screen), quantized to 12
//     bins of 15 degrees with neighbouring bins blended, so strokes stay
//     straight within a patch, as hatching by hand does.
//   - contours: pass 1's lines read through a wobbling offset, twice (the
//     second at another offset by `sketchy`), so outlines overshoot and
//     double the way quick sketch lines do. Where `clutter` asks, a line
//     fades where pass 1's lines already cover most of the disc round it
//     (#883): a meadow's blades, each a silhouette, would merge there into
//     one black fill.
//   - paper: a tone with grain at three scales; graphite catches on the
//     paper's tooth, so strokes break up where the paper dips. A paper
//     image (#390) takes the noise's place, and a stroke image the
//     analytic dash's (style_image.wgsl reads both).
// Every random choice is an integer hash of the stroke's row, its dash,
// its layer and the seed: a fixed seed draws the same picture every run.

@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(3) @binding(0) var prep_texture: texture_2d<f32>;
@group(3) @binding(1) var prep_sampler: sampler;
// The tensor flow, blurred wide by pencil_flow.wgsl (#392); read only by
// rows whose direction is tensor
@group(3) @binding(2) var flow_texture: texture_2d<f32>;

const PI: f32 = 3.14159265;
const BINS: f32 = 12.0;

// Clutter (#883): how much of a disc round `uv` pass 1 has inked, 0..1.
// Sixteen bilinear taps on a sunflower spiral out to `radius` pixels, each
// averaging four texels. One outline through the disc covers about a fifth
// of it (a corner two fifths); a field of small silhouettes, every blade a
// depth step against the ground behind it, two thirds and more. Measured on
// the reveal_tear meadow under rotoscope: the props' lines read 0.17 at the
// median and 0.39 at the 95th percentile, the meadow 0.65
const CLUTTER_TAPS: i32 = 16;

fn ink_density(uv: vec2<f32>, radius: f32, texel: vec2<f32>) -> f32 {
    var sum = 0.0;
    for (var k = 0; k < CLUTTER_TAPS; k++) {
        let a = f32(k) * 2.3999632;
        let r = radius * sqrt((f32(k) + 0.5) / f32(CLUTTER_TAPS));
        let o = vec2<f32>(cos(a), sin(a)) * r;
        sum += textureSampleLevel(prep_texture, prep_sampler, uv + o * texel, 0.0).y;
    }
    return sum / f32(CLUTTER_TAPS);
}

// The near disc clutter measures, in pixels: two outline widths, at least
// three; the wide one is three times it
fn clutter_radius(row: PencilRow, ppp: f32) -> f32 {
    return max(2.0 * row.b.w * ppp, 3.0);
}

// One layer's strokes at angle `ang`: coverage 0..1 at point `p` (points).
// `grow` (0..1) is how far past its threshold the tone is.
fn strokes(p: vec2<f32>, ang: f32, layer: u32, seed: u32, row: PencilRow, grow: f32, aa: f32) -> f32 {
    let spacing = max(row.a.x * (1.0 - 0.08 * f32(layer)), 0.75);
    let dir = vec2<f32>(cos(ang), sin(ang));
    let along = dot(p, dir);
    var across = dot(p, vec2<f32>(-dir.y, dir.x));
    // A slow bend, shared by neighbouring strokes like a hand's drift
    let bend = vnoise(vec2<f32>(along / (spacing * 9.0), across / (spacing * 5.0)), seed ^ (layer * 7919u));
    across += (bend - 0.5) * 2.0 * row.c.w * 0.6;

    let r = floor(across / spacing);
    let ri = i32(r);
    let h = hash3(ri, i32(layer), seed);
    let off = across - (r + 0.5 + (h.x - 0.5) * 0.4) * spacing;

    // Dashes along the stroke, each its own length, a few left out
    let len = spacing * (8.0 + 16.0 * h.y);
    let s = (along + h.z * len) / len;
    let seg = floor(s);
    let x = s - seg;
    let hs = hash3(ri, i32(seg), seed + layer * 104729u + 17u);
    if hs.x < 0.1 {
        return 0.0;
    }
    if row.h.w > 0.5 {
        return strip(x, off, row.a.y * 2.0 * sqrt(grow), len * u.pixels_per_point, row, hs);
    }
    let taper = smoothstep(0.0, 0.15, x) * smoothstep(1.0, 0.85, x);
    let width = row.a.y * (0.45 + 0.55 * taper) * sqrt(grow);
    let cov = 1.0 - smoothstep(width * 0.5 - aa, width * 0.5 + aa, abs(off));
    return cov * mix(0.5, 1.0, taper) * (0.7 + 0.3 * hs.y);
}

// Drawn on (#879): how much of the dash under `p` the hatch front `front`
// has drawn, 0..1, from its start. The dash's geometry is `strokes`'s, line
// for line; it is its own function because a cut inside `strokes` changed
// how the driver compiled it, moving 83 pixels of every pencil frame by one
// level even with nothing drawing on.
fn stroke_drawn(p: vec2<f32>, ang: f32, layer: u32, seed: u32, row: PencilRow, aa: f32, front: f32) -> f32 {
    let spacing = max(row.a.x * (1.0 - 0.08 * f32(layer)), 0.75);
    let dir = vec2<f32>(cos(ang), sin(ang));
    let along = dot(p, dir);
    var across = dot(p, vec2<f32>(-dir.y, dir.x));
    let bend = vnoise(vec2<f32>(along / (spacing * 9.0), across / (spacing * 5.0)), seed ^ (layer * 7919u));
    across += (bend - 0.5) * 2.0 * row.c.w * 0.6;

    let r = floor(across / spacing);
    let ri = i32(r);
    let h = hash3(ri, i32(layer), seed);
    let off = across - (r + 0.5 + (h.x - 0.5) * 0.4) * spacing;

    let len = spacing * (8.0 + 16.0 * h.y);
    let s = (along + h.z * len) / len;
    let seg = floor(s);
    let x = s - seg;
    let hs = hash3(ri, i32(seg), seed + layer * 104729u + 17u);
    // The dash's turn is the order at its middle
    let mid = vec2<f32>(along - (x - 0.5) * len, across - off);
    let at = mid.x * dir + mid.y * vec2<f32>(-dir.y, dir.x);
    let order = draw_order(at, row.a.x * ORDER_ROWS, u32(row.i.w));
    return dash_cut(x, dash_progress(front, order, hs.z), aa / len);
}

// A dash drawn from the stroke image (#390): `x` 0..1 along the dash,
// `off` across it from its middle, in the units of `band`, the row's
// height (twice the stroke width, thinner while the tone has only just
// reached the layer). `len_px` is the dash's length on screen in pixels.
// The image is `stroke_rows` strokes stacked top to bottom, each drawn
// left to right; a dash takes one by its hash. Its ink is the image's
// dark, or opaque, texels over the strip's measured ink. Screen and
// object grids call it in points, the world grid (#389) in metres.
fn strip(x: f32, off: f32, band: f32, len_px: f32, row: PencilRow, hs: vec3<f32>) -> f32 {
    let v = off / max(band, 1e-9) + 0.5;
    if v <= 0.0 || v >= 1.0 {
        return 0.0;
    }
    let rows = max(u32(row.h.y + 0.5), 1u);
    let pick = min(u32(hs.z * f32(rows)), rows - 1u);
    let dims = vec2<f32>(textureDimensions(stroke_tex, 0));
    let rh = dims.y / f32(rows);
    // The level from the texels a pixel spans along the dash, but never
    // so coarse that a row is under two texels tall: across, a stroke is a
    // pixel or two whatever the image, and rows mustn't bleed together
    let along = dims.x / max(len_px, 1e-3);
    let lod = min(log2(max(along, 1.0)), log2(max(rh * 0.5, 1.0)));
    let lo = vec2<f32>(0.0, f32(pick) * rh);
    let hi = vec2<f32>(dims.x, f32(pick + 1u) * rh);
    let c = img_read(stroke_tex, vec2<f32>(x * dims.x, (f32(pick) + v) * rh), lod, false, lo, hi);
    let ink = c.a * (1.0 - luma(c.rgb)) / max(u.img.z, 0.02);
    return clamp(ink, 0.0, 1.0) * (0.75 + 0.25 * hs.y);
}

// --- strokes on the world's surfaces (#389) ---------------------------------
//
// With `grid = "world"` the strokes are laid on the surfaces themselves, so
// they travel with the props as the camera moves instead of the surfaces
// sliding under a sheet of screen hatching. Each pixel's world position
// (from the depth) is projected onto the three axis planes, triplanar,
// weighted by its normal, and each plane is hatched with stripes measured
// in metres.
//
// Stripes a fixed number of metres apart would crowd together into grey
// with distance, so the spacing comes in octaves: level k puts its rows
// 2^k metres apart. A pixel picks the level whose spacing on screen is
// nearest the `spacing` asked for, and cross-fades between that level and
// the next as a mip chain does, so the strokes keep their size on screen
// while each one stays where it lies on the surface. Every even row of
// level k is a row of level k+1 too, and hashes a row by its odd part and
// the level where that becomes odd, so a stroke keeps its hand (its dashes,
// its jitter's seed) as it climbs the octaves.

// The identity of row `r` at level `k`, the same at every level it is a
// row of: its odd part and the level that gives it.
fn row_identity(r: i32, k: i32) -> vec2<i32> {
    if r == 0 {
        return vec2<i32>(0, 1024);
    }
    let tz = i32(countTrailingZeros(bitcast<u32>(r)));
    return vec2<i32>(r >> u32(tz), k + tz);
}

// One layer's strokes at one level on one plane: coverage 0..1 at `q`
// (metres in the plane). `s` is the level's spacing, `k` its number;
// `width` and `wob` are metres; `aa` is how far `across` moves in a
// pixel, metres.
fn world_strokes(q: vec2<f32>, ang: f32, layer: u32, seed: u32, s: f32, k: i32, width: f32, wob: f32, grow: f32, aa: f32, row: PencilRow) -> f32 {
    let spacing = s * (1.0 - 0.08 * f32(layer));
    let dir = vec2<f32>(cos(ang), sin(ang));
    let along = dot(q, dir);
    var across = dot(q, vec2<f32>(-dir.y, dir.x));
    let bend = vnoise(vec2<f32>(along / (spacing * 9.0), across / (spacing * 5.0)), seed ^ (layer * 7919u) ^ pcg(bitcast<u32>(k)));
    across += (bend - 0.5) * 2.0 * wob;

    let r = floor(across / spacing);
    let id = row_identity(i32(r), k);
    let salt = pcg(bitcast<u32>(id.y) + 0x9e3779b9u);
    let h = hash3(id.x, i32(layer), seed ^ salt);
    let off = across - (r + 0.5 + (h.x - 0.5) * 0.4) * spacing;

    let len = spacing * (8.0 + 16.0 * h.y);
    let t = (along + h.z * len) / len;
    let seg = floor(t);
    let x = t - seg;
    let hs = hash3(id.x, i32(seg), (seed + layer * 104729u + 17u) ^ salt);
    if hs.x < 0.1 {
        return 0.0;
    }
    if row.h.w > 0.5 {
        // `aa` is 0.7 of a pixel in metres, so a pixel is aa / 0.7
        return strip(x, off, width * 2.0 * sqrt(grow), len * 0.7 / max(aa, 1e-9), row, hs);
    }
    let taper = smoothstep(0.0, 0.15, x) * smoothstep(1.0, 0.85, x);
    let w = width * (0.45 + 0.55 * taper) * sqrt(grow);
    let a = clamp(aa, 1e-6, spacing * 0.35);
    let cov = 1.0 - smoothstep(w * 0.5 - a, w * 0.5 + a, abs(off));
    return cov * mix(0.5, 1.0, taper) * (0.7 + 0.3 * hs.y);
}

// Drawn on (#879): `stroke_drawn` for `world_strokes`'s dashes, the order
// laid out in metres on the plane so it holds to the surface as they do
fn world_stroke_drawn(q: vec2<f32>, ang: f32, layer: u32, seed: u32, s: f32, k: i32, wob: f32, aa: f32, row: PencilRow, front: f32) -> f32 {
    let spacing = s * (1.0 - 0.08 * f32(layer));
    let dir = vec2<f32>(cos(ang), sin(ang));
    let along = dot(q, dir);
    var across = dot(q, vec2<f32>(-dir.y, dir.x));
    let bend = vnoise(vec2<f32>(along / (spacing * 9.0), across / (spacing * 5.0)), seed ^ (layer * 7919u) ^ pcg(bitcast<u32>(k)));
    across += (bend - 0.5) * 2.0 * wob;

    let r = floor(across / spacing);
    let id = row_identity(i32(r), k);
    let salt = pcg(bitcast<u32>(id.y) + 0x9e3779b9u);
    let h = hash3(id.x, i32(layer), seed ^ salt);
    let off = across - (r + 0.5 + (h.x - 0.5) * 0.4) * spacing;

    let len = spacing * (8.0 + 16.0 * h.y);
    let t = (along + h.z * len) / len;
    let seg = floor(t);
    let x = t - seg;
    let hs = hash3(id.x, i32(seg), (seed + layer * 104729u + 17u) ^ salt);
    let mid = vec2<f32>(along - (x - 0.5) * len, across - off);
    let at = mid.x * dir + mid.y * vec2<f32>(-dir.y, dir.x);
    let order = draw_order(at, spacing * ORDER_ROWS, u32(row.i.w));
    return dash_cut(x, dash_progress(front, order, hs.z), aa / len);
}

// The plane coordinates of a world point on axis plane `axis`
fn plane_coords(w: vec3<f32>, axis: u32) -> vec2<f32> {
    if axis == 0u {
        return w.zy;
    }
    if axis == 1u {
        return w.xz;
    }
    return w.xy;
}

// --- drawn on (#879) ------------------------------------------------------------
//
// With `transition = "draw"` a blend between 0 and 1 is a front, not a mix:
// the outlines are drawn up to it, the hatching follows them a stroke at a
// time, and the paper's wash comes last. Where the front hasn't reached a
// line it is absent, not faint. Every mark asks one order field when it is
// drawn: a smooth noise about a dozen hatch rows across, flattened through
// its measured distribution so that a front at t has drawn t of each line.
// Along a line the field rises and falls slowly, so as the front moves the
// drawn parts grow out along the line from the field's low points, the way
// a hand traces. The field is keyed by the row's own seed, without the
// boil's steps, so a boiling drawing grows in one order while its lines
// redraw.

// Hatch rows the order field's features span
const ORDER_ROWS: f32 = 14.0;

// The order noise's quantiles (k / 8, k = 0..8), measured over a million
// samples of 0.65 v(q) + 0.35 v(2.03 q + 17): flattening through them
// spreads the order evenly over 0..1
const ORDER_KNOTS = array<f32, 9>(0.0, 0.3105, 0.3864, 0.4464, 0.5015, 0.556, 0.6158, 0.6911, 1.0);

// When in the drawing the point `p` (points, or metres on a world plane)
// is reached, 0..1; `scale` is the field's feature size in p's units
fn draw_order(p: vec2<f32>, scale: f32, seed: u32) -> f32 {
    let q = p / max(scale, 1e-6);
    let s = pcg(seed ^ 0x6a09e667u);
    let n = 0.65 * vnoise(q, s) + 0.35 * vnoise(q * 2.03 + 17.0, s + 1u);
    var knots = ORDER_KNOTS;
    var lo = 0.0;
    for (var k = 1; k < 9; k++) {
        let a = knots[k - 1];
        let b = knots[k];
        if n <= b || k == 8 {
            return clamp((f32(k - 1) + (n - a) / max(b - a, 1e-6)) / 8.0, 0.0, 1.0);
        }
    }
    return lo;
}

// How much of a mark whose turn is at `order` the front `t` (0..1) has
// drawn: none at t = 0, all at t = 1, rising over `soft` of the order
fn drawn(t: f32, order: f32, soft: f32) -> f32 {
    return clamp((t * (1.0 + soft) - order) / soft, 0.0, 1.0);
}

// The front's three phases: outlines first, hatching after them, the
// paper's wash last (each 0..1 of its own run)
fn contour_front(f: f32) -> f32 {
    return clamp(f / 0.9, 0.0, 1.0);
}

fn hatch_front(f: f32) -> f32 {
    return clamp((f - 0.25) / 0.7, 0.0, 1.0);
}

fn paper_front(f: f32) -> f32 {
    return clamp((f - 0.4) / 0.6, 0.0, 1.0);
}

// How much of its order a dash takes to draw from end to end
const DASH_DRAW: f32 = 0.15;

// How far along a dash (0..1 from its start) the hatch front `t` has drawn
// it, the dash's turn `order` jittered by its own hash
fn dash_progress(t: f32, order: f32, jitter: f32) -> f32 {
    let o = clamp(order * 0.85 + jitter * 0.15, 0.0, 1.0);
    return drawn(t, o, DASH_DRAW);
}

// A dash's coverage cut at `progress`, `x` its position along it (0..1),
// `ax` how far x moves in a pixel
fn dash_cut(x: f32, progress: f32, ax: f32) -> f32 {
    if progress >= 1.0 {
        return 1.0;
    }
    return 1.0 - smoothstep(progress - ax, progress, x);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.pos.xy);
    // The surface's world position and how it moves across a pixel, taken
    // before anything branches (derivatives need every pixel of a quad)
    let wp = world_uv(in.uv);
    let wdx = dpdx(wp.xyz);
    let wdy = dpdy(wp.xyz);
    let src = textureLoad(input_texture, px, 0);
    let id = mask_id(mask_px(in.uv));
    let slot = slot_of(id);
    if passes_through(slot) {
        return src;
    }
    let row = u.rows[slot];
    let ppp = u.pixels_per_point;
    let texel = 1.0 / u.size;
    let prep = textureSampleLevel(prep_texture, prep_sampler, in.uv, 0.0);
    let seed = u32(max(row.d.w, 0.0));

    // Points from the row's origin (an object grid's anchor)
    let p = (in.pos.xy - row.g.xy) / ppp;
    let pt = in.pos.xy / ppp;
    let aa = 0.7 / ppp;

    // Drawn on (#879): how far the drawing has got here. The whole frame's
    // front (its table's blend, when the table draws), times the entity's
    // blend from the mask when its row draws. At 1 nothing below changes:
    // the drawing is whole, as a fade draws it
    var front = u.front;
    if row.i.z > 0.5 && slot != 0u {
        front *= mask_blend(mask_px(in.uv));
    }
    let drawing = DRAW_ON && front < 1.0;
    // Above 1 tells the strokes to draw whole
    let hatch_at = select(2.0, hatch_front(front), drawing);
    let order_scale = row.a.x * ORDER_ROWS;
    let order_seed = u32(row.i.w);
    var order_here = 0.0;
    if drawing {
        order_here = draw_order(p, order_scale, order_seed);
    }

    // The flow, with the base angle underneath where it is unsure. Surface
    // directions are blurred here over about a stroke's length (they are
    // smooth already, and a wider blur would carry them over silhouettes);
    // tensor ones come blurred over three spacings by the flow passes
    let base = row.a.w;
    let screen = vec2<f32>(cos(2.0 * base), sin(2.0 * base));
    var v = screen;
    let direction = u32(row.e.w + 0.5);
    // World strokes on a surface need no flow; the sky keeps the screen's
    let on_world = world_grid(row) && wp.w > 0.5;
    // A row with no hatch layers (line only, #404) draws no strokes, so it
    // skips the flow's taps: the same picture, cheaper
    let layers = u32(clamp(row.a.z, 0.0, 4.0) + 0.5);
    if layers == 0u {
        // no strokes, no flow
    } else if direction == 2u && !on_world {
        v = textureSampleLevel(flow_texture, prep_sampler, in.uv, 0.0).xy + screen * 0.12;
    } else if direction == 0u && !on_world {
        let reach = row.a.x * ppp * 1.5;
        var sum = vec2<f32>(0.0);
        for (var j = -2; j <= 2; j++) {
            for (var i = -2; i <= 2; i++) {
                let o = vec2<f32>(f32(i), f32(j)) * reach * 0.5 * texel;
                sum += textureSampleLevel(prep_texture, prep_sampler, in.uv + o, 0.0).zw;
            }
        }
        v = sum / 25.0 + screen * 0.12;
    }
    let theta = 0.5 * atan2(v.y, v.x);

    // Bins from the base angle, so screen hatching is exactly the angle.
    // Screen mode takes bin 0 outright: through atan2 its theta is base
    // give or take a rounding, and fract(-epsilon) is bin 11 at a blend of
    // nearly 1, the strokes drawn the other way round with every hash
    // moved. Vulkan and DX12 rounded to opposite sides (#314)
    var t = fract((theta - base) / PI) * BINS;
    if direction == 1u {
        t = 0.0;
    }
    let k = floor(t);
    let blend = smoothstep(0.25, 0.75, t - k);
    let a0 = base + k * PI / BINS;
    let a1 = a0 + PI / BINS;

    // Hatching: layer i joins once the darkness passes its threshold
    let dark = 1.0 - prep.x;
    // Each layer crosses the last off the square, so no two make a grid
    var offsets = array<f32, 4>(0.0, 0.36 * PI, -0.22 * PI, 0.14 * PI);
    var light = 1.0;

    // A world grid's plane weights, from the surface's normal
    var weights = vec3<f32>(0.0);
    if on_world {
        let n = oct_decode(textureLoad(prep_texture, px, 0).zw);
        let an = pow(abs(n), vec3<f32>(6.0));
        weights = an / max(an.x + an.y + an.z, 1e-6);
    }

    for (var i = 0u; i < layers; i++) {
        let threshold = 0.12 + f32(i) * 0.8 / f32(layers);
        let grow = clamp((dark - threshold) / 0.2, 0.0, 1.0);
        if grow <= 0.0 {
            continue;
        }
        var cov = 0.0;
        if on_world {
            let ang = base + offsets[i];
            let dir = vec2<f32>(cos(ang), sin(ang));
            let perp = vec2<f32>(-dir.y, dir.x);
            for (var axis = 0u; axis < 3u; axis++) {
                let wt = weights[axis];
                if wt < 0.02 {
                    continue;
                }
                let q = plane_coords(wp.xyz, axis);
                // Metres `across` moves per point on screen, from the
                // quad's derivatives: the octave is picked per plane and
                // per stroke direction, so a floor seen edge-on climbs to
                // wider rows across its depth (an anisotropic mip)
                let rate = max(length(vec2<f32>(
                    dot(plane_coords(wdx, axis), perp),
                    dot(plane_coords(wdy, axis), perp),
                )) * ppp, 1e-7);
                let lvl = log2(row.a.x * rate);
                let octave = floor(lvl);
                let fade = lvl - octave;
                let s0 = exp2(octave);
                let width = row.a.y * rate;
                let wob0 = row.c.w * 0.6 / row.a.x * s0;
                let aa_w = 0.7 * rate / ppp;
                var c0 = world_strokes(q, ang, i, seed, s0, i32(octave), width, wob0, grow, aa_w, row);
                var c1 = world_strokes(q, ang, i, seed, s0 * 2.0, i32(octave) + 1, width, wob0 * 2.0, grow, aa_w, row);
                if drawing {
                    c0 *= world_stroke_drawn(q, ang, i, seed, s0, i32(octave), wob0, aa_w, row, hatch_at);
                    c1 *= world_stroke_drawn(q, ang, i, seed, s0 * 2.0, i32(octave) + 1, wob0 * 2.0, aa_w, row, hatch_at);
                }
                cov += wt * mix(c0, c1, fade);
            }
        } else {
            var c0 = strokes(p, a0 + offsets[i], i, seed, row, grow, aa);
            var c1 = strokes(p, a1 + offsets[i], i, seed, row, grow, aa);
            if drawing {
                c0 *= stroke_drawn(p, a0 + offsets[i], i, seed, row, aa, hatch_at);
                c1 *= stroke_drawn(p, a1 + offsets[i], i, seed, row, aa, hatch_at);
            }
            cov = mix(c0, c1, blend);
        }
        light *= 1.0 - cov * row.g.w * mix(0.6, 1.0, dark);
    }
    // Rubbed graphite under the strokes: soft tone for the darks, rubbed in
    // with the hatching while it draws on
    var rubbed = row.f.w;
    if drawing {
        rubbed *= drawn(hatch_at, order_here, 0.2);
    }
    light *= 1.0 - rubbed * dark * dark;

    // Contours, read through a wobble; a second, fainter line beside the
    // first where `sketchy` asks
    let wob = row.c.w * ppp;
    let freq = 1.0 / (row.a.x * 7.0);
    let w1 = vec2<f32>(vnoise(pt * freq, seed + 101u), vnoise(pt * freq, seed + 202u)) - 0.5;
    let w2 = vec2<f32>(vnoise(pt * freq * 1.3, seed + 303u), vnoise(pt * freq * 1.3, seed + 404u)) - 0.5;
    let reach = max(row.b.w * 0.5 * ppp, 0.5);
    var line1 = 0.0;
    var line2 = 0.0;
    var taps = array<vec2<f32>, 5>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0)
    );
    for (var i = 0; i < 5; i++) {
        let o = taps[i] * reach;
        line1 = max(line1, textureSampleLevel(prep_texture, prep_sampler, in.uv + (w1 * 2.0 * wob + o) * texel, 0.0).y);
        line2 = max(line2, textureSampleLevel(prep_texture, prep_sampler, in.uv + (w2 * 3.0 * wob + o) * texel, 0.0).y);
    }
    // Drawn on (#879): each line as far as the outline front has reached,
    // asked where its wobble reads it, so the tip moves with the line
    if drawing {
        let at = contour_front(front);
        line1 *= drawn(at, draw_order(p + w1 * 2.0 * wob / ppp, order_scale, order_seed), 0.02);
        line2 *= drawn(at, draw_order(p + w2 * 3.0 * wob / ppp, order_scale, order_seed), 0.02);
    }
    let pressure = 0.65 + 0.35 * vnoise(pt * freq * 2.0, seed + 505u);
    var contour = max(line1, line2 * row.d.x * 0.7) * pressure * row.b.z;
    // Clutter (#883): where the lines would already merge into a fill, over
    // a disc two outline widths across and over one three times that, they
    // fade, so a meadow is a sheet with its near edge and its horizon, not a
    // black band. Both discs, because a small figure's limbs fill the near
    // one too, but only a field fills the wide one
    let clutter = row.j.x;
    if clutter > 0.0 && contour > 0.0 {
        let r = clutter_radius(row, ppp);
        let near = smoothstep(0.35, 0.65, ink_density(in.uv, r, texel));
        let wide = smoothstep(0.30, 0.55, ink_density(in.uv, 3.0 * r, texel));
        contour *= 1.0 - clutter * min(near, wide);
    }
    light *= 1.0 - clamp(contour, 0.0, 1.0) * 0.92;

    // Paper: a tone with grain, and tooth the graphite skips over
    let grain = row.d.y;
    var paper_noise: f32;
    var tooth: f32;
    var paper: vec3<f32>;
    if row.h.z > 0.5 {
        // The paper image (#390), tiled in points from the screen's corner:
        // its height is the tooth and the tone, its hue tints the paper
        let scale = max(row.h.x, 0.05);
        let lod = log2(max(1.0 / (scale * ppp), 1e-3));
        let c = img_read(paper_tex, pt / scale, lod, true, vec2<f32>(0.0), vec2<f32>(0.0));
        let hgt = img_paper_height(c, u.img.x, u.img.y);
        paper_noise = hgt;
        tooth = smoothstep(0.2, 0.75, hgt);
        paper = row.e.xyz * img_paper_tint(c) * (1.0 - grain * 0.5 * (0.5 - hgt));
    } else {
        let fine = vnoise(pt * 0.9, 7u);
        let mid = vnoise(pt * 0.21, 11u);
        let broad = vnoise(pt * 0.03, 13u);
        paper_noise = 0.45 * fine + 0.35 * mid + 0.2 * broad;
        tooth = smoothstep(0.2, 0.75, fine * 0.7 + mid * 0.3);
        paper = row.e.xyz * (1.0 - grain * 0.12 * (1.0 - paper_noise));
    }
    var ink = 1.0 - light;
    ink *= mix(1.0, 0.35 + 0.65 * tooth, grain);

    // Coloured ground and lead by `color`
    let scene = shown(src.rgb);
    var ground = mix(paper, scene * mix(1.0, 0.85 + 0.3 * paper_noise, grain), row.d.z);
    // Drawn on (#879): the paper's wash floods in last, over the scene, in
    // an order three times broader and with a wide soft edge, so it reads
    // as a wash rather than patches
    if drawing {
        let wash_order = draw_order(p, order_scale * 3.0, order_seed + 7u);
        ground = mix(scene, ground, drawn(paper_front(front), wash_order, 0.8));
    }
    let lead = mix(row.f.xyz, row.f.xyz * 0.5 + scene * 0.35, row.d.z * 0.5);
    let out = mix(ground, lead, clamp(ink, 0.0, 1.0));
    let encoded = select(out, to_linear(out), u.srgb > 0.5);
    return vec4<f32>(encoded, src.a);
}
