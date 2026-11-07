// Beat 03, edges (#267). A slow push past the ball and the barrel; at
// 2.5 s the edge glyphs fade up over a second and a half, so the cut
// shows the same frame without them and with them.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::dolly([3.6, 0.8, 2.9], [3.0, 0.95, 2.1], [0.7, 0.55, -1.0], [0.5, 0.65, -1.2],
        film::ease(t / 8.0));
    set_scene_style_param("ascii", "edges", film::ramp(t, 2.5, 1.5));
}
