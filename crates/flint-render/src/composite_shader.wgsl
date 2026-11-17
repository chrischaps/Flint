// Composite post-processing shader
//
// Fullscreen triangle that reads from the HDR scene buffer, applies
// radial blur, depth of field, chromatic aberration, bloom, fog, exposure,
// ACES tonemapping, desaturation, and vignette. Outputs LINEAR values — the
// sRGB render target handles gamma encoding automatically via hardware
// conversion.

struct PostProcessUniforms {
    exposure: f32,
    bloom_intensity: f32,
    bloom_threshold: f32,
    bloom_soft_threshold: f32,
    vignette_intensity: f32,
    vignette_smoothness: f32,
    texel_size: vec2<f32>,
    chromatic_aberration: f32,
    radial_blur: f32,
    desaturate: f32,
    // Film grain intensity (rides the former _pad slot; 0 = off)
    grain_intensity: f32,
    // Fog parameters
    fog_color: vec3<f32>,
    fog_density: f32,
    fog_start: f32,
    fog_end: f32,
    fog_height_falloff: f32,
    fog_height_origin: f32,
    camera_pos: vec3<f32>,
    fog_enabled: f32,
    near: f32,
    far: f32,
    fog_height_enabled: f32,
    dither_intensity: f32,
    inv_view_proj: mat4x4<f32>,
    // Depth of field (trailing row appended after the mat4 — see PostProcessUniforms)
    dof_strength: f32,
    dof_focus_distance: f32,
    dof_focus_range: f32,
    // Where dither is added (#503; rides the former _pad2): 0 = linear
    // light, 1 = Bayer in display levels, 2 = triangular noise in display
    // levels.
    dither_style: f32,
    // The post time film grain reads. The render modes that shared this
    // row are display styles since #746 (stylize/matrix.rs and the rest).
    post_time: f32,
    _pad_time0: f32,
    _pad_time1: f32,
    _pad_time2: f32,
    // Color grade (ADR 0050): pow(max(color * gain + lift, 0), 1/gamma),
    // applied right after ACES. grade_enabled is CPU-computed so the
    // neutral path is the original code verbatim.
    grade_lift: vec3<f32>,
    grade_enabled: f32,
    grade_gamma: vec3<f32>,
    _pad3: f32,
    grade_gain: vec3<f32>,
    _pad4: f32,
    // Aerial tint and height fog density (ADR 0101), two appended rows.
    // fog_tint_strength 0 = no tint; fog_tint_end is kept > fog_tint_start
    // on the CPU so smoothstep is defined.
    fog_tint_color: vec3<f32>,
    fog_tint_strength: f32,
    fog_tint_start: f32,
    fog_tint_end: f32,
    fog_height_density: f32,
    _pad5: f32,
    // Ground fog (ADR 0106), four appended rows: fog on the floors of
    // ground_fog_map, marched through the box from ground_fog_floor_base
    // to ground_fog_ceiling over origin..origin + size in X and Z.
    ground_fog_color: vec3<f32>,
    ground_fog_density: f32,
    ground_fog_origin: vec2<f32>,
    ground_fog_size: vec2<f32>,
    ground_fog_floor_base: f32,
    ground_fog_ceiling: f32,
    ground_fog_falloff: f32,
    ground_fog_steps: f32,
    ground_fog_texel: f32,
    ground_fog_max_lod: f32,
    ground_fog_enabled: f32,
    _pad6: f32,
};

@group(0) @binding(0)
var<uniform> params: PostProcessUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

@group(2) @binding(0)
var bloom_texture: texture_2d<f32>;
@group(2) @binding(1)
var bloom_sampler: sampler;

@group(3) @binding(0)
var ssao_texture: texture_2d<f32>;
@group(3) @binding(1)
var ssao_sampler: sampler;

@group(3) @binding(2)
var depth_texture: texture_2d<f32>;
@group(3) @binding(3)
var depth_sampler_nn: sampler;

