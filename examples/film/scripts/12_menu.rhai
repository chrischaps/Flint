// Beat 12, tuning by hand (#268). The camera barely drifts, leaving the
// left of the frame to the picture and the right to the menu; everything
// else is the input script's.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::dolly([3.4, 1.5, 3.0], [3.1, 1.45, 2.7], [-0.4, 0.75, -0.2], [-0.5, 0.75, -0.3],
        film::ease(t / 12.0));
}
