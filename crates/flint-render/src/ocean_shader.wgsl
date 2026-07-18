// Flint ocean shader — Gerstner displacement + stylized cel-banded shading.
//
// The wave array is generated on the CPU (flint-core/src/ocean.rs) and
// uploaded verbatim; this shader only sums it. Keep the summation formulas
// IDENTICAL to WaveSpectrum::displacement()/normal() — CPU/GPU parity is
// what buoyancy relies on (tested by gpu_packing_matches_cpu_evaluation).
//
// Mesh: a normalized [-1,1]² grid, radially warped so the center is dense
// (sub-meter cells for displacement) and the rim reaches kilometers (flat,
// fog-eaten horizon). The whole grid follows the camera via grid_offset,
// snapped to inner-cell multiples so waves stay world-anchored.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Two vec4s per wave: a = [dir.x, dir.y, k, amp], b = [phase_t, q, omega_eff, 0].
// phase_t = (omega*t - phase0) mod 2pi, precomputed in f64 on the CPU.
// omega_eff = omega*speed_scale — the rate phase_t advances at, for the
// analytic surface velocity (contact foam).
struct Wave {
    a: vec4<f32>,
    b: vec4<f32>,
};

struct OceanUniforms {
    waves: array<Wave, 16>,
    deep_color: vec4<f32>,
    shallow_color: vec4<f32>,
    foam_color: vec4<f32>,
    sss_color: vec4<f32>,
    grid_offset: vec2<f32>,     // snapped camera XZ
    num_waves: u32,
    enable_tonemapping: u32,
    grid_scale: f32,            // inner metric scale of the warped grid
    fade_start: f32,            // displacement fade start (m from camera)
    fade_end: f32,              // fully flat beyond (m)
    foam_threshold: f32,        // foam where Jacobian < threshold
    foam_noise_scale: f32,      // world-space noise frequency for foam breakup
    ramp_steps: f32,            // cel bands in the diffuse ramp
    specular_strength: f32,
    time: f32,                  // seconds (foam drift only; waves use phase_t)
    absorption_color: vec3<f32>, // per-channel absorption (red dies first)
    turbidity: f32,             // cloudiness: 0 = glass-clear
    refraction_strength: f32,   // screen-space distortion amount
    camera_near: f32,
    camera_far: f32,
    grab_enabled: u32,          // 1 when scene color/depth copies are valid
    // Self-fog: the composite fog skips depth ≥ 0.9999 (to spare the sky),
    // which with GL-style depth is all water beyond ~700 m — it would render
    // as an unfogged dark band at the horizon. The ocean fades itself to the
    // scene fog color before that threshold instead.
    fog_color: vec4<f32>,
    // Analytic sky reflection: a snapshot of the procedural sky's gradient
    // (copied per frame from the `sky` component, so reflections track the
    // time of day for free). Deliberately no sun disc — the cel specular IS
    // the sun's reflection; including it here would double the sun.
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_haze: vec4<f32>,           // rgb + haze strength in alpha
    sky_reflection_strength: f32,  // 0 disables (also 0 when no sky component)
    // Contact foam: splash ring around the scene's ocean_contact hull.
    splash_strength: f32,          // overall gain; 0 disables (also 0 with no hull)
    splash_width: f32,             // max band width outward from the hull (m)
    splash_flicker_speed: f32,     // lapping-animation rate
    raft_a: vec4<f32>,             // [center.x, center.z, cos(yaw), sin(yaw)]
    raft_b: vec4<f32>,             // [half_x, half_z, baseline, noise_scale]
    raft_c: vec4<f32>,             // [hull_vel.xyz, response]
    // Cel band edge treatment (see cel_band + the ramp in fs_main).
    band_wobble: f32,              // ramp noise wobble amplitude (0 = off)
    band_dither: f32,              // halftone transition width, 0 = hard line
    band_dither_scale: f32,        // halftone dot grid frequency (dots/m)
    _pad_band: f32,
    foam_glow: vec4<f32>,          // bioluminescence: rgb + strength in .a
};