@group(3) @binding(4)
var volumetric_texture: texture_2d<f32>;
@group(3) @binding(5)
var volumetric_sampler: sampler;
// The ground fog's map (ADR 0106): r = floor over ground_fog_floor_base,
// g = amount. Read through volumetric_sampler (linear, clamped, mipped).
@group(3) @binding(6)
var ground_fog_map: texture_2d<f32>;

// The fog veil (#513, `scene_renderer/fog_veil.rs`), read only where
// FOG_VEILED, which Rust prepends: the picture as it stood before the
// first translucent draw, and the veil (r = weight x (1 - depth),
// g = weight, a = the share of what's behind that still shows).
@group(3) @binding(7)
var fog_behind_texture: texture_2d<f32>;
@group(3) @binding(8)
var fog_veil_texture: texture_2d<f32>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_composite(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle: 3 vertices cover the whole screen
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    // Map clip space to UV: [-1,1] -> [0,1], flip Y
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// ACES filmic tone mapping: `aces_filmic`, tonemap.wgsl (#738), put in front

// Linear to sRGB gamma correction
fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2));
}

// Reconstruct world position from depth buffer UV + depth value
fn world_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let clip = vec4<f32>(uv * 2.0 - 1.0, depth, 1.0);
    // wgpu clip space has Y flipped relative to UV space
    let clip_y_flip = vec4<f32>(clip.x, -clip.y, clip.z, 1.0);
    let world = params.inv_view_proj * clip_y_flip;
    return world.xyz / world.w;
}

// Linearize a [0,1] depth-buffer value to world-space view depth.
// Matches the wgpu-convention projection since ADR 0055.
fn linear_depth(depth: f32) -> f32 {
    return params.near * params.far / (params.far - depth * (params.far - params.near));
}

// Sky test shared by every depth-reading path below. The sky and skybox
// never write depth, and every pass clears it to exactly 1.0, so a pixel is
// sky iff its depth is 1.0. Anything drawn inside the far plane is < 1.0
// (Depth32Float keeps ~6e-8 of precision there), so geometry is fogged,
// shaded and focused at any distance; a "near 1.0" cut would leave distant
// ground unfogged (ADR 0078).
fn is_sky_depth(depth: f32) -> bool {
    return depth >= 1.0;
}

// Circle-of-confusion at a UV: 0 = in focus, 1 = fully defocused.
// Sky (depth at the far plane) is treated as at the far plane so it
// defocuses naturally when the focus plane is near. textureSampleLevel —
// this is called from non-uniform control flow inside the DoF gather loop.
fn coc_at(uv: vec2<f32>) -> f32 {
    let depth = textureSampleLevel(depth_texture, depth_sampler_nn, uv, 0.0).r;
    var z = params.far;
    if (!is_sky_depth(depth)) {
        z = linear_depth(depth);
    }
    return clamp(abs(z - params.dof_focus_distance) / params.dof_focus_range, 0.0, 1.0)
        * params.dof_strength;
}

// Height fog's optical depth along the view ray to `world_pos`, `dist`
// away (ADR 0101). The density is fog_height_density at
// fog_height_origin and falls by e every 1/fog_height_falloff metres
// above it (rising below it), integrated exactly along the ray:
//   density_at_eye * dist * (1 - e^-x) / x,  x = falloff * (p.y - eye.y)
// Unlike the distance fog it isn't held off by fog_start: it's a layer of
// air, and lies round the camera too. The exponents are clamped so a
// camera or a ray deep under the origin stays finite (it's fully fogged
// there anyway).
fn height_fog_depth(world_pos: vec3<f32>, dist: f32) -> f32 {
    let k = max(params.fog_height_falloff, 1e-5);
    let eye = min(-k * (params.camera_pos.y - params.fog_height_origin), 30.0);
    let at_eye = params.fog_height_density * exp(eye);
    let x = max(k * (world_pos.y - params.camera_pos.y), -30.0);
    // The share of the eye's density the ray averages; 1 - x/2 near level.
    var share = 1.0 - 0.5 * x;
    if (abs(x) > 1e-3) {
        share = (1.0 - exp(-x)) / x;
    }
    return at_eye * dist * share;
}

