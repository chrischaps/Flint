// Procedural sky shader — gradient + sun disc + hash stars + FBM clouds.
//
// Replaces the static equirect skybox when a `sky` component is present.
// Everything is driven by a SkyUniforms block that a time-of-day controller
// (game script) interpolates each frame; there are no discrete sky states,
// so dawn blends into noon into dusk into starfield with no pops.
//
// The sun disc direction/color are copied CPU-side from the scene's first
// directional light — the drawn sun and the lighting can never disagree.

struct SkyUniforms {
    inv_view_proj: mat4x4<f32>,     // rotation-only inverse view-projection
    sun_dir: vec3<f32>,             // TOWARD the sun (light convention)
    sun_disc_size: f32,             // angular radius in radians
    sun_color: vec4<f32>,           // rgb * intensity (linear HDR)
    zenith_color: vec4<f32>,
    horizon_color: vec4<f32>,
    haze_color: vec4<f32>,          // thin band hugging the horizon
    cloud_tint: vec4<f32>,
    sun_glow: f32,                  // glow strength around the disc
    star_opacity: f32,              // 0 day … 1 night
    cloud_coverage: f32,            // 0 clear … 1 overcast
    cloud_density: f32,             // edge softness / thickness
    cloud_scale: f32,               // world-ish frequency of cloud noise
    cloud_drift_x: f32,
    cloud_drift_y: f32,
    time: f32,
};

@group(0) @binding(0) var<uniform> sky: SkyUniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ray_dir: vec3<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    let clip = vec4<f32>(x, y, 1.0, 1.0);
    let world = sky.inv_view_proj * clip;

    var out: VsOut;
    out.position = vec4<f32>(x, y, 1.0, 1.0);   // far plane
    out.ray_dir = world.xyz / world.w;
    return out;
}

// ── Noise ───────────────────────────────────────────────────────────────

// Sinless hashes (Hoskins): GPU sin() is only accurate near the origin, and
// fbm octaves + cloud drift push lattice coords far past that — the old
// fract(sin(...)) hash degenerated into blocky lattice artifacts.
fn hash2(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn hash3(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y + p3.z) * p3.z);
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
    for (var i = 0; i < 4; i = i + 1) {
        v += a * value_noise(q);
        q = q * 2.11 + vec2<f32>(13.7, 5.3);
        a *= 0.5;
    }
    return v;
}

// ── Fragment ────────────────────────────────────────────────────────────

@fragment
fn fs_sky(in: VsOut) -> @location(0) vec4<f32> {
    let dir = normalize(in.ray_dir);
    let up = clamp(dir.y, -1.0, 1.0);

    // ── Gradient: horizon → zenith, with a haze band at the waterline ──
    let zen_t = pow(clamp(up, 0.0, 1.0), 0.62);
    var color = mix(sky.horizon_color.rgb, sky.zenith_color.rgb, zen_t);
    let haze = exp(-abs(up) * 9.0);
    color = mix(color, sky.haze_color.rgb, haze * sky.haze_color.a);

    // ── Stars: hash-cell points, twinkling, faded near the horizon ─────
    if (sky.star_opacity > 0.001 && up > 0.0) {
        let grid = 90.0;
        let cell = floor(dir * grid);
        let h = hash3(cell);
        if (h > 0.982) {
            // Star position within the cell; distance to the ray.
            let star_pos = (cell + vec3<f32>(
                hash3(cell + 1.7), hash3(cell + 4.2), hash3(cell + 9.1))) / grid;
            let d = length(normalize(star_pos) - dir);
            let core = 1.0 - smoothstep(0.0, 0.0035, d);
            let twinkle = 0.7 + 0.3 * sin(sky.time * 2.5 + h * 40.0);
            let horizon_fade = smoothstep(0.02, 0.18, up);
            let brightness = (0.4 + 0.6 * fract(h * 91.7));
            color += vec3<f32>(1.0, 0.98, 0.92)
                * (core * twinkle * brightness * sky.star_opacity * horizon_fade);
        }
    }

    // ── Sun disc + glow ─────────────────────────────────────────────────
    let sun_d = normalize(sky.sun_dir);
    let cos_ang = clamp(dot(dir, sun_d), -1.0, 1.0);
    let ang = acos(cos_ang);
    let disc = 1.0 - smoothstep(sky.sun_disc_size * 0.72, sky.sun_disc_size, ang);
    let glow_tight = pow(clamp(cos_ang, 0.0, 1.0), 160.0);
    let glow_wide = pow(clamp(cos_ang, 0.0, 1.0), 5.0);
    var sun = sky.sun_color.rgb * (disc * 1.6 + (glow_tight * 0.9 + glow_wide * 0.10) * sky.sun_glow);

    // ── Clouds: FBM on the projected sky plane, occluding the sun ──────
    var cloud_alpha = 0.0;
    if (sky.cloud_coverage > 0.002 && up > 0.015) {
        // Perspective foreshortening toward the horizon for free.
        let puv = dir.xz / (dir.y + 0.18) * sky.cloud_scale
            + vec2<f32>(sky.cloud_drift_x, sky.cloud_drift_y) * sky.time;
        let n = fbm(puv);
        let threshold = 1.0 - sky.cloud_coverage;
        let edge = max(sky.cloud_density, 0.02);
        cloud_alpha = smoothstep(threshold, threshold + edge, n)
            * smoothstep(0.015, 0.12, up);    // fade at the horizon line
    }

    // Sun behind clouds; clouds pick up a touch of sun glow on their tint.
    color += sun * (1.0 - cloud_alpha * 0.92);
    let lit_cloud = sky.cloud_tint.rgb + sky.sun_color.rgb * glow_wide * 0.12;
    color = mix(color, lit_cloud, cloud_alpha * sky.cloud_tint.a);

    return vec4<f32>(color, 1.0);
}
