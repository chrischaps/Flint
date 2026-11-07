// Palette matching (#320), shared by the ASCII glyph pass and the test
// that holds it to `Palette::pick` (palette.rs). The includer defines
//   fn palette_ink(i: u32) -> vec3<f32>      the ink, display values
//   fn palette_ink_lab(i: u32) -> vec3<f32>  the same ink in OKLab
// (OKLab precomputed on the CPU, once a frame, by `Palette::lab_colors`).
//
// Inks are matched in OKLab, where distance is how different two colours
// look: plain RGB distance sends a full red to CGA's dim #aa0000 rather
// than #ff5555, and a muted colour to the nearest grey. `chroma` scales the
// colour's a/b before matching, so a muted scene still reaches the inks,
// and `dither` mixes the two nearest inks by a 4x4 Bayer pattern on the
// cell grid, so a gradient steps through inks instead of banding.

// Display (sRGB-encoded) 0..1 to OKLab (Björn Ottosson, 2020).
fn pal_oklab(display: vec3<f32>) -> vec3<f32> {
    let c = clamp(display, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    let rgb = select(hi, lo, c <= vec3<f32>(0.04045));
    let lms = vec3<f32>(
        0.4122214708 * rgb.r + 0.5363325363 * rgb.g + 0.0514459929 * rgb.b,
        0.2119034982 * rgb.r + 0.6806995451 * rgb.g + 0.1073969566 * rgb.b,
        0.0883024619 * rgb.r + 0.2817188376 * rgb.g + 0.6299787005 * rgb.b,
    );
    let q = pow(max(lms, vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
    return vec3<f32>(
        0.2104542553 * q.x + 0.7936177850 * q.y - 0.0040720468 * q.z,
        1.9779984951 * q.x - 2.4285922050 * q.y + 0.4505937099 * q.z,
        0.0259040371 * q.x + 0.7827717662 * q.y - 0.8086757660 * q.z,
    );
}

// A 4x4 Bayer threshold in (0, 1), fixed to the cell grid.
fn pal_bayer(cell: vec2<i32>) -> f32 {
    let m = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let i = vec2<u32>(cell & vec2<i32>(3));
    return (f32(m[i.y * 4u + i.x]) + 0.5) / 16.0;
}

// The ink of the `len` inks at `start` for `color` (display values), its
// chroma scaled by `chroma`, dithered by `dither` (0..1) in cell `cell`.
fn pal_pick(start: u32, len: u32, color: vec3<f32>, chroma: f32, dither: f32, cell: vec2<i32>) -> vec3<f32> {
    let n = max(len, 1u);
    var lab = pal_oklab(color);
    lab = vec3<f32>(lab.x, lab.yz * chroma);
    var best = 0u;
    var second = 0u;
    var best_d = 1e9;
    var second_d = 1e9;
    for (var i = 0u; i < n; i++) {
        let e = palette_ink_lab(start + i) - lab;
        let d = dot(e, e);
        if d < best_d {
            second = best;
            second_d = best_d;
            best = i;
            best_d = d;
        } else if d < second_d {
            second = i;
            second_d = d;
        }
    }
    // How far the colour sits from the nearest ink toward the next, 0 at
    // the nearest and 1/2 midway: that share of the cells take the next
    if dither > 0.0 && n > 1u {
        let d1 = sqrt(best_d);
        let share = d1 / max(d1 + sqrt(second_d), 1e-6);
        if pal_bayer(cell) < share * dither {
            return palette_ink(start + second);
        }
    }
    return palette_ink(start + best);
}
