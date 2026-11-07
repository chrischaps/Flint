// Beat 13, many hands. Close on the table, then a long rise and pull back
// until the still life is small in the evening and the sky takes the
// frame: the film's last shot, with room above for a title.

import "film" as film;

fn on_update() {
    let t = total_time();
    let k = film::ease(t / 12.0);
    film::dolly([3.0, 1.4, 3.1], [8.0, 3.6, 8.6], [0.0, 0.75, 0.0], [-1.5, 3.4, -3.0], k);
}
