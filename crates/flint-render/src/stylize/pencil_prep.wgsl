// Pencil sketch (#273), pass 1: what the drawing pass needs to know about
// each pixel, before any stroke is placed.
//   x: tone, 0 black .. 1 paper (the picture's luminance, or with
//      `tone = "light"` the light's, #869; its brightness and contrast
//      applied)
//   y: contour, 0..1: where the scene's depth steps (on its near side),
//      where its surface creases, where a styled entity's mask ends, and
//      (by `detail`) where its luminance has an edge
//   zw: the local stroke direction as a double-angle vector (cos 2θ,
//      sin 2θ) weighted by how sure it is; the drawing pass blurs it into
//      a smooth flow, so it is a structure tensor in all but name.
//      surface: strokes run along the surface's iso-depth lines (round a
//      barrel, across a floor); tensor: along the luminance's edges;
//      screen: nothing here, the drawing pass uses the base angle.
//      A world grid (#389) needs no flow: zw hold the surface normal,
//      octahedral, for the drawing pass's triplanar strokes.

@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;

fn luma_at(p: vec2<i32>) -> f32 {
    let size = vec2<i32>(u.size);
    return luma(shown(textureLoad(input_texture, clamp(p, vec2<i32>(0), size - 1), 0).rgb));
}

// The light at a pixel (#869): the picture's luminance over its paint's,
// both in display values, so a dark floor in the sun reads as lit and a
// white prop in shadow as dark. Where no mesh was drawn (alpha 0: the sky,
// terrain, water) the picture's own luminance stands, and so it does,
// fading in, as the paint nears black, where the ratio says nothing. Under
// MSAA the albedo is resolved, so its rgb is the covered samples' times the
// coverage in alpha.
fn light_luma(uv: vec2<f32>, picture: f32) -> f32 {
    let size = vec2<i32>(textureDimensions(albedo_tex));
    let q = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let a = textureLoad(albedo_tex, q, 0);
    if a.a <= 0.0 {
        return picture;
    }
    let paint = luma(to_display(a.rgb / a.a));
    let lit = picture / max(paint, 1e-3);
    return mix(picture, lit, a.a * smoothstep(0.02, 0.08, paint));
}

// One axis's difference for a normal: the shorter of the two sides, so a
// silhouette's far side doesn't bend it, but never a side of length 0: at
// the frame's edge a neighbour is clamped onto the centre, and a zero side
// made the normal there degenerate (#404)
fn shorter_side(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let la = dot(a, a);
    let lb = dot(b, b);
    return select(b, a, la > 0.0 && (la < lb || lb == 0.0));
}

