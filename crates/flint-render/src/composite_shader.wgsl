// Composite post-processing shader
//
// Fullscreen triangle that reads from the HDR scene buffer, applies
// radial blur, chromatic aberration, bloom, fog, exposure, ACES tonemapping,
// and vignette. Outputs LINEAR values — the sRGB render target handles
// gamma encoding automatically via hardware conversion.

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
    _pad: vec2<f32>,
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
    // Render mode: 0 none, 1 Matrix, 2 blood, 3 drunk, 4 Tron (reality
    // tears), 5 underwater (submerged-eye moments; not a tear — no mask).
    render_mode: u32,
    mode_mix: f32,
    mode_time: f32,
    _pad_mode: f32,
    // Per-mode meaning. Tears (1-4): x = bleed-mask scale, y = mask style
    // (0 fbm / 1 iris), z = rate, w = spare. Underwater (5): x = signed eye
    // depth in meters (+ = submerged; waterline Y = camera_pos.y + x),
    // y = sea energy/turbidity 0..1, z = daylight 0..1, w = biolum 0..1.
    mode_params: vec4<f32>,
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

// ACES filmic tone mapping curve
fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

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

// Compute fog factor from depth buffer
fn compute_fog(uv: vec2<f32>) -> f32 {
    let depth = textureSample(depth_texture, depth_sampler_nn, uv).r;
    // Skip skybox (depth at or near far plane)
    if (depth >= 0.9999) {
        return 0.0;
    }

    let world_pos = world_pos_from_depth(uv, depth);
    let dist = length(world_pos - params.camera_pos);

    // Exponential distance fog with start offset
    let dist_factor = max(dist - params.fog_start, 0.0);
    let dist_fog = 1.0 - exp(-params.fog_density * dist_factor);

    // Height fog (exponential falloff above fog_height_origin)
    var height_fog = 0.0;
    if (params.fog_height_enabled > 0.5) {
        let height_above = max(world_pos.y - params.fog_height_origin, 0.0);
        height_fog = exp(-params.fog_height_falloff * height_above);
        // Modulate by distance so nearby objects aren't fully fogged
        height_fog = height_fog * dist_fog;
    }

    return clamp(max(dist_fog, height_fog), 0.0, 1.0);
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

// ── Reality-tear helpers ────────────────────────────────────────────────
// Noise ported from ocean_shader.wgsl (no include mechanism): Hoskins-style
// sinless hash so large time-drifted coords don't degenerate.
fn hash2(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 3; i = i + 1) {
        v += a * value_noise(q);
        q = q * 2.13 + vec2<f32>(17.0, 9.2);
        a *= 0.5;
    }
    return v;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Where the other reality shows through: 1 = torn, 0 = normal view.
// Style 0 (mode_params.y < 0.5): drifting fbm patches whose coverage grows
// from scattered holes to full-screen as mode_mix rises — patch interiors
// are fully torn so the effect reads as reality tearing, not a crossfade.
// Style 1: radial iris from screen center with an fbm-wobbled rim.
fn tear_mask(uv: vec2<f32>) -> f32 {
    let m = params.mode_mix;
    let aspect = params.texel_size.y / params.texel_size.x; // width / height
    let scale = max(params.mode_params.x, 0.0001);
    let t = params.mode_time;
    if (params.mode_params.y < 0.5) {
        let p = uv * vec2<f32>(aspect, 1.0) * scale
            + vec2<f32>(t * 0.02, -t * 0.013);
        // 3-octave fbm lives in ~[0, 0.875] with mean ≈ 0.44: sweep the
        // threshold across that span so coverage tracks mode_mix, and pin
        // the top so mix = 1 is guaranteed fully torn.
        let th = 0.44 + 0.4 * (1.0 - 2.0 * m);
        let patches = smoothstep(th - 0.08, th + 0.08, fbm(p));
        return max(patches, smoothstep(0.85, 1.0, m));
    }
    let d = length((uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0));
    let dir = normalize(uv - vec2<f32>(0.5, 0.5) + vec2<f32>(0.0001, 0.0));
    let wobble = (fbm(dir * 3.0 + vec2<f32>(t * 0.1, 0.0)) - 0.5) * 0.1;
    let r0 = m * (0.5 * aspect + 0.65) + wobble;
    return smoothstep(r0, r0 - 0.15, d);
}

// Mode 1 — Matrix code: screen quantized into glyph cells, procedural
// dot-matrix glyphs lit by the scene's own luminance (foam crests read as
// bright code) with per-column falling rain heads.
fn matrix_code(frag_xy: vec2<f32>) -> vec3<f32> {
    let cell_px = vec2<f32>(8.0, 12.0);
    let cell = floor(frag_xy / cell_px);
    let cell_uv = fract(frag_xy / cell_px);

    // Scene luminance at the cell center, taken through the same exposure +
    // tonemap as the normal path so brightness matches what the eye saw.
    let center_uv = (cell + 0.5) * cell_px * params.texel_size;
    let hdr_c = textureSampleLevel(hdr_texture, hdr_sampler, center_uv, 0.0).rgb;
    let lum = luma(aces_filmic(hdr_c * params.exposure));

    // Glyph reroll: per-cell phase so the field flickers, not strobes.
    var rate = params.mode_params.z;
    if (rate < 0.01) { rate = 6.0; }
    let gid = floor(params.mode_time * rate + hash2(cell) * 8.0);

    // Luminance contrast curve: the cel ocean is uniformly bright, so a
    // linear map saturates — square it and let glyph DENSITY carry the
    // structure too (dark cells go sparse, foam crests go dense).
    let li = clamp(lum * lum * 1.4, 0.0, 1.0);

    // 3x5 live dots inside a 4x6 grid — the dead row/column separates glyphs.
    let g = floor(cell_uv * vec2<f32>(4.0, 6.0));
    let f = fract(cell_uv * vec2<f32>(4.0, 6.0));
    var dot_on = step(mix(0.78, 0.38, li),
        hash2(cell * 17.0 + g + vec2<f32>(gid * 31.0, gid * 7.0)));
    dot_on *= step(g.x, 2.5) * step(g.y, 4.5);
    let inset = step(0.15, f.x) * step(f.x, 0.85) * step(0.1, f.y) * step(f.y, 0.9);
    let dots = dot_on * inset;

    // Falling rain head per column: bright leader, exponential trail above.
    let rows = 1.0 / (params.texel_size.y * cell_px.y);
    let speed = 0.25 + 0.75 * hash2(vec2<f32>(cell.x, 7.0));
    let head_row = fract(params.mode_time * speed * 0.12 + hash2(vec2<f32>(cell.x, 13.0)))
        * (rows + 20.0) - 10.0;
    let behind = head_row - cell.y; // >0 above/behind the falling head
    var trail = 0.12;
    if (behind >= 0.0) {
        trail = 0.12 + 0.88 * exp(-0.2 * behind);
    }
    let head_glow = smoothstep(1.5, 0.0, abs(behind));

    let intensity = clamp(li * 1.1 + 0.05, 0.0, 1.0) * trail * dots;
    var mcol = mix(vec3<f32>(0.0, 0.025, 0.005), vec3<f32>(0.15, 1.0, 0.3), intensity);
    mcol += vec3<f32>(0.7, 1.0, 0.8) * head_glow * dots * 0.6;
    return mcol;
}

// Mode 2 — blood grade (sky and geometry side; the ocean recolors itself with full
// detail in ocean_shader.wgsl). Luminance-preserving crimson tint, weighted
// away from pixels that are ALREADY red so the ocean's pools and pale-pink
// foam keep their structure while the rest of the world sinks into key.
fn blood_grade(mapped: vec3<f32>, mask: f32) -> vec3<f32> {
    let l = luma(mapped);
    let tinted = mix(vec3<f32>(0.02, 0.0, 0.004),
        vec3<f32>(l * 1.45, l * 0.2, l * 0.24),
        smoothstep(0.0, 0.18, l));
    let redness = clamp((mapped.r - max(mapped.g, mapped.b)) * 3.0, 0.0, 1.0);
    return mix(mapped, tinted, 0.85 * (1.0 - redness) * mask);
}

// Linearize a conventional (0 = near, 1 = far) depth value.
fn linearize_depth(d: f32) -> f32 {
    return params.near * params.far / (params.far - d * (params.far - params.near));
}

// Mode 4 — Tron: scene dimmed to a ghost, depth/luminance edges glow cyan,
// and a world-space grid reconstructed from depth rides the wave surface.
fn tron_view(mapped: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let cyan = vec3<f32>(0.1, 0.9, 1.0);
    let depth_c = textureSampleLevel(depth_texture, depth_sampler_nn, uv, 0.0).r;
    var out_col = mapped * 0.08;

    if (depth_c >= 0.9999) {
        // Sky: keep only the dim ghost.
        return mapped * 0.05;
    }

    let px = params.texel_size;
    let lin_c = linearize_depth(depth_c);
    let lin_l = linearize_depth(textureSampleLevel(depth_texture, depth_sampler_nn, uv - vec2<f32>(px.x, 0.0), 0.0).r);
    let lin_r = linearize_depth(textureSampleLevel(depth_texture, depth_sampler_nn, uv + vec2<f32>(px.x, 0.0), 0.0).r);
    let lin_u = linearize_depth(textureSampleLevel(depth_texture, depth_sampler_nn, uv - vec2<f32>(0.0, px.y), 0.0).r);
    let lin_d = linearize_depth(textureSampleLevel(depth_texture, depth_sampler_nn, uv + vec2<f32>(0.0, px.y), 0.0).r);
    // Depth discontinuities, normalized by view distance (silhouettes).
    let d_edge = (abs(lin_r - lin_l) + abs(lin_d - lin_u)) / max(lin_c * 0.05, 0.05);

    // Luminance gradient assist picks up interior detail (foam lace, logs).
    let lum_l = luma(textureSampleLevel(hdr_texture, hdr_sampler, uv - vec2<f32>(px.x, 0.0), 0.0).rgb);
    let lum_r = luma(textureSampleLevel(hdr_texture, hdr_sampler, uv + vec2<f32>(px.x, 0.0), 0.0).rgb);
    let lum_u = luma(textureSampleLevel(hdr_texture, hdr_sampler, uv - vec2<f32>(0.0, px.y), 0.0).rgb);
    let lum_d = luma(textureSampleLevel(hdr_texture, hdr_sampler, uv + vec2<f32>(0.0, px.y), 0.0).rgb);
    let l_edge = (abs(lum_r - lum_l) + abs(lum_d - lum_u)) * 2.0;

    let edge = smoothstep(0.25, 0.9, d_edge + l_edge);

    // World-space grid that follows the displaced surface.
    let wp = world_pos_from_depth(uv, depth_c);
    let dist = length(wp - params.camera_pos);
    let fade = exp(-dist * 0.02);
    let g1 = abs(fract(wp.xz / 2.0) - 0.5) * 2.0;
    let g2 = abs(fract(wp.xz / 10.0) - 0.5) * 2.0;
    let line1 = max(smoothstep(0.93, 0.99, g1.x), smoothstep(0.93, 0.99, g1.y));
    let line2 = max(smoothstep(0.96, 0.995, g2.x), smoothstep(0.96, 0.995, g2.y));
    let pulse = 0.75 + 0.25 * sin(params.mode_time * 2.0 - dist * 0.15);
    let grid = (line1 * 0.35 + line2 * 0.7) * fade * pulse;

    out_col += cyan * (edge * 0.9 + grid * 0.5);
    return out_col;
}

// Mode 5 — underwater: the sub-second moments when a swell buries the eye.
// NOT a reality tear, so no tear_mask — the mask here is the WATERLINE,
// tested per pixel against the flat plane y = camera_pos.y + mode_params.x
// (the signed eye depth the driving script publishes). Cel grammar
// throughout: banded absorption instead of an exponential gradient,
// two-hard-step light shafts, and a flat white foam lip on the line.
fn underwater_view(mapped: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let mix_amt = params.mode_mix;
    let eye_depth = params.mode_params.x; // + = submerged
    let churn = clamp(params.mode_params.y, 0.0, 1.0);
    let daylight = clamp(params.mode_params.z, 0.0, 1.0);
    let biolum = clamp(params.mode_params.w, 0.0, 1.0);
    let aspect = params.texel_size.y / params.texel_size.x;

    // ── Waterline: near-plane world point vs the wobbled water plane. The
    // near plane is only ~0.15 m tall in world units, so wobble amplitudes
    // are millimeters — enough to break the ruler-straight edge.
    let nearw = world_pos_from_depth(uv, 0.0);
    let wobble = (value_noise(vec2<f32>(
        uv.x * aspect * 9.0 + params.mode_time * 1.3,
        params.mode_time * 0.7)) - 0.5) * (0.006 + 0.010 * churn);
    let delta = (params.camera_pos.y + eye_depth + wobble) - nearw.y;
    let under = smoothstep(-0.0015, 0.0015, delta);
    if (under <= 0.0) {
        return mapped;
    }

    // ── Water column length behind this pixel. Sky depth only survives the
    // ocean grid's far fade, so treat it as a full column of water.
    let depth = textureSampleLevel(depth_texture, depth_sampler_nn, uv, 0.0).r;
    var dist = 60.0;
    if (depth < 0.9999) {
        dist = length(world_pos_from_depth(uv, depth) - params.camera_pos);
    }

    // ── Banded absorption: 3 hard cel steps toward flat murk. Turbidity
    // (churn) and depth pull the bands closer; night kills the value.
    let absorb = 0.22 + 0.45 * churn + 0.15 * clamp(eye_depth, 0.0, 1.0);
    let t = 1.0 - exp(-dist * absorb);
    let ts = t * 3.0;
    let tq = clamp((floor(ts) + smoothstep(0.88, 0.97, fract(ts))) / 3.0, 0.0, 1.0);

    let light = 0.12 + 0.88 * daylight;
    let murk_near = mix(vec3<f32>(0.07, 0.30, 0.46), vec3<f32>(0.08, 0.30, 0.30), churn);
    let murk_far = mix(vec3<f32>(0.015, 0.10, 0.24), vec3<f32>(0.02, 0.10, 0.14), churn);

    let tint = mapped * vec3<f32>(0.45, 0.82, 1.05);
    var wcol = mix(tint, mix(murk_near, murk_far, tq) * light, tq);

    // ── Snell window: the hard-rimmed bright circle straight overhead.
    let vdir = normalize(nearw - params.camera_pos);
    let snell = smoothstep(0.62, 0.68, vdir.y);
    wcol += snell * daylight * vec3<f32>(0.18, 0.28, 0.30);

    // ── Light shafts: 1D noise over a slanted screen coordinate, quantized
    // to two hard steps, drifting slowly. Fade down-screen, drown in churn.
    let sc = uv.x * aspect * 2.2 - uv.y * 0.8 + params.mode_time * 0.06;
    let sn = value_noise(vec2<f32>(sc * 7.0, params.mode_time * 0.11));
    let shaft = (step(0.68, sn) * 0.5 + step(0.80, sn) * 0.5)
        * (1.0 - uv.y * 0.75) * daylight * (1.0 - 0.7 * churn);
    wcol += shaft * tq * vec3<f32>(0.05, 0.11, 0.13);

    // ── Bioluminescent specks (night foam glow carried underwater).
    if (biolum > 0.001) {
        let sp = uv * vec2<f32>(aspect, 1.0) * 42.0
            + vec2<f32>(params.mode_time * 0.15, params.mode_time * 0.35);
        let h = hash2(floor(sp));
        let dotd = length(fract(sp) - 0.5);
        let tw = 0.5 + 0.5 * sin(params.mode_time * 3.0 + h * 40.0);
        wcol += step(0.965, h) * smoothstep(0.22, 0.06, dotd) * tw
            * biolum * vec3<f32>(0.15, 0.9, 0.8) * 0.8;
    }

    // ── Soft edge darkening, kept local to the mode (the sticky vignette
    // knob belongs to reality mode 1 — never write it from here).
    let vd = length((uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0));
    wcol *= 1.0 - 0.4 * smoothstep(0.45, 0.95, vd);

    // ── Hard white foam lip riding the line itself.
    let lip = under * (1.0 - smoothstep(0.005, 0.009, delta));
    var outc = mix(mapped, clamp(wcol, vec3<f32>(0.0), vec3<f32>(1.5)), under * mix_amt);
    outc = mix(outc, vec3<f32>(0.94, 0.98, 1.0), lip * mix_amt);
    return clamp(outc, vec3<f32>(0.0), vec3<f32>(4.0));
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;

    // ── Hook A: drunk sway (mode 3) and underwater refraction sway (mode 5)
    // warp the UV every downstream sample uses, so the whole frame (blur,
    // CA, fog, bloom) moves together.
    if ((params.render_mode == 3u || params.render_mode == 5u)
        && params.mode_mix > 0.001) {
        let t = params.mode_time;
        var amp = 0.006 * params.mode_mix;
        if (params.render_mode == 5u) {
            // Only sway once genuinely submerged: a straddle frame keeps
            // its above-water half rock steady.
            amp = 0.004 * params.mode_mix
                * smoothstep(0.1, 0.3, params.mode_params.x);
        }
        uv += vec2<f32>(
            sin(t * 0.7 + uv.y * 3.0),
            cos(t * 0.9 + uv.x * 2.5) * 0.6
        ) * amp;
    }

    let center = vec2<f32>(0.5, 0.5);
    let dir_from_center = uv - center;
    let dist = length(dir_from_center);

    // ── Radial Blur (8-tap, center stays sharp, edges blur) ──
    var color: vec3<f32>;
    if (params.radial_blur > 0.001) {
        let blur_str = params.radial_blur * dist * 0.04;
        let blur_dir = normalize(dir_from_center + vec2<f32>(0.0001, 0.0001)) * blur_str;
        var acc = vec3<f32>(0.0);
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -3.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -2.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -1.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -0.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  0.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  1.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  2.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  3.5).rgb;
        let blurred = acc / 8.0;
        let blur_mix = smoothstep(0.1, 0.7, dist) * params.radial_blur;
        color = mix(textureSample(hdr_texture, hdr_sampler, uv).rgb, blurred, clamp(blur_mix, 0.0, 1.0));
    } else {
        color = textureSample(hdr_texture, hdr_sampler, uv).rgb;
    }

    // ── Reality tear: drunk double vision (ghost image drifts sideways) ──
    if (params.render_mode == 3u && params.mode_mix > 0.001) {
        let ghost_off = vec2<f32>(0.012 + 0.010 * sin(params.mode_time * 0.5), 0.0)
            * params.mode_mix;
        let ghost = textureSampleLevel(hdr_texture, hdr_sampler, uv + ghost_off, 0.0).rgb;
        color = mix(color, ghost, 0.35 * params.mode_mix);
    }

    // ── Chromatic Aberration (R/B offset radially, G stays) ──
    if (params.chromatic_aberration > 0.001) {
        let offset = dir_from_center * params.chromatic_aberration * 0.012;
        let r = textureSample(hdr_texture, hdr_sampler, uv + offset).r;
        let b = textureSample(hdr_texture, hdr_sampler, uv - offset).b;
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

    // ── Fog (applied in linear HDR space, before tonemapping) ──
    if (params.fog_enabled > 0.5) {
        let fog_factor = compute_fog(uv);
        color = mix(color, params.fog_color, fog_factor);
    }

    // ── Exposure → Tonemapping ──
    // Output stays LINEAR — the sRGB render target applies gamma encoding.
    color = color * params.exposure;
    var mapped = aces_filmic(color);

    // ── Hook B: stylization owns the display-referred color. After ACES so
    // phosphor greens / duotones aren't compressed; before vignette + dither
    // so those still apply on top (CRT cohesion). Underwater (5) is not a
    // tear — it masks by the per-pixel waterline, never tear_mask.
    if (params.render_mode == 5u && params.mode_mix > 0.001) {
        mapped = underwater_view(mapped, uv);
    } else if (params.render_mode != 0u && params.mode_mix > 0.001) {
        let mask = tear_mask(uv);
        if (params.render_mode == 1u) {
            mapped = mix(mapped, matrix_code(in.position.xy), mask);
        } else if (params.render_mode == 2u) {
            mapped = blood_grade(mapped, mask);
        } else if (params.render_mode == 4u) {
            mapped = mix(mapped, tron_view(mapped, uv), mask);
        }
    }

    // ── Vignette (linear-space attenuation) ──
    if (params.vignette_intensity > 0.0) {
        let vdist = dist * 1.41421356;
        let vignette = 1.0 - pow(vdist, params.vignette_smoothness) * params.vignette_intensity;
        mapped = mapped * max(vignette, 0.0);
    }

    // ── Ordered (Bayer) Dither ──
    if (params.dither_intensity > 0.0) {
        let dither = bayer8(in.position.xy) - 0.5;
        mapped = mapped + vec3<f32>(dither * params.dither_intensity);
        mapped = max(mapped, vec3<f32>(0.0));
    }

    return vec4<f32>(mapped, 1.0);
}
