// ASCII edges (#267), pass 1 of 3: which pixels lie on an edge.
//
// Edges come from the scene's geometry, never its colour, so a wood
// texture makes none but a table's outline and its creases do (the ASCII
// spike's finding, #261). Three sources, each a 3x3 Sobel in the depth
// texture's own pixels:
//   - depth: the view distance relative to the centre's, so a far edge
//     counts as much as a near one;
//   - normals: rebuilt from the depth's world positions, for creases where
//     two faces meet at one depth (a box's corners);
//   - the style mask: where an ascii-styled entity meets anything else, so
//     a styled entity's outline is always drawn, whatever the thresholds.
// Sobel marks both sides of a step, which would draw every outline twice:
// a depth or mask edge is kept on its near side only.
//
// Output, per depth pixel: the unit gradient's structure tensor
// (gx*gx, gx*gy, gy*gy) in screen space (y down), and w = the source that
// decided it: 0 (no edge), 1 (a crease), 2 (a depth step) or 3 (a styled
// entity's outline). The cell pass sums them over each glyph cell; the
// edge debug view (#349) colours each pixel by it.

struct EdgeUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad0: u32,
    _pad: vec2<f32>,
    depth_threshold: f32,
    normal_threshold: f32,
    // The plain `ascii` mask id (0 = none)
    plain_id: u32,
    // Scalars, not a vec3 (which would align to 16 and grow the struct)
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
};

@group(0) @binding(0) var<uniform> u: EdgeUniforms;
// Each mask id's variant slot (#336, the glyph pass's `slots`): non-zero
// for every per-entity variant's id
@group(0) @binding(1) var<storage, read> slots: array<u32>;

// A mask id drawn in this style (#266): the plain `ascii` id, or one the
// variant table gives a slot
fn is_ours(id: u32) -> bool {
    if id == 0u {
        return false;
    }
    if id == u.plain_id {
        return true;
    }
    return id < arrayLength(&slots) && slots[id] != 0u;
}
@group(1) @binding(0) var scene_depth: texture_depth_2d;
@group(2) @binding(0) var mask: texture_2d<u32>;

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
    // w that f32 rounds to about 0: positions at infinity, a Sobel of inf -
    // inf, and NaN edges along every skyline (they came out doubled on one
    // side of an object and missing on the other). Stop just short of it.
    let d = min(textureLoad(scene_depth, p, 0), 0.99999);
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = u.inv_view_proj * ndc;
    return w.xyz / select(w.w, 1e-6, abs(w.w) < 1e-6);
}

// The mask id under depth pixel `q`; the mask may differ in size from the
// depth, so it is read by UV.
fn id_at(q: vec2<i32>, size: vec2<i32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(mask));
    let uv = (vec2<f32>(clamp(q, vec2<i32>(0), size - vec2<i32>(1))) + 0.5) / vec2<f32>(size);
    let m = clamp(vec2<i32>(uv * vec2<f32>(msize)), vec2<i32>(0), msize - vec2<i32>(1));
    return textureLoad(mask, m, 0).x;
}

