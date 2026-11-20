p = r"C:\Users\chris\dev\Flint\.claude\worktrees\signal-963-0276\target\spike963\shaders_f16\kuwahara.wgsl"
t = open(p, encoding="utf-8", newline="").read()
nl = "\r\n" if "\r\n" in t else "\n"
t = t.replace(nl, "\n")
t = "enable f16;\n// SPIKE #963: the sector loop in half precision.\n" + t
start = t.index("    // Per-sector accumulators")
end = t.index("    if (total_weight > 0.0) {")
body = '''    // Per-sector accumulators (f32: the blend of eight sectors)
    var total_weight = 0.0;
    var total_color = vec3<f32>(0.0);

    let sector_angle = f16(2.0 * PI / f32(NUM_SECTORS));
    let cph = f16(cos_phi);
    let sph = f16(sin_phi);
    let an = f16(aniso);
    let rad = f16(radius);
    let hard = f16(params.hardness);

    for (var s: u32 = 0u; s < NUM_SECTORS; s = s + 1u) {
        var mean = vec3<f16>(0.0h);
        var mean_sq = vec3<f16>(0.0h);
        var w_sum = 0.0h;

        for (var y: i32 = -radius; y <= radius; y = y + 1) {
            for (var x: i32 = -radius; x <= radius; x = x + 1) {
                let fx = f16(x);
                let fy = f16(y);
                let rx = fx * cph + fy * sph;
                let ry = -fx * sph + fy * cph;
                let sx = rx;
                let sy = ry * an;
                let dist = sqrt(sx * sx + sy * sy);
                if (dist <= rad) {
                    let sample_angle = atan2(sy, sx) + f16(PI);
                    let sector_idx = u32(floor(sample_angle / sector_angle)) % NUM_SECTORS;
                    if (sector_idx == s) {
                        let norm_dist = dist / rad;
                        let w = pow(1.0h - norm_dist, hard);
                        let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
                        let color = vec3<f16>(textureSampleLevel(hdr_texture, hdr_sampler, uv + offset, 0.0).rgb);
                        mean += color * w;
                        mean_sq += color * color * w;
                        w_sum += w;
                    }
                }
            }
        }

        if (w_sum > 0.0h) {
            let m = vec3<f32>(mean) / f32(w_sum);
            let m2 = vec3<f32>(mean_sq) / f32(w_sum);
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
