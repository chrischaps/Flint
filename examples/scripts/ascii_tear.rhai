// Reality tearing into text (#397). The frame's tear (`[post_process]
// tear`, #746) has no look of its own: the only thing it shows is the props
// whose ascii_style (or style_mask) says `blend_source = "tear"`, which turn
// to glyphs where it passes. The patches drift on their own (the post time
// moves them); the script breathes the tear, so it opens, nearly swallows
// the clearing, and closes again. Twelve seconds, looping, on game time.
//
//   0 s     calm: no tear, every prop its own shading
//   1-5 s   holes open and spread over the props
//   5-7 s   most of the clearing is text
//   7-11 s  the tear closes; calm again by 11 s
//
// set_tear holds until it's set again, so the script sets it every frame,
// 0 included. The props' styles stay off while the frame doesn't tear.

let period = 12.0;

fn on_init() {
    // Patches a little bigger than the default 3 across the frame's height
    set_tear_shape(2.4, false);
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
    set_tear(s * s * (3.0 - 2.0 * s) * 0.8);
}