// The surface normal at patch index `at`, from one neighbour on each axis:
// the one nearer the centre's view distance (#484). A central difference
// spans a step whenever one sits beside the pixel, and the normal it makes
// belongs to neither surface. Where the step is too shallow for the depth
// Sobel (a pillar's foot, with the ground just behind it), those normals
// marked a crease band 4 px wide, and its cells were dropped as texture. A
// one-sided tangent stays on the pixel's own surface, so a normal changes
// only where the surface does and the Sobel marks the two pixels astride
// it. On one surface either side gives the same plane, and a fold's own
// pixel takes one face's normal.
fn normal_at(pos: ptr<function, array<vec3<f32>, 25>>, rel: ptr<function, array<f32, 25>>, at: i32) -> vec3<f32> {
    let p = (*pos)[at];
    let r = (*rel)[at];
    let left = abs((*rel)[at - 1] - r) < abs((*rel)[at + 1] - r);
    let up = abs((*rel)[at - 5] - r) < abs((*rel)[at + 5] - r);
    // Each tangent points the same way (+x, +y in the patch) whichever
    // side it was taken from, so the cross keeps the central form's sign
    let tx = select((*pos)[at + 1] - p, p - (*pos)[at - 1], left);
    let ty = select((*pos)[at + 5] - p, p - (*pos)[at - 5], up);
    return normalize(cross(tx, ty) + vec3<f32>(1e-9));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The target is the depth's size (smaller than the output under a
    // render scale): a depth pixel is one edge pixel, so a line is one
    // pixel wide and falls in one glyph cell, not two
    let size = vec2<i32>(textureDimensions(scene_depth));
    let c = vec2<i32>(in.position.xy);
    // At the border the patch clamps, and repeated positions make a
    // degenerate normal that reads as a crease down the frame's edge.
    if c.x < 2 || c.y < 2 || c.x >= size.x - 2 || c.y >= size.y - 2 {
        return vec4<f32>(0.0);
    }

    // A 5x5 patch of world positions gives the 3x3 normals their neighbours
    // and their view distances, relative to the centre's
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
    let centre_id = id_at(c, size);

    var dgx = 0.0;
    var dgy = 0.0;
    var ngx = vec3<f32>(0.0);
    var ngy = vec3<f32>(0.0);
    var igx = 0.0;
    var igy = 0.0;
    var mean_dist = 0.0;
    var outline = false;
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            let at = (j + 1) * 5 + (i + 1);
            let k = j * 3 + i;
            let dist = rel[at];
            let n = normal_at(&pos, &rel, at);
            dgx += kx[k] * dist;
            dgy += ky[k] * dist;
            ngx += kx[k] * n;
            ngy += ky[k] * n;
            mean_dist += dist / 9.0;
            // A styled entity's outline: an id change where one side is
            // this style's, kept where this pixel is the nearer one. Two of
            // its variants meeting is an outline too, which is what
            // separates one styled prop from the next.
            let id = id_at(c + vec2<i32>(i - 1, j - 1), size);
            if id != centre_id && (is_ours(id) || is_ours(centre_id)) {
                let step = select(-1.0, 1.0, id > centre_id);
                igx += kx[k] * step;
                igy += ky[k] * step;
                if dist >= 0.999 {
                    outline = true;
                }
            }
        }
    }

    var depth_edge = length(vec2<f32>(dgx, dgy)) / u.depth_threshold;
    if mean_dist < 1.0 {
        depth_edge = 0.0;
    }
    var normal_edge = sqrt(dot(ngx, ngx) + dot(ngy, ngy)) / u.normal_threshold;
    // Normals rebuilt across a depth step are nonsense, and they marked a
    // band 4-5 px wide on both sides of every silhouette (wide enough that
    // the cells counted as texture, and the outline vanished). Creases
    // count only where the 5x5 patch is one surface; a step is a depth
    // edge, near side only.
    if !(spread < 0.5 * u.depth_threshold) {
        normal_edge = 0.0;
    }

    // A depth step is a silhouette and sets the direction; a crease only
    // where there's none. (Strongest-wins let the normal Sobel take over
    // along skylines, where the patch spans the far plane and its
    // direction can sum to nothing: the pixel was dropped, and a pillar
    // against the sky lost its left side and its top.)
    let dg = vec2<f32>(dgx, dgy);
    // The normal Sobel's direction: the component that changes most across
    // both axes, keeping its sign in each (picking from the x-gradient
    // alone turns a horizontal crease into a row of `/`)
    let ax = abs(ngx) + abs(ngy);
    let lx = select(select(ngx.z, ngx.y, ax.y >= ax.z), ngx.x, ax.x >= max(ax.y, ax.z));
    let ly = select(select(ngy.z, ngy.y, ax.y >= ax.z), ngy.x, ax.x >= max(ax.y, ax.z));
    let ng = vec2<f32>(lx, ly);
    var g = ng;
    var strength = normal_edge;
    var kind = 1.0;
    if depth_edge >= 1.0 && length(dg) > 1e-6 {
        g = dg;
        strength = depth_edge;
        kind = 2.0;
    }
    if outline && (abs(igx) + abs(igy)) > 0.5 {
        strength = max(strength, 1.0);
        g = vec2<f32>(igx, igy);
        kind = 3.0;
    }
    let len = length(g);
    // `!(x >= 1)` also turns away a NaN that got through
    if !(strength >= 1.0) || !(len > 1e-6) {
        return vec4<f32>(0.0);
    }
    let d = g / len;
    return vec4<f32>(d.x * d.x, d.x * d.y, d.y * d.y, kind);
}
