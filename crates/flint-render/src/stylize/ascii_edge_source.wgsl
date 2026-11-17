// ASCII's edge pixels (#267), read from the shared edges (#743).
//
// The geometry is the shared edge pass's (`edges.wgsl`, once a frame for
// every style): depth steps on their near side and creases, each a
// strength over the scene's `edge_depth` / `edge_normal` and a direction.
// ASCII scales the strengths by its own `edge_depth` / `edge_normal` and
// adds the one source the shared pass can't know: the style mask, where
// one of this style's entities meets anything else, so a styled entity's
// outline is always drawn, whatever the thresholds. A mask edge is kept
// on its near side only, as a depth step is.
//
// The outlines are a pass of their own (ascii_edge_outline.wgsl, drawn
// only while the mask is bound), one fragment a depth pixel: asked of
// every pixel of a cell inside the cell pass, their nine mask reads ran
// one sample after another and cost more than the whole style.
//
// An edge pixel, as the cell pass (ascii_edge_cells.wgsl) sums them over
// each glyph cell and the edge debug view (#349) colours each pixel: the
// unit gradient's structure tensor (gx*gx, gx*gy, gy*gy) in screen space
// (y down), and w = the source that decided it: 0 (no edge), 1 (a
// crease), 2 (a depth step) or 3 (a styled entity's outline). Each pass
// that includes this declares its uniform `u` with this struct as its
// `src`, the edge table in group 0 (binding 1) and these textures in
// group 1.

struct EdgeSource {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    // The plain `ascii` mask id (0 = none)
    plain_id: u32,
    // The style's thresholds against the shared ones: the shared
    // `edge_depth` over the style's, and `edge_normal` likewise
    depth_scale: f32,
    normal_scale: f32,
    // 1 while the mask is bound: the outline pass ran
    outlines: u32,
    _pad: u32,
};

// Each mask id's variant slot (#336, the glyph pass's `slots`): non-zero
// for every per-entity variant's id
@group(0) @binding(1) var<storage, read> slots: array<u32>;
@group(1) @binding(0) var shared_edges: texture_2d<f32>;
@group(1) @binding(1) var mask: texture_2d<u32>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;

// A mask id drawn in this style (#266): the plain `ascii` id, or one the
// variant table gives a slot
fn is_ours(id: u32) -> bool {
    if id == 0u {
        return false;
    }
    if id == u.src.plain_id {
        return true;
    }
    return id < arrayLength(&slots) && slots[id] != 0u;
}

// The mask id under depth pixel `q`; the mask may differ in size from the
// depth, so it is read by UV.
fn id_at(q: vec2<i32>, size: vec2<i32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(mask));
    let uv = (vec2<f32>(clamp(q, vec2<i32>(0), size - vec2<i32>(1))) + 0.5) / vec2<f32>(size);
    let m = clamp(vec2<i32>(uv * vec2<f32>(msize)), vec2<i32>(0), msize - vec2<i32>(1));
    return textureLoad(mask, m, 0).x;
}

// The view distance at depth pixel `q` (the sky stopped just short of the
// far plane, as the edge pass stops it)
fn view_distance(q: vec2<i32>, size: vec2<i32>) -> f32 {
    let p = clamp(q, vec2<i32>(0), size - vec2<i32>(1));
    let d = min(textureLoad(scene_depth, p, 0), 0.99999);
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(size);
    return distance(flint_unproject(u.src.inv_view_proj, uv, d), u.src.camera_pos);
}

fn on_border(q: vec2<i32>, size: vec2<i32>) -> bool {
    return q.x < 2 || q.y < 2 || q.x >= size.x - 2 || q.y >= size.y - 2;
}

// A styled entity's outline at depth pixel `q`, as an edge pixel (w = 3),
// else 0: an id change where one side is this style's, kept where this
// pixel is the nearer one. Two of its variants meeting is an outline too,
// which is what separates one styled prop from the next.
fn outline_pixel(q: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    // At the border the mask's patch would clamp
    if on_border(q, size) {
        return vec4<f32>(0.0);
    }
    let centre_id = id_at(q, size);
    var igx = 0.0;
    var igy = 0.0;
    var outline = false;
    var centre_dist = -1.0;
    var kx = array<f32, 9>(-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0);
    var ky = array<f32, 9>(-1.0, -2.0, -1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0);
    for (var k = 0; k < 9; k++) {
        let off = vec2<i32>(k % 3 - 1, k / 3 - 1);
        let id = id_at(q + off, size);
        if id != centre_id && (is_ours(id) || is_ours(centre_id)) {
            let step = select(-1.0, 1.0, id > centre_id);
            igx += kx[k] * step;
            igy += ky[k] * step;
            if centre_dist < 0.0 {
                centre_dist = max(view_distance(q, size), 1e-4);
            }
            if view_distance(q + off, size) / centre_dist >= 0.999 {
                outline = true;
            }
        }
    }
    let g = vec2<f32>(igx, igy);
    if !outline || !((abs(igx) + abs(igy)) > 0.5) {
        return vec4<f32>(0.0);
    }
    let d = g / length(g);
    return vec4<f32>(d.x * d.x, d.x * d.y, d.y * d.y, 3.0);
}

// The edge pixel at depth pixel `q`, given the outline pass's texel there
// (0 while it didn't run): an outline wins, else the shared edges at this
// style's thresholds
fn edge_pixel(q: vec2<i32>, outline: vec4<f32>) -> vec4<f32> {
    if outline.w > 2.5 {
        return outline;
    }
    let size = vec2<i32>(textureDimensions(shared_edges));
    if on_border(q, size) {
        return vec4<f32>(0.0);
    }
    let e = textureLoad(shared_edges, q, 0);
    // A depth step is a silhouette and sets the direction; a crease only
    // where there's none (the shared pass's own rule, so at the scene's
    // thresholds the direction is the one it stored)
    var strength = e.g * u.src.normal_scale;
    var kind = 1.0;
    let depth_edge = e.r * u.src.depth_scale;
    if depth_edge >= 1.0 {
        strength = depth_edge;
        kind = 2.0;
    }
    let len = length(e.ba);
    // `!(x >= 1)` also turns away a NaN that got through
    if !(strength >= 1.0) || !(len > 1e-6) {
        return vec4<f32>(0.0);
    }
    let d = e.ba / len;
    return vec4<f32>(d.x * d.x, d.x * d.y, d.y * d.y, kind);
}
