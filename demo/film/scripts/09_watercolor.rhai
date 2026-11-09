// Beat 09, watercolor (#272). A slow descent toward the table; at 5 s
// the paint lifts off the ground and sky and stays on the still life.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::dolly([4.6, 2.1, 4.4], [3.2, 1.35, 2.9], [0.0, 0.6, 0.0], [-0.2, 0.75, -0.2],
        film::ease(t / 9.0));
    if t >= 5.0 {
        set_post_style_param("watercolor", "scope", "entities");
    }
}
