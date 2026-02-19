// Showcase Ambience — real-time post-process atmosphere pulsing
// Demonstrates: set_bloom_intensity(), set_radial_blur(), set_ssao_intensity()

let base_bloom = 0.06;
let in_crucible = false;

fn on_init() {
    let scene = current_scene();
    if scene == "demo/showcase_crucible.scene.toml" {
        in_crucible = true;
        base_bloom = 0.10;
    }
}

fn on_update() {
    let t = total_time();

    // Gentle bloom breathing — mimics firelight fluctuation
    let bloom_pulse = sin(t * 0.7) * 0.01 + sin(t * 1.3) * 0.005;
    set_bloom_intensity(base_bloom + bloom_pulse);

    if in_crucible {
        // Heat shimmer — periodic radial blur waves
        let shimmer_raw = sin(t * 0.3);
        let shimmer = shimmer_raw * shimmer_raw * 0.04;
        set_radial_blur(shimmer);

        // SSAO pulses slightly with forge heat
        let ssao_pulse = sin(t * 0.5) * 0.1;
        set_ssao_intensity(1.4 + ssao_pulse);
    }
}
