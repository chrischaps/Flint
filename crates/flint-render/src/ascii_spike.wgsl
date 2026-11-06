// ASCII spike (#261): the glyph pass.
//
// Reads the HDR scene (after Kuwahara, when that's on) and writes an HDR
// image of the same size that the composite then tonemaps in its place.
// Each cell takes the mean colour of a 4x4 lattice inside it, picks a glyph
// from a ten-step ramp by that colour's (Reinhard-compressed) luminance and
// draws it in the cell's colour over a dark ground of the same hue.
//
// mode 1: every pixel, one screen grid.
// mode 2: only masked pixels, one screen grid.
// mode 3: only masked pixels, each entity's grid anchored at its own
//         projected origin (the mask's gb).
// In the masked modes a cell averages only the lattice points of its own
// entity, so a cell cut by a silhouette keeps the object's colour.

struct AsciiUniforms {
    size: vec2<f32>,
    cell: f32,
    mode: u32,
    exposure: f32,
    ground: f32,
    ink: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: AsciiUniforms;
@group(1) @binding(0) var scene_tex: texture_2d<f32>;
@group(1) @binding(1) var mask_tex: texture_2d<f32>;

// 8x8 glyphs, ordered by inked pixels (0, 4, 6, 8, 10, 12, 14, 20, 24, 29):
// " .-:+=%#*@". x holds rows 0-3 and y rows 4-7, top row in the high byte,
// leftmost column in each byte's high bit.
var<private> GLYPHS: array<vec2<u32>, 10> = array<vec2<u32>, 10>(
    vec2<u32>(0x00000000u, 0x00000000u), // ' '
    vec2<u32>(0x00000000u, 0x00181800u), // '.'
    vec2<u32>(0x0000007Eu, 0x00000000u), // '-'
    vec2<u32>(0x00181800u, 0x00181800u), // ':'
    vec2<u32>(0x0018187Eu, 0x18180000u), // '+'
    vec2<u32>(0x00007E00u, 0x007E0000u), // '='
    vec2<u32>(0x00626408u, 0x10264600u), // '%'
    vec2<u32>(0x00247E24u, 0x247E2400u), // '#'
    vec2<u32>(0x00663CFFu, 0x3C660000u), // '*'
    vec2<u32>(0x3C666E6Au, 0x6E603E00u), // '@'
);

fn glyph_bit(g: u32, x: u32, y: u32) -> f32 {
    let rows = GLYPHS[g];
    var word = rows.x;
    if y >= 4u {
        word = rows.y;
    }
    let byte = (word >> (8u * (3u - (y & 3u)))) & 0xFFu;
    return f32((byte >> (7u - x)) & 1u);
}

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let scene = textureLoad(scene_tex, p, 0);
    let masked = u.mode >= 2u;
    var id = 0.0;
    var origin = vec2<f32>(0.0);
    if masked {
        let m = textureLoad(mask_tex, p, 0);
        if m.x < 0.5 {
            return scene;
        }
        id = m.x;
        if u.mode == 3u {
            origin = m.yz;
        }
    }

    let cs = u.cell;
    let rel = in.pos.xy - origin;
    let cell = floor(rel / cs);
    let cell_min = cell * cs + origin;
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);

    var sum = vec3<f32>(0.0);
    var n = 0.0;
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let q = cell_min + (vec2<f32>(f32(i), f32(j)) + 0.5) * (cs / 4.0);
            let qi = clamp(vec2<i32>(q), vec2<i32>(0), max_px);
            if masked && abs(textureLoad(mask_tex, qi, 0).x - id) > 0.5 {
                continue;
            }
            sum += textureLoad(scene_tex, qi, 0).rgb;
            n += 1.0;
        }
    }
    var col = scene.rgb;
    if n > 0.0 {
        col = sum / n;
    }

    let lum = dot(col * u.exposure, vec3<f32>(0.2126, 0.7152, 0.0722));
    let t = sqrt(lum / (1.0 + lum));
    let g = min(u32(t * 10.0), 9u);

    let local = rel - cell * cs;
    let gx = min(u32(local.x * 8.0 / cs), 7u);
    let gy = min(u32(local.y * 8.0 / cs), 7u);
    let bit = glyph_bit(g, gx, gy);

    // Density carries brightness and the ink carries hue: the cell's colour
    // at full value, dimmed only partly with the ramp, so a dark object's
    // sparse glyphs still read as its colour rather than as black.
    let hue = col / max(max(col.r, max(col.g, col.b)), 1e-4);
    let ink = hue * (0.35 + t) * u.ink;
    let ground = col * u.ground;
    return vec4<f32>(mix(ground, ink, bit), scene.a);
}
