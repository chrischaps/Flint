// The light every lit surface gathers from the scene's lights (#738): each
// directional (the first through the sun's cascades), point and spot light
// (the casting spot through its map), through the one BRDF. Meshes, skinned
// meshes and terrain call it with their own surface; before #738 each had its
// own copy of these loops, and terrain's had drifted (no area sources, no
// clay levers, no normal offset on its cascades). Needs `brdf.wgsl` and
// `shadows.wgsl`.
//
// `world_pos` is the fragment's, `geo_normal` the surface's own normalized
// normal (the shadows' normal offset), `N` the shading normal (a normal map's
// where there is one), `view_depth` its distance from the camera (cascade
// choice) and `frag_xy` its pixel (the spot's PCSS turns its disk by it).
fn direct_light(
    world_pos: vec3<f32>,
    geo_normal: vec3<f32>,
    N: vec3<f32>,
    V: vec3<f32>,
    albedo: vec3<f32>,
    f0: vec3<f32>,
    metallic: f32,
    roughness: f32,
    n_dot_v: f32,
    view_depth: f32,
    frag_xy: vec2<f32>,
) -> vec3<f32> {
    // === Directional lights ===
    // Diffuse-wrap knob rides ambient_sky.w, encoded as (1 + wrap) so every
    // legacy CPU write of 1.0 decodes to wrap = 0 (exact original shading).
    // Oren-Nayar blend rides ambient_ground.w the same way (ADR 0048).
    let wrap = max(lights.ambient_sky.w - 1.0, 0.0);
    let oren = max(lights.ambient_ground.w - 1.0, 0.0);
    var Lo = vec3<f32>(0.0);
    for (var i = 0u; i < lights.directional_count; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = normalize(light.direction);
        let radiance = light.color * light.intensity;
        var contribution = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v, wrap, oren);

        // Apply shadow from cascaded shadow maps to the first directional light
        if (i == 0u) {
            let sf = shadow_factor(world_pos, view_depth, geo_normal);
            contribution = contribution * sf;
        }

        Lo += contribution;
    }

    // === Point lights ===
    for (var i = 0u; i < lights.point_count; i = i + 1u) {
        let light = lights.point_lights[i];
        let light_vec = light.position - world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        // Representative-point area source (ADR 0056): a source of radius r
        // seen from distance d widens the specular lobe by ~r/(2d), and the
        // shading distance never falls inside the source. source_radius = 0
        // passes the original arguments verbatim.
        var roughness_l = roughness;
        var shade_dist = distance;
        if (light.source_radius > 0.0) {
            roughness_l = clamp(
                roughness + light.source_radius / (2.0 * max(distance, light.source_radius)),
                roughness, 1.0);
            shade_dist = max(distance, light.source_radius);
        }
        let atten = attenuation(shade_dist, light.radius);
        let radiance = light.color * light.intensity * atten;
        var contribution = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness_l, n_dot_v, wrap, oren);
        // The one point light whose cube map is drawn (#1112), where it
        // gives light
        if (light.shadow > 0.5 && atten > 0.0) {
            contribution = contribution * point_shadow_factor(world_pos, geo_normal,
                light.position, light.source_radius, frag_xy);
        }
        Lo += contribution;
    }

    // === Spot lights ===
    for (var i = 0u; i < lights.spot_count; i = i + 1u) {
        let light = lights.spot_lights[i];
        let light_vec = light.position - world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        // Representative-point area source — same construction as the point
        // loop above (ADR 0056); 0 = verbatim legacy.
        var roughness_l = roughness;
        var shade_dist = distance;
        if (light.source_radius > 0.0) {
            roughness_l = clamp(
                roughness + light.source_radius / (2.0 * max(distance, light.source_radius)),
                roughness, 1.0);
            shade_dist = max(distance, light.source_radius);
        }
        let atten = attenuation(shade_dist, light.radius);
        // The cone is about the way the spot shines, light to fragment:
        // light_vec runs the other way (#500, #682).
        let cone = spot_cone_factor(-light_vec, light.direction, light.inner_angle, light.outer_angle);
        let radiance = light.color * light.intensity * atten * cone;
        var contribution = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness_l, n_dot_v, wrap, oren);
        // The one spot whose shadow map is drawn (#504), where it gives
        // light: outside its cone and range the filter would darken nothing
        if (light.shadow > 0.5 && atten * cone > 0.0) {
            contribution = contribution * spot_shadow_factor(world_pos, geo_normal,
                light.position, light.direction, light.source_radius, frag_xy);
        }
        Lo += contribution;
    }
    return Lo;
}
