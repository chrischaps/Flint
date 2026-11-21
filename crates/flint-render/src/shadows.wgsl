// Shadows every lit surface takes (#738): the sun's cascades (`shadow_factor`),
// the casting spot's map (`spot_shadow_factor`) and the casting point light's
// cube (`point_shadow_factor`, #1112). Needs `light_bindings.wgsl`.
// Terrain took the cascades without the normal offset until #738, and the
// ocean without PCSS; both read this one now.

// Compute shadow factor for a world-space position using cascaded shadow maps
// Returns 1.0 (fully lit) or a value approaching 0.0 (shadowed)
// N is the geometric surface normal, used for normal-offset receiver bias:
// slope-scale bias in the depth pass alone can't prevent acne at glancing
// angles on curved surfaces (low-poly cylinders/capsules band visibly).
fn shadow_factor(world_pos: vec3<f32>, view_depth: f32, N: vec3<f32>) -> f32 {
    // cascade_splits.z is the shadow far plane — fade out over the last 15%
    let shadow_far = shadow.cascade_splits.z;
    let fade_start = shadow_far * 0.75;
    if (view_depth > shadow_far) {
        return 1.0;
    }
    let distance_fade = 1.0 - smoothstep(fade_start, shadow_far, view_depth);

    // Select cascade based on view-space depth
    var cascade: i32 = 0;
    if (view_depth > shadow.cascade_splits.x) {
        cascade = 1;
    }
    if (view_depth > shadow.cascade_splits.y) {
        cascade = 2;
    }

    // Normal-offset bias: push the receiver position out along the surface
    // normal by ~2 shadow texels (in world units) before projecting. The
    // cascade's world-per-texel comes from the ortho projection row scale.
    // Texel size rides cascade_splits.w (0 = unset -> legacy hardcoded
    // 1/2048, the exact pre-lever behavior at the default 2048 resolution;
    // powers of two keep the fallback bit-identical).
    var texel_size = shadow.cascade_splits.w;
    if (texel_size <= 0.0) {
        texel_size = 1.0 / 2048.0;
    }

    let m = shadow.cascade_view_proj[cascade];
    let row0_len = length(vec3<f32>(m[0].x, m[1].x, m[2].x));
    let texel_world = 2.0 * texel_size / max(row0_len, 0.0001);
    let biased_pos = world_pos + N * texel_world * 2.0;

    // Project world position into shadow map space
    let light_space = shadow.cascade_view_proj[cascade] * vec4<f32>(biased_pos, 1.0);
    let proj = light_space.xyz / light_space.w;

    // Convert from clip space [-1,1] to texture space [0,1]
    let shadow_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);

    // Fade at shadow map UV edges to avoid hard boundary
    let edge_fade = min(
        min(smoothstep(0.0, 0.02, shadow_uv.x), smoothstep(0.0, 0.02, 1.0 - shadow_uv.x)),
        min(smoothstep(0.0, 0.02, shadow_uv.y), smoothstep(0.0, 0.02, 1.0 - shadow_uv.y))
    );
    if (edge_fade <= 0.0) {
        return 1.0;
    }

    let depth = proj.z;

    var raw_shadow = 1.0;
    if (shadow.pcss.w > 0.0) {
        // === PCSS: penumbra grows with occluder distance (ADR 0057) ===
        // pcss.w = tan(sun angular size); pcss[cascade] = the light-ortho
        // depth range in world units (converts [0,1] depth deltas back to
        // world distances). The ortho row scale converts world -> UV.
        let tan_a = shadow.pcss.w;
        let depth_range = shadow.pcss[cascade];
        let uv_per_world = row0_len * 0.5;
        let max_radius_uv = 8.0 * texel_size; // kernel-cost / cascade-bleed cap
        let resolution = 1.0 / texel_size;
        let max_coord = i32(resolution) - 1;

        // Blocker search: average occluder depth over a Vogel disk sized by
        // how far a blocker across the whole depth range could spread the
        // penumbra at this receiver. textureLoad (raw depth reads) — the
        // comparison sampler can't return depths and Depth32Float is not
        // filterable, so no new binding is needed.
        let search_uv = clamp(tan_a * depth * depth_range * uv_per_world,
            texel_size, max_radius_uv);
        var blocker_sum = 0.0;
        var blocker_count = 0.0;
        for (var i = 0; i < 16; i = i + 1) {
            // Vogel disk: r = sqrt((i+0.5)/N), golden-angle steps.
            let r = sqrt((f32(i) + 0.5) / 16.0);
            let theta = f32(i) * 2.39996;
            let tap_uv = shadow_uv + vec2<f32>(cos(theta), sin(theta)) * r * search_uv;
            let coords = clamp(vec2<i32>(tap_uv * resolution),
                vec2<i32>(0), vec2<i32>(max_coord));
            let d = textureLoad(shadow_maps, coords, cascade, 0);
            if (d < depth) {
                blocker_sum += d;
                blocker_count += 1.0;
            }
        }

        if (blocker_count > 0.0) {
            // Directional/ortho penumbra: width = tan(angular) x world-space
            // occluder distance. No divide (that's the point-light form) —
            // the clamps below are the firefly-lesson guards (ADR 0048).
            let avg_blocker = blocker_sum / blocker_count;
            let penumbra_world = tan_a * max(depth - avg_blocker, 0.0) * depth_range;
            let filter_uv = clamp(penumbra_world * uv_per_world,
                texel_size, max_radius_uv);

            var shadow_sum = 0.0;
            for (var i = 0; i < 16; i = i + 1) {
                let r = sqrt((f32(i) + 0.5) / 16.0);
                let theta = f32(i) * 2.39996;
                let offset = vec2<f32>(cos(theta), sin(theta)) * r * filter_uv;
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps,
                    shadow_sampler,
                    shadow_uv + offset,
                    cascade,
                    depth
                );
            }
            raw_shadow = shadow_sum / 16.0;
        }
        // No blockers found -> fully lit (raw_shadow stays 1.0).
    } else {
        // Legacy 3x3 PCF (percentage-closer filtering) — verbatim pre-PCSS
        // path; pcss.w = 0 means the lever is off (ADR 0057).
        var shadow_sum = 0.0;
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps,
                    shadow_sampler,
                    shadow_uv + offset,
                    cascade,
                    depth
                );
            }
        }
        raw_shadow = shadow_sum / 9.0;
    }

    // Blend shadow toward 1.0 (lit) based on distance and edge fades
    return mix(1.0, raw_shadow, distance_fade * edge_fade);
}

