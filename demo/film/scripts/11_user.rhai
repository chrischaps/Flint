// Beat 11, styles you write (#274). Switching user styles on and off is
// the same call as the engine's own: set_post_style.
//
//   0.0  the figure and the ball projected (hologram)
//   3.6  the whole frame a risograph print
//   6.8  survey contours instead

import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(-0.2, 0.8, 0.0, 4.6, 1.45, film::lerp(0.95, 0.55, film::ease(t / 10.0)));

    let step = if t < 3.6 { 0 } else if t < 6.8 { 1 } else { 2 };
    if step != get_field(self_entity(), "film", "step") {
        set_post_style("risograph", step == 1);
        set_post_style("contour_map", step == 2);
        set_field(self_entity(), "film", "step", step);
    }
}
