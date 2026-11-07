// ASCII spike (#261): the glyph pass.
//
// Reads the HDR scene (after Kuwahara, when that's on), the mask and the
// edge pass, and writes an HDR image of the same size that the composite
// then tonemaps in its place.
//
// Each cell is one of two things:
//   - an edge: when enough of its 4x4 lattice lands on edge pixels, it
//     draws the line glyph (| / - \) the votes agree on, so outlines and
//     creases read as drawn lines;
//   - a tone: otherwise, a glyph from a ten-step ramp chosen by the
//     luminance of a footprint `footprint` cells wide, quantized to `levels`
//     bands, so a texture's grain settles into a few flat tones instead of
//     per-cell noise.
//
// mode 1: every pixel, one screen grid.
// mode 2: masked pixels stylized, one screen grid.
// mode 3: masked pixels stylized, each entity's grid anchored at its own
//         projected origin (the mask's gb).
// In the masked modes, pixels outside the mask are black with their edges
// drawn (background 0), or the plain scene (background 1).

struct AsciiUniforms {
    size: vec2<f32>,
    cell: f32,
    mode: u32,
    exposure: f32,
    ground: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    edge_min: f32,
    background: u32,
    edges: u32,
    edge_color: vec3<f32>,
    edge_tint: f32,
    edge_max: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> u: AsciiUniforms;
@group(1) @binding(0) var scene_tex: texture_2d<f32>;
@group(1) @binding(1) var mask_tex: texture_2d<f32>;
@group(1) @binding(2) var edge_tex: texture_2d<f32>;

// 8x8 glyphs, ordered by inked pixels (0, 4, 6, 8, 10, 12, 14, 20, 24, 29):
// " .-:+=%#*@", then the four edge glyphs | / - \.
// x holds rows 0-3 and y rows 4-7, top row in the high byte, leftmost
// column in each byte's high bit.
var<private> GLYPHS: array<vec2<u32>, 14> = array<vec2<u32>, 14>(
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
    vec2<u32>(0x18181818u, 0x18181818u), // edge '|'
    vec2<u32>(0x03060C18u, 0x3060C080u), // edge '/'
    vec2<u32>(0x000000FFu, 0xFF000000u), // edge '-'
    vec2<u32>(0xC0603018u, 0x0C060301u), // edge '\'
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
    var in_object = true;
    var id = 0.0;
    var origin = vec2<f32>(0.0);
    if masked {
        let m = textureLoad(mask_tex, p, 0);
        in_object = m.x >= 0.5;
        if in_object {
            id = m.x;
            if u.mode == 3u {
                origin = m.yz;
            }
        } else if u.background == 1u {
            return scene;
        }
    }

    let cs = u.cell;
    let rel = in.pos.xy - origin;
    let cell = floor(rel / cs);
    let cell_min = cell * cs + origin;
    let local = rel - cell * cs;
    let gx = min(u32(local.x * 8.0 / cs), 7u);
    let gy = min(u32(local.y * 8.0 / cs), 7u);
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);

    // Edge votes over the cell's own 4x4 lattice
    var votes = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var edge_count = 0.0;
    if u.edges == 1u {
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let q = cell_min + (vec2<f32>(f32(i), f32(j)) + 0.5) * (cs / 4.0);
                let e = textureLoad(edge_tex, clamp(vec2<i32>(q), vec2<i32>(0), max_px), 0);
                if e.r > 0.5 {
                    let bin = u32(floor(fract(e.g + 0.125) * 4.0)) & 3u;
                    votes[bin] += 1.0;
                    edge_count += 1.0;
                }
            }
        }
    }

    // Tone: the mean colour of a footprint `footprint` cells wide, centred on
    // the cell, over the cell's own entity in the masked modes.
    var col = vec3<f32>(0.0);
    if in_object {
        let foot = cs * max(u.footprint, 1.0);
        let centre = cell_min + 0.5 * cs;
        var sum = vec3<f32>(0.0);
        var n = 0.0;
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let q = centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot;
                let qi = clamp(vec2<i32>(q), vec2<i32>(0), max_px);
                if masked && abs(textureLoad(mask_tex, qi, 0).x - id) > 0.5 {
                    continue;
                }
                sum += textureLoad(scene_tex, qi, 0).rgb;
                n += 1.0;
            }
        }
        col = scene.rgb;
        if n > 0.0 {
            col = sum / n;
        }
    }
    let hue = col / max(max(col.r, max(col.g, col.b)), 1e-4);

    // A cell crowded with edge pixels is texture-like geometry (grass,
    // foliage, a fence seen far off), not an outline: give it tone.
    if edge_count >= u.edge_min && edge_count <= u.edge_max {
        var best = 0u;
        for (var b = 1u; b < 4u; b++) {
            if votes[b] > votes[best] {
                best = b;
            }
        }
        // Gradient angle bins 0, 45, 90, 135 degrees draw the line across
        // them: | / - \ (screen y runs down).
        let bit = glyph_bit(10u + best, gx, gy);
        var ink = u.edge_color;
        if in_object {
            ink = mix(u.edge_color, hue, u.edge_tint);
        }
        var ground = vec3<f32>(0.0);
        if in_object {
            ground = col * u.ground;
        }
        return vec4<f32>(mix(ground, ink, bit), scene.a);
    }

    if !in_object {
        return vec4<f32>(0.0, 0.0, 0.0, scene.a);
    }

    let lum = dot(col * u.exposure, vec3<f32>(0.2126, 0.7152, 0.0722));
    var t = sqrt(lum / (1.0 + lum));
    var g = min(u32(t * 10.0), 9u);
    if u.levels >= 1.0 {
        t = round(t * u.levels) / u.levels;
        g = min(u32(t * 9.0 + 0.5), 9u);
    }
    let bit = glyph_bit(g, gx, gy);

    // Density carries brightness and the ink carries hue: the cell's colour
    // at full value, dimmed only partly with the ramp, so a dark object's
    // sparse glyphs still read as its colour rather than as black.
    let ink = hue * (0.35 + t) * u.ink;
    let ground = col * u.ground;
    return vec4<f32>(mix(ground, ink, bit), scene.a);
}