// The spot light's shadow (#504, ADR 0124): layer 3 of the shadow map array,
// a perspective map drawn from the spot that `cast_shadows` chose. Returns
// 1.0 (lit) to 0.0 (shadowed). `source_radius` above 0 softens it the way a
// sun's `angular_size` softens the cascades (PCSS, ADR 0057): sharp where a
// caster meets what it shadows, wider as they part. 0 is a 3x3 PCF.
// N is the geometric normal, for the receiver's normal offset; frag_xy the
// fragment's pixel, which turns the soft kernel a little each pixel so its
// taps blend into fine grain instead of stepping in rings.
const SPOT_SHADOW_LAYER: i32 = 3;

// A depth from the spot map back to metres along the light's axis
fn spot_linear_depth(d: f32) -> f32 {
    let near = shadow.spot.x;
    let far = shadow.spot.y;
    return near * far / max(far - d * (far - near), 1e-6);
}

// Metres along the light's axis back to the spot map's depth
fn spot_map_depth(z: f32) -> f32 {
    let near = shadow.spot.x;
    let far = shadow.spot.y;
    return far * (z - near) / ((far - near) * max(z, 1e-6));
}

fn spot_shadow_factor(world_pos: vec3<f32>, N: vec3<f32>, light_pos: vec3<f32>, light_dir: vec3<f32>, source_radius: f32, frag_xy: vec2<f32>) -> f32 {
    if (shadow.spot.w <= 0.0) {
        return 1.0;
    }
    let near = shadow.spot.x;
    let tan_half = shadow.spot.z;
    var texel_size = shadow.cascade_splits.w;
    if (texel_size <= 0.0) {
        texel_size = 1.0 / 2048.0;
    }

    // Normal offset by ~1.5 texels as wide as the map is at this depth: a
    // perspective map's texels grow with distance from the lamp
    let z_axis = max(dot(world_pos - light_pos, normalize(light_dir)), near);
    let texel_world = 2.0 * z_axis * tan_half * texel_size;
    let biased_pos = world_pos + N * texel_world * 1.5;

    let clip = shadow.spot_view_proj * vec4<f32>(biased_pos, 1.0);
    if (clip.w <= near) {
        return 1.0; // behind the lamp
    }
    let proj = clip.xyz / clip.w;
    let shadow_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let depth = proj.z;
    if (depth >= 1.0) {
        return 1.0; // past the light's range, which it doesn't reach
    }
    // Fade at the map's edge, which sits outside the cone
    let edge_fade = min(
        min(smoothstep(0.0, 0.02, shadow_uv.x), smoothstep(0.0, 0.02, 1.0 - shadow_uv.x)),
        min(smoothstep(0.0, 0.02, shadow_uv.y), smoothstep(0.0, 0.02, 1.0 - shadow_uv.y))
    );
    if (edge_fade <= 0.0) {
        return 1.0;
    }

    var raw_shadow = 1.0;
    if (source_radius > 0.0) {
        // === PCSS for a lamp with a size ===
        // From the receiver, the lamp's disc of radius R spans, at depth z,
        // R (z_r - z) / z_r metres; the map is 2 z tan_half metres wide
        // there. Blockers hide anywhere in that disc, widest at the near
        // plane, so the search covers that, capped.
        let z_r = spot_linear_depth(depth);
        // A surface the key grazes tilts away along the map: a tap w metres
        // off the centre lands on the receiver's own plane up to w * slope
        // nearer the lamp. Each tap is held that far under the receiver, so
        // the wide kernel never reads the surface as its own blocker.
        let cos_l = max(dot(N, normalize(light_pos - world_pos)), 0.05);
        let slope = min(sqrt(1.0 - cos_l * cos_l) / cos_l, 8.0);
        let world_per_uv = 2.0 * z_r * tan_half;
        let max_radius_uv = 48.0 * texel_size; // kernel cost cap
        let resolution = 1.0 / texel_size;
        let max_coord = i32(resolution) - 1;
        // Interleaved gradient noise (Jimenez 2014): a turn of the disk per pixel
        let spin = 6.2831853 * fract(52.9829189 * fract(dot(frag_xy, vec2<f32>(0.06711056, 0.00583715))));
        let search_uv = clamp(
            source_radius * (z_r - near) / (z_r * 2.0 * near * tan_half),
            texel_size, max_radius_uv);
        var blocker_sum = 0.0;
        var blocker_count = 0.0;
        for (var i = 0; i < 16; i = i + 1) {
            let r = sqrt((f32(i) + 0.5) / 16.0);
            let theta = f32(i) * 2.39996 + spin;
            let tap_uv = shadow_uv + vec2<f32>(cos(theta), sin(theta)) * r * search_uv;
            let coords = clamp(vec2<i32>(tap_uv * resolution),
                vec2<i32>(0), vec2<i32>(max_coord));
            let z_d = spot_linear_depth(textureLoad(shadow_maps, coords, SPOT_SHADOW_LAYER, 0));
            if (z_d < z_r - r * search_uv * world_per_uv * slope - texel_world) {
                blocker_sum += z_d;
                blocker_count += 1.0;
            }
        }
        if (blocker_count > 0.0) {
            // Penumbra radius on the receiver: R (z_r - z_b) / z_b metres,
            // over the map's width at the receiver
            let z_b = max(blocker_sum / blocker_count, near);
            let penumbra_world = source_radius * max(z_r - z_b, 0.0) / z_b;
            let filter_uv = clamp(penumbra_world / (2.0 * z_r * tan_half),
                texel_size, max_radius_uv);
            var shadow_sum = 0.0;
            for (var i = 0; i < 32; i = i + 1) {
                let r = sqrt((f32(i) + 0.5) / 32.0);
                let theta = f32(i) * 2.39996 + spin;
                let offset = vec2<f32>(cos(theta), sin(theta)) * r * filter_uv;
                let tap_depth = spot_map_depth(max(z_r - r * filter_uv * world_per_uv * slope, near));
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps, shadow_sampler, shadow_uv + offset, SPOT_SHADOW_LAYER, tap_depth);
            }
            raw_shadow = shadow_sum / 32.0;
        }
        // No blockers found -> fully lit
    } else {
        var shadow_sum = 0.0;
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps, shadow_sampler, shadow_uv + offset, SPOT_SHADOW_LAYER, depth);
            }
        }
        raw_shadow = shadow_sum / 9.0;
    }
    return mix(1.0, raw_shadow, edge_fade);
}

