// The scene's point and spot lights for a surface that shades in its own look
// rather than through the BRDF (#1048): grass and water. Before this they
// declared the lights and never read them, so a lantern's pool stopped at the
// grass line and the waterline. Each function gives the light at a point,
// the way to it and the radiance arriving, with the falloff, area-source
// distance, cone and casting spot's and point light's shadows `direct_light`
// gives a mesh; what the surface does with it is its own. Needs `brdf.wgsl`
// (`attenuation`, `spot_cone_factor`) and `shadows.wgsl`
// (`spot_shadow_factor`, `point_shadow_factor`).

struct LocalLight {
    // Toward the light, normalized
    L: vec3<f32>,
    // What reaches the point: colour x intensity x falloff (x cone x shadow)
    radiance: vec3<f32>,
};

// Point light `i`. The shading distance never falls inside the source
// (ADR 0056), as on meshes; a punctual light's is the distance itself. The
// point light whose cube map is drawn (#1112) shadows where it gives light;
// `geo_normal` and `frag_xy` as for `spot_light_at`.
fn point_light_at(i: u32, world_pos: vec3<f32>, geo_normal: vec3<f32>, frag_xy: vec2<f32>) -> LocalLight {
    let light = lights.point_lights[i];
    let light_vec = light.position - world_pos;
    let atten = attenuation(max(length(light_vec), light.source_radius), light.radius);
    var radiance = light.color * light.intensity * atten;
    if (light.shadow > 0.5 && atten > 0.0) {
        radiance = radiance * point_shadow_factor(world_pos, geo_normal,
            light.position, light.source_radius, frag_xy);
    }
    return LocalLight(normalize(light_vec), radiance);
}

// Spot light `i`: the cone takes light-to-fragment (#500, #682), and the
// spot whose map is drawn (#504) shadows where it gives light. `geo_normal`
// is the receiver's own normal, for the map's normal offset; `frag_xy` its
// pixel, which turns the soft shadow's disk.
fn spot_light_at(i: u32, world_pos: vec3<f32>, geo_normal: vec3<f32>, frag_xy: vec2<f32>) -> LocalLight {
    let light = lights.spot_lights[i];
    let light_vec = light.position - world_pos;
    let atten = attenuation(max(length(light_vec), light.source_radius), light.radius);
    let cone = spot_cone_factor(-light_vec, light.direction, light.inner_angle, light.outer_angle);
    var radiance = light.color * light.intensity * atten * cone;
    if (light.shadow > 0.5 && atten * cone > 0.0) {
        radiance = radiance * spot_shadow_factor(world_pos, geo_normal,
            light.position, light.direction, light.source_radius, frag_xy);
    }
    return LocalLight(normalize(light_vec), radiance);
}
