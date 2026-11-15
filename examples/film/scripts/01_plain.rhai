// Beat 01, the clearing (#435): a slow push from wide to the table, no
// style on anything. The baseline every later beat redraws.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::dolly([5.4, 2.2, 7.6], [3.1, 1.35, 4.1], [0.0, 0.8, 0.0], [-0.1, 0.75, 0.0],
        film::ease(t / 6.0));
}