// The point light's shadow (#1112, ADR 0132): a cube of six perspective maps
// from the point light that `cast_shadows` chose, side by side in layer 4 of
// the shadow map array, three across and two down. A fragment reads the face
// its direction from the lamp leans on most; each face reaches a little past
// 45 degrees (`point.z`), so a soft kernel near a seam still finds real
// depths on the face it started on, and is held inside that face's square.
// Softened by `source_radius` as the spot is (PCSS), with the same slope
// hold; 0 is a 3x3 PCF. Returns 1.0 (lit) to 0.0 (shadowed).
const POINT_SHADOW_LAYER: i32 = 4;

// A depth from a cube face back to metres along its axis
fn point_linear_depth(d: f32) -> f32 {
    let near = shadow.point.x;
    let far = shadow.point.y;
    return near * far / max(far - d * (far - near), 1e-6);
}

// Metres along a face's axis back to its map's depth
fn point_map_depth(z: f32) -> f32 {
    let near = shadow.point.x;
    let far = shadow.point.y;
    return far * (z - near) / ((far - near) * max(z, 1e-6));
}

// The cube face a direction from the lamp falls on: its dominant axis, in the
// order +X, -X, +Y, -Y, +Z, -Z, x before y before z on a tie
// (`shadow::point_shadow_cube` draws them in that order)
fn point_face(d: vec3<f32>) -> i32 {
    let a = abs(d);
    if (a.x >= a.y && a.x >= a.z) {
        return select(1, 0, d.x > 0.0);
    }
    if (a.y >= a.z) {
        return select(3, 2, d.y > 0.0);
    }
    return select(5, 4, d.z > 0.0);
}

