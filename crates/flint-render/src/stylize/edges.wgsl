// Shared geometry edges (#301): one pass a frame, for every style that
// asks for them (`needs: &[StyleNeed::Edges]`). Since #743 it is the only
// geometry edge detector in `stylize/`: ASCII, pencil and contour read it.
//
// Edges come from the scene's geometry, never its colour (the ASCII
// spike's finding, #261): a wood texture makes none, a table's outline and
// its creases do. Two sources, each a 3x3 Sobel in the depth texture's own
// pixels:
//   - depth: the view distance relative to the centre's, so a far edge
//     counts as much as a near one, kept on its near side only (Sobel marks
//     both sides of a step, which would draw every outline twice), and
//     only where the depth really steps: a plane seen edge-on changes its
//     distance from row to row as fast as a silhouette does, but its depth
//     buffer value is affine across the screen, so its second difference
//     is 0 (a step's is as big as the step);
//   - normals: rebuilt from the depth's world positions, one-sided so each
//     stays on its own surface (#484, #952), for creases where two faces meet at
//     one depth (a box's corners), counted only where the 5x5 patch is one
//     surface (across a depth step rebuilt normals are nonsense and mark a
//     band 4-5 px wide on both sides).
//
// Output, per depth pixel (Rgba16Float):
//   r: depth edge strength, the Sobel over the shared `edge_depth`
//   g: crease strength, the Sobel over the shared `edge_normal`
//   ba: the unit gradient across the edge in screen space (x right, y down):
//       the depth Sobel's where r >= 1, else the crease's
// A value >= 1 is an edge at the shared thresholds; a style that wants
// more or fewer lines compares against its own level instead
// (`edges_read.wgsl` turns the strengths back into a step and a turn).

struct EdgeUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    depth_threshold: f32,
    normal_threshold: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> u: EdgeUniforms;
@group(1) @binding(0) var scene_depth: texture_depth_2d;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// How far a step departs from a plane along one line of three depths:
// the second difference over the two first ones, 0 on any plane (whatever
// its slope) and about 1 across a step
fn bend(a: f32, c: f32, b: f32) -> f32 {
    return abs(a + b - 2.0 * c) / (abs(a - c) + abs(b - c) + 1e-7);
}

// How far the pixel `at` lies off the line through its two neighbours on
// one side (`s` the index step to them, signed): 0 when all three are on
// one plane, since a plane's depth is affine across the screen
fn off_side(d: ptr<function, array<f32, 25>>, at: i32, s: i32) -> f32 {
    return abs(2.0 * (*d)[at + s] - (*d)[at + 2 * s] - (*d)[at]);
}

// Whether a tangent along one axis takes the pixel's - side (else its +
// side): the side whose two neighbours lie on the pixel's own plane.
// `k` is the pixel's place on the axis in the 5x5 patch (1..3), `s` the
// index step along it. The middle pixel can test both sides; one off the
// middle has two neighbours on its inner side only, and takes that side
// when the pixel lies on their line (to a tenth of how far the centred
// second difference is off it), else the other.
fn minus_side(d: ptr<function, array<f32, 25>>, at: i32, k: i32, s: i32) -> bool {
    if k == 2 {
        return off_side(d, at, -s) < off_side(d, at, s);
    }
    let mid = abs((*d)[at - s] + (*d)[at + s] - 2.0 * (*d)[at]);
    let inner = select(-s, s, k == 1);
    let off = off_side(d, at, inner);
    let on_inner = off <= 0.1 * (off + mid) + 4e-7;
    return on_inner == (k == 3);
}