@group(1) @binding(0)
var<uniform> ocean: OceanUniforms;

// Bind group 2: lights + shadows (same layout as shader.wgsl / terrain)
struct DirectionalLight {
    direction: vec3<f32>,
    volumetric_intensity: f32,
    color: vec3<f32>,
    intensity: f32,
    volumetric_color: vec3<f32>,
    _pad1: f32,
};

struct PointLight {
    position: vec3<f32>,
    radius: f32,
    color: vec3<f32>,
    intensity: f32,
};

struct SpotLight {
    position: vec3<f32>,
    radius: f32,
    direction: vec3<f32>,
    inner_angle: f32,
    color: vec3<f32>,
    outer_angle: f32,
    intensity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

struct LightUniforms {
    directional_lights: array<DirectionalLight, 4>,
    point_lights: array<PointLight, 16>,
    spot_lights: array<SpotLight, 8>,
    directional_count: u32,
    point_count: u32,
    spot_count: u32,
    _pad: u32,
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
};

@group(2) @binding(0)
var<uniform> lights: LightUniforms;

@group(2) @binding(1)
var shadow_maps: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>,
};

@group(2) @binding(3)
var<uniform> shadow: ShadowUniforms;

// Grab pass (valid when grab_enabled == 1): the opaque scene rendered
// before the ocean, for refraction of whatever is underwater.
@group(3) @binding(0)
var grab_color: texture_2d<f32>;
@group(3) @binding(1)
var grab_depth: texture_2d<f32>;    // depth stored as R32Float

struct VertexInput {
    @location(0) pos: vec2<f32>,    // normalized grid coords in [-1, 1]
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,   // displaced surface position
    @location(1) param_xz: vec2<f32>,    // pre-displacement world column
    @location(2) fade: f32,              // displacement fade factor
};

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;

// ── Grid warp ───────────────────────────────────────────────────────────
// Chebyshev-radius exponential-ish warp: dense center, km-scale rim.
// The divisor floor puts the rim ~7.5 km out (grid_scale 60) — beyond the
// geometric horizon at seated eye height, so the water edge is never seen.
fn warp_grid(p: vec2<f32>) -> vec2<f32> {
    let r = max(abs(p.x), abs(p.y));
    return p * (ocean.grid_scale / (1.008 - r));
}

// ── Gerstner evaluation (MUST mirror flint-core ocean.rs) ───────────────

fn gerstner_offset(p: vec2<f32>, amp_scale: f32) -> vec3<f32> {
    var off = vec3<f32>(0.0);
    for (var i = 0u; i < ocean.num_waves; i = i + 1u) {
        let w = ocean.waves[i];
        let dir = w.a.xy;
        let k = w.a.z;
        let amp = w.a.w * amp_scale;
        let theta = k * dot(dir, p) - w.b.x;
        let s = sin(theta);
        let c = cos(theta);
        let q = w.b.y;
        off.x += q * amp * dir.x * c;
        off.y += amp * s;
        off.z += q * amp * dir.y * c;
    }
    return off;
}

// Analytic surface velocity at param point p: d/dt of gerstner_offset.
// theta advances at -omega_eff (w.b.z), so per wave
//   v.xz = q*amp*dir*omega*sin(theta),  v.y = -amp*omega*cos(theta).
// Derived motion only — surface SHAPE parity with flint-core is untouched.
fn gerstner_velocity(p: vec2<f32>, amp_scale: f32) -> vec3<f32> {
    var vel = vec3<f32>(0.0);
    for (var i = 0u; i < ocean.num_waves; i = i + 1u) {
        let w = ocean.waves[i];
        let dir = w.a.xy;
        let k = w.a.z;
        let amp = w.a.w * amp_scale;
        let theta = k * dot(dir, p) - w.b.x;
        let q = w.b.y;
        let omega = w.b.z;
        vel.x += q * amp * dir.x * omega * sin(theta);
        vel.y -= amp * omega * cos(theta);
        vel.z += q * amp * dir.y * omega * sin(theta);
    }
    return vel;
}

