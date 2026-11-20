// Reality tears (#746): the five reality styles, one after another, each
// switched on and tuned by name. A 30-second loop on game time:
//
//   0-6 s    matrix: code rains through the tear's patches
//   6-12 s   blood: the world sinks into crimson, the sea's own tint pools
//   12-18 s  tron: an iris opens onto a wire world
//   18-23 s  drunk: the frame rolls and sees double
//   23-30 s  underwater: a swell buries the eye and lets it go
//
// The tear (`set_post("tear", ...)`) and every style param hold until set again, so the
// script sets what each act needs every frame and puts it back after.

let period = 30.0;

// 0 outside [a, b], up to 1 over `ease` seconds after a, down before b
fn hump(t, a, b, ease) {
    if t < a || t > b {
        return 0.0;
    }
    let s = ((t - a) / ease).min((b - t) / ease).min(1.0);
    s * s * (3.0 - 2.0 * s)
}

fn on_init() {
    // Which way each style shows: through the tear, or by its blend
    set_post_style_param("matrix", "blend_source", "tear");
    set_post_style_param("blood", "blend_source", "tear");
    set_post_style_param("tron", "blend_source", "tear");
    set_post_style_param("tron", "spacing", 1.5);
}

fn on_update() {
    let t = total_time() % period;
    let ocean = get_entity("ocean");

    // The camera rides the swell, dipping under it in the last act
    let dip = hump(t, 23.5, 29.5, 1.6);
    set_camera_position(0.0, 1.6 - 1.9 * dip, 9.0);
    set_camera_target(0.0, 0.8 - 0.6 * dip, -6.0);

    // Each tear opens to just past half the frame, so its edges show
    let matrix = hump(t, 0.0, 6.0, 1.8) * 0.62;
    let blood = hump(t, 6.0, 12.0, 1.8) * 0.7;
    let tron = hump(t, 12.0, 18.0, 1.8) * 0.5;
    let drunk = hump(t, 18.0, 23.0, 1.5);
    let under = dip;

    // One tear at a time: patches for the code and the blood, an iris
    // for the wire world
    set_post("tear_scale", 3.0);
    set_post("tear_iris", t >= 12.0 && t < 18.0);
    set_post("tear", matrix.max(blood).max(tron));

    set_post_style("matrix", matrix > 0.0);
    set_post_style("blood", blood > 0.0);
    set_post_style("tron", tron > 0.0);
    set_post_style("drunk", drunk > 0.0);
    set_post_style_param("drunk", "blend", drunk);
    set_post_style("underwater", under > 0.0);
    set_post_style_param("underwater", "blend", under);
    // The waterline climbs past the eye and falls back
    set_post_style_param("underwater", "depth", 0.6 * under - 0.1);

    // The sea's half of the blood: its own tint, whose defaults are the
    // crimson tide, pooling as it rises
    if ocean >= 0 {
        set_field(ocean, "ocean", "tint", blood / 0.7);
    }
}