// The optical depth a step of length `len` gathers through ground fog of
// `density` at its floor, falling by e every 1/k above it, from `ya` to `yb`
// over the floor; under the floor it holds at the floor's (ADR 0106).
// ground_fog::fog_step in Rust is this, line for line.
fn fog_step(density: f32, k: f32, ya: f32, yb: f32, len: f32) -> f32 {
    let lo0 = min(ya, yb);
    let hi = max(ya, yb);
    let span = hi - lo0;
    if (span < 1e-4) {
        return density * len * exp(-k * max(lo0, 0.0));
    }
    let under = clamp((0.0 - lo0) / span, 0.0, 1.0);
    let lo = max(lo0, 0.0);
    let x = k * (hi - lo);
    var share = 1.0 - 0.5 * x;
    if (x > 1e-3) {
        share = (1.0 - exp(-x)) / x;
    }
    return density * len * (under + (1.0 - under) * exp(-k * lo) * share);
}

// Where step i of n starts on a march from t0 to t1 along the ray: each
// step longer than the last in proportion to its distance from the eye
// (counted from a metre behind it), so the steps are as fine near the eye
// as far off they need not be, wherever the march begins.
// ground_fog::step_at in Rust is this.
fn fog_step_at(i: f32, n: f32, t0: f32, t1: f32) -> f32 {
    return (t0 + 1.0) * pow((t1 + 1.0) / (t0 + 1.0), i / n) - 1.0;
}

// The ground fog's map at world `p`: x = the floor (world Y), y = the
// amount. Read at the mip level whose texels span `spacing` metres of
// ground, so a long step sees the fog averaged over what it crosses.
fn ground_fog_at(p: vec3<f32>, spacing: f32) -> vec2<f32> {
    let map_uv = (p.xz - params.ground_fog_origin) / params.ground_fog_size;
    let lod = clamp(log2(max(spacing / params.ground_fog_texel, 1.0)), 0.0, params.ground_fog_max_lod);
    let texel = textureSampleLevel(ground_fog_map, volumetric_sampler, map_uv, lod).rg;
    return vec2<f32>(params.ground_fog_floor_base + texel.x, texel.y);
}

// Ground fog's optical depth along the view ray through `uv` (ADR 0106).
// The ray is clipped to the box the fog can lie in, and to what the depth
// buffer says it meets (the sky: nothing), then marched in steps that
// lengthen with distance. The map is read at each step's ends, so each
// read serves two steps, and along a step the height over the floor runs
// straight from one end's to the other's: on a slope the floor follows
// the ground, and the falloff's integral stays exact. The ends are moved
// along the ray by a hash of the pixel, so what's left of the steps reads
// as fine grain, not bands.
fn ground_fog_depth(uv: vec2<f32>, depth: f32, pixel: vec2<f32>) -> f32 {
    let eye = params.camera_pos;
    var dir: vec3<f32>;
    var reach = 1e9;
    if (is_sky_depth(depth)) {
        dir = normalize(world_pos_from_depth(uv, 1.0) - eye);
    } else {
        let to = world_pos_from_depth(uv, depth) - eye;
        reach = length(to);
        dir = to / max(reach, 1e-6);
    }
    let lo = vec3<f32>(params.ground_fog_origin.x, params.ground_fog_floor_base, params.ground_fog_origin.y);
    let hi = vec3<f32>(lo.x + params.ground_fog_size.x, params.ground_fog_ceiling, lo.z + params.ground_fog_size.y);
    // A zero component goes to a huge slope of the same sign
    let safe = select(dir, vec3<f32>(1e-8), abs(dir) < vec3<f32>(1e-8));
    let a = (lo - eye) / safe;
    let b = (hi - eye) / safe;
    let enter = min(a, b);
    let leave = max(a, b);
    let t0 = max(max(max(enter.x, enter.y), enter.z), 0.0);
    let t1 = min(min(min(leave.x, leave.y), leave.z), reach);
    if (t1 <= t0) {
        return 0.0;
    }
    let n = params.ground_fog_steps;
    let k = params.ground_fog_falloff;
    // The ground a step at distance t runs over: (t + 1) ln r of ray, r
    // the ratio of one step's distance to the last's
    let per_metre = length(dir.xz) * log((t1 + 1.0) / (t0 + 1.0)) / n;
    let q = vec2<u32>(pixel);
    var h = q.x * 1664525u + q.y * 22695477u;
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    let jitter = f32(h >> 8u) / 16777216.0;
    var ta = t0;
    var fa = ground_fog_at(eye + dir * ta, (ta + 1.0) * per_metre);
    var optical = 0.0;
    // n + 1 steps: t0, then the n ends moved by the jitter, then t1
    for (var i = 0.0; i <= n; i = i + 1.0) {
        var tb = t1;
        if (i < n) {
            tb = fog_step_at(i + jitter, n, t0, t1);
        }
        let fb = ground_fog_at(eye + dir * tb, (tb + 1.0) * per_metre);
        let amount = 0.5 * (fa.y + fb.y);
        if (amount > 0.0) {
            optical = optical + fog_step(
                params.ground_fog_density * amount,
                k,
                eye.y + dir.y * ta - fa.x,
                eye.y + dir.y * tb - fb.x,
                tb - ta,
            );
        }
        // Past e^-12 nothing behind shows
        if (optical > 12.0) {
            break;
        }
        ta = tb;
        fa = fb;
    }
    return optical;
}

