// Palette matching (#276, shared since #324): every style that reads a
// palette picks its inks here, so ASCII and the palette style can't drift
// apart. Exported as `palette::PALETTE_WGSL` and concatenated after the
// includer's own declarations. The includer defines
//   fn palette_ink(i: u32) -> vec3<f32>   ink `i` of its table, display values
// (WGSL lets a module-scope function call one declared after it, so this
// file depends on a function its includer provides.) `Palette::pick` in
// palette.rs is the same function on the CPU, and a GPU test holds them to
// each other.

// A 4x4 Bayer threshold in (0, 1), fixed to a grid of cells or pixels.
fn pal_bayer(cell: vec2<i32>) -> f32 {
    let m = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let i = vec2<u32>(cell & vec2<i32>(3));
    return (f32(m[i.y * 4u + i.x]) + 0.5) / 16.0;
}

// The ink of the `len` inks at `start` for `color` (display values) and
// tone `t` (0-1). `matching` 0 = nearest (plain RGB distance), 1 = a
// luminance ladder, darkest first, that `t` climbs. `threshold` is the
// ordered-dither value of the cell or pixel (`pal_bayer`) and `dither`
// (0-1) how far it reaches: a nearest colour lying `share` of the way from
// its nearest ink to the next takes the next in `share * dither` of the
// cells, and a ladder's tone moves up to half a rung either way. Dither 0
// is the plain pick.
fn pal_pick(start: u32, len: u32, matching: u32, color: vec3<f32>, t: f32, dither: f32, threshold: f32) -> vec3<f32> {
    let n = max(len, 1u);
    if matching == 1u {
        let td = clamp(t + (threshold - 0.5) * dither / f32(n), 0.0, 1.0);
        return palette_ink(start + min(u32(td * f32(n)), n - 1u));
    }
    var best = 0u;
    var second = 0u;
    var best_d = 1e9;
    var second_d = 1e9;
    for (var i = 0u; i < n; i++) {
        let e = palette_ink(start + i) - color;
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
    if dither > 0.0 && n > 1u {
        let d1 = sqrt(best_d);
        let share = d1 / max(d1 + sqrt(second_d), 1e-6);
        if threshold < share * dither {
            return palette_ink(start + second);
        }
    }
    return palette_ink(start + best);
}
