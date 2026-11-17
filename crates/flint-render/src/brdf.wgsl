// The one Cook-Torrance BRDF (metallic-roughness) every lit surface shades
// with (#738): meshes, skinned meshes and terrain. Needs `light_bindings.wgsl`
// (the sheen tint rides `lights`).

const PI: f32 = 3.14159265359;

// GGX/Trowbridge-Reitz Normal Distribution Function
fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let denom_term = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom_term * denom_term);
}

// Smith-Schlick Geometry function (single direction)
fn geometry_schlick(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

// Smith's method combining both view and light directions
fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick(n_dot_v, roughness) * geometry_schlick(n_dot_l, roughness);
}

// Schlick Fresnel approximation
fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

// Smooth distance attenuation with radius falloff
fn attenuation(distance: f32, radius: f32) -> f32 {
    let d2 = distance * distance;
    let r2 = radius * radius;
    let factor = d2 / r2;
    let falloff = saturate(1.0 - factor * factor);
    return falloff * falloff / max(d2, 0.0001);
}

// Spotlight cone falloff. `light_to_frag` runs from the light to the surface:
// the way the spot shines (#500 was two callers passing it backwards).
fn spot_cone_factor(light_to_frag: vec3<f32>, spot_dir: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    let cos_angle = dot(normalize(light_to_frag), normalize(spot_dir));
    return saturate((cos_angle - cos_outer) / max(cos_inner - cos_outer, 0.0001));
}

// Evaluate a single light using Cook-Torrance BRDF.
// `wrap` softens only the diffuse terminator (wrap-diffuse, a cheap
// subsurface-ish cue for matte materials); specular keeps the true n·l.
// `oren` blends the diffuse magnitude from Lambert toward the Fujii
// qualitative Oren-Nayar approximation (sigma = material roughness) —
// a flatter, chalkier falloff for rough matte surfaces. The two levers
// are orthogonal: wrap replaces the diffuse n·l, Oren-Nayar scales the
// diffuse magnitude computed from the raw geometric angles.
// wrap = 0 and oren = 0 take the original code path exactly.
fn evaluate_light(
    N: vec3<f32>,
    V: vec3<f32>,
    L: vec3<f32>,
    radiance: vec3<f32>,
    albedo: vec3<f32>,
    f0: vec3<f32>,
    metallic: f32,
    roughness: f32,
    n_dot_v: f32,
    wrap: f32,
    oren: f32,
) -> vec3<f32> {
    let H = normalize(V + L);

    let n_dot_l = max(dot(N, L), 0.0);
    let n_dot_h = max(dot(N, H), 0.0);
    let h_dot_v = max(dot(H, V), 0.0);

    let D = distribution_ggx(n_dot_h, roughness);
    let G = geometry_smith(n_dot_v, n_dot_l, roughness);
    let F = fresnel_schlick(h_dot_v, f0);

    let numerator = D * G * F;
    let denominator = 4.0 * n_dot_v * n_dot_l + 0.0001;
    let specular = numerator / denominator;

    let kS = F;
    let kD = (vec3<f32>(1.0) - kS) * (1.0 - metallic);

    let sheen_strength = lights.sheen_color_strength.w;
    if (wrap <= 0.0 && oren <= 0.0 && sheen_strength <= 0.0) {
        return (kD * albedo / PI + specular) * radiance * n_dot_l;
    }
    // Charlie sheen rim (Estevez & Kulla 2017 NDF, fixed sheen roughness,
    // no visibility term — a velvety grazing response, tinted per scene;
    // masked by n·l so it reads as rim under each light, not a screen glow).
    var sheen = vec3<f32>(0.0);
    if (sheen_strength > 0.0) {
        let alpha_s = 0.5;
        let sin2h = max(1.0 - n_dot_h * n_dot_h, 1e-4);
        let d_charlie = (2.0 + 1.0 / alpha_s) * pow(sin2h, 0.5 / alpha_s) / (2.0 * PI);
        sheen = lights.sheen_color_strength.rgb * sheen_strength * d_charlie;
    }
    // Wrap replaces the diffuse n·l (terminator softening only).
    var n_dot_l_diffuse = n_dot_l;
    if (wrap > 0.0) {
        n_dot_l_diffuse = max((dot(N, L) + wrap) / (1.0 + wrap), 0.0);
    }
    // Fujii's energy-conserving qualitative Oren-Nayar; `oren` blends.
    var diffuse_scale = 1.0;
    if (oren > 0.0) {
        let sigma = roughness;
        let fujii_a = 1.0 / (1.0 + (0.5 - 2.0 / (3.0 * PI)) * sigma);
        let fujii_b = sigma * fujii_a;
        let s = dot(L, V) - n_dot_l * n_dot_v;
        let t = select(1.0, max(n_dot_l, n_dot_v), s > 0.0);
        // Clamp s/t to 1: with normal-mapped grazing angles both n·l and
        // n·v can approach 0 while s stays positive, exploding the ratio
        // into white fireflies (and wrap-diffuse removes the n·l damping
        // that would normally hide them). The physical range is [0, 1].
        let s_over_t = clamp(max(s, 0.0) / max(t, 1e-4), 0.0, 1.0);
        diffuse_scale = mix(1.0, fujii_a + fujii_b * s_over_t, oren);
    }
    return (kD * albedo / PI) * diffuse_scale * radiance * n_dot_l_diffuse
        + (specular + sheen) * radiance * n_dot_l;
}