// Fog at a UV from the depth buffer: x = the fog's share (0..1), y = the
// aerial tint's share (0..fog_tint_strength), both 0 for the sky.
fn compute_fog(uv: vec2<f32>) -> vec2<f32> {
    let depth = textureSample(depth_texture, depth_sampler_nn, uv).r;
    return fog_at(uv, depth);
}

// The same at a depth of the caller's: the fog veil's (#513).
fn fog_at(uv: vec2<f32>, depth: f32) -> vec2<f32> {
    // Skip the sky; everything drawn is fogged, however far
    if (is_sky_depth(depth)) {
        return vec2<f32>(0.0, 0.0);
    }

    let world_pos = world_pos_from_depth(uv, depth);
    let dist = length(world_pos - params.camera_pos);

    // Exponential distance fog with start offset
    let dist_factor = max(dist - params.fog_start, 0.0);
    var fog = 1.0 - exp(-params.fog_density * dist_factor);

    // Height fog adds its optical depth to the distance fog's (ADR 0101)
    if (params.fog_height_enabled > 0.5) {
        let optical = params.fog_density * dist_factor + height_fog_depth(world_pos, dist);
        fog = 1.0 - exp(-min(optical, 80.0));
    }

    // Aerial tint: far things take fog_tint_color over a distance ramp
    var tint = 0.0;
    if (params.fog_tint_strength > 0.0) {
        tint = params.fog_tint_strength
            * smoothstep(params.fog_tint_start, params.fog_tint_end, dist);
    }

    return vec2<f32>(clamp(fog, 0.0, 1.0), tint);
}

// The ground fog, aerial tint and fog over one layer of a pixel (#513):
// `c` is the layer's light, already weighted by `share`, the part of the
// pixel it holds, at `depth`; each fog takes its colour by that share. A
// pixel's two layers (the picture behind a translucent draw and the draw's
// own light) are fogged apart and added.
fn fog_layer(c_in: vec3<f32>, share: f32, uv: vec2<f32>, depth: f32, pixel: vec2<f32>) -> vec3<f32> {
    var c = c_in;
    if (params.ground_fog_enabled > 0.5) {
        let optical = ground_fog_depth(uv, depth, pixel);
        c = mix(c, params.ground_fog_color * share, 1.0 - exp(-optical));
    }
    if (params.fog_enabled > 0.5) {
        let fog = fog_at(uv, depth);
        if (fog.y > 0.0) {
            c = mix(c, params.fog_tint_color * share, fog.y);
        }
        c = mix(c, params.fog_color * share, fog.x);
    }
    return c;
}

