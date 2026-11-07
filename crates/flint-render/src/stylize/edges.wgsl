// Shared geometry edges (#301): one pass a frame, for every style that
// asks for them (`needs: &[StyleNeed::Edges]`).
//
// Edges come from the scene's geometry, never its colour (the ASCII
// spike's finding, #261): a wood texture makes none, a table's outline and
// its creases do. Two sources, each a 3x3 Sobel in the depth texture's own
// pixels:
//   - depth: the view distance relative to the centre's, so a far edge
//     counts as much as a near one, kept on its near side only (Sobel marks
//     both sides of a step, which would draw every outline twice);
//   - normals: rebuilt from the depth's world positions, for creases where
//     two faces meet at one depth (a box's corners), counted only where the
//     5x5 patch is one surface (across a depth step rebuilt normals are
//     nonsense and mark a band 4-5 px wide on both sides).
//
// Output, per depth pixel (Rgba16Float):
//   r: depth edge strength, the Sobel over the shared `edge_depth`
//   g: crease strength, the Sobel over the shared `edge_normal`
//   ba: the unit gradient across the edge in screen space (x right, y down):
//       the depth Sobel's where r >= 1, else the crease's
// A value >= 1 is an edge at the shared thresholds; a style that wants
// more or fewer lines compares against its own level instead.

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

fn world_at(q: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let p = clamp(q, vec2<i32>(0), size - vec2<i32>(1));
    // The sky is cleared to depth 1, which unprojects to the far plane by a
    // w that f32 rounds to about 0: positions at infinity and NaN edges
    // along every skyline. Stop just short of it.
    let d = min(textureLoad(scene_depth, p, 0), 0.99999);
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = u.inv_view_proj * ndc;
    return w.xyz / select(w.w, 1e-6, abs(w.w) < 1e-6);
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

    var pos: array<vec3<f32>, 25>;
    var rel: array<f32, 25>;
    for (var j = 0; j < 5; j++) {
        for (var i = 0; i < 5; i++) {
            pos[j * 5 + i] = world_at(c + vec2<i32>(i - 2, j - 2), size);
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
            let n = normalize(cross(pos[at + 1] - pos[at - 1], pos[at + 5] - pos[at - 5]) + vec3<f32>(1e-9));
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
