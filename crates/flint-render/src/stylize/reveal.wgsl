// A whole-frame style's reveal (#406): a screen-space region, grown by one
// number from nothing (0) to the whole frame (1), inside which the style
// shows and outside which the frame is what the style read. Along the
// boundary a drawn line in ink. `tear_mask.wgsl` is prepended for `hash2`,
// `value_noise` and `fbm`, the noise the render-mode tears use.
//
// So a tear reads as torn paper (#876), two more things can sit on the
// edge: a rim of bare paper on the style's side, ragged as a torn sheet's
// white core is, and a soft shadow the torn edge casts on the live side,
// longer below and to the right (the light comes from the top left).
//
// Drawn after the style, over its output, with premultiplied alpha: the
// colour written is what the style read, by how much of it comes back,
// plus the ink; the alpha is how much of the style's own result it covers.
// One full-screen draw, only while a reveal is between 0 and 1.

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

struct Reveal {
    // shape (0 tear, 1 wipe, 2 iris, 3 panel), scale, ragged, threshold
    shape: vec4<f32>,
    // centre x, y (0..1, y down), wipe direction cos, sin
    place: vec4<f32>,
    // edge half-width in pixels, blend, boil step, seed
    edge: vec4<f32>,
    // ink rgb (in the target's space), unused
    ink: vec4<f32>,
    // target width, height, 1 / aspect-space extent of the field, unused
    size: vec4<f32>,
    // rim rgb (in the target's space), rim width in pixels
    rim: vec4<f32>,
    // shadow strength, reach in pixels, unused, unused
    shadow: vec4<f32>,
};

@group(1) @binding(0) var<uniform> reveal: Reveal;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

// Noise centred on 0 and inside -1..1: big lobes from the fbm, then teeth
// and grit from finer value noise folded at its middle (ridged), so the
// edge has the cusps of torn paper rather than the waves of a cut. The
// boil steps the teeth and grit (the drawn edge is redrawn each step); the
// lobes stay where they are.
fn ragged_noise(q: vec2<f32>, scale: f32, seed: vec2<f32>, boil: vec2<f32>) -> f32 {
    let lobes = fbm(q * scale + seed) - 0.44;
    let teeth = abs(value_noise(q * scale * 6.0 + seed * 3.1 + boil) * 2.0 - 1.0) - 0.5;
    let grit = abs(value_noise(q * scale * 23.0 + seed * 5.7 + boil * 1.3) * 2.0 - 1.0) - 0.5;
    // |lobes| <= 0.44, |teeth|, |grit| <= 0.5:
    // 0.74 * 0.44 + 0.2 * 0.5 + 0.06 * 0.5 = 0.4556
    return (0.74 * lobes + 0.2 * teeth + 0.06 * grit) / 0.4556;
}

// 0 where the reveal starts, 1 at the last pixel it reaches, before the
// ragged edge moves it. `q` is aspect space (x scaled by width / height).
fn base_field(uv: vec2<f32>, aspect: f32) -> f32 {
    let c = reveal.place.xy;
    let q = (uv - c) * vec2<f32>(aspect, 1.0);
    let shape = u32(reveal.shape.x + 0.5);
    if (shape == 1u) {
        // Wipe: how far along the direction of travel, 0 at the frame's
        // trailing corner and 1 at its leading one
        let dir = reveal.place.zw;
        let half = vec2<f32>(0.5 * aspect, 0.5);
        let reach = abs(dir.x) * half.x + abs(dir.y) * half.y;
        let along = dot((uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0), dir);
        return (along + reach) / (2.0 * reach);
    }
    // The farthest corner from the centre, per axis
    let far = vec2<f32>(max(c.x, 1.0 - c.x) * aspect, max(c.y, 1.0 - c.y));
    if (shape == 3u) {
        // Panel: a rectangle the frame's shape, from the centre to every edge
        return max(abs(q.x) / far.x, abs(q.y) / far.y);
    }
    // Tear and iris: distance from the centre, 1 at the farthest corner
    return length(q) / length(far);
}