// 8x8 Bayer ordered dither matrix — returns value in [0, 1)
fn bayer8(pos: vec2<f32>) -> f32 {
    let x = u32(pos.x) % 8u;
    let y = u32(pos.y) % 8u;

    // Build recursively from 2x2 base
    var value = 0u;
    var xm = x;
    var ym = y;

    // Bit 5-4 (from 4x4 → 8x8)
    value = value + ((((xm ^ ym) & 4u) >> 1u) | ((ym & 4u) >> 2u));
    // Bit 3-2 (from 2x2 → 4x4)
    value = value * 4u + ((((xm ^ ym) & 2u)) | ((ym & 2u) >> 1u));
    // Bit 1-0 (base 2x2)
    value = value * 4u + ((((xm ^ ym) & 1u) << 1u) | (ym & 1u));

    return f32(value) / 64.0;
}

// The exact sRGB curves (IEC 61966-2-1), the ones the target's encode uses,
// so a value dithered in display levels comes back out where it was put.
// Not linear_to_srgb above, which is a 2.2 power.
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let lo = x / 12.92;
    let hi = pow((x + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, x <= vec3<f32>(0.04045));
}

// Roberts' R2 sequence over the pixel grid, in [0, 1): a quasi-random mask
// with blue-noise-like spacing and no 8x8 tile to read.
fn r2_dither(pos: vec2<f32>) -> f32 {
    let p = floor(pos);
    return fract(0.5 + p.x * 0.75487766624669 + p.y * 0.56984029099805);
}

// A uniform [0, 1) value to a triangular one in (-1, 1), through the
// triangle's inverse CDF, so the mask keeps its spacing.
fn tpdf(u: f32) -> f32 {
    let r = u * 2.0 - 1.0;
    return sign(r) * (1.0 - sqrt(1.0 - abs(r)));
}

// hash2 (film grain's) lives in tear_mask.wgsl, which Rust prepends to
// this file (`postprocess::COMPOSITE_WGSL`). The tear itself is drawn by
// the styles that follow it and the style mask, not here (#746).

