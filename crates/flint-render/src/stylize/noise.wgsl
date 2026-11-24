// The one hash and the one value noise of the styles that boil or grain
// (#1256): pencil, contour, Kuwahara, watercolor, ASCII's dissolve, and
// the prelude a user style is compiled with (`custom/compose.rs`). Each
// used to keep its own: a PCG chain, a multiply-xorshift hash written out
// twice, another with different constants, and two fract() hashes, one of
// them on sin(). Plain functions of their arguments: no bindings, so any
// shader can include it.
//
// Integer, so it reads the same on every GPU and doesn't break down far
// from the origin the way sin() and fract() hashes do. PCG is the hash
// Jarzynski and Olano ("Hash Functions for GPU Rendering", JCGT 2020)
// recommend for quality against cost.
//
// Not here: the frame's tear noise (`../tear_mask.wgsl`'s `hash2` /
// `value_noise` / `fbm`). It is shared outside `stylize/` (the composite's
// film grain, the style mask) and its callers hash continuous, drifting
// float coordinates rather than lattice points; see that file.

// PCG's output permutation of one step (Jarzynski and Olano's `pcg`)
fn flint_pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

// A hash's top 24 bits as 0..1 (exclusive of 1)
fn flint_unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// A lattice point (x, y) under a seed, 0..1
fn flint_hash1(x: i32, y: i32, seed: u32) -> f32 {
    return flint_unit(flint_pcg(bitcast<u32>(x) ^ flint_pcg(bitcast<u32>(y) ^ flint_pcg(seed))));
}

// Three independent 0..1 values of a lattice point under a seed; .x is
// flint_hash1's
fn flint_hash3(x: i32, y: i32, seed: u32) -> vec3<f32> {
    let h = flint_pcg(bitcast<u32>(x) ^ flint_pcg(bitcast<u32>(y) ^ flint_pcg(seed)));
    let h2 = flint_pcg(h);
    return vec3<f32>(flint_unit(h), flint_unit(h2), flint_unit(flint_pcg(h2)));
}

// Value noise in 0..1: flint_hash1 at the lattice points around p, blended
// by smoothstep
fn flint_vnoise(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let s = f * f * (3.0 - 2.0 * f);
    let x = i32(i.x);
    let y = i32(i.y);
    let a = flint_hash1(x, y, seed);
    let b = flint_hash1(x + 1, y, seed);
    let c = flint_hash1(x, y + 1, seed);
    let d = flint_hash1(x + 1, y + 1, seed);
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

// Any vec2 to 0..1, by its bits: a different value for every distinct
// input, whole or not (the user prelude's `flint_hash`, ADR 0112)
fn flint_hash(v: vec2<f32>) -> f32 {
    return flint_unit(flint_pcg(bitcast<u32>(v.x) ^ flint_pcg(bitcast<u32>(v.y))));
}
