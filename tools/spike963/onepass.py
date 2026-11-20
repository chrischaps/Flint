p = r"C:\Users\chris\dev\Flint\.claude\worktrees\signal-963-0276\target\spike963\shaders_onepass\kuwahara.wgsl"
t = open(p, encoding="utf-8", newline="").read()
nl = "\r\n" if "\r\n" in t else "\n"
t = t.replace(nl, "\n")
start = t.index("    // Per-sector accumulators")
end = t.index("    if (total_weight > 0.0) {")
body = '''    // SPIKE #963 aside: one walk over the window, each sample into its sector.
    var total_weight = 0.0;
    var total_color = vec3<f32>(0.0);
    let sector_angle = 2.0 * PI / f32(NUM_SECTORS);
    var mean: array<vec3<f32>, 8>;
    var mean_sq: array<vec3<f32>, 8>;
    var w_sum: array<f32, 8>;
    for (var y: i32 = -radius; y <= radius; y = y + 1) {
        for (var x: i32 = -radius; x <= radius; x = x + 1) {
            let fx = f32(x);
            let fy = f32(y);
            let rx = fx * cos_phi + fy * sin_phi;
            let ry = -fx * sin_phi + fy * cos_phi;
            let sx = rx;
            let sy = ry * aniso;
            let dist = sqrt(sx * sx + sy * sy);
            if (dist <= f32(radius)) {
                let sample_angle = atan2(sy, sx) + PI;
                let s = u32(floor(sample_angle / sector_angle)) % NUM_SECTORS;
                let norm_dist = dist / f32(radius);
                let w = pow(1.0 - norm_dist, params.hardness);
                let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
                let color = textureSampleLevel(hdr_texture, hdr_sampler, uv + offset, 0.0).rgb;
                mean[s] += color * w;
                mean_sq[s] += color * color * w;
                w_sum[s] += w;
            }
        }
    }
    for (var s: u32 = 0u; s < NUM_SECTORS; s = s + 1u) {
        if (w_sum[s] > 0.0) {
            let m = mean[s] / w_sum[s];
            let m2 = mean_sq[s] / w_sum[s];
            let variance = dot(m2 - m * m, vec3<f32>(1.0 / 3.0));
            let sector_weight = exp(-params.sharpness * max(variance, 0.0));
            total_color += m * sector_weight;
            total_weight += sector_weight;
        }
    }

'''
t = t[:start] + body + t[end:]
open(p, "w", encoding="utf-8", newline="").write(t.replace("\n", nl))
print("ok")
