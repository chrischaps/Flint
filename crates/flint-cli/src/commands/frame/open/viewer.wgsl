// The frame debugger's target viewer (#1088): one captured image, drawn
// into the pane egui gives it, at the viewer's zoom and pan, through the
// same mapping as `flint frame image` (`commands/frame/display.rs`,
// `Display::map`, which this mirrors line for line). The texture holds the
// stored values as floats (an 8-bit PNG's divided by 255), so exposure,
// channel and depth range change here, on the GPU, with no new upload.

struct View {
    // The image's top-left corner, framebuffer pixels
    origin: vec2<f32>,
    // Framebuffer pixels per texel, across and down
    scale: vec2<f32>,
    // The image's size in texels
    size: vec2<f32>,
    // How far the pixel grid shows (0: not at all)
    grid: f32,
    exposure: f32,
    lo: f32,
    hi: f32,
    max_v: f32,
    // Display::Mode: 0 tonemap, 1 linear, 2 stored, 3 depth, 4 ids, 5 log,
    // 6 signed, 7 metres
    mode: u32,
    // Channel: 0 rgb, 1 r, 2 g, 3 b, 4 a, 5 luma
    channel: u32,
    // How many channels the image stores
    channels: u32,
    // 1 when the target encodes sRGB on write (so this writes linear)
    srgb_target: u32,
    // Metres (#1176): the camera's planes, and the log scale's ends in m
    near: f32,
    far: f32,
    lo_m: f32,
    hi_m: f32,
    orthographic: u32,
};

@group(0) @binding(0) var<uniform> u: View;
@group(0) @binding(1) var image: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0),
    );
    return vec4(corners[vi], 0.0, 1.0);
}

fn aces(x: f32) -> f32 {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

fn linear_to_srgb(v: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0);
    if (x <= 0.0031308) {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn hsv(h: f32, s: f32, v: f32) -> vec3<f32> {
    let i = floor(h * 6.0);
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    switch (u32(i) % 6u) {
        case 0u: { return vec3(v, t, p); }
        case 1u: { return vec3(q, v, p); }
        case 2u: { return vec3(p, v, t); }
        case 3u: { return vec3(p, q, v); }
        case 4u: { return vec3(t, p, v); }
        default: { return vec3(v, p, q); }
    }
}

fn id_colour(id: u32) -> vec3<f32> {
    if (id == 0u) {
        return vec3(0.0);
    }
    return hsv(fract(f32(id) * 0.618034), 0.62, 0.95);
}

fn exposed(x: f32) -> f32 {
    let e = x * u.exposure;
    if (u.mode == 0u) {
        return linear_to_srgb(aces(e));
    }
    return linear_to_srgb(e);
}

// One scalar through the mode's curve (the modes that spread channels)
fn curve(x: f32) -> f32 {
    switch (u.mode) {
        case 0u, 1u: { return exposed(x); }
        case 5u: { return log(1.0 + max(x, 0.0)) / log(1.0 + max(u.max_v, 1e-6)); }
        case 6u: { return 0.5 + 0.5 * x / max(u.max_v, 1.0); }
        default: { return x; }
    }
}

// DepthCamera::metres: a stored depth (0 near, 1 far) in metres
fn metres(d: f32) -> f32 {
    if (u.orthographic == 1u) {
        return u.near + d * (u.far - u.near);
    }
    return u.near * u.far / (u.far - d * (u.far - u.near));
}

fn map_texel(p: vec4<f32>, channels: u32) -> vec3<f32> {
    if (u.mode == 4u) {
        return id_colour(u32(max(p.x, 0.0)));
    }
    if (u.mode == 7u) {
        // Metres::scaled: the log of the distance over the drawn range
        let v = p.x;
        if (v < 1.0 && v > 0.0 && u.lo_m <= u.hi_m) {
            let lo = max(u.lo_m, 0.01);
            let hi = max(u.hi_m, lo);
            let z = max(metres(v), lo);
            let t = clamp(log(z / lo) / max(log(hi / lo), 1e-6), 0.0, 1.0);
            return vec3(1.0 - 0.85 * t);
        }
        return vec3(0.0);
    }
    if (u.mode == 3u) {
        let v = p.x;
        let span = max(u.hi - u.lo, 1e-12);
        if (v < 1.0 && v > 0.0 && u.lo <= u.hi) {
            return vec3(1.0 - 0.85 * clamp((v - u.lo) / span, 0.0, 1.0));
        }
        return vec3(0.0);
    }
    if (channels == 1u) {
        return vec3(curve(p.x));
    }
    if (channels == 2u) {
        return vec3(curve(p.x), curve(p.y), 0.0);
    }
    return vec3(curve(p.x), curve(p.y), curve(p.z));
}

fn to_target(c: vec3<f32>) -> vec4<f32> {
    if (u.srgb_target == 1u) {
        return vec4(srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), 1.0);
    }
    return vec4(c, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = (pos.xy - u.origin) / u.scale;
    if (p.x < 0.0 || p.y < 0.0 || p.x >= u.size.x || p.y >= u.size.y) {
        // Off the image: a quiet checker, so the image's edge is plain
        let cell = vec2<u32>(floor(pos.xy / 12.0));
        let odd = f32((cell.x + cell.y) % 2u);
        return to_target(vec3(mix(0.075, 0.095, odd)));
    }
    let t = vec2<i32>(floor(p));
    var v = textureLoad(image, t, 0);
    var channels = u.channels;
    switch (u.channel) {
        case 1u: { v = vec4(v.x, 0.0, 0.0, 1.0); channels = 1u; }
        case 2u: { v = vec4(v.y, 0.0, 0.0, 1.0); channels = 1u; }
        case 3u: { v = vec4(v.z, 0.0, 0.0, 1.0); channels = 1u; }
        case 4u: { v = vec4(v.w, 0.0, 0.0, 1.0); channels = 1u; }
        case 5u: { v = vec4(dot(v.xyz, vec3(0.2126, 0.7152, 0.0722)), 0.0, 0.0, 1.0); channels = 1u; }
        default: {}
    }
    var c = map_texel(v, channels);
    // NaN shows black, as the PNG writer's does
    c = select(clamp(c, vec3(0.0), vec3(1.0)), vec3(0.0), c != c);
    if (u.grid > 0.0) {
        // One framebuffer pixel of line between texels once they are big
        let f = fract(p) * u.scale;
        if (f.x < 1.0 || f.y < 1.0) {
            c = mix(c, vec3(0.5), 0.35 * u.grid);
        }
    }
    return to_target(c);
}
