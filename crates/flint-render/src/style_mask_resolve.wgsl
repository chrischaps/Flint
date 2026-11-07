// The style mask's MSAA resolve (#306): fold the 4x mask into the
// single-sample mask the styles read. Any sample a style drew wins over
// the empty ones, so a feature thinner than a pixel that the scene's
// samples saw keeps its pixels. Where two ids share a pixel, the one with
// more samples wins, ties to the lower sample index; the blend (.g) is
// the winner's first sample's.

@group(0) @binding(0) var mask_ms: texture_multisampled_2d<u32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_resolve(@builtin(position) p: vec4<f32>) -> @location(0) vec2<u32> {
    let c = vec2<i32>(p.xy);
    var s: array<vec2<u32>, 4>;
    for (var i = 0; i < 4; i++) {
        s[i] = textureLoad(mask_ms, c, i).xy;
    }
    var best = vec2<u32>(0u, 0u);
    var best_n = 0u;
    for (var i = 0; i < 4; i++) {
        if s[i].x == 0u {
            continue;
        }
        var n = 0u;
        for (var j = 0; j < 4; j++) {
            n += select(0u, 1u, s[j].x == s[i].x);
        }
        if n > best_n {
            best = s[i];
            best_n = n;
        }
    }
    return best;
}
