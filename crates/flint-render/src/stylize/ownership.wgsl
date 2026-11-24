// Style ownership (#384): after a style draws, the pixels another style
// owns get the style's input back, so each pixel is one style's. A pixel
// is owned by the style its mask id's variant names (`owner`, the
// registry index; NONE for unstyled ids and opt-outs); the style that just
// drew is `instance_index`. Its own pixels and unowned ones keep what it
// drew (discarded); a foreign pixel takes the input, `blend` of the way
// (the mask's .g), through the pipeline's alpha blend, so a foreign entity
// fading out of its style (blend 0) is this style's again.

// owners[id] for every id the frame has (#740), NONE past them
@group(0) @binding(0) var<storage, read> owners: array<u32>;
@group(1) @binding(0) var input_tex: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var flint_mask_texture: texture_2d<u32>;

const NONE: u32 = 0xffffffffu;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) style: u32,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) style: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.style = style;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(input_tex));
    let px = vec2<i32>(in.position.xy);
    // The mask is the scene's size, smaller than the output at the display
    // stage under a render scale: read it by UV
    let uv = in.position.xy / size;
    let m = flint_mask_load(flint_mask_texel(uv));
    let id = m.x;
    if id == 0u || id >= arrayLength(&owners) {
        discard;
    }
    let owner = owners[id];
    if owner == NONE || owner == in.style {
        discard;
    }
    let blend = flint_mask_blend(m);
    if blend <= 0.0 {
        discard;
    }
    let c = textureLoad(input_tex, px, 0);
    return vec4<f32>(c.rgb, blend);
}

// The cut snapped to a whole-frame style's cells (#534): a display style
// drawing the whole frame in cells (ASCII, `silhouette = "cell"`) hands
// its grid over, and each cell at another style's edge goes wholly to one
// side. Of the cell's 16 lattice samples (where ASCII samples its cells),
// those another style owns are foreign; the snapping style holds the cell
// while at least `held` of the 16 aren't. A held cell's foreign pixels are
// the whole frame's (WHOLE): a style that draws the whole frame keeps what
// it drew there, one that draws only its entities gives its input back.
// A cell it doesn't hold is the foreign style's (the one with the most
// samples, of the first two met), the whole frame's pixels in it too, so
// the snapping style gives its input back on all of it. A plain pass
// (`fs_main`) is left byte for byte as it was.
struct Cells {
    // cell (xy) and origin (zw), output pixels
    grid: vec4<f32>,
    // where another style's ids can be (x0, y0, x1, y1): outside it no
    // cell reads one
    rect: vec4<f32>,
    // x: the snapping style's registry index
    style: vec4<u32>,
    // x: the least of a cell's 16 samples the snapping style must hold
    held: vec4<f32>,
}

@group(3) @binding(0) var<uniform> cells: Cells;

// A held cell's foreign pixel: the whole frame's, not the entity's
const WHOLE: u32 = 0xfffffffeu;

fn owner_of(id: u32) -> u32 {
    if id == 0u || id >= arrayLength(&owners) {
        return NONE;
    }
    return owners[id];
}

// The mask under output pixel `p`, as ASCII reads it (`mask_texel`)
fn mask_under(p: vec2<f32>, size: vec2<f32>) -> vec2<u32> {
    return flint_mask_load(flint_mask_texel_of(p, size));
}

@fragment
fn fs_cells(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    if any(p < cells.rect.xy) || any(p >= cells.rect.zw) {
        discard;
    }
    // The instance: the style that drew, and whether it covers the frame
    let me = in.style >> 1u;
    let covers = (in.style & 1u) != 0u;
    let snapper = cells.style.x;
    let size = vec2<f32>(textureDimensions(input_tex));
    let m = mask_under(p, size);
    let own = owner_of(m.x);
    let cell = cells.grid.xy;
    let cell_min = floor((p - cells.grid.zw) / cell) * cell + cells.grid.zw;
    var foreign = 0u;
    var a = NONE;
    var a_n = 0u;
    var a_blend = 1.0;
    var b = NONE;
    var b_n = 0u;
    var b_blend = 1.0;
    // No `continue` (FXC, #910)
    for (var k = 0; k < 16; k++) {
        let q = floor(cell_min + (vec2<f32>(f32(k & 3), f32(k >> 2u)) + 0.5) / 4.0 * cell);
        let s = mask_under(q, size);
        let o = owner_of(s.r);
        if o != NONE && o != snapper {
            foreign += 1u;
            if o == a {
                a_n += 1u;
            } else if o == b {
                b_n += 1u;
            } else if a == NONE {
                a = o;
                a_n = 1u;
                a_blend = f32(s.g) / 65535.0;
            } else if b == NONE {
                b = o;
                b_n = 1u;
                b_blend = f32(s.g) / 65535.0;
            }
        }
    }
    var owner = own;
    var blend = flint_mask_blend(m);
    let foreign_own = own != NONE && own != snapper;
    if f32(16u - foreign) + 1e-3 >= cells.held.x {
        // The snapping style holds the cell
        if foreign_own {
            owner = WHOLE;
        }
    } else if !foreign_own {
        // The foreign style's cell, this pixel too
        if b_n > a_n {
            a = b;
            a_blend = b_blend;
        }
        owner = a;
        blend = a_blend;
    }
    if owner == NONE || owner == me {
        discard;
    }
    if owner == WHOLE {
        if covers {
            discard;
        }
        blend = 1.0;
    }
    if blend <= 0.0 {
        discard;
    }
    let c = textureLoad(input_tex, vec2<i32>(p), 0);
    return vec4<f32>(c.rgb, blend);
}