// What the lens makes of the HDR picture at `uv`: the radial blur, then
// depth of field. Chromatic aberration reads red and blue through this at
// its offsets, so all three channels go through the same blurs; it read
// them from the sharp source once, and wherever DoF or the radial blur
// spread a bright surface, green spread with it and red and blue didn't
// (#1156: emissive edges ringed green). textureSampleLevel throughout: the
// HDR target has one mip, so it reads what textureSample did, and the
// gathers are free to run from non-uniform control flow.
fn lens(uv: vec2<f32>) -> vec3<f32> {
    let dir_from_center = uv - vec2<f32>(0.5, 0.5);
    let dist = length(dir_from_center);

    // ── Radial Blur (8-tap, center stays sharp, edges blur) ──
    var color: vec3<f32>;
    if (params.radial_blur > 0.001) {
        let blur_str = params.radial_blur * dist * 0.04;
        let blur_dir = normalize(dir_from_center + vec2<f32>(0.0001, 0.0001)) * blur_str;
        var acc = vec3<f32>(0.0);
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir * -3.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir * -2.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir * -1.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir * -0.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir *  0.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir *  1.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir *  2.5, 0.0).rgb;
        acc += textureSampleLevel(hdr_texture, hdr_sampler, uv + blur_dir *  3.5, 0.0).rgb;
        let blurred = acc / 8.0;
        let blur_mix = smoothstep(0.1, 0.7, dist) * params.radial_blur;
        color = mix(textureSampleLevel(hdr_texture, hdr_sampler, uv, 0.0).rgb, blurred, clamp(blur_mix, 0.0, 1.0));
    } else {
        color = textureSampleLevel(hdr_texture, hdr_sampler, uv, 0.0).rgb;
    }

    // ── Depth of Field (single-pass CoC-weighted disc gather) ──
    // Gathers from the raw HDR source (same as radial blur) so the two blurs
    // stay order-stable; taps are weighted by their own CoC as a cheap guard
    // against in-focus foreground bleeding into defocused regions.
    if (params.dof_strength > 0.001) {
        let center_coc = coc_at(uv);
        let radius_px = center_coc * 10.0;
        if (radius_px > 0.5) {
            var acc = textureSampleLevel(hdr_texture, hdr_sampler, uv, 0.0).rgb;
            var wsum = 1.0;
            for (var i = 0u; i < 12u; i = i + 1u) {
                let ang = f32(i) * 2.39996323; // golden angle
                let r = radius_px * sqrt((f32(i) + 0.5) / 12.0);
                let tuv = uv + vec2<f32>(cos(ang), sin(ang)) * r * params.texel_size;
                let w = coc_at(tuv);
                acc = acc + textureSampleLevel(hdr_texture, hdr_sampler, tuv, 0.0).rgb * w;
                wsum = wsum + w;
            }
            color = mix(color, acc / wsum, center_coc);
        }
    }
    return color;
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;

    let center = vec2<f32>(0.5, 0.5);
    let dir_from_center = uv - center;
    let dist = length(dir_from_center);

    var color = lens(uv);

    // ── Chromatic Aberration (R/B offset radially, G stays) ──
    // Red and blue through the same lens as green (see `lens`). With no
    // blur on, lens() is one plain read, so this costs what it always did;
    // under DoF it is the gather three times, while both are on.
    if (params.chromatic_aberration > 0.001) {
        let offset = dir_from_center * params.chromatic_aberration * 0.012;
        let r = lens(uv + offset).r;
        let b = lens(uv - offset).b;
        color = vec3<f32>(r, color.g, b);
    }

    // ── SSAO ──
    let ao = textureSample(ssao_texture, ssao_sampler, uv).r;
    color = color * ao;

    // ── Volumetric (God Rays) — additive blend before bloom ──
    let vol = textureSample(volumetric_texture, volumetric_sampler, uv).rgb;
    color = color + vol;

    // ── Bloom ──
    let bloom = textureSample(bloom_texture, bloom_sampler, uv).rgb;
    color = color + bloom * params.bloom_intensity;

    // ── The fog veil (#513): where a translucent draw lies, the pixel is
    // two layers, fogged apart. What still shows of the picture behind it
    // (`shows` x that picture) at the scene's depth, as any pixel is; the
    // draw's own light (the rest) at the veil's depth, so an echo in front
    // of a horizon is one colour across it. Everywhere else, the fog below,
    // which runs everywhere so its texture reads stay in uniform control
    // flow; the veiled pixels take this instead after it.
    var veiled = false;
    var veiled_color = vec3<f32>(0.0);
    if (FOG_VEILED) {
        let veil = textureSampleLevel(fog_veil_texture, depth_sampler_nn, uv, 0.0);
        let behind = textureSampleLevel(fog_behind_texture, hdr_sampler, uv, 0.0).rgb;
        let drawn = textureSampleLevel(hdr_texture, hdr_sampler, uv, 0.0).rgb;
        let depth = textureSampleLevel(depth_texture, depth_sampler_nn, uv, 0.0).r;
        if (veil.g > 1e-4 && (params.fog_enabled > 0.5 || params.ground_fog_enabled > 0.5)) {
            veiled = true;
            let shows = clamp(veil.a, 0.0, 1.0);
            let own = drawn - shows * behind;
            let veil_depth = clamp(1.0 - veil.r / veil.g, 0.0, 1.0);
            veiled_color = fog_layer(color - own, shows, uv, depth, in.position.xy)
                + fog_layer(own, 1.0 - shows, uv, veil_depth, in.position.xy);
        }
    }

    // ── Ground fog (ADR 0106): a mist on floors of its own, under the
    // aerial tint and the fog, which take it with what lies behind it ──
    if (params.ground_fog_enabled > 0.5) {
        let depth = textureSample(depth_texture, depth_sampler_nn, uv).r;
        let optical = ground_fog_depth(uv, depth, in.position.xy);
        color = mix(color, params.ground_fog_color, 1.0 - exp(-optical));
    }

    // ── Fog (applied in linear HDR space, before tonemapping) ──
    // The aerial tint lies under the fog, so the farthest things still run
    // into fog_color, and with it the sky's horizon (ADR 0101).
    if (params.fog_enabled > 0.5) {
        let fog = compute_fog(uv);
        if (fog.y > 0.0) {
            color = mix(color, params.fog_tint_color, fog.y);
        }
        color = mix(color, params.fog_color, fog.x);
    }
    if (veiled) {
        color = veiled_color;
    }

    // ── Exposure → Tonemapping ──
    // Output stays LINEAR — the sRGB render target applies gamma encoding.
    color = color * params.exposure;
    var mapped = aces_filmic(color);

    // ── Color grade (ADR 0050): ASC-CDL-shaped lift/gamma/gain, right after
    // tonemapping so it grades the display-referred image. Gated by the
    // CPU-computed enable flag — never trust pow(x, 1.0) float identity.
    if (params.grade_enabled > 0.5) {
        let graded = max(mapped * params.grade_gain + params.grade_lift, vec3<f32>(0.0));
        mapped = pow(graded, vec3<f32>(1.0) / max(params.grade_gamma, vec3<f32>(0.01)));
    }

    // ── Desaturation (mix toward darkened ash-grey, never neutral) ──
    // Rec.601 luma and the 0.62 grey target match the ladder stage of the
    // play-chart window harness so the disintegration language reads the same.
    if (params.desaturate > 0.0) {
        let luma = dot(mapped, vec3<f32>(0.299, 0.587, 0.114));
        mapped = mix(mapped, vec3<f32>(luma * 0.62), clamp(params.desaturate, 0.0, 1.0));
    }

    // ── Vignette (linear-space attenuation) ──
    if (params.vignette_intensity > 0.0) {
        let vdist = dist * 1.41421356;
        let vignette = 1.0 - pow(vdist, params.vignette_smoothness) * params.vignette_intensity;
        mapped = mapped * max(vignette, 0.0);
    }

    // ── Film grain (ADR 0050): hash noise on pixel coords, time-quantized
    // to 24 Hz (film cadence; the Tier-3 "on twos" tie-in is this one
    // constant), luma-weighted so highlights stay clean. Before dither —
    // grain is signal, dither is a quantization aid and stays last. Uses
    // the shared post time, which headless render fixes at 0 so PNG gates
    // stay deterministic.
    if (params.grain_intensity > 0.0) {
        let grain_frame = floor(params.post_time * 24.0);
        let n = hash2(in.position.xy + vec2<f32>(grain_frame * 17.0, grain_frame * 61.0)) - 0.5;
        let g_luma = dot(mapped, vec3<f32>(0.299, 0.587, 0.114));
        let weight = 0.3 + 0.7 * (1.0 - saturate(g_luma));
        mapped = mapped + vec3<f32>(n * params.grain_intensity * weight);
        mapped = max(mapped, vec3<f32>(0.0));
    }

    // ── Dither (#503) ──
    // Style 0 adds Bayer in linear light, before the target's sRGB encode,
    // so its size on screen follows the encode's slope: about 13x one level
    // near black, well under one in the highlights. Kept as it was, a look
    // of its own. Styles 1 and 2 add it in display levels, where the 8-bit
    // quantization happens, so dither_intensity is the same number of
    // levels at every brightness (1/255 = one step) and then decode again
    // for the hardware to encode.
    if (params.dither_intensity > 0.0) {
        if (params.dither_style < 0.5) {
            let dither = bayer8(in.position.xy) - 0.5;
            mapped = mapped + vec3<f32>(dither * params.dither_intensity);
            mapped = max(mapped, vec3<f32>(0.0));
        } else {
            var d: f32;
            if (params.dither_style < 1.5) {
                // [-0.5, 0.5): one step of intensity is ±half a step
                d = bayer8(in.position.xy) - 0.5;
            } else {
                // Triangular in (-1, 1): one step of intensity is ±1 step,
                // which leaves no banding and no error tied to the signal
                d = tpdf(r2_dither(in.position.xy));
            }
            let encoded = srgb_encode(mapped) + vec3<f32>(d * params.dither_intensity);
            mapped = srgb_decode(max(encoded, vec3<f32>(0.0)));
        }
    }

    return vec4<f32>(mapped, 1.0);
}
