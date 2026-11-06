// ASCII spike (#261): the edge pass.
//
// Finds edges per pixel from the scene's geometry, never from its colour,
// so a wood texture makes no edges but a table's outline and its creases
// do. Three sources, each a Sobel over a 3x3 neighbourhood:
//   - depth: the view distance, relative to the centre's, so a far edge
//     counts as much as a near one;
//   - normals: rebuilt from the depth's world positions (central
//     differences), which finds creases where two faces meet at one depth;
//   - silhouettes: a change of style id in the mask (masked modes only).
// Output: r = 1 on an edge, g = the gradient's angle / pi in [0, 1).

struct EdgeUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    mode: u32,
    size: vec2<f32>,
    depth_threshold: f32,
    normal_threshold: f32,
};

@group(0) @binding(0) var<uniform> u: EdgeUniforms;
@group(1) @binding(0) var scene_depth: texture_depth_2d;
@group(1) @binding(1) var mask_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    let uv = vec2<f32>(f32((vid << 1u) & 2u), f32(vid & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn world_at(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let q = clamp(p, vec2<i32>(0), max_px);
    let d = textureLoad(scene_depth, q, 0);
    let uv = (vec2<f32>(q) + 0.5) / u.size;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = u.inv_view_proj * ndc;
    return w.xyz / w.w;
}

fn id_at(p: vec2<i32>) -> f32 {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    return textureLoad(mask_tex, clamp(p, vec2<i32>(0), max_px), 0).x;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = vec2<i32>(in.pos.xy);
    // At the border the patch clamps, and repeated positions make a
    // degenerate normal that reads as a crease down the frame's edge.
    let size_px = vec2<i32>(u.size);
    if c.x < 2 || c.y < 2 || c.x >= size_px.x - 2 || c.y >= size_px.y - 2 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // A 5x5 patch of world positions gives the 3x3 normals their neighbours
    var pos: array<vec3<f32>, 25>;
    for (var j = 0; j < 5; j++) {
        for (var i = 0; i < 5; i++) {
            pos[j * 5 + i] = world_at(c + vec2<i32>(i - 2, j - 2));
        }
    }

    var kx = array<f32, 9>(-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0);
    var ky = array<f32, 9>(-1.0, -2.0, -1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0);
    let centre_dist = max(distance(pos[12], u.camera_pos), 1e-4);

    var dgx = 0.0;
    var dgy = 0.0;
    var ngx = vec3<f32>(0.0);
    var ngy = vec3<f32>(0.0);
    var igx = 0.0;
    var igy = 0.0;
    var mean_dist = 0.0;
    var max_id = 0.0;
    let centre_id = id_at(c);
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            let at = (j + 1) * 5 + (i + 1);
            let k = j * 3 + i;
            let dist = distance(pos[at], u.camera_pos) / centre_dist;
            let n = normalize(cross(pos[at + 1] - pos[at - 1], pos[at + 5] - pos[at - 5]) + vec3<f32>(1e-9));
            dgx += kx[k] * dist;
            dgy += ky[k] * dist;
            ngx += kx[k] * n;
            ngy += ky[k] * n;
            mean_dist += dist / 9.0;
            if u.mode >= 2u {
                let id = id_at(c + vec2<i32>(i - 1, j - 1));
                igx += kx[k] * id;
                igy += ky[k] * id;
                max_id = max(max_id, id);
            }
        }
    }

    // Sobel marks both sides of a step, which draws outlines twice: keep a
    // depth edge on its near side only, and a silhouette on the side of the
    // object (the higher id where two meet).
    var depth_edge = length(vec2<f32>(dgx, dgy)) / u.depth_threshold;
    if mean_dist < 1.0 {
        depth_edge = 0.0;
    }
    let normal_edge = sqrt(dot(ngx, ngx) + dot(ngy, ngy)) / u.normal_threshold;
    let id_edge = select(0.0, 1.0, abs(igx) + abs(igy) > 0.5 && centre_id >= max_id);

    // The angle comes from whichever source is strongest
    var g = vec2<f32>(dgx, dgy);
    var strength = depth_edge;
    if normal_edge > strength {
        strength = normal_edge;
        // The normal Sobel's direction: the component that changes most
        // across both axes, keeping its sign in each
        let ax = abs(ngx) + abs(ngy);
        let lx = select(select(ngx.z, ngx.y, ax.y >= ax.z), ngx.x, ax.x >= max(ax.y, ax.z));
        let ly = select(select(ngy.z, ngy.y, ax.y >= ax.z), ngy.x, ax.x >= max(ax.y, ax.z));
        g = vec2<f32>(lx, ly);
    }
    if id_edge >= strength && id_edge > 0.0 {
        strength = 1.0;
        g = vec2<f32>(igx, igy);
    }
    if strength < 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // Angle mod pi: a line has no direction
    var a = atan2(g.y, g.x);
    if a < 0.0 {
        a += 3.14159265;
    }
    return vec4<f32>(1.0, a / 3.14159265, 0.0, 1.0);
}
