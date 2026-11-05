// Shallow wooze (#215): the world-space drift hand-made and imported meshes share.
//
// Each vertex moves by a smooth vector noise of where it stands in the world, so
// two meshes on the same beat are displaced by one continuous field: what touches
// stays touching, and vertices split for hard edges stay together. Each target on
// the beat is the same noise somewhere else in its domain, so holding and gliding
// between targets is exactly the blend a living prop does between two poses.
//
// Prepended to wooze_vertex.wgsl and wooze_shadow_shader.wgsl, so a shadow is
// displaced by the very same function as what casts it. Only a draw with a
// `shallow_wooze` component ever runs this: every other draw runs today's shaders.

struct ShallowWooze {
    // xyz = where in the noise's domain the target being left lives,
    // w = metres at full swing, already scaled by the field's amplitude.
    leaving: vec4<f32>,
    // xyz = where the next target lives, w = how far toward it, 0 to 1.
    arriving: vec4<f32>,
    // x = one over the wavelength in metres; the rest is unused.
    params: vec4<f32>,
};

// PCG3D (Jarzynski and Olano): integers, so the same on every GPU, where a
// fract(sin()) hash isn't. Each component comes back in -1 to 1.
fn wooze_corner(cell: vec3<i32>) -> vec3<f32> {
    var v = vec3<u32>(cell) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v) / 2147483648.0 - 1.0;
}

// Value noise with a vector at each lattice corner, quintically eased so that it
// has no creases. A mix of corners, so each component stays in -1 to 1, which is
// what lets a draw's bounds grow by exactly its amplitude.
fn wooze_noise(p: vec3<f32>) -> vec3<f32> {
    let floored = floor(p);
    let f = p - floored;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let c = vec3<i32>(floored);
    let x00 = mix(wooze_corner(c), wooze_corner(c + vec3<i32>(1, 0, 0)), u.x);
    let x10 = mix(wooze_corner(c + vec3<i32>(0, 1, 0)), wooze_corner(c + vec3<i32>(1, 1, 0)), u.x);
    let x01 = mix(wooze_corner(c + vec3<i32>(0, 0, 1)), wooze_corner(c + vec3<i32>(1, 0, 1)), u.x);
    let x11 = mix(wooze_corner(c + vec3<i32>(0, 1, 1)), wooze_corner(c + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

// How far a vertex at `world` moves this frame.
fn wooze_offset(world: vec3<f32>, wooze: ShallowWooze) -> vec3<f32> {
    let p = world * wooze.params.x;
    let from_target = wooze_noise(p + wooze.leaving.xyz);
    let to_target = wooze_noise(p + wooze.arriving.xyz);
    return mix(from_target, to_target, wooze.arriving.w) * wooze.leaving.w;
}
