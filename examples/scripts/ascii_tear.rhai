// Reality tearing into text (#397). Render mode 6 is a tear with no look
// of its own: the only thing it shows is the props whose ascii_style (or
// style_mask) says `blend_source = "tear"`, which turn to glyphs where it
// passes. The patches drift on their own (the post time moves them); the
// script breathes the mix, so the tear opens, nearly swallows the
// clearing, and closes again. Twelve seconds, looping, on game time.
//
//   0 s     calm: no tear, every prop its own shading
//   1-5 s   holes open and spread over the props
//   5-7 s   most of the clearing is text
//   7-11 s  the tear closes; calm again by 11 s
//
// A render mode shows only the frames it's called in, so it's called
// every frame. The props' styles stay off while it isn't.

let period = 12.0;

fn on_init() {
    // Patches about a third of the screen across (scale 3), fbm style (0)
    set_render_mode_params(2.4, 0.0, 6.0, 0.0);
}

fn on_update() {
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(0.6, 0.6, 3.6);

    let t = total_time() % period;
    // Up from 1 s, held about 6 s, down to 0 by 11 s: a smooth hump
    let rise = ((t - 1.0) / 4.0).max(0.0).min(1.0);
    let fall = ((11.0 - t) / 4.0).max(0.0).min(1.0);
    let s = rise.min(fall);
    let mix = s * s * (3.0 - 2.0 * s) * 0.8;
    if mix > 0.0 {
        set_render_mode(6, mix);
    }
}