// Normal + horizontal-displacement Jacobian determinant at param point p.
// Jacobian < 1 means particles bunch (crest pinch) — that's where foam lives.
fn gerstner_normal_jacobian(p: vec2<f32>, amp_scale: f32) -> vec4<f32> {
    var n = vec3<f32>(0.0, 1.0, 0.0);
    var jxx = 1.0;
    var jzz = 1.0;
    var jxz = 0.0;
    for (var i = 0u; i < ocean.num_waves; i = i + 1u) {
        let w = ocean.waves[i];
        let dir = w.a.xy;
        let k = w.a.z;
        let amp = w.a.w * amp_scale;
        let theta = k * dot(dir, p) - w.b.x;
        let s = sin(theta);
        let c = cos(theta);
        let q = w.b.y;
        let ka = k * amp;
        n.x -= dir.x * ka * c;
        n.y -= q * ka * s;
        n.z -= dir.y * ka * c;
        let qkas = q * ka * s;
        jxx -= dir.x * dir.x * qkas;
        jzz -= dir.y * dir.y * qkas;
        jxz -= dir.x * dir.y * qkas;
    }
    let jacobian = jxx * jzz - jxz * jxz;
    return vec4<f32>(normalize(n), jacobian);
}

// ── Noise (foam breakup) ────────────────────────────────────────────────

// Sinless hash (Hoskins): GPU sin() is only accurate near the origin, and
// fbm octaves + time drift push lattice coords far past that — the old
// fract(sin(...)) hash degenerated into blocky lattice artifacts.
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

// ── Vertex ──────────────────────────────────────────────────────────────

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let flat_xz = warp_grid(in.pos) + ocean.grid_offset;
    let cam_dist = distance(flat_xz, transform.camera_pos.xz);
    let fade = 1.0 - smoothstep(ocean.fade_start, ocean.fade_end, cam_dist);

    let off = gerstner_offset(flat_xz, fade);
    let world_pos = vec3<f32>(flat_xz.x + off.x, off.y, flat_xz.y + off.z);

    out.clip_position = transform.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_pos = world_pos;
    out.param_xz = flat_xz;
    out.fade = fade;
    return out;
}

// ── Shadow sampling (same as terrain_shader.wgsl) ───────────────────────

fn shadow_factor(world_pos: vec3<f32>, view_depth: f32) -> f32 {
    let shadow_far = shadow.cascade_splits.z;
    let fade_start = shadow_far * 0.75;
    if (view_depth > shadow_far) {
        return 1.0;
    }
    let distance_fade = 1.0 - smoothstep(fade_start, shadow_far, view_depth);

    var cascade: i32 = 0;
    if (view_depth > shadow.cascade_splits.x) {
        cascade = 1;
    }
    if (view_depth > shadow.cascade_splits.y) {
        cascade = 2;
    }

    let light_space = shadow.cascade_view_proj[cascade] * vec4<f32>(world_pos, 1.0);
    let proj = light_space.xyz / light_space.w;
    let shadow_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);

    let edge_fade = min(
        min(smoothstep(0.0, 0.02, shadow_uv.x), smoothstep(0.0, 0.02, 1.0 - shadow_uv.x)),
        min(smoothstep(0.0, 0.02, shadow_uv.y), smoothstep(0.0, 0.02, 1.0 - shadow_uv.y))
    );
    if (edge_fade <= 0.0) {
        return 1.0;
    }

    let depth = proj.z;
    let texel_size = 1.0 / 2048.0;
    var shadow_sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            shadow_sum += textureSampleCompareLevel(
                shadow_maps, shadow_sampler, shadow_uv + offset, cascade, depth
            );
        }
    }

    let raw_shadow = shadow_sum / 9.0;
    return mix(1.0, raw_shadow, distance_fade * edge_fade);
}

