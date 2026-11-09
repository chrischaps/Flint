// A dream told in whole-frame styles (#369): the clearing is awake, then
// the world softens into paint as if falling asleep, breathes there a
// while, and wakes. Ten seconds, looping. Game time, so a --frame-step
// capture shows the same beats every run.
//
//   0 s  awake
//   2 s  falling asleep: kuwahara on, its blend 0 -> 1, the brush growing
//   5 s  the dream: the brush breathes
//   7 s  waking: blend 1 -> 0
//   9 s  awake, kuwahara off
//
// `blend` is a setting every style takes: how much of it shows over what
// it read. set_post_style_param every frame is a uniform write and a
// blend constant: no GPU allocation.

let period = 10.0;

fn smooth(x) {
    let t = if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x };
    t * t * (3.0 - 2.0 * t)
}

fn on_update() {
    // A slow drift that keeps the clearing in frame
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(1.4, 0.4, 4.0);

    let t = total_time() % period;
    let dreaming = get_field(self_entity(), "story", "beat");

    // The beat changes: switch on or off once, on the frame it starts
    let beat = if t < 2.0 || t >= 9.0 { 0 } else { 1 };
    if beat != dreaming {
        set_post_style("kuwahara", beat == 1);
        set_field(self_entity(), "story", "beat", beat);
    }

    // How deep in the dream: in over 3 s, out over 2 s
    let depth = if t < 7.0 { smooth((t - 2.0) / 3.0) } else { 1.0 - smooth((t - 7.0) / 2.0) };
    if beat == 1 {
        let breath = 0.5 + 0.5 * ((t - 5.0) * 1.6).sin();
        set_post_style_param("kuwahara", "blend", depth);
        set_post_style_param("kuwahara", "radius", 3.0 + 4.0 * depth + 1.0 * depth * breath);
        set_post_style_param("kuwahara", "anisotropy", 1.0 + 0.6 * depth);
    }
    // The fixed passes follow the dream: warmer light, a deeper vignette
    set_exposure(1.25 + 0.25 * depth);
    set_vignette(0.25 + 0.3 * depth);
    set_bloom_intensity(0.05 + 0.1 * depth);
}

fn on_event(name, data) {
    if name == "post_style_changed" {
        if data.enabled {
            log(`falling asleep: ${data.style} on`);
        } else {
            log(`awake: ${data.style} off`);
        }
    }
}
