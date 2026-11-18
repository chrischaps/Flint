// The helpers every style shares (#743): the display transfer, luminance
// and depth to world. Built-in styles put this file first in their
// `kit::wgsl!` list, and the prelude a user style is compiled with
// (`custom.rs`, ADR 0112) starts from the same text, so each is defined
// once in `stylize/`. Plain functions of their arguments: no bindings, so
// any shader can include it.

// Linear light to display values (the sRGB curve). A negative channel
// takes the linear segment, so `max` only keeps `pow` off a negative.
fn flint_to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// Display values to linear light
fn flint_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Rec. 709 luminance
fn flint_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The world point under `uv` (0..1, y down) at depth-buffer value `depth`,
// homogeneous: xyz over w. `flint_unproject` divides it.
fn flint_unproject_h(inv_view_proj: mat4x4<f32>, uv: vec2<f32>, depth: f32) -> vec4<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    return inv_view_proj * ndc;
}

// A homogeneous w safe to divide by: one f32 rounds to within 1e-6 of 0
// (the far plane, where the sky's depth of 1 lands) is held at 1e-6,
// rather than making positions at infinity and NaN edges along skylines
fn flint_safe_w(w: f32) -> f32 {
    return select(w, 1e-6, abs(w) < 1e-6);
}

// The world position the depth `depth` at `uv` shows
fn flint_unproject(inv_view_proj: mat4x4<f32>, uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let h = flint_unproject_h(inv_view_proj, uv, depth);
    return h.xyz / flint_safe_w(h.w);
}
