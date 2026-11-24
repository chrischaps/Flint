// The fog veil's texel (#513), written once for the meshes' surface and the
// particles (#1244): a surface's weight `w` and its depth `z` (its
// fragment's `@builtin(position).z`), so the composite fogs it at its own
// depth and not at the depth of what's behind it. r = w x (1 - z), g = w,
// a = how much of what's behind it hides, "over" blended into an
// Rgba16Float target cleared to (0, 0, 0, 1) whose alpha keeps 1 - alpha of
// what's behind (`fog_veil.rs`).

// A surface that covers what's behind it by its weight
fn fog_veil_texel(w: f32, z: f32) -> vec4<f32> {
    return vec4<f32>(w * (1.0 - z), w, 0.0, w);
}

// A surface that only adds light: it weighs in where it is, and hides
// nothing behind it
fn fog_veil_light_texel(w: f32, z: f32) -> vec4<f32> {
    return vec4<f32>(w * (1.0 - z), w, 0.0, 0.0);
}