// The surface normal at patch index `at`, from one neighbour on each axis,
// on the pixel's own plane. A central difference spans a step whenever one
// sits beside the pixel, and the normal it makes belongs to neither
// surface: where the step is too shallow for the depth Sobel (a pillar's
// side, with the ground just behind it), those normals marked a crease
// band 4 px wide. #484 took the neighbour nearer the centre's view
// distance, which finds the side of a step but not of a fold, where the
// distance runs on unbroken: the steeper face's last pixel took the
// tangent across the fold, and a pillar's foot seen square on was a band
// 3 px tall (#952). Asking which side's neighbours are on the pixel's
// plane answers both, so a normal changes only where the surface does and
// the Sobel marks the two pixels astride it.
fn normal_at(pos: ptr<function, array<vec3<f32>, 25>>, depth: ptr<function, array<f32, 25>>, at: i32) -> vec3<f32> {
    let p = (*pos)[at];
    let left = minus_side(depth, at, at % 5, 1);
    let up = minus_side(depth, at, at / 5, 5);
    // Each tangent points the same way (+x, +y in the patch) whichever
    // side it was taken from, so the cross keeps the central form's sign
    let tx = select((*pos)[at + 1] - p, p - (*pos)[at - 1], left);
    let ty = select((*pos)[at + 5] - p, p - (*pos)[at - 5], up);
    return normalize(cross(tx, ty) + vec3<f32>(1e-9));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let c = vec2<i32>(in.position.xy);
    // At the border the patch clamps, and repeated positions make a
    // degenerate normal that reads as a crease down the frame's edge
    if c.x < 2 || c.y < 2 || c.x >= size.x - 2 || c.y >= size.y - 2 {
        return vec4<f32>(0.0);
    }

    // A 5x5 patch of world positions gives the 3x3 normals their neighbours
    // and their view distances, relative to the centre's. The sky is
    // cleared to depth 1, which unprojects to the far plane by a w that f32
    // rounds to about 0: positions at infinity and NaN edges along every
    // skyline. Its depth stops just short of it.
    var pos: array<vec3<f32>, 25>;
    var rel: array<f32, 25>;
    var depth: array<f32, 25>;
    for (var k = 0; k < 25; k++) {
        depth[k] = min(textureLoad(scene_depth, c + vec2<i32>(k % 5 - 2, k / 5 - 2), 0), 0.99999);
    }
    // A patch on one plane holds no crease and no step: its depth is affine
    // across the screen, so every second difference along its rows and
    // columns is 0 beside the first ones (to float noise). Most of a frame
    // is such patches, and they skip the unprojections and normals, which
    // are most of the pass's cost (#743)
    var flat = true;
    for (var k = 0; k < 15; k++) {
        let a = (k / 3) * 5 + k % 3;
        let b = (k % 5) + (k / 5) * 5;
        let h = abs(depth[a] + depth[a + 2] - 2.0 * depth[a + 1]);
        let v = abs(depth[b] + depth[b + 10] - 2.0 * depth[b + 5]);
        flat = flat && h <= 0.01 * (abs(depth[a] - depth[a + 1]) + abs(depth[a + 2] - depth[a + 1])) + 4e-7
            && v <= 0.01 * (abs(depth[b] - depth[b + 5]) + abs(depth[b + 10] - depth[b + 5])) + 4e-7;
    }
    if flat {
        return vec4<f32>(0.0);
    }
    for (var j = 0; j < 5; j++) {
        for (var i = 0; i < 5; i++) {
            let p = c + vec2<i32>(i - 2, j - 2);
            let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
            pos[j * 5 + i] = flint_unproject(u.inv_view_proj, uv, depth[j * 5 + i]);
        }
    }
    let centre_dist = max(distance(pos[12], u.camera_pos), 1e-4);
    var spread = 0.0;
    for (var k = 0; k < 25; k++) {
        rel[k] = distance(pos[k], u.camera_pos) / centre_dist;
        spread = max(spread, abs(rel[k] - 1.0));
    }

    var kx = array<f32, 9>(-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0);
    var ky = array<f32, 9>(-1.0, -2.0, -1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0);
    var dgx = 0.0;
    var dgy = 0.0;
    var ngx = vec3<f32>(0.0);
    var ngy = vec3<f32>(0.0);
    var mean_dist = 0.0;
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            let at = (j + 1) * 5 + (i + 1);
            let k = j * 3 + i;
            let dist = rel[at];
            let n = normal_at(&pos, &depth, at);
            dgx += kx[k] * dist;
            dgy += ky[k] * dist;
            ngx += kx[k] * n;
            ngy += ky[k] * n;
            mean_dist += dist / 9.0;
        }
    }

    let dg = vec2<f32>(dgx, dgy);
    var depth_edge = length(dg) / u.depth_threshold;
    // Near side only: the far side's neighbours are, on average, nearer
    if mean_dist < 1.0 {
        depth_edge = 0.0;
    }
    // A plane seen edge-on is no silhouette (#743, contour's rule since
    // #214 and pencil's `grazing` since #404): the depth must bend along
    // some line through the centre, across or diagonal
    let steps = max(
        max(bend(depth[11], depth[12], depth[13]), bend(depth[7], depth[12], depth[17])),
        max(bend(depth[6], depth[12], depth[18]), bend(depth[8], depth[12], depth[16])),
    );
    depth_edge *= smoothstep(0.2, 0.5, steps);
    var crease = sqrt(dot(ngx, ngx) + dot(ngy, ngy)) / u.normal_threshold;
    if !(spread < 0.5 * u.depth_threshold) {
        crease = 0.0;
    }

    // The crease's direction: the normal component that changes most
    // across both axes, keeping its sign in each (picking from the
    // x-gradient alone turns a horizontal crease into a row of `/`)
    let ax = abs(ngx) + abs(ngy);
    let lx = select(select(ngx.z, ngx.y, ax.y >= ax.z), ngx.x, ax.x >= max(ax.y, ax.z));
    let ly = select(select(ngy.z, ngy.y, ax.y >= ax.z), ngy.x, ax.x >= max(ax.y, ax.z));
    var g = vec2<f32>(lx, ly);
    // A silhouette owns its direction; a crease only where there's none
    // (strongest-wins let the normal Sobel take over along skylines, where
    // the patch spans the far plane and its direction can sum to nothing)
    if depth_edge >= 1.0 && length(dg) > 1e-6 {
        g = dg;
    }
    let len = length(g);
    var dir = vec2<f32>(0.0);
    if len > 1e-6 {
        dir = g / len;
    }
    // f16 holds up to 65504; a NaN that got through becomes no edge
    let r = select(0.0, min(depth_edge, 60000.0), depth_edge >= 0.0);
    let gc = select(0.0, min(crease, 60000.0), crease >= 0.0);
    if !(r > 0.0) && !(gc > 0.0) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(r, gc, dir);
}