// The field at `uv`, the ragged edge's noise added: the edge is where it
// crosses the threshold
fn field(uv: vec2<f32>, aspect: f32, seed: vec2<f32>, boil: vec2<f32>) -> f32 {
    let shape = u32(reveal.shape.x + 0.5);
    let scale = reveal.shape.y;
    let ragged = reveal.shape.z;
    var f = base_field(uv, aspect);
    if (ragged > 0.0) {
        let q = uv * vec2<f32>(aspect, 1.0);
        var n: f32;
        if (shape == 2u) {
            // Iris: a slow wobble round the rim, by angle and with no
            // teeth, so it stays a circle at any size
            let dir = normalize((uv - reveal.place.xy) * vec2<f32>(aspect, 1.0) + vec2<f32>(0.0001, 0.0));
            n = (fbm(dir * scale * 0.5 + seed + boil * 0.02) - 0.44) / 0.44;
        } else {
            n = ragged_noise(q, scale, seed, boil);
        }
        f += ragged * n;
    }
    return f;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = reveal.size.xy;
    let aspect = size.x / size.y;
    let uv = in.uv;
    let scale = reveal.shape.y;
    let seed = vec2<f32>(reveal.edge.w * 7.13, reveal.edge.w * 3.71);
    let step = reveal.edge.z;
    let boil = vec2<f32>(hash2(vec2<f32>(step, 1.7)), hash2(vec2<f32>(step, 9.1))) * 37.0;

    let f = field(uv, aspect, seed, boil);

    // How far over the threshold, in pixels: negative is inside (the
    // style), positive outside (the frame it read)
    let fw = max(fwidth(f), 1e-6);
    let px = (f - reveal.shape.w) / fw;
    let inside = 1.0 - smoothstep(-0.5, 0.5, px);

    // The line: a pen's width that swells and thins along the edge
    var ink = 0.0;
    let half = reveal.edge.x;
    if (half > 0.0) {
        let q = uv * vec2<f32>(aspect, 1.0);
        let pressure = 0.55 + 0.9 * value_noise(q * scale * 3.0 + seed + boil * 0.5);
        let w = half * pressure;
        ink = 1.0 - smoothstep(w - 0.5, w + 0.5, abs(px));
    }

    let blend = reveal.edge.y;
    // How much of what the style read comes back: all of it outside, none
    // inside at blend 1 (the table's blend still fades the style itself)
    let keep = 1.0 - blend * inside;
    let e = ink * blend;
    let read = textureSample(input_texture, input_sampler, uv);

    // Torn paper (#876). Its own branch, so a reveal without it keeps the
    // arithmetic it always had, byte for byte
    let rim_w = reveal.rim.w;
    let strength = reveal.shadow.x;
    if (rim_w > 0.0 || strength > 0.0) {
        let q = uv * vec2<f32>(aspect, 1.0);
        // The rim: bare paper from the edge in, its width measured from
        // the line's inner side so it shows as wide as it says. Wider and
        // narrower along the edge, and frayed finer still at its inner
        // side, as a torn sheet's fibres are. A pixel is rim when the
        // edge is within that width of it: a ring of points round it,
        // and one half as wide, asks whether any is outside. (`px` would
        // say so wherever the field dips toward the cut without crossing
        // it, and fleck the sheet with paper.) Only pixels inside and
        // near the edge by `px` ask, and no derivative is taken in here
        var rim = 0.0;
        let span = half + rim_w * 1.65;
        if (rim_w > 0.0 && inside > 0.001 && -px < 3.0 * span) {
            let swell = value_noise(q * scale * 11.0 + seed * 2.3 + boil * 0.7);
            let fray = value_noise(q * scale * 47.0 + seed * 4.9 + boil * 1.1) - 0.5;
            let w = half + rim_w * (0.6 + 0.8 * swell) + rim_w * 0.5 * fray;
            var near = 0.0;
            for (var k = 0; k < 12; k++) {
                // 8 round the ring at w, then 4 at half of it
                let outer = k < 8;
                let turn = select(f32(k - 8) * 1.5708 + 0.785, f32(k) * 0.7854 + 0.39, outer);
                let r = select(0.5 * w, w, outer);
                let at = uv + vec2<f32>(cos(turn), sin(turn)) * r / size;
                let fk = field(at, aspect, seed, boil);
                near = max(near, smoothstep(-1.0, 1.0, (fk - reveal.shape.w) / fw));
            }
            rim = inside * near;
        }
        // The shadow on the live side: a thin one hugging the edge, and
        // the region's drop, cast away from a light at the top left, so it
        // lies longer below and to the right of the sheet. The drop is the
        // region looked for back toward the light, a few steps out to the
        // shadow's reach, the nearer steps weighing more: each step only
        // asks which side it is on, so a tooth's shadow is the tooth's
        // shape. (`px` is a distance only near the edge; far from it the
        // field's cusps break it.) Only the live side within three
        // reaches by `px` asks, which skips most of the frame: 0.07 ms at
        // 1080p, for 145 pixels round pinholes, where `px` overstates the
        // distance, losing a faint shadow (#876). No derivative is taken
        // in here, so the branch may differ pixel to pixel
        var shade = 1.0;
        if (strength > 0.0 && inside < 0.999 && px < 3.0 * reveal.shadow.y) {
            let reach = reveal.shadow.y;
            let toward = vec2<f32>(-0.6, -0.8) * reach / size;
            var fall = 0.0;
            var weights = 0.0;
            for (var k = 1; k <= 5; k++) {
                let t = f32(k) / 5.0;
                let fk = field(uv + toward * t, aspect, seed, boil);
                let w = 1.6 - t;
                fall += w * (1.0 - smoothstep(-1.5, 1.5, (fk - reveal.shape.w) / fw));
                weights += w;
            }
            let hug = 1.0 - smoothstep(0.0, 0.3 * reach, px);
            shade = 1.0 - strength * max(fall / weights, 0.6 * hug) * (1.0 - inside);
        }
        let rb = rim * blend;
        let live = (1.0 - rb) * keep;
        let rgb_t = (1.0 - e) * (live * shade * read.rgb + rb * reveal.rim.rgb) + e * reveal.ink.rgb;
        let a_t = 1.0 - (1.0 - e) * (1.0 - rb) * (1.0 - keep);
        return vec4<f32>(rgb_t, a_t);
    }

    let rgb = (1.0 - e) * keep * read.rgb + e * reveal.ink.rgb;
    // How much of the style's own result this covers: the blend unit
    // keeps (1 - a) of it
    let a = 1.0 - (1.0 - e) * (1.0 - keep);
    return vec4<f32>(rgb, a);
}
