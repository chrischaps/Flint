// The ACES filmic fit (Narkowicz 2015), the one curve every shader that maps
// HDR to display tonemaps with (#738): the composite, and the lit shaders'
// own path when post-processing is off. `flint frame`'s display
// (`commands/frame/display.rs`) mirrors it on the CPU.
fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}
