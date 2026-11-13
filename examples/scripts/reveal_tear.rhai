// The live frame tears open into a pencil sketch (#406), and is pulled
// back. Twelve seconds, looping, on game time, so a --frame-step capture
// (or `flint render --scripts --time T`) lands on the same edge every run.
//
//   0 s  live: the scene's table has pencil at reveal 0
//   1 s  the tear: one call eases reveal 0 -> 1 over 4 s
//   5 s  all sketch
//   8 s  pulled back: a wipe, reveal 1 -> 0 over 3 s
//  12 s  live; the tear is put back and it starts again
//
// Each beat is one call, made once, on the frame the beat starts: the
// engine eases the number from there and fires post_style_tween_done at
// the end. The beat is kept on the director's `story` component, since a
// script's top level runs again before every callback.

let period = 12.0;

fn on_update() {
    // A slow drift, so the live side and the sketch both move
    let drift = total_time() * 0.05;
    set_camera_position(-1.8 + 0.8 * drift.sin(), 1.6, 8.6);
    set_camera_target(1.4, 0.4, 4.0);

    let t = total_time() % period;
    let beat = if t < 1.0 { 0 } else if t < 8.0 { 1 } else { 2 };
    let me = self_entity();
    if beat == get_field(me, "story", "beat") {
        return;
    }
    set_field(me, "story", "beat", beat);
    if beat == 1 {
        // Rip it open
        set_post_style_param("pencil", "reveal_shape", "tear");
        tween_post_style_param("pencil", "reveal", 1.0, 4.0, "ease_in_out");
    } else if beat == 2 {
        // Pulled back to life from the right
        set_post_style_param("pencil", "reveal_shape", "wipe");
        set_post_style_param("pencil", "reveal_angle", 0.0);
        tween_post_style_param("pencil", "reveal", 0.0, 3.0, "ease_in");
    }
}

fn on_event(name, data) {
    if name == "post_style_tween_done" {
        log("the " + data.style + " " + data.param + " tween is done");
    }
}
