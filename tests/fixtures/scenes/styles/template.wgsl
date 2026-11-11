// The template for a user-authored style (ADR 0112, #275): a split tone.
// Shadows lean toward one colour, highlights toward another, and distance
// fades into the shadow colour, the way a printed photograph's toner sits.
//
// This file holds one function, `style`, called once for each pixel. The
// engine writes everything around it (you never declare a binding):
//
//   p.uv     where the pixel is, 0..1 from the top left
//   p.pos    where the pixel is, in pixels
//   p.color  the picture at this pixel. At the display stage it is a
//            display colour (what the eye sees, 0..1) on any target
//            format; at the hdr stage, linear light that may exceed 1.
//
//   params.<name>    each [params.<name>] of the .shader.toml
//   flint.resolution, flint.texel (1 / resolution), flint.time (seconds),
//   flint.frame, flint.near, flint.far, flint.pixels_per_point,
//   flint.camera_pos
//
//   flint_sample(uv)       the picture anywhere (neighbours, offsets)
//   flint_luma(rgb)        perceived brightness
//   flint_hash(v)          a stable 0..1 noise for a vec2
//   flint_view_depth(uv)   metres from the camera ("depth" input)
//   flint_world_pos(uv)    the world position under a pixel ("depth")
//   flint_mask_id(uv), flint_style_blend(uv)   ("mask" input)
//
// Return the colour to draw. A mistake here never crashes the game: the
// style draws its last good build (or the picture unchanged), and the error
// shows in the corner of the window with its line in this file.
//
// Loops are allowed. Flint guards each one natively (#411): a run of a loop
// stops after 4096 turns, so a loop that never ends draws a wrong pixel
// instead of hanging the GPU. Keep bounds constant or a clamped param
// anyway: the web doesn't guard them yet.

fn tone(c: vec3<f32>) -> vec3<f32> {
    let shadow = vec3<f32>(params.shadow_r, params.shadow_g, params.shadow_b);
    let light = vec3<f32>(params.light_r, params.light_g, params.light_b);
    let l = flint_luma(c);
    // 0 in the shadows, 1 in the highlights, soft around `balance`
    let t = smoothstep(params.balance - 0.3, params.balance + 0.3, l);
    // Multiply-style toning keeps the picture's own light and dark
    let toned = mix(shadow * 2.0 * l, 1.0 - 2.0 * (1.0 - light) * (1.0 - l), t);
    return mix(c, clamp(toned, vec3<f32>(0.0), vec3<f32>(1.0)), params.split);
}

fn style(p: StylePixel) -> vec4<f32> {
    var c = tone(p.color.rgb);

    // Haze: past `haze_start` metres, ease toward the shadow tint. The sky
    // is at the far plane, so it takes the full haze.
    let shadow = vec3<f32>(params.shadow_r, params.shadow_g, params.shadow_b);
    let d = flint_view_depth(p.uv);
    let far = 1.0 - exp(-max(d - params.haze_start, 0.0) * 0.03);
    c = mix(c, mix(shadow, vec3<f32>(flint_luma(c)), 0.5), far * params.haze);

    return vec4<f32>(c, p.color.a);
}
