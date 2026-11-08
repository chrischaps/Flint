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
//     double the way quick sketch lines do.
//   - paper: a tone with grain at three scales; graphite catches on the
//     paper's tooth, so strokes break up where the paper dips.
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
    let taper = smoothstep(0.0, 0.15, x) * smoothstep(1.0, 0.85, x);
    let width = row.a.y * (0.45 + 0.55 * taper) * sqrt(grow);
    let cov = 1.0 - smoothstep(width * 0.5 - aa, width * 0.5 + aa, abs(off));
    return cov * mix(0.5, 1.0, taper) * (0.7 + 0.3 * hs.y);
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
fn world_strokes(q: vec2<f32>, ang: f32, layer: u32, seed: u32, s: f32, k: i32, width: f32, wob: f32, grow: f32, aa: f32) -> f32 {
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
    let taper = smoothstep(0.0, 0.15, x) * smoothstep(1.0, 0.85, x);
    let w = width * (0.45 + 0.55 * taper) * sqrt(grow);
    let a = clamp(aa, 1e-6, spacing * 0.35);
    let cov = 1.0 - smoothstep(w * 0.5 - a, w * 0.5 + a, abs(off));
    return cov * mix(0.5, 1.0, taper) * (0.7 + 0.3 * hs.y);
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
    if direction == 2u && !on_world {
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
    let layers = u32(clamp(row.a.z, 0.0, 4.0) + 0.5);
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
                let c0 = world_strokes(q, ang, i, seed, s0, i32(octave), width, wob0, grow, aa_w);
                let c1 = world_strokes(q, ang, i, seed, s0 * 2.0, i32(octave) + 1, width, wob0 * 2.0, grow, aa_w);
                cov += wt * mix(c0, c1, fade);
            }
        } else {
            let c0 = strokes(p, a0 + offsets[i], i, seed, row, grow, aa);
            let c1 = strokes(p, a1 + offsets[i], i, seed, row, grow, aa);
            cov = mix(c0, c1, blend);
        }
        light *= 1.0 - cov * row.g.w * mix(0.6, 1.0, dark);
    }
    // Rubbed graphite under the strokes: soft tone for the darks
    light *= 1.0 - row.f.w * dark * dark;

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
    let pressure = 0.65 + 0.35 * vnoise(pt * freq * 2.0, seed + 505u);
    let contour = max(line1, line2 * row.d.x * 0.7) * pressure * row.b.z;
    light *= 1.0 - clamp(contour, 0.0, 1.0) * 0.92;

    // Paper: a tone with grain, and tooth the graphite skips over
    let grain = row.d.y;
    let fine = vnoise(pt * 0.9, 7u);
    let mid = vnoise(pt * 0.21, 11u);
    let broad = vnoise(pt * 0.03, 13u);
    let paper_noise = 0.45 * fine + 0.35 * mid + 0.2 * broad;
    let tooth = smoothstep(0.2, 0.75, fine * 0.7 + mid * 0.3);
    var ink = 1.0 - light;
    ink *= mix(1.0, 0.35 + 0.65 * tooth, grain);
    let paper = row.e.xyz * (1.0 - grain * 0.12 * (1.0 - paper_noise));

    // Coloured ground and lead by `color`
    let scene = shown(src.rgb);
    let ground = mix(paper, scene * mix(1.0, 0.85 + 0.3 * paper_noise, grain), row.d.z);
    let lead = mix(row.f.xyz, row.f.xyz * 0.5 + scene * 0.35, row.d.z * 0.5);
    let out = mix(ground, lead, clamp(ink, 0.0, 1.0));
    let encoded = select(out, to_linear(out), u.srgb > 0.5);
    return vec4<f32>(encoded, src.a);
}