// A normal from a centre and its four neighbours
fn normal_from(c: vec3<f32>, l: vec3<f32>, r: vec3<f32>, up: vec3<f32>, dn: vec3<f32>) -> vec3<f32> {
    let dx = shorter_side(c - l, r - c);
    let dy = shorter_side(c - up, dn - c);
    return normalize(cross(dx, dy) + vec3<f32>(1e-12));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.pos.xy);
    let mp = mask_px(in.uv);
    let id = mask_id(mp);
    let slot = slot_of(id);
    if passes_through(slot) {
        return vec4<f32>(1.0, 0.0, 0.0, 0.0);
    }
    let row = u.rows[slot];

    // Tone and the luminance gradient (Sobel)
    var l: array<f32, 9>;
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            l[j * 3 + i] = luma_at(px + vec2<i32>(i - 1, j - 1));
        }
    }
    let gx = (l[2] + 2.0 * l[5] + l[8]) - (l[0] + 2.0 * l[3] + l[6]);
    let gy = (l[6] + 2.0 * l[7] + l[8]) - (l[0] + 2.0 * l[1] + l[2]);
    var lum = l[4];
    if row.i.y > 0.5 {
        lum = light_luma(in.uv, lum);
    }
    let tone = clamp((lum - 0.5) * row.b.y + 0.5 + row.b.x, 0.0, 1.0);

    // World positions round the pixel, in depth texels
    let q = vec2<i32>(in.uv * vec2<f32>(depth_size()));
    let c = world_at(q);
    let l1 = world_at(q + vec2<i32>(-1, 0));
    let r1 = world_at(q + vec2<i32>(1, 0));
    let u1 = world_at(q + vec2<i32>(0, -1));
    let d1 = world_at(q + vec2<i32>(0, 1));
    let l2 = world_at(q + vec2<i32>(-2, 0));
    let r2 = world_at(q + vec2<i32>(2, 0));
    let u2 = world_at(q + vec2<i32>(0, -2));
    let d2 = world_at(q + vec2<i32>(0, 2));
    let lu = world_at(q + vec2<i32>(-1, -1));
    let ru = world_at(q + vec2<i32>(1, -1));
    let ld = world_at(q + vec2<i32>(-1, 1));
    let rd = world_at(q + vec2<i32>(1, 1));

    let cam = u.camera_pos;
    let dc = max(distance(c, cam), 1e-4);
    let dl = distance(l1, cam);
    let dr = distance(r1, cam);
    let du = distance(u1, cam);
    let dd = distance(d1, cam);

    // Depth: a neighbour further away by a share of this pixel's distance,
    // so the line sits on the near side of a step and far edges count as
    // much as near ones
    let gap = max(max(dl, dr), max(du, dd)) - dc;
    var depth_edge = smoothstep(0.02, 0.06, gap / dc * row.c.x);

    // Grazing planes (#404): a floor seen edge-on steps in distance from
    // row to row as much as a silhouette does, and drew a black band along
    // the horizon. 1/z is affine across a plane, so its second difference
    // is 0 on one and large where the neighbours lie on another surface;
    // `grazing` keeps a depth step only where that says it is one
    let grazing = row.i.x;
    if grazing > 0.0 {
        let iz = 1.0 / max(dot(c - cam, u.forward), 1e-4);
        let izl = 1.0 / max(dot(l1 - cam, u.forward), 1e-4);
        let izr = 1.0 / max(dot(r1 - cam, u.forward), 1e-4);
        let izu = 1.0 / max(dot(u1 - cam, u.forward), 1e-4);
        let izd = 1.0 / max(dot(d1 - cam, u.forward), 1e-4);
        let bend = max(abs(izl + izr - 2.0 * iz), abs(izu + izd - 2.0 * iz)) / iz;
        let stepped = smoothstep(0.02, 0.08, bend * max(row.c.x, 1e-3));
        depth_edge *= mix(1.0, stepped, grazing);
    }

    // Creases: the normal turns between this pixel and a neighbour. Within
    // two texels of the frame's edge the neighbours' normals are built from
    // clamped positions, and drew a line round the whole frame in every
    // pencil look; no crease is drawn there (#404)
    let nc = normal_from(c, l1, r1, u1, d1);
    let nl = normal_from(l1, l2, c, lu, ld);
    let nr = normal_from(r1, c, r2, ru, rd);
    let nu = normal_from(u1, lu, ru, u2, c);
    let nd = normal_from(d1, ld, rd, c, d2);
    let turn = 1.0 - min(min(dot(nc, nl), dot(nc, nr)), min(dot(nc, nu), dot(nc, nd)));
    let last = depth_size() - 1;
    let near_border = q.x < 2 || q.y < 2 || q.x > last.x - 2 || q.y > last.y - 2;
    let crease_edge = select(smoothstep(0.08, 0.25, turn * row.c.y), 0.0, near_border);

    // A styled entity's outline: the mask's id changes, drawn on the styled
    // side (the whole frame's settings draw every side)
    var id_edge = 0.0;
    if u.info.z == 1u && slot != 0u {
        let others = mask_id(mp + vec2<i32>(1, 0)) != id || mask_id(mp + vec2<i32>(-1, 0)) != id
            || mask_id(mp + vec2<i32>(0, 1)) != id || mask_id(mp + vec2<i32>(0, -1)) != id;
        id_edge = select(0.0, 1.0, others);
    }

    // Texture detail: luminance edges, by `detail`
    let luma_edge = smoothstep(0.2, 0.6, length(vec2<f32>(gx, gy)) * row.c.z);

    let edge = max(max(depth_edge, crease_edge), max(id_edge, luma_edge));

    if world_grid(row) {
        return vec4<f32>(tone, edge, oct_encode(nc));
    }

    // The stroke direction, perpendicular to a gradient, as a double angle
    var flow = vec2<f32>(0.0);
    let direction = u32(row.e.w + 0.5);
    if direction == 0u {
        // Along iso-depth lines: across the gradient of 1/z, which is
        // constant over a plane (so a box face hatches straight) and turns
        // smoothly round a curve. One-sided differences keep a
        // silhouette's step out of it.
        let zc = max(dot(c - cam, u.forward), 1e-4);
        let iz = 1.0 / zc;
        let izl = 1.0 / max(dot(l1 - cam, u.forward), 1e-4);
        let izr = 1.0 / max(dot(r1 - cam, u.forward), 1e-4);
        let izu = 1.0 / max(dot(u1 - cam, u.forward), 1e-4);
        let izd = 1.0 / max(dot(d1 - cam, u.forward), 1e-4);
        let hx = select(izr - iz, iz - izl, abs(iz - izl) < abs(izr - iz));
        let hy = select(izd - iz, iz - izu, abs(iz - izu) < abs(izd - iz));
        let g = vec2<f32>(hx, hy) * zc;
        let m = length(g);
        if m > 1e-7 {
            let n = g / m;
            let sure = clamp(m * 250.0, 0.0, 1.0);
            flow = -vec2<f32>(n.x * n.x - n.y * n.y, 2.0 * n.x * n.y) * sure;
        }
    } else if direction == 2u {
        // Along the luminance's edges: the structure tensor's minor axis
        flow = -vec2<f32>(gx * gx - gy * gy, 2.0 * gx * gy) * 6.0;
    }
    return vec4<f32>(tone, edge, flow);
}