// The depth steps alone (#743), for a frame whose every reader asks for
// no creases (`StyleNeed::EdgeSteps`: contour): the 3x3 patch the depth
// Sobel needs, no normals. r is fs_main's, g is 0, and the direction is
// always the depth step's.
@fragment
fn fs_steps(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let c = vec2<i32>(in.position.xy);
    if c.x < 2 || c.y < 2 || c.x >= size.x - 2 || c.y >= size.y - 2 {
        return vec4<f32>(0.0);
    }
    var pos: array<vec3<f32>, 9>;
    var depth: array<f32, 9>;
    for (var k = 0; k < 9; k++) {
        depth[k] = min(textureLoad(scene_depth, c + vec2<i32>(k % 3 - 1, k / 3 - 1), 0), 0.99999);
    }
    // Where the depth bends along no line through the centre there is no
    // step, whatever the Sobel says, so nothing to unproject
    let steps = max(
        max(bend(depth[3], depth[4], depth[5]), bend(depth[1], depth[4], depth[7])),
        max(bend(depth[0], depth[4], depth[8]), bend(depth[2], depth[4], depth[6])),
    );
    let stepped = smoothstep(0.2, 0.5, steps);
    if stepped <= 0.0 {
        return vec4<f32>(0.0);
    }
    for (var k = 0; k < 9; k++) {
        let p = c + vec2<i32>(k % 3 - 1, k / 3 - 1);
        let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
        pos[k] = flint_unproject(u.inv_view_proj, uv, depth[k]);
    }
    let centre_dist = max(distance(pos[4], u.camera_pos), 1e-4);
    var kx = array<f32, 9>(-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0);
    var ky = array<f32, 9>(-1.0, -2.0, -1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0);
    var dgx = 0.0;
    var dgy = 0.0;
    var mean_dist = 0.0;
    for (var k = 0; k < 9; k++) {
        let dist = distance(pos[k], u.camera_pos) / centre_dist;
        dgx += kx[k] * dist;
        dgy += ky[k] * dist;
        mean_dist += dist / 9.0;
    }
    let dg = vec2<f32>(dgx, dgy);
    var depth_edge = length(dg) / u.depth_threshold;
    if mean_dist < 1.0 {
        depth_edge = 0.0;
    }
    depth_edge *= stepped;
    let r = select(0.0, min(depth_edge, 60000.0), depth_edge >= 0.0);
    let len = length(dg);
    if !(r > 0.0) || !(len > 1e-6) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(r, 0.0, dg / len);
}