// Invert the engine's GL-style projection: stored depth d ∈ [0,1] maps to
// view distance z = 2nf / ((f+n) − d·(f−n)).
fn linearize_depth(d: f32) -> f32 {
    let n = ocean.camera_near;
    let f = ocean.camera_far;
    return (2.0 * n * f) / max((f + n) - d * (f - n), 1e-4);
}

fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

// Quantize a 0..1 value into `steps` hard bands with fwidth anti-aliasing.
// AA width is clamped away from zero: fwidth() is 0 in flat regions, and a
// degenerate smoothstep(e, e, x) is 0/0 = NaN — one NaN pixel poisons the
// whole bloom mip chain into black rectangles.
fn cel_band(value: f32, steps: f32, p: vec2<f32>) -> f32 {
    let scaled = value * steps;
    let band = floor(scaled);
    let frac_part = scaled - band;
    let aa = clamp(fwidth(scaled) * 1.5, 1e-4, 0.5);
    var edge = smoothstep(0.5 - aa, 0.5 + aa, frac_part);

    // Halftone band edges (band_dither > 0): a 45°-rotated world-space dot
    // grid decides band membership inside a widened transition zone — dots
    // of the upper band grow and merge across it, dissolving the boundary
    // screen-print style while every dot edge stays hard (cel).
    if (ocean.band_dither > 0.001) {
        let zone = 0.5 * clamp(ocean.band_dither, 0.0, 1.0);
        let t = clamp((frac_part - (0.5 - zone)) / (2.0 * zone), 0.0, 1.0);
        let rp = vec2<f32>(p.x - p.y, p.x + p.y) * 0.7071
            * ocean.band_dither_scale;
        // Clustered-dot threshold: distance from the cell center, so dots
        // start as points (t≈0) and merge past circle-packing (t→1). The
        // 1.45 overshoot lets t=1 clear the corner distance (~1.34).
        let pat = length(fract(rp) - vec2<f32>(0.5)) * 1.9;
        let px = length(fwidth(rp));            // cell units per pixel
        let paa = clamp(px * 2.8, 1e-4, 0.6);
        let dotted = smoothstep(pat - paa, pat + paa, t * 1.45);
        // Fall back to the plain hard edge when dots go subpixel
        // (distance / grazing angles) — subpixel halftone reads as shimmer.
        let minify = smoothstep(0.25, 0.55, px);
        edge = mix(dotted, edge, minify);
    }
    return clamp((band + edge) / steps, 0.0, 1.0);
}

