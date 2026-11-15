// Cyanotype, pass 2: the print. The sharp light at this pixel and the
// spread dose from pass 1 go through the iron's tone curve, then the coat,
// the paper's fibres and the brushed border. Display values in and out.

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = flint_hash(i);
    let b = flint_hash(i + vec2<f32>(1.0, 0.0));
    let c = flint_hash(i + vec2<f32>(0.0, 1.0));
    let d = flint_hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0u; i < 4u; i = i + 1u) {
        total = total + amp * noise(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        amp = amp * 0.5;
    }
    return total;
}

// Prussian blue at full dose, through the paler blues, to the paper
const DEEP: vec3<f32> = vec3<f32>(0.035, 0.105, 0.265);
const BLUE: vec3<f32> = vec3<f32>(0.090, 0.270, 0.520);
const PALE: vec3<f32> = vec3<f32>(0.520, 0.680, 0.800);
const PAPER: vec3<f32> = vec3<f32>(0.955, 0.940, 0.890);
// The paper past the coating: a touch warmer, unwashed
const BARE: vec3<f32> = vec3<f32>(0.940, 0.915, 0.850);

// 0 = untouched paper, 1 = the deepest blue the iron gives
fn tone(blue: f32) -> vec3<f32> {
    let t = clamp(blue, 0.0, 1.0);
    let paler = mix(PAPER, PALE, smoothstep(0.0, 0.35, t));
    let mid = mix(PALE, BLUE, smoothstep(0.35, 0.7, t));
    let deep = mix(BLUE, DEEP, smoothstep(0.7, 1.0, t));
    return select(select(deep, mid, t < 0.7), paler, t < 0.35);
}

fn style(p: StylePixel) -> vec4<f32> {
    let original = clamp(p.color.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let sharp = flint_luma(original);
    let spread = flint_target_dose(p.uv).r;

    // Light finds its way under edges: the brighter of the sharp and the
    // spread dose, by how much it creeps
    let light = mix(sharp, max(sharp, spread), params.bleed);
    // The iron's response has a toe and a shoulder: little light leaves the
    // paper white, and past a point it can't get bluer. A longer exposure
    // prints the same light deeper.
    let reached = smoothstep(0.22, 0.86, pow(clamp(light, 0.0, 1.0), 1.0 / params.exposure));
    var blue = select(1.0 - reached, reached, params.process == FLINT_PROCESS_PHOTOGRAM);

    // The coat is never even: the brush laid more iron in places
    let aspect = flint.resolution.x / flint.resolution.y;
    let q = vec2<f32>(p.uv.x * aspect, p.uv.y);
    blue = blue * (0.86 + 0.24 * fbm(q * 3.1 + vec2<f32>(4.0, 1.0)));

    // Paper fibres: long, thin, mostly across the sheet, paper showing
    // through the blue where they lie; and the paper's tooth, a point wide
    let fibre = noise(q * vec2<f32>(46.0, 520.0)) * noise(q * vec2<f32>(210.0, 31.0));
    let tooth = flint_hash(floor(p.pos / max(flint.pixels_per_point, 1.0)));
    blue = blue - params.grain * (0.16 * fibre + 0.05 * (tooth - 0.5));

    var color = tone(blue);

    // The brushed border: the coat stops short of each edge by `coating`
    // of the sheet's height, unevenly, in bristle streaks
    let edge_x = min(q.x, aspect - q.x);
    let edge_y = min(q.y, 1.0 - q.y);
    let wander_x = fbm(vec2<f32>(q.y * 7.0, 3.0 + step(aspect * 0.5, q.x) * 11.0));
    let wander_y = fbm(vec2<f32>(q.x * 7.0, 23.0 + step(0.5, q.y) * 11.0));
    let reach_x = params.coating * (0.6 + 0.75 * wander_x + 0.1 * noise(vec2<f32>(q.y * 70.0, 5.0)));
    let reach_y = params.coating * (0.6 + 0.75 * wander_y + 0.1 * noise(vec2<f32>(q.x * 70.0, 7.0)));
    // The coat thins toward its end before it stops
    let band = params.coating * 0.3;
    let inside = smoothstep(reach_x - band, reach_x + band, edge_x)
        * smoothstep(reach_y - band, reach_y + band, edge_y);
    // Where the bristles ran dry, a faint wash on the bare paper, in
    // strokes along the edge it belongs to
    let along = select(q.x, q.y, edge_x < edge_y);
    let across = select(edge_y, edge_x, edge_x < edge_y);
    let dry = fbm(vec2<f32>(along * 34.0, across * 5.0)) * 0.12 * (1.0 - inside)
        * smoothstep(reach_x * 0.3, reach_x, min(edge_x, edge_y) + band);
    color = mix(mix(BARE, PALE, dry), color, select(1.0, inside, params.coating > 0.0));

    return vec4<f32>(mix(original, color, params.strength), p.color.a);
}
