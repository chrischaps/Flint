// Beat 08, pencil (#273). An arc round the still life; at 5 s the scene's
// pencil table goes from the props to the whole frame. Boil steps the
// strokes' seed by post time, which the player advances on the game clock,
// so the same strokes land on the same frames every run.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.75, -0.2, 4.6, 1.55, film::lerp(1.25, 0.7, film::ease(t / 10.0)));
    if t >= 5.0 {
        set_scene_style_param("pencil", "scope", "screen");
    }
}