fn point_shadow_factor(world_pos: vec3<f32>, N: vec3<f32>, light_pos: vec3<f32>, source_radius: f32, frag_xy: vec2<f32>) -> f32 {
    if (shadow.point.w <= 0.0) {
        return 1.0;
    }
    let near = shadow.point.x;
    let tan_half = shadow.point.z;
    // A face's width in the layer's UV, and one texel in the face's own
    let tile = shadow.point_atlas.x;
    let face_texel = 1.0 / max(shadow.point_atlas.y, 1.0);
    var texel_size = shadow.cascade_splits.w;
    if (texel_size <= 0.0) {
        texel_size = 1.0 / 2048.0;
    }

    // Normal offset by ~1.5 texels as wide as the face is at this depth
    let a = abs(world_pos - light_pos);
    let z_axis = max(max(max(a.x, a.y), a.z), near);
    let texel_world = 2.0 * z_axis * tan_half * face_texel;
    let biased_pos = world_pos + N * texel_world * 1.5;

    let face = point_face(biased_pos - light_pos);
    let clip = shadow.point_view_proj[face] * vec4<f32>(biased_pos, 1.0);
    if (clip.w <= near) {
        return 1.0; // inside the lamp's near plane
    }
    let proj = clip.xyz / clip.w;
    let face_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let depth = proj.z;
    if (depth >= 1.0) {
        return 1.0; // past the light's range, which it doesn't reach
    }
    // The face's square of the layer, half a texel in: no tap leaves it
    let origin = vec2<f32>(f32(face % 3), f32(face / 3)) * tile;
    let lo = origin + vec2<f32>(0.5 * texel_size);
    let hi = origin + vec2<f32>(tile - 0.5 * texel_size);
    let shadow_uv = origin + face_uv * tile;

    var raw_shadow = 1.0;
    if (source_radius > 0.0) {
        // The spot's PCSS in a face's terms: radii in the face's UV, then
        // `tile` of the layer's per unit
        let z_r = point_linear_depth(depth);
        let cos_l = max(dot(N, normalize(light_pos - world_pos)), 0.05);
        let slope = min(sqrt(1.0 - cos_l * cos_l) / cos_l, 8.0);
        let world_per_uv = 2.0 * z_r * tan_half;
        // Inside the band past the face's edge (25 texels at tan 1.08)
        let max_radius_uv = 24.0 * face_texel;
        let resolution = 1.0 / texel_size;
        let max_coord = i32(resolution) - 1;
        let spin = 6.2831853 * fract(52.9829189 * fract(dot(frag_xy, vec2<f32>(0.06711056, 0.00583715))));
        let search_uv = clamp(
            source_radius * (z_r - near) / (z_r * 2.0 * near * tan_half),
            face_texel, max_radius_uv);
        var blocker_sum = 0.0;
        var blocker_count = 0.0;
        for (var i = 0; i < 16; i = i + 1) {
            let r = sqrt((f32(i) + 0.5) / 16.0);
            let theta = f32(i) * 2.39996 + spin;
            let tap_uv = clamp(shadow_uv + vec2<f32>(cos(theta), sin(theta)) * r * search_uv * tile,
                lo, hi);
            let coords = clamp(vec2<i32>(tap_uv * resolution),
                vec2<i32>(0), vec2<i32>(max_coord));
            let z_d = point_linear_depth(textureLoad(shadow_maps, coords, POINT_SHADOW_LAYER, 0));
            if (z_d < z_r - r * search_uv * world_per_uv * slope - texel_world) {
                blocker_sum += z_d;
                blocker_count += 1.0;
            }
        }
        if (blocker_count > 0.0) {
            let z_b = max(blocker_sum / blocker_count, near);
            let penumbra_world = source_radius * max(z_r - z_b, 0.0) / z_b;
            let filter_uv = clamp(penumbra_world / world_per_uv, face_texel, max_radius_uv);
            var shadow_sum = 0.0;
            for (var i = 0; i < 32; i = i + 1) {
                let r = sqrt((f32(i) + 0.5) / 32.0);
                let theta = f32(i) * 2.39996 + spin;
                let offset = vec2<f32>(cos(theta), sin(theta)) * r * filter_uv * tile;
                let tap_depth = point_map_depth(max(z_r - r * filter_uv * world_per_uv * slope, near));
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps, shadow_sampler, clamp(shadow_uv + offset, lo, hi),
                    POINT_SHADOW_LAYER, tap_depth);
            }
            raw_shadow = shadow_sum / 32.0;
        }
        // No blockers found -> fully lit
    } else {
        var shadow_sum = 0.0;
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps, shadow_sampler, clamp(shadow_uv + offset, lo, hi),
                    POINT_SHADOW_LAYER, depth);
            }
        }
        raw_shadow = shadow_sum / 9.0;
    }
    return raw_shadow;
}