// ── Fragment ────────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let nj = gerstner_normal_jacobian(in.param_xz, in.fade);
    let N = nj.xyz;
    let jacobian = nj.w;

    let V = normalize(transform.camera_pos - in.world_pos);
    let view_depth = length(transform.camera_pos - in.world_pos);
    let ndv = max(dot(N, V), 0.0);
    // Schlick fresnel, F0 = 0.02 for water: ALL reflections — the sun's
    // specular included — are ~2% looking straight down and only get strong
    // at grazing angles. Skipping this on the specular painted a big
    // bloomed sun-disc on the water right under the camera.
    let fresnel = 0.02 + 0.98 * pow(clamp(1.0 - ndv, 0.0, 1.0), 5.0);

    // Primary light (the sun). Fall back to top-down white if scene has none.
    var L = vec3<f32>(0.0, 1.0, 0.0);
    var sun_radiance = vec3<f32>(1.0);
    if (lights.directional_count > 0u) {
        let sun = lights.directional_lights[0];
        L = normalize(sun.direction);
        sun_radiance = sun.color * sun.intensity;
    }

    // ── Cel-banded shade: wave shape + sun drive the deep→shallow ramp ──
    // Height rides the swell (troughs deep, crests light) so the bands read
    // as graphic wave shapes even with the sun overhead — the woodblock look.
    let ndl = max(dot(N, L), 0.0);
    let height01 = clamp(in.world_pos.y * 0.5 + 0.5, 0.0, 1.0);
    let shade = clamp(ndl * 0.55 + height01 * 0.65 - 0.10, 0.0, 1.0);
    // Woodblock wobble (band_wobble > 0): world-anchored noise raggedizes
    // the ramp boundaries. A razor band edge sweeping across a big smooth
    // swell reads as a hard "water level" contour; wobbling the field before
    // quantizing keeps the edge HARD (cel style) but makes it an organic
    // printed-ink line. Two scales: broad undulation (~11 m) bends the
    // contour, fine detail (~2 m) frays it.
    var shade_w = shade;
    if (ocean.band_wobble > 0.0001) {
        let wob = (fbm(in.param_xz * 0.09) - 0.44)
            + (fbm(in.param_xz * 0.50 + vec2<f32>(31.7, 7.9)) - 0.44) * 0.5;
        shade_w = clamp(shade + wob * ocean.band_wobble, 0.0, 1.0);
    }
    let ramp = cel_band(shade_w, max(ocean.ramp_steps, 1.0), in.param_xz);
    var water_color = mix(ocean.deep_color.rgb, ocean.shallow_color.rgb, ramp);

    // ── Fake subsurface: sun glowing through wave flanks facing us ─────
    // clamp: N.y can exceed 1.0 by a float ulp after normalize, and
    // pow(negative, 1.5) is NaN — one NaN poisons the bloom chain black.
    let rim = pow(clamp(1.0 - N.y, 0.0, 1.0), 1.5);
    let toward_sun = pow(max(dot(V, -L), 0.0), 2.0);
    water_color += ocean.sss_color.rgb * (rim * toward_sun * height01);

    // ── Foam: crest pinch (Jacobian) broken up by drifting noise ───────
    // Foam fades with distance: graphic detail near the raft, flat color
    // toward the horizon (matches the reference art and hides aliasing).
    let foam_distance_fade = 1.0 - smoothstep(60.0, 140.0, view_depth);
    let foam_noise = fbm(in.param_xz * ocean.foam_noise_scale
        + vec2<f32>(ocean.time * 0.35, ocean.time * -0.21));
    let foam_field = jacobian + (foam_noise - 0.5) * 0.30
        + (1.0 - foam_distance_fade) * 10.0;
    let foam_aa = clamp(fwidth(foam_field) * 1.5, 1e-4, 0.4);
    let foam_solid = 1.0 - smoothstep(ocean.foam_threshold - foam_aa,
                                      ocean.foam_threshold + foam_aa, foam_field);

    // Lace: a second, higher-frequency noise punches holes in the foam mass
    // (real foam is filigree, not slabs — see reference/water). Holes are
    // biggest at the mass edge and close up toward the crest pinch, so the
    // freshest churn stays solid while the skirt goes ragged. Edges stay
    // hard (cel style) — the breakup is topology, not softness.
    let lace = fbm(in.param_xz * ocean.foam_noise_scale * 6.0
        + vec2<f32>(ocean.time * -0.13, ocean.time * 0.19));
    let lace_aa = clamp(fwidth(lace) * 1.5, 1e-4, 0.4);
    let foam_depth = clamp((ocean.foam_threshold - foam_field) / 0.35, 0.0, 1.0);
    let hole_cut = mix(0.52, 0.30, foam_depth);
    let foam_hard = foam_solid
        * smoothstep(hole_cut - lace_aa, hole_cut + lace_aa, lace);

    // Flecks: past the mass edge the same noise is thresholded into sparse
    // full-strength dots that thin with distance from the crest — foam
    // trails off in scatter instead of the old soft gradient halo.
    let halo_band = 1.0 - smoothstep(ocean.foam_threshold,
                                     ocean.foam_threshold + 0.30, foam_field);
    let fleck_cut = mix(0.78, 0.55, halo_band);
    let foam_flecks = smoothstep(fleck_cut - lace_aa, fleck_cut + lace_aa, lace)
        * halo_band * (1.0 - foam_solid);

    // ── Contact foam: churn where water strikes the ocean_contact hull ──
    // Rounded-rect SDF in the hull's yawed local frame; churn is driven by
    // the water's orbital velocity relative to the hull, projected INTO the
    // nearest hull face — so a flat sea (or the lee side, where flow moves
    // away from the hull) stays quiet, and the windward face flares.
    var contact_foam = 0.0;
    if (ocean.splash_strength > 0.001) {
        let rel = in.world_pos.xz - ocean.raft_a.xy;
        let cy = ocean.raft_a.z;
        let sy = ocean.raft_a.w;
        // World → hull-local (matches raft.rhai: fwd = (sin,cos), right = (cos,-sin)).
        let local = vec2<f32>(rel.x * cy - rel.y * sy, rel.x * sy + rel.y * cy);
        // Corner radius ~ log radius so foam wraps the outer logs.
        let corner = 0.13;
        let qd = abs(local) - (ocean.raft_b.xy - vec2<f32>(corner));
        let sd = length(max(qd, vec2<f32>(0.0))) + min(max(qd.x, qd.y), 0.0) - corner;
        // Derivative BEFORE the divergent branch below.
        let sd_aa = clamp(fwidth(sd) * 1.5, 1e-4, 0.4);

        if (sd < ocean.splash_width * 2.0 + 0.3) {
            let rel_vel = gerstner_velocity(in.param_xz, in.fade) - ocean.raft_c.xyz;
            // Outward hull normal from the SDF gradient, hull-local frame.
            var g_local: vec2<f32>;
            if (sd > 0.0 && (qd.x > 0.0 || qd.y > 0.0)) {
                g_local = sign(local) * normalize(max(qd, vec2<f32>(0.0)));
            } else if (qd.x > qd.y) {
                g_local = vec2<f32>(sign(local.x), 0.0);
            } else {
                g_local = vec2<f32>(0.0, sign(local.y));
            }
            // sign() is 0 exactly on a hull axis — a zero vector would
            // normalize to NaN and poison the bloom chain.
            if (dot(g_local, g_local) < 1e-6) {
                g_local = vec2<f32>(0.0, 1.0);
            }
            // Hull-local → world (inverse of the rotation above).
            let n_out = normalize(vec2<f32>(g_local.x * cy + g_local.y * sy,
                                            -g_local.x * sy + g_local.y * cy));
            let impact = max(dot(vec2<f32>(rel_vel.x, rel_vel.z), -n_out), 0.0)
                + 0.6 * abs(rel_vel.y);
            // Crest gate: orbital flow is circular (downwind at crests,
            // upwind in troughs, equal speed), so without this the churn
            // would alternate sides evenly. Water riding UP the logs is
            // what actually churns — weight impact by surface height so
            // windward crest-strikes dominate and the lee stays quiet.
            let crest = clamp(0.5 + in.world_pos.y * 1.2, 0.25, 1.25);
            let churn = clamp(ocean.raft_b.z + impact * crest * ocean.raft_c.w, 0.0, 1.5);

            // Scalloped, lapping edge; churn drives both width and opacity.
            let lap = fbm(local * ocean.raft_b.w + vec2<f32>(
                ocean.time * 0.9 * ocean.splash_flicker_speed,
                ocean.time * -0.6 * ocean.splash_flicker_speed));
            let width = ocean.splash_width * churn * (0.55 + 0.9 * lap);
            let band = 1.0 - smoothstep(width - sd_aa, width + sd_aa, sd);
            contact_foam = band * clamp(ocean.splash_strength * churn, 0.0, 1.5);
            // Same lace as the crest foam breaks the ring into a dashed,
            // bubbly lap — a solid band reads as a glow decal at night.
            contact_foam *= 0.30 + 0.70 * smoothstep(0.38 - lace_aa,
                                                     0.38 + lace_aa, lace);
        }
    }

    // ── Cel specular glint (sun path sparkle) ───────────────────────────
    let H = normalize(V + L);
    let spec_raw = pow(max(dot(N, H), 0.0), 220.0) * ocean.specular_strength;
    let spec_aa = clamp(fwidth(spec_raw) * 1.5, 1e-4, 0.4);
    let spec = smoothstep(0.30 - spec_aa, 0.30 + spec_aa, spec_raw);

    // ── Shadows (raft silhouette on the water) ──────────────────────────
    let sf = shadow_factor(in.world_pos, view_depth);

    // Lighting composition. The water body color is treated as pre-lit
    // (stylized): tinted by the sun, dimmed in shadow, foam takes light fully.
    // Ambient lights the COMPOSED surface (water + foam) so moonlit nights
    // keep their foam instead of crushing it to black when the sun is out.
    let sun_tint = clamp(sun_radiance, vec3<f32>(0.0), vec3<f32>(1.25));
    let foam_mix = clamp(foam_hard + foam_flecks + contact_foam, 0.0, 1.0);
    let sky_weight = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(lights.ambient_ground.rgb, lights.ambient_sky.rgb, sky_weight);
    // Shadow dimming scales with actual sun light: with the sun down
    // (intensity ~0 at dusk/night) a caster must not darken the water —
    // a breaching whale was painting a 40 m twilight shadow streak.
    let sun_lum = clamp(max(sun_radiance.r, max(sun_radiance.g, sun_radiance.b)), 0.0, 1.0);
    let shadow_dim = mix(1.0 - 0.45 * sun_lum, 1.0, sf);

    // Foam ambient is pulled toward the sky's horizon glow so foam follows
    // the time-of-day palette. foam_color itself is constant white and the
    // raw hemisphere ambient is desaturated — together they rendered dusk
    // foam as daylight blue-gray slabs on a purple sea. Water keeps plain
    // ambient: its palette is already TOD-driven by time_of_day.rhai.
    var foam_ambient = ambient;
    if (ocean.sky_reflection_strength > 0.001) {
        let sky_glow = mix(ocean.sky_horizon.rgb, ocean.sky_haze.rgb,
                           ocean.sky_haze.a);
        foam_ambient = mix(ambient, sky_glow, 0.6);
    }

    // Styled opaque water body (no foam) — also the infinite-depth limit of
    // the transmission below, so high turbidity converges to it seamlessly.
    let water_body = water_color * sun_tint * shadow_dim + ambient * water_color * 0.5;

    let base = mix(water_color, ocean.foam_color.rgb, foam_mix);
    var color = base * sun_tint * shadow_dim;
    // Fresnel-weighted glint: full glitter path at grazing (sunset), nearly
    // nothing looking straight down (×3 so the path saturates early).
    color += sun_radiance * spec * sf * foam_distance_fade
        * clamp(fresnel * 3.0, 0.0, 1.0);
    color += mix(ambient * water_color, foam_ambient * ocean.foam_color.rgb,
                 foam_mix) * 0.5;

    // ── Refraction + turbidity (grab pass): see through to the legs ─────
    if (ocean.grab_enabled == 1u) {
        let dims = vec2<f32>(textureDimensions(grab_color));
        let pixel = in.clip_position.xy;
        // Distortion attenuates with distance so far water doesn't shimmer.
        let offset = N.xz * ocean.refraction_strength * dims.y * 0.25
            / max(view_depth, 1.0);
        var sample_px = clamp(pixel + offset, vec2<f32>(0.0), dims - 1.0);

        let water_lin = linearize_depth(in.clip_position.z);
        var scene_lin = linearize_depth(
            textureLoad(grab_depth, vec2<i32>(sample_px), 0).r);
        // Guard: if the refracted sample is CLOSER than the water surface it
        // belongs to something above the waterline (legs against sky) —
        // fall back to the undistorted column instead of smearing it.
        if (scene_lin < water_lin - 0.02) {
            sample_px = pixel;
            scene_lin = linearize_depth(
                textureLoad(grab_depth, vec2<i32>(sample_px), 0).r);
        }

        let column = max(scene_lin - water_lin, 0.0);
        // Beer-Lambert per-channel absorption; turbidity IS the cloudiness
        // slider (0 = glass, high = murk within half a meter).
        let absorb = exp(-max(ocean.turbidity, 0.0) * ocean.absorption_color * column);
        let scene_rgb = textureLoad(grab_color, vec2<i32>(sample_px), 0).rgb;
        let transmitted = scene_rgb * absorb + water_body * (vec3<f32>(1.0) - absorb);

        // Looking straight down = window into the water; grazing = mirror-ish
        // styled surface. Foam is always opaque.
        let see_through = pow(ndv, 1.5) * (1.0 - foam_mix) * foam_distance_fade * 0.9;
        color = mix(color, transmitted, see_through);
    }

    // ── Analytic sky reflection (Schlick fresnel, F0 = 0.02 for water) ──
    // Our sky is a function, not a cubemap: evaluate its gradient with the
    // reflected ray. Strongest at grazing angles, suppressed by foam.
    if (ocean.sky_reflection_strength > 0.001) {
        let R = reflect(-V, N);
        let r_up = pow(clamp(R.y, 0.0, 1.0), 0.62);
        var sky_ref = mix(ocean.sky_horizon.rgb, ocean.sky_zenith.rgb, r_up);
        let r_haze = exp(-abs(clamp(R.y, -1.0, 1.0)) * 9.0);
        sky_ref = mix(sky_ref, ocean.sky_haze.rgb, r_haze * ocean.sky_haze.a);

        // Gate by the shadow factor only as far as the SUN is lit: the sky
        // is an area source, so a caster blocking the sun must not erase
        // the sky sheen at dusk/night (the whale's shadow was drawing a
        // long dark streak across the twilight water).
        let reflect_sf = mix(1.0, sf, sun_lum);
        let reflect_amt = fresnel * ocean.sky_reflection_strength
            * (1.0 - foam_mix) * reflect_sf;
        color = mix(color, sky_ref, clamp(reflect_amt, 0.0, 1.0));
    }

    // ── Bioluminescent foam (foam_glow.a > 0, driven by biolum.rhai on
    // rare night bloom windows). Emissive, added AFTER reflection/
    // refraction mixing so nothing washes it out. Weighted hard toward
    // agitation — bioluminescence is STIMULATED light: the churned
    // contact wake flares, flecks spark, and the passive crest mass gets
    // almost nothing, so the raft's wake is unmistakably the source.
    if (ocean.foam_glow.a > 0.001) {
        let agitation = clamp(
            foam_hard * 0.08 + foam_flecks * 0.35 + contact_foam * 1.6,
            0.0, 1.6);
        // Superlinear response pushes CHURNED foam past the bloom
        // threshold (post HDR pipeline, threshold ~1.0): the stirred
        // wake HALOS like a light source, sparse flecks stay a quiet
        // sprinkle, and the passive crest mass barely registers — the
        // stimulation gradient is the read.
        let glow = ocean.foam_glow.a * agitation * (1.0 + agitation * 2.5);
        color += ocean.foam_glow.rgb * glow;
    }

    // ── Self-fog toward the horizon (see fog_color comment above) ───────
    let horizon_fade = smoothstep(380.0, 640.0, view_depth);
    color = mix(color, ocean.fog_color.rgb, horizon_fade * ocean.fog_color.a);

    if (ocean.enable_tonemapping == 1u) {
        color = aces_filmic(color);
    }

    return vec4<f32>(color, 1.0);
}
