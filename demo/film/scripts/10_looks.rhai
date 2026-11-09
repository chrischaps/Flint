// Beat 10, looks (#279). One steady orbit, the same in every take, so a
// cut from one look to the next lands on the next frame of the same move.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.75, 0.0, 5.0, 1.6, film::lerp(0.9, 1.5, t / 8.0));
}
