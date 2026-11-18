// Blood (#746; render mode 2's grade of the sky and the land): a
// luminance-keeping crimson tint, weighted away from pixels that are
// ALREADY red, so a tinted sea's pools and pale-pink foam (the ocean's own
// `tint`, which a game drives with it) keep their structure while the rest
// of the world sinks into key.
//
// p0 = the tint's rgb (times the pixel's luma), strength
// p1 = the shadow's rgb (what black goes to), the luma where the tint is full
// p2 = how strongly red pixels are spared (0 = not at all)

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let mapped = picture(in.uv);
    let l = flint_luma(mapped);
    let tinted = mix(r.p1.rgb, l * r.p0.rgb, smoothstep(0.0, max(r.p1.w, 1e-4), l));
    let redness = clamp((mapped.r - max(mapped.g, mapped.b)) * r.p2.x, 0.0, 1.0);
    return shown(mix(mapped, tinted, r.p0.w * (1.0 - redness)), in.uv);
}
